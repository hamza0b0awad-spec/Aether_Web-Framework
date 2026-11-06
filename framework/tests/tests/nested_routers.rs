//! Nested state and mountable sub-applications, over real sockets.
//!
//! Every claim here is about what a registration *under* a scope sees: the prefix its route lands
//! behind, the order its guards run in, and which state container answers for it — the scope's own
//! first, the application's after. `RouteGroup::manage` and `AetherApp::mount` share one mechanism
//! (a chain entry that hands the request the scope's container before that scope's middleware
//! run), so a test that only checks the handler's answer would pass with the entry sitting behind
//! the group's guards, and a test that only checks one route would pass with the value leaking up
//! into the enclosing scope. Both halves are pinned per scope, on the wire.

use aether::prelude::*;
use bytes::Bytes;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Clone)]
struct Tenant(&'static str);
#[derive(Clone)]
struct Pool(usize);
#[derive(Clone)]
struct Unshadowed(&'static str);

fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "aether_nested_{}_{}_{}",
        std::process::id(),
        tag,
        counter
    ));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

/// The whole point of a sub-app is that it is written without knowing where it will be mounted,
/// so the prefix is the mount's contribution — and only the mount's.
#[tokio::test]
async fn a_mounted_router_serves_under_its_prefix_and_not_without_it() {
    let mut api = Router::new();
    api.get_async("/health", |_: RequestContext| async move { "healthy" });

    let mut app = AetherApp::new();
    app = app.mount("/api", api);
    app = app.get("/other", |_| "other");

    let client = TestClient::start(app).await;
    let mounted = client.get("/api/health").send().await;
    assert_eq!(mounted.status, 200);
    assert_eq!(mounted.text(), "healthy");

    let unmounted = client.get("/health").send().await;
    assert_eq!(
        unmounted.status, 404,
        "a router's own path must not also be reachable at the root"
    );

    let sibling = client.get("/other").send().await;
    assert_eq!(
        sibling.status, 200,
        "mounting must not disturb the app's routes"
    );
    client.shutdown().await;
}

/// A scope's value wins for the type it registered, and the app keeps answering for the types it
/// did not. Both halves are the contract: the first makes a mount mean something, the second is
/// what lets a sub-app rely on the application's database handle, logger or configuration.
#[tokio::test]
async fn a_mounted_router_state_answers_before_the_app_state() {
    let mut api = Router::new();
    api.manage(Tenant("sub"));
    api.get_async("/whoami", |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0)
            .unwrap_or("none")
            .to_string()
    });

    let mut app = AetherApp::new();
    app = app.manage(Tenant("app"));
    app = app.manage(Unshadowed("app"));
    app = app.mount("/api", api);
    app = app.get_async("/tenant", |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0)
            .unwrap_or("none")
            .to_string()
    });

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/api/whoami").send().await.text(), "sub");
    assert_eq!(
        client.get("/tenant").send().await.text(),
        "app",
        "a mount must not change what the app's own routes resolve"
    );
    client.shutdown().await;
}

/// The fall-through half of the previous rule, on its own: a scope that registers nothing of a
/// type must not answer `None` for it.
#[tokio::test]
async fn a_scope_that_does_not_register_a_type_keeps_the_app_value() {
    let mut api = Router::new();
    api.manage(Tenant("sub"));
    api.get_async("/pool", |ctx: RequestContext| async move {
        format!(
            "tenant={}|pool={}",
            ctx.state::<Tenant>().map(|t| t.0).unwrap_or("none"),
            ctx.state::<Pool>().map(|p| p.0).unwrap_or(0)
        )
    });

    let mut app = AetherApp::new();
    app = app.manage(Pool(7));
    app = app.mount("/api", api);

    let client = TestClient::start(app).await;
    assert_eq!(
        client.get("/api/pool").send().await.text(),
        "tenant=sub|pool=7",
        "the app's services must stay visible to a mounted router that never mentions them"
    );
    client.shutdown().await;
}

