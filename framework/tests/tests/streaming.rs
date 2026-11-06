//! Streamed response bodies over real sockets.
//!
//! Every claim here is about bytes that arrive after the head: that an undeclared body is
//! chunked on HTTP/1.1 and close-delimited on HTTP/1.0, that a declared length is honoured in
//! both directions, that a source which fails or ends early ends the *connection* rather than
//! leaving a client reading the next response as the rest of this one, that a `HEAD` never
//! polls the source, and that a static file is answered from metadata with its bytes streamed.
//!
//! What is *not* measured here: resident memory. The per-chunk registration is pinned by unit
//! tests on the pricing helpers; no test in this suite samples RSS, and none claims to.

use aether::prelude::*;
use bytes::Bytes;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
struct Head {
    version: String,
    status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Everything that followed the head on the wire, in the shape the framing announced.
#[derive(Debug)]
enum Body {
    /// `Content-Length` bytes.
    Sized(Vec<u8>),
    /// A chunked body: the decoded payload and the raw bytes that carried it.
    Chunked { decoded: Vec<u8>, raw: Vec<u8> },
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

    async fn send_request(&mut self, method: &str, path: &str, headers: &[(&str, &str)]) {
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: test\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("Content-Length: 0\r\n\r\n");
        self.send(request.as_bytes()).await;
    }

    /// One more read into `buffered`. `false` means the peer closed.
    async fn fill(&mut self) -> bool {
        let mut chunk = [0u8; 8192];
        let read = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
            .await
            .expect("timed out waiting for bytes")
            .expect("failed to read");
        self.buffered.extend_from_slice(&chunk[..read]);
        read > 0
    }

    async fn head(&mut self) -> Head {
        loop {
            if let Some(end) = find_subslice(&self.buffered, b"\r\n\r\n") {
                let text = String::from_utf8(self.buffered[..end].to_vec()).expect("utf-8 head");
                self.buffered.drain(..end + 4);
                let mut lines = text.split("\r\n");
                let status_line = lines.next().expect("status line");
                let mut fields = status_line.split_whitespace();
                let version = fields.next().expect("version").to_string();
                let status = fields
                    .next()
                    .expect("status")
                    .parse::<u16>()
                    .expect("numeric status");
                let headers = lines
                    .filter_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        Some((name.trim().to_string(), value.trim().to_string()))
                    })
                    .collect();
                return Head {
                    version,
                    status,
                    headers,
                };
            }
            assert!(
                self.fill().await,
                "connection closed inside a response head"
            );
        }
    }

    /// The whole message: head plus whatever the head's framing announced.
    async fn message(&mut self) -> (Head, Body) {
        let head = self.head().await;
        let chunked = head
            .header("Transfer-Encoding")
            .is_some_and(|value| value.eq_ignore_ascii_case("chunked"));
        if chunked {
            let (decoded, raw) = self.chunked_body().await;
            return (head, Body::Chunked { decoded, raw });
        }
        let len = head
            .header("Content-Length")
            .map(|value| value.parse::<usize>().expect("numeric Content-Length"))
            .unwrap_or(0);
        while self.buffered.len() < len {
            assert!(self.fill().await, "connection closed inside a sized body");
        }
        let body = self.buffered.drain(..len).collect();
        (head, Body::Sized(body))
    }

    async fn chunked_body(&mut self) -> (Vec<u8>, Vec<u8>) {
        let mut decoded = Vec::new();
        let mut raw = Vec::new();
        loop {
            let size_end = loop {
                if let Some(end) = find_subslice(&self.buffered, b"\r\n") {
                    break end;
                }
                assert!(self.fill().await, "chunked body ended without a terminator");
            };
            let line = String::from_utf8(self.buffered[..size_end].to_vec()).expect("chunk line");
            let size = usize::from_str_radix(line.split(';').next().unwrap().trim(), 16)
                .expect("hex chunk size");
            let total = size_end + 2 + size + 2;
            while self.buffered.len() < total {
                assert!(self.fill().await, "chunked body ended inside a chunk");
            }
            raw.extend_from_slice(&self.buffered[..total]);
            decoded.extend_from_slice(&self.buffered[size_end + 2..size_end + 2 + size]);
            self.buffered.drain(..total);
            if size == 0 {
                return (decoded, raw);
            }
        }
    }

    /// Every byte still to come, until the peer closes.
    async fn rest_until_close(&mut self) -> Vec<u8> {
        while self.fill().await {}
        std::mem::take(&mut self.buffered)
    }
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
        "aether_stream_{}_{}_{}",
        std::process::id(),
        tag,
        counter
    ));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

