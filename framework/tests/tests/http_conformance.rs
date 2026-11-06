//! RFC 9112 §3.2 request-target conformance, driven over real TCP sockets.
//!
//! The forms are a closed set — origin-form, absolute-form, authority-form (CONNECT only) and
//! asterisk-form (server-wide OPTIONS only) — and each has a different answer: origin-form is
//! routed as written, the absolute-form's path is routed while its authority replaces the `Host`
//! field, the asterisk names the server rather than a resource, and anything outside the set is a
//! `400` rather than a path that happens to match a route. These tests assert the answers on the
//! wire, because the parser unit tests can only show what the head became, not what the server did
//! with it.

use aether::prelude::*;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

type ServeResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TestServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<ServeResult>,
}

impl TestServer {
    async fn start(app: AetherApp) -> Self {
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
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        match tokio::time::timeout(Duration::from_secs(10), &mut self.task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => panic!("server returned an error: {error}"),
            Ok(Err(join)) => panic!("server task failed: {join}"),
            Err(_) => panic!("server did not finish graceful shutdown within 10s"),
        }
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Send a complete request and read the complete answer. Every request here asks for
/// `Connection: close`, so the read ends when the server is done with the connection.
async fn ask(addr: SocketAddr, raw: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.set_nodelay(true).expect("set TCP_NODELAY");
    stream.write_all(raw).await.expect("write request");
    stream.flush().await.expect("flush request");
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
        .await
        .expect("timed out reading the response")
        .expect("failed to read the response");

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response has no header terminator");
    let head = std::str::from_utf8(&buf[..header_end]).expect("response head is not UTF-8");
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .expect("status line has no numeric status");
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let body = buf[header_end + 4..].to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

fn close_request(request_line: &str, host: &str) -> Vec<u8> {
    format!("{request_line}\r\nHost: {host}\r\nConnection: close\r\n\r\n").into_bytes()
}

/// An absolute-form target is routed by its own path and query, exactly like the origin-form of
/// the same URI (RFC 9112 §3.2.2 requires a server to accept it).
#[tokio::test]
async fn absolute_form_routes_on_the_same_path_and_query_as_origin_form() {
    let mut app = AetherApp::new();
    app = app.get("/", |_| "root");
    app = app.get("/hello", |req| {
        format!("path={}|x={}", req.path, req.query("x").unwrap_or("<none>"))
    });
    let server = TestServer::start(app).await;

    let origin = ask(
        server.addr,
        &close_request("GET /hello?x=1 HTTP/1.1", "test"),
    )
    .await;
    assert_eq!(origin.status, 200, "the origin-form baseline must route");
    assert_eq!(origin.body_text(), "path=/hello|x=1");

    for request_line in [
        "GET http://test/hello?x=1 HTTP/1.1",
        "GET https://test/hello?x=1 HTTP/1.1",
        "GET HTTP://TEST:8080/hello?x=1 HTTP/1.1",
    ] {
        let reply = ask(server.addr, &close_request(request_line, "test")).await;
        assert_eq!(
            reply.status, 200,
            "{request_line} must reach the same route, not 404 on the absolute URI"
        );
        assert_eq!(
            reply.body_text(),
            "path=/hello|x=1",
            "the handler must see the path and query of the target, not the raw URI"
        );
    }

    // An absolute target with no path is the root resource (before this, it was 404).
    let root = ask(
        server.addr,
        &close_request("GET http://test HTTP/1.1", "test"),
    )
    .await;
    assert_eq!(root.status, 200);
    assert_eq!(root.body_text(), "root");

    server.stop().await;
}

/// RFC 9112 §3.2.2: the origin server "MUST ignore the received Host header field and instead
/// use the host information of the request-target". Every consumer has to see that authority —
/// the handler, and the allow-list below.
#[tokio::test]
async fn absolute_form_authority_replaces_the_host_field() {
    let mut app = AetherApp::new();
    app = app.get("/hello", |req| {
        format!("host={}", req.header("host").unwrap_or("<none>"))
    });
    let server = TestServer::start(app).await;

    // The field disagrees with the target: the target wins, per the RFC.
    let reply = ask(
        server.addr,
        &close_request("GET http://example.com/hello HTTP/1.1", "attacker.test"),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.body_text(),
        "host=example.com",
        "the handler must read the target's authority, not the wire Host field"
    );

    // A port in the authority is part of the value the field carries.
    let reply = ask(
        server.addr,
        &close_request("GET http://example.com:8080/hello HTTP/1.1", "example.com"),
    )
    .await;
    assert_eq!(reply.body_text(), "host=example.com:8080");

    server.stop().await;
}

/// The host allow-list decides on the value the request is addressed to. An absolute-form target
/// cannot be vouched for by a `Host` field that names something else, and a `Host` field that
/// disagrees with an allowed target cannot get the request refused.
#[tokio::test]
async fn host_validation_reads_the_absolute_form_authority() {
    let mut app = AetherApp::new();
    app = app.enable_host_validation(["good.test"]);
    app = app.get("/hello", |_| "ok");
    let server = TestServer::start(app).await;

    // Authority on the list, Host off it: the target is what was addressed, so this is served.
    let allowed = ask(
        server.addr,
        &close_request("GET http://good.test/hello HTTP/1.1", "evil.test"),
    )
    .await;
    assert_eq!(
        allowed.status, 200,
        "the allow-list must read the target's authority, not the replaced field"
    );

    // Authority off the list, Host on it: the field cannot vouch for a target it does not name.
    let refused = ask(
        server.addr,
        &close_request("GET http://evil.test/hello HTTP/1.1", "good.test"),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "an allowed Host field must not launder an off-list absolute target"
    );
    assert!(
        !refused.body_text().contains("evil.test"),
        "the refusal must not reflect the rejected authority: {:?}",
        refused.body_text()
    );

    server.stop().await;
}

/// RFC 9112 §3.2.4 and RFC 9110 §9.3.7: `OPTIONS *` asks about the server, not a resource, and
/// the answer names the methods any route dispatches to.
#[tokio::test]
async fn options_asterisk_answers_for_the_whole_server() {
    let mut app = AetherApp::new();
    app = app.get("/a", |_| "a");
    app = app.post("/a", |_| "posted");
    app = app.put("/b", |_| "b");
    let server = TestServer::start(app).await;

    let reply = ask(server.addr, &close_request("OPTIONS * HTTP/1.1", "test")).await;
    assert_eq!(
        reply.status, 204,
        "a server-wide OPTIONS is a no-content success"
    );
    assert_eq!(
        reply.header("allow"),
        Some("GET, POST, PUT, HEAD, OPTIONS"),
        "Allow must name every method a route answers, across all paths"
    );
    assert!(reply.body.is_empty());

    // A registered path still answers its own narrower Allow; the server-wide answer did not
    // replace the per-path one.
    let path_reply = ask(server.addr, &close_request("OPTIONS /a HTTP/1.1", "test")).await;
    assert_eq!(path_reply.status, 204);
    assert_eq!(path_reply.header("allow"), Some("GET, POST, HEAD, OPTIONS"));

    server.stop().await;
}

/// Targets outside the four forms are refused with `400`, never routed as if they were paths.
/// `GET hello` used to match the route `/hello`; `GET ?x=1` matched `/`.
#[tokio::test]
async fn targets_outside_the_four_forms_are_refused() {
    let mut app = AetherApp::new();
    app = app.get("/hello", |_| "hi");
    let server = TestServer::start(app).await;

    for raw in [
        // No leading slash: this is not an origin-form target, even though it names a route.
        "GET hello HTTP/1.1".to_string(),
        // A query with no path.
        "GET ?x=1 HTTP/1.1".to_string(),
        // The asterisk belongs to OPTIONS alone.
        "GET * HTTP/1.1".to_string(),
        // A fragment is never sent to a server.
        "GET /hello#frag HTTP/1.1".to_string(),
        // Absolute-form needs a non-empty authority and no userinfo.
        "GET http:///hello HTTP/1.1".to_string(),
        "GET http://user@test/hello HTTP/1.1".to_string(),
        // Only http and https name a resource this server can represent.
        "GET ftp://test/hello HTTP/1.1".to_string(),
        // authority-form belongs to CONNECT alone.
        "GET test:443 HTTP/1.1".to_string(),
        "OPTIONS test:443 HTTP/1.1".to_string(),
        // CONNECT needs an authority-form target with a port.
        "CONNECT /hello HTTP/1.1".to_string(),
        "CONNECT test HTTP/1.1".to_string(),
        "CONNECT test: HTTP/1.1".to_string(),
    ] {
        let reply = ask(server.addr, &close_request(&raw, "test")).await;
        assert_eq!(
            reply.status, 400,
            "{raw:?} is outside the four request-target forms and must be refused"
        );
    }

    server.stop().await;
}

/// An authority-form target is carried through for CONNECT (§3.2.3) — and answered by §9.3.6:
/// this framework has no tunnel support, so the refusal names the server's missing capability
/// (501) rather than the router's 404 for a target it never registered.
#[tokio::test]
async fn connect_authority_form_is_answered_not_implemented() {
    let mut app = AetherApp::new();
    app = app.get("/hello", |_| "hi");
    let server = TestServer::start(app).await;

    let reply = ask(
        server.addr,
        &close_request("CONNECT test:443 HTTP/1.1", "test"),
    )
    .await;
    assert_eq!(
        reply.status, 501,
        "a well-formed CONNECT target is refused for the missing tunnel, not reinterpreted as a path"
    );

    server.stop().await;
}

/// The `501` answers a host this server does serve. A CONNECT addressed to a host on no
/// allow-list is a misdirected request, and the security decision is made before the capability
/// answer.
#[tokio::test]
async fn host_validation_outranks_the_connect_refusal() {
    let mut app = AetherApp::new();
    app = app.enable_host_validation(["good.test"]);
    app = app.get("/hello", |_| "hi");
    let server = TestServer::start(app).await;

    let refused = ask(
        server.addr,
        &close_request("CONNECT evil.test:443 HTTP/1.1", "evil.test"),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "an off-list CONNECT is refused by the host policy, not answered as a declined tunnel"
    );

    server.stop().await;
}