/// Two scopes registering the same type answer for their own routes only. This is the shape a
/// multi-tenant or multi-module server is written in, and it is also the test that fails if a
/// scope's `manage` were to write into the application's container instead of its own.
#[tokio::test]
async fn two_groups_can_register_the_same_type_and_answer_for_their_own_routes() {
    let whoami = |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0.to_string())
            .unwrap_or_else(|| "none".to_string())
    };

    let mut app = AetherApp::new();
    app = app.manage(Tenant("app"));
    app = app.group("/red", |g| {
        g.manage(Tenant("red"));
        g.get_async("/who", whoami);
    });
    app = app.group("/blue", |g| {
        g.manage(Tenant("blue"));
        g.get_async("/who", whoami);
    });
    app = app.get_async("/who", whoami);

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/red/who").send().await.text(), "red");
    assert_eq!(
        client.get("/blue/who").send().await.text(),
        "blue",
        "a sibling group must not inherit the value the other one registered"
    );
    assert_eq!(
        client.get("/who").send().await.text(),
        "app",
        "a group's value must not replace the app's for routes outside it"
    );
    client.shutdown().await;
}

/// Nesting: the innermost value wins, and the outer scope's *other* types stay visible. A merge
/// that replaced the whole container would answer `none` for `Unshadowed` under the inner group.
#[tokio::test]
async fn a_nested_group_shadows_its_parent_and_keeps_its_other_types() {
    let mut app = AetherApp::new();
    app = app.group("/outer", |outer| {
        outer.manage(Tenant("outer"));
        outer.manage(Unshadowed("outer"));
        outer.get_async("/who", |ctx: RequestContext| async move {
            format!(
                "tenant={}|unshadowed={}",
                ctx.state::<Tenant>().map(|t| t.0).unwrap_or("none"),
                ctx.state::<Unshadowed>().map(|u| u.0).unwrap_or("none")
            )
        });
        outer.group("/inner", |inner| {
            inner.manage(Tenant("inner"));
            inner.get_async("/who", |ctx: RequestContext| async move {
                format!(
                    "tenant={}|unshadowed={}",
                    ctx.state::<Tenant>().map(|t| t.0).unwrap_or("none"),
                    ctx.state::<Unshadowed>().map(|u| u.0).unwrap_or("none")
                )
            });
        });
    });

    let client = TestClient::start(app).await;
    assert_eq!(
        client.get("/outer/who").send().await.text(),
        "tenant=outer|unshadowed=outer"
    );
    assert_eq!(
        client.get("/outer/inner/who").send().await.text(),
        "tenant=inner|unshadowed=outer",
        "the inner value must shadow only the type it registered"
    );
    client.shutdown().await;
}

/// A router mounted **inside** a group inherits the group's guards and the app-wide stack, and its
/// own values shadow the group's — the same rule one level up, so a sub-app written for a group
/// and a sub-app written for the app behave alike.
#[tokio::test]
async fn a_router_mounted_in_a_group_inherits_its_guards_and_shadows_its_state() {
    let mut leaf = Router::new();
    leaf.manage(Tenant("router"));
    leaf.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Router", "1".to_string()));
        res
    });
    leaf.get_async("/leaf", |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0)
            .unwrap_or("none")
            .to_string()
    });

    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-App", "1".to_string()));
        res
    });
    app = app.group("/v1", |group| {
        group.manage(Tenant("group"));
        group.use_middleware(|req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Group", "1".to_string()));
            res
        });
        group.get_async("/plain", |ctx: RequestContext| async move {
            ctx.state::<Tenant>()
                .map(|t| t.0)
                .unwrap_or("none")
                .to_string()
        });
        group.mount("/sub", leaf);
    });

    let client = TestClient::start(app).await;
    let mounted = client.get("/v1/sub/leaf").send().await;
    assert_eq!(mounted.text(), "router");
    assert_eq!(mounted.header("x-app").unwrap(), "1");
    assert_eq!(
        mounted.header("x-group").unwrap(),
        "1",
        "a guard registered on the enclosing group must still protect a route the mount adds"
    );
    assert_eq!(mounted.header("x-router").unwrap(), "1");

    assert_eq!(client.get("/v1/plain").send().await.text(), "group");
    client.shutdown().await;
}

/// The order is the contract, and a chain built by concatenation can put it wrong in a way no
/// single-layer test sees: app-wide, then the enclosing group, then the router, then the handler.
#[tokio::test]
async fn the_chain_order_is_app_then_group_then_router_then_handler() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    let mut leaf = Router::new();
    leaf.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("router");
                next(req).await
            }
        }
    });
    leaf.get_async("/leaf", {
        let log = Arc::clone(&log);
        move |req: RequestContext| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("handler");
                let _ = req;
                "ok"
            }
        }
    });

    let mut app = AetherApp::new();
    app = app.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("app");
                next(req).await
            }
        }
    });
    app = app.group("/g", |group| {
        group.use_middleware({
            let log = Arc::clone(&log);
            move |req: RequestContext, next: Next| {
                let log = Arc::clone(&log);
                async move {
                    log.lock().unwrap().push("group");
                    next(req).await
                }
            }
        });
        group.mount("/r", leaf);
    });

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/g/r/leaf").send().await.status, 200);
    client.shutdown().await;

    assert_eq!(
        log.lock().unwrap().clone(),
        vec!["app", "group", "router", "handler"]
    );
}