/// A source whose chunks come from a list.
struct Chunks(std::vec::IntoIter<Bytes>);

impl BodySource for Chunks {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
        Box::pin(async move { self.0.next().map(Ok) })
    }
}

fn chunks(pieces: &[&'static [u8]]) -> Chunks {
    Chunks(
        pieces
            .iter()
            .map(|piece| Bytes::from_static(piece))
            .collect::<Vec<_>>()
            .into_iter(),
    )
}

/// A source that yields one chunk and then fails.
struct FailingAfterOne(bool);

impl BodySource for FailingAfterOne {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
        Box::pin(async move {
            if self.0 {
                return Some(Err(std::io::Error::other("the source failed")));
            }
            self.0 = true;
            Some(Ok(Bytes::from_static(b"abc")))
        })
    }
}

/// A source that counts how many times it was asked for a chunk.
struct Counted {
    chunks: Chunks,
    polls: Arc<AtomicUsize>,
}

impl BodySource for Counted {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
        Box::pin(async move {
            self.polls.fetch_add(1, Ordering::SeqCst);
            self.chunks.next_chunk().await
        })
    }
}

fn streaming_app(polls: Option<Arc<AtomicUsize>>) -> AetherApp {
    let mut app = AetherApp::new();
    app = app.get_async("/ping", |_req: RequestContext| async {
        Response::text("pong")
    });
    app = app.get_async("/stream", |_req: RequestContext| async {
        Response::stream(
            chunks(&[&b"hello"[..], &b""[..], &b" world"[..], &b"!"[..]]),
            "text/plain; charset=utf-8",
        )
    });
    app = app.get_async("/sized", |_req: RequestContext| async {
        Response::stream_with_length(
            chunks(&[&b"abc"[..], &b"def"[..]]),
            "text/plain; charset=utf-8",
            6,
        )
    });
    app = app.get_async("/broken", |_req: RequestContext| async {
        Response::stream(FailingAfterOne(false), "text/plain; charset=utf-8")
    });
    app = app.get_async("/short", |_req: RequestContext| async {
        Response::stream_with_length(chunks(&[&b"abc"[..]]), "text/plain; charset=utf-8", 6)
    });
    app = app.get_async("/long", |_req: RequestContext| async {
        Response::stream_with_length(chunks(&[&b"abcdef"[..]]), "text/plain; charset=utf-8", 3)
    });
    app = app.get_async("/empty", |_req: RequestContext| async {
        // A status that forbids a body drops the attached source unread.
        let mut response = Response::status(204, "No Content");
        response.stream = Some(BodyStream::from_chunks([Bytes::from_static(b"ignored")]));
        response
    });
    let polls = polls.expect("this app needs its poll counter");
    app = app.get_async("/counted", move |_req: RequestContext| {
        let polls = Arc::clone(&polls);
        async move {
            Response::stream_with_length(
                Counted {
                    chunks: chunks(&[&b"abc"[..], &b"def"[..]]),
                    polls,
                },
                "text/plain; charset=utf-8",
                6,
            )
        }
    });
    app
}

#[tokio::test]
async fn a_chunked_response_is_decoded_and_the_connection_stays_usable() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/stream", &[]).await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Transfer-Encoding"), Some("chunked"));
    assert_eq!(head.header("Content-Length"), None);
    let Body::Chunked { decoded, raw } = body else {
        panic!("an undeclared stream must be chunked");
    };
    // The empty chunk the source produced was skipped: an empty frame is the terminator.
    assert_eq!(decoded, b"hello world!");
    assert!(
        raw.ends_with(b"\r\n0\r\n\r\n"),
        "{:?}",
        String::from_utf8_lossy(&raw)
    );

    // The terminator is what makes the socket reusable at all.
    client.send_request("GET", "/ping", &[]).await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 200);
    assert!(matches!(body, Body::Sized(body) if body == b"pong"));

    server.stop().await;
}

