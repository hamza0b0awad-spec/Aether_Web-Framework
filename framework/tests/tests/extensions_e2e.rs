//! Request-scoped extensions, end to end over the HTTP/1.1 frontend.
//!
//! The unit tests in `aether/src/extensions.rs` cover the type-map itself. What these wire tests
//! have to prove is the part a type-map alone cannot: that a value inserted by a middleware on a
//! real connection is the *same request's* value by the time the handler runs, that nothing is
//! pre-populated for a request that nobody served, and that one connection carrying several
//! requests does not carry one request's values into the next.

use aether::SchedulingLayer;
use aether::prelude::*;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

/// A per-request value deliberately **not** `Clone`: storing it proves the map does not require
/// cloning (which would silently forbid the common case of a handler taking ownership).
struct Principal(String);

/// A second type, so a test can show two middlewares coexist rather than overwrite.
struct Tenant(u32);

fn marked_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        // Stamps only an authenticated-looking request, which is what an auth middleware really
        // does — and what makes the "no marker" case below a test of absence rather than of an
        // empty value.
        if let Some(marker) = req.header("x-marker") {
            let marker = marker.to_string();
            req.insert_extension(Principal(format!("mw:{marker}")));
        }
        next(req).await
    });
    app = app.get_async("/who", |req: RequestContext| async move {
        match req.extension::<Principal>() {
            Some(principal) => Response::text_dynamic(principal.0.clone()),
            None => Response::text_dynamic("none".to_string()),
        }
    });
    app
}

#[tokio::test]
async fn an_extension_set_by_middleware_reaches_the_handler() {
    let client = TestClient::start(marked_app()).await;
    let served = client.get("/who").header("X-Marker", "alpha").send().await;
    assert_eq!(served.status, 200);
    assert_eq!(served.text(), "mw:alpha");

    // The negative half on the same server: the middleware ran and chose not to stamp, so the
    // handler must read an absent extension.
    let unstamped = client.get("/who").send().await;
    assert_eq!(unstamped.text(), "none");
    client.shutdown().await;
}

#[tokio::test]
async fn two_middlewares_share_one_map_and_both_values_survive() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        req.insert_extension(Tenant(42));
        next(req).await
    });
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        // Reads the first middleware's value, which proves the chain passes one context along
        // rather than a copy per link.
        let tenant = req.extension::<Tenant>().map(|t| t.0).unwrap_or(0);
        req.insert_extension(Principal(format!("second:see{tenant}")));
        next(req).await
    });
    app = app.get_async("/who", |req: RequestContext| async move {
        let principal = req
            .extension::<Principal>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let tenant = req.extension::<Tenant>().map(|t| t.0).unwrap_or_default();
        Response::text_dynamic(format!("{principal}|tenant={tenant}"))
    });

    let client = TestClient::start(app).await;
    let response = client.get("/who").send().await;
    assert_eq!(response.text(), "second:see42|tenant=42");
    client.shutdown().await;
}

#[tokio::test]
async fn a_group_extension_reaches_its_routes_and_no_others() {
    let mut app = AetherApp::new();
    app = app.group("/admin", |g| {
        g.use_middleware(|mut req: RequestContext, next: Next| async move {
            req.insert_extension(Principal("admin".to_string()));
            next(req).await
        });
        g.get_async("/who", |req: RequestContext| async move {
            Response::text_dynamic(
                req.extension::<Principal>()
                    .map(|p| p.0.clone())
                    .unwrap_or("none".to_string()),
            )
        });
    });
    // Same handler, registered outside the group: the scoped middleware never runs for it, so the
    // map must be empty rather than inherited from the neighbouring route.
    app = app.get_async("/public/who", |req: RequestContext| async move {
        Response::text_dynamic(
            req.extension::<Principal>()
                .map(|p| p.0.clone())
                .unwrap_or("none".to_string()),
        )
    });

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/admin/who").send().await.text(), "admin");
    assert_eq!(client.get("/public/who").send().await.text(), "none");
    client.shutdown().await;
}

/// The isolation claim that only a shared connection can test: keep-alive reuses one task, one read
/// buffer and one connection registration for request two, so a map that was connection-scoped
/// rather than request-scoped would answer the second request with the first's principal.
#[tokio::test]
async fn extensions_do_not_leak_between_requests_on_one_connection() {
    let client = TestClient::start(marked_app()).await;
    let addr = client.addr();
    let mut tcp = TcpStream::connect(addr).await.expect("connect");

    tcp.write_all(
        b"GET /who HTTP/1.1\r\nHost: local\r\nX-Marker: first\r\nConnection: keep-alive\r\n\r\n",
    )
    .await
    .expect("write first request");
    let first = read_one_response(&mut tcp).await;
    assert!(first.contains("mw:first"), "first request: {first}");

    // Same connection, no marker at all: what the handler must see is an absent extension.
    tcp.write_all(b"GET /who HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
        .await
        .expect("write second request");
    let mut second = String::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut second))
        .await
        .expect("second request answered")
        .expect("read second response");
    assert!(
        second.contains("none") && !second.contains("mw:first"),
        "second request on the same connection saw the first one's extension: {second}"
    );
    client.shutdown().await;
}

