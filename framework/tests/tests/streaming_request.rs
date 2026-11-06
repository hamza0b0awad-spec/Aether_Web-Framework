//! Streamed request bodies over real sockets.
//!
//! Every claim here is about bytes that arrive *after* the handler started: that a route
//! registered with `route_stream` is dispatched from its head, that the handler pulls the body in
//! chunks it did not have to buffer first, that a `Content-Length` body and a chunked one reach
//! the handler as the same bytes, that a declared length over the streaming ceiling is refused
//! before dispatch, that an unmet `Expect` refuses the lane before its handler is entered, that a failed body closes the connection instead of leaving its tail to be
//! read as the next request, that an early answer either settles the body (small, already
//! buffered) or closes (large, still on the wire), and that a body which overgrows the length its
//! head promised ends the lane at that promise so the surplus is framed as the next request.
//!
//! What is *not* measured here: resident memory. The one-slot lane and the per-chunk registration
//! are pinned by unit tests on the lane and the pump; no test in this file samples RSS.

use aether::prelude::*;
use bytes::Bytes;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};

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

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "body is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

struct Client {
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect to test server");
        stream.set_nodelay(true).expect("set TCP_NODELAY");
        Self {
            stream,
            buffered: Vec::new(),
        }
    }

    async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("write request");
        self.stream.flush().await.expect("flush request");
    }

    /// A complete request: head first, then the body in one write.
    async fn send_request(&mut self, method: &str, path: &str, body: &[u8]) {
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        self.send(&bytes).await;
    }

    fn take_response(&mut self) -> Option<HttpResponse> {
        let header_end = find_subslice(&self.buffered, b"\r\n\r\n")?;
        let header_text = std::str::from_utf8(&self.buffered[..header_end]).ok()?;
        let mut lines = header_text.split("\r\n");
        let status = lines
            .next()?
            .split_whitespace()
            .nth(1)?
            .parse::<u16>()
            .ok()?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        let body_len = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let total = header_end + 4 + body_len;
        if self.buffered.len() < total {
            return None;
        }
        let body = self.buffered[header_end + 4..total].to_vec();
        self.buffered.drain(..total);
        Some(HttpResponse {
            status,
            headers,
            body,
        })
    }

    async fn response(&mut self) -> HttpResponse {
        loop {
            if let Some(response) = self.take_response() {
                return response;
            }
            let mut chunk = [0u8; 8192];
            let read = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
                .await
                .expect("timed out waiting for a response")
                .expect("failed to read a response");
            assert!(read > 0, "connection closed before a complete response");
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    async fn expect_closed(&mut self) {
        let mut chunk = [0u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk)).await {
                Ok(Ok(0)) | Ok(Err(_)) => return,
                Ok(Ok(_)) => continue,
                Err(_) => panic!("server kept the connection open instead of closing it"),
            }
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().enumerate().fold(0u64, |acc, (index, byte)| {
        acc.wrapping_add((*byte as u64).wrapping_mul(index as u64 + 1))
    })
}

/// One refusal shape for every handler in this file. `Response::status` takes a `&'static str`
/// body, so a dynamic message has to travel as JSON — and it must, or a failing read would answer
/// with a bare status and say nothing about which error the lane produced.
fn refuse(status: u16, text: &'static str, error: impl std::fmt::Display) -> Response {
    Response::json_status(
        status,
        text,
        Bytes::from(json!({ "error": error.to_string() }).to_string()),
    )
}

/// Encode `body` as one chunked request with the given chunk sizes.
fn chunked_request(path: &str, body: &[u8], sizes: &[usize]) -> Vec<u8> {
    let mut out =
        format!("POST {path} HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n")
            .into_bytes();
    let mut offset = 0;
    let mut index = 0;
    while offset < body.len() {
        let take = sizes[index % sizes.len()].min(body.len() - offset);
        out.extend_from_slice(format!("{take:x}\r\n").as_bytes());
        out.extend_from_slice(&body[offset..offset + take]);
        out.extend_from_slice(b"\r\n");
        offset += take;
        index += 1;
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

#[tokio::test]
async fn a_streamed_route_is_dispatched_before_its_body_arrives() {
    let entered = Arc::new(Notify::new());
    let signal = Arc::clone(&entered);
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", move |mut req: RequestContext| {
        let signal = Arc::clone(&signal);
        async move {
            // The test waits on this before sending a single body byte, so a frontend that
            // buffered the body before dispatching would deadlock here rather than pass.
            signal.notify_one();
            let buffered = req.body.len();
            let mut chunks = 0usize;
            let mut bytes = 0usize;
            let mut all = Vec::new();
            while let Some(chunk) = req.body_stream().expect("lane").next_chunk().await {
                match chunk {
                    Ok(chunk) => {
                        chunks += 1;
                        bytes += chunk.len();
                        all.extend_from_slice(&chunk);
                    }
                    Err(error) => return refuse(400, "Bad Request", error),
                }
            }
            let sum = checksum(&all);
            Response::json(Bytes::from(
                json!({ "buffered": buffered, "chunks": chunks, "bytes": bytes, "sum": sum })
                    .to_string(),
            ))
        }
    });
    app = app.get("/ping", |_| "pong");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();

    client
        .send(
            format!(
                "POST /up HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await;
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("the handler was not dispatched before the body was sent");

    client.send(&body).await;
    let response = client.response().await;
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let json = response.json();
    assert_eq!(
        json["buffered"], 0,
        "a streamed request must carry an empty body copy"
    );
    assert_eq!(json["bytes"], body.len());
    assert!(json["chunks"].as_u64().unwrap() >= 1);
    assert_eq!(json["sum"], checksum(&body));

    // The handler read the body to its end, so the framing is complete and the connection may
    // carry another request.
    client.send_request("GET", "/ping", b"").await;
    let next = client.response().await;
    assert_eq!(
        (next.status, next.body.as_slice()),
        (200, b"pong".as_slice())
    );

    server.stop().await;
}

#[tokio::test]
async fn a_chunked_body_reaches_the_handler_decoded() {
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(usize::MAX).await {
            Ok(body) => Response::json(Bytes::from(
                json!({ "bytes": body.len(), "sum": checksum(&body) }).to_string(),
            )),
            Err(error) => refuse(400, "Bad Request", error),
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let body: Vec<u8> = (0..50_000u32).map(|i| (i % 199) as u8).collect();

    client
        .send(&chunked_request("/up", &body, &[7, 1000, 16_384]))
        .await;
    let response = client.response().await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["bytes"], body.len());
    assert_eq!(json["sum"], checksum(&body));

    server.stop().await;
}

#[tokio::test]
async fn a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch() {
    let dispatched = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&dispatched);
    let mut app = AetherApp::with_config(AetherConfig {
        max_stream_body_bytes: 128,
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", move |req: RequestContext| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            format!("{}", req.body.len())
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"POST /up HTTP/1.1\r\nHost: test\r\nContent-Length: 4096\r\n\r\n")
        .await;

    let response = client.response().await;
    assert_eq!(response.status, 413);
    assert_eq!(response.header("Connection"), Some("close"));
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        0,
        "the handler must not run for a body its route already refused by length"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The streaming ceiling is the only length a streaming route refuses by declaring it. A
/// declared length above `max_body_bytes` is the upload a streaming route exists for — the config
/// says to leave `max_stream_body_bytes` well above the buffered ceiling for exactly this reason —
/// so the buffered parser's ceiling must not decide it. Before this test, the head parse refused
/// the request with `413` before the route was consulted, and a streamed route could carry past
/// the buffered ceiling only with chunked framing.
#[tokio::test]
async fn a_declared_length_over_the_buffered_ceiling_streams_to_its_route() {
    let mut app = AetherApp::with_config(AetherConfig {
        max_body_bytes: 64,
        max_stream_body_bytes: 4096,
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(usize::MAX).await {
            Ok(body) => Response::text_dynamic(format!("{} {}", body.len(), checksum(&body))),
            Err(error) => refuse(400, "Bad Request", error),
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let body: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    client.send_request("POST", "/up", &body).await;

    let response = client.response().await;
    let expected = format!("1000 {}", checksum(&body));
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, expected.as_bytes()),
        "the streaming ceiling allows this length; the buffered ceiling is not the route's"
    );
    assert!(
        response.header("Connection").is_none(),
        "a consumed body keeps the connection"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_body_past_the_ceiling_mid_stream_fails_the_read_and_closes() {
    let mut app = AetherApp::with_config(AetherConfig {
        max_stream_body_bytes: 64,
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(4096).await {
            Ok(body) => Response::text_dynamic(format!("{}", body.len())),
            Err(BodyError::TooLarge) => Response::status(413, "Payload Too Large"),
            Err(error) => refuse(400, "Bad Request", error),
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let body = vec![b'x'; 200];
    // Chunked, so no declared length can refuse it before dispatch: the ceiling has to be met
    // while the body is arriving.
    client.send(&chunked_request("/up", &body, &[10])).await;

    let response = client.response().await;
    assert_eq!(response.status, 413);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn an_unread_body_closes_the_connection_after_the_response() {
    let mut app = AetherApp::new();
    // The handler answers without touching the body, which is the case the close rule exists for.
    app = app.route_stream(Method::Post, "/ignore", |_req: RequestContext| async move {
        "done"
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let body = vec![b'y'; 64 * 1024];
    client
        .send(
            format!(
                "POST /ignore HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await;
    client.send(&body).await;

    let response = client.response().await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"done");
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn a_small_early_answered_body_keeps_the_connection_alive() {
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/ignore", |_req: RequestContext| async move {
        "done"
    });
    app = app.get("/ping", |_| "pong");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    // The whole body fits in the same read that carried the head, so the pump settles it after
    // the answer without touching the socket and the connection stays framed.
    client
        .send_request("POST", "/ignore", &vec![b'z'; 512])
        .await;

    let response = client.response().await;
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, b"done".as_slice())
    );
    assert!(
        response.header("Connection").is_none(),
        "a settled body must not close the connection"
    );

    client.send_request("GET", "/ping", b"").await;
    let next = client.response().await;
    assert_eq!(
        (next.status, next.body.as_slice()),
        (200, b"pong".as_slice())
    );

    server.stop().await;
}

#[tokio::test]
async fn a_body_that_stops_arriving_times_out_for_the_handler() {
    let mut app = AetherApp::with_config(AetherConfig {
        max_receive_time: Duration::from_millis(300),
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(1024).await {
            Ok(body) => Response::text_dynamic(format!("{}", body.len())),
            Err(BodyError::Timeout) => Response::status(408, "Request Timeout"),
            Err(error) => refuse(400, "Bad Request", error),
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    // Ten declared bytes, three sent: the rest never arrives.
    client
        .send(b"POST /up HTTP/1.1\r\nHost: test\r\nContent-Length: 10\r\n\r\nabc")
        .await;

    let response = client.response().await;
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn a_streaming_route_answers_100_continue_before_the_body() {
    let entered = Arc::new(Notify::new());
    let signal = Arc::clone(&entered);
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", move |mut req: RequestContext| {
        let signal = Arc::clone(&signal);
        async move {
            signal.notify_one();
            match req.read_body(1024).await {
                Ok(body) => Response::text_dynamic(format!("{}", body.len())),
                Err(error) => refuse(400, "Bad Request", error),
            }
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"POST /up HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\n",
        )
        .await;

    // The interim response arrives before the body is sent; the handler is already running.
    let interim = {
        let mut chunk = [0u8; 64];
        let read = tokio::time::timeout(Duration::from_secs(2), client.stream.read(&mut chunk))
            .await
            .expect("no interim response")
            .expect("read interim response");
        String::from_utf8_lossy(&chunk[..read]).into_owned()
    };
    assert!(
        interim.starts_with("HTTP/1.1 100 Continue"),
        "unexpected interim answer: {interim:?}"
    );
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("the handler should be running while the client holds its body");

    client.send(b"hello").await;
    let response = client.response().await;
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, b"5".as_slice())
    );

    server.stop().await;
}

/// The other half of the same rule: a streamed route must not answer `Expect` for a request that
/// is already whole. The interim exists because the client paused, and this client paused for
/// nothing - head and body came in one write, exactly the shape the buffered path refuses in
/// `expect_100_continue_is_not_sent_for_10_or_a_complete_request`. A streamed route that answered
/// it anyway would make one request mean two things depending on which route the app registered,
/// and `response()` here returns the *first* message, so an interim shows as a `100`, not as a
/// delay.
#[tokio::test]
async fn a_streaming_route_sends_no_interim_for_a_request_that_arrived_whole() {
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(1024).await {
            Ok(body) => Response::text_dynamic(format!("{}", body.len())),
            Err(error) => refuse(400, "Bad Request", error),
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"POST /up HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\nhello",
        )
        .await;

    let response = client.response().await;
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, b"5".as_slice()),
        "a complete request on a streamed route was answered with an interim"
    );

    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// The declared length on a *streamed* route (track 110). The lane is the one place the framing
// rule could plausibly bend: the handler is already running while the body arrives, and the whole
// surplus may already be sitting in the same socket read. Both shapes below were measured on the
// wire first, and both come out the same way they do on the buffered path — the lane ends at the
// promised byte and everything after it belongs to the next message.
//
// The syntax rules are deliberately not repeated here. `Content-Length` parsing, the OWS trim and
// the one-line-only rule all sit in the shared head parser, ahead of any lane, so the buffered
// tests in `http_e2e.rs` (`an_ows_padded_content_length_is_served_on_http_1`,
// `a_repeated_content_length_is_refused_on_http_1_even_when_it_agrees`) already price the only
// decision this frontend makes before the body path splits. What splits is what happens to the
// bytes after the promise, and that is what these two measure.
// ---------------------------------------------------------------------------------------------

/// The route both tests below share: it reads its lane to the end and reports the bytes it was
/// given, so a lane that ran past the declared length would show in the body, not hide.
fn lane_echo_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.route_stream(
        Method::Post,
        "/echo",
        |mut req: RequestContext| async move {
            match req.read_body(1024).await {
                Ok(body) => Response::text_dynamic(format!(
                    "{}|{}",
                    body.len(),
                    String::from_utf8_lossy(&body)
                )),
                Err(error) => refuse(400, "Bad Request", error),
            }
        },
    );
    app = app.get("/ping", |_| "pong");
    app
}

#[tokio::test]
async fn a_streamed_lane_stops_at_its_declared_length_and_the_surplus_is_the_next_request() {
    let server = TestServer::start(lane_echo_app()).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 6\r\n\r\n\
              abcdefGET /ping HTTP/1.1\r\nHost: test\r\n\r\n",
        )
        .await;

    let first = client.response().await;
    assert_eq!(
        (first.status, first.body.as_slice()),
        (200, b"6|abcdef".as_slice()),
        "the lane must close at the promised byte even though the surplus arrived in the same read"
    );
    let second = client.response().await;
    assert_eq!(
        (second.status, second.body.as_slice()),
        (200, b"pong".as_slice()),
        "a streamed route that stops where it was told leaves the connection framed for what follows"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_streamed_lane_whose_surplus_is_not_a_request_is_400_and_close() {
    let server = TestServer::start(lane_echo_app()).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 6\r\n\r\nabcdefGARBAGE\r\n\r\n",
        )
        .await;

    let first = client.response().await;
    assert_eq!(
        (first.status, first.body.as_slice()),
        (200, b"6|abcdef".as_slice()),
        "the promised body is answered before anything is said about the surplus"
    );
    assert_eq!(
        client.response().await.status,
        400,
        "the surplus is the next message and a streamed route reframes it no more than a buffered one"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The streamed lane is a third reader of the same head, so RFC 9110 §10.1.1 refuses it the same
/// way: no handler is entered, no body pump is started, and the answer is `417` while the body is
/// still unsent. The rule lives in the shared head parser precisely so this lane cannot bend it.
#[tokio::test]
async fn a_streaming_route_refuses_an_unsupported_expectation_before_dispatch() {
    let entered = Arc::new(AtomicUsize::new(0));
    let signal = Arc::clone(&entered);
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", move |mut req: RequestContext| {
        let signal = Arc::clone(&signal);
        async move {
            signal.fetch_add(1, Ordering::SeqCst);
            match req.read_body(1024).await {
                Ok(body) => Response::text_dynamic(format!("{}", body.len())),
                Err(error) => refuse(400, "Bad Request", error),
            }
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"POST /up HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nExpect: foo=bar\r\n\r\n")
        .await;

    let refusal = {
        let mut chunk = [0u8; 64];
        let read = tokio::time::timeout(Duration::from_secs(2), client.stream.read(&mut chunk))
            .await
            .expect("no refusal of the unsupported expectation")
            .expect("read the refusal");
        String::from_utf8_lossy(&chunk[..read]).into_owned()
    };
    assert!(
        refusal.starts_with("HTTP/1.1 417 "),
        "unexpected answer on a streaming route: {refusal:?}"
    );
    assert_eq!(
        entered.load(Ordering::SeqCst),
        0,
        "a streaming handler ran for a request refused on its expectation"
    );
    client.expect_closed().await;

    server.stop().await;
}