#[tokio::test]
async fn a_declared_length_stream_sends_exactly_that_many_bytes() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/sized", &[]).await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Length"), Some("6"));
    assert_eq!(head.header("Transfer-Encoding"), None);
    assert!(matches!(body, Body::Sized(body) if body == b"abcdef"));

    client.send_request("GET", "/ping", &[]).await;
    assert_eq!(client.message().await.0.status, 200);

    server.stop().await;
}

#[tokio::test]
async fn a_source_error_ends_the_body_without_a_terminator() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/broken", &[]).await;
    let head = client.head().await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Transfer-Encoding"), Some("chunked"));

    let rest = client.rest_until_close().await;
    assert_eq!(
        rest, b"3\r\nabc\r\n",
        "the body that was produced is sent, and nothing else"
    );
    assert!(
        !rest.windows(5).any(|window| window == b"0\r\n\r\n"),
        "a failed source must not be framed as a finished body"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_stream_that_ends_early_of_its_declared_length_closes_the_connection() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/short", &[]).await;
    let head = client.head().await;
    assert_eq!(head.header("Content-Length"), Some("6"));
    let rest = client.rest_until_close().await;
    assert_eq!(rest, b"abc");

    server.stop().await;
}

#[tokio::test]
async fn a_stream_that_overruns_its_declared_length_is_cut_at_the_boundary() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/long", &[]).await;
    let head = client.head().await;
    assert_eq!(head.header("Content-Length"), Some("3"));
    let rest = client.rest_until_close().await;
    assert_eq!(
        rest, b"abc",
        "the declared length is the framing, so the extra bytes are never sent"
    );

    server.stop().await;
}

#[tokio::test]
async fn head_never_polls_a_streamed_source() {
    let polls = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(streaming_app(Some(Arc::clone(&polls)))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("HEAD", "/counted", &[]).await;
    let head = client.head().await;
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("Content-Length"),
        Some("6"),
        "a HEAD carries the GET's framing"
    );
    assert_eq!(polls.load(Ordering::SeqCst), 0, "HEAD drained the source");

    client.send_request("GET", "/counted", &[]).await;
    let (_, body) = client.message().await;
    assert!(matches!(body, Body::Sized(body) if body == b"abcdef"));
    assert!(polls.load(Ordering::SeqCst) >= 1);

    server.stop().await;
}

#[tokio::test]
async fn an_http10_request_gets_a_close_delimited_stream() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send(b"GET /stream HTTP/1.0\r\nHost: test\r\n\r\n")
        .await;
    let head = client.head().await;
    assert_eq!(head.version, "HTTP/1.0");
    assert_eq!(head.header("Transfer-Encoding"), None);
    assert_eq!(head.header("Content-Length"), None);
    assert_eq!(head.header("Connection"), Some("close"));
    let rest = client.rest_until_close().await;
    assert_eq!(rest, b"hello world!", "the close is the end of the body");

    server.stop().await;
}

#[tokio::test]
async fn a_body_forbidding_status_sends_none_of_an_attached_stream() {
    let server = TestServer::start(streaming_app(Some(Arc::new(AtomicUsize::new(0))))).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/empty", &[]).await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 204);
    assert_eq!(head.header("Transfer-Encoding"), None);
    assert_eq!(head.header("Content-Length"), None);
    assert!(matches!(body, Body::Sized(body) if body.is_empty()));

    // Nothing followed the head, so the connection is still in sync.
    client.send_request("GET", "/ping", &[]).await;
    assert_eq!(client.message().await.0.status, 200);

    server.stop().await;
}