/// Read exactly one response from a keep-alive socket, so the next read starts at the following
/// response instead of mid-frame.
async fn read_one_response(tcp: &mut TcpStream) -> String {
    let mut buffer = vec![0u8; 4096];
    let mut text = String::new();
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buffer))
            .await
            .expect("response arrived before the deadline")
            .expect("read");
        if n == 0 {
            panic!("server closed before finishing the response: {text}");
        }
        text.push_str(&String::from_utf8_lossy(&buffer[..n]));
        let Some(head_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let length = text[..head_end]
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
            .and_then(|line| line.split(':').nth(1))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or_else(|| panic!("no Content-Length in: {text}"));
        if text[head_end + 4..].len() >= length {
            return text;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The `Extension<T>` extractor: the same map, reached as a typed handler argument.
// ---------------------------------------------------------------------------------------------

/// A `Clone` per-request value, which is what the extractor form requires (Axum's `Extension<T>`
/// is gated the same way — the map holds one owner, so handing a copy onward is the only safe form).
#[derive(Clone)]
struct TenantId(u32);

fn extractor_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        if req.header("x-tenant").is_some() {
            req.insert_extension(TenantId(7));
        }
        next(req).await
    });
    // `Extension` before the context-consuming last argument: the borrowing position the parts
    // extractors are documented to occupy.
    app = app.get_async(
        "/tenant",
        |Extension(tenant): Extension<TenantId>, req: RequestContext| async move {
            Response::text_dynamic(format!(
                "tenant={}|header={}",
                tenant.0,
                req.header("x-tenant").unwrap_or("")
            ))
        },
    );
    app
}

#[tokio::test]
async fn the_extension_extractor_hands_the_middleware_value_to_the_handler() {
    let client = TestClient::start(extractor_app()).await;
    let served = client.get("/tenant").header("X-Tenant", "7").send().await;
    assert_eq!(served.status, 200);
    assert_eq!(served.text(), "tenant=7|header=7");
    client.shutdown().await;
}

#[tokio::test]
async fn extracting_an_absent_extension_is_a_500_and_the_handler_never_runs() {
    let client = TestClient::start(extractor_app()).await;
    // Absent marker → nothing in the map → the extractor rejects before the handler body exists.
    // A "tenant=…" body here would mean the handler ran anyway.
    let refused = client.get("/tenant").send().await;
    assert_eq!(refused.status, 500);
    assert!(
        refused.text().contains("no extension of this type"),
        "rejection body: {}",
        refused.text()
    );
    client.shutdown().await;
}

/// The two ends the test holds while one request is parked: a signal that it arrived, and the
/// stop that lets it leave.
type Parking = (oneshot::Sender<()>, oneshot::Receiver<()>);

/// Hands the middleware a parking brake: `arm` loads one stop into the chain, `park` (called by the
/// middleware) releases the notifier the test waits on and then waits to be let go. One request is
/// parked at a time, which is all this test needs and why the slot is taken rather than shared.
#[derive(Clone, Default)]
struct Park(Arc<tokio::sync::Mutex<Option<Parking>>>);

impl Park {
    async fn arm(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (leave_tx, leave_rx) = oneshot::channel();
        *self.0.lock().await = Some((entered_tx, leave_rx));
        (entered_rx, leave_tx)
    }

    async fn park(&self) {
        let Some((entered, leave)) = self.0.lock().await.take() else {
            return;
        };
        let _ = entered.send(());
        let _ = leave.await;
    }
}

/// A server with the scheduler on, and its layer reachable — `TestClient` hides both.
struct ScheduledServer {
    addr: SocketAddr,
    layer: Arc<SchedulingLayer>,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
}