/// Order inside a router is the same contract as inside a group: a value reaches the
/// registrations that follow it, never the ones already collected. Without this, a router built by
/// a function that registers routes before the caller has configured state would silently depend
/// on call order at mount time instead of build time.
#[tokio::test]
async fn a_router_value_reaches_only_the_routes_collected_after_it() {
    let whoami = |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0.to_string())
            .unwrap_or_else(|| "none".to_string())
    };

    let mut api = Router::new();
    api.get_async("/before", whoami);
    api.manage(Tenant("sub"));
    api.get_async("/after", whoami);

    let mut app = AetherApp::new();
    app = app.mount("/api", api);

    let client = TestClient::start(app).await;
    assert_eq!(
        client.get("/api/before").send().await.text(),
        "none",
        "a route collected before the value must not resolve it"
    );
    assert_eq!(client.get("/api/after").send().await.text(), "sub");
    client.shutdown().await;
}

/// `State<T>` is the extractor handlers use, and it goes through the same resolution — pinned
/// here rather than assumed, because an extractor reading the app container directly would compile
/// and pass every `ctx.state` test above.
#[tokio::test]
async fn the_state_extractor_resolves_the_scoped_value() {
    let mut api = Router::new();
    api.manage(Pool(3));
    api.get_async("/size", |State(pool): State<Pool>| async move {
        format!("pool={}", pool.0)
    });

    let mut app = AetherApp::new();
    app = app.manage(Pool(9));
    app = app.mount("/api", api);
    app = app.get_async("/size", |State(pool): State<Pool>| async move {
        format!("pool={}", pool.0)
    });

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/api/size").send().await.text(), "pool=3");
    assert_eq!(client.get("/size").send().await.text(), "pool=9");
    client.shutdown().await;
}

/// A group that registers nothing adds no layer: its routes must answer exactly as the same route
/// registered on the app, including for a type the app manages.
#[tokio::test]
async fn a_group_without_state_resolves_the_app_container_unchanged() {
    let mut app = AetherApp::new();
    app = app.manage(Pool(5));
    app = app.group("/plain", |g| {
        g.get_async("/size", |ctx: RequestContext| async move {
            format!("pool={}", ctx.state::<Pool>().map(|p| p.0).unwrap_or(0))
        });
    });

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/plain/size").send().await.text(), "pool=5");
    client.shutdown().await;
}

/// A mount at no prefix is the whole application: this is how a sub-app written in another crate
/// is served as-is, and it is the one prefix shape a concatenation rule gets wrong by adding a
/// stray segment.
#[tokio::test]
async fn an_empty_prefix_mount_serves_the_router_absolute() {
    let mut api = Router::new();
    api.manage(Tenant("sub"));
    api.get_async("/root", |ctx: RequestContext| async move {
        format!(
            "{}:root",
            ctx.state::<Tenant>().map(|t| t.0).unwrap_or("none")
        )
    });

    let mut app = AetherApp::new();
    app = app.mount("", api);
    app = app.get("/app", |_| "app-route");

    let client = TestClient::start(app).await;
    assert_eq!(client.get("/root").send().await.text(), "sub:root");
    assert_eq!(client.get("/app").send().await.text(), "app-route");
    client.shutdown().await;
}

/// `static_dir` reached through a router: the tree's own catch-all is a registration like any
/// other, so it must land under the mount prefix and carry the scope's middleware with it.
#[tokio::test]
async fn a_router_static_dir_serves_files_under_the_mount_prefix() {
    let base = temp_dir("static");
    std::fs::write(base.join("a.txt"), b"asset-a").unwrap();

    let mut api = Router::new();
    api.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Api", "1".to_string()));
        res
    });
    api.static_dir("/assets", base.clone());

    let mut app = AetherApp::new();
    app = app.mount("/files", api);

    let client = TestClient::start(app).await;
    let served = client.get("/files/assets/a.txt").send().await;
    assert_eq!(served.status, 200, "body: {}", served.text());
    assert_eq!(served.text(), "asset-a");
    assert_eq!(
        served.header("x-api").unwrap(),
        "1",
        "the router's guard runs for its static routes too"
    );
    let missed = client.get("/assets/a.txt").send().await;
    assert_eq!(missed.status, 404);
    client.shutdown().await;

    let _ = std::fs::remove_dir_all(&base);
}

