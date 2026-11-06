//! End-to-end tests over real TCP sockets.
//!
//! Every test binds `127.0.0.1:0`, reads the assigned port from `AetherServer::local_addr`,
//! and drives the server with raw bytes. This suite is checked in so the behaviours the
//! documentation claims (chunked bodies, duplicate headers, multipart, static ranges,
//! CORS scoping, timeouts, panic isolation, graceful shutdown, WebSocket upgrade) stay
//! verified over time.

use aether::prelude::*;
use bytes::Bytes;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
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
struct HttpResponse {
    /// The version token exactly as it arrived on the status line, because "which version
    /// did the server claim to answer with" is a separately testable fact from the status.
    version: String,
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

    async fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await
    }

    async fn send_request(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> std::io::Result<()> {
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: test\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            && !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
        {
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        let mut bytes = request.into_bytes();
        bytes.extend_from_slice(body);
        self.send(&bytes).await
    }

    fn take_response(&mut self, expect_body: bool) -> Option<HttpResponse> {
        let header_end = find_subslice(&self.buffered, b"\r\n\r\n")?;
        let header_text = std::str::from_utf8(&self.buffered[..header_end]).ok()?;
        let mut lines = header_text.split("\r\n");
        let status_line = lines.next()?;
        let mut status_fields = status_line.split_whitespace();
        let version = status_fields.next()?.to_string();
        let status = status_fields.next()?.parse::<u16>().ok()?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();

        let body_len = if expect_body {
            headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .unwrap_or(0)
        } else {
            0
        };
        let total = header_end + 4 + body_len;
        if self.buffered.len() < total {
            return None;
        }
        let body = self.buffered[header_end + 4..total].to_vec();
        self.buffered.drain(..total);
        Some(HttpResponse {
            version,
            status,
            headers,
            body,
        })
    }

    async fn response(&mut self, expect_body: bool) -> HttpResponse {
        loop {
            if let Some(response) = self.take_response(expect_body) {
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

    async fn poll_response(&mut self, expect_body: bool) -> Option<HttpResponse> {
        if let Some(response) = self.take_response(expect_body) {
            return Some(response);
        }
        let mut chunk = [0u8; 8192];
        match tokio::time::timeout(Duration::from_millis(5), self.stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) => None,
            Ok(Ok(read)) => {
                self.buffered.extend_from_slice(&chunk[..read]);
                self.take_response(expect_body)
            }
            Ok(Err(_)) => None,
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

    async fn read_exact_ws(&mut self, len: usize) -> Vec<u8> {
        while self.buffered.len() < len {
            let mut chunk = [0u8; 8192];
            let read = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
                .await
                .expect("timed out waiting for WebSocket bytes")
                .expect("failed to read WebSocket bytes");
            assert!(read > 0, "connection closed before the frame was complete");
            self.buffered.extend_from_slice(&chunk[..read]);
        }
        self.buffered.drain(..len).collect()
    }

    async fn read_ws_frame(&mut self) -> (u8, bool, Vec<u8>) {
        let head = self.read_exact_ws(2).await;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0f;
        assert_eq!(head[1] & 0x80, 0, "server frames must not be masked");
        let len = match head[1] & 0x7f {
            126 => {
                let ext = self.read_exact_ws(2).await;
                u16::from_be_bytes([ext[0], ext[1]]) as usize
            }
            127 => {
                let ext = self.read_exact_ws(8).await;
                u64::from_be_bytes(ext.try_into().expect("8 bytes")) as usize
            }
            n => n as usize,
        };
        let payload = self.read_exact_ws(len).await;
        (opcode, fin, payload)
    }
}

fn ws_client_frame(fin: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push((if fin { 0x80 } else { 0 }) | opcode);
    match payload.len() {
        len @ 0..=125 => out.push(0x80 | len as u8),
        len @ 126..=65535 => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index & 3]),
    );
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "aether_e2e_{}_{}_{}",
        std::process::id(),
        tag,
        counter
    ));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[tokio::test]
async fn routing_params_405_allow_head_and_404() {
    let mut app = AetherApp::new();
    app = app.get("/text", |_| "hello world");
    app = app.get("/users/{id}", |req| {
        let id = req.param("id").unwrap_or_default().to_string();
        Response::json(Bytes::from(json!({ "id": id }).to_string()))
    });
    app = app.put("/users/{id}", |_| Response::status(204, "No Content"));
    app = app.get("/static-only", |_| "static");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/text", &[], b"").await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"hello world");
    assert_eq!(
        response.header("Content-Type"),
        Some("text/plain; charset=utf-8")
    );

    // Route captures are percent-decoded exactly once, after routing on the raw path.
    client
        .send_request("GET", "/users/%D9%85", &[], b"")
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["id"], "م");

    client
        .send_request("DELETE", "/users/7", &[], b"")
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 405);
    let allow = response.header("Allow").expect("405 must carry Allow");
    assert!(allow.contains("GET"), "Allow was {allow}");
    assert!(allow.contains("PUT"), "Allow was {allow}");
    assert!(!allow.contains("DELETE"), "Allow was {allow}");

    client
        .send_request("GET", "/missing", &[], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 404);

    // HEAD falls back to GET, preserving the representation headers but not the payload.
    client
        .send_request("HEAD", "/text", &[], b"")
        .await
        .unwrap();
    let response = client.response(false).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Length"), Some("11"));
    assert!(response.body.is_empty());

    // The connection is still usable for a subsequent request.
    client
        .send_request("GET", "/static-only", &[], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.body, b"static");

    server.stop().await;
}