#[tokio::test]
async fn a_static_file_is_streamed_with_its_length_and_its_ranges() {
    let base = temp_dir("static");
    let public = base.join("public");
    std::fs::create_dir_all(&public).unwrap();
    // Five 64 KiB reads and a remainder: the file is answered in pieces, not as one buffer.
    let content: Vec<u8> = (0..300 * 1024u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(public.join("big.bin"), &content).unwrap();
    std::fs::write(base.join("report.txt"), b"registered file").unwrap();

    let mut app = AetherApp::new();
    app = app.static_dir("/assets", public.clone());
    app = app.static_file("/report", path_to_static(&base.join("report.txt")));
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/assets/big.bin", &[]).await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 200);
    let whole_file = (300 * 1024).to_string();
    assert_eq!(head.header("Content-Length"), Some(whole_file.as_str()));
    assert_eq!(head.header("Transfer-Encoding"), None);
    let Body::Sized(body) = body else {
        panic!("a file with a known length is not chunked");
    };
    assert_eq!(
        body, content,
        "every byte of a multi-chunk file arrives in order"
    );

    client
        .send_request("GET", "/assets/big.bin", &[("Range", "bytes=65530-65550")])
        .await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 206);
    assert_eq!(
        head.header("Content-Range"),
        Some("bytes 65530-65550/307200")
    );
    assert!(matches!(body, Body::Sized(body) if body == content[65530..=65550]));

    client.send_request("GET", "/report", &[]).await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("Content-Length"), Some("15"));
    assert!(matches!(body, Body::Sized(body) if body == b"registered file"));

    client.send_request("HEAD", "/assets/big.bin", &[]).await;
    let head = client.head().await;
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("Content-Length"),
        Some(whole_file.as_str()),
        "a HEAD names the length without reading the file"
    );

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// `static_file` takes the path at registration time, so the test needs a `'static` string.
fn path_to_static(path: &std::path::Path) -> &'static str {
    let leaked: &'static str = Box::leak(path.to_string_lossy().into_owned().into_boxed_str());
    leaked
}

#[tokio::test]
async fn a_streamed_file_revalidates_weakly_and_resumes_from_its_date() {
    let base = temp_dir("validator");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("data.txt"), b"0123456789").unwrap();

    let mut app = AetherApp::new();
    app = app.static_dir("/assets", base.clone());
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/assets/data.txt", &[]).await;
    let (head, _) = client.message().await;
    assert_eq!(head.status, 200);
    let etag = head
        .header("ETag")
        .expect("a file answer has a validator")
        .to_string();
    assert!(
        etag.starts_with("W/\""),
        "a streamed file cannot publish a content hash: {etag}"
    );
    let date = head
        .header("Last-Modified")
        .expect("a streamed file publishes its modification date")
        .to_string();

    client
        .send_request("GET", "/assets/data.txt", &[("If-None-Match", &etag)])
        .await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 304);
    assert!(matches!(body, Body::Sized(body) if body.is_empty()));

    // The weak tag may not satisfy If-Range; the date the origin published may.
    client
        .send_request(
            "GET",
            "/assets/data.txt",
            &[("Range", "bytes=2-5"), ("If-Range", &etag)],
        )
        .await;
    let (head, body) = client.message().await;
    assert_eq!(
        head.status, 200,
        "a weak validator must not authorise a range"
    );
    assert!(matches!(body, Body::Sized(body) if body == b"0123456789"));

    client
        .send_request(
            "GET",
            "/assets/data.txt",
            &[("Range", "bytes=2-5"), ("If-Range", &date)],
        )
        .await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 206);
    assert!(matches!(body, Body::Sized(body) if body == b"2345"));

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn an_unsatisfiable_range_is_416_with_the_full_length() {
    let base = temp_dir("range416");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("data.txt"), b"0123456789").unwrap();

    let mut app = AetherApp::new();
    app = app.static_dir("/assets", base.clone());
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request("GET", "/assets/data.txt", &[("Range", "bytes=99-")])
        .await;
    let (head, body) = client.message().await;
    assert_eq!(head.status, 416);
    assert_eq!(head.header("Content-Range"), Some("bytes */10"));
    assert!(matches!(body, Body::Sized(body) if body.is_empty()));

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}