impl ScheduledServer {
    async fn start(app: AetherApp) -> Self {
        let mut app = app;
        app = app.with_scheduling_resources(
            aether::SchedulingConfig::default(),
            aether::SchedulingResourceConfig::default(),
        );
        let layer = app.scheduling().expect("scheduling enabled");
        let server = app.bind("127.0.0.1:0").await.expect("bind ephemeral port");
        let addr = server.local_addr();
        let (shutdown, wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = wait.await;
                })
                .await
        });
        Self {
            addr,
            layer,
            shutdown: Some(shutdown),
            task,
        }
    }

    /// What the whole process has registered right now, summed across connections and jobs.
    fn registered(&self) -> u64 {
        self.layer.resources().registered_memory_bytes
    }

    async fn stop(self) {
        drop(self.shutdown);
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

/// Runs one `GET {path}`, parks the request after its middleware has attached whatever that route
/// attaches, and returns what the scheduler had registered while it was parked.
async fn registration_while_parked(server: &ScheduledServer, park: &Park, path: &str) -> u64 {
    let (entered, leave) = park.arm().await;
    let addr = server.addr;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").into_bytes();
    let reader = tokio::spawn(async move {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream.write_all(&request).await.expect("write request");
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.expect("read response");
        String::from_utf8_lossy(&got).into_owned()
    });
    entered.await.expect("the middleware parked");
    let registered = server.registered();
    leave.send(()).expect("the parked request is still there");
    let body = reader
        .await
        .expect("the request completed after being let go");
    assert!(body.contains("200 OK"), "response: {body}");
    registered
}

/// The serving path, not the type-map. The unit tests drive `Extensions` against a job the test
/// admitted by hand; only a live server can show that `dispatch` attaches the ticket at all, that
/// the price a middleware declared is what the registration gained, and that the bytes are gone
/// when the connection is. Both routes park at the same point, so the difference between the two
/// readings is the extension and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_extension_is_registered_for_the_live_request_and_released_with_it() {
    const HEAP: usize = 1024 * 1024;
    let expected = aether::scheduling::EXTENSION_ENTRY_MEMORY
        + std::mem::size_of::<Vec<u8>>() as u64
        + HEAP as u64;

    let park = Park::default();
    let chained = park.clone();
    let mut app = AetherApp::new();
    app = app.use_middleware(move |mut req: RequestContext, next: Next| {
        let chained = chained.clone();
        async move {
            if req.path == "/park/big" {
                req.insert_extension_sized(vec![7u8; HEAP], HEAP);
            }
            if req.path.starts_with("/park/") {
                chained.park().await;
            }
            next(req).await
        }
    });
    app = app.get_async("/park/plain", |_req: RequestContext| async {
        Response::text_dynamic("plain".to_string())
    });
    app = app.get_async("/park/big", |_req: RequestContext| async {
        Response::text_dynamic("big".to_string())
    });

    let server = ScheduledServer::start(app).await;
    let plain = registration_while_parked(&server, &park, "/park/plain").await;
    let big = registration_while_parked(&server, &park, "/park/big").await;
    assert_eq!(
        big - plain,
        expected,
        "the second request held the same connection and job, plus one declared megabyte"
    );

    // Released with the request: nothing from either parked run is still registered once the
    // connections have closed, so a declared payload cannot outlive the job it was charged to.
    for _ in 0..200 {
        if server.registered() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        server.registered(),
        0,
        "an extension charge outlived its request"
    );
    server.stop().await;
}

/// The other half of the serving path: a payload nobody declared is still charged what it can be —
/// the container and the inline handle — and the handler's answer is unaffected. This is the case
/// the roadmap's "values inside the map are unpriced" option would have left at zero, and the reason
/// the entry cost is charged even when nothing is declared.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_extension_is_charged_its_container_and_still_serves() {
    let park = Park::default();
    let chained = park.clone();
    let mut app = AetherApp::new();
    app = app.use_middleware(move |mut req: RequestContext, next: Next| {
        let chained = chained.clone();
        async move {
            if req.path.starts_with("/park/") {
                if req.path == "/park/small" {
                    req.insert_extension(vec![7u8; 4096]);
                }
                chained.park().await;
            }
            next(req).await
        }
    });
    app = app.get_async("/park/plain", |_req: RequestContext| async {
        Response::text_dynamic("plain".to_string())
    });
    app = app.get_async("/park/small", |req: RequestContext| async move {
        Response::text_dynamic(match req.extension::<Vec<u8>>() {
            Some(v) => format!("{} bytes held", v.len()),
            None => "none".to_string(),
        })
    });

    let server = ScheduledServer::start(app).await;
    let plain = registration_while_parked(&server, &park, "/park/plain").await;
    let small = registration_while_parked(&server, &park, "/park/small").await;
    assert_eq!(
        small - plain,
        aether::scheduling::EXTENSION_ENTRY_MEMORY + std::mem::size_of::<Vec<u8>>() as u64,
        "a Vec's 4 KiB of heap is invisible to the budget; its handle and container are not"
    );
    server.stop().await;
}