#[tokio::test]
async fn keep_alive_pipelining_and_chunked_bodies() {
    let mut app = AetherApp::new();
    app = app.get("/a", |_| "A");
    app = app.get("/b", |_| "B");
    app = app.post("/echo", |req| {
        let body = String::from_utf8_lossy(&req.body).into_owned();
        Response::json(Bytes::from(
            json!({ "body": body, "len": req.body.len() }).to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // Two complete requests in one TCP write must be answered in order.
    let pipelined = b"GET /a HTTP/1.1\r\nHost: test\r\n\r\nGET /b HTTP/1.1\r\nHost: test\r\n\r\n";
    client.send(pipelined).await.unwrap();
    let first = client.response(true).await;
    let second = client.response(true).await;
    assert_eq!(
        (first.status, first.body.as_slice()),
        (200, b"A".as_slice())
    );
    assert_eq!(
        (second.status, second.body.as_slice()),
        (200, b"B".as_slice())
    );

    // A third request on the same persistent connection.
    client.send_request("GET", "/b", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.body, b"B");

    let chunked = b"POST /echo HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n\
5\r\nhello\r\n1\r\n \r\n5\r\nworld\r\n0\r\nX-Trailer: accepted\r\n\r\n";
    client.send(chunked).await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["body"], "hello world");
    assert_eq!(json["len"], 11);

    // Conflicting framing is rejected and the connection is closed.
    let smuggling = b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    client.send(smuggling).await.unwrap();
    assert_eq!(client.response(true).await.status, 400);
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn duplicate_request_header_fields_are_preserved() {
    let mut app = AetherApp::new();
    app = app.get("/headers", |req| {
        Response::json(Bytes::from(
            json!({
                "cookies": req.header_values("cookie"),
                "dup": req.header_values("X-Dup"),
                "first": req.header("x-dup"),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /headers HTTP/1.1\r\nHost: test\r\nCookie: a=1\r\nX-Dup: one\r\ncookie: b=2\r\nx-dup: two\r\n\r\n")
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["cookies"], json!(["a=1", "b=2"]));
    assert_eq!(json["dup"], json!(["one", "two"]));
    assert_eq!(json["first"], "one");

    server.stop().await;
}

/// RFC 9113 §8.2.3 lets a client split `Cookie` across repeated fields, and a receiver reads the
/// combined field; RFC 6265 §5.4 wraps the whole jar in one `Cookie` field for exactly that
/// reason. A name that arrived on the second line is present, not absent — and `cookie()` used to
/// answer from the first line alone, while `signed_cookie` and the CSRF check read them all.
#[tokio::test]
async fn cookie_lookup_reads_every_cookie_field_line() {
    let mut app = AetherApp::new();
    app = app.get("/probe", |req| {
        Response::json(Bytes::from(
            json!({
                "first_line": req.cookie("a"),
                "second_line": req.cookie("theme"),
                "missing": req.cookie("absent"),
                "duplicated": req.cookie("dupe"),
                "case_variant": req.cookie("THEME"),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /probe HTTP/1.1\r\nHost: test\r\nCookie: a=1; dupe=first\r\nCookie: theme=dark; dupe=second\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["first_line"], "1");
    assert_eq!(
        json["second_line"], "dark",
        "a cookie that arrived on the second field line is a cookie the client sent"
    );
    assert_eq!(json["missing"], Value::Null);
    assert_eq!(
        json["duplicated"], "first",
        "the first occurrence wins across lines exactly as it does within one"
    );
    assert_eq!(
        json["case_variant"],
        Value::Null,
        "a cookie name is case-sensitive (RFC 6265 §5.2), and reading more lines must not blur it"
    );

    server.stop().await;
}

/// A browser submits a GET form as `application/x-www-form-urlencoded`, so a field named
/// `first name` arrives as `first+name=...`: the `+`-is-space rule covers the key, not only the
/// value. `query_decoded` applied that rule to the value and plain percent-decoding to the key,
/// so it answered `None` for a parameter that [`RequestContext::queries_decoded`] and the `Query`
/// extractor both returned from the same string.
#[tokio::test]
async fn decoded_query_lookup_decodes_the_key_by_the_form_rule() {
    let mut app = AetherApp::new();
    app = app.get("/probe", |req| {
        let map = req.queries_decoded();
        Response::json(Bytes::from(
            json!({
                "plus_key": req.query_decoded("first name"),
                "escaped_key": req.query_decoded("second name"),
                "literal_plus_key": req.query_decoded("a+b"),
                "both_key": req.query_decoded("x y+z"),
                "map_plus_key": map.get("first name").cloned(),
                "raw_plus_key": req.query("first+name"),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /probe?first+name=plus&second%20name=escaped&a%2Bb=literal&x+y%2Bz=both HTTP/1.1\r\nHost: test\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(
        json["plus_key"], "plus",
        "an encoded space in the key is the space the caller asked for"
    );
    assert_eq!(json["escaped_key"], "escaped");
    assert_eq!(
        json["literal_plus_key"], "literal",
        "%2B is a literal plus and must not be decoded into a space"
    );
    assert_eq!(
        json["both_key"], "both",
        "one key can carry both encodings; `+` is substituted before percent-decoding"
    );
    assert_eq!(
        json["map_plus_key"], "plus",
        "the singular accessor and the map read the same query string"
    );
    assert_eq!(json["raw_plus_key"], "plus", "the raw accessor stays raw");

    server.stop().await;
}

/// The read accessors resolve one store, so `header()`, `header_values()` and `headers()` cannot
/// disagree, and a field name is matched case-insensitively whatever casing the client used.
#[tokio::test]
async fn header_accessors_read_one_store() {
    let mut app = AetherApp::new();
    app = app.get("/probe", |req| {
        let enumerated: serde_json::Map<String, Value> = req
            .headers()
            .map(|(name, value)| (name.to_string(), Value::String(value.to_string())))
            .collect();
        Response::json(Bytes::from(
            json!({
                "map": enumerated,
                "header_upper": req.header("X-DUP"),
                "header_lower": req.header("x-dup"),
                "values": req.header_values("X-Dup"),
                "mixed_case_lookup": req.header("Content-Type"),
                "plain_lookup": req.header("content-type"),
                "missing": req.header("x-nope"),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /probe HTTP/1.1\r\nHost: test\r\nX-Dup: one\r\nx-dup: two\r\nX-DUP: three\r\n\
                content-type: text/plain\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    // One entry per field, keyed by the normalised (lowercase) name, valued by the first line.
    assert_eq!(
        json["map"],
        json!({ "host": "test", "x-dup": "one", "content-type": "text/plain" })
    );
    assert_eq!(json["header_upper"], "one");
    assert_eq!(json["header_lower"], "one");
    assert_eq!(json["values"], json!(["one", "two", "three"]));
    // `header()` is case-insensitive whatever the wire casing was.
    assert_eq!(json["mixed_case_lookup"], "text/plain");
    assert_eq!(json["plain_lookup"], "text/plain");
    assert_eq!(json["missing"], Value::Null);

    server.stop().await;
}

/// `Connection` is a list field: a `close` directive on any line ends the connection, even
/// when an earlier line asked to keep it alive (RFC 9110 §5.3, §7.3.1).
#[tokio::test]
async fn repeated_connection_lines_still_signal_close() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ping HTTP/1.1\r\nHost: test\r\nConnection: keep-alive\r\nconnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    server.stop().await;
}

/// A preflight reads the first `Access-Control-Request-Method` line, so a client that
/// repeats the field cannot make a non-routable method look routable.
#[tokio::test]
async fn cors_preflight_reads_the_first_request_method_line() {
    let mut app = AetherApp::new();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig::default()));
    app = app.post("/submit", |_| "submitted");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"OPTIONS /submit HTTP/1.1\r\nHost: test\r\nOrigin: https://example.com\r\n\
                Access-Control-Request-Method: POST\r\naccess-control-request-method: DELETE\r\n\r\n",
        )
        .await
        .unwrap();
    let preflight = client.response(true).await;
    assert_eq!(preflight.status, 204);
    assert_eq!(preflight.header("Access-Control-Allow-Origin"), Some("*"));

    // With only the non-routable method on the first line the preflight is refused.
    client
        .send(
            b"OPTIONS /submit HTTP/1.1\r\nHost: test\r\nOrigin: https://example.com\r\n\
                Access-Control-Request-Method: DELETE\r\naccess-control-request-method: POST\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 403);

    server.stop().await;
}

#[tokio::test]
async fn multipart_form_data_is_parsed_end_to_end() {
    let mut app = AetherApp::new();
    app = app.post("/upload", |req| {
        let form = match req.multipart() {
            Ok(form) => form,
            Err(error) => {
                return Response::json_status(
                    400,
                    "Bad Request",
                    Bytes::from(json!({ "error": error.to_string() }).to_string()),
                );
            }
        };
        let mut fields = serde_json::Map::new();
        let mut filename = None;
        let mut file_len = 0usize;
        for part in form.parts() {
            match part.name.as_deref() {
                Some("file") => {
                    filename = part.filename.clone();
                    file_len = part.data.len();
                }
                Some(name) => {
                    if let Ok(text) = part.text() {
                        fields.insert(name.to_string(), json!(text));
                    }
                }
                None => {}
            }
        }
        Response::json(Bytes::from(
            json!({
                "fields": fields,
                "filename": filename,
                "file_len": file_len,
                "parts": form.parts().len(),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    let boundary = "AaB03x";
    let body = format!(
        "--{boundary}\r\n\
Content-Disposition: form-data; name=\"title\"\r\n\
\r\n\
hello\r\n\
--{boundary}\r\n\
Content-Disposition: form-data; name=\"file\"; filename=\"report.txt\"\r\n\
Content-Type: text/plain\r\n\
\r\n\
file-data\r\n\
--{boundary}--\r\n"
    );
    client
        .send_request(
            "POST",
            "/upload",
            &[(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )],
            body.as_bytes(),
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["fields"]["title"], "hello");
    assert_eq!(json["filename"], "report.txt");
    assert_eq!(json["file_len"], 9);
    assert_eq!(json["parts"], 2);

    client
        .send_request(
            "POST",
            "/upload",
            &[("Content-Type", "multipart/form-data")],
            b"anything",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 400);
    assert!(
        response.json()["error"]
            .as_str()
            .unwrap()
            .contains("boundary")
    );

    // Two delimiter lines with nothing between them describe no part at all. This used to
    // slice the part's byte range backwards and panic the handler, which the loop answers as
    // a 500 of its own; the 400 is the pin that the refusal happens before any panic.
    let adjacent = format!("--{boundary}\r\n--{boundary}--\r\n");
    client
        .send_request(
            "POST",
            "/upload",
            &[(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )],
            adjacent.as_bytes(),
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 400);
    assert!(
        response.json()["error"]
            .as_str()
            .unwrap()
            .contains("header block"),
        "the empty part must be refused for the header block it lacks"
    );

    server.stop().await;
}

#[tokio::test]
async fn static_files_support_conditional_and_range_requests() {
    let base = temp_dir("static");
    let public = base.join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("data.txt"), b"0123456789").unwrap();
    std::fs::write(base.join("secret.txt"), b"secret").unwrap();

    let mut app = AetherApp::new();
    app = app.static_dir("/assets", public.clone());
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/assets/data.txt", &[], b"")
        .await
        .unwrap();
    let full = client.response(true).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.body, b"0123456789");
    assert_eq!(full.header("Accept-Ranges"), Some("bytes"));
    let etag = full
        .header("ETag")
        .expect("static response has an ETag")
        .to_string();
    assert!(
        etag.starts_with("W/\""),
        "a validator built from file metadata is weak, never a strong content tag: {etag}"
    );
    let last_modified = full
        .header("Last-Modified")
        .expect("a streamed file answer publishes the date its ETag was built from")
        .to_string();

    client
        .send_request("GET", "/assets/data.txt", &[("If-None-Match", &etag)], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 304);

    client
        .send_request("GET", "/assets/data.txt", &[("Range", "bytes=2-5")], b"")
        .await
        .unwrap();
    let partial = client.response(true).await;
    assert_eq!(partial.status, 206);
    assert_eq!(partial.body, b"2345");
    assert_eq!(partial.header("Content-Range"), Some("bytes 2-5/10"));

    // A weak validator never satisfies `If-Range` (RFC 9110 §13.1.5), so the range is ignored and
    // the whole representation comes back — 200, not a 206 that happens to carry every byte.
    client
        .send_request(
            "GET",
            "/assets/data.txt",
            &[("Range", "bytes=0-99"), ("If-Range", &etag)],
            b"",
        )
        .await
        .unwrap();
    let weak_if_range = client.response(true).await;
    assert_eq!(
        weak_if_range.status, 200,
        "a weak ETag must not earn a partial answer"
    );
    assert_eq!(weak_if_range.body, b"0123456789");

    // The date the answer published is a strong-enough validator for this purpose, so the same
    // range with `Last-Modified` as `If-Range` earns the partial answer.
    client
        .send_request(
            "GET",
            "/assets/data.txt",
            &[("Range", "bytes=2-5"), ("If-Range", &last_modified)],
            b"",
        )
        .await
        .unwrap();
    let date_if_range = client.response(true).await;
    assert_eq!(
        date_if_range.status, 206,
        "the published date must satisfy If-Range"
    );
    assert_eq!(date_if_range.body, b"2345");

    client
        .send_request("GET", "/assets/data.txt", &[("Range", "bytes=99-")], b"")
        .await
        .unwrap();
    let unsatisfiable = client.response(true).await;
    assert_eq!(unsatisfiable.status, 416);
    assert_eq!(unsatisfiable.header("Content-Range"), Some("bytes */10"));

    for hostile in [
        "/assets/%2e%2e/secret.txt",
        "/assets/..%2fsecret.txt",
        "/assets/CON",
    ] {
        client.send_request("GET", hostile, &[], b"").await.unwrap();
        let response = client.response(true).await;
        assert_eq!(
            response.status, 404,
            "hostile static path {hostile} served content"
        );
    }

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// A link inside the static root is a name, not a boundary: the answer is decided by where its
/// canonical target lives. `..` and `%2e%2e` are refused lexically before the filesystem is
/// touched, so a reparse point is the one way a name spelled under the root can reach an inode
/// outside it — and the precompressed sibling and `index.html` are already checked this way.
#[tokio::test]
async fn static_dir_does_not_follow_a_link_out_of_its_root() {
    let base = temp_dir("static_link");
    let public = base.join("public");
    let outside = base.join("outside");
    let real = public.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(real.join("ok.txt"), b"inside").unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

    make_directory_link(&public.join("escape"), &outside)
        .expect("create a directory link out of the static root");
    make_directory_link(&public.join("inner"), &real)
        .expect("create a directory link inside the static root");

    let mut app = AetherApp::new();
    app = app.static_dir("/assets", public.clone());
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/assets/escape/secret.txt", &[], b"")
        .await
        .unwrap();
    let escaped = client.response(true).await;
    assert_eq!(
        escaped.status, 404,
        "a link out of the root served a file outside it"
    );
    assert_eq!(
        escaped.body, b"",
        "the refusal must not carry the file it refused"
    );

    client
        .send_request("GET", "/assets/escape", &[], b"")
        .await
        .unwrap();
    assert_eq!(
        client.response(true).await.status,
        404,
        "the directory the link names is outside the root too"
    );

    client
        .send_request("GET", "/assets/inner/ok.txt", &[], b"")
        .await
        .unwrap();
    let inside = client.response(true).await;
    assert_eq!(
        inside.status, 200,
        "a link that stays inside the root is still a served name"
    );
    assert_eq!(inside.body, b"inside");

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(windows)]
fn make_directory_link(link: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    // A junction is a reparse point an unprivileged account can create; a file symlink would
    // need developer mode or elevation, which the test machine does not have.
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("mklink /J failed"))
    }
}

#[cfg(unix)]
fn make_directory_link(link: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[tokio::test]
async fn static_file_serves_disk_content_over_async_route() {
    let base = temp_dir("static_file");
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("report.css");
    std::fs::write(&file, b"body{margin:0}").unwrap();
    let served: &'static str = Box::leak(
        file.into_os_string()
            .into_string()
            .unwrap()
            .into_boxed_str(),
    );
    let missing: &'static str = Box::leak(
        base.join("gone.css")
            .into_os_string()
            .into_string()
            .unwrap()
            .into_boxed_str(),
    );

    let mut app = AetherApp::new();
    app = app.static_file("/report.css", served);
    app = app.static_file("/gone.css", missing);
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/report.css", &[], b"")
        .await
        .unwrap();
    let full = client.response(true).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.body, b"body{margin:0}");
    assert!(full.header("Content-Type").unwrap().starts_with("text/css"));

    client
        .send_request("GET", "/gone.css", &[], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 404);

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// A directory with no `index.html` stays a `404` until the operator opts in: publishing the
/// file names under a root is a decision, and the listing must not widen the resolver.
#[tokio::test]
async fn directory_listing_is_opt_in_and_never_widens_the_resolver() {
    let base = temp_dir("listing");
    let public = base.join("docs_root");
    let sub = public.join("guide");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(public.join("a.txt"), b"a").unwrap();
    std::fs::write(public.join("rock&roll.txt"), b"ampersand").unwrap();
    std::fs::write(public.join(".hidden"), b"dot").unwrap();
    std::fs::write(sub.join("guide.txt"), b"guide").unwrap();
    std::fs::write(base.join("secret.txt"), b"secret").unwrap();

    let mut closed = AetherApp::new();
    closed = closed.static_dir("/assets", public.clone());
    let server = TestServer::start(closed).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request("GET", "/assets/guide/", &[], b"")
        .await
        .unwrap();
    assert_eq!(
        client.response(true).await.status,
        404,
        "a listing was rendered without being asked for"
    );
    client
        .send_request("GET", "/assets/", &[], b"")
        .await
        .unwrap();
    assert_eq!(
        client.response(true).await.status,
        404,
        "the tree root answered without a listing being asked for"
    );
    server.stop().await;

    let mut open = AetherApp::new();
    open = open.static_dir_with_options(
        "/assets",
        public.clone(),
        StaticDirOptions {
            directory_listing: true,
            ..Default::default()
        },
    );
    let server = TestServer::start(open).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/assets/guide/", &[], b"")
        .await
        .unwrap();
    let listing = client.response(true).await;
    assert_eq!(listing.status, 200);
    assert!(
        listing
            .header("Content-Type")
            .unwrap()
            .starts_with("text/html"),
        "{:?}",
        listing.header("Content-Type")
    );
    assert_eq!(listing.header("Cache-Control"), Some("no-cache"));
    let html = String::from_utf8(listing.body).unwrap();
    assert!(
        html.contains("<a href=\"guide.txt\">guide.txt</a>"),
        "{html}"
    );
    assert!(html.contains("<a href=\"../\">../</a>"), "{html}");
    assert!(
        html.contains("<title>Index of /assets/guide</title>"),
        "{html}"
    );

    client
        .send_request("GET", "/assets/", &[], b"")
        .await
        .unwrap();
    let root = String::from_utf8(client.response(true).await.body).unwrap();
    assert!(root.contains("rock&amp;roll.txt"), "{root}");
    assert!(
        root.contains("href=\"rock%26roll.txt\""),
        "the href must be requestable as written: {root}"
    );
    assert!(!root.contains(".hidden"), "a dotfile was listed: {root}");
    assert!(
        !root.contains("secret.txt"),
        "the listing escaped its root: {root}"
    );
    assert!(
        !root.contains("href=\"../\""),
        "the root listing has no parent to link to: {root}"
    );

    // The tree's own root is reachable: it redirects to the slash-terminated form, which lists.
    client
        .send_request("GET", "/assets", &[], b"")
        .await
        .unwrap();
    let root_redirect = client.response(true).await;
    assert_eq!(root_redirect.status, 301);
    assert_eq!(root_redirect.header("Location"), Some("/assets/"));

    // The escaped href round-trips: the same bytes the listing published are served.
    client
        .send_request("GET", "/assets/rock%26roll.txt", &[], b"")
        .await
        .unwrap();
    let round_trip = client.response(true).await;
    assert_eq!(round_trip.status, 200);
    assert_eq!(round_trip.body, b"ampersand");

    // Reached without its trailing slash, a directory redirects: every relative link on a
    // listing resolves against the final segment, and that segment is empty only after `/`.
    client
        .send_request("GET", "/assets/guide", &[], b"")
        .await
        .unwrap();
    let redirect = client.response(true).await;
    assert_eq!(redirect.status, 301);
    assert_eq!(redirect.header("Location"), Some("/assets/guide/"));

    for hostile in [
        "/assets/%2e%2e/secret.txt",
        "/assets/..%2fsecret.txt",
        "/assets/../secret.txt",
    ] {
        client.send_request("GET", hostile, &[], b"").await.unwrap();
        assert_eq!(
            client.response(true).await.status,
            404,
            "the listing opened {hostile}"
        );
    }

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// An `index.html` is the directory's own answer: switching listing on must not replace a page
/// the application ships.
#[tokio::test]
async fn a_directory_with_an_index_page_is_still_that_page() {
    let base = temp_dir("listing_index");
    let public = base.join("site");
    let sub = public.join("app");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("index.html"), b"<h1>app</h1>").unwrap();
    std::fs::write(sub.join("other.txt"), b"other").unwrap();

    let mut app = AetherApp::new();
    app = app.static_dir_with_options(
        "/assets",
        public.clone(),
        StaticDirOptions {
            directory_listing: true,
            ..Default::default()
        },
    );
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    for path in ["/assets/app/", "/assets/app"] {
        client.send_request("GET", path, &[], b"").await.unwrap();
        let page = client.response(true).await;
        assert_eq!(page.status, 200, "{path}");
        assert_eq!(page.body, b"<h1>app</h1>", "{path}");
    }

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// `index.html` is the directory's own page whether or not a listing is on: the flag opts into
/// publishing the names a directory holds, not into serving the page it already holds. The
/// listing-off branch is the one that used to answer `404` before looking, and the tree's own
/// root had no route at all, so both forms of both directories are asked for here.
#[tokio::test]
async fn an_index_page_is_served_when_listings_are_off() {
    let base = temp_dir("index_off");
    let public = base.join("site");
    let page_dir = public.join("app");
    std::fs::create_dir_all(&page_dir).unwrap();
    std::fs::write(public.join("index.html"), b"<h1>home</h1>").unwrap();
    std::fs::write(public.join("other.txt"), b"other").unwrap();
    std::fs::write(page_dir.join("index.html"), b"<h1>app</h1>").unwrap();

    let mut app = AetherApp::new();
    app = app.static_dir("/assets", public.clone());
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    for (path, expected) in [
        ("/assets/app/", "<h1>app</h1>"),
        ("/assets/app", "<h1>app</h1>"),
        ("/assets/", "<h1>home</h1>"),
        ("/assets", "<h1>home</h1>"),
    ] {
        client.send_request("GET", path, &[], b"").await.unwrap();
        let page = client.response(true).await;
        assert_eq!(page.status, 200, "{path}");
        assert_eq!(page.body, expected.as_bytes(), "{path}");
        assert_eq!(
            page.header("Content-Type"),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
    }

    // Opting out of a listing still means a directory with no page is a `404`, and a file
    // beside a page is still that file.
    client
        .send_request("GET", "/assets/missing/", &[], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 404);
    client
        .send_request("GET", "/assets/other.txt", &[], b"")
        .await
        .unwrap();
    let file = client.response(true).await;
    assert_eq!(file.status, 200);
    assert_eq!(file.body, b"other");

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// The `.br`/`.gz` siblings are opt-in, chosen by q-value, labelled with `Content-Encoding`, and
/// the answer carries `Vary` so a cache never replays one encoding as another.
#[tokio::test]
async fn precompressed_variants_are_selected_by_accept_encoding() {
    let base = temp_dir("precompressed");
    let public = base.join("static");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("app.js"), b"console.log('plain');").unwrap();
    std::fs::write(public.join("app.js.br"), b"fake-brotli").unwrap();
    std::fs::write(public.join("app.js.gz"), b"fake-gzip").unwrap();
    std::fs::write(public.join("plain.txt"), b"plain-body").unwrap();

    let mut off = AetherApp::new();
    off = off.static_dir("/assets", public.clone());
    let server = TestServer::start(off).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request(
            "GET",
            "/assets/app.js",
            &[("Accept-Encoding", "br, gzip")],
            b"",
        )
        .await
        .unwrap();
    let uncompressed = client.response(true).await;
    assert_eq!(uncompressed.status, 200);
    assert_eq!(uncompressed.body, b"console.log('plain');");
    assert_eq!(uncompressed.header("Content-Encoding"), None);
    assert_eq!(
        uncompressed.header("Vary"),
        None,
        "a route that never varies must not claim to"
    );
    server.stop().await;

    let mut app = AetherApp::new();
    app = app.static_dir_with_options(
        "/assets",
        public.clone(),
        StaticDirOptions {
            precompressed: true,
            ..Default::default()
        },
    );
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/assets/app.js", &[("Accept-Encoding", "br")], b"")
        .await
        .unwrap();
    let brotli = client.response(true).await;
    assert_eq!(brotli.status, 200);
    assert_eq!(brotli.body, b"fake-brotli");
    assert_eq!(brotli.header("Content-Encoding"), Some("br"));
    assert!(
        brotli
            .header("Content-Type")
            .unwrap()
            .starts_with("application/javascript"),
        "{:?}",
        brotli.header("Content-Type")
    );
    assert_eq!(brotli.header("Vary"), Some("Accept-Encoding"));

    // A tie prefers Brotli, and gzip is still reachable on its own.
    client
        .send_request(
            "GET",
            "/assets/app.js",
            &[("Accept-Encoding", "gzip, br")],
            b"",
        )
        .await
        .unwrap();
    assert_eq!(
        client.response(true).await.header("Content-Encoding"),
        Some("br")
    );

    client
        .send_request("GET", "/assets/app.js", &[("Accept-Encoding", "gzip")], b"")
        .await
        .unwrap();
    let gzip = client.response(true).await;
    assert_eq!(gzip.body, b"fake-gzip");
    assert_eq!(gzip.header("Content-Encoding"), Some("gzip"));

    // No preference stated is no reason to hand out a compressed body the client never asked
    // for, but the answer still varies, so a cache keeps the two apart.
    client
        .send_request("GET", "/assets/app.js", &[], b"")
        .await
        .unwrap();
    let identity = client.response(true).await;
    assert_eq!(identity.body, b"console.log('plain');");
    assert_eq!(identity.header("Content-Encoding"), None);
    assert_eq!(identity.header("Vary"), Some("Accept-Encoding"));

    // The entity-tag names the representation that was actually sent.
    let br_etag = brotli
        .header("ETag")
        .expect("a variant answer has an ETag")
        .to_string();
    client
        .send_request(
            "GET",
            "/assets/app.js",
            &[("Accept-Encoding", "br"), ("If-None-Match", &br_etag)],
            b"",
        )
        .await
        .unwrap();
    let not_modified = client.response(true).await;
    assert_eq!(not_modified.status, 304);
    assert_eq!(not_modified.header("Content-Encoding"), None);
    assert_eq!(not_modified.header("Vary"), Some("Accept-Encoding"));

    // Refusing identity is honoured, with a variant or without one.
    for path in ["/assets/app.js", "/assets/plain.txt"] {
        client
            .send_request("GET", path, &[("Accept-Encoding", "identity;q=0")], b"")
            .await
            .unwrap();
        let refusal = client.response(true).await;
        assert_eq!(refusal.status, 406, "{path}");
        assert_eq!(refusal.header("Vary"), Some("Accept-Encoding"), "{path}");
    }
    // `identity` stays acceptable while only other codings are excluded.
    client
        .send_request(
            "GET",
            "/assets/app.js",
            &[("Accept-Encoding", "br;q=0, gzip;q=0")],
            b"",
        )
        .await
        .unwrap();
    let plain_chosen = client.response(true).await;
    assert_eq!(plain_chosen.status, 200);
    assert_eq!(plain_chosen.body, b"console.log('plain');");

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn http10_requests_close_after_one_response_by_default() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send(b"GET /ping HTTP/1.0\r\nHost: t\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"pong");
    assert_eq!(
        resp.version, "HTTP/1.0",
        "the status line must carry the version of the request it answers"
    );
    assert_eq!(
        resp.header("Connection"),
        Some("close"),
        "HTTP/1.0 has no implicit persistence (RFC 9112 §9)"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The version echo and the HEAD body suppression interact: a 1.0 HEAD must still say
/// `HTTP/1.0`, keep the `Content-Length` the equivalent GET would send, and carry no payload.
#[tokio::test]
async fn http10_head_echoes_the_version_and_sends_no_body() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send(b"HEAD /ping HTTP/1.0\r\nHost: t\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(false).await;
    assert_eq!(resp.version, "HTTP/1.0");
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("Content-Length").map(|v| v.parse::<usize>()),
        Some(Ok(4))
    );
    assert!(resp.body.is_empty(), "a HEAD response carries no payload");
    // Nothing may follow the head: the connection closes, so stray bytes would be a
    // desynchronised next-response boundary rather than a second message.
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn http10_explicit_keepalive_persists_and_is_advertised() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    let request = b"GET /ping HTTP/1.0\r\nHost: t\r\nConnection: keep-alive\r\n\r\n";
    client.send(request).await.unwrap();
    let first = client.response(true).await;
    assert_eq!(first.status, 200);
    assert_eq!(
        first.version, "HTTP/1.0",
        "a persisted 1.0 connection is still an HTTP/1.0 exchange"
    );
    assert_eq!(
        first.header("Connection"),
        Some("keep-alive"),
        "a persisted HTTP/1.0 connection must advertise it"
    );

    client.send(request).await.unwrap();
    assert_eq!(client.response(true).await.body, b"pong");

    server.stop().await;
}

#[tokio::test]
async fn http11_default_persistence_is_unchanged() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    for _ in 0..2 {
        client.send_request("GET", "/ping", &[], b"").await.unwrap();
        let resp = client.response(true).await;
        assert_eq!(resp.version, "HTTP/1.1");
        assert_eq!(resp.body, b"pong");
    }
    // No implicit close on 1.1: the connection is still open for a third request.
    client.send_request("GET", "/ping", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);

    server.stop().await;
}

#[tokio::test]
async fn idle_keepalive_connection_closes_after_keep_alive_timeout() {
    let config = AetherConfig {
        keep_alive_timeout: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/ping", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);
    // Idle: nothing sent, nothing received — the server must drop the socket on its own.
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn http10_chunked_request_is_rejected() {
    let mut app = AetherApp::new();
    app = app.post("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // Transfer-Encoding chunked is an HTTP/1.1 framing; over 1.0 it must not be accepted.
    client
        .send(b"POST /ping HTTP/1.0\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 400);

    server.stop().await;
}

#[tokio::test]
async fn cors_preflight_is_scoped_to_real_routes() {
    let mut app = AetherApp::new();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig::default()));
    app = app.post("/submit", |_| "submitted");
    app = app.route_sync(Method::Options, "/custom", |_| "custom-options");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request(
            "OPTIONS",
            "/submit",
            &[
                ("Origin", "https://example.com"),
                ("Access-Control-Request-Method", "POST"),
                ("Access-Control-Request-Headers", "Content-Type"),
            ],
            b"",
        )
        .await
        .unwrap();
    let preflight = client.response(true).await;
    assert_eq!(preflight.status, 204);
    assert_eq!(preflight.header("Access-Control-Allow-Origin"), Some("*"));
    assert!(
        preflight
            .header("Access-Control-Allow-Methods")
            .unwrap()
            .contains("POST")
    );

    // A registered OPTIONS handler wins over the automatic preflight response.
    client
        .send_request(
            "OPTIONS",
            "/custom",
            &[
                ("Origin", "https://example.com"),
                ("Access-Control-Request-Method", "POST"),
            ],
            b"",
        )
        .await
        .unwrap();
    let custom = client.response(true).await;
    assert_eq!(
        (custom.status, custom.body.as_slice()),
        (200, b"custom-options".as_slice())
    );

    // A preflight for a non-routable path or disallowed method is refused.
    for (path, method) in [("/missing", "POST"), ("/submit", "DELETE")] {
        client
            .send_request(
                "OPTIONS",
                path,
                &[
                    ("Origin", "https://example.com"),
                    ("Access-Control-Request-Method", method),
                ],
                b"",
            )
            .await
            .unwrap();
        assert_eq!(
            client.response(true).await.status,
            403,
            "preflight {method} {path}"
        );
    }

    server.stop().await;
}

/// A CORS decision depends on the request's `Origin`, so a refusal must vary on it too
/// (RFC 9111 §4.1). Without the field a shared cache can store the bare refusal and replay
/// it to an origin the allow-list does accept.
#[tokio::test]
async fn a_denied_origin_response_still_varies_on_origin() {
    let mut app = AetherApp::new();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig {
        allowed_origins: &["https://app.example.com"],
        ..CorsConfig::default()
    }));
    app = app.get("/data", |_| "payload");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request(
            "GET",
            "/data",
            &[("Origin", "https://evil.example.com")],
            b"",
        )
        .await
        .unwrap();
    let denied = client.response(true).await;
    assert_eq!(denied.status, 200);
    assert_eq!(denied.header("Access-Control-Allow-Origin"), None);
    assert!(
        denied
            .header("Vary")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("origin"),
        "a refusal must vary on Origin, got {:?}",
        denied.header("Vary")
    );

    client
        .send_request(
            "GET",
            "/data",
            &[("Origin", "https://app.example.com")],
            b"",
        )
        .await
        .unwrap();
    let allowed = client.response(true).await;
    assert_eq!(
        allowed.header("Access-Control-Allow-Origin"),
        Some("https://app.example.com")
    );

    server.stop().await;
}

/// An approved preflight for a method the router does not serve is still a refusal, and the
/// rebuilt 403 must not carry the approval headers the middleware had started to write.
#[tokio::test]
async fn an_approved_preflight_for_an_unroutable_method_is_answered_without_cors_headers() {
    let mut app = AetherApp::new();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig {
        allowed_origins: &["https://app.example.com"],
        allow_methods: "GET, POST, PATCH",
        ..CorsConfig::default()
    }));
    app = app.post("/submit", |_| "submitted");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request(
            "OPTIONS",
            "/submit",
            &[
                ("Origin", "https://app.example.com"),
                ("Access-Control-Request-Method", "PATCH"),
            ],
            b"",
        )
        .await
        .unwrap();
    let refused = client.response(true).await;
    assert_eq!(refused.status, 403);
    for name in [
        "Access-Control-Allow-Origin",
        "Access-Control-Allow-Methods",
        "Access-Control-Allow-Headers",
        "Access-Control-Max-Age",
    ] {
        assert_eq!(refused.header(name), None, "{name} leaked onto the 403");
    }
    assert!(
        refused
            .header("Vary")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("origin"),
        "the refusal is still origin-dependent"
    );

    server.stop().await;
}

/// A refused preflight is decided by the same three request fields as an approved one, and
/// the serving core must keep the `Vary` the middleware recorded when it rebuilds the 403.
#[tokio::test]
async fn a_refused_preflight_still_varies_on_the_fields_that_decided_it() {
    let mut app = AetherApp::new();
    app = app.enable_cors(CorsMiddleware::new(CorsConfig {
        allowed_origins: &["https://app.example.com"],
        allow_methods: "GET, POST",
        ..CorsConfig::default()
    }));
    app = app.post("/submit", |_| "submitted");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request(
            "OPTIONS",
            "/submit",
            &[
                ("Origin", "https://app.example.com"),
                ("Access-Control-Request-Method", "DELETE"),
            ],
            b"",
        )
        .await
        .unwrap();
    let refused = client.response(true).await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.header("Access-Control-Allow-Origin"), None);
    let vary = refused
        .header("Vary")
        .unwrap_or_default()
        .to_ascii_lowercase();
    for token in [
        "origin",
        "access-control-request-method",
        "access-control-request-headers",
    ] {
        assert!(vary.contains(token), "{token} missing from {:?}", vary);
    }

    server.stop().await;
}

/// A CSP nonce is only worth anything if the header and the markup carry the *same* value: a
/// policy that names one nonce and a page that renders another blocks the page's own script.
/// This is the end-to-end claim — one value drawn by the server, substituted into the header,
/// and reachable by the handler — and its other half: an answer that renders no markup draws
/// nothing, so its policy goes out with the nonce expression removed.
#[tokio::test]
async fn a_csp_nonce_reaches_the_handler_and_the_header_that_authorises_it() {
    let mut app = AetherApp::new();
    app = app.enable_security_headers_with(SecurityHeadersMiddleware {
        content_security_policy: Some("default-src 'self'; script-src 'self' 'nonce-{nonce}'"),
        ..SecurityHeadersMiddleware::default()
    });
    // Three requests are let through; the fourth is the limiter's refusal. Both framework answers
    // below exercise the path that runs before a handler.
    app = app.enable_rate_limit(RateLimitConfig {
        burst: 3,
        per: Duration::from_secs(60),
        max_clients: 16,
    });
    app = app.get("/page", |req: RequestContext| {
        let nonce = req
            .extension::<CspNonce>()
            .map(|nonce| nonce.as_str().to_string())
            .unwrap_or_default();
        format!("<script nonce=\"{nonce}\">go()</script>")
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    async fn fetch(client: &mut Client) -> (String, String) {
        client.send_request("GET", "/page", &[], b"").await.unwrap();
        let response = client.response(true).await;
        assert_eq!(response.status, 200);
        let policy = response
            .header("Content-Security-Policy")
            .expect("the configured policy is applied")
            .to_string();
        assert!(
            !policy.contains("{nonce}"),
            "the placeholder must never reach the wire: {policy}"
        );
        let nonce = policy
            .split("'nonce-")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap_or_else(|| panic!("no nonce source expression in {policy}"))
            .to_string();
        (
            nonce,
            String::from_utf8(response.body).expect("the page is utf-8"),
        )
    }

    /// A framework answer renders no markup that could carry a nonce, so none is drawn: the
    /// policy still goes out, with the source expression removed rather than filled or left as
    /// an empty-and-therefore-ignored one.
    async fn fetch_framework_answer(client: &mut Client, path: &str, status: u16) {
        client.send_request("GET", path, &[], b"").await.unwrap();
        let response = client.response(true).await;
        assert_eq!(response.status, status);
        let policy = response
            .header("Content-Security-Policy")
            .expect("a framework answer carries the configured policy too")
            .to_string();
        assert!(
            !policy.contains("nonce-"),
            "no handler ran, yet the policy names a nonce: {policy}"
        );
        assert_eq!(policy, "default-src 'self'; script-src 'self'");
    }

    // A path no route matched: the 404 is written by the router, never dispatched.
    fetch_framework_answer(&mut client, "/missing", 404).await;

    let (first_nonce, first_body) = fetch(&mut client).await;
    assert_eq!(first_nonce.len(), 22, "16 random bytes as base64url");
    assert_eq!(
        first_body,
        format!("<script nonce=\"{first_nonce}\">go()</script>"),
        "the handler rendered a different nonce than the header names"
    );

    // Freshness is the protection: a value reused across responses is one an injected markup
    // could carry just as well as the application's own.
    let (second_nonce, second_body) = fetch(&mut client).await;
    assert_ne!(first_nonce, second_nonce, "two responses shared a nonce");
    assert_eq!(
        second_body,
        format!("<script nonce=\"{second_nonce}\">go()</script>")
    );

    // The limiter's refusal is written before the handler on a route that exists. It must be the
    // 429 the limiter decided, not a 500 from the nonce draw: the draw happens after it.
    fetch_framework_answer(&mut client, "/page", 429).await;

    server.stop().await;
}

/// The opt-out half: a policy that does not name the placeholder is emitted exactly as written
/// and no nonce is drawn or attached, so an application that never asked pays nothing.
#[tokio::test]
async fn a_policy_without_the_nonce_placeholder_draws_nothing() {
    let mut app = AetherApp::new();
    app = app.enable_security_headers_with(SecurityHeadersMiddleware {
        content_security_policy: Some("default-src 'self'"),
        ..SecurityHeadersMiddleware::default()
    });
    app = app.get("/plain", |req: RequestContext| {
        match req.extension::<CspNonce>() {
            Some(_) => "attached".to_string(),
            None => "none".to_string(),
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request("GET", "/plain", &[], b"")
        .await
        .unwrap();
    let response = client.response(true).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("Content-Security-Policy"),
        Some("default-src 'self'")
    );
    assert_eq!(
        String::from_utf8(response.body).expect("utf-8"),
        "none",
        "a policy without the placeholder must not attach a nonce to the request"
    );

    server.stop().await;
}

/// RFC 9110 §9.3.7: OPTIONS asks what the target supports, so a routable path answers it even
/// when no OPTIONS route was registered — answering `405` for a method the server does support
/// is false, and `404` for a path that exists is worse. The `GET` that follows on the same
/// connection is the framing control: the `204` must not desynchronise the stream.
#[tokio::test]
async fn an_options_request_answers_the_methods_a_path_serves() {
    let mut app = AetherApp::new();
    app = app.get("/thing", |_| "thing");
    app = app.post("/submit", |_| "submitted");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("OPTIONS", "/thing", &[], b"")
        .await
        .unwrap();
    let options = client.response(true).await;
    assert_eq!(options.status, 204);
    assert!(options.body.is_empty(), "a 204 must carry no body");
    assert_eq!(
        options.header("Allow"),
        Some("GET, HEAD, OPTIONS"),
        "Allow must name what the server answers, HEAD and OPTIONS included"
    );

    client
        .send_request("GET", "/thing", &[], b"")
        .await
        .unwrap();
    let after = client.response(true).await;
    assert_eq!(
        (after.status, after.body.as_slice()),
        (200, b"thing".as_slice())
    );

    // OPTIONS does not invent a resource: an unregistered path is still 404.
    client
        .send_request("OPTIONS", "/missing", &[], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 404);

    server.stop().await;
}

/// The `Allow` on a `405` is the field a client programs against, so it has to name the methods
/// the server actually answers, not the ones that were registered: `HEAD` is served out of the
/// `GET` handler and `OPTIONS` by the generic responder.
#[tokio::test]
async fn a_405_allow_header_names_the_methods_the_server_answers() {
    let mut app = AetherApp::new();
    app = app.get("/thing", |_| "thing");
    app = app.post("/submit", |_| "submitted");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("PUT", "/thing", &[], b"")
        .await
        .unwrap();
    let res = client.response(true).await;
    assert_eq!(res.status, 405);
    assert_eq!(res.header("Allow"), Some("GET, HEAD, OPTIONS"));

    // No `GET` bit, so there is no `HEAD` to inherit — but `OPTIONS` is still served.
    client
        .send_request("GET", "/submit", &[], b"")
        .await
        .unwrap();
    let res = client.response(true).await;
    assert_eq!(res.status, 405);
    assert_eq!(res.header("Allow"), Some("POST, OPTIONS"));

    server.stop().await;
}

/// A WebSocket route is the one place the inherited `HEAD` is wrong: the upgrade refuses every
/// non-`GET` before the handler, so `HEAD` there is a `405`. The `Allow` on both the generic
/// OPTIONS answer and that `405` must agree on it.
#[tokio::test]
async fn options_on_a_websocket_route_lists_get_without_head() {
    let mut app = AetherApp::new();
    app = app.ws("/live", |_req, _socket| async move {});

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("OPTIONS", "/live", &[], b"")
        .await
        .unwrap();
    let options = client.response(true).await;
    assert_eq!(
        (options.status, options.header("Allow")),
        (204, Some("GET, OPTIONS"))
    );

    client
        .send_request("HEAD", "/live", &[], b"")
        .await
        .unwrap();
    let head = client.response(true).await;
    assert_eq!(
        (head.status, head.header("Allow")),
        (405, Some("GET, OPTIONS"))
    );

    server.stop().await;
}

/// A registered OPTIONS route is a route: the generic responder only fills in when the router
/// reports the method as unregistered.
#[tokio::test]
async fn a_registered_options_route_wins_over_the_generic_answer() {
    let mut app = AetherApp::new();
    app = app.get("/thing", |_| "thing");
    app = app.route_sync(Method::Options, "/custom", |_| "custom-options");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("OPTIONS", "/custom", &[], b"")
        .await
        .unwrap();
    let custom = client.response(true).await;
    assert_eq!(
        (custom.status, custom.body.as_slice()),
        (200, b"custom-options".as_slice())
    );

    server.stop().await;
}

#[tokio::test]
async fn body_and_header_limits_are_enforced_with_413_and_431() {
    let body_config = AetherConfig {
        max_body_bytes: 16,
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(body_config);
    app = app.post("/echo", |req| format!("{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request("POST", "/echo", &[], &[b'x'; 20])
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 413);
    client.expect_closed().await;
    server.stop().await;

    // A small header budget must produce 431, not the connection-buffer 413 guard. Keep the
    // body budget large so the total buffered request stays under the connection limit.
    let header_config = AetherConfig {
        max_header_bytes: 64,
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(header_config);
    app = app.get("/echo", |_| "ok");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let oversized_header = "x".repeat(200);
    client
        .send_request("GET", "/echo", &[("X-Large", &oversized_header)], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 431);
    client.expect_closed().await;
    server.stop().await;
}

#[tokio::test]
async fn handler_panics_are_isolated_to_one_response() {
    let mut app = AetherApp::new();
    app = app.get("/ok", |_| "ok");
    app = app.get("/panic", |_| -> &'static str {
        panic!("intentional test panic")
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/panic", &[], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 500);

    client.send_request("GET", "/ok", &[], b"").await.unwrap();
    let ok = client.response(true).await;
    assert_eq!((ok.status, ok.body.as_slice()), (200, b"ok".as_slice()));

    server.stop().await;
}

/// The total receive deadline is its own setting: a trickle is refused by `max_receive_time`
/// while the handler budget stays at its default, so the two numbers cannot be confused again.
#[tokio::test]
async fn slow_trickle_receives_408_within_the_total_receive_deadline() {
    let config = AetherConfig {
        max_receive_time: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.get("/slow", |_| "slow");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /slow HTTP/1.1\r\nHost: test\r\n")
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let response = loop {
        if let Some(response) = client.poll_response(false).await {
            break response;
        }
        if Instant::now() > deadline {
            panic!("server never enforced the total receive deadline");
        }
        let _ = client.send(b"X").await;
        tokio::time::sleep(Duration::from_millis(40)).await;
    };
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));

    server.stop().await;
}

/// The other direction: a legitimately slow upload is judged by the receive budget alone. With a
/// 100 ms handler timeout and a 5 s receive deadline, a body trickled over ~0.6 s must be served —
/// while these shared one number, the receive phase hit the 100 ms handler budget and answered 408.
#[tokio::test]
async fn a_slow_upload_is_not_bounded_by_the_handler_timeout() {
    let config = AetherConfig {
        max_request_time: Duration::from_millis(100),
        max_receive_time: Duration::from_secs(5),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.post("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 4\r\n\r\n")
        .await
        .unwrap();

    let started = Instant::now();
    for byte in *b"abcd" {
        tokio::time::sleep(Duration::from_millis(150)).await;
        client.send(&[byte]).await.unwrap();
    }
    let response = client.response(true).await;
    assert_eq!(
        response.status, 200,
        "an upload slower than the handler budget must still be served"
    );
    assert_eq!(response.body, b"got:4");
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "the body really was trickled past the handler timeout"
    );

    server.stop().await;
}

/// PROB-0034: a request that stops arriving is a *receive* timeout, and a receive timeout is
/// answered. The budget is what expires here — 600 ms against a 5 s idle timeout — so a server
/// that closes on the idle timeout instead cannot answer inside the window this asserts, and
/// one that never answers fails the read. Shapes: a head that never finishes, a body that never
/// starts, a body that stops part way, and a chunked body that stops mid-chunk.
#[tokio::test]
async fn a_request_that_stops_arriving_is_answered_408_by_the_receive_budget() {
    let config = AetherConfig {
        max_receive_time: Duration::from_millis(600),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.post("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;
    let head = b"POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\n";
    async fn answer(client: &mut Client) -> HttpResponse {
        tokio::time::timeout(Duration::from_millis(1500), client.response(true))
            .await
            .expect("the receive budget, not the 5 s idle timeout, must bound a stopped request")
    }

    // A head that starts and never finishes: the request has begun, so it is owed an answer.
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /upload HTTP/1.1\r\nHost: test\r\n")
        .await
        .unwrap();
    let response = answer(&mut client).await;
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    // Nothing of the body yet: the client paused after a complete head.
    let mut client = Client::connect(server.addr).await;
    client.send(head).await.unwrap();
    let response = answer(&mut client).await;
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    // Three of five bytes, then silence: a partially received body is bounded the same way.
    let mut client = Client::connect(server.addr).await;
    client.send(head).await.unwrap();
    client.send(b"abc").await.unwrap();
    let response = answer(&mut client).await;
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    // The same pause against a chunked body: the clock is the receive phase, not the framing.
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"POST /upload HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    client.send(b"3\r\nabc\r\n").await.unwrap();
    let response = answer(&mut client).await;
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));
    client.expect_closed().await;

    server.stop().await;
}

/// The read that follows a late body byte waits for what is left of the budget, not for a fresh
/// one. One byte at 1.1 s of a 1.5 s budget, then silence: the answer belongs at ~1.5 s. A read
/// capped at the whole budget instead of the remainder would answer near 2.6 s, and one that
/// answers any pause at once would answer at ~1.1 s — both miss the window below.
#[tokio::test]
async fn a_stopped_body_waits_no_longer_than_the_budget_it_started_with() {
    let config = AetherConfig {
        max_receive_time: Duration::from_millis(1500),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.post("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let started = Instant::now();
    client
        .send(b"POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    client.send(b"a").await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(3), client.response(true))
        .await
        .expect("a body that pauses late is still bounded by the budget it started with");
    assert_eq!(response.status, 408);
    assert!(
        started.elapsed() >= Duration::from_millis(1400),
        "the answer waited for the budget, not for the first gap: {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_millis(2000),
        "the wait was capped by the budget's remainder, not by a fresh budget: {:?}",
        started.elapsed()
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The other clock still answers. With the default 30 s budget, a request that stops arriving
/// is bounded by the 5 s per-read idle timeout, and that bound is a receive timeout too: the
/// peer learns `408` instead of a bare FIN. A socket idle *between* keep-alive requests has no
/// request to answer and is still closed without one, which is why this path stays silent.
#[tokio::test]
async fn a_body_that_stops_arriving_is_answered_408_by_the_idle_timeout() {
    let mut app = AetherApp::new();
    app = app.post("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    let started = Instant::now();
    let deadline = started + Duration::from_secs(8);
    let response = loop {
        if let Some(response) = client.poll_response(false).await {
            break response;
        }
        assert!(
            Instant::now() < deadline,
            "server closed the connection without answering the paused body"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));
    assert!(
        started.elapsed() >= Duration::from_secs(4),
        "the default 5 s idle timeout is what bounds this wait; answering sooner would refuse \
         a client that merely paused"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The idle close between keep-alive requests is not a receive timeout: once the request was
/// served, the connection owes the peer nothing, so it closes without writing. This is the arm
/// the `408` above must not swallow.
#[tokio::test]
async fn an_idle_keep_alive_connection_is_closed_without_an_answer() {
    let config = AetherConfig {
        keep_alive_timeout: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.get("/ok", |_| "ok");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client.send_request("GET", "/ok", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);
    assert!(
        client.buffered.is_empty(),
        "the served response must not have dragged a second one with it: {:?}",
        String::from_utf8_lossy(&client.buffered)
    );

    // Nothing more will be sent: the next thing on this socket must be the FIN, not a status.
    let mut chunk = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(3), client.stream.read(&mut chunk))
        .await
        .expect("the keep-alive budget never closed the idle connection")
        .expect("read from the idle connection failed");
    assert_eq!(
        read,
        0,
        "an idle keep-alive connection must be closed, not answered: {:?}",
        String::from_utf8_lossy(&chunk[..read])
    );

    server.stop().await;
}

/// RFC 9112 §9.3.3: a client that sends `Expect: 100-continue` stops after the head and
/// waits for an interim response before sending the body. The server must answer while the
/// body is still absent, then serve the request normally once it arrives.
#[tokio::test]
async fn expect_100_continue_is_answered_before_the_body_arrives() {
    let mut app = AetherApp::new();
    app = app.put("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    // Nothing else has been sent, so this read can only be the interim response; a server
    // that never implements Expect would time out here.
    let interim = tokio::time::timeout(Duration::from_secs(5), client.response(false))
        .await
        .expect("no interim response while the body was still pending");
    assert_eq!(interim.status, 100);
    assert!(
        interim.headers.is_empty(),
        "an interim response must not carry headers: {:?}",
        interim.headers
    );

    // The expectation is answered once, not per partial read.
    assert!(
        client.poll_response(false).await.is_none(),
        "server sent a second interim response"
    );

    client.send(b"hello").await.unwrap();
    let final_response = client.response(true).await;
    assert_eq!(
        (final_response.status, final_response.body.as_slice()),
        (200, b"got:5".as_slice())
    );

    // The expectation is per request: the next keep-alive request that asks for it earns
    // its own interim response rather than inheriting the first one.
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nContent-Length: 3\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), client.response(false))
            .await
            .expect("second Expect: 100-continue was never answered")
            .status,
        100
    );
    client.send(b"abc").await.unwrap();
    let second = client.response(true).await;
    assert_eq!(
        (second.status, second.body.as_slice()),
        (200, b"got:3".as_slice())
    );

    server.stop().await;
}

/// One expectation, one answer — and the piecewise body is the only shape that proves it. The latch
/// lives in the loop that reads the body, so a client that writes every byte at once never re-enters
/// that branch with a head still pending, and a frontend that answered *per partial read* would look
/// identical to this server on a single-packet upload. Head, pause, three bytes, pause, then the
/// rest: three reads of the same complete head, and at most one `100` among them.
#[tokio::test]
async fn a_body_that_arrives_in_pieces_earns_one_interim_not_one_per_piece() {
    let mut app = AetherApp::new();
    app = app.put("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nContent-Length: 10\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), client.response(false))
            .await
            .expect("no interim response while the body was still pending")
            .status,
        100
    );

    // Three of the ten bytes: the reader needs more data and looks at the same head again.
    client.send(b"abc").await.unwrap();
    for _ in 0..8 {
        assert!(
            client.poll_response(false).await.is_none(),
            "the interim response was repeated for a body that arrived in pieces"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    client.send(b"defghij").await.unwrap();
    let served = client.response(true).await;
    assert_eq!(
        (served.status, served.body.as_slice()),
        (200, b"got:10".as_slice()),
        "the split body was not assembled"
    );

    server.stop().await;
}

/// The interim response is scoped to HTTP/1.1 expectations, and a request that already
/// arrived in full must not be preceded by one.
#[tokio::test]
async fn expect_100_continue_is_not_sent_for_10_or_a_complete_request() {
    let mut app = AetherApp::new();
    app = app.put("/upload", |req| format!("got:{}", req.body.len()));

    let server = TestServer::start(app).await;

    // HTTP/1.0 has no Expect semantics: the head must earn no answer at all, and the
    // request is served once the body arrives.
    let mut legacy = Client::connect(server.addr).await;
    legacy
        .send(b"PUT /upload HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();
    for _ in 0..8 {
        assert!(
            legacy.poll_response(false).await.is_none(),
            "server sent an interim response to an HTTP/1.0 request"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    legacy.send(b"hello").await.unwrap();
    let served = legacy.response(true).await;
    assert_eq!(
        (served.status, served.body.as_slice()),
        (200, b"got:5".as_slice())
    );

    // Head and body in one write: the request is served directly with no interim response.
    let mut whole = Client::connect(server.addr).await;
    whole
        .send_request("PUT", "/upload", &[("Expect", "100-continue")], b"hello")
        .await
        .unwrap();
    let served = whole.response(true).await;
    assert_eq!(
        (served.status, served.body.as_slice()),
        (200, b"got:5".as_slice())
    );

    server.stop().await;
}

/// A `/upload` that records every handler entry, so a refusal can be shown to have refused
/// before dispatch rather than answered only after the route had already run.
fn upload_counting_entries() -> (AetherApp, Arc<AtomicUsize>) {
    upload_counting_entries_with(AetherConfig::default())
}

fn upload_counting_entries_with(config: AetherConfig) -> (AetherApp, Arc<AtomicUsize>) {
    let ran = Arc::new(AtomicUsize::new(0));
    let mut app = AetherApp::with_config(config);
    let counter = Arc::clone(&ran);
    app = app.put("/upload", move |req| {
        counter.fetch_add(1, Ordering::SeqCst);
        format!("got:{}", req.body.len())
    });
    (app, ran)
}

/// RFC 9110 §10.1.1: an expectation-extension the server cannot meet is answered `417
/// (Expectation Failed)` and the request is not processed. The client is paused after the head, so
/// this is also the proof that the answer does not wait for a body the client was told not to send.
#[tokio::test]
async fn an_unsupported_expectation_is_refused_before_the_body_is_sent() {
    let (app, ran) = upload_counting_entries();
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    // Nothing else has been written by this client, so the only possible answer is the refusal.
    // `true` reads the declared body, so the emptiness below is the wire's `Content-Length: 0`
    // rather than the client's `expect_body = false` shortcut.
    let refusal = tokio::time::timeout(Duration::from_secs(5), client.response(true))
        .await
        .expect("no response while the client was paused on its expectation");
    assert_eq!(refusal.status, 417);
    assert_eq!(refusal.version, "HTTP/1.1");
    assert_eq!(refusal.header("Connection"), Some("close"));
    // The refusal promises nothing to read: 417 is an ordinary final answer whose whole content
    // is the status, and a body here would be bytes the client must drain before the close it was
    // told to expect. The HTTP/2 twin (`an_unsupported_expectation_is_refused_over_http2_from_the_head`)
    // asserts the same emptiness; track 116 recorded that this frontend did not, track 118 pinned it.
    assert!(
        refusal.body.is_empty(),
        "a refusal arrived with a body: {:?}",
        refusal.body
    );
    assert_eq!(refusal.header("Content-Length"), Some("0"));
    // The refusal is the last word on this connection: the rest of the stream is undecidable, so
    // the socket closes rather than carrying a second request the client may never have intended.
    client.expect_closed().await;
    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "a handler ran for a request refused on its expectation"
    );

    server.stop().await;
}

/// The `Expect` value is a list of tokens, not a substring search: `100-continue-x` names a
/// different expectation-extension, so it is refused rather than answered with the interim the
/// text inside it seems to ask for. A server that matched loosely would tell this client to send
/// a body it is about to reject.
#[tokio::test]
async fn an_expectation_token_that_merely_contains_the_supported_one_is_refused() {
    let (app, ran) = upload_counting_entries();
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue-x\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    let answer = tokio::time::timeout(Duration::from_secs(5), client.response(false))
        .await
        .expect("a near-miss expectation was never answered");
    assert_eq!(
        answer.status, 417,
        "a token containing `100-continue` was treated as that expectation"
    );
    client.expect_closed().await;
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    server.stop().await;
}

/// The refusal judges the promise, not how much of the body had arrived. A client that ignored the
/// expectation and sent everything anyway still gets `417` - and the answer still has to reach it,
/// which on Windows means the unread request bytes must not destroy the response.
#[tokio::test]
async fn an_unsupported_expectation_refuses_a_body_that_already_arrived() {
    let (app, ran) = upload_counting_entries();
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\nhello")
        .await
        .unwrap();

    let refusal = client.response(false).await;
    assert_eq!(refusal.status, 417);
    client.expect_closed().await;
    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "a handler ran for a request refused on its expectation"
    );

    server.stop().await;
}

/// A list of expectations promises *every* element will be met, so one unsupported element is
/// enough to refuse the whole request: answering the interim for the half this server supports
/// would tell the client to send a body that is about to be rejected anyway.
#[tokio::test]
async fn a_mixed_expectation_list_is_refused_rather_than_half_answered() {
    let (app, ran) = upload_counting_entries();
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue, foo=bar\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    let refusal = tokio::time::timeout(Duration::from_secs(5), client.response(false))
        .await
        .expect("no response while the client was paused on its expectation");
    assert_eq!(
        refusal.status, 417,
        "a list containing an unsupported expectation was answered with {:?}",
        refusal.status
    );
    client.expect_closed().await;
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    server.stop().await;
}

/// A promise split over two `Expect` fields is still one list (RFC 9110 §5.5 joins repeated
/// fields with commas), so the half the server cannot meet refuses the whole request. The second
/// half of this test is the other side of the same rule: two fields naming the one supported
/// expectation are one expectation, answered with exactly one interim - not two and not none.
#[tokio::test]
async fn repeated_expectation_fields_are_one_list() {
    let (app, ran) = upload_counting_entries();
    let server = TestServer::start(app).await;

    let mut split = Client::connect(server.addr).await;
    split
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();
    let refusal = tokio::time::timeout(Duration::from_secs(5), split.response(false))
        .await
        .expect("a split promise was never answered");
    assert_eq!(
        refusal.status, 417,
        "a repeated field naming an unsupported expectation was served rather than refused"
    );
    split.expect_closed().await;
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    let mut doubled = Client::connect(server.addr).await;
    doubled
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();
    let interim = tokio::time::timeout(Duration::from_secs(5), doubled.response(false))
        .await
        .expect("two fields naming the supported expectation earned no interim");
    assert_eq!(interim.status, 100);
    assert!(
        doubled.poll_response(false).await.is_none(),
        "a duplicated expectation was answered twice"
    );
    doubled.send(b"hello").await.unwrap();
    let served = doubled.response(true).await;
    assert_eq!(
        (served.status, served.body.as_slice()),
        (200, b"got:5".as_slice()),
        "a duplicated supported expectation cost the request its answer"
    );

    server.stop().await;
}

/// RFC 9110 §5.5's `#` rule makes an empty element no element at all: `Expect:` carrying nothing
/// but commas states no expectation, so nothing is owed - neither an interim nor a refusal - and
/// the request is served as if the field were absent.
#[tokio::test]
async fn an_empty_expectation_list_owes_neither_an_interim_nor_a_refusal() {
    let mut app = AetherApp::new();
    app = app.put("/upload", |req| format!("got:{}", req.body.len()));
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.1\r\nHost: test\r\nExpect: ,\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();
    for _ in 0..8 {
        assert!(
            client.poll_response(false).await.is_none(),
            "an empty expectation list was answered"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    client.send(b"hello").await.unwrap();
    let served = client.response(true).await;
    assert_eq!(
        (served.status, served.body.as_slice()),
        (200, b"got:5".as_slice())
    );

    server.stop().await;
}

/// HTTP/1.0 forbids the interim response, not the refusal: `417` is an ordinary final answer, so
/// the version rule that silences a supported expectation must not silence an unmet one.
#[tokio::test]
async fn an_unsupported_expectation_is_refused_on_http_10_too() {
    let (app, ran) = upload_counting_entries();
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /upload HTTP/1.0\r\nExpect: foo\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    let refusal = tokio::time::timeout(Duration::from_secs(5), client.response(false))
        .await
        .expect("no response while the client was paused on its expectation");
    assert_eq!(refusal.status, 417);
    client.expect_closed().await;
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    server.stop().await;
}

/// The expectation is judged while the head is being checked, so it outranks the verdict about the
/// body's size: a request this server has already declined never reaches the budget it would have
/// blown. Both refusals close the connection, so only the status distinguishes them.
#[tokio::test]
async fn an_unmet_expectation_outranks_the_body_budget_it_would_have_breached() {
    let config = AetherConfig {
        max_body_bytes: 4,
        ..AetherConfig::default()
    };
    let (app, _) = upload_counting_entries_with(config);
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request(
            "PUT",
            "/upload",
            &[("Expect", "foo=bar"), ("Content-Length", "5")],
            b"hello",
        )
        .await
        .unwrap();
    let refusal = client.response(false).await;
    assert_eq!(
        refusal.status, 417,
        "an over-budget body was judged before the expectation it was framed with"
    );

    // The same request with a met expectation reaches the budget verdict instead.
    let mut sized = Client::connect(server.addr).await;
    sized
        .send_request(
            "PUT",
            "/upload",
            &[("Expect", "100-continue"), ("Content-Length", "5")],
            b"hello",
        )
        .await
        .unwrap();
    let over_budget = sized.response(false).await;
    assert_eq!(over_budget.status, 413);

    server.stop().await;
}

/// A WebSocket upgrade is a request like any other, and the `101` it asks for is a promise about a
/// protocol this server has not agreed to yet. An unmet expectation refuses the request before that
/// agreement, so no upgrade is built on a promise the server broke.
#[tokio::test]
async fn a_websocket_upgrade_cannot_be_built_on_an_unmet_expectation() {
    let mut app = AetherApp::new();
    app = app.ws("/ws", |_req, mut socket| async move {
        while let Ok(Some(message)) = socket.recv().await {
            match message {
                WsMessage::Text(text) => {
                    if socket.send_text(&text).await.is_err() {
                        break;
                    }
                }
                WsMessage::Binary(bytes) => {
                    if socket.send_binary(&bytes).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nExpect: foo\r\nUpgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();

    let refusal = client.response(false).await;
    assert_eq!(
        refusal.status, 417,
        "an upgrade was accepted over an expectation the server could not meet"
    );
    assert_ne!(refusal.header("Upgrade"), Some("websocket"));
    client.expect_closed().await;

    server.stop().await;
}

/// The interim answer is not routed: a client holding a supported expectation is waiting on the
/// server's willingness to take the body, and that is decided before the path is looked up. The
/// `404` that follows is the route's own answer, delivered after the body arrived.
#[tokio::test]
async fn a_supported_expectation_is_answered_even_on_a_path_with_no_route() {
    let mut app = AetherApp::new();
    app = app.put("/upload", |req| format!("got:{}", req.body.len()));
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"PUT /missing HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n")
        .await
        .unwrap();

    let interim = tokio::time::timeout(Duration::from_secs(5), client.response(false))
        .await
        .expect("no interim response while the body was still pending");
    assert_eq!(interim.status, 100);
    client.send(b"hello").await.unwrap();
    assert_eq!(client.response(true).await.status, 404);

    server.stop().await;
}

/// A chunked body split across reads must be decoded from where each read stopped: the
/// connection keeps one decoder, so the payload cannot be lost, doubled, or leaked into the
/// next request on the same connection.
#[tokio::test]
async fn chunked_body_split_across_reads_is_decoded_once_per_connection() {
    let mut app = AetherApp::new();
    app = app.post("/upload", |req| {
        String::from_utf8_lossy(&req.body).into_owned()
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    let head = b"POST /upload HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n";
    // Split so a read boundary falls inside a size line, inside a chunk's payload, and
    // between the payload and its trailing CRLF.
    client.send(head).await.unwrap();
    for piece in [
        "7", "\r\nAaa", "BBX", "x", "\r\n", "4\r", "\nyyyy\r", "\n0\r\n", "\r\n",
    ] {
        client.send(piece.as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let first = client.response(true).await;
    assert_eq!(
        (first.status, first.body.as_slice()),
        (200, b"AaaBBXxyyyy".as_slice())
    );

    // Second chunked request on the same connection, arriving whole.
    client
        .send(
            b"POST /upload HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nzzz\r\n0\r\n\r\n",
        )
        .await
        .unwrap();
    let second = client.response(true).await;
    assert_eq!(
        (second.status, second.body.as_slice()),
        (200, b"zzz".as_slice())
    );

    // A chunked request followed by a pipelined one in the same write stays framed.
    client
        .send(
            b"POST /upload HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nqq\r\n0\r\n\r\nGET /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n",
        )
        .await
        .unwrap();
    let third = client.response(true).await;
    assert_eq!(
        (third.status, third.body.as_slice()),
        (200, b"qq".as_slice())
    );
    let fourth = client.response(false).await;
    assert_eq!(
        fourth.status, 405,
        "GET on a POST-only route is method-not-allowed"
    );

    server.stop().await;
}

#[tokio::test]
async fn graceful_shutdown_stops_accepting_and_drains_keep_alive() {
    let mut app = AetherApp::new();
    app = app.get("/", |_| "alive");

    let server = TestServer::start(app).await;
    let addr = server.addr;
    let mut client = Client::connect(addr).await;
    client.send_request("GET", "/", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);

    // The idle keep-alive connection is closed when shutdown starts, and the accept loop
    // finishes rather than waiting for the read timeout.
    let shutdown = tokio::spawn(async move { server.stop().await });
    tokio::time::timeout(Duration::from_secs(5), client.expect_closed())
        .await
        .expect("idle keep-alive connection was not closed on shutdown");
    shutdown.await.expect("shutdown task panicked");

    let refused = TcpStream::connect(addr).await;
    assert!(
        refused.is_err(),
        "a connection was accepted after graceful shutdown"
    );
}

#[test]
fn static_range_parser_contract() {
    use aether_http::static_files::{RangeDecision, parse_range_header};
    assert_eq!(
        parse_range_header("bytes=1-3", 10),
        RangeDecision::Satisfiable { start: 1, end: 3 }
    );
    assert_eq!(
        parse_range_header("bytes=99-", 10),
        RangeDecision::Unsatisfiable
    );
    assert_eq!(
        parse_range_header("bytes=0-1,4-5", 10),
        RangeDecision::Ignore
    );
}

#[tokio::test]
async fn websocket_upgrade_echo_fragmentation_ping_and_close() {
    let mut app = AetherApp::new();
    app = app.ws("/ws/echo", |_req, mut socket| async move {
        while let Ok(Some(message)) = socket.recv().await {
            let sent = match message {
                WsMessage::Text(text) => socket.send_text(&text).await,
                WsMessage::Binary(bytes) => socket.send_binary(&bytes).await,
            };
            if sent.is_err() {
                break;
            }
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // RFC 6455 §1.3 sample key: the accept value is the published test vector.
    client
        .send(
            b"GET /ws/echo HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let upgrade = client.response(false).await;
    assert_eq!(upgrade.status, 101);
    assert!(
        upgrade
            .header("Upgrade")
            .unwrap()
            .eq_ignore_ascii_case("websocket")
    );
    assert!(
        upgrade
            .header("Connection")
            .unwrap()
            .eq_ignore_ascii_case("upgrade")
    );
    assert_eq!(
        upgrade.header("Sec-WebSocket-Accept"),
        Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    );
    assert!(
        upgrade.header("Content-Length").is_none(),
        "101 must not be framed as a body"
    );

    client
        .send(&ws_client_frame(true, 0x1, b"hello", [1, 2, 3, 4]))
        .await
        .unwrap();
    let (opcode, fin, payload) = client.read_ws_frame().await;
    assert_eq!((opcode, fin), (0x1, true));
    assert_eq!(payload, b"hello");

    // Control frame interleaved with data is answered by the driver, not the handler.
    client
        .send(&ws_client_frame(true, 0x9, b"ping!", [5, 6, 7, 8]))
        .await
        .unwrap();
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0xA);
    assert_eq!(payload, b"ping!");

    // Fragmented text is reassembled before the handler sees it.
    client
        .send(&ws_client_frame(false, 0x1, b"frag", [1, 1, 1, 1]))
        .await
        .unwrap();
    client
        .send(&ws_client_frame(true, 0x0, b"mented", [2, 2, 2, 2]))
        .await
        .unwrap();
    let (opcode, fin, payload) = client.read_ws_frame().await;
    assert_eq!((opcode, fin), (0x1, true));
    assert_eq!(payload, b"fragmented");

    // Close is echoed and the TCP connection is released.
    let mut close_payload = 1000u16.to_be_bytes().to_vec();
    close_payload.extend_from_slice(b"done");
    client
        .send(&ws_client_frame(true, 0x8, &close_payload, [3, 3, 3, 3]))
        .await
        .unwrap();
    let (opcode, _, echoed) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x8);
    assert_eq!(echoed, close_payload);
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn websocket_handshake_failures_and_unmasked_frames_are_rejected() {
    let mut app = AetherApp::new();
    app = app.ws("/ws", |_req, mut socket| async move {
        while let Ok(Some(WsMessage::Text(text))) = socket.recv().await {
            if socket.send_text(&text).await.is_err() {
                break;
            }
        }
    });

    let server = TestServer::start(app).await;

    // Missing Sec-WebSocket-Key.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 400);

    // Unsupported version advertises the only supported version.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 8\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 426);
    assert_eq!(response.header("Sec-WebSocket-Version"), Some("13"));

    // The route is GET-only; other methods are never upgraded.
    let mut client = Client::connect(server.addr).await;
    client.send_request("POST", "/ws", &[], b"").await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 405);
    assert!(response.header("Allow").unwrap().contains("GET"));

    // RFC 6455 §5.1: client frames must be masked. The server closes with 1002.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    client.send(&[0x81, 0x02, b'h', b'i']).await.unwrap();
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x8);
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1002);
    client.expect_closed().await;

    server.stop().await;
}

/// An upgrade needs a request version that defines upgrades.
///
/// `Upgrade` is an HTTP/1.1 mechanism (RFC 9110 §6.7), and the handshake the upgrade path writes
/// is a hardcoded `HTTP/1.1 101 Switching Protocols`. Accepting a 1.0 request would answer a
/// client with a status line naming a version it never sent, on a connection it already treats as
/// one response long. The frontend used to assert `allows_upgrade: true` for every head it parsed.
#[tokio::test]
async fn an_http10_upgrade_request_is_refused_instead_of_switching_protocols() {
    static SESSIONS: AtomicUsize = AtomicUsize::new(0);
    SESSIONS.store(0, Ordering::Relaxed);

    let mut app = AetherApp::new();
    app = app.ws("/ws", |_req, mut socket| async move {
        SESSIONS.fetch_add(1, Ordering::SeqCst);
        let _ = socket.recv().await;
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.0\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(
        response.version, "HTTP/1.0",
        "the rejection must be framed for the version the client used"
    );
    assert_eq!(
        response.status, 400,
        "a 1.0 request cannot ask to switch protocol"
    );
    assert_eq!(
        SESSIONS.load(Ordering::SeqCst),
        0,
        "the handler would own a socket whose client still speaks HTTP/1.0"
    );
    client.expect_closed().await;

    // The same head over 1.1 does upgrade, so the refusal above is the version and nothing else.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    for _ in 0..100 {
        if SESSIONS.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        SESSIONS.load(Ordering::SeqCst),
        1,
        "the 1.1 upgrade must reach the handler, or the 400 above proves nothing about versions"
    );
    drop(client);

    server.stop().await;
}

#[tokio::test]
async fn error_responses_survive_unread_pipelined_bytes() {
    // Regression: closing an error-path socket without a graceful FIN makes
    // Windows send an RST over the client's still-unread request bytes, which
    // discards the 431/400 response that was just written. The client must
    // actually receive the error status line, not a connection reset.
    let header_config = AetherConfig {
        max_header_bytes: 4096,
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(header_config);
    app = app.get("/echo", |_| "ok");

    let server = TestServer::start(app).await;
    let mut stream = TcpStream::connect(server.addr).await.expect("connect");
    let mut payload = b"GET /echo HTTP/1.1\r\nHost: x\r\nX-Big: ".to_vec();
    payload.extend_from_slice(&[b'A'; 20_000]);
    payload.extend_from_slice(b"\r\n\r\n");
    stream.write_all(&payload).await.unwrap();
    // Let the server parse, respond and close WITHOUT us reading, so request
    // bytes are still sitting in its receive buffer at close time.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await;
    server.stop().await;
    match read {
        Ok(Ok(_)) => {
            let text = String::from_utf8_lossy(&buf);
            let head = &text[..text.len().min(64)];
            assert!(
                text.starts_with("HTTP/1.1 431"),
                "expected 431, got {head:?}"
            );
        }
        Ok(Err(err)) => panic!("connection was reset instead of closing with FIN: {err}"),
        Err(_) => panic!("no response and no close within timeout"),
    }
}

/// The connection-buffer guard refuses a request once the **wire** bytes it has to hold pass
/// `max_header_bytes + max_body_bytes`, and a chunked body pays 6 wire bytes per payload byte in
/// one-byte chunks. A client that is still mid-upload when that happens has megabytes queued, so
/// writing the 413 and closing is not enough: the refusal leaves those bytes unread in the
/// receive queue, and Windows resets the connection over them — the client then reads no status
/// line at all for a request the server had already decided about. So the refusal drains what the
/// client has already queued (discarding it — the bound is about memory, and discarding into a
/// fixed 8 KiB buffer grows nothing) under the same read timeout the receive loop already uses,
/// and only then closes with FIN.
#[tokio::test]
async fn an_over_budget_chunked_upload_is_still_answered_with_413() {
    static HITS: AtomicUsize = AtomicUsize::new(0);
    HITS.store(0, Ordering::Relaxed);

    let mut app = AetherApp::new();
    app = app.post("/ingest", |_| {
        HITS.fetch_add(1, Ordering::Relaxed);
        "stored"
    });

    let server = TestServer::start(app).await;
    let stream = TcpStream::connect(server.addr).await.expect("connect");
    let (mut reader, mut writer) = stream.into_split();

    // 400_000 payload bytes in one-byte chunks is 2,400,000 framing-and-payload wire bytes,
    // past the 2,105,344-byte default connection budget — the size track 69 measured as
    // undeliverable (Windows os error 10053 after ~135-150 ms).
    let body = tokio::spawn(async move {
        let mut wire: Vec<u8> = b"POST /ingest HTTP/1.1\r\nHost: x\r\n\
                                  Transfer-Encoding: chunked\r\n\r\n"
            .to_vec();
        wire.reserve(2_400_064);
        for _ in 0..400_000 {
            wire.extend_from_slice(b"1\r\nx\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
        let sent = writer.write_all(&wire).await;
        let _ = writer.shutdown().await;
        sent
    });

    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut buf)).await;
    let sent = body.await.expect("upload task did not panic");

    match read {
        Ok(Ok(_)) => {
            let text = String::from_utf8_lossy(&buf);
            let head = &text[..text.len().min(64)];
            assert!(
                text.starts_with("HTTP/1.1 413 Content Too Large"),
                "expected RFC 9110's 413 phrase, got {head:?}"
            );
            assert!(
                text.contains("connection: close") || text.contains("Connection: close"),
                "an over-budget refusal must not invite keep-alive: {head:?}"
            );
        }
        Ok(Err(err)) => panic!(
            "the refusal reset the connection instead of delivering its 413: {err} \
             (upload write: {sent:?})"
        ),
        Err(_) => panic!("no response and no close within 5s"),
    }
    assert_eq!(
        HITS.load(Ordering::Relaxed),
        0,
        "a refused upload must never reach the handler"
    );

    server.stop().await;
}

/// Which byte the connection guard counts is decided here, one payload byte apart.
///
/// The guard compares *wire* bytes against `max_header_bytes + max_body_bytes` (8 192 + 2 097 152 =
/// 2 105 344 by default) while the parser compares the *decoded* body against `max_body_bytes`, and
/// a chunked body pays framing: this request's head is 62 bytes and a one-byte chunk costs 6 wire
/// bytes (`1\r\nx\r\n`), so 350 879 payload bytes are 2 105 341 on the wire and are served, while
/// 350 880 are 2 105 347 and are refused. The refused request's decoded body is 350 880 bytes —
/// 16.7 % of the 2 MiB the `max_body_bytes` field is named after, and a fifth of what the parser
/// would have accepted — which is what makes the refusal about framing and not about the body.
///
/// Both halves matter: the guard is correct memory pricing (a `BytesMut` holding 2.1 MiB of wire
/// costs 2.1 MiB whether or not the bytes inside it are payload), and the label was what was wrong.
/// This test is the falsification of the label, and it was run as one — disabling the guard's wire
/// comparison answers the second request 200, because the parser's decoded 2 097 152 would have taken
/// it; moving `FITS` to 350 880 fails the first half with a 413, and moving it back to 350 873 fails
/// the second with a 200. Rename the field to say "wire" and this pair is exactly the sentence that
/// rename has to describe.
#[tokio::test]
async fn a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes() {
    const FITS: usize = 350_879;
    fn upload(payload: usize) -> Vec<u8> {
        let mut wire: Vec<u8> = b"POST /ingest HTTP/1.1\r\nHost: x\r\n\
                                  Transfer-Encoding: chunked\r\n\r\n"
            .to_vec();
        wire.reserve(6 * payload + 8);
        for _ in 0..payload {
            wire.extend_from_slice(b"1\r\nx\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
        wire
    }

    let mut app = AetherApp::new();
    app = app.post("/ingest", |req| req.body.len().to_string());
    let server = TestServer::start(app).await;

    // Inside by three wire bytes: served, with the whole decoded body in the handler's hands.
    let mut client = Client::connect(server.addr).await;
    let sent = client.send(&upload(FITS)).await;
    let _ = client.stream.shutdown().await;
    let served = client.response(true).await;
    assert_eq!(
        served.status, 200,
        "a 2 105 341-byte upload fits the 2 105 344-byte budget (write: {sent:?})"
    );
    assert_eq!(
        String::from_utf8_lossy(&served.body),
        FITS.to_string(),
        "the handler must have the complete decoded body, not a truncated one"
    );

    // One payload byte further: the same decoder would have accepted it, the socket budget did not.
    let mut client = Client::connect(server.addr).await;
    let sent = client.send(&upload(FITS + 1)).await;
    let _ = client.stream.shutdown().await;
    let refused = client.response(false).await;
    assert_eq!(
        refused.status, 413,
        "2 105 347 wire bytes pass the budget by 3, though the body is 350 880 of 2 097 152 \
         (write: {sent:?})"
    );

    server.stop().await;
}

/// The FIN goes out *before* the socket drains, not after it. A client that has been refused and
/// then keeps its own half open — every real client that is still pushing bytes, or one that simply
/// waits before closing — must not have to wait for the server's drain to time out to learn that
/// the exchange ended. So the refusal is answered, the write half is closed, and only the client's
/// already-queued bytes are discarded afterwards.
///
/// The deadline is the instrument: 2 s is well inside the 5 s `read_timeout` that bounds the
/// discard, so a refusal that FINs after draining cannot pass this, and the fixed path lands in
/// milliseconds.
#[tokio::test]
async fn a_buffer_limit_refusal_finishes_the_exchange_without_waiting_for_the_client() {
    let mut app = AetherApp::new();
    app = app.post("/ingest", |req| req.body.len().to_string());

    let server = TestServer::start(app).await;
    let (mut reader, mut writer) = TcpStream::connect(server.addr)
        .await
        .expect("connect")
        .into_split();

    let body = tokio::spawn(async move {
        let mut wire: Vec<u8> = b"POST /ingest HTTP/1.1\r\nHost: x\r\n\
                                  Transfer-Encoding: chunked\r\n\r\n"
            .to_vec();
        wire.reserve(2_400_064);
        for _ in 0..400_000 {
            wire.extend_from_slice(b"1\r\nx\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
        let sent = writer.write_all(&wire).await;
        // Deliberately no `shutdown()` here and no drop: this client keeps its half open after
        // being refused, which is the case the ordering exists for.
        (sent, writer)
    });

    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), reader.read_to_end(&mut buf)).await;
    let (sent, held) = body.await.expect("upload task did not panic");

    assert!(
        read.is_ok(),
        "the client read its bytes but was never told the exchange was over \
         (upload write: {sent:?})"
    );
    let text = String::from_utf8_lossy(&buf);
    let head = &text[..text.len().min(64)];
    assert!(
        text.starts_with("HTTP/1.1 413"),
        "expected a 413 status line, got {head:?}"
    );

    drop(held);
    server.stop().await;
}

/// `enable_host_validation` refuses before the handler: the rule is decided before dispatch, so
/// the handler must not run, and the rejection body must not echo the hostile value back to the
/// client.
#[tokio::test]
async fn host_validation_refuses_an_off_list_host_before_the_handler() {
    static HITS: AtomicUsize = AtomicUsize::new(0);
    HITS.store(0, Ordering::Relaxed);

    let mut app = AetherApp::new();
    app = app.enable_host_validation(["good.com"]);
    app = app.get("/ping", |_| {
        HITS.fetch_add(1, Ordering::Relaxed);
        "pong"
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send(b"GET /ping HTTP/1.1\r\nHost: evil.com\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 400);
    assert!(
        !resp.body.windows(6).any(|w| w == b"evil.c"),
        "the rejected host was reflected back: {:?}",
        String::from_utf8_lossy(&resp.body)
    );
    assert_eq!(
        HITS.load(Ordering::Relaxed),
        0,
        "a refused request must never reach the handler"
    );

    // The allow-list is not over-broad either: the accepted name still serves, and the
    // handler runs exactly once for it.
    let mut ok = Client::connect(server.addr).await;
    ok.send(b"GET /ping HTTP/1.1\r\nHost: good.com\r\n\r\n")
        .await
        .unwrap();
    let resp = ok.response(true).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"pong");
    assert_eq!(HITS.load(Ordering::Relaxed), 1);

    server.stop().await;
}

/// Behind a proxy the limiter must key on the forwarded caller, or every client shares one bucket.
#[tokio::test]
async fn a_trusted_proxy_gives_each_forwarded_client_its_own_budget() {
    let mut app = AetherApp::new();
    app = app.enable_rate_limit(RateLimitConfig {
        burst: 1,
        per: Duration::from_secs(60),
        max_clients: 16,
    });
    app = app.enable_trusted_proxies(["127.0.0.1/32"]);
    app = app.get("/ping", |_| "pong");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/ping", &[("x-forwarded-for", "198.51.100.7")], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 200);

    // The same forwarded client is over its budget, although the socket is unchanged: the bucket
    // belongs to the identity the chain resolved, not to the connection.
    client
        .send_request("GET", "/ping", &[("x-forwarded-for", "198.51.100.7")], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 429);

    // A different forwarded client on the same socket has its own bucket.
    client
        .send_request("GET", "/ping", &[("x-forwarded-for", "198.51.100.8")], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 200);

    server.stop().await;
}

/// A claim the framework never had reason to believe buys no extra budget.
#[tokio::test]
async fn an_untrusted_peer_cannot_name_its_own_bucket() {
    // Two shapes of "untrusted": no list at all, and a list that does not contain this peer.
    let cases: [(&str, Option<[&str; 1]>); 2] = [
        ("no proxy list", None),
        ("peer outside the list", Some(["10.0.0.0/8"])),
    ];
    for (case, proxies) in cases {
        let mut app = AetherApp::new();
        app = app.enable_rate_limit(RateLimitConfig {
            burst: 1,
            per: Duration::from_secs(60),
            max_clients: 16,
        });
        if let Some(proxies) = proxies {
            app = app.enable_trusted_proxies(proxies);
        }
        app = app.get("/ping", |_| "pong");

        let server = TestServer::start(app).await;
        let mut client = Client::connect(server.addr).await;

        client
            .send_request("GET", "/ping", &[("x-forwarded-for", "198.51.100.7")], b"")
            .await
            .unwrap();
        assert_eq!(client.response(true).await.status, 200, "{case}");
        client
            .send_request("GET", "/ping", &[("x-forwarded-for", "198.51.100.8")], b"")
            .await
            .unwrap();
        assert_eq!(
            client.response(true).await.status,
            429,
            "{case}: a forwarded claim from an untrusted peer must not buy a second bucket"
        );

        server.stop().await;
    }
}

/// The identity a handler reads is the resolved caller; `peer_addr` keeps naming the socket.
#[tokio::test]
async fn the_forwarding_chain_resolves_to_what_the_handler_sees() {
    let mut app = AetherApp::new();
    app = app.enable_trusted_proxies(["127.0.0.1/32"]);
    app = app.get("/who", |req: RequestContext| {
        Response::json(Bytes::from(
            json!({
                "client": req.client_addr().map(|addr| addr.to_string()),
                "peer": req.peer_addr().map(|peer| peer.ip().to_string()),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // A forged left entry cannot win: the honest proxy's right-hand entry decides first.
    client
        .send_request(
            "GET",
            "/who",
            &[("x-forwarded-for", "6.6.6.6, 198.51.100.7")],
            b"",
        )
        .await
        .unwrap();
    let body = client.response(true).await.json();
    assert_eq!(body["client"], "198.51.100.7");
    assert_eq!(body["peer"], "127.0.0.1");

    // `Forwarded` wins when both fields are present, rather than the two being mixed.
    client
        .send_request(
            "GET",
            "/who",
            &[
                ("forwarded", "for=192.0.2.10;proto=https"),
                ("x-forwarded-for", "203.0.113.9"),
            ],
            b"",
        )
        .await
        .unwrap();
    let body = client.response(true).await.json();
    assert_eq!(body["client"], "192.0.2.10");

    // A malformed element stops the walk: nothing to its left is believed.
    client
        .send_request(
            "GET",
            "/who",
            &[("x-forwarded-for", "6.6.6.6, garbage")],
            b"",
        )
        .await
        .unwrap();
    let body = client.response(true).await.json();
    assert_eq!(body["client"], "127.0.0.1");

    // A port-carrying hop resolves to its address.
    client
        .send_request(
            "GET",
            "/who",
            &[("x-forwarded-for", "198.51.100.9:47011")],
            b"",
        )
        .await
        .unwrap();
    let body = client.response(true).await.json();
    assert_eq!(body["client"], "198.51.100.9");

    // Repeated field lines are one value in wire order (RFC 9110 §5.2), so a line the client
    // wrote itself cannot outrank the line the proxy appended after it.
    client
        .send_request(
            "GET",
            "/who",
            &[
                ("x-forwarded-for", "6.6.6.6"),
                ("x-forwarded-for", "198.51.100.11"),
            ],
            b"",
        )
        .await
        .unwrap();
    let body = client.response(true).await.json();
    assert_eq!(body["client"], "198.51.100.11");

    client
        .send_request(
            "GET",
            "/who",
            &[
                ("forwarded", "for=6.6.6.6"),
                ("forwarded", "for=198.51.100.12"),
            ],
            b"",
        )
        .await
        .unwrap();
    let body = client.response(true).await.json();
    assert_eq!(body["client"], "198.51.100.12");

    server.stop().await;
}

/// A client over its budget gets 429 with a real `Retry-After`, and the connection survives
/// the refusal — a rate limit is not a disconnect.
#[tokio::test]
async fn rate_limit_answers_429_with_retry_after_and_keeps_the_connection() {
    let mut app = AetherApp::new();
    app = app.enable_rate_limit(RateLimitConfig {
        burst: 2,
        per: Duration::from_secs(60),
        max_clients: 16,
    });
    app = app.get("/ping", |_| "pong");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    for _ in 0..2 {
        client.send_request("GET", "/ping", &[], b"").await.unwrap();
        let resp = client.response(true).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"pong");
    }

    client.send_request("GET", "/ping", &[], b"").await.unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 429);
    let retry_after = resp.header("Retry-After").map(|v| v.parse::<u64>());
    assert!(
        matches!(retry_after, Some(Ok(secs)) if secs >= 1),
        "Retry-After must be a real interval, got {:?}",
        resp.header("Retry-After")
    );

    // Still the same connection: a further request is answered, not dropped.
    client.send_request("GET", "/ping", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 429);

    server.stop().await;
}

/// The counters a metrics route would read have to move with what the wire actually saw.
#[tokio::test]
async fn rate_limit_counters_track_every_refusal() {
    let mut app = AetherApp::new();
    app = app.enable_rate_limit(RateLimitConfig {
        burst: 1,
        per: Duration::from_secs(60),
        max_clients: 16,
    });
    app = app.get("/ping", |_| "pong");
    let limiter = app
        .rate_limiter()
        .expect("the limiter handle is taken before serve");
    assert_eq!(limiter.counters().limited, 0);

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    for _ in 0..3 {
        client.send_request("GET", "/ping", &[], b"").await.unwrap();
        let _ = client.response(true).await;
    }
    assert_eq!(
        limiter.counters().limited,
        2,
        "three requests, one token, two refusals"
    );

    server.stop().await;
}

/// Both middlewares are opt-in: with neither enabled the loop must behave exactly as before,
/// including a client that sends no `Host` at all.
#[tokio::test]
async fn request_context_exposes_the_tcp_peer_addr() {
    let mut app = AetherApp::new();
    app = app.get("/who", |req| {
        req.peer_addr()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| "none".to_string())
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client.send_request("GET", "/who", &[], b"").await.unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        String::from_utf8_lossy(&resp.body),
        "127.0.0.1",
        "the request context must carry the accepted peer, not a placeholder"
    );

    server.stop().await;
}

/// The shape of value an auth middleware would stamp: a per-request, non-`Clone` wrapper.
struct WsMarker(String);

/// **PROB-0017**, found while wiring request-scoped extensions. `app.ws` used to register into a
/// handler map that `wrap_with_middleware` never touched, and the core answered an upgrade by
/// returning a `WebSocketUpgrade` outcome instead of dispatching a chain — so user middleware
/// did not run for a WebSocket route at all. An auth chain guarded every HTTP route and left
/// every `app.ws` route wide open, and a middleware-inserted extension never reached the handler.
///
/// A WebSocket route now goes through the same chain as an HTTP route. The HTTP control route on
/// the same server is asserted alongside it, so a middleware that quietly stopped running cannot
/// make both halves pass by returning the same wrong answer.
#[tokio::test]
async fn a_websocket_upgrade_runs_the_app_middleware_chain() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        req.insert_extension(WsMarker("stamped".to_string()));
        next(req).await
    });
    app = app.get("/who", |req: RequestContext| {
        req.extension::<WsMarker>()
            .map(|marker| marker.0.as_str())
            .unwrap_or("absent")
            .to_string()
    });
    app = app.ws("/ws/who", |req, mut socket| async move {
        let seen = req
            .extension::<WsMarker>()
            .map(|marker| marker.0.clone())
            .unwrap_or_else(|| "absent".to_string());
        let _ = socket.send_text(&seen).await;
    });

    let server = TestServer::start(app).await;

    let mut http = Client::connect(server.addr).await;
    http.send_request("GET", "/who", &[], b"").await.unwrap();
    let served = http.response(true).await;
    assert_eq!(served.status, 200);
    assert_eq!(
        String::from_utf8_lossy(&served.body),
        "stamped",
        "the control route must prove the middleware works on this server"
    );

    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x1);
    assert_eq!(
        payload, b"stamped",
        "an extension inserted by middleware must reach a WebSocket handler"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The other half of "middleware runs": the power to refuse. A middleware that answers without
/// calling `next` must produce that answer instead of a `101`, and the handler must never start.
#[tokio::test]
async fn a_middleware_can_refuse_a_websocket_upgrade() {
    let started = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&started);
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.header("authorization").is_some() {
            next(req).await
        } else {
            Response::status(401, "Unauthorized")
        }
    });
    app = app.get("/ping", |_req: RequestContext| "pong".to_string());
    app = app.ws("/ws/who", move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket.send_text("handler-ran").await;
        }
    });

    let server = TestServer::start(app).await;

    let mut refused = Client::connect(server.addr).await;
    refused
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let denied = refused.response(true).await;
    assert_eq!(
        denied.status, 401,
        "a refusing middleware must answer the upgrade request, not upgrade it"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "the WebSocket handler must not start once a middleware refused the upgrade"
    );

    // Refusing must not damage the connection either: an upgrade request that gets a status
    // answer is still an ordinary HTTP/1.1 request on a keep-alive socket.
    refused
        .send(b"GET /ping HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer letmein\r\n\r\n")
        .await
        .unwrap();
    let after = refused.response(true).await;
    assert_eq!(after.status, 200);
    assert_eq!(
        String::from_utf8_lossy(&after.body),
        "pong",
        "the refused upgrade must leave the connection usable"
    );

    let mut allowed = Client::connect(server.addr).await;
    allowed
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Authorization: Bearer letmein\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(allowed.response(false).await.status, 101);
    let (opcode, _, payload) = allowed.read_ws_frame().await;
    assert_eq!((opcode, payload.as_slice()), (0x1u8, &b"handler-ran"[..]));

    server.stop().await;
}

/// The refusal does not have to come before `next`. A guard that awaits the route and then
/// answers anyway — the shape of a guard that re-validates a session once the call returns —
/// must still win over the terminal's `101`, and the socket must stay unowned.
#[tokio::test]
async fn a_middleware_that_answers_after_next_refuses_the_upgrade() {
    let started = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&started);
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.header("x-late-check").is_some() {
            let _upgrade = next(req).await;
            return Response::status(403, "Forbidden");
        }
        next(req).await
    });
    app = app.get("/ping", |_req: RequestContext| "pong".to_string());
    app = app.ws("/ws/who", move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket.send_text("handler-ran").await;
        }
    });

    let server = TestServer::start(app).await;

    let mut refused = Client::connect(server.addr).await;
    refused
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-late-check: 1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let denied = refused.response(true).await;
    assert_eq!(
        denied.status, 403,
        "a guard that answers after `next` replaces the `101` sentinel, so its answer is served"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "a refused-after-handoff upgrade must not start the WebSocket session"
    );
    refused
        .send(b"GET /ping HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let after = refused.response(true).await;
    assert_eq!(
        after.status, 200,
        "the refused upgrade must leave the connection usable"
    );

    let mut allowed = Client::connect(server.addr).await;
    allowed
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(allowed.response(false).await.status, 101);
    let (opcode, _, payload) = allowed.read_ws_frame().await;
    assert_eq!((opcode, payload.as_slice()), (0x1u8, &b"handler-ran"[..]));

    server.stop().await;
}

/// `request_timeout` covers the whole chain, including the part after the terminal handed the
/// request over. A guard that hangs on the way back out must produce the same 504 + close an
/// HTTP route gets — never a `101` followed by a socket nobody owns.
#[tokio::test]
async fn a_middleware_hanging_after_next_times_the_upgrade_request_out() {
    let config = AetherConfig {
        max_request_time: Duration::from_millis(150),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let response = next(req).await;
        if response.status == 101 {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        response
    });
    app = app.ws("/ws/who", |_req, mut socket| async move {
        let _ = socket.send_text("handler-ran").await;
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let timed_out = client.response(true).await;
    assert_eq!(
        timed_out.status, 504,
        "a guard that stalls after the handoff must time out as a status, not upgrade"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// `Next` hands over a `BoxFutureResponse` that is `Send + 'static`, so a middleware *can* run
/// the rest of the chain in another task. That task still reaches the terminal and records the
/// handoff, which is exactly the shape a flag-based handover used to mistake for a granted
/// upgrade. The guard's own answer must win, the forked `101` must be discarded, and no socket
/// may be handed to a session nobody is going to drive.
#[tokio::test]
async fn a_middleware_that_forks_the_chain_still_answers_the_upgrade_request() {
    let started = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&started);
    let forked_status = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&forked_status);
    let mut app = AetherApp::new();
    app = app.use_middleware(move |req: RequestContext, next: Next| {
        let observed = Arc::clone(&observed);
        async move {
            if req.header("x-fork").is_some() {
                // Awaiting the join handle keeps the race deterministic: the terminal really does
                // run, in another task, before the guard answers.
                let status = tokio::spawn(next(req))
                    .await
                    .expect("the forked chain completes")
                    .status;
                observed.store(status as usize, Ordering::SeqCst);
                return Response::status(202, "Accepted");
            }
            next(req).await
        }
    });
    app = app.get("/ping", |_req: RequestContext| "pong".to_string());
    app = app.ws("/ws/who", move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket.send_text("handler-ran").await;
        }
    });

    let server = TestServer::start(app).await;

    let mut forked = Client::connect(server.addr).await;
    forked
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-fork: 1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answered = forked.response(true).await;
    assert_eq!(
        forked_status.load(Ordering::SeqCst),
        101,
        "the forked chain must really have reached the terminal, otherwise this proves nothing"
    );
    assert_eq!(
        answered.status, 202,
        "the guard's answer is served even though the chain completed with a 101"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "a forked chain that the guard overrode must not start the session"
    );
    forked
        .send(b"GET /ping HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(forked.response(true).await.status, 200);

    server.stop().await;
}

/// Constructing the chain future and dropping it is the other half of the `select` shape. Note
/// what the measurement actually shows: calling `next` records the request *synchronously*, so the
/// handover is not refused because the terminal was unreachable — it is refused because the status
/// the chain returns is the guard's own `503`, not the terminal's `101`. Falsified: with the status
/// guard replaced by `.filter(|_| true)`, this test fails `left: 101, right: 503`.
#[tokio::test]
async fn a_middleware_that_abandons_the_chain_future_refuses_the_upgrade() {
    let started = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&started);
    let mut app = AetherApp::new();
    app = app.use_middleware(move |req: RequestContext, next: Next| async move {
        if req.header("x-abandon").is_some() {
            let never_polled = next(req);
            drop(never_polled);
            return Response::status(503, "Service Unavailable");
        }
        next(req).await
    });
    app = app.ws("/ws/who", move |_req, mut socket| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket.send_text("handler-ran").await;
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-abandon: 1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let refused = client.response(true).await;
    assert_eq!(refused.status, 503);
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "the chain returned a refusal, not the terminal's 101, so no session may start"
    );

    server.stop().await;
}

/// A middleware has no way to edit the request's field lines: `RequestContext` exposes
/// `header`, `header_values` and `headers()` and nothing that writes. The handshake validator
/// therefore always sees what the client sent, so a guard cannot grant or revoke an upgrade by
/// rewriting headers — refusal has to be a returned response, which the `x-deny` branch below
/// demonstrates against the same route.
///
/// The `x-observe` branch also pins where the pre-handshake check sits: `allows_upgrade` is a
/// *transport* capability (HTTP/2 has no `Upgrade`, RFC 7540 §8.1), not a scan of these headers,
/// so a plain `GET` on a WS route does run the chain, and the handshake failure is decided by the
/// validator afterwards.
#[tokio::test]
async fn a_middleware_cannot_grant_or_revoke_a_websocket_upgrade() {
    let observed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&observed);
    let mut app = AetherApp::new();
    app = app.use_middleware(move |req: RequestContext, next: Next| {
        let counter = Arc::clone(&counter);
        async move {
            if req.header("x-deny").is_some() {
                // The only power a guard has over an upgrade: answer instead of calling the chain.
                return Response::status(403, "denied");
            }
            if req.header("x-observe").is_some() {
                // Read-only: the middleware can see the lines and cannot rewrite them. Count them
                // so the assertion below knows the chain actually ran.
                let fields = req.headers().count();
                counter.fetch_add(fields, Ordering::SeqCst);
            }
            next(req).await
        }
    });
    app = app.get("/ping", |_req: RequestContext| "pong".to_string());
    app = app.ws("/ws/who", |_req, mut socket| async move {
        let _ = socket.send_text("handler-ran").await;
    });

    let server = TestServer::start(app).await;

    // A handshake the client sent is not affected by the middleware observing it.
    let mut upgraded = Client::connect(server.addr).await;
    upgraded
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-observe: 1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(upgraded.response(false).await.status, 101);
    let (opcode, _, payload) = upgraded.read_ws_frame().await;
    assert_eq!((opcode, payload.as_slice()), (0x1u8, &b"handler-ran"[..]));
    assert!(
        observed.load(Ordering::SeqCst) > 0,
        "the chain must have run for the upgraded request"
    );

    // A request with no handshake headers at all cannot become an upgrade: the chain does run (the
    // pre-handshake check is about the *transport* handing a stream over, not about these headers),
    // and the validator then finds no `Upgrade` in the store it alone reads.
    let mut injected = Client::connect(server.addr).await;
    injected
        .send(b"GET /ws/who HTTP/1.1\r\nHost: test\r\nx-observe: 1\r\n\r\n")
        .await
        .unwrap();
    let refused = injected.response(true).await;
    assert_eq!(
        refused.status, 400,
        "a non-upgrade request for a WS route must not be able to become an upgrade"
    );
    injected
        .send(b"GET /ping HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let after = injected.response(true).await;
    assert_eq!(
        String::from_utf8_lossy(&after.body),
        "pong",
        "a refused upgrade must leave the connection in HTTP mode, not half-upgraded"
    );

    // The contrast: the same middleware *can* refuse, by answering. A guard that wants the
    // handshake not to happen has to return a response; there is no header edit that would do it.
    let mut denied = Client::connect(server.addr).await;
    denied
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-deny: 1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let denial = denied.response(true).await;
    assert_eq!(denial.status, 403);

    server.stop().await;
}

/// A panic inside a guard must not become a handed-over socket: the same task boundary that
/// maps a handler panic to a status now covers the upgrade request too. Both halves matter —
/// panicking before `next` never reaches the terminal, and panicking *after* `next` reaches it
/// and then throws its `101` away, which is the case a plain "did the route run" flag misses.
#[tokio::test]
async fn a_panicking_middleware_answers_the_upgrade_request_with_500() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.header("x-boom").is_some() {
            panic!("middleware exploded");
        }
        if req.header("x-boom-after").is_some() {
            next(req).await;
            panic!("middleware exploded after the upgrade was handed over");
        }
        next(req).await
    });
    app = app.ws("/ws/who", |_req, mut socket| async move {
        let _ = socket.send_text("handler-ran").await;
    });

    let server = TestServer::start(app).await;

    for boom_header in ["x-boom: 1", "x-boom-after: 1"] {
        let mut boom = Client::connect(server.addr).await;
        boom.send(
            format!(
                "GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
                  Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n{boom_header}\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let answered = boom.response(true).await;
        assert_eq!(
            answered.status, 500,
            "a middleware panic ({boom_header}) must be mapped to a status, never upgraded past"
        );
    }

    // The same server still upgrades a request whose middleware ran to completion.
    let mut fine = Client::connect(server.addr).await;
    fine.send(
        b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
          Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await
    .unwrap();
    assert_eq!(fine.response(false).await.status, 101);
    let (opcode, _, payload) = fine.read_ws_frame().await;
    assert_eq!((opcode, payload.as_slice()), (0x1u8, &b"handler-ran"[..]));

    server.stop().await;
}

/// Middleware composes at registration time — the rule an HTTP route already follows — so a
/// guard added *after* a WebSocket route does not wrap it. Pinned because "snapshot at
/// registration" and "live list" are both plausible readings of the fix.
#[tokio::test]
async fn middleware_registered_after_a_websocket_route_does_not_wrap_it() {
    let mut app = AetherApp::new();
    app = app.ws("/ws/who", |req, mut socket| async move {
        let seen = req
            .extension::<WsMarker>()
            .map(|marker| marker.0.clone())
            .unwrap_or_else(|| "absent".to_string());
        let _ = socket.send_text(&seen).await;
    });
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        req.insert_extension(WsMarker("too-late".to_string()));
        next(req).await
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws/who HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x1);
    assert_eq!(
        payload, b"absent",
        "the route was registered before the middleware, so its chain must stay empty"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// The per-route form of the same guarantee: `ws_with` wraps the upgrade request of the one
/// registration that names it, and a guard that refuses gets its answer sent with no socket
/// handed over — while a plain `ws` route on the same server stays unguarded.
#[tokio::test]
async fn per_route_middleware_wraps_the_upgrade_request_of_its_websocket_route() {
    let mut app = AetherApp::new();
    app = app.ws_with(
        "/live/feed",
        |mut req: RequestContext, next: Next| async move {
            let room = req
                .header("x-room")
                .map(|raw| raw.to_string())
                .unwrap_or_else(|| "none".to_string());
            req.insert_extension(WsMarker(format!("route:{room}")));
            next(req).await
        },
        |req, mut socket| async move {
            let seen = req
                .extension::<WsMarker>()
                .map(|marker| marker.0.clone())
                .unwrap_or_else(|| "absent".to_string());
            let _ = socket.send_text(&seen).await;
        },
    );
    app = app.ws("/open", |_req, mut socket| async move {
        let _ = socket.send_text("no-route-chain").await;
    });

    let server = TestServer::start(app).await;

    let mut guarded = Client::connect(server.addr).await;
    guarded
        .send(
            b"GET /live/feed HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-room: 7\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(guarded.response(false).await.status, 101);
    let (opcode, _, payload) = guarded.read_ws_frame().await;
    assert_eq!(opcode, 0x1);
    assert_eq!(
        payload, b"route:7",
        "a route's own middleware must run for that route's WebSocket upgrade"
    );
    guarded.expect_closed().await;

    let mut plain = Client::connect(server.addr).await;
    plain
        .send(
            b"GET /open HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-room: 7\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(plain.response(false).await.status, 101);
    let (opcode, _, payload) = plain.read_ws_frame().await;
    assert_eq!(opcode, 0x1);
    assert_eq!(
        payload, b"no-route-chain",
        "the chain is bound to /live/feed's registration, not to the server"
    );
    plain.expect_closed().await;

    server.stop().await;
}

/// A per-route WebSocket guard refuses before the handover: the status is the guard's answer,
/// not a `101`, and the handler never sees a socket.
#[tokio::test]
async fn a_per_route_guard_refuses_a_websocket_upgrade_before_the_handover() {
    let started = Arc::new(AtomicUsize::new(0));
    let mut app = AetherApp::new();
    app = app.ws_with(
        "/live/feed",
        |req: RequestContext, next: Next| async move {
            if req.header("x-api-key") == Some("secret") {
                next(req).await
            } else {
                Response::status(401, "Unauthorized")
            }
        },
        {
            let started = Arc::clone(&started);
            move |_req, mut socket| {
                let started = Arc::clone(&started);
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    let _ = socket.send_text("entered").await;
                }
            }
        },
    );

    let server = TestServer::start(app).await;

    let mut refused = Client::connect(server.addr).await;
    refused
        .send(
            b"GET /live/feed HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = refused.response(true).await;
    assert_eq!(answer.status, 401);
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "the handler must not run"
    );

    let mut allowed = Client::connect(server.addr).await;
    allowed
        .send(
            b"GET /live/feed HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-api-key: secret\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(allowed.response(false).await.status, 101);
    let (opcode, _, payload) = allowed.read_ws_frame().await;
    assert_eq!((opcode, payload.as_slice()), (0x1u8, &b"entered"[..]));
    assert_eq!(started.load(Ordering::SeqCst), 1);
    allowed.expect_closed().await;

    server.stop().await;
}

/// `RouteGroup::ws` used to call `AetherApp::ws` and drop the group's own middleware on the
/// floor, so a guard registered inside a group protected that group's HTTP routes only.
#[tokio::test]
async fn group_middleware_wraps_the_upgrade_request_of_a_group_websocket_route() {
    let mut app = AetherApp::new();
    app = app.group("/live", |group| {
        group.use_middleware(|mut req: RequestContext, next: Next| async move {
            let room = req
                .header("x-room")
                .map(|raw| raw.to_string())
                .unwrap_or_else(|| "none".to_string());
            req.insert_extension(WsMarker(format!("group:{room}")));
            next(req).await
        });
        group.ws("/feed", |req, mut socket| async move {
            let seen = req
                .extension::<WsMarker>()
                .map(|marker| marker.0.clone())
                .unwrap_or_else(|| "absent".to_string());
            let _ = socket.send_text(&seen).await;
        });
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /live/feed HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-room: 42\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x1);
    assert_eq!(
        payload, b"group:42",
        "a group's own middleware must run for that group's WebSocket route"
    );
    client.expect_closed().await;

    server.stop().await;
}

/// RFC 6455 §4.2.2: the server may pick one of the client's offered subprotocols. The route's
/// declaration is the preference order, so the client's own listing order must not decide.
#[tokio::test]
async fn an_offered_subprotocol_is_echoed_from_the_route_declaration() {
    let mut app = AetherApp::new();
    app = app.ws_subprotocols(
        "/chat",
        &["chat.v2", "chat.v1"],
        |_req, mut socket| async move {
            let _ = socket.send_text("ready").await;
        },
    );

    let server = TestServer::start(app).await;

    // The client lists its second preference first; the route's first declaration still wins.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /chat HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Protocol: chat.v1, chat.v2\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = client.response(false).await;
    assert_eq!(answer.status, 101);
    assert_eq!(
        answer.header("sec-websocket-protocol"),
        Some("chat.v2"),
        "the route's declaration order is the server's preference"
    );
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!((opcode, payload.as_slice()), (0x1u8, &b"ready"[..]));
    client.expect_closed().await;

    // Offering only the route's second preference selects that one — not the first declaration.
    let mut single = Client::connect(server.addr).await;
    single
        .send(
            b"GET /chat HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Protocol: chat.v1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = single.response(false).await;
    assert_eq!(answer.status, 101);
    assert_eq!(answer.header("sec-websocket-protocol"), Some("chat.v1"));
    single.expect_closed().await;

    server.stop().await;
}

/// A route that declares no subprotocol keeps the pre-negotiation behaviour: the offer is
/// ignored and the `101` names none.
#[tokio::test]
async fn a_route_without_a_protocol_declaration_ignores_the_offer() {
    let mut app = AetherApp::new();
    app = app.ws("/plain", |_req, mut socket| async move {
        let _ = socket.send_text("plain").await;
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /plain HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Protocol: chat.v2\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = client.response(false).await;
    assert_eq!(answer.status, 101);
    assert_eq!(
        answer.header("sec-websocket-protocol"),
        None,
        "nothing was negotiated, so the response must not name a subprotocol"
    );
    let (_, _, payload) = client.read_ws_frame().await;
    assert_eq!(payload, b"plain");
    client.expect_closed().await;

    server.stop().await;
}

/// A client that asks for a subprotocol the route does not speak is refused **before** the
/// handover. The `101` must not go out: a client that requested a subprotocol and receives none
/// is required to fail the connection itself, so the refusal has to be visible as an HTTP answer.
#[tokio::test]
async fn an_offer_that_matches_no_declared_protocol_is_refused() {
    let sessions = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&sessions);
    let mut app = AetherApp::new();
    app = app.get("/health", |_req: RequestContext| "ok");
    app = app.ws_subprotocols("/chat", &["chat.v2"], move |_req, _socket| {
        let started = Arc::clone(&started);
        async move {
            started.fetch_add(1, Ordering::SeqCst);
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /chat HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Protocol: chat.v3\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = client.response(true).await;
    assert_eq!(answer.status, 400);
    assert_eq!(answer.header("sec-websocket-protocol"), None);
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        0,
        "no session may start when the handshake was refused"
    );

    // The socket never switched framing, so it still answers an ordinary HTTP request.
    client
        .send(b"GET /health HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let after = client.response(true).await;
    assert_eq!(after.status, 200);
    assert_eq!(after.body, b"ok");

    server.stop().await;
}

/// The declared list is what the route *speaks*, not something the client must use: a client
/// that offers nothing gets the plain upgrade.
#[tokio::test]
async fn a_client_that_offers_no_subprotocol_still_gets_the_route() {
    let mut app = AetherApp::new();
    app = app.ws_subprotocols("/chat", &["chat.v2"], |_req, mut socket| async move {
        let _ = socket.send_text("no-offer").await;
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /chat HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = client.response(false).await;
    assert_eq!(answer.status, 101);
    assert_eq!(answer.header("sec-websocket-protocol"), None);
    let (_, _, payload) = client.read_ws_frame().await;
    assert_eq!(payload, b"no-offer");
    client.expect_closed().await;

    server.stop().await;
}

/// The group registration path carries the declaration too, under the group prefix.
#[tokio::test]
async fn a_group_websocket_route_negotiates_its_subprotocol() {
    let mut app = AetherApp::new();
    app = app.group("/live", |group| {
        group.ws_subprotocols("/feed", &["feed.v1"], |_req, mut socket| async move {
            let _ = socket.send_text("feed").await;
        });
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /live/feed HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Protocol: feed.v1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let answer = client.response(false).await;
    assert_eq!(answer.status, 101);
    assert_eq!(answer.header("sec-websocket-protocol"), Some("feed.v1"));
    let (_, _, payload) = client.read_ws_frame().await;
    assert_eq!(payload, b"feed");
    client.expect_closed().await;

    server.stop().await;
}

/// The `405`/`Allow` pair for a non-`GET` on a WS route is already pinned by
/// `websocket_handshake_failures_and_unmasked_frames_are_rejected`. What that test leaves open is
/// *where* the refusal comes from: the docs claim it is a pre-chain gate, and only the code showed
/// it. This is the wire version of that claim, with the handshake headers present so the refusal
/// cannot be blamed on a malformed request - and with a positive control, because a counter that
/// never increments proves nothing.
#[tokio::test]
async fn a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain() {
    let chain_runs = Arc::new(AtomicUsize::new(0));
    let sessions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&chain_runs);
    let started = Arc::clone(&sessions);
    let mut app = AetherApp::new();
    app = app.use_middleware(move |req: RequestContext, next: Next| {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            next(req).await
        }
    });
    app = app.get("/ping", |_req: RequestContext| "pong".to_string());
    app = app.ws("/ws", move |_req, _socket| {
        let started = Arc::clone(&started);
        async move {
            started.fetch_add(1, Ordering::SeqCst);
        }
    });

    let server = TestServer::start(app).await;

    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"POST /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Content-Length: 0\r\n\r\n",
        )
        .await
        .unwrap();
    let refused = client.response(true).await;
    assert_eq!(
        refused.status,
        405,
        "body: {}",
        String::from_utf8_lossy(&refused.body)
    );
    // `OPTIONS` joined the list when the generic responder landed: the path answers it with a
    // `204` + `Allow`, so a 405 that named only `GET` would be understating the target. `HEAD`
    // stays absent - an upgrade cannot be completed by a bodiless method.
    assert_eq!(
        refused.header("Allow"),
        Some("GET, OPTIONS"),
        "a 405 must name every method the target answers"
    );
    assert_eq!(
        chain_runs.load(Ordering::SeqCst),
        0,
        "the method gate must answer before the middleware chain, not through it"
    );
    assert_eq!(sessions.load(Ordering::SeqCst), 0, "no session may start");

    // Positive control: the same connection still serves an ordinary route, and that request does
    // run the chain. Without this line a broken probe would look identical to the gate.
    client
        .send(b"GET /ping HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let after = client.response(true).await;
    assert_eq!(after.status, 200);
    assert_eq!(
        String::from_utf8_lossy(&after.body),
        "pong",
        "the refusal must leave the connection usable"
    );
    assert_eq!(
        chain_runs.load(Ordering::SeqCst),
        1,
        "the counter is live: an ordinary request runs the chain the upgrade request skipped"
    );

    server.stop().await;
}

/// A `101` means "the protocol transition happened". A guard that returns one without letting the
/// terminal run has not performed it: the handoff needs the request context the terminal records,
/// so a fabricated `101` has no handoff behind it. The core refuses the claim with `500` instead of
/// sending it, because serving it would put the client into WebSocket framing on a socket this
/// server still parses as HTTP/1.1. The refusal is a fresh answer, so none of the claims the guard
/// built around the `101` survive into it — and the positive control at the bottom shows a real
/// upgrade through the same middleware still switches protocols.
#[tokio::test]
async fn a_hand_built_101_from_a_middleware_is_refused_with_500() {
    let sessions = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&sessions);
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        let fake = req.header("x-fake-upgrade").is_some();
        if fake {
            // Deliberately not calling `next`: nothing records a request context, so the core has
            // no handoff to authorize. The headers and body are the ones a real handshake answer
            // would carry, so the refusal can be checked for leaking any of them.
            return Response::json_status(
                101,
                "Switching Protocols",
                Bytes::from_static(b"{\"switched\":true}"),
            )
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Accept", "not-a-real-accept-token");
        }
        next(req).await
    });
    app = app.get("/ping", |_req: RequestContext| "pong".to_string());
    app = app.ws("/ws", move |_req, mut socket| {
        let started = Arc::clone(&started);
        async move {
            started.fetch_add(1, Ordering::SeqCst);
            let _ = socket.send_text("session-started").await;
        }
    });

    let server = TestServer::start(app).await;

    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nx-fake-upgrade: 1\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(
        response.status,
        500,
        "an upgrade the guard never performed must not reach the client: {}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(response.version, "HTTP/1.1", "status line: {response:?}");
    // The measured shape of the refusal, not an assumed one.
    assert!(
        response.body.is_empty(),
        "the refused upgrade must not carry the guard's body: {:?}",
        String::from_utf8_lossy(&response.body)
    );
    for claimed in [
        "upgrade",
        "connection",
        "sec-websocket-accept",
        "content-type",
    ] {
        assert_eq!(
            response.header(claimed),
            None,
            "a 500 must not repeat the handshake the guard only claimed ({claimed}): {:?}",
            response.headers
        );
    }
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        0,
        "a hand-built 101 must not start a session"
    );

    // The connection did not switch protocols: the same socket still answers HTTP/1.1.
    client
        .send(b"GET /ping HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let after = client.response(true).await;
    assert_eq!(after.status, 200);
    assert_eq!(String::from_utf8_lossy(&after.body), "pong");

    // Positive control, and the condition that decides whether the fix is safe: the same route and
    // the same middleware still upgrade when the guard actually calls `next`.
    let mut real = Client::connect(server.addr).await;
    real.send(
        b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await
    .unwrap();
    let upgraded = real.response(false).await;
    assert_eq!(
        upgraded.status, 101,
        "a real upgrade must still switch: {upgraded:?}"
    );
    assert_eq!(
        upgraded.header("sec-websocket-accept"),
        Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "the synthesized handshake must still be the RFC 6455 one"
    );
    let (opcode, _, payload) = real.read_ws_frame().await;
    assert_eq!(
        (opcode, payload.as_slice()),
        (0x1u8, &b"session-started"[..]),
        "the session must start for a real upgrade"
    );
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        1,
        "exactly one session ran"
    );

    server.stop().await;
}

/// `101` is what the framework emits when it hands a socket over, so an informational status it did
/// not produce is refused wherever it appears — a guard's claim or a handler's, on a WebSocket route
/// or an ordinary one. The class is the rule, not the one status (track 89): `100 Continue` is
/// written straight to the stream by the request loop, so no `1xx` reaches the response exit from a
/// transition this server performed, and a `103` is refused exactly like a `101`. Everything outside
/// `1xx` still travels the ordinary path untouched: a `418` is served as written.
#[tokio::test]
async fn a_101_without_a_handoff_is_never_served() {
    let mut app = AetherApp::new();
    app = app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.header("x-fake-upgrade").is_some() {
            return Response::status(101, "Switching Protocols");
        }
        if req.header("x-early-hints").is_some() {
            return Response::status(103, "Early Hints");
        }
        if req.header("x-teapot").is_some() {
            return Response::status(418, "I'm a teapot");
        }
        next(req).await
    });
    app = app.get("/plain", |_req: RequestContext| "plain".to_string());
    // The same claim from the handler rather than the chain: the rule is about the status, not
    // about who wrote it.
    app = app.get("/handler-101", |_req: RequestContext| {
        Response::status(101, "Switching Protocols")
    });
    app = app.ws("/real", |_req, mut socket| async move {
        let _ = socket.send_text("handler-ran").await;
    });

    let server = TestServer::start(app).await;

    // A guard's `101` on a route with no handoff: no protocol transition happened, so serving it
    // would switch the client's framing on a socket this server still parses as HTTP/1.
    let mut fake = Client::connect(server.addr).await;
    fake.send(b"GET /plain HTTP/1.1\r\nHost: test\r\nx-fake-upgrade: 1\r\n\r\n")
        .await
        .unwrap();
    let refused = fake.response(true).await;
    assert_eq!(
        refused.status, 500,
        "a 101 the framework did not produce must not be served: {:?}",
        refused.headers
    );
    assert_eq!(
        refused.header("upgrade"),
        None,
        "the refusal is a fresh response, so the guard's claim headers go with it"
    );
    assert_eq!(refused.header("content-length"), Some("0"));

    // The same socket keeps serving HTTP/1, which is what the claim would have contradicted.
    fake.send(b"GET /plain HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(
        fake.response(true).await.status,
        200,
        "the connection is still HTTP/1 after it"
    );

    // A handler that returns the same status meets the same rule.
    let mut handler = Client::connect(server.addr).await;
    handler
        .send(b"GET /handler-101 HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(
        handler.response(true).await.status,
        500,
        "a handler-authored 101 is refused exactly like a guard's"
    );

    // A `1xx` that is not `101` on a WebSocket route: the refusal covers the class. The only
    // informational status this server writes itself is `100 Continue`, straight to the stream,
    // so a guard's `103` has no transition behind it either.
    let mut hints = Client::connect(server.addr).await;
    hints
        .send(
            b"GET /real HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nx-early-hints: 1\r\n\r\n",
        )
        .await
        .unwrap();
    let early = hints.response(true).await;
    assert_eq!(
        early.status, 500,
        "an informational answer with no transition behind it is refused: {:?}",
        early.headers
    );

    // Any other status the chain returns on a WebSocket route is served exactly as written.
    let mut refused = Client::connect(server.addr).await;
    refused
        .send(
            b"GET /real HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nx-teapot: 1\r\n\r\n",
        )
        .await
        .unwrap();
    let teapot = refused.response(true).await;
    assert_eq!(
        teapot.status, 418,
        "only the 101 claim is replaced: {:?}",
        teapot.headers
    );

    server.stop().await;
}

/// Graceful shutdown ends a live WebSocket session the way RFC 6455 says a server going away
/// should: one close frame with status 1001, after which the socket closes. The session task
/// ends on its own terms — the handler's receive loop returns — instead of being aborted.
#[tokio::test]
async fn shutdown_sends_a_going_away_close_frame_and_ends_the_session() {
    let returned = Arc::new(AtomicUsize::new(0));
    let finished = Arc::clone(&returned);
    let mut app = AetherApp::new();
    app = app.ws("/ws", move |_req, mut socket| {
        let finished = Arc::clone(&finished);
        async move {
            while let Ok(Some(_)) = socket.recv().await {
                let _ = socket.send_text("alive").await;
            }
            finished.fetch_add(1, Ordering::SeqCst);
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    client
        .send(&ws_client_frame(true, 0x1, b"ping", [1, 2, 3, 4]))
        .await
        .unwrap();
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(
        (opcode, payload.as_slice()),
        (0x1u8, &b"alive"[..]),
        "the session must be demonstrably live before shutdown"
    );

    server.stop().await;

    let (opcode, fin, payload) = client.read_ws_frame().await;
    assert_eq!(
        opcode, 0x8,
        "shutdown must announce itself with a close frame"
    );
    assert!(fin, "a control frame is never fragmented");
    assert!(
        payload.len() >= 2,
        "a close frame carries a status code: {payload:?}"
    );
    assert_eq!(
        u16::from_be_bytes([payload[0], payload[1]]),
        1001,
        "RFC 6455 §7.4.1.1: going away"
    );
    assert_eq!(&payload[2..], b"server shutting down");
    assert_eq!(
        returned.load(Ordering::SeqCst),
        1,
        "the receive loop ended on the close, so the handler returned rather than being aborted"
    );

    // Nothing follows the close frame.
    let mut tail = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(5), client.stream.read(&mut tail))
        .await
        .expect("the connection stayed open after the close frame")
        .expect("failed to read the shutdown tail");
    assert_eq!(
        read,
        0,
        "shutdown sent bytes instead of closing: {:?}",
        &tail[..read]
    );
}

/// The close frame must not turn shutdown into a wait on the handler: a session that never
/// returns to `recv` (so it never sees the shutdown signal) is still aborted after the bounded
/// grace, and the connection closes with no frame because there was no receive loop to write one.
#[tokio::test]
async fn a_session_that_never_returns_to_recv_is_still_aborted_at_shutdown() {
    let entered = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let entered_handler = Arc::clone(&entered);
    let finished_handler = Arc::clone(&finished);
    let mut app = AetherApp::new();
    app = app.ws("/sleepy", move |_req, _socket| {
        let entered = Arc::clone(&entered_handler);
        let finished = Arc::clone(&finished_handler);
        async move {
            entered.store(true, Ordering::SeqCst);
            // Far past the shutdown grace and this test's own bound, so finishing would mean
            // the session was never aborted.
            tokio::time::sleep(Duration::from_secs(30)).await;
            finished.store(true, Ordering::SeqCst);
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /sleepy HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    for _ in 0..200 {
        if entered.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        entered.load(Ordering::SeqCst),
        "the handler never started, so shutdown would measure nothing"
    );

    let started = Instant::now();
    server.stop().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "shutdown waited on a handler that never returned to recv: {:?}",
        started.elapsed()
    );
    assert!(
        !finished.load(Ordering::SeqCst),
        "the session ran to completion instead of being aborted at the grace"
    );

    let mut tail = [0u8; 16];
    match tokio::time::timeout(Duration::from_secs(5), client.stream.read(&mut tail)).await {
        Ok(Ok(0)) | Ok(Err(_)) => {}
        Ok(Ok(read)) => panic!("bytes arrived from an aborted session: {:?}", &tail[..read]),
        Err(_) => panic!("the aborted session's socket stayed open"),
    }
}

/// A handler that returns `Err` is answered by the blanket `IntoResponse for Result<T, E:
/// IntoResponseError>`. `&'static str` implements that trait as a 400 with its own text, and the
/// body carries the matching 4xx code: the two halves have to agree, which is what this pins. The
/// contradiction it retires was `E5002` "unhandled internal server error" sitting on a 400.
#[tokio::test]
async fn a_handler_error_answers_400_and_says_so_in_the_body() {
    let mut app = AetherApp::new();
    app = app.get_async("/db-lookup", || async {
        Err::<&'static str, &'static str>("database is unreachable")
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request("GET", "/db-lookup", &[], b"")
        .await
        .unwrap();
    let response = client.response(true).await;

    assert_eq!(response.status, 400);
    let body = response.json();
    assert_eq!(
        body["code"].as_str(),
        Some("E4003"),
        "the body promised a 5xx code on a 4xx answer: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("database is unreachable"),
        "the handler message must still reach the caller: {body}"
    );

    server.stop().await;
}

/// `AetherError` now chooses the status per variant instead of inheriting the blanket's single
/// guess, so a validation failure and an internal failure are distinguishable on the wire — in the
/// status line, in the `code`, and in whether the message is sent at all.
#[tokio::test]
async fn typed_handler_errors_answer_their_own_class() {
    let mut app = AetherApp::new();
    app = app.get_async("/client-mistake", || async {
        Err::<&'static str, AetherError>(AetherError::ValidationFailed("name is required".into()))
    });
    app = app.get_async("/too-large", || async {
        Err::<&'static str, AetherError>(AetherError::BodyLimitExceeded)
    });
    app = app.get_async("/internal", || async {
        Err::<&'static str, AetherError>(AetherError::Io(std::io::Error::other(
            "replica db.internal:6379 refused",
        )))
    });
    app = app.get_async("/deadline", || async {
        Err::<&'static str, AetherError>(AetherError::Timeout)
    });
    // The same variant the parser raises for an unmet `Expect`, reached here as a handler's own
    // answer: the status, the code and the message are three separate arms that must agree.
    app = app.get_async("/unmet-expectation", || async {
        Err::<&'static str, AetherError>(AetherError::UnsupportedExpectation)
    });

    let server = TestServer::start(app).await;

    async fn ask(addr: SocketAddr, path: &str) -> HttpResponse {
        let mut client = Client::connect(addr).await;
        client.send_request("GET", path, &[], b"").await.unwrap();
        client.response(true).await
    }

    let mistake = ask(server.addr, "/client-mistake").await;
    assert_eq!(400, mistake.status);
    let mistake = mistake.json();
    assert_eq!("E4002", mistake["code"].as_str().unwrap());
    assert!(
        mistake["error"]
            .as_str()
            .unwrap()
            .contains("name is required"),
        "a 4xx names the caller's own mistake: {mistake}"
    );

    let too_large = ask(server.addr, "/too-large").await;
    assert_eq!(413, too_large.status);
    assert_eq!(
        "E3002",
        too_large.json()["code"].as_str().unwrap(),
        "a 413 must not answer with a 4xx or 5xx code"
    );

    let internal = ask(server.addr, "/internal").await;
    assert_eq!(500, internal.status);
    let internal = internal.json();
    assert_eq!("E5002", internal["code"].as_str().unwrap());
    assert_eq!(
        "Internal Server Error",
        internal["error"].as_str().unwrap(),
        "a 5xx must not leak the internal failure: {internal}"
    );

    let deadline = ask(server.addr, "/deadline").await;
    assert_eq!(504, deadline.status);
    assert_eq!("E5002", deadline.json()["code"].as_str().unwrap());

    let expectation = ask(server.addr, "/unmet-expectation").await;
    assert_eq!(417, expectation.status);
    let expectation = expectation.json();
    assert_eq!(
        "E3003",
        expectation["code"].as_str().unwrap(),
        "a 417 must not answer with the payload-budget or generic 4xx code"
    );
    assert!(
        expectation["error"]
            .as_str()
            .unwrap()
            .contains("unsupported expectation"),
        "a 4xx names the caller's own promise: {expectation}"
    );

    server.stop().await;
}

/// The serving core reads `Accept-Encoding` only when a compression layer is configured, so
/// this is the test that a configured layer still receives the header: the middleware's own
/// unit tests call `apply` directly and cannot see the read disappear. The second half pins
/// the other side of the same gate - an app with no compression layer never compresses.
#[tokio::test]
async fn accept_encoding_reaches_compression_only_when_a_layer_is_configured() {
    let mut app = AetherApp::new();
    app = app.enable_compression();
    app = app.get("/big", |_| "a".repeat(8_000));
    let server = TestServer::start(app).await;

    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /big HTTP/1.1\r\nHost: test\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("Content-Encoding"),
        Some("gzip"),
        "a configured compression layer lost the Accept-Encoding read: {:?}",
        resp.header("Content-Encoding")
    );
    assert!(
        resp.header("Vary")
            .is_some_and(|vary| vary.contains("Accept-Encoding")),
        "a compressed answer must still tell a cache the body is encoding-dependent"
    );
    assert!(
        resp.header("X-Request-ID").is_some(),
        "the core still tags every response with the request it generated"
    );
    server.stop().await;

    let mut plain = AetherApp::new();
    plain = plain.get("/big", |_| "a".repeat(8_000));
    let server = TestServer::start(plain).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /big HTTP/1.1\r\nHost: test\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("Content-Encoding"),
        None,
        "an app that never enabled compression must not answer with a coded body"
    );
    assert_eq!(resp.body.len(), 8_000);
    server.stop().await;
}

/// The unit tests call `CompressionMiddleware::apply` directly; this pin is the wire answer.
/// A client that refuses identity with no coding to fall back on gets a bodyless 406 that
/// keeps `Vary`, and one that offers gzip gets the coded body instead.
#[tokio::test]
async fn a_refused_identity_is_answered_on_the_wire() {
    let mut app = AetherApp::new();
    app = app.enable_compression();
    app = app.get("/big", |_| "a".repeat(8_000));
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send(
            b"GET /big HTTP/1.1\r\nHost: test\r\nAccept-Encoding: identity;q=0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let refused = client.response(true).await;
    assert_eq!(refused.status, 406);
    assert!(refused.body.is_empty(), "a 406 must not carry the body");
    assert_eq!(refused.header("Content-Encoding"), None);
    assert_eq!(refused.header("Content-Length"), Some("0"));
    assert!(
        refused
            .header("Vary")
            .is_some_and(|vary| vary.contains("Accept-Encoding")),
        "the refusal depends on Accept-Encoding and must say so"
    );

    // The first request asked for `Connection: close`, so this socket is finished; the harness
    // does not reconnect on its own and a second `send` would land on a closed socket.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /big HTTP/1.1\r\nHost: test\r\nAccept-Encoding: gzip, identity;q=0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let coded = client.response(true).await;
    assert_eq!(coded.status, 200);
    assert_eq!(coded.header("Content-Encoding"), Some("gzip"));
    assert!(!coded.body.is_empty());

    server.stop().await;
}

/// `serve_static` is the framework's own strong-validator path — a content hash plus
/// `Accept-Ranges: bytes` — and it is exactly the shape compression transforms. The coded
/// answer must not carry the validator or the range promise that named the bytes it replaced,
/// while the identity answer, which is the stored representation, keeps both. Otherwise a client
/// caches a gzip body under the identity ETag, and a later `Range` with `If-Range` is answered
/// from the stored bytes and spliced into the compressed entry.
#[tokio::test]
async fn a_compressed_static_answer_drops_the_validator_the_identity_answer_keeps() {
    let payload = Bytes::from("const answer = 42;\n".repeat(400));
    let mut app = AetherApp::new();
    app = app.enable_compression();
    let asset = payload.clone();
    app = app.get("/asset.js", move |_| {
        serve_static("asset.js", asset.clone(), None, 60)
    });
    let server = TestServer::start(app).await;

    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /asset.js HTTP/1.1\r\nHost: test\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let coded = client.response(true).await;
    assert_eq!(coded.status, 200);
    assert_eq!(coded.header("Content-Encoding"), Some("gzip"));
    assert_eq!(
        coded.header("ETag"),
        None,
        "the coded body carried the identity validator: {:?}",
        coded.header("ETag")
    );
    assert_eq!(
        coded.header("Accept-Ranges"),
        None,
        "the coded body advertised ranges of the stored bytes"
    );
    assert!(
        coded
            .header("Vary")
            .is_some_and(|vary| vary.contains("Accept-Encoding")),
        "the coded answer must still Vary on Accept-Encoding"
    );
    let declared: usize = coded
        .header("Content-Length")
        .expect("a body must declare its length")
        .parse()
        .expect("Content-Length is a number");
    assert_eq!(
        declared,
        coded.body.len(),
        "Content-Length matches the coded body"
    );

    // The same resource under the identity coding is the stored representation, so its
    // validator and range support still describe the bytes now on the wire.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /asset.js HTTP/1.1\r\nHost: test\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let plain = client.response(true).await;
    assert_eq!(plain.status, 200);
    assert_eq!(plain.header("Content-Encoding"), None);
    let etag = generate_etag(&payload);
    assert_eq!(plain.header("ETag"), Some(etag.as_str()));
    assert_eq!(plain.header("Accept-Ranges"), Some("bytes"));
    assert_eq!(plain.body, payload);

    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// The declared length of a *request* body on HTTP/1 (RFC 9112 §6.3) — the other half of the
// division track 110 measured. Every shape below was run on the wire before it was written down.
//
// On HTTP/1 the length field is itself the framing, so the body reader stops at the number and a
// peer that sends more bytes than it promised has not produced one malformed request, it has
// produced two messages. `Content-Length: 6` followed by `abcdef` plus a whole second request is
// answered twice on one connection; the same promise followed by bytes that are not a request is
// answered `400` and the connection closes, because a server that reframes what it cannot parse is
// the desync this whole family of rules exists to prevent.
//
// Two divergences from HTTP/2 are pinned from both ends on purpose — a rule only one frontend
// states is an accident waiting for whichever reader notices it and "fixes" the other side:
// optional whitespace around the digits is accepted here (RFC 9110 §5.5 allows OWS on any field
// value) and refused over h2 by `a_padded_content_length_is_reset_over_h2`; a second
// `content-length` line is refused here even when it agrees with the first, while agreeing repeats
// are served over h2 by `matching_repeated_content_lengths_are_served_over_h2`. HTTP/2 has no
// length-framed body for a repeat to disagree with; HTTP/1 does, and that is where smuggling lives.
//
// The acceptance has its own edge, and it is not leniency: RFC 9110 §5.5 defines OWS as SP and
// HTAB and nothing else, so a pad of any other character is not padding but part of the value —
// and `a_non_ascii_padded_content_length_is_refused_on_http_1` pins that this frontend reads a
// U+00A0 before the digits as no length at all, not as the number the pad hides. That is the seam
// the parser's ASCII-only trim exists to close, and the injection that proves it: replacing it with
// a Unicode trim turns that refusal and the parser's own unit test red, while leaving the accepted
// SP-padded shape green.
// ---------------------------------------------------------------------------------------------

/// A route that reports how many bytes it was given and which ones, so the framing decision is
/// visible in the answer rather than inferred from a status code.
fn declared_length_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.post("/echo", |req| {
        format!("{}|{}", req.body.len(), String::from_utf8_lossy(&req.body))
    });
    app = app.get("/ping", |_| "pong");
    app
}

#[tokio::test]
async fn a_body_longer_than_its_declared_length_leaves_the_surplus_for_the_next_request() {
    let server = TestServer::start(declared_length_app()).await;
    let mut client = Client::connect(server.addr).await;
    // One write, one head promising six bytes, eleven bytes of tail: six of body and a complete
    // `GET /ping`. The declared length wins, and what is left over is the next message.
    client
        .send(
            b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 6\r\n\r\n\
                abcdefGET /ping HTTP/1.1\r\nHost: test\r\n\r\n",
        )
        .await
        .unwrap();

    let first = client.response(true).await;
    assert_eq!(
        (first.status, first.body.as_slice()),
        (200, b"6|abcdef".as_slice()),
        "the handler must receive exactly the bytes the head promised, not the surplus"
    );
    let second = client.response(true).await;
    assert_eq!(
        (second.status, second.body.as_slice()),
        (200, b"pong".as_slice()),
        "bytes past the declared length are the next request, so they must still be served"
    );

    server.stop().await;
}

#[tokio::test]
async fn bytes_past_the_declared_length_that_are_not_a_request_are_400_and_close() {
    let server = TestServer::start(declared_length_app()).await;
    let mut client = Client::connect(server.addr).await;
    // The same surplus, this time not parseable as a request line. Refusing it as the *second*
    // message is what keeps the first answer honest: a server that swallowed it, or reframed it,
    // would be the request-smuggling proxy these rules are written against.
    client
        .send(
            b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 6\r\n\r\nabcdefGARBAGE\r\n\r\n",
        )
        .await
        .unwrap();

    let first = client.response(true).await;
    assert_eq!(
        (first.status, first.body.as_slice()),
        (200, b"6|abcdef".as_slice()),
        "the promised body is served before anything is said about the surplus"
    );
    assert_eq!(
        client.response(true).await.status,
        400,
        "the surplus is a second message and is answered as one"
    );
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn an_ows_padded_content_length_is_served_on_http_1() {
    let server = TestServer::start(declared_length_app()).await;
    let mut client = Client::connect(server.addr).await;
    // Space on both sides of the digits. HTTP/2 resets this request
    // (`a_padded_content_length_is_reset_over_h2`); here the field value is OWS around the value
    // like any other field, and the digits inside it are what the framing reads.
    client
        .send(b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length:   6  \r\n\r\nabcdef")
        .await
        .unwrap();

    let response = client.response(true).await;
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, b"6|abcdef".as_slice()),
        "the padding must not shift the body boundary the handler receives"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_non_ascii_padded_content_length_is_refused_on_http_1() {
    let server = TestServer::start(declared_length_app()).await;
    let mut client = Client::connect(server.addr).await;
    // U+00A0 before the digits. HTTP field syntax allows OWS, which RFC 9110 §5.5 defines as SP
    // and HTAB *only*, so this value is not a length at all and the request is refused. A trim that
    // took any Unicode whitespace would read it as `6`, frame a six-byte body from it, and hand the
    // two pad bytes to the next message — the seam `ows_trim` exists to close, and the reason this
    // is measured on a socket as well as in the parser's own unit test.
    client
        .send(b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: \xc2\xa06\r\n\r\nabcdef")
        .await
        .unwrap();

    assert_eq!(
        client.response(true).await.status,
        400,
        "a non-ASCII pad around the digits must not be read as a length"
    );
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn a_repeated_content_length_is_refused_on_http_1_even_when_it_agrees() {
    let server = TestServer::start(declared_length_app()).await;
    let mut client = Client::connect(server.addr).await;
    // Two identical promises. HTTP/2 serves this request
    // (`matching_repeated_content_lengths_are_served_over_h2`) because DATA frames do the framing
    // and the field is a redundant statement about them; here the field *is* the framing, so two
    // lines are two candidates for a boundary and the request is refused outright.
    client
        .send(b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 6\r\nContent-Length: 6\r\n\r\nabcdef")
        .await
        .unwrap();

    assert_eq!(
        client.response(true).await.status,
        400,
        "an agreeing pair is still a second content-length on the length-framed frontend"
    );
    client.expect_closed().await;

    server.stop().await;
}