/// A streaming route inside a sub-app: the route id has to reach `streaming_routes` through the
/// replay, or the frontend buffers the body before dispatch. The shape is asserted, not the bytes,
/// because a buffered registration answers the same ten bytes through `read_body`'s fallback — the
/// first version of this test did exactly that and survived the mutation it was written to catch.
#[tokio::test]
async fn a_route_stream_in_a_mounted_router_streams_and_sees_its_state() {
    let mut api = Router::new();
    api.manage(Tenant("sub"));
    api.route_stream(
        Method::Post,
        "/upload",
        |mut ctx: RequestContext| async move {
            let tenant = ctx
                .state::<Tenant>()
                .map(|t| t.0)
                .unwrap_or("none")
                .to_string();
            let lane = ctx.body_stream().is_some();
            let buffered = ctx.body.len();
            match ctx.read_body(1024 * 1024).await {
                Ok(body) => {
                    format!(
                        "tenant={tenant}|lane={lane}|buffered={buffered}|bytes={}",
                        body.len()
                    )
                }
                Err(_) => format!("tenant={tenant}|lane={lane}|error"),
            }
        },
    );

    let mut app = AetherApp::new();
    app = app.mount("/api", api);

    let client = TestClient::start(app).await;
    let res = client
        .post("/api/upload")
        .body(Bytes::from_static(b"0123456789"))
        .send()
        .await;
    assert_eq!(res.status, 200, "body: {}", res.text());
    assert_eq!(
        res.text(),
        "tenant=sub|lane=true|buffered=0|bytes=10",
        "a mounted route must still be dispatched from its head with the body in the lane"
    );
    client.shutdown().await;
}

/// A WebSocket route reached through a mount: its chain is stored per route and unfolded at serve
/// time, so the overlay entry has to survive that split — and the upgrade hands the request to the
/// socket instead of answering it, which is the one path where a chain entry could be dropped
/// without any HTTP response showing it.
#[tokio::test]
async fn a_router_websocket_route_upgrades_under_the_prefix_with_its_state() {
    #[derive(Clone)]
    struct Marker(&'static str);

    let mut api = Router::new();
    api.manage(Marker("sub"));
    api.ws("/live", |req: RequestContext, mut socket| async move {
        let seen = req
            .state::<Marker>()
            .map(|m| m.0)
            .unwrap_or("absent")
            .to_string();
        let _ = socket.send_text(&seen).await;
    });

    let mut app = AetherApp::new();
    app = app.mount("/api", api);

    let server = RawServer::start(app).await;
    let mut tcp = TcpStream::connect(server.addr).await.unwrap();
    tcp.write_all(
        b"GET /api/live HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
          Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await
    .unwrap();

    let head = tcp.read_head().await;
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "a mounted WebSocket route must still upgrade: {head}"
    );

    let mut frame = [0u8; 2];
    tcp.read_exact(&mut frame).await.unwrap();
    assert_eq!(frame[0] & 0x0f, 0x1, "expected a text frame");
    let len = (frame[1] & 0x7f) as usize;
    let mut payload = vec![0u8; len];
    tcp.read_exact(&mut payload).await.unwrap();
    assert_eq!(
        payload, b"sub",
        "the mounted router's state must reach the upgraded handler"
    );

    server.stop().await;
}

/// A server the raw-socket test above can drive: `TestClient` speaks request/response HTTP/1.1 and
/// cannot hold a socket open across a protocol upgrade.
struct RawServer {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
}

impl RawServer {
    async fn start(app: AetherApp) -> Self {
        let server = app.bind("127.0.0.1:0").await.expect("bind ephemeral port");
        let addr = server.local_addr();
        let (shutdown, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = wait.await;
                })
                .await
        });
        Self {
            addr,
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn stop(self) {
        let Self {
            mut shutdown, task, ..
        } = self;
        if let Some(sender) = shutdown.take() {
            let _ = sender.send(());
        }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(15), task).await;
    }
}

trait ReadHead {
    async fn read_head(&mut self) -> String;
}

impl ReadHead for TcpStream {
    async fn read_head(&mut self) -> String {
        let mut bytes = Vec::new();
        let mut buf = [0u8; 1];
        while !bytes.ends_with(b"\r\n\r\n") {
            let read = self.read(&mut buf).await.expect("head bytes");
            assert_ne!(read, 0, "the server closed before the head ended");
            bytes.push(buf[0]);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
