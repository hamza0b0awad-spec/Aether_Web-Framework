//! HTTP/2 end-to-end tests: a real rustls handshake, ALPN `h2`, a real `h2` client, real
//! sockets, and the framework's own serving core behind it — plus the cleartext prior-knowledge
//! path (`enable_h2c`) on a plaintext socket.
//!
//! These are the tests that decide whether the HTTP/2 frontend works, as opposed to whether it
//! compiles. Each one asserts something the protocol can get wrong and HTTP/1 never could: that
//! one socket carries many streams and a slow one must not block the rest, that a connection-
//! specific field from a handler must not destroy a response, that `:authority` has to satisfy
//! the same host policy a `Host` header satisfies, and that a stream that never finishes must
//! give its memory registration back.
//!
//! Certificates are minted per run by `rcgen`; nothing here is secret and nothing is committed.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aether::prelude::*;
use aether::scheduling::CONN_BASE_MEMORY;
use aether::tls::{TlsAcceptor, TlsCertSet};
use aether::{SchedulingConfig, SchedulingLayer, SchedulingResourceConfig};
use bytes::Bytes;
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

type TestError = Box<dyn std::error::Error + Send + Sync>;

struct TestServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), TestError>>,
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

    /// Ask the server to stop and wait for it to say it did. A drain that times out is a failure,
    /// not a slow test.
    async fn stop(self) {
        let Self {
            shutdown,
            task,
            addr: _,
        } = self;
        drop(shutdown.expect("shutdown sender"));
        match tokio::time::timeout(Duration::from_secs(15), task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => panic!("server returned an error: {error}"),
            Ok(Err(join)) => panic!("server task failed: {join}"),
            Err(_) => panic!("server did not finish graceful shutdown within 15s"),
        }
    }
}

struct Pki {
    chain_pem: Vec<u8>,
    key_pem: Vec<u8>,
    ca_der: CertificateDer<'static>,
}

fn pki(name: &str) -> Pki {
    use rcgen::{
        BasicConstraints, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };

    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "Aether test CA");
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");
    let issuer = Issuer::new(ca_params, ca_key);

    let mut leaf_params =
        rcgen::CertificateParams::new(vec![name.to_string()]).expect("leaf params");
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, name.to_string());
    leaf_params.use_authority_key_identifier_extension = true;
    leaf_params
        .key_usages
        .push(KeyUsagePurpose::DigitalSignature);
    leaf_params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ServerAuth);
    let leaf_key = KeyPair::generate().expect("leaf key");
    let leaf = leaf_params
        .signed_by(&leaf_key, &issuer)
        .expect("signed leaf");

    let mut chain_pem = leaf.pem().into_bytes();
    chain_pem.extend_from_slice(ca_cert.pem().as_bytes());
    Pki {
        chain_pem,
        key_pem: leaf_key.serialize_pem().into_bytes(),
        ca_der: ca_cert.der().clone(),
    }
}

fn acceptor(pki: &Pki) -> TlsAcceptor {
    let set = TlsCertSet::from_pem(&pki.chain_pem, &pki.key_pem).expect("valid material");
    TlsAcceptor::new(set).expect("acceptor")
}

/// One client-side HTTP/2 connection: the request sender, plus the driver task that must keep
/// being polled for anything on the connection to make progress.
struct H2Client {
    send: h2::client::SendRequest<Bytes>,
    origin: String,
    driver: Option<tokio::task::JoinHandle<Result<(), h2::Error>>>,
}

impl H2Client {
    /// Connect, complete TLS negotiating ALPN `h2`, and run the HTTP/2 handshake.
    ///
    /// The ALPN assertion is part of these tests, not a detail: a client that negotiated
    /// `http/1.1` would be served by the other frontend, and every expectation below would then
    /// be measuring the wrong code path.
    async fn connect(addr: SocketAddr, domain: &str, ca: &CertificateDer<'static>) -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.clone()).expect("trust anchor");
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];

        let tcp = TcpStream::connect(addr).await.expect("connect");
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let name =
            rustls::pki_types::ServerName::try_from(domain.to_string()).expect("domain is a name");
        let tls = connector.connect(name, tcp).await.expect("tls handshake");

        let negotiated = tls
            .get_ref()
            .1
            .alpn_protocol()
            .expect("server must negotiate an ALPN protocol")
            .to_vec();
        assert_eq!(
            negotiated, b"h2",
            "a client offering only h2 must be negotiated onto h2"
        );

        let (send, conn) = h2::client::handshake(tls).await.expect("h2 handshake");
        let driver = tokio::spawn(conn);
        Self {
            send,
            origin: format!("https://{domain}"),
            driver: Some(driver),
        }
    }

    /// Open a **prior-knowledge** h2c connection: no TLS, no ALPN, no upgrade request — the
    /// client simply starts the connection with the HTTP/2 preface, which is the only cleartext
    /// route HTTP/2 has (RFC 9113 §3.3).
    ///
    /// This client is the same `h2` crate the server uses, so what it proves is Aether's own
    /// cleartext path end to end. Independent-implementation evidence for h2c is recorded
    /// separately in `FEATURE_COMPLETENESS_MATRIX.md`, because no off-the-shelf cleartext HTTP/2
    /// client ships with Node's `http2`, `curl` here, or a browser.
    async fn connect_plain(addr: SocketAddr) -> Self {
        let tcp = TcpStream::connect(addr).await.expect("connect");
        let (send, conn) = h2::client::handshake(tcp).await.expect("h2c handshake");
        let driver = tokio::spawn(conn);
        Self {
            send,
            origin: format!("http://{addr}"),
            driver: Some(driver),
        }
    }

    fn uri(&self, path: &str) -> String {
        format!("{}{}", self.origin, path)
    }

    /// Send one request and read the whole response.
    async fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        headers: &[(&str, &str)],
    ) -> H2Response {
        let mut builder = http::Request::builder().method(method).uri(self.uri(path));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(()).expect("request");

        let mut send = self.send.clone();
        let (response_future, mut send_stream) = send
            .send_request(request, body.is_none())
            .expect("stream accepted");

        if let Some(bytes) = body {
            // One DATA frame with END_STREAM: these tests exercise the server's accounting of a
            // complete body, not a client that trickles (that case has its own test below).
            send_stream
                .send_data(Bytes::from(bytes), true)
                .expect("body accepted by flow control");
        }

        let response = response_future.await.expect("response head");
        read_body(response).await
    }

    /// Send a request whose URI is used exactly as written — no origin is prefixed — and leave the
    /// stream half-open when `end_stream` is false.
    ///
    /// `h2` builds the pseudo-header block from this URI, so this is how the two target shapes
    /// HTTP/2 spells differently from a URL reach the server: the bare `*` of a server-wide
    /// OPTIONS (RFC 9113 §8.3.1) and a CONNECT, which carries an authority and no path at all
    /// (§8.5). It also lets a test address one host on the stream while the TLS connection names
    /// another, which is what separates `:authority` from the peer the socket was opened to.
    async fn request_raw(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        end_stream: bool,
        within: Duration,
    ) -> H2Response {
        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(()).expect("request");

        let mut send = self.send.clone();
        let (response_future, _send_stream) = send
            .send_request(request, end_stream)
            .expect("stream accepted");

        // A refusal that never arrives is the defect, so the wait is bounded rather than
        // unlimited: a frontend that holds the answer behind a deadline fails here instead of
        // finishing green after thirty seconds.
        let response = tokio::time::timeout(within, response_future)
            .await
            .expect("response head within the bound")
            .expect("response head");
        read_body(response).await
    }

    /// Send a request head and hold its body back, then report what the frontend did while the
    /// stream was still paused.
    ///
    /// `h2` surfaces an informational response only through `ResponseFuture::poll_informational`,
    /// a poll API with no async wrapper, so [`NextEvent`] is the client half of the interim answer:
    /// without it the interim a frontend sends is invisible to a test, and a frontend that never
    /// sends one looks identical.
    async fn paused_body(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        while_paused: Duration,
    ) -> PausedStream {
        let mut builder = http::Request::builder().method(method).uri(self.uri(path));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(()).expect("request");
        let mut send = self.send.clone();
        // `end_stream` is false: the HEADERS frame arrives with no DATA behind it, which is what a
        // client that stopped to wait for its expectation actually sends.
        let (mut response_future, mut send_stream) =
            send.send_request(request, false).expect("stream accepted");

        let mut informational = Vec::new();
        let answered_early = tokio::time::timeout(while_paused, async {
            loop {
                match NextEvent(&mut response_future).await {
                    Some(Ok(head)) => informational.push(head.status().as_u16()),
                    Some(Err(error)) => panic!("informational response failed: {error}"),
                    // The queue says the next event is the final response, so this stream was
                    // answered from its head.
                    None => return true,
                }
            }
        })
        .await
        .unwrap_or(false);

        let _ = send_stream.send_data(Bytes::from(body.to_vec()), true);
        let response = tokio::time::timeout(Duration::from_secs(5), response_future)
            .await
            .expect("final response head within the bound")
            .expect("final response head");
        PausedStream {
            informational,
            answered_early,
            response: read_body(response).await,
        }
    }

    /// Send a request whose HEADERS frame already carries END_STREAM, and report what the
    /// frontend queued before its final response.
    ///
    /// This is the client that never pauses: the body — here, no body at all — is complete before
    /// the server looks at the expectation, so an interim would announce nothing. It is the h2
    /// shape of HTTP/1's "a request that already arrived owes no `100`", and unlike a streamed
    /// body it is not a race: END_STREAM is in the frame the verdict reads.
    async fn ended_at_head(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        while_watching: Duration,
    ) -> PausedStream {
        let mut builder = http::Request::builder().method(method).uri(self.uri(path));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(()).expect("request");
        let mut send = self.send.clone();
        let (mut response_future, _send_stream) =
            send.send_request(request, true).expect("stream accepted");

        let mut informational = Vec::new();
        let answered_early = tokio::time::timeout(while_watching, async {
            loop {
                match NextEvent(&mut response_future).await {
                    Some(Ok(head)) => informational.push(head.status().as_u16()),
                    Some(Err(error)) => panic!("informational response failed: {error}"),
                    None => return true,
                }
            }
        })
        .await
        .unwrap_or(false);

        let response = tokio::time::timeout(Duration::from_secs(5), response_future)
            .await
            .expect("final response head within the bound")
            .expect("final response head");
        PausedStream {
            informational,
            answered_early,
            response: read_body(response).await,
        }
    }

    /// Send a request and hand back a task that resolves with its response, so several streams
    /// can genuinely be in flight at once.
    fn begin(&mut self, method: &str, path: &str) -> tokio::task::JoinHandle<H2Response> {
        let mut send = self.send.clone();
        let request = http::Request::builder()
            .method(method)
            .uri(self.uri(path))
            .body(())
            .expect("request");
        let (response_future, _send_stream) =
            send.send_request(request, true).expect("stream accepted");
        tokio::spawn(async move { read_body(response_future.await.expect("response head")).await })
    }

    async fn close(mut self) {
        if let Some(driver) = self.driver.take() {
            drop(self.send);
            // The driver resolves when the connection ends. Left unbounded, a connection that
            // never closes would hang the test instead of failing it, so it is timed.
            match tokio::time::timeout(Duration::from_secs(10), driver).await {
                Ok(Ok(_)) => {}
                Ok(Err(join)) => panic!("h2 client driver failed: {join}"),
                Err(_) => panic!("h2 client driver never finished: the connection did not close"),
            }
        }
    }
}

/// The next event queued on a stream whose final response has not arrived: `Some` is a 1xx
/// informational response, `None` says the event behind it is the final one.
///
/// This is `h2`'s own `ResponseFuture::poll_informational` contract, wrapped because the crate
/// offers no async form of it.
struct NextEvent<'a>(&'a mut h2::client::ResponseFuture);

impl std::future::Future for NextEvent<'_> {
    type Output = Option<Result<http::Response<()>, h2::Error>>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.get_mut().0.poll_informational(cx)
    }
}

/// What a frontend did to a stream whose body was deliberately held back.
struct PausedStream {
    /// Every 1xx the frontend sent while the body was still unsent, in arrival order.
    informational: Vec<u16>,
    /// True when the final response was already queued before a single body byte was written:
    /// the frontend answered the head alone, which is what a refusal must do.
    answered_early: bool,
    response: H2Response,
}

struct H2Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    /// The payload exactly as sent. A coded body is not UTF-8, so a pin about gzip has to read
    /// bytes rather than the lossy string above.
    raw: Vec<u8>,
}

impl H2Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }
}

/// Poll until the layer registers `expected` bytes, then report what it holds.
///
/// A stream's registration is released by the stream's own task, which finishes on its own
/// schedule, so the assertion is "it settles", with a bound so a leak fails rather than hangs.
async fn wait_for_registration(layer: &SchedulingLayer, expected: u64) -> u64 {
    for _ in 0..200 {
        let now = layer.resources().registered_memory_bytes;
        if now == expected {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    layer.resources().registered_memory_bytes
}

async fn read_body(response: http::Response<h2::RecvStream>) -> H2Response {
    let (parts, mut body) = response.into_parts();
    let mut payload = Vec::new();
    while let Some(chunk) = body.data().await {
        let len = chunk.as_ref().map(|c| c.len()).unwrap_or(0);
        let chunk = chunk.expect("body chunk");
        payload.extend_from_slice(&chunk);
        drop(chunk);
        // Same rule the server is held to, and the client has to obey it too: `h2` credits the
        // peer only when the application releases capacity, so a harness that never releases
        // stalls its own server after one receive window and turns a large response into a
        // timeout. Errors here mean the stream already ended, which the loop below detects.
        let _ = body.flow_control().release_capacity(len);
    }
    H2Response {
        status: parts.status.as_u16(),
        headers: parts
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
        body: String::from_utf8_lossy(&payload).into_owned(),
        raw: payload,
    }
}

/// The application every test below runs, so a difference in outcome can only come from the
/// protocol, not from the routes.
fn build_app(config: Option<AetherConfig>) -> AetherApp {
    let mut app = match config {
        Some(config) => AetherApp::with_config(config),
        None => AetherApp::new(),
    };
    app = app.get("/fast", |_| Response::text("fast"));
    app = app.get_async("/slow", |_req: RequestContext| async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        Response::text("slow")
    });
    app = app.get("/payload", |_| Response::text("hello-payload"));
    app = app.post_async("/echo", |req: RequestContext| async move {
        Response::text_dynamic(format!(
            "method={}|path={}|query={}|x-marker={}|body={}",
            req.method,
            req.path,
            req.query_str.as_deref().unwrap_or(""),
            req.header("x-marker").unwrap_or(""),
            String::from_utf8_lossy(&req.body),
        ))
    });
    app = app.get("/closing", |_| {
        let mut response = Response::text("with-connection-field");
        // Exactly what an HTTP/1 handler may legitimately do, and what must not be able to
        // destroy the same response on a multiplexed connection.
        response.add_header("Connection", "close");
        response
    });
    // A handler that dies. Only the panic test asks for it, and what it proves is that the panic
    // is contained by the task boundary the dispatch already runs every handler on.
    app = app.get("/boom", |_| -> Response {
        panic!("deliberate: a panicking handler must cost one response only")
    });
    app
}

fn tls_app(pki: &Pki) -> AetherApp {
    build_app(None).tls(acceptor(pki))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2_over_tls_answers_a_post_with_framing_and_headers() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(tls_app(&pki)).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client
        .request(
            "POST",
            "/echo?flag=on",
            Some(b"payload-bytes".to_vec()),
            &[("x-marker", "marked")],
        )
        .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body, "method=POST|path=/echo|query=flag=on|x-marker=marked|body=payload-bytes",
        "the h2 frontend must produce the same request view HTTP/1 does"
    );
    assert_eq!(
        response.header("content-length"),
        Some(response.body.len().to_string().as_str()),
        "content-length must describe the payload that was sent"
    );
    assert!(response.has("date"), "a response must carry a Date");
    assert!(
        response.header("x-request-id").is_some(),
        "the core's request id must reach an h2 response too"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiplexed_streams_are_served_concurrently() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(tls_app(&pki)).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // One slow stream opened first, then a fast one on the same connection. If the accept loop
    // ever awaited a handler, the fast stream could only start after the slow one finished.
    let slow = client.begin("GET", "/slow");
    let started = Instant::now();
    let fast = client.request("GET", "/fast", None, &[]).await;
    let fast_elapsed = started.elapsed();

    assert_eq!(fast.status, 200);
    assert_eq!(fast.body, "fast");
    assert!(
        fast_elapsed < Duration::from_millis(300),
        "a fast stream waited {fast_elapsed:?} behind a 400ms one: something is awaiting a \
         handler instead of returning to accept()"
    );

    let slow = slow.await.expect("slow stream");
    assert_eq!((slow.status, slow.body.as_str()), (200, "slow"));

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn head_carries_the_framing_and_no_payload() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(tls_app(&pki)).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client.request("HEAD", "/payload", None, &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body, "", "HEAD must not carry a payload");
    assert_eq!(
        response.header("content-length"),
        Some("13"),
        "HEAD must still describe what GET would have returned"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authority_satisfies_the_host_policy_a_host_header_would() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    app = app.enable_host_validation(["aether-h2.test"]);
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // An h2 client sends `:authority`, never a `Host` header. A frontend that did not fold one
    // into the other would answer 421 to a perfectly valid request.
    let response = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        response.status, 200,
        ":authority must satisfy host validation, got {} {}",
        response.status, response.body
    );

    client.close().await;
    server.stop().await;
}

/// A route that reports the `Host` field its handler is handed. The precedence itself is decided
/// one layer down, in `header_store`; this is the value a request author and a security decision
/// both end up reading, so it has to be measured on the response.
fn host_report_app() -> AetherApp {
    let mut app = build_app(None);
    app = app.get_async("/host", |req: RequestContext| async move {
        Response::text_dynamic(format!(
            "single={}|multi={:?}",
            req.header("host").unwrap_or("<none>"),
            req.header_values("host"),
        ))
    });
    app
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_authority_replaces_a_disagreeing_host_on_the_wire() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(host_report_app().tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // RFC 9113 §8.3.1: when `:authority` is present the recipient MUST NOT use `Host` to work out
    // the target URI, and an intermediary that rebuilds one field from the other "replaces any
    // existing Host field to avoid potential vulnerabilities in HTTP routing". One field must
    // survive, and it must be the addressed one — a handler that sees the other name is routing
    // virtual-host logic the frontend already decided against.
    let disagreed = client
        .request_raw(
            "GET",
            "https://aether-h2.test/host",
            &[("host", "disagreeing.test")],
            true,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(
        (disagreed.status, disagreed.body.as_str()),
        (200, r#"single=aether-h2.test|multi=["aether-h2.test"]"#),
        "the authority must be the only host the handler sees, got {} {}",
        disagreed.status,
        disagreed.body
    );

    // Two `Host` lines are the same case with more noise: the fold collapses them with the
    // replacement, it does not leave the duplicates behind for the multi store.
    let duplicated = client
        .request_raw(
            "GET",
            "https://aether-h2.test/host",
            &[("host", "one.test"), ("host", "two.test")],
            true,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(
        (duplicated.status, duplicated.body.as_str()),
        (200, r#"single=aether-h2.test|multi=["aether-h2.test"]"#),
        "duplicate Host lines must not survive the authority, got {} {}",
        duplicated.status,
        duplicated.body
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_validation_reads_the_authority_and_not_the_field_it_replaces() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    app = app.enable_host_validation(["aether-h2.test"]);
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    // The TLS connection is named by SNI; the stream is addressed by `:authority`, which `h2`
    // takes from the URI given to `request_raw`. Only the second request needs that gap: a field
    // on the list must not launder a target that is not on it.
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let allowed = client
        .request_raw(
            "GET",
            "https://aether-h2.test/fast",
            &[("host", "evil.test")],
            true,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(
        allowed.status, 200,
        "the allow-list must read the authority, not the replaced field, got {}",
        allowed.status
    );

    let refused = client
        .request_raw(
            "GET",
            "https://evil.test/fast",
            &[("host", "aether-h2.test")],
            true,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "an on-list Host must not launder an off-list authority, got {}",
        refused.status
    );
    assert!(
        !refused.body.contains("evil.test"),
        "the refusal must not reflect the rejected authority: {:?}",
        refused.body
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn options_asterisk_answers_for_the_whole_server_over_h2() {
    let server = TestServer::start(build_app(None).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // RFC 9113 §8.3.1 does not drop the asterisk form: "A request in asterisk form (for OPTIONS)
    // includes the value '*' for the :path pseudo-header field". So the server-wide answer the
    // HTTP/1 side gives belongs here too, and `h2` hands it through as a path of exactly `*`.
    let response = client
        .request_raw("OPTIONS", "*", &[], true, Duration::from_secs(5))
        .await;
    assert_eq!(
        response.status, 204,
        "OPTIONS * must be answered for the server, got {} {}",
        response.status, response.body
    );
    let allow = response
        .header("allow")
        .expect("a server-wide OPTIONS must advertise Allow")
        .to_string();
    for method in ["GET", "HEAD", "OPTIONS", "POST"] {
        assert!(
            allow.contains(method),
            "{method} is routed on this server but is missing from {allow:?}"
        );
    }

    let neighbour = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (neighbour.status, neighbour.body.as_str()),
        (200, "fast"),
        "the connection did not survive the request it answered"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_asterisk_on_a_method_that_is_not_options_is_answered_400_over_h2() {
    let server = TestServer::start(build_app(None).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // The four request-targets are a closed, method-conditioned set (RFC 9112 §3.2, carried into
    // HTTP/2 by §8.3.1), and the HTTP/1 parser refuses a `*` on any method but OPTIONS. This
    // frontend has to refuse it the same way: `h2` sees a perfectly well-formed path string, so
    // nothing below here knows the asterisk was never a path. Routing it instead answers 404,
    // which tells the client that a resource named `*` was looked for and not found.
    let refused = client
        .request_raw("GET", "*", &[], true, Duration::from_secs(5))
        .await;
    assert_eq!(
        refused.status, 400,
        "a `*` target on GET is not a request-target this server routes, got {}",
        refused.status
    );

    let neighbour = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (neighbour.status, neighbour.body.as_str()),
        (200, "fast"),
        "the connection did not survive the request it refused"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_that_is_not_origin_form_is_reset_and_the_connection_survives() {
    let server = TestServer::start(build_app(None).enable_h2c()).await;

    // Everything §8.3.1 forbids in `:path` — empty, absolute-form, or without the leading `/` —
    // is malformed (§8.1.1), and §8.1.1's required handling is a stream error of type
    // PROTOCOL_ERROR. These three streams are written raw because no client library will send
    // them; what is being pinned is that the reset costs the stream and not the socket, which is
    // the whole point of a multiplexed protocol.
    let mut frames = Vec::new();
    for (index, path) in ["", "http://other.test/fast", "fast"].iter().enumerate() {
        let stream_id = 1 + 2 * index as u32;
        frames.extend(h2_frame(
            1,
            0x05,
            stream_id,
            &h2_head(&[
                (":method", "GET"),
                (":scheme", "http"),
                (":authority", "aether-h2c.test"),
                (":path", path),
            ]),
        ));
    }
    // One ordinary stream behind them, on the same connection.
    let survivor = 1 + 2 * 3;
    frames.extend(h2_frame(
        1,
        0x05,
        survivor,
        &h2_head(&[
            (":method", "GET"),
            (":scheme", "http"),
            (":authority", "aether-h2c.test"),
            (":path", "/fast"),
        ]),
    ));

    let mut tcp = handframed(server.addr, &frames).await;
    let read = read_frames(&mut tcp).await;
    let (_, reset) = tally(&read);
    let mut codes: Vec<(u32, u32)> = reset
        .into_iter()
        .filter(|(id, _)| *id != survivor)
        .collect();
    codes.sort_unstable();
    assert_eq!(
        codes,
        vec![(1, 1), (3, 1), (5, 1)],
        "RFC 9113 §8.1.1: each malformed target must cost its stream a PROTOCOL_ERROR"
    );

    let answered: Vec<u32> = {
        let mut ids: Vec<u32> = read
            .iter()
            .filter(|(kind, _, _)| *kind == 1)
            .map(|(_, id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(
        answered,
        vec![survivor],
        "the well-formed stream behind them must still be answered on that connection"
    );

    drop(tcp);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connect_is_answered_from_the_head_without_a_body() {
    let server = TestServer::start(build_app(None).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // RFC 9113 §8.5: a CONNECT carries an authority and *omits* `:scheme` and `:path`, and ending
    // the stream means a TCP FIN — which a client only sends once the tunnel exists. So this is
    // the shape of a conformant CONNECT, and this server does not tunnel: the answer belongs to
    // the head (RFC 9110 §9.3.6 gives 405 or 501, and 501 names the missing capability). Waiting
    // for a body first would leave the refusal behind the receive deadline instead.
    let refused = client
        .request_raw(
            "CONNECT",
            "http://aether-h2c.test:8080",
            &[],
            false,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(
        refused.status, 501,
        "a CONNECT this server cannot tunnel must be answered as a missing capability, got {}",
        refused.status
    );

    let neighbour = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (neighbour.status, neighbour.body.as_str()),
        (200, "fast"),
        "the connection did not survive the CONNECT it declined"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_field_from_a_handler_does_not_reset_the_stream() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(tls_app(&pki)).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client.request("GET", "/closing", None, &[]).await;

    assert_eq!(response.status, 200, "the stream must still be answered");
    assert_eq!(response.body, "with-connection-field");
    assert!(
        !response.has("connection"),
        "RFC 9113 §8.2.2: a connection-specific field must not reach an h2 client"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_route_reached_over_h2_answers_400() {
    let pki = pki("aether-h2.test");
    // The counter is the point of this assertion, not the status. This refusal is supposed to be
    // a *pre-chain* gate: `allows_upgrade` is false for the whole frontend, so the request never
    // reaches a middleware. A test that only checks `400` would also pass if the chain ran and
    // some link answered 400 — which is a different contract, and a much more expensive one.
    let chain_runs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&chain_runs);
    let mut app = build_app(None);
    // A policy that asks for a nonce makes the same "no chain ran" claim observable on the wire:
    // a nonce is drawn only where a handler can render it, so this refusal must carry the policy
    // with the source expression removed.
    app = app.enable_security_headers_with(SecurityHeadersMiddleware {
        content_security_policy: Some("default-src 'self'; script-src 'self' 'nonce-{nonce}'"),
        ..SecurityHeadersMiddleware::default()
    });
    app = app.use_middleware(move |req: RequestContext, next: Next| {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::Relaxed);
            next(req).await
        }
    });
    app = app.ws("/live", |_ctx, _socket| async move {});
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // HTTP/2 has no Upgrade mechanism. A 101 here would be a framing lie on a multiplexed
    // stream, so the core refuses the request instead of handing the socket over.
    //
    // The upgrade fields themselves are not sent: a conformant h2 client refuses to build a
    // request carrying `connection` or `upgrade` at all (this harness's client answers
    // `MalformedHeaders`), so the frontend's `allows_upgrade: false` is a guarantee the core
    // enforces on a GET that reaches a WebSocket route — not a response to an upgrade attempt.
    let response = client
        .request(
            "GET",
            "/live",
            None,
            &[
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
                ("sec-websocket-version", "13"),
            ],
        )
        .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        chain_runs.load(Ordering::Relaxed),
        0,
        "the HTTP/2 refusal must come from the transport gate, before the middleware chain"
    );
    let policy = response
        .header("content-security-policy")
        .expect("the refusal carries the configured policy");
    assert!(
        !policy.contains("nonce-"),
        "no chain ran, yet the policy names a nonce: {policy}"
    );
    assert_eq!(policy, "default-src 'self'; script-src 'self'");

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_101_from_a_middleware_is_refused_over_h2() {
    let pki = pki("aether-h2.test");
    // The refusal is made by the core, before either frontend writes anything, so it cannot be an
    // HTTP/1-only rule: the same guard claim on the same route gets the same answer here.
    // Middleware registered after a route does not wrap it, so the chain goes on first.
    let mut app = AetherApp::new();
    app = app.use_middleware(|_req: RequestContext, _next: Next| async move {
        Response::status(101, "Switching Protocols")
    });
    app = app.get("/fast", |_| Response::text("fast"));
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client.request("GET", "/fast", None, &[]).await;

    assert_eq!(
        response.status, 500,
        "a 101 no handoff produced must not reach an h2 stream: {}",
        response.body
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_over_the_budget_is_413_and_the_connection_still_works() {
    let pki = pki("aether-h2.test");
    let config = AetherConfig {
        max_body_bytes: 16,
        ..AetherConfig::default()
    };
    let server = TestServer::start(build_app(Some(config)).tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client
        .request("POST", "/echo", Some(vec![b'x'; 64]), &[])
        .await;
    assert_eq!(response.status, 413, "an oversized body must be refused");

    // The refusal belongs to the stream, never to the connection: the socket is still good, and
    // so is the accounting behind it.
    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(after.status, 200, "the connection survived the refusal");
    assert_eq!(after.body, "fast");

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_larger_than_the_receive_window_arrives_whole() {
    // h2 credits the peer for received data only when the application releases the capacity, and
    // 256 KiB is four receive windows. Before that release existed, this is the request that never
    // finished: the server read one window, waited for a second that flow control no longer
    // allowed the client to send, and answered 408 at the receive deadline.
    let pki = pki("aether-h2.test");
    let server = TestServer::start(tls_app(&pki)).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let body = vec![b'k'; 256 * 1024];
    let response = client
        .request(
            "POST",
            "/echo",
            Some(body.clone()),
            &[("x-marker", "big-upload")],
        )
        .await;

    assert_eq!(
        response.status, 200,
        "a whole upload must be served, not timed out"
    );
    assert!(
        response.body.contains("x-marker=big-upload"),
        "headers: {}",
        &response.body[..60.min(response.body.len())]
    );
    let echoed = response.body.split_once("|body=").expect("echo shape").1;
    assert_eq!(echoed.len(), body.len(), "the body arrived truncated");
    assert!(
        echoed.bytes().all(|b| b == b'k'),
        "the body arrived corrupted"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_over_the_limit_is_413_when_the_limit_is_crossed_mid_stream() {
    // The 16-byte-limit test above refuses on the first frame. This is the case a real upload
    // hits: many accepted frames, and the limit crossed somewhere in the middle.
    let pki = pki("aether-h2.test");
    let config = AetherConfig {
        max_body_bytes: 128 * 1024,
        ..AetherConfig::default()
    };
    let server = TestServer::start(build_app(Some(config)).tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client
        .request("POST", "/echo", Some(vec![b'm'; 256 * 1024]), &[])
        .await;
    assert_eq!(
        response.status, 413,
        "a body that crosses the budget mid-stream is refused"
    );

    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(after.status, 200, "the connection survived the refusal");
    assert_eq!(after.body, "fast");

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_body_gets_a_response_instead_of_a_hang() {
    let pki = pki("aether-h2.test");
    let config = AetherConfig {
        max_receive_time: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let server = TestServer::start(build_app(Some(config)).tls(acceptor(&pki))).await;
    let client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // Open a stream, send half a body, and never end it. Without a receive deadline the task,
    // its buffer and its stream registration would live as long as the connection does.
    let mut send = client.send.clone();
    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/echo"))
        .body(())
        .expect("request");
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");
    send_stream
        .send_data(Bytes::from_static(b"half"), false)
        .expect("first chunk accepted");

    let started = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(5), response_future)
        .await
        .expect("a stalled body must be answered, not waited on forever")
        .expect("response head");
    // The body of the refusal is read for real: an unread response body keeps the client's own
    // stream open, and a connection with an open stream does not close.
    let response = read_body(response).await;
    assert_eq!(response.status, 408);
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "the answer came before the deadline it is supposed to be enforcing"
    );

    drop(send_stream);
    // A `SendRequest` handle keeps the connection open: h2 closes once *every* clone is dropped,
    // so this extra handle has to go before `close()` can see the connection end.
    drop(send);
    client.close().await;
    server.stop().await;
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().enumerate().fold(0u64, |acc, (index, byte)| {
        acc.wrapping_add((*byte as u64).wrapping_mul(index as u64 + 1))
    })
}

/// A `route_stream` route is dispatched from its head on HTTP/2, exactly as it is on HTTP/1. The
/// handler signals before it has read a byte and the test waits for that signal before sending
/// any DATA, so a frontend that collected the body first would deadlock here rather than pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_h2_route_is_dispatched_before_its_body_arrives() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&entered);
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", move |mut req: RequestContext| {
        let signal = Arc::clone(&signal);
        async move {
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
                    Err(error) => return Response::text_dynamic(format!("error: {error}")),
                }
            }
            Response::text_dynamic(format!(
                "buffered={buffered} chunks={chunks} bytes={bytes} sum={}",
                checksum(&all)
            ))
        }
    });
    app = app.get("/fast", |_| Response::text("fast"));

    let pki = pki("aether-h2.test");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/up"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");

    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("the handler was not dispatched before the body was sent");

    let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    send_stream
        .send_data(Bytes::from(body.clone()), true)
        .expect("body accepted by flow control");

    let response = read_body(response_future.await.expect("response head")).await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert!(
        response.body.starts_with("buffered=0 "),
        "a streamed request must carry an empty body copy: {}",
        response.body
    );
    assert!(
        response.body.contains(&format!("bytes={} ", body.len()))
            && response.body.ends_with(&format!("sum={}", checksum(&body))),
        "the handler must receive the whole body it pulled: {}",
        response.body
    );

    // The handler read the body to its end, so this stream's framing is complete and the
    // connection is untouched.
    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (after.status, after.body.as_str()),
        (200, "fast"),
        "the connection must still serve after a streamed request"
    );

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// The streamed body crosses the default 65,535-byte receive window four times. It can only
/// arrive whole if the pump credits the peer as the handler takes chunks: a pump that never calls
/// `release_capacity` reads one window and then waits forever for bytes flow control forbids.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_h2_body_arrives_whole_across_receive_windows() {
    let mut app = AetherApp::new();
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        // Reported so a frontend that buffered the body instead of streaming it cannot pass by
        // producing the same bytes in the same order.
        let lane = req.body_stream().is_some();
        match req.read_body(usize::MAX).await {
            Ok(body) => Response::text_dynamic(format!(
                "lane={lane} bytes={} sum={}",
                body.len(),
                checksum(&body)
            )),
            Err(error) => Response::text_dynamic(format!("error: {error}")),
        }
    });

    let pki = pki("aether-h2.test");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/up"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");

    let body: Vec<u8> = (0..256 * 1024u32).map(|i| (i % 199) as u8).collect();
    for chunk in body.chunks(16 * 1024) {
        send_stream
            .send_data(Bytes::copy_from_slice(chunk), false)
            .expect("chunk accepted");
    }
    send_stream.send_data(Bytes::new(), true).expect("end");

    let response = read_body(response_future.await.expect("response head")).await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        response.body,
        format!("lane=true bytes={} sum={}", body.len(), checksum(&body)),
        "the body reached the handler truncated, corrupted, or already buffered"
    );

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// A `content-length` the streaming ceiling forbids is refused before dispatch, the same rule
/// HTTP/1 applies. The handler is what proves it: its counter never moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch_over_h2() {
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
    app = app.get("/fast", |_| Response::text("fast"));

    let pki = pki("aether-h2.test");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/up"))
        .header("content-length", "4096")
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response_future, send_stream) = send.send_request(request, false).expect("stream");

    let response = read_body(response_future.await.expect("response head")).await;
    assert_eq!(response.status, 413, "{}", response.body);
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        0,
        "the handler must not run for a body its route already refused by length"
    );

    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        after.status, 200,
        "the refusal belongs to the stream, not to the connection"
    );
    assert_eq!(after.body, "fast");

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// A body that crosses the ceiling while it arrives fails the handler's read, and the handler
/// answers `413` itself — the framework cannot answer for a handler that has not returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_h2_body_past_the_ceiling_mid_stream_fails_the_read() {
    let mut app = AetherApp::with_config(AetherConfig {
        max_stream_body_bytes: 64,
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(4096).await {
            Ok(body) => Response::text_dynamic(format!("{}", body.len())),
            Err(BodyError::TooLarge) => Response::status(413, "Payload Too Large"),
            Err(_) => Response::status(400, "Bad Request"),
        }
    });
    app = app.get("/fast", |_| Response::text("fast"));

    let pki = pki("aether-h2.test");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/up"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");
    // No declared length: the ceiling has to be met while the body is arriving, which is the case
    // a real upload hits.
    let mut sent = 0;
    for _ in 0..4 {
        match send_stream.send_data(Bytes::from_static(&[b'x'; 50]), false) {
            Ok(()) => sent += 50,
            // Once the handler's read has failed, the server may reset the request stream, and a
            // later write is then refused on the client. That refusal follows the crossing this
            // test is about, so it is not the property under test.
            Err(_) => break,
        }
    }
    assert!(
        sent > 64,
        "the body must cross the 64-byte ceiling before any reset: only {sent} bytes were enqueued"
    );

    let response = read_body(response_future.await.expect("response head")).await;
    assert_eq!(response.status, 413, "{}", response.body);

    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(after.status, 200, "the connection survived the refusal");
    assert_eq!(after.body, "fast");

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// A handler that answers without reading its body leaves DATA on the stream. On HTTP/1 that
/// closes the connection (the tail cannot be framed as another request); on HTTP/2 the stream
/// carries its own framing, so the connection serves the next stream normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unread_h2_body_answers_and_the_connection_stays_usable() {
    let mut app = AetherApp::new();
    app = app.route_stream(
        Method::Post,
        "/ignore",
        |mut req: RequestContext| async move {
            // Which path served this request is the whole point: a buffered frontend has already
            // collected the body, so it answers `done-buffered` even though it never read it here.
            if req.body_stream().is_some() {
                "done-streamed"
            } else {
                "done-buffered"
            }
        },
    );
    app = app.get("/fast", |_| Response::text("fast"));

    let pki = pki("aether-h2.test");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/ignore"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");
    send_stream
        .send_data(Bytes::from(vec![b'y'; 64 * 1024]), true)
        .expect("body accepted");

    let response = read_body(response_future.await.expect("response head")).await;
    assert_eq!(
        (response.status, response.body.as_str()),
        (200, "done-streamed"),
        "an answer that never read the body must still be written, from the streamed path"
    );

    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (after.status, after.body.as_str()),
        (200, "fast"),
        "an unread body costs one stream on HTTP/2, never the connection"
    );

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// A body that stops arriving is the streamed route's `receive_timeout`, and the handler — not
/// the framework — is the party that answers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_h2_body_that_stops_arriving_times_out_for_the_handler() {
    let mut app = AetherApp::with_config(AetherConfig {
        max_receive_time: Duration::from_millis(300),
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", |mut req: RequestContext| async move {
        match req.read_body(1024).await {
            Ok(body) => Response::text_dynamic(format!("{}", body.len())),
            // The body distinguishes this answer from one the framework writes when *it* times a
            // stalled body out: only the handler's read returns `BodyError::Timeout`.
            Err(BodyError::Timeout) => Response::json_status(
                408,
                "Request Timeout",
                Bytes::from_static(b"{\"by\":\"handler\"}"),
            ),
            Err(_) => Response::status(400, "Bad Request"),
        }
    });

    let pki = pki("aether-h2.test");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/up"))
        .header("content-length", "10")
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");
    // Three of ten declared bytes, and the stream is left open: the rest never arrives.
    send_stream
        .send_data(Bytes::from_static(b"abc"), false)
        .expect("chunk accepted");

    let started = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(5), response_future)
        .await
        .expect("a stalled body must be answered, not waited on forever")
        .expect("response head");
    let response = read_body(response).await;
    assert_eq!(response.status, 408, "{}", response.body);
    assert_eq!(
        response.body, "{\"by\":\"handler\"}",
        "the deadline has to reach the handler's read, not answer for it"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "the answer came before the deadline it is supposed to be enforcing"
    );

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plaintext_http1_still_shares_the_port_with_h2() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    app = app.get("/ping", |_| Response::text("pong"));
    let server = TestServer::start(app.tls(acceptor(&pki))).await;

    // With ALPN now offering `h2` by default, a plaintext client on the same port must still get
    // HTTP/1.1: choosing a frontend by negotiation happens on a TLS connection and nowhere else.
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(b"GET /ping HTTP/1.1\r\nHost: aether-h2.test\r\nConnection: close\r\n\r\n")
        .await
        .expect("write request");
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut text))
        .await
        .expect("plaintext must be answered")
        .expect("read response");
    assert!(
        text.starts_with("HTTP/1.1 200 OK"),
        "plaintext on the same port got: {text}"
    );

    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;
    let response = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(response.status, 200, "h2 on the same port still works");

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_drains_streams_in_flight() {
    let pki = pki("aether-h2.test");
    let started = Arc::new(AtomicBool::new(false));
    let started_handler = Arc::clone(&started);
    let mut app = AetherApp::new();
    app = app.get_async("/slow", move |_: RequestContext| {
        let started = Arc::clone(&started_handler);
        async move {
            started.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(400)).await;
            Response::text("slow")
        }
    });
    // `shutdown` is taken out of the server rather than dropped, so the listener is asked to
    // drain while a stream is still inside its handler.
    let mut server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let slow = client.begin("GET", "/slow");
    // The handler running is the only proof the server has read this stream's HEADERS. A sleep
    // races the request's arrival instead: `h2`'s final GOAWAY names the last stream id it
    // *received*, so a request still in the socket is one the client is told it may retry, and
    // the response future fails with `GOAWAY`/`NO_ERROR` rather than a response.
    for _ in 0..200 {
        if started.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        started.load(Ordering::SeqCst),
        "the handler never started, so no stream was in flight to drain"
    );
    drop(server.shutdown.take().expect("shutdown sender"));

    let response = tokio::time::timeout(Duration::from_secs(10), slow)
        .await
        .expect("an in-flight stream must be allowed to finish")
        .expect("stream task");
    assert_eq!((response.status, response.body.as_str()), (200, "slow"));

    match tokio::time::timeout(Duration::from_secs(15), server.task)
        .await
        .expect("the listener must finish its drain")
    {
        Ok(Ok(())) => {}
        Err(join) => panic!("serve task failed: {join}"),
        Ok(Err(error)) => panic!("serve returned an error: {error}"),
    }
    client.close().await;
}

// ---------------------------------------------------------------------------------------------
// PROB-0021: the idle timer on a connection is not a progress timer for its streams.
//
// Measured before the fix: a 64 MiB h2 response was dropped at `keep_alive_timeout` (60 s)
// with the client having consumed 0 bytes. These four tests pin the required behaviour: a
// stream that keeps being read outlives the idle timeout; a reader that stops consuming is
// bounded by `write_timeout` once h2's send buffer is full, with an RST_STREAM(CANCEL) rather
// than a silent cut; and the connection itself stays usable. A body that fits h2's send buffer
// never arms that timer and is bounded by the connection's idle timer instead (see the
// `write_timeout` note on `send` in http2.rs). Shutdown is a separate cut: the listener shares
// the same drain budget and force-aborts at it, so T6 below pins the combined path; the
// connection's own deadline is pinned separately by the unit test in http2.rs, which drives
// `serve` with no listener in the path.
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_active_response_outlives_the_idle_keep_alive_timeout() {
    let config = AetherConfig {
        // The transfer below takes ~2.4 s (16 chunks, 150 ms apart), so it outlives this twice
        // over. The margin above the client's local drain is deliberate: flow control lets the
        // server finish up to one window (64 KiB = four chunks, ~600 ms here) ahead of the
        // application, and the idle timer legitimately starts when the server's stream closes.
        // The request that asserts the connection is still usable must not race that close.
        keep_alive_timeout: Duration::from_millis(1000),
        // Deliberately below the ~2.4 s transfer: this pins the *interval* semantics. A total
        // per-response cap would reset the stream, while an interval renewed by real send
        // progress must not.
        max_response_time: Duration::from_millis(800),
        ..AetherConfig::default()
    };
    let mut app = build_app(Some(config));
    app = app.get("/slow-body", |_| {
        Response::bytes(
            Bytes::from(vec![b'x'; 256 * 1024]),
            "application/octet-stream",
        )
    });
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/slow-body"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response, hold) = send.send_request(request, true).expect("stream accepted");
    let response = response.await.expect("response head");
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    let started = Instant::now();
    let mut received = 0usize;
    let mut multiplexed = false;
    while let Some(chunk) = body.data().await {
        let chunk = chunk.expect("an actively read stream must not be cut by the idle timer");
        received += chunk.len();
        body.flow_control()
            .release_capacity(chunk.len())
            .expect("release capacity");
        if !multiplexed {
            // While this stream waits for the next window, the connection must still accept
            // and answer another stream. HEAD keeps that answer free of flow-controlled DATA,
            // so this assertion is about connection progress, not about window credit.
            let second = client.request("HEAD", "/fast", None, &[]).await;
            assert_eq!(
                second.status, 200,
                "a second stream waited behind the slow one"
            );
            multiplexed = true;
        }
        if received < 256 * 1024 {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    assert_eq!(received, 256 * 1024, "the whole body must arrive");
    assert!(
        started.elapsed() > Duration::from_millis(1000),
        "the transfer must have outlived keep_alive_timeout to prove the point"
    );

    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        after.status, 200,
        "the connection must still serve after the slow response"
    );

    drop(hold);
    drop(send);
    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_that_stops_gets_a_stream_reset_without_killing_the_connection() {
    let config = AetherConfig {
        // Longer than this test: only the response-progress bound may end the stream. The bound
        // is wide enough (1 s) that the registration read below cannot race it on a loaded host.
        keep_alive_timeout: Duration::from_secs(30),
        max_response_time: Duration::from_millis(1000),
        ..AetherConfig::default()
    };
    let mut app = build_app(Some(config));
    app = app.get("/stall-body", |_| {
        Response::bytes(
            Bytes::from(vec![b'x'; 256 * 1024]),
            "application/octet-stream",
        )
    });
    app = app.with_scheduling_resources(
        SchedulingConfig::default(),
        SchedulingResourceConfig::default(),
    );
    let layer = app.scheduling().expect("scheduling must be enabled");
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // The connection's own registration lands on the server's schedule, not on the client's
    // handshake, so the baseline is taken only once it is actually there.
    let open = wait_for_registration(&layer, CONN_BASE_MEMORY).await;
    assert_eq!(
        open, CONN_BASE_MEMORY,
        "the idle connection's own charge is the baseline this test measures against"
    );
    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/stall-body"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response, hold) = send.send_request(request, true).expect("stream accepted");
    let response = response.await.expect("response head");
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    // The response head exists only because the stream's task is alive, and that task holds the
    // stream's own charge while it writes — and the charge now covers the body's block, because
    // `Response::bytes` hands the stream a uniquely-owned allocation. So this reading is a lower
    // bound on the body, not just on the stream: the 256 KiB it keeps alive must be registered on
    // top of the connection's base. Reading it here is deterministic, unlike sampling before the
    // server has admitted the connection.
    let during = layer.resources().registered_memory_bytes;
    assert!(
        during >= open + 256 * 1024,
        "the stream writing a stalled response must keep its body's block registered: {during} vs {open}"
    );

    // The h2 driver keeps reading the socket; the application stops consuming and never
    // releases capacity. Releasing here would reopen the window and turn this into the
    // active case above.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let mut received = 0usize;
    loop {
        match tokio::time::timeout(Duration::from_secs(1), body.data()).await {
            Ok(Some(Ok(chunk))) => received += chunk.len(),
            // The stream error must be the RST_STREAM(CANCEL) this test is about, not an IO
            // failure or a connection-wide error wearing the same result type.
            Ok(Some(Err(error))) => {
                assert_eq!(
                    error.reason(),
                    Some(h2::Reason::CANCEL),
                    "the stalled stream was not reset with CANCEL: {error:?}"
                );
                break;
            }
            // A clean end would mean the server framed a truncated body as complete, which is
            // the opposite claim.
            Ok(None) => panic!(
                "the stream ended cleanly after {received} of {} bytes",
                256 * 1024
            ),
            Err(_) => panic!("the server never reset a reader that stopped consuming"),
        }
    }
    assert!(
        received < 256 * 1024,
        "a reset stream must not have delivered the whole body"
    );

    // The bytes the stalled stream received still count against the *connection* window. `h2`
    // returns that credit when the closed stream's last handle is dropped, so a client that
    // discards a dead stream — both halves of it — is what makes the connection usable for the
    // next response; holding either handle would leave the connection without send credit and
    // prove nothing about the reset.
    drop(body);
    drop(hold);

    let settled = wait_for_registration(&layer, open).await;
    assert_eq!(settled, open, "a reset stream kept its memory registration");

    let after = client.request("HEAD", "/fast", None, &[]).await;
    assert_eq!(
        after.status, 200,
        "resetting one stream must not kill the connection"
    );

    drop(send);
    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trickling_reader_is_cut_at_the_deadline_it_cannot_earn() {
    // PROB-0022's pre-declared Refute branch, measured rather than argued: the peer grants credit
    // inside every write interval, so a deadline that renews on *any* progress is never what ends
    // the stream. Renewal now costs `MIN_PROGRESS_FOR_RENEWAL` bytes (1024, in http2.rs), and a
    // grant of one byte per period cannot accumulate it before the interval expires. The old
    // version of this test asserted the opposite — survival past every deadline — and its own
    // comment said a bound had to invert it; this is that inversion.
    const BODY: usize = 256 * 1024;
    const MIN_PROGRESS: usize = 1024;
    const TIMEOUT_MS: u64 = 1500;
    const GRANT_MS: u64 = 100;
    const OBSERVE_MS: u64 = 5000;
    // The premises, as arithmetic rather than as comments: ten grant periods fit inside one write
    // deadline, so credit is never absent, and even granting at every period the interval cannot
    // reach the renewal minimum — which is what makes this a trickle and not a slow download. The
    // observation window is more than three whole deadlines long.
    const _: () = assert!(GRANT_MS * 10 <= TIMEOUT_MS);
    const _: () = assert!((TIMEOUT_MS / GRANT_MS) < MIN_PROGRESS as u64);
    const _: () = assert!(OBSERVE_MS > 3 * TIMEOUT_MS);

    let config = AetherConfig {
        // Both limits are the same value on purpose. The keep-alive timer cannot fire while a
        // stream exists, so the timer this test isolates is `write_timeout`; the equal keep-alive
        // value shows the idle timer does not cut a live stream either.
        keep_alive_timeout: Duration::from_millis(TIMEOUT_MS),
        max_response_time: Duration::from_millis(TIMEOUT_MS),
        ..AetherConfig::default()
    };
    let mut app = build_app(Some(config));
    app = app.get("/stall-body", |_| {
        Response::bytes(Bytes::from(vec![b'x'; BODY]), "application/octet-stream")
    });
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/stall-body"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response, hold) = send.send_request(request, true).expect("stream accepted");
    let response = response.await.expect("response head");
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    // One read before the loop: the stream is live now, and what follows is progress rather
    // than a stream that never started. Nothing is released here, so the server writes until
    // the client's window is spent and then has to wait for the grants below.
    let first = tokio::time::timeout(Duration::from_secs(2), body.data())
        .await
        .expect("the first frame of a 256 KiB body must arrive")
        .expect("stream end before any data")
        .expect("first chunk");
    let mut received = first.len();

    // Drain the rest of the first burst quickly, so the deadline the server last earned is the
    // one being tested and the pre-grant phase cannot itself expire. What the progress assertion
    // below measures can then only be credited to the grants.
    loop {
        match tokio::time::timeout(Duration::from_millis(100), body.data()).await {
            Ok(Some(Ok(chunk))) => received += chunk.len(),
            Ok(Some(Err(error))) => {
                panic!("the stream ended with an error before the grants began: {error:?}")
            }
            Ok(None) => panic!("the stream ended cleanly before the grants began"),
            Err(_) => break,
        }
    }
    let before_window = received;

    let started = Instant::now();
    let mut cut = None;
    while started.elapsed() < Duration::from_millis(OBSERVE_MS) {
        tokio::time::sleep(Duration::from_millis(GRANT_MS)).await;
        // Exactly one byte of credit: enough to renew a deadline that renews on any progress, far
        // too little to earn one under the renewal minimum.
        body.flow_control()
            .release_capacity(1)
            .expect("one byte of credit must be accepted");
        loop {
            match tokio::time::timeout(Duration::from_millis(20), body.data()).await {
                Ok(Some(Ok(chunk))) => received += chunk.len(),
                // Any end inside the window is what the bound now does. The reset must be this
                // stream's own CANCEL, not an IO failure or a connection-wide error wearing the
                // same result type.
                Ok(Some(Err(error))) => {
                    assert_eq!(
                        error.reason(),
                        Some(h2::Reason::CANCEL),
                        "the trickling stream was not reset with CANCEL: {error:?}"
                    );
                    cut = Some(started.elapsed());
                    break;
                }
                Ok(None) => panic!(
                    "the trickling stream ended cleanly after {received} of {BODY} bytes, \
                     which would frame a truncated body as complete"
                ),
                Err(_) => break,
            }
        }
        if cut.is_some() {
            break;
        }
    }
    let cut_at = cut.unwrap_or_else(|| {
        panic!(
            "a peer granting one byte per {GRANT_MS} ms still held the stream open for the whole \
             {OBSERVE_MS} ms window after {received} of {BODY} bytes; the write deadline is \
             renewed by progress that was never earned"
        )
    });

    // Progress, not merely a stream that never ended: bytes did flow after the window drained, so
    // this cut is not the no-reader case the stall test above already covers.
    assert!(
        received > before_window,
        "the stream was cut without moving a byte after the window was drained, so credit is what \
         was missing here, not the bytes it earned"
    );
    // And the movement stayed a trickle: less than one renewal minimum was ever handed over after
    // the window closed, which is why the deadline could not be earned.
    assert!(
        received - before_window < MIN_PROGRESS,
        "the trickling peer moved {} bytes after the window drained, so it earned a renewal and \
         this run did not measure the trickle shape",
        received - before_window
    );
    assert!(
        received < BODY,
        "the body finished inside the window, so this measured throughput rather than the bound"
    );
    // The cut landed inside the interval budget, not at some absolute total cap: at least one grant
    // period had to have been offered before it.
    assert!(
        cut_at >= Duration::from_millis(GRANT_MS),
        "the stream was cut after {cut_at:?}, before any credit was granted at all"
    );
    assert!(
        cut_at < Duration::from_millis(OBSERVE_MS),
        "the stream survived {cut_at:?} of the {OBSERVE_MS} ms window"
    );

    // The connection is still usable, which is the half a per-connection deadline would break:
    // the trickled stream did not cost the peer its other streams.
    let after = client.request("HEAD", "/fast", None, &[]).await;
    assert_eq!(
        after.status, 200,
        "a trickling stream took the connection down with it"
    );

    drop(body);
    drop(hold);
    drop(send);
    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_reader_that_earns_every_deadline_finishes_the_whole_body() {
    // The other half of PROB-0022's fix, and the test that fails if the bound is turned into the
    // absolute per-response cap D1 refused. A peer that consumes 2 KiB every 40 ms is slow — this
    // transfer outlives four write deadlines — but every interval earns its renewal, so the
    // interval semantics must let it finish. Paired with the trickle test above, the two pin the
    // shape: bytes per interval decide it, elapsed time does not.
    const BODY: usize = 100 * 1024;
    const MIN_PROGRESS: usize = 1024;
    const GRANT: usize = 2048;
    const TIMEOUT_MS: u64 = 200;
    const GRANT_MS: u64 = 40;
    // The premises: a grant is comfortably above the renewal minimum, and the credit the client
    // withholds at first (its default 65,535-byte stream window) is smaller than the body, so part
    // of this transfer can only happen through earned renewals.
    const _: () = assert!(GRANT > MIN_PROGRESS);
    const _: () = assert!(BODY > 65_535);

    let config = AetherConfig {
        keep_alive_timeout: Duration::from_secs(30),
        max_response_time: Duration::from_millis(TIMEOUT_MS),
        ..AetherConfig::default()
    };
    let mut app = build_app(Some(config));
    app = app.get("/paced-body", |_| {
        Response::bytes(Bytes::from(vec![b'x'; BODY]), "application/octet-stream")
    });
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/paced-body"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response, hold) = send.send_request(request, true).expect("stream accepted");
    let response = response.await.expect("response head");
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    let started = Instant::now();
    let mut received = 0usize;
    let mut intervals = 0usize;
    while received < BODY {
        // Take what the open window offers, one frame at a time. The first frame is the client's
        // whole default stream window, so everything after it can only arrive through pacing.
        let chunk = match tokio::time::timeout(Duration::from_secs(4), body.data()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(error))) => panic!(
                "a reader that earns every deadline was cut after {received} of {BODY} bytes in \
                 {intervals} paced intervals: {error:?}"
            ),
            Ok(None) => break,
            Err(_) => panic!(
                "the paced reader stalled after {received} of {BODY} bytes in {intervals} intervals"
            ),
        };
        received += chunk.len();
        if received >= BODY {
            break;
        }
        // Pace: hand back one interval's worth of credit - more than the renewal minimum, far less
        // than what was already buffered - then let the application lag behind it.
        body.flow_control()
            .release_capacity(GRANT)
            .expect("credit for one paced interval");
        tokio::time::sleep(Duration::from_millis(GRANT_MS)).await;
        intervals += 1;
    }
    assert_eq!(received, BODY, "the whole body must arrive");
    assert!(
        intervals >= 4,
        "the transfer needed only {intervals} paced intervals, so the window never had to be \
         reopened by earned credit and this proved nothing"
    );
    assert!(
        started.elapsed() > Duration::from_millis(3 * TIMEOUT_MS),
        "the body arrived in {:?}, inside three write deadlines, so the interval semantics were \
         not exercised",
        started.elapsed()
    );

    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        after.status, 200,
        "the connection must still serve afterwards"
    );

    drop(body);
    drop(hold);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// CONTRACT: a client that resets one stream while its handler runs cancels that handler at its
/// next await point, and the reset costs only that stream — the session still answers the next
/// request. `RST_STREAM` is HTTP/2's form of walking away from one stream; left uncancelled, a
/// walk-away would keep its handler, and the memory budget it holds, alive for the whole
/// request deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_reset_cancels_the_running_handler_and_keeps_the_session() {
    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicUsize::new(0));
    let started_handler = Arc::clone(&started);
    let finished_handler = Arc::clone(&finished);
    let mut app = AetherApp::new();
    app = app.get_async("/slow", move |_: RequestContext| {
        let started = Arc::clone(&started_handler);
        let finished = Arc::clone(&finished_handler);
        async move {
            started.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(400)).await;
            finished.fetch_add(1, Ordering::SeqCst);
            Response::text("late")
        }
    });
    app = app.get("/ok", |_| "ok");

    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/slow"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (_response, mut stream) = send.send_request(request, true).expect("stream accepted");
    for _ in 0..200 {
        if started.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        started.load(Ordering::SeqCst),
        "the handler never started, so the reset would measure nothing"
    );
    stream.send_reset(h2::Reason::CANCEL);

    // Past the handler's own 400 ms sleep: an uncancelled handler has finished by now.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the handler ran to completion after its stream was reset"
    );

    let after = client.request("GET", "/ok", None, &[]).await;
    assert_eq!(after.status, 200, "the reset must cost only its own stream");

    // The reset stream's own handles keep the client's connection state alive, so the session
    // cannot report closed until they are gone.
    drop(_response);
    drop(stream);
    drop(send);
    client.close().await;
    server.stop().await;
}

/// CONTRACT: a client that drops the whole HTTP/2 connection while a handler runs cancels that
/// handler, just as a per-stream reset does. A connection close fails every open stream at once,
/// so the per-stream watch sees it — but only if the connection's server loop gives its stream
/// tasks the chance to run before it tears them down, and does not hold them for the graceful
/// drain budget a client that is still reading would justify.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_h2_connection_cancels_its_running_handlers() {
    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicUsize::new(0));
    let started_handler = Arc::clone(&started);
    let finished_handler = Arc::clone(&finished);
    let mut app = AetherApp::new();
    app = app.get_async("/slow", move |_: RequestContext| {
        let started = Arc::clone(&started_handler);
        let finished = Arc::clone(&finished_handler);
        async move {
            started.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(400)).await;
            finished.fetch_add(1, Ordering::SeqCst);
            Response::text("late")
        }
    });

    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/slow"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response, stream) = send.send_request(request, true).expect("stream accepted");
    for _ in 0..200 {
        if started.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        started.load(Ordering::SeqCst),
        "the handler never started, so dropping the connection would measure nothing"
    );

    // The driver owns the socket, so aborting it is the client walking away from the whole
    // connection — not from one stream. Awaiting the abort is what makes the close a fact before
    // the measurement starts; the client-side handles go with it.
    let driver = client.driver.take().expect("driver task");
    driver.abort();
    let _ = driver.await;
    drop(response);
    drop(stream);
    drop(send);
    drop(client);

    // Past the handler's own 400 ms sleep: an uncancelled handler has finished by now.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the handler ran to completion after its connection was dropped"
    );

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_h2_connection_still_closes_after_keep_alive_timeout() {
    let config = AetherConfig {
        keep_alive_timeout: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let app = build_app(Some(config));
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let response = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(response.status, 200);

    // Keep `client.send` alive: dropping it would close the client side and prove nothing.
    let started = Instant::now();
    let driver = client.driver.take().expect("client driver");
    let outcome = tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("an idle h2 connection must close at keep_alive_timeout");
    match outcome {
        Ok(_) => {}
        Err(join) => panic!("h2 client driver failed: {join}"),
    }
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the connection closed before the keep-alive timeout"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the idle connection outlived its keep-alive timeout by too much"
    );

    drop(client.send);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draining_h2_connection_abandons_a_stream_at_the_drain_budget() {
    // `drain_budget` is `keep_alive_timeout.min(30s)` for the h2 connection, so 300 ms here
    // makes the drain budget 300 ms; the listener uses the same expression for its own bound.
    let config = AetherConfig {
        keep_alive_timeout: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let mut app = build_app(Some(config));
    app = app.get("/stall-body", |_| {
        Response::bytes(
            Bytes::from(vec![b'x'; 1024 * 1024]),
            "application/octet-stream",
        )
    });
    let mut server = TestServer::start(app.enable_h2c()).await;
    let client = H2Client::connect_plain(server.addr).await;

    let request = http::Request::builder()
        .method("GET")
        .uri(client.uri("/stall-body"))
        .body(())
        .expect("request");
    let mut send = client.send.clone();
    let (response, hold) = send.send_request(request, true).expect("stream accepted");
    let response = response.await.expect("response head");
    // Not released: the stream is mid-write when shutdown starts, so its task is waiting for
    // flow-control credit rather than progressing.
    let mut body = response.into_body();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    drop(server.shutdown.take().expect("shutdown sender"));

    let stopped = tokio::time::timeout(Duration::from_secs(2), server.task)
        .await
        .expect("the listener must finish even with a stalled stream in flight");
    match stopped {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("serve returned an error: {error}"),
        Err(join) => panic!("serve task failed: {join}"),
    }
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "a stream in flight was cut before its drain budget"
    );

    // The listener is gone; the body must not end as if it had been written whole. This is the
    // connection-level cut of a shutdown, not the per-stream CANCEL that `write_timeout` sends,
    // and the listener's drain uses the same budget as the connection's, so the timing below
    // pins the combined path rather than the connection deadline in isolation. The unit test
    // `the_connection_cuts_a_stalled_stream_at_its_own_drain_deadline` covers that isolation.
    let mut delivered = 0usize;
    let clean_end = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(chunk) = body.data().await {
            match chunk {
                Ok(chunk) => delivered += chunk.len(),
                Err(_) => return false,
            }
        }
        true
    })
    .await
    .expect("the abandoned stream must end after its connection is gone");
    assert!(
        !clean_end,
        "the abandoned stream ended cleanly instead of being cut"
    );
    assert!(
        delivered < 1024 * 1024,
        "the abandoned stream delivered its whole body"
    );

    drop(hold);
    drop(send);
    client.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finished_streams_leave_no_memory_registration_behind() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    app = app.with_scheduling_resources(
        SchedulingConfig::default(),
        SchedulingResourceConfig::default(),
    );
    let layer = app.scheduling().expect("scheduling must be enabled");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // "Nothing leaked" cannot be measured against zero while the connection's own charge is still
    // landing, and it lands on the server's accept schedule, not on the client's handshake. So the
    // baseline is taken only once it is actually there: the connection's `CONN_BASE_MEMORY`, and
    // nothing else. What this asserts is the stream-level charge on top of it — a finished stream
    // gives its buffer registration back while the socket behind it stays open.
    let open = wait_for_registration(&layer, CONN_BASE_MEMORY).await;
    assert_eq!(
        open, CONN_BASE_MEMORY,
        "the idle connection's own charge is the baseline this test measures against"
    );
    let mut send = client.send.clone();
    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/echo"))
        .body(())
        .expect("request");
    let (response_future, mut send_stream) = send.send_request(request, false).expect("stream");
    for chunk_index in 0..4 {
        // Trickle, then end the stream on the last frame. A POST that never sets END_STREAM has
        // no complete body, and a server that waits for one is correct — that case is the
        // stalled-body test above, not this one.
        send_stream
            .send_data(Bytes::from_static(b"1234"), chunk_index == 3)
            .expect("chunk accepted");
    }
    let response = read_body(response_future.await.expect("response head")).await;
    assert_eq!(
        (response.status, response.body.as_str()),
        (
            200,
            "method=POST|path=/echo|query=|x-marker=|body=1234123412341234"
        ),
        "a body delivered in several DATA frames must reassemble intact"
    );
    drop(send_stream);
    // The extra `SendRequest` handle must go too: h2 keeps the connection open while any clone
    // of it exists, and the registration this test measures is only released when it closes.
    drop(send);

    let served = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(served.status, 200);

    // A stream's registration is given back by its own task, which ends independently of the
    // client observing the response, so the honest question is whether it settles, not when.
    let settled = wait_for_registration(&layer, open).await;
    assert_eq!(
        settled, open,
        "a finished stream kept its buffer registration while the connection stayed open"
    );

    client.close().await;
    server.stop().await;
    assert_eq!(
        layer.resources().registered_memory_bytes,
        0,
        "the connection's own registration survived shutdown"
    );
}

/// The HTTP/2 frontend runs the handler through the same core dispatch, so a declared extension is
/// priced by the same registration there — shown here rather than assumed from code sharing. Each
/// handler samples the layer from inside the server at the same phase of its own request, so the
/// reading is not a race against a parked stream, and both requests ride one connection with the
/// same stream base and the same body admission. The difference between the two readings is the
/// extension and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_extension_is_priced_on_the_http_2_path_and_released_with_the_stream() {
    const HEAP: usize = 512 * 1024;

    fn registered_of(response: &H2Response) -> u64 {
        response
            .body
            .split('|')
            .find_map(|field| field.strip_prefix("registered="))
            .unwrap_or_else(|| panic!("handler did not report a registration: {}", response.body))
            .parse()
            .unwrap_or_else(|_| panic!("registration is not a number: {}", response.body))
    }

    let mut app = AetherApp::new();
    app = app.with_scheduling_resources(
        SchedulingConfig::default(),
        SchedulingResourceConfig::default(),
    );
    let layer = app.scheduling().expect("scheduling must be enabled");

    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        if req.path == "/ext/big" {
            req.insert_extension_sized(vec![7u8; HEAP], HEAP);
        }
        next(req).await
    });
    for (path, declares) in [("/ext/plain", false), ("/ext/big", true)] {
        let reporter = Arc::clone(&layer);
        app = app.get_async(path, move |req: RequestContext| {
            let reporter = Arc::clone(&reporter);
            async move {
                let held = if declares {
                    req.extension::<Vec<u8>>().map_or(0, Vec::len)
                } else {
                    0
                };
                Response::text_dynamic(format!(
                    "held={held}|registered={}",
                    reporter.resources().registered_memory_bytes
                ))
            }
        });
    }

    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;
    // The connection's own `CONN_BASE_MEMORY` lands on the server's accept schedule, so the
    // baseline the stream charge must return to is taken only once it is actually there. Sampling
    // it at zero would let the settle assertion below pass without proving any release.
    let open = wait_for_registration(&layer, CONN_BASE_MEMORY).await;
    assert_eq!(
        open, CONN_BASE_MEMORY,
        "the idle connection's own charge is the baseline this test measures against"
    );

    let plain = client.request("GET", "/ext/plain", None, &[]).await;
    let big = client.request("GET", "/ext/big", None, &[]).await;

    assert_eq!(plain.status, 200, "body: {}", plain.body);
    assert_eq!(big.status, 200, "body: {}", big.body);
    assert!(
        big.body.starts_with(&format!("held={HEAP}|")),
        "the handler must see the value the middleware declared: {}",
        big.body
    );
    assert!(
        plain.body.starts_with("held=0|"),
        "a route that declared nothing must see nothing: {}",
        plain.body
    );
    assert_eq!(
        registered_of(&big) - registered_of(&plain),
        aether::scheduling::EXTENSION_ENTRY_MEMORY
            + std::mem::size_of::<Vec<u8>>() as u64
            + HEAP as u64,
        "a declared payload over HTTP/2 must register the same bytes it does over HTTP/1"
    );

    // Released with the stream, not with the connection: the handler job's registration is given
    // back by the stream task while the socket behind it is still open, so this is sampled before
    // the client disconnects.
    let settled = wait_for_registration(&layer, open).await;
    assert_eq!(
        settled, open,
        "a declared extension outlived its HTTP/2 stream"
    );
    client.close().await;
    server.stop().await;
    assert_eq!(
        layer.resources().registered_memory_bytes,
        0,
        "the connection's own registration survived shutdown"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_the_memory_budget_cannot_cover_is_refused_without_harming_the_other() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    // Room for one connection, one stream and its handler — and no more. Sequential streams
    // would each succeed, because a finished stream releases its registration; the pressure only
    // appears while two are genuinely live at once, which is what HTTP/2 makes possible on one
    // socket and HTTP/1.1 does not.
    let resources = SchedulingResourceConfig {
        memory_budget_bytes: 72 * 1024,
        ..SchedulingResourceConfig::default()
    };
    app = app.with_scheduling_resources(SchedulingConfig::default(), resources);
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let slow = client.begin("GET", "/slow");
    // Wait for the admitted stream to be holding its charges inside the handler.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let second = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        second.status, 503,
        "a stream the budget cannot cover must be shed, not queued onto a stalled connection"
    );
    assert_eq!(second.header("retry-after"), Some("1"));

    // Shedding one stream must not disturb the one already running: it holds the same socket.
    let slow = slow.await.expect("the admitted stream must still finish");
    assert_eq!((slow.status, slow.body.as_str()), (200, "slow"));

    client.close().await;
    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// HTTP/2 over cleartext — prior knowledge (RFC 9113 §3.3), reached only through
// `AetherApp::enable_h2c`.
//
// These tests are the cleartext half of the same frontend, so they assert the properties that
// could only be broken by the plaintext path: that the framing decision survives the loss of
// ALPN, that `require_tls` still refuses cleartext even when HTTP/2 is offered there, that
// enabling h2c does not disturb the HTTP/1.1 connections sharing the port, and that a client
// that sends the preface and then stops talking is closed rather than held.
// ---------------------------------------------------------------------------------------------

/// Wait for the server to end a connection it was given too little of to serve, and fail if it
/// ever served anything.
///
/// Two protocol facts shape this. First, a refusal can arrive as a clean FIN *or* as a reset: the
/// client here has bytes the server never read, and dropping a socket with unread bytes in the
/// receive buffer makes Windows answer an RST. Both mean the connection is over, and unlike a
/// byte-level refusal this path has no response the client is still reading, so a reset costs
/// nothing. Second, bytes on the wire are expected: `h2` writes its own SETTINGS as soon as the
/// connection exists, without waiting for the peer's (RFC 9113 §3.5 permits SETTINGS at any time
/// after the client preface). What must never appear is a DATA or HEADERS frame — that would mean
/// a client that never finished its handshake got an answer, on a socket held open for it.
async fn closed_without_a_response(tcp: &mut TcpStream) {
    let mut bytes = Vec::new();
    match tokio::time::timeout(Duration::from_secs(10), tcp.read_to_end(&mut bytes)).await {
        Err(_) => panic!("the connection stayed open"),
        // Reset before anything readable: nothing was served, the connection is over.
        Ok(Err(_)) => return,
        Ok(Ok(_)) => {}
    }
    let mut offset = 0usize;
    while offset + 9 <= bytes.len() {
        let length = (bytes[offset] as usize) << 16
            | (bytes[offset + 1] as usize) << 8
            | bytes[offset + 2] as usize;
        let kind = bytes[offset + 3];
        assert!(
            kind != 0 && kind != 1,
            "frame type {kind} served on a connection that never finished its handshake"
        );
        offset += 9 + length;
    }
    // Every byte accounted for, so a short buffer can never make the loop above exit early and
    // pass without looking at a frame at all.
    assert_eq!(
        offset,
        bytes.len(),
        "the server left a partial frame on a connection it refused"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2c_answers_a_post_on_a_plaintext_socket_when_enabled() {
    let server = TestServer::start(build_app(None).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let response = client
        .request(
            "POST",
            "/echo?flag=on",
            Some(b"cleartext-payload".to_vec()),
            &[("x-marker", "h2c")],
        )
        .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body, "method=POST|path=/echo|query=flag=on|x-marker=h2c|body=cleartext-payload",
        "a cleartext HTTP/2 request must produce the same view HTTP/1 and TLS/HTTP/2 do"
    );
    assert_eq!(
        response.header("content-length"),
        Some(response.body.len().to_string().as_str())
    );
    assert!(
        response.header("x-request-id").is_some(),
        "the core's request id must reach a cleartext h2 response too"
    );

    // HEAD on the same cleartext connection: framing without a payload.
    let head = client.request("HEAD", "/payload", None, &[]).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty(), "HEAD must not carry a payload");
    assert_eq!(head.header("content-length"), Some("13"));

    client.close().await;
    server.stop().await;
}

/// A mounted sub-app is a registration-time chain, so the scope's state has to reach a handler on
/// whichever frontend the request arrived over. HTTP/1.1 covers this in `nested_routers.rs`; this
/// pins the second frontend, because the h2 path builds its context separately and a chain entry
/// that only the HTTP/1 loop applied would leave cleartext HTTP/2 serving the app-wide value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mounted_router_answers_with_its_own_state_over_h2c() {
    #[derive(Clone)]
    struct Tenant(&'static str);

    let mut api = Router::new();
    api.manage(Tenant("sub"));
    api.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("x-scope", "router".to_string()));
        res
    });
    api.get_async("/whoami", |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0.to_string())
            .unwrap_or_else(|| "none".to_string())
    });

    let mut app = build_app(None);
    app = app.manage(Tenant("app"));
    app = app.get_async("/state", |ctx: RequestContext| async move {
        ctx.state::<Tenant>()
            .map(|t| t.0.to_string())
            .unwrap_or_else(|| "none".to_string())
    });
    app = app.mount("/api", api);

    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let mounted = client.request("GET", "/api/whoami", None, &[]).await;
    assert_eq!((mounted.status, mounted.body.as_str()), (200, "sub"));
    assert_eq!(mounted.header("x-scope"), Some("router"));

    // The same connection asks for the app's own view of the same type: the mount shadowed it for
    // its routes only.
    let plain = client.request("GET", "/state", None, &[]).await;
    assert_eq!((plain.status, plain.body.as_str()), (200, "app"));

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2c_multiplexes_without_stealing_the_port_from_http1() {
    let mut app = build_app(None);
    app = app.get("/ping", |_| Response::text("pong"));
    let server = TestServer::start(app.enable_h2c()).await;

    // Enabling cleartext HTTP/2 must not change what an HTTP/1.1 client sees on the same port:
    // the preface is the only thing that diverts a connection.
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(b"GET /ping HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .expect("write request");
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut text))
        .await
        .expect("plaintext HTTP/1 must still be answered")
        .expect("read response");
    assert!(
        text.starts_with("HTTP/1.1 200 OK"),
        "HTTP/1.1 on an h2c port got: {text}"
    );

    let mut client = H2Client::connect_plain(server.addr).await;
    let slow = client.begin("GET", "/slow");
    let started = Instant::now();
    let fast = client.request("GET", "/fast", None, &[]).await;
    assert_eq!((fast.status, fast.body.as_str()), (200, "fast"));
    assert!(
        started.elapsed() < Duration::from_millis(350),
        "a fast stream had to wait for a slow one: {:?}",
        started.elapsed()
    );
    let slow = slow.await.expect("slow stream");
    assert_eq!((slow.status, slow.body.as_str()), (200, "slow"));

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_route_reached_over_h2c_answers_400() {
    // Same contract as the TLS variant, same reason: the counter is the assertion, not the status.
    let chain_runs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&chain_runs);
    let mut app = build_app(None);
    app = app.use_middleware(move |req: RequestContext, next: Next| {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::Relaxed);
            next(req).await
        }
    });
    app = app.ws("/live", |_ctx, _socket| async move {});
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // The refusal is a property of HTTP/2, not of the transport under it: the core is told
    // `allows_upgrade: false` for every h2 stream, cleartext included, so a WebSocket route
    // answers 400 instead of emitting a 101 onto a multiplexed stream.
    let response = client
        .request(
            "GET",
            "/live",
            None,
            &[
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
                ("sec-websocket-version", "13"),
            ],
        )
        .await;
    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        chain_runs.load(Ordering::Relaxed),
        0,
        "the cleartext HTTP/2 refusal must come from the transport gate too, before the chain"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2c_is_still_refused_when_tls_is_required() {
    let pki = pki("aether-h2.test");
    let app = build_app(None)
        .tls(acceptor(&pki))
        .require_tls(true)
        .enable_h2c();
    let server = TestServer::start(app).await;

    // `enable_h2c` opens cleartext HTTP/2, not cleartext in general. An operator who required TLS
    // gets the same refusal whichever framing the plaintext connection asked for, otherwise
    // turning h2c on would be a way to get served over `http://` on a `require_tls` port.
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .expect("send preface");
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buf))
        .await
        .expect("closed within the read timeout")
        .expect("io");
    assert_eq!(
        read, 0,
        "a required-TLS port must not serve cleartext HTTP/2"
    );

    // And the refusal is per connection: the listener is unchanged, and TLS still works.
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;
    let response = client.request("GET", "/fast", None, &[]).await;
    assert_eq!((response.status, response.body.as_str()), (200, "fast"));

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cleartext_connection_that_stops_at_the_preface_is_closed() {
    // `read_timeout` is 5s and is not configurable through `AetherConfig`, so this test is slow
    // by construction: it is waiting for the framework's own bound, not for a fixture.
    let mut app = build_app(None);
    app = app.with_scheduling_resources(
        SchedulingConfig::default(),
        SchedulingResourceConfig::default(),
    );
    let layer = app.scheduling().expect("scheduling must be enabled");
    let server = TestServer::start(app.enable_h2c()).await;

    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    // The preface only — no SETTINGS frame. `h2`'s handshake waits for both, and without a bound
    // it would wait for as long as the client likes: task, connection permit and the
    // connection's memory registration all held by a socket that never said anything else.
    tcp.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .expect("send preface");
    closed_without_a_response(&mut tcp).await;

    let settled = wait_for_registration(&layer, 0).await;
    assert_eq!(
        settled, 0,
        "a connection closed during the HTTP/2 handshake kept its registration"
    );

    // The port is still healthy for the framings it did serve.
    let mut next = TcpStream::connect(server.addr).await.expect("reconnect");
    next.write_all(b"GET /fast HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .expect("write request");
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(5), next.read_to_string(&mut text))
        .await
        .expect("answered")
        .expect("read");
    assert!(text.starts_with("HTTP/1.1 200 OK"), "got: {text}");

    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// The bounds HTTP/2 puts on the server, and the failures that must stay inside one stream.
//
// Hand-framed on purpose. A client that respects the server's SETTINGS can never produce the
// overflow it is advertising against, and a harness built on `h2`'s own client would then be
// measuring that library's courtesy rather than this server's bound. What these tests write is
// bytes; what they read is frames.
//
// `max_concurrent_streams` and `max_headers_count` are reachable through `AetherConfig`, so each
// bound is pinned twice below: once as the default it produces, read from `ServerLimits::default()`
// rather than repeated as a literal, and once as a number that only a wired configuration field
// can produce.
// ---------------------------------------------------------------------------------------------

/// One HTTP/2 frame: 24-bit length, type, flags, 31-bit stream id.
fn h2_frame(kind: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
    let length = payload.len();
    let mut out = vec![
        (length >> 16) as u8,
        (length >> 8) as u8,
        length as u8,
        kind,
        flags,
    ];
    out.extend_from_slice(&(stream_id & 0x7f_ff_ff_ff).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// HPACK "literal header field without indexing": the 0x10 pattern, then two length-prefixed
/// strings. Every name and value below is ASCII and shorter than 128 bytes, so the 7-bit length
/// prefix never needs a continuation byte — and no dynamic table is touched, so each block is
/// decodable on its own.
fn h2_field(name: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x10, name.len() as u8];
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
    out
}

fn h2_head(fields: &[(&str, &str)]) -> Vec<u8> {
    fields
        .iter()
        .flat_map(|(name, value)| h2_field(name, value))
        .collect()
}

/// Open a cleartext HTTP/2 connection and write `frames` onto it: the client preface, an empty
/// SETTINGS, then whatever the caller built.
async fn handframed(addr: SocketAddr, frames: &[u8]) -> TcpStream {
    let mut tcp = TcpStream::connect(addr).await.expect("connect");
    let mut out = Vec::from(&b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"[..]);
    out.extend_from_slice(&h2_frame(4, 0, 0, &[]));
    out.extend_from_slice(frames);
    tcp.write_all(&out).await.expect("write frames");
    tcp
}

/// Read frames until the server goes silent for two seconds.
async fn read_frames(tcp: &mut TcpStream) -> Vec<(u8, u32, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut filled = [0u8; 16 * 1024];
    let mut frames = Vec::new();
    loop {
        let read = match tokio::time::timeout(Duration::from_secs(2), tcp.read(&mut filled)).await {
            Err(_) | Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&filled[..read]);
        while buf.len() >= 9 {
            let length = (buf[0] as usize) << 16 | (buf[1] as usize) << 8 | buf[2] as usize;
            if buf.len() < 9 + length {
                break;
            }
            let kind = buf[3];
            let id = u32::from_be_bytes([buf[5] & 0x7f, buf[6], buf[7], buf[8]]);
            frames.push((kind, id, buf[9..9 + length].to_vec()));
            buf.drain(..9 + length);
        }
    }
    frames
}

/// Count what each opened stream was given back: an answer, or a reset.
fn tally(frames: &[(u8, u32, Vec<u8>)]) -> (usize, Vec<(u32, u32)>) {
    let answered = frames
        .iter()
        .filter(|(kind, _, _)| *kind == 1)
        .map(|(_, id, _)| *id)
        .collect::<Vec<_>>();
    let reset = frames
        .iter()
        .filter(|(kind, _, payload)| *kind == 3 && payload.len() == 4)
        .map(|(_, id, payload)| {
            (
                *id,
                u32::from_be_bytes(payload[..4].try_into().expect("4 bytes")),
            )
        })
        .collect::<Vec<_>>();
    (answered.len(), reset)
}

/// A SETTINGS parameter as it was advertised on the wire: the connection-setting pair (0x3,
/// value) read out of the server's SETTINGS frames, not out of the config object the app was
/// built from. Padding is ignored per RFC 9113 §6.5.2, so only full 6-byte pairs are read.
fn advertised_setting(frames: &[(u8, u32, Vec<u8>)], setting: u16) -> Option<u32> {
    frames
        .iter()
        .filter(|(kind, stream_id, _)| *kind == 4 && *stream_id == 0)
        .find_map(|(_, _, payload)| {
            payload
                .chunks(6)
                .filter(|pair| pair.len() == 6)
                .find_map(|pair| {
                    (u16::from_be_bytes([pair[0], pair[1]]) == setting)
                        .then(|| u32::from_be_bytes([pair[2], pair[3], pair[4], pair[5]]))
                })
        })
}

/// Open `cap + 5` streams at a server configured with a concurrency cap of `cap`, and assert what
/// comes back. Shared by the two tests below because the wire behaviour is the same either way;
/// only the number and how it was set differ.
async fn streams_over_a_cap_are_reset(cap: usize, app: AetherApp) {
    let server = TestServer::start(app.enable_h2c()).await;

    // `cap + 5` GETs opened in one write, before any of them can finish. A conformant client
    // would queue the overflow behind its own `poll_ready` and never let the server see it, so
    // this is the only way to ask what the advertisement is actually backed by.
    let mut frames = Vec::new();
    let opened = cap + 5;
    for index in 0..opened {
        let stream_id = 1 + 2 * index as u32;
        frames.extend(h2_frame(
            1,
            0x04 | 0x01, // END_HEADERS | END_STREAM
            stream_id,
            &h2_head(&[
                (":method", "GET"),
                (":scheme", "http"),
                (":authority", "aether-h2c.test"),
                (":path", "/fast"),
            ]),
        ));
    }
    let mut tcp = handframed(server.addr, &frames).await;
    let got = read_frames(&mut tcp).await;

    // The cap is only a bound if the connection says so: SETTINGS_MAX_CONCURRENT_STREAMS (0x3)
    // carries this number, so a client can be told the same thing the server then enforces.
    assert_eq!(
        advertised_setting(&got, 0x3),
        Some(cap as u32),
        "the server enforced a cap of {cap} that it did not advertise"
    );

    let (answered, reset) = tally(&got);

    assert!(
        answered <= cap,
        "{answered} streams were answered on a connection that advertises a cap of {cap}"
    );
    assert!(
        !reset.is_empty(),
        "the overflow was neither answered nor reset: {answered} answered of {opened} opened"
    );
    // REFUSED_STREAM (7) is the code that says "this stream, not this connection"; anything else
    // would mean the server treated its own advertisement as a protocol fault.
    for (stream_id, code) in &reset {
        assert_eq!(
            *code, 7,
            "stream {stream_id} was reset with error code {code}, not REFUSED_STREAM"
        );
    }
    assert_eq!(
        answered + reset.len(),
        opened,
        "an over-cap stream was neither answered nor refused — it was dropped without a word"
    );
    assert!(
        !got.iter().any(|(kind, _, _)| *kind == 7),
        "refusing the overflow must not send GOAWAY: the cap belongs to streams, not connections"
    );

    // And the connection is still a connection: a stream opened after the refusal is served.
    let follow_up = h2_frame(
        1,
        0x04 | 0x01,
        1 + 2 * opened as u32,
        &h2_head(&[
            (":method", "GET"),
            (":scheme", "http"),
            (":authority", "aether-h2c.test"),
            (":path", "/fast"),
        ]),
    );
    tcp.write_all(&follow_up).await.expect("follow-up stream");
    let after = read_frames(&mut tcp).await;
    assert!(
        after.iter().any(|(kind, _, _)| *kind == 1),
        "the connection stopped serving after refusing the overflow"
    );

    // End the connection from this side before stopping the server. The refused streams are
    // closed by the RST the server sent, but the *connection* is only driven to its end by
    // something: a client that keeps an idle socket open after GOAWAY makes the drain run out its
    // 30s bound first. That is the framework's bound holding, not a hang, and it is the listener's
    // behaviour under a stuck peer rather than anything this test is asserting — so the peer does
    // what a finished client does.
    tcp.write_all(&h2_frame(7, 0, 0, &0u32.to_be_bytes()))
        .await
        .expect("GOAWAY");
    drop(tcp);

    server.stop().await;
}

/// What the default produces: the cap `ServerLimits` carries, advertised and enforced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streams_over_the_advertised_concurrency_cap_are_reset_not_silently_dropped() {
    use aether_http::limits::ServerLimits;
    let cap = ServerLimits::default().max_concurrent_streams as usize;
    streams_over_a_cap_are_reset(cap, build_app(None)).await;
}

/// The same wire behaviour reached through the public configuration object. Without this, the
/// knob `AetherConfig` now carries would be an unexercised field: the test above pins a default,
/// and a default can be wired by luck. A cap of 4 is also the only way to see the refusal on a
/// handful of streams rather than a hundred.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrency_cap_set_through_the_config_is_advertised_and_enforced() {
    let config = AetherConfig {
        max_concurrent_streams: 4,
        ..AetherConfig::default()
    };
    streams_over_a_cap_are_reset(4, build_app(Some(config))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_header_list_over_the_field_line_budget_is_431() {
    use aether_http::limits::ServerLimits;
    let lines = ServerLimits::default().max_headers_count;
    let server = TestServer::start(build_app(None).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // A list small enough to pass the byte bound `h2` enforces from the advertised
    // MAX_HEADER_LIST_SIZE, and long enough to cross the per-line bound this frontend counts
    // itself. Short values keep it that way: the two limits have to be shown to be different.
    let many: Vec<(String, String)> = (0..lines + 20)
        .map(|i| (format!("x-pad-{i}"), "1".to_string()))
        .collect();
    let pairs: Vec<(&str, &str)> = many
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();

    let response = client.request("GET", "/fast", None, &pairs).await;
    assert_eq!(
        response.status, 431,
        "{lines} field lines must be refused on the same bound HTTP/1 uses"
    );

    // The refusal is the stream's, and the counting is reset with it.
    let after = client.request("GET", "/fast", None, &[]).await;
    assert_eq!((after.status, after.body.as_str()), (200, "fast"));

    client.close().await;
    server.stop().await;
}

/// The field-line budget moved through `AetherConfig` must be the number the frontend counts,
/// including at its boundary: `budget` lines still served, `budget + 1` answered 431. Pinning
/// the default alone cannot tell a wired field from a coincidental agreement between two
/// separately-written constants.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_field_line_budget_set_through_the_config_is_the_number_it_counts() {
    const BUDGET: usize = 8;
    let config = AetherConfig {
        max_headers_count: BUDGET,
        ..AetherConfig::default()
    };
    let server = TestServer::start(build_app(Some(config)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let many: Vec<(String, String)> = (0..BUDGET + 1)
        .map(|i| (format!("x-pad-{i}"), "1".to_string()))
        .collect();

    // One line over the configured budget: refused. One line under it would have passed the
    // default of a hundred, so this response says the configuration reached the counter.
    let over: Vec<(&str, &str)> = many
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let response = client.request("GET", "/fast", None, &over).await;
    assert_eq!(
        response.status, 431,
        "the configured budget of {BUDGET} lines was not the bound used"
    );

    // Exactly the budget: served. `max_headers_count` means "this many lines are allowed", and a
    // frontend that refused at the boundary would be enforcing budget-minus-one.
    let at: Vec<(&str, &str)> = many[..BUDGET]
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let response = client.request("GET", "/fast", None, &at).await;
    assert_eq!(
        (response.status, response.body.as_str()),
        (200, "fast"),
        "a request that only fills the configured budget was refused"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_token_this_framework_cannot_name_is_answered_400() {
    let server = TestServer::start(build_app(None).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // `h2` hands the method through as `http::Method`, so a token the framework cannot name still
    // reaches this frontend. HTTP/1 rejects the same token on the request line, and the router
    // cannot hold a route for a method the framework cannot represent — so the answer is 400 on
    // that stream, in HTTP/2 framing, not a reset and not a hang.
    let response = client.request("WIDGET", "/fast", None, &[]).await;
    assert_eq!(
        response.status, 400,
        "an unusable method token must be answered, body: {}",
        response.body
    );

    let neighbour = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (neighbour.status, neighbour.body.as_str()),
        (200, "fast"),
        "the connection did not survive the request it refused"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_handler_costs_one_stream_and_not_the_connection() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(tls_app(&pki)).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // The handler runs on its own task, and its `JoinError` becomes a 500. Over HTTP/1 a panic
    // could only ever cost a request; here it must not cost the socket, which is what the other
    // streams on it are living on.
    let panicked = client.request("GET", "/boom", None, &[]).await;
    assert_eq!(
        panicked.status, 500,
        "a panicking handler must be answered as a failure, not left unanswered"
    );

    let neighbour = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (neighbour.status, neighbour.body.as_str()),
        (200, "fast"),
        "the connection did not survive the panic of one stream"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_that_vanishes_mid_body_leaves_no_registration_behind() {
    let mut app = build_app(None);
    app = app.with_scheduling_resources(
        SchedulingConfig::default(),
        SchedulingResourceConfig::default(),
    );
    let layer = app.scheduling().expect("scheduling must be enabled");
    let server = TestServer::start(app.enable_h2c()).await;
    assert_eq!(layer.resources().registered_memory_bytes, 0, "no baseline");

    // A POST whose body is never ended: one HEADERS block, four DATA bytes, no END_STREAM. Then
    // the peer disappears at the transport instead of finishing the protocol — no RST_STREAM, no
    // END_STREAM, just a closed socket.
    let mut frames = Vec::new();
    frames.extend(h2_frame(
        1,
        0x04, // END_HEADERS only
        1,
        &h2_head(&[
            (":method", "POST"),
            (":scheme", "http"),
            (":authority", "aether-h2c.test"),
            (":path", "/echo"),
            ("content-type", "application/octet-stream"),
        ]),
    ));
    frames.extend(h2_frame(0, 0, 1, b"half"));
    let tcp = handframed(server.addr, &frames).await;
    // Flushed and closed: the server sees the bytes and then a FIN where an END_STREAM should be.
    drop(tcp);

    // Without the transport fault reaching `accept()`, this registration would be held until
    // `request_timeout` (30s) — and the stream task's buffer with it.
    let settled = wait_for_registration(&layer, 0).await;
    assert_eq!(
        settled, 0,
        "a connection whose peer vanished mid-body kept its registration"
    );

    // The port still serves what it always served.
    let mut next = H2Client::connect_plain(server.addr).await;
    let served = next
        .request("POST", "/echo", Some(b"whole".to_vec()), &[])
        .await;
    assert_eq!(served.status, 200, "body: {}", served.body);
    next.close().await;

    server.stop().await;
    assert_eq!(
        layer.resources().registered_memory_bytes,
        0,
        "shutdown left a registration behind"
    );
}

// ---------------------------------------------------------------------------------------------
// Request-scoped extensions over HTTP/2.
//
// The type-map itself is unit-tested in `aether/src/extensions.rs` and its HTTP/1.1 wire path in
// `tests/extensions_e2e.rs`. What is only decidable here is the shape HTTP/2 gives the question:
// one connection carries many requests *at the same time*, so a value that leaked into shared
// state would show up on a neighbouring stream rather than on the next request down a keep-alive
// line. That is the property these three tests hold the h2 frontend to.
// ---------------------------------------------------------------------------------------------

/// A per-request value deliberately **not** `Clone`, so a frontend that cloned the context on the
/// way to the handler could not pass these tests.
struct Principal(String);

/// The application the tests below run. It stamps from a header or, since `begin()` opens a
/// stream without custom headers, from `?m=`, and then sits inside the middleware chain for
/// 100 ms holding that value — long enough for several streams to overlap there.
fn ext_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
        let marker = req
            .header("x-marker")
            .map(str::to_string)
            .or_else(|| marker_from_query(req.query_str.as_deref()));
        if let Some(marker) = marker {
            req.insert_extension(Principal(format!("mw:{marker}")));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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

fn marker_from_query(query: Option<&str>) -> Option<String> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("m="))
        .map(str::to_string)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_extension_set_by_middleware_reaches_a_handler_over_h2() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(ext_app().tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let stamped = client
        .request("GET", "/who", None, &[("x-marker", "alpha")])
        .await;
    assert_eq!(
        (stamped.status, stamped.body.as_str()),
        (200, "mw:alpha"),
        "a value inserted by middleware must survive the h2 request→handler handover"
    );

    // The negative half on the same connection: the middleware ran and chose not to stamp, so an
    // empty map — not a stale one — must reach the handler.
    let unstamped = client.request("GET", "/who", None, &[]).await;
    assert_eq!((unstamped.status, unstamped.body.as_str()), (200, "none"));

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extensions_are_per_stream_never_per_connection() {
    let server = TestServer::start(ext_app().enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // Three streams in flight at once, all inside the middleware's 100 ms hold, plus one that was
    // never stamped. If the map were attached to the connection, the last writer would win and
    // the first two would report `mw:three`.
    let started = Instant::now();
    let one = client.begin("GET", "/who?m=one");
    let two = client.begin("GET", "/who?m=two");
    let three = client.begin("GET", "/who?m=three");
    let none = client.begin("GET", "/who");

    assert_eq!(one.await.expect("stream 1").body, "mw:one");
    assert_eq!(two.await.expect("stream 2").body, "mw:two");
    assert_eq!(three.await.expect("stream 3").body, "mw:three");
    assert_eq!(
        none.await.expect("unmarked stream").body,
        "none",
        "a stream that shares a connection with stamped streams must still see an empty map"
    );
    // The isolation claim is only worth what the concurrency behind it is real: three handlers
    // each holding 100 ms inside the chain answer in ~100 ms together, or in ≥300 ms one after
    // another. Sequential streams would make the four distinct bodies above trivially correct
    // without proving anything about per-request state, so this test fails if they did not overlap.
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(280),
        "the four streams did not overlap ({elapsed:?} for three 100 ms holds): the isolation \
         assertion below would have passed without concurrency"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_request_gives_the_same_extension_over_http1_and_h2c() {
    let server = TestServer::start(ext_app().enable_h2c()).await;

    // Both frontends build the same `RequestContext` in the same serving core, and extensions
    // live on that context. So the transport must not be able to change the answer: any
    // difference would mean one frontend drops or replaces the map on the way in.
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(b"GET /who?m=parity HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .expect("write request");
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_string(&mut text))
        .await
        .expect("HTTP/1.1 answered")
        .expect("read response");
    assert!(
        text.ends_with("mw:parity"),
        "HTTP/1.1 on an h2c port got: {text}"
    );

    let mut client = H2Client::connect_plain(server.addr).await;
    let over_h2 = client.request("GET", "/who?m=parity", None, &[]).await;
    assert_eq!((over_h2.status, over_h2.body.as_str()), (200, "mw:parity"));

    client.close().await;
    server.stop().await;
}

/// The HTTP/1 read side learned to charge the block the allocator handed out rather than the
/// length written into it (`buffered_body_is_priced_by_the_block_the_allocator_handed_out`).
/// The h2 body buffer was still priced per frame by `chunk.len()`, which hides the same
/// overshoot: `Vec` growth is `max(2 * capacity, needed)`, so 1,441,792 B of body sits in a
/// 2 MiB block and only the block is RSS. `read_body` now re-prices with `buf.capacity()`.
///
/// The stream is ended on the last frame so the request is served normally — an abandoned
/// half-open stream makes the drain wait for `request_timeout`, which costs the suite a minute.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_h2_body_is_priced_by_the_block_the_allocator_handed_out() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    app = app.with_scheduling_resources(
        SchedulingConfig::default(),
        SchedulingResourceConfig::default(),
    );
    let layer = app.scheduling().expect("scheduling must be enabled");
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let mut send = client.send.clone();
    let request = http::Request::builder()
        .method("POST")
        .uri(client.uri("/echo"))
        .body(())
        .expect("request");
    let (response, mut send_stream) = send.send_request(request, false).expect("stream accepted");

    let sampler_layer = layer.clone();
    let sampler = tokio::spawn(async move {
        let mut peak = 0u64;
        for _ in 0..3000 {
            peak = peak.max(sampler_layer.resources().registered_memory_bytes);
            if peak >= 2_000_000 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        peak
    });
    // 176 x 8 KiB = 1,441,792 B, below the 2 MiB `max_body_bytes` so the parser waits for the
    // body instead of refusing it. The server releases receive capacity as it reads, which is
    // what lets a body larger than one flow-control window arrive at all.
    let chunk = Bytes::from_static(&[7u8; 8 * 1024]);
    for index in 0..176 {
        send_stream
            .send_data(chunk.clone(), index == 175)
            .expect("DATA frame accepted");
    }
    let peak = sampler.await.expect("sampler task");
    assert!(
        peak >= 2_000_000,
        "peak {peak} B for 1,441,792 B written: the reservation above the written length is \
         unpriced (measured under the old per-frame length price: 1,449,982 B)",
    );

    let served = read_body(response.await.expect("response head")).await;
    assert_eq!(served.status, 200, "body: {}", served.body);
    assert_eq!(
        served.body.len(),
        "method=POST|path=/echo|query=|x-marker=|body=".len() + 1_441_792,
        "the echoed body was not delivered whole"
    );

    drop(send_stream);
    drop(send);
    client.close().await;
    server.stop().await;
    assert_eq!(
        layer.resources().registered_memory_bytes,
        0,
        "a served stream kept its body charge"
    );
}

// ---------------------------------------------------------------------------------------------
// Streamed response bodies on HTTP/2: DATA frames under both framings, and a `HEAD` that must
// never ask the source for a byte it is not going to send.
// ---------------------------------------------------------------------------------------------

/// A source that counts how many times it was asked for a chunk.
struct StreamedPieces {
    pieces: std::vec::IntoIter<Bytes>,
    polls: Arc<AtomicUsize>,
}

impl BodySource for StreamedPieces {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
        Box::pin(async move {
            self.polls.fetch_add(1, Ordering::SeqCst);
            self.pieces.next().map(Ok)
        })
    }
}

fn streamed_app(polls: Arc<AtomicUsize>) -> AetherApp {
    let mut app = build_app(None);
    app = app.get_async("/streamed", |_req: RequestContext| async {
        Response::stream(
            BodyStream::from_chunks([
                Bytes::from_static(b"hello"),
                Bytes::from_static(b" "),
                Bytes::from_static(b"world"),
            ]),
            "text/plain; charset=utf-8",
        )
    });
    app = app.get_async("/streamed-sized", |_req: RequestContext| async {
        Response::stream_with_length(
            BodyStream::from_chunks([Bytes::from_static(b"hello"), Bytes::from_static(b"!")]),
            "text/plain; charset=utf-8",
            6,
        )
    });
    app = app.get_async("/counted", move |_req: RequestContext| {
        let polls = Arc::clone(&polls);
        async move {
            Response::stream_with_length(
                StreamedPieces {
                    pieces: vec![Bytes::from_static(b"abcdef")].into_iter(),
                    polls,
                },
                "text/plain; charset=utf-8",
                6,
            )
        }
    });
    app
}

/// An undeclared stream has no framing field to fall back on: no `content-length` (which would
/// have to name a length nobody knows) and no `transfer-encoding`, which HTTP/2 does not define.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_body_arrives_whole_with_no_framing_field() {
    let pki = pki("aether-h2.test");
    let polls = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(streamed_app(polls).tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client.request("GET", "/streamed", None, &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body, "hello world",
        "the chunks must arrive in order"
    );
    assert!(
        !response.has("content-length"),
        "an undeclared stream must not promise a length: {:?}",
        response.header("content-length")
    );
    assert!(
        !response.has("transfer-encoding"),
        "HTTP/2 has no transfer coding: {:?}",
        response.header("transfer-encoding")
    );

    client.close().await;
    server.stop().await;
}

/// A declared length is a promise, and HTTP/2 carries it in `content-length` like HTTP/1.1 does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_stream_length_is_the_length_the_peer_is_told() {
    let pki = pki("aether-h2.test");
    let polls = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(streamed_app(polls).tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let response = client.request("GET", "/streamed-sized", None, &[]).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body, "hello!");
    assert_eq!(response.header("content-length"), Some("6"));

    client.close().await;
    server.stop().await;
}

/// `HEAD` earns the GET's metadata and no payload, so the source is dropped unpolled — the same
/// rule the HTTP/1 writer is held to, proven here against a source that counts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_head_over_h2_never_polls_the_stream() {
    let pki = pki("aether-h2.test");
    let polls = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(streamed_app(Arc::clone(&polls)).tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let head = client.request("HEAD", "/counted", None, &[]).await;

    assert_eq!(head.status, 200);
    assert_eq!(head.body, "", "HEAD must not carry a payload");
    assert_eq!(head.header("content-length"), Some("6"));
    assert_eq!(
        polls.load(Ordering::SeqCst),
        0,
        "serialising a HEAD polled the stream"
    );

    let get = client.request("GET", "/counted", None, &[]).await;
    assert_eq!(get.body, "abcdef");
    assert!(
        polls.load(Ordering::SeqCst) > 0,
        "a GET must actually pull the source"
    );

    client.close().await;
    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// The declared length of a *request* body over HTTP/2 (RFC 9113 §8.1.1 and §8.1.2.6).
//
// Every shape below was measured on the wire before it was written, hand-framed so that no client
// library could be credited for the answer: `content-length` 3 with 10 bytes of DATA, `10` with 7
// split across two frames, `abc`, ` 5 `, the pair `5` and `7`, twenty-three digits, a head that
// promises 5 and ends where it is, and `content-length` beside `transfer-encoding`. All eight ended
// the same way: that stream reset with `PROTOCOL_ERROR`, no response head on it, no GOAWAY, and the
// next stream served on the same connection. `content-length` 5 twice with 5 bytes of DATA was
// served, because two equal values are one value.
//
// The enforcement sits in `h2`'s receive path (`proto/streams/recv.rs`: the value must be digits
// with no padding, repeats must agree, every DATA frame decrements the promise and END_STREAM must
// find it at zero). What this frontend owns is the ceiling answer that goes out *before* dispatch
// and the body it hands the handler. These tests pin both halves together, so a dependency that
// stops enforcing the first half fails here instead of quietly becoming this server's behaviour —
// the same reason the inbound connection-specific fields are pinned from bytes rather than trusted.
// ---------------------------------------------------------------------------------------------

/// Hand-frame one POST `/echo` stream: `fields` as its head, then one DATA frame per entry of
/// `data`, the last ending the stream. With no `data` at all the head itself ends the stream, which
/// is how a request promises bytes and never gets a chance to send them.
///
/// Then assert the only answer RFC 9113 §8.1.1 allows for a malformed request: a stream error of
/// type PROTOCOL_ERROR, nothing answered, no GOAWAY, and a connection that still serves.
async fn lying_length_is_reset(fields: &[(&str, &str)], data: &[&[u8]]) {
    let server = TestServer::start(build_app(None).enable_h2c()).await;

    let mut head = vec![
        (":method", "POST"),
        (":scheme", "http"),
        (":authority", "aether-h2c.test"),
        (":path", "/echo"),
    ];
    head.extend_from_slice(fields);
    let head_flags = if data.is_empty() { 0x04 | 0x01 } else { 0x04 };
    let mut frames = h2_frame(1, head_flags, 1, &h2_head(&head));
    for (index, chunk) in data.iter().enumerate() {
        let last = index + 1 == data.len();
        frames.extend(h2_frame(0, if last { 0x01 } else { 0 }, 1, chunk));
    }

    let mut tcp = handframed(server.addr, &frames).await;
    let got = read_frames(&mut tcp).await;
    let (answered, reset) = tally(&got);
    assert_eq!(
        answered, 0,
        "a request whose declared length does not describe its DATA must not be answered"
    );
    assert_eq!(
        reset,
        vec![(1, 1)],
        "RFC 9113 §8.1.1 makes a malformed request a stream error of PROTOCOL_ERROR (1) on that stream"
    );
    assert!(
        !got.iter().any(|(kind, _, _)| *kind == 7),
        "the lie is one stream's; a GOAWAY would end the connection over it"
    );

    // The refusal costs the stream that produced it and nothing else.
    let follow_up = h2_frame(
        1,
        0x04 | 0x01,
        3,
        &h2_head(&[
            (":method", "GET"),
            (":scheme", "http"),
            (":authority", "aether-h2c.test"),
            (":path", "/fast"),
        ]),
    );
    tcp.write_all(&follow_up).await.expect("follow-up stream");
    let after = read_frames(&mut tcp).await;
    assert!(
        after.iter().any(|(kind, id, _)| *kind == 1 && *id == 3),
        "the connection stopped serving after refusing the lie"
    );

    tcp.write_all(&h2_frame(7, 0, 0, &0u32.to_be_bytes()))
        .await
        .expect("GOAWAY");
    drop(tcp);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_longer_than_its_declared_length_is_reset_over_h2() {
    // The shape HTTP/1 cannot even express as a desync, because there the declared length wins and
    // the surplus becomes the next message. On HTTP/2 the DATA *is* the framing, so a promise that
    // the frames overgrow is not a boundary dispute to resolve — it is a malformed request.
    lying_length_is_reset(&[("content-length", "3")], &[b"0123456789"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_shorter_than_its_declared_length_is_reset_over_h2() {
    // Split across two DATA frames on purpose: the promise is held against the *sum*, so a body
    // that arrives in pieces is not one frame's business.
    lying_length_is_reset(&[("content-length", "10")], &[b"abcdef", b"abc"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_numeric_content_length_is_reset_over_h2() {
    // `declared_length` in the frontend would read this as "no ceiling to check" and hand the head
    // on. The rule is stricter than that helper, and it is enforced before dispatch.
    lying_length_is_reset(&[("content-length", "abc")], &[b"abcde"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_padded_content_length_is_reset_over_h2() {
    // HTTP/1 trims optional whitespace around this value and accepts the same request; here the
    // field value is the digits and nothing else. The divergence is pinned from both ends, which is
    // what makes it a rule rather than an accident of two parsers.
    lying_length_is_reset(&[("content-length", " 5 ")], &[b"abcde"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conflicting_repeated_content_lengths_are_reset_over_h2() {
    // The pair the frontend's first-value read would silently pick a side of: 5 and 7 do not agree,
    // so there is no body length to enforce the ceiling against and no request to dispatch.
    lying_length_is_reset(
        &[("content-length", "5"), ("content-length", "7")],
        &[b"abcde"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unparseably_large_content_length_is_reset_over_h2() {
    // Twenty-three digits: past `u64`, so a parse that wrapped would name a small body. HTTP/1 has
    // its own test for this (`test_content_length_overflow_is_rejected_not_wrapped`).
    lying_length_is_reset(
        &[("content-length", "99999999999999999999999")],
        &[b"abcde"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_head_that_promises_bytes_and_ends_where_it_is_reset_over_h2() {
    // END_STREAM on the HEADERS frame says "no DATA will follow" while the head says "five bytes
    // will follow". One of the two has to be wrong, and the request is the thing being refused.
    lying_length_is_reset(&[("content-length", "5")], &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_content_length_beside_transfer_encoding_is_reset_over_h2() {
    // HTTP/1 refuses the same pair as a smuggling risk (CL.TE). HTTP/2 has no transfer coding at
    // all, so the field is not an alternative framing to reconcile — it is prohibited on the stream.
    lying_length_is_reset(
        &[("content-length", "5"), ("transfer-encoding", "chunked")],
        &[b"abcde"],
    )
    .await;
}

/// A route that reports exactly what the frontend handed a handler: every `content-length` line,
/// in the order they arrived, and the body it was given.
fn length_report_app() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.post("/cl", |req: RequestContext| {
        Response::text_dynamic(format!(
            "values={:?} received={} body=[{}]",
            req.header_values("content-length"),
            req.body.len(),
            String::from_utf8_lossy(&req.body),
        ))
    });
    app
}

/// The one repeated form that is not a conflict: two lines that agree are one value, so the
/// request is well-formed and the handler keeps both lines the peer sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matching_repeated_content_lengths_are_served_over_h2() {
    let server = TestServer::start(length_report_app().enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let response = client
        .request(
            "POST",
            "/cl",
            Some(b"abcde".to_vec()),
            &[("content-length", "5"), ("content-length", "5")],
        )
        .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body, "values=[\"5\", \"5\"] received=5 body=[abcde]",
        "agreeing lines are one length, and the handler must still see the lines it was sent"
    );

    client.close().await;
    server.stop().await;
}

/// HTTP/2 frames its messages with DATA, not with a length field, so a request is complete without
/// `content-length`. Pinned because a future "require the field" change would look reasonable to a
/// reader of the HTTP/1 rules and would break every conformant HTTP/2 client that omits it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_h2_request_needs_no_declared_length_to_be_served() {
    let server = TestServer::start(length_report_app().enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let response = client
        .request("POST", "/cl", Some(b"0123456789".to_vec()), &[])
        .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body, "values=[] received=10 body=[0123456789]",
        "the DATA frames are the body; nothing may be dropped for want of a declared length"
    );

    client.close().await;
    server.stop().await;
}

/// The ceiling answer this frontend owns, seen with more than one `content-length` line on the
/// head. The h2 client's own path is not enough to prove this: the frontend's ceiling read takes
/// the *first* line, which is a safe promise only because a peer that sends two disagreeing ones
/// never gets this far (`conflicting_repeated_content_lengths_are_reset_over_h2` pins that). A
/// ceiling check that read no line at all would dispatch this handler and pay for a body its route
/// had already refused, so the handler's counter is the assertion here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_declared_length_over_the_streaming_ceiling_is_413_before_dispatch() {
    let dispatched = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&dispatched);
    let mut app = AetherApp::with_config(AetherConfig {
        max_stream_body_bytes: 4,
        ..AetherConfig::default()
    });
    app = app.route_stream(Method::Post, "/up", move |req: RequestContext| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            format!("{}", req.body.len())
        }
    });
    app = app.get("/fast", |_| Response::text("fast"));

    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let response = client
        .request(
            "POST",
            "/up",
            Some(b"abcde".to_vec()),
            &[("content-length", "5"), ("content-length", "5")],
        )
        .await;

    assert_eq!(
        response.status, 413,
        "a declared length the streaming ceiling forbids is refused before dispatch: {}",
        response.body
    );
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        0,
        "the handler ran for a body its route had already refused by length"
    );

    let neighbour = client.request("GET", "/fast", None, &[]).await;
    assert_eq!(
        (neighbour.status, neighbour.body.as_str()),
        (200, "fast"),
        "the refusal belongs to the stream, not to the connection"
    );

    client.close().await;
    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// Signed cookies over HTTP/2.
//
// `RequestContext::signed_cookie` reads the request's decoded header store, and on this frontend
// that store is built by `header_store` in `framework/aether/src/http2.rs` — a second builder, not
// the HTTP/1 loop's `header_store`. So these tests are not `signed_cookies.rs` repeated on a
// different socket: they hold that builder to the two properties the accessor depends on, namely
// that a `cookie` field reaches the handler with its value untouched (not decoded, not case- or
// separator-normalised) and that a name carried twice is answered `Ambiguous` whichever shape it
// arrived in. Measured before writing: two `cookie` fields arrive as two lines and one field
// carrying `a=1; b=2` arrives as one line. What the second property actually pins is the
// *separator*: a store that joined repeated fields with a comma would still answer 409 for a name
// sent twice on one line and 403 where the accessor must answer 409, because `session=a,session=b`
// is one pair whose value is `b`'s bytes with `a`'s prefix. Joining them with `"; "` instead is
// indistinguishable through this accessor — it splits each line on `;` itself — so no test here
// claims to pin separate lines, and only the comma shape is refuted below.
// ---------------------------------------------------------------------------------------------

const COOKIE_SECRET: &[u8] = b"h2-wire-cookie-secret-0123456789ab";
const FOREIGN_SECRET: &[u8] = b"the-attackers-own-secret-0123456789";

fn h2_signer() -> Arc<CookieSigner> {
    Arc::new(CookieSigner::new(COOKIE_SECRET))
}

/// `GET /login` issues one signed `session` cookie and `GET /whoami` classifies the one it
/// receives. The four statuses are the ones `framework/tests/tests/signed_cookies.rs` answers
/// with, so a difference between the two files can only come from the transport.
///
/// Registration is a function rather than an app builder because `use_middleware` wraps only the
/// routes registered **after** it, so the held variant below has to install its middleware before
/// calling this.
fn add_signed_cookie_routes(app: AetherApp, signer: &Arc<CookieSigner>) -> AetherApp {
    let set_side = Arc::clone(signer);
    let app = app.get("/login", move |_: RequestContext| {
        let value = set_side.value("session", "user-42").expect("a token name");
        Response::ok().set_cookie("session", &value, true, true, "Lax", Some(3600))
    });
    let read_side = Arc::clone(signer);
    app.get("/whoami", move |req: RequestContext| {
        match req.signed_cookie(&read_side, "session") {
            SignedCookie::Present(payload) => Response::text_dynamic(payload),
            SignedCookie::Absent => Response::status(401, "Unauthorized"),
            SignedCookie::Invalid => Response::status(403, "Forbidden"),
            SignedCookie::Ambiguous => Response::status(409, "Conflict"),
        }
    })
}

fn signed_cookie_app(signer: Arc<CookieSigner>) -> AetherApp {
    add_signed_cookie_routes(AetherApp::new(), &signer)
}

/// The same two routes with every request held inside the middleware chain for 100 ms, so the
/// streams of `overlapping_h2_streams_answer_for_their_own_cookie` genuinely overlap instead of
/// queueing one after another down a single keep-alive line.
///
/// The middleware is registered first and deliberately: registered after the routes it wraps
/// nothing and never runs, which costs this test its premise without costing it a single failure.
/// The elapsed window asserted below is what catches that shape.
fn held_signed_cookie_app(signer: Arc<CookieSigner>) -> AetherApp {
    let app = AetherApp::new();
    let app = app.use_middleware(|req: RequestContext, next: Next| async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        next(req).await
    });
    add_signed_cookie_routes(app, &signer)
}

/// The `session=<value>` pair out of the `set-cookie` field a response carried, attributes dropped.
fn issued_pair(response: &H2Response) -> String {
    response
        .header("set-cookie")
        .expect("a Set-Cookie field on the response")
        .split(';')
        .next()
        .expect("the name=value pair")
        .to_owned()
}

/// The signed value for `session`, minted without a socket: these tests need the bytes to plant,
/// not a response to read them out of.
fn issued_value(signer: &CookieSigner) -> String {
    signer.value("session", "user-42").expect("a token name")
}

/// The number of `set-cookie` fields a response carried.
///
/// `issued_pair` below takes the first, and on HTTP/1 the same choice is pinned by
/// `framework/tests/tests/signed_cookies.rs` refusing two `Set-Cookie` lines outright; this is the
/// h2 half of that pin, so a store that split the issued cookie across fields cannot slip past it.
fn set_cookie_fields(response: &H2Response) -> usize {
    response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .count()
}

/// `begin` with request headers, for the streams that must be in flight at the same time and each
/// carry a cookie of their own. Answers with the response and how long *that stream alone* took,
/// measured from handing its request to draining its body — the per-stream latency is what tells a
/// connection that held all four apart from one that held the first and answered the rest instantly.
fn begin_with_headers(
    client: &mut H2Client,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> tokio::task::JoinHandle<(H2Response, Duration)> {
    let mut send = client.send.clone();
    let mut builder = http::Request::builder()
        .method(method)
        .uri(client.uri(path));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(()).expect("request");
    let sent = Instant::now();
    let (response_future, _send_stream) =
        send.send_request(request, true).expect("stream accepted");
    tokio::spawn(async move {
        let response = read_body(response_future.await.expect("response head")).await;
        (response, sent.elapsed())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_cookie_issued_over_h2c_round_trips_and_is_read_back() {
    let server = TestServer::start(signed_cookie_app(h2_signer()).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let login = client.request("GET", "/login", None, &[]).await;
    assert_eq!(login.status, 200, "body: {}", login.body);
    assert_eq!(
        set_cookie_fields(&login),
        1,
        "the issued cookie reached the h2 wire split across fields"
    );
    let pair = issued_pair(&login);
    assert!(pair.starts_with("session="), "{pair}");
    let value = pair.strip_prefix("session=").expect("a value");
    assert_eq!(value.split('.').count(), 2, "{value}");
    assert!(
        !value.contains('=') && !value.contains('%'),
        "a signed value is base64url of an RFC 6265-safe payload, so nothing on the way out \
         should have had to encode it: {value}"
    );

    // The attributes are the other half of what `/login` set, and they ride the same field.
    let attributes = login
        .header("set-cookie")
        .expect("a Set-Cookie field")
        .split_once(';')
        .expect("attributes after the pair")
        .1
        .to_owned();
    for expected in [
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        "Path=/",
        "Max-Age=3600",
    ] {
        assert!(
            attributes.contains(expected),
            "{attributes} lacks {expected}"
        );
    }

    let whoami = client
        .request("GET", "/whoami", None, &[("cookie", pair.as_str())])
        .await;
    assert_eq!(
        (whoami.status, whoami.body.as_str()),
        (200, "user-42"),
        "the cleartext HTTP/2 frontend must hand the accessor byte-for-byte what it issued"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_planted_or_tampered_signed_cookie_is_refused_over_h2c() {
    let signer = h2_signer();
    let good = issued_value(&signer);
    let planted = CookieSigner::new(FOREIGN_SECRET)
        .value("session", "user-admin")
        .expect("a token name");
    let theme = signer.value("theme", "user-42").expect("a token name");
    let mac = good.split_once('.').expect("two halves").1.to_owned();
    let forged = format!(
        "{}.{}",
        aether::security::jwt::encode_base64url(b"user-43"),
        mac
    );
    // The accessor documents that it does not percent-decode, and the only way to hold this
    // frontend's store to that is to send a signed value whose separator is written `%2E`: as it
    // arrives it carries no `.`, so it is a malformed value — and a store that decoded on the way
    // in would hand `verify` a perfect one and answer 200 to a cookie nobody ever issued.
    let encoded_separator = good.replace('.', "%2E");

    let server = TestServer::start(signed_cookie_app(signer).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    for (cookie, why) in [
        ("session=user-42".to_string(), "no signature at all"),
        (
            format!("session={planted}"),
            "a real MAC under a key this server does not hold",
        ),
        (
            format!("session={theme}"),
            "a value this server signed, offered under another name",
        ),
        (
            format!("session={forged}"),
            "a payload swapped underneath a genuine MAC",
        ),
        (
            format!("session={encoded_separator}"),
            "a percent-decoding store turned this into a well-formed signed value",
        ),
    ] {
        let response = client
            .request("GET", "/whoami", None, &[("cookie", cookie.as_str())])
            .await;
        assert_eq!(
            response.status, 403,
            "{why} was accepted over h2c: {cookie}"
        );
    }

    // Absent is its own decision, and so is a name that is not `session`: the field is there, the
    // cookie the handler asked for is not.
    let anonymous = client.request("GET", "/whoami", None, &[]).await;
    assert_eq!(anonymous.status, 401, "a request with no cookie");
    for name in ["Session", "SESSION", "sessions", "x-session", "sess"] {
        let cookie = format!("{name}={good}");
        let response = client
            .request("GET", "/whoami", None, &[("cookie", cookie.as_str())])
            .await;
        assert_eq!(response.status, 401, "{name} matched `session` over h2c");
    }

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_duplicated_cookie_name_over_h2_is_ambiguous_whichever_shape_it_arrives_in() {
    let signer = h2_signer();
    let good = format!("session={}", issued_value(&signer));
    let planted = format!(
        "session={}",
        CookieSigner::new(FOREIGN_SECRET)
            .value("session", "user-admin")
            .expect("a token name")
    );
    let server = TestServer::start(signed_cookie_app(signer).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    // One field carrying the name twice, and two fields the header store had to keep apart. The
    // second shape only exists because HTTP/2 sends a repeated field as repeated block entries
    // instead of continuation lines, and it is the shape this frontend's own builder is
    // responsible for.
    let joined = format!("{good}; {planted}");
    let one_field = client
        .request("GET", "/whoami", None, &[("cookie", joined.as_str())])
        .await;
    assert_eq!(
        one_field.status, 409,
        "one cookie field carrying the name twice"
    );

    let two_fields = client
        .request(
            "GET",
            "/whoami",
            None,
            &[("cookie", good.as_str()), ("cookie", planted.as_str())],
        )
        .await;
    assert_eq!(
        two_fields.status, 409,
        "two cookie fields with the same name: the store joined them instead of keeping them apart"
    );

    // Identical values duplicate just as loudly: the refusal is about the server having no basis
    // to choose, not about the two values disagreeing.
    let twice_same = client
        .request(
            "GET",
            "/whoami",
            None,
            &[("cookie", good.as_str()), ("cookie", good.as_str())],
        )
        .await;
    assert_eq!(twice_same.status, 409, "the same field sent twice");

    client.close().await;
    server.stop().await;
}

/// The plain accessor over the multiplexed frontend. RFC 9113 §8.2.3 lets a client split `Cookie`
/// across repeated fields, which makes the fields one jar rather than two headers — the same
/// property `signed_cookie` and the CSRF check already rely on. `cookie()` used to read the first
/// line alone, so a name the client sent on the second was reported as never sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cookie_split_across_fields_is_read_by_the_plain_accessor_over_h2() {
    let mut app = AetherApp::new();
    app = app.get("/probe", |req| {
        Response::text_dynamic(format!(
            "second={}|dupe={}|absent={}",
            req.cookie("theme").unwrap_or("<absent>"),
            req.cookie("dupe").unwrap_or("<absent>"),
            req.cookie("missing").is_none(),
        ))
    });
    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let reply = client
        .request(
            "GET",
            "/probe",
            None,
            &[
                ("cookie", "a=1; dupe=first"),
                ("cookie", "theme=dark; dupe=second"),
            ],
        )
        .await;
    assert_eq!(reply.status, 200, "body: {}", reply.body);
    assert_eq!(
        reply.body, "second=dark|dupe=first|absent=true",
        "the second field is the same jar as the first, and the first occurrence still wins"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_h2_streams_answer_for_their_own_cookie() {
    let signer = h2_signer();
    let good = issued_value(&signer);
    let planted = CookieSigner::new(FOREIGN_SECRET)
        .value("session", "user-admin")
        .expect("a token name");
    let good_field = format!("session={good}");
    let planted_field = format!("session={planted}");

    let server = TestServer::start(held_signed_cookie_app(signer).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let started = Instant::now();
    let valid = begin_with_headers(
        &mut client,
        "GET",
        "/whoami",
        &[("cookie", good_field.as_str())],
    );
    let foreign = begin_with_headers(
        &mut client,
        "GET",
        "/whoami",
        &[("cookie", planted_field.as_str())],
    );
    // One stream carries both of them: a store attached to the connection would have made every
    // stream above answer the same thing, and this one would answer it too.
    let both = begin_with_headers(
        &mut client,
        "GET",
        "/whoami",
        &[
            ("cookie", good_field.as_str()),
            ("cookie", planted_field.as_str()),
        ],
    );
    let anonymous = begin_with_headers(&mut client, "GET", "/whoami", &[]);

    let (valid, valid_took) = valid.await.expect("valid stream");
    assert_eq!(
        (valid.status, valid.body.as_str()),
        (200, "user-42"),
        "a stream sharing a connection with refused streams must still read its own cookie"
    );
    let (foreign, foreign_took) = foreign.await.expect("foreign stream");
    assert_eq!(
        foreign.status, 403,
        "the stream that carried a MAC this server cannot verify"
    );
    let (both, both_took) = both.await.expect("duplicated stream");
    assert_eq!(
        both.status, 409,
        "the stream that carried the name twice while its neighbours carried it once"
    );
    let (anonymous, anonymous_took) = anonymous.await.expect("anonymous stream");
    assert_eq!(
        anonymous.status, 401,
        "the stream that carried no cookie at all"
    );

    // The isolation claim is only worth what the concurrency behind it is real, and four distinct
    // answers do not by themselves prove any: a chain that ran once per *connection* would delay
    // the first stream, answer the other three instantly, and still get all four statuses right.
    // So each stream is timed on its own — every one of them has to carry the 100 ms the middleware
    // puts on a request. That floor is also what catches a mis-registered middleware, which wraps
    // nothing and never runs (see `held_signed_cookie_app`), and costs a test like this its premise
    // without costing it a single status.
    for (stream, took) in [
        ("valid", valid_took),
        ("foreign", foreign_took),
        ("duplicated", both_took),
        ("anonymous", anonymous_took),
    ] {
        assert!(
            took >= Duration::from_millis(100),
            "the {stream} stream answered in {took:?}, faster than the 100 ms hold every request \
             is supposed to wait through"
        );
    }

    // Four 100 ms holds one after another cost >= 400 ms; overlapping on one connection cost ~100.
    // The ceiling is what separates the two, and it is deliberately above the four floors rather
    // than tight: on a loaded machine a genuinely-overlapping connection can drift, while the
    // serialised shape it is refuting cannot come in under 400 ms.
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(350),
        "the four streams did not overlap ({elapsed:?} for four 100 ms holds): the per-stream \
         answers above would have passed on a connection that served them one at a time"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_cookie_issued_over_tls_http2_is_read_back_on_the_same_connection() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(signed_cookie_app(h2_signer()).tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let login = client.request("GET", "/login", None, &[]).await;
    assert_eq!(login.status, 200, "body: {}", login.body);
    assert_eq!(
        set_cookie_fields(&login),
        1,
        "the issued cookie reached the h2 wire split across fields"
    );
    let pair = issued_pair(&login);

    let whoami = client
        .request("GET", "/whoami", None, &[("cookie", pair.as_str())])
        .await;
    assert_eq!(
        (whoami.status, whoami.body.as_str()),
        (200, "user-42"),
        "the ALPN-negotiated frontend must classify what its own response issued"
    );

    // The negative half on the same live connection, after a positive answer: with the HPACK
    // context warm and everything, a value this server did not sign is still refused.
    let planted = CookieSigner::new(FOREIGN_SECRET)
        .value("session", "user-admin")
        .expect("a token name");
    let foreign = client
        .request(
            "GET",
            "/whoami",
            None,
            &[("cookie", format!("session={planted}").as_str())],
        )
        .await;
    assert_eq!(
        foreign.status, 403,
        "a foreign MAC accepted over TLS/HTTP/2"
    );

    client.close().await;
    server.stop().await;
}

/// A `/upload` that records every handler entry, so a refusal over HTTP/2 can be shown to have
/// refused before dispatch rather than answered after the route had run.
fn expectation_upload_app(ran: Arc<AtomicUsize>) -> AetherApp {
    let mut app = AetherApp::new();
    app = app.put_async("/upload", move |req: RequestContext| {
        let ran = Arc::clone(&ran);
        async move {
            ran.fetch_add(1, Ordering::SeqCst);
            Response::text_dynamic(format!("handler:{}", req.body.len()))
        }
    });
    app
}

/// RFC 9110 §10.1.1 is message semantics, not connection semantics, so `Expect` means the same
/// thing over HTTP/2: an unsupported expectation is refused, and the refusal comes from the HEADERS
/// frame - no DATA frame is asked for, because the whole point is that the client may not send one.
#[tokio::test]
async fn an_unsupported_expectation_is_refused_over_http2_from_the_head() {
    let ran = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(expectation_upload_app(Arc::clone(&ran)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let paused = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", "foo=bar")],
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert!(
        paused.answered_early,
        "the h2 frontend waited for a body before refusing the expectation"
    );
    assert_eq!(paused.response.status, 417);
    assert!(
        paused.response.body.is_empty(),
        "a refusal arrived with a body: {:?}",
        paused.response.body
    );
    assert!(
        paused.informational.is_empty(),
        "an unsupported expectation was answered with an interim: {:?}",
        paused.informational
    );
    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "a handler ran for a request refused on its expectation"
    );

    client.close().await;
    server.stop().await;
}

/// One head can break two rules at once, and then both frontends must name the same one. HTTP/1
/// refuses on the field-line ceiling before it reads the expectation, so HTTP/2 answers the same
/// head the same way - and refuses it before its body is asked for, since a `100` would tell the
/// client to send bytes for a request that is already refused.
#[tokio::test]
async fn the_field_line_ceiling_outranks_the_expectation_over_http2() {
    const BUDGET: usize = 8;
    let config = AetherConfig {
        max_headers_count: BUDGET,
        ..AetherConfig::default()
    };
    let server = TestServer::start(build_app(Some(config)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let many: Vec<(String, String)> = (0..BUDGET + 1)
        .map(|i| (format!("x-pad-{i}"), "1".to_string()))
        .collect();
    let pads = || many.iter().map(|(n, v)| (n.as_str(), v.as_str()));

    let unsupported: Vec<(&str, &str)> = std::iter::once(("expect", "foo=bar"))
        .chain(pads())
        .collect();
    let paused = client
        .paused_body(
            "POST",
            "/echo",
            &unsupported,
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert!(
        paused.answered_early,
        "the h2 frontend waited for DATA before refusing a head over the ceiling"
    );
    assert_eq!(
        paused.response.status, 431,
        "an over-ceiling head carrying an unsupported expectation must answer the ceiling"
    );
    assert!(
        paused.informational.is_empty(),
        "a head that will not be held was promised an interim: {:?}",
        paused.informational
    );
    assert!(
        paused.response.body.is_empty(),
        "the refusal arrived with a body: {:?}",
        paused.response.body
    );

    let supported: Vec<(&str, &str)> = std::iter::once(("expect", "100-continue"))
        .chain(pads())
        .collect();
    let paused = client
        .paused_body(
            "POST",
            "/echo",
            &supported,
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert!(
        paused.answered_early,
        "a paused stream over the ceiling waited for DATA"
    );
    assert_eq!(
        paused.informational.as_slice(),
        &[] as &[u16],
        "a supported expectation beat the field-line ceiling over h2"
    );
    assert_eq!(paused.response.status, 431);

    client.close().await;
    server.stop().await;
}

/// A list of expectations promises every element will be met, so one unsupported element refuses
/// the request instead of earning a `100` for the supported half of it - the same reading the
/// HTTP/1 parser applies, because both frontends call one classifier.
#[tokio::test]
async fn a_mixed_expectation_list_is_refused_over_http2_too() {
    let ran = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(expectation_upload_app(Arc::clone(&ran)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let paused = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", "100-continue, foo=bar")],
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert!(paused.answered_early);
    assert_eq!(paused.response.status, 417);
    assert!(
        paused.informational.is_empty(),
        "a mixed expectation list was half-answered over h2: {:?}",
        paused.informational
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    client.close().await;
    server.stop().await;
}

/// RFC 9110 §5.5 spells a list two ways: commas in one field, or the same field repeated. HTTP/2
/// keeps both spellings observable, because a HEADERS block may carry the same name twice, so the
/// frontend that reads only the first line would serve a request the parser refuses.
#[tokio::test]
async fn repeated_expectation_fields_are_one_list_over_http2_too() {
    let ran = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(expectation_upload_app(Arc::clone(&ran)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let paused = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", "100-continue"), ("expect", "foo=bar")],
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert!(
        paused.answered_early,
        "the h2 frontend answered only the first `expect` field line and waited for the body"
    );
    assert_eq!(paused.response.status, 417);
    assert!(
        paused.informational.is_empty(),
        "repeated expectation fields were half-answered over h2: {:?}",
        paused.informational
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0);

    client.close().await;
    server.stop().await;
}

/// The interim answer this frontend owed before and never sent: a client that paused after its
/// HEADERS frame with `Expect: 100-continue` gets a `100` while the stream is still open, then the
/// handler's own response once the body arrives.
#[tokio::test]
async fn a_supported_expectation_earns_an_informational_response_over_http2() {
    let ran = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(expectation_upload_app(Arc::clone(&ran)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let paused = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", "100-continue")],
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert_eq!(
        paused.informational,
        vec![100],
        "a paused h2 stream holding a supported expectation was never answered"
    );
    assert!(
        !paused.answered_early,
        "the final response arrived before the body was ever sent"
    );
    assert_eq!(
        (paused.response.status, paused.response.body.as_str()),
        (200, "handler:5"),
        "the request was not served after its expectation was met"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    // One expectation per request: a second stream that makes none is served with no interim at
    // all, so the answer is not a property of the connection.
    let plain = client
        .paused_body("PUT", "/upload", &[], b"abc", Duration::from_millis(300))
        .await;
    assert!(
        plain.informational.is_empty(),
        "a request with no Expect was answered with an interim: {:?}",
        plain.informational
    );
    assert_eq!(
        (plain.response.status, plain.response.body.as_str()),
        (200, "handler:3")
    );

    client.close().await;
    server.stop().await;
}

/// A `route_stream` route is dispatched from the HEADERS frame, so on this frontend the expectation
/// is settled before that dispatch and both answers carry over to it: the refusal never enters the
/// handler, and the interim arrives exactly once while no DATA frame has been written. A frontend
/// that honoured `Expect` only on its buffered routes would pass every test above and fail here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_h2_route_answers_its_expectation_before_it_dispatches() {
    let entered = Arc::new(AtomicUsize::new(0));
    let mut app = AetherApp::new();
    let counter = Arc::clone(&entered);
    app = app.route_stream(Method::Put, "/upload", move |mut req: RequestContext| {
        let entered = Arc::clone(&counter);
        async move {
            entered.fetch_add(1, Ordering::SeqCst);
            let mut bytes = 0usize;
            while let Some(chunk) = req.body_stream().expect("lane").next_chunk().await {
                match chunk {
                    Ok(chunk) => bytes += chunk.len(),
                    Err(error) => return Response::text_dynamic(format!("error: {error}")),
                }
            }
            Response::text_dynamic(format!("streamed:{bytes}"))
        }
    });

    let server = TestServer::start(app.enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let refused = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", "foo=bar")],
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert!(
        refused.answered_early,
        "a streamed h2 route waited for DATA before refusing the expectation"
    );
    assert_eq!(refused.response.status, 417);
    assert!(
        refused.informational.is_empty(),
        "an unsupported expectation on a streamed route was answered with an interim: {:?}",
        refused.informational
    );
    assert_eq!(
        entered.load(Ordering::SeqCst),
        0,
        "a streamed handler ran for a request refused on its expectation"
    );

    let accepted = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", "100-continue")],
            b"hello",
            Duration::from_secs(2),
        )
        .await;
    assert_eq!(
        accepted.informational,
        vec![100],
        "a streamed h2 route must answer its supported expectation exactly once"
    );
    assert!(
        !accepted.answered_early,
        "the streamed route answered before its body was sent"
    );
    assert_eq!(
        (accepted.response.status, accepted.response.body.as_str()),
        (200, "streamed:5"),
        "the streamed body was not served after its expectation was met"
    );
    assert_eq!(entered.load(Ordering::SeqCst), 1);

    client.close().await;
    server.stop().await;
}

/// An `Expect` field that names nothing (RFC 9110 §5.5's `#` rule drops the empty elements) is no
/// expectation: the stream earns neither an interim nor a refusal, and the body is asked for as if
/// the field had not been sent.
#[tokio::test]
async fn an_empty_expectation_list_owes_nothing_over_http2() {
    let ran = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(expectation_upload_app(Arc::clone(&ran)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let paused = client
        .paused_body(
            "PUT",
            "/upload",
            &[("expect", ",")],
            b"hello",
            Duration::from_millis(300),
        )
        .await;
    assert!(
        !paused.answered_early,
        "an empty expectation list was answered over h2"
    );
    assert!(paused.informational.is_empty());
    assert_eq!(
        (paused.response.status, paused.response.body.as_str()),
        (200, "handler:5")
    );

    client.close().await;
    server.stop().await;
}

/// RFC 9112 §9.3.3 answers a client that stopped, not a client that never started. A stream whose
/// HEADERS frame already ended the body has nothing pending, so a `100` over it would be noise:
/// the request is served straight through, exactly as the HTTP/1 frontend serves a request that
/// already arrived whole.
#[tokio::test]
async fn a_stream_that_already_ended_its_body_earns_no_informational_over_http2() {
    let ran = Arc::new(AtomicUsize::new(0));
    let server = TestServer::start(expectation_upload_app(Arc::clone(&ran)).enable_h2c()).await;
    let mut client = H2Client::connect_plain(server.addr).await;

    let sent = client
        .ended_at_head(
            "PUT",
            "/upload",
            &[("expect", "100-continue")],
            Duration::from_secs(2),
        )
        .await;
    assert!(
        sent.answered_early,
        "a stream with no pending body was not answered from its head"
    );
    assert!(
        sent.informational.is_empty(),
        "an interim was sent for a body that had already ended: {:?}",
        sent.informational
    );
    assert_eq!(
        (sent.response.status, sent.response.body.as_str()),
        (200, "handler:0"),
        "a supported expectation cost the request its answer"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    client.close().await;
    server.stop().await;
}

// ---------------------------------------------------------------------------------------------
// Track 123 answered two request-dependent questions on the shared response path: content
// negotiation honours a refused identity, and every CORS refusal names the fields that decided
// it. Those fixes were pinned on HTTP/1.1 only, because that is where the wire was read. Both
// frontends run the same `ServiceCore` response exit, so the h2 pins below exist to prove the
// claim travels the multiplexed path too rather than to re-argue it: if the core ever grew a
// frontend-specific exit, these fail while the HTTP/1 pins stay green.
// ---------------------------------------------------------------------------------------------

fn cors_app(allowed: &'static [&'static str], methods: &'static str) -> AetherApp {
    let mut app = build_app(None);
    app = app.enable_cors(CorsMiddleware::new(CorsConfig {
        allowed_origins: allowed,
        allow_methods: methods,
        ..CorsConfig::default()
    }));
    app = app.get("/data", |_| Response::text("payload"));
    app
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_origin_varies_on_origin_over_h2() {
    let pki = pki("aether-h2.test");
    let server =
        TestServer::start(cors_app(&["https://app.example.com"], "GET, POST").tls(acceptor(&pki)))
            .await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let denied = client
        .request(
            "GET",
            "/data",
            None,
            &[("origin", "https://evil.example.com")],
        )
        .await;
    assert_eq!(denied.status, 200, "body: {}", denied.body);
    assert_eq!(
        denied.header("access-control-allow-origin"),
        None,
        "an origin outside the allow-list must not be echoed"
    );
    assert!(
        denied
            .header("vary")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("origin"),
        "a refusal must vary on Origin, got {:?}",
        denied.header("vary")
    );

    let allowed = client
        .request(
            "GET",
            "/data",
            None,
            &[("origin", "https://app.example.com")],
        )
        .await;
    assert_eq!(
        allowed.header("access-control-allow-origin"),
        Some("https://app.example.com"),
        "the allow-listed origin must still be reflected on the same connection"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_preflight_varies_on_the_fields_that_decided_it_over_h2() {
    let pki = pki("aether-h2.test");
    let server =
        TestServer::start(cors_app(&["https://app.example.com"], "GET").tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // `POST /echo` exists, so the target is routable and only the middleware's method check can
    // refuse this. With a method the app does not serve, the rebuild would refuse it on its own
    // and this pin would stay green with the check deleted.
    let refused = client
        .request(
            "OPTIONS",
            "/echo",
            None,
            &[
                ("origin", "https://app.example.com"),
                ("access-control-request-method", "POST"),
            ],
        )
        .await;
    assert_eq!(refused.status, 403, "body: {}", refused.body);
    assert_eq!(refused.header("access-control-allow-origin"), None);
    let vary = refused
        .header("vary")
        .unwrap_or_default()
        .to_ascii_lowercase();
    for token in [
        "origin",
        "access-control-request-method",
        "access-control-request-headers",
    ] {
        assert!(
            vary.contains(token),
            "{token} missing from {:?} on an h2 refusal",
            refused.header("vary")
        );
    }

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_preflight_for_an_unroutable_method_is_answered_without_cors_headers_over_h2() {
    let pki = pki("aether-h2.test");
    let server = TestServer::start(
        cors_app(&["https://app.example.com"], "GET, POST, PATCH").tls(acceptor(&pki)),
    )
    .await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // PATCH is allowed by the CORS config, so the middleware would approve it, but no route
    // serves PATCH here and the answer is a rebuilt 403. The approval headers must not survive
    // that rebuild, and the `Vary` the middleware wrote must.
    let refused = client
        .request(
            "OPTIONS",
            "/echo",
            None,
            &[
                ("origin", "https://app.example.com"),
                ("access-control-request-method", "PATCH"),
            ],
        )
        .await;
    assert_eq!(refused.status, 403, "body: {}", refused.body);
    for name in [
        "access-control-allow-origin",
        "access-control-allow-methods",
        "access-control-allow-headers",
        "access-control-max-age",
    ] {
        assert_eq!(refused.header(name), None, "{name} leaked onto the 403");
    }
    assert!(
        refused
            .header("vary")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("origin"),
        "the rebuilt refusal is still origin-dependent"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_preflight_is_answered_with_its_allow_fields_over_h2() {
    let pki = pki("aether-h2.test");
    let server =
        TestServer::start(cors_app(&["https://app.example.com"], "GET, POST").tls(acceptor(&pki)))
            .await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    // The refusal pins above are all satisfied by a server that refuses every preflight, so the
    // approval side is pinned on the same connection: `POST /echo` exists and is allowed, and
    // `authorization` is in the configured allow-headers list.
    let approved = client
        .request(
            "OPTIONS",
            "/echo",
            None,
            &[
                ("origin", "https://app.example.com"),
                ("access-control-request-method", "POST"),
                ("access-control-request-headers", "authorization"),
            ],
        )
        .await;
    assert_eq!(approved.status, 204, "body: {}", approved.body);
    assert!(approved.raw.is_empty(), "a 204 preflight carries no body");
    assert_eq!(
        approved.header("access-control-allow-origin"),
        Some("https://app.example.com")
    );
    assert_eq!(
        approved.header("access-control-allow-methods"),
        Some("GET, POST")
    );
    assert_eq!(
        approved.header("access-control-allow-headers"),
        Some("Authorization, Content-Type, Accept")
    );
    let vary = approved
        .header("vary")
        .unwrap_or_default()
        .to_ascii_lowercase();
    for token in [
        "origin",
        "access-control-request-method",
        "access-control-request-headers",
    ] {
        assert!(
            vary.contains(token),
            "{token} missing from {:?} on an h2 approval",
            approved.header("vary")
        );
    }

    let refused = client
        .request(
            "OPTIONS",
            "/echo",
            None,
            &[
                ("origin", "https://evil.example.com"),
                ("access-control-request-method", "POST"),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 403,
        "a server that approves every preflight must fail here"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_identity_is_answered_over_h2() {
    let pki = pki("aether-h2.test");
    let mut app = build_app(None);
    app = app.enable_compression();
    app = app.get("/big", |_| Response::text_dynamic("a".repeat(8_000)));
    let server = TestServer::start(app.tls(acceptor(&pki))).await;
    let mut client = H2Client::connect(server.addr, "aether-h2.test", &pki.ca_der).await;

    let refused = client
        .request("GET", "/big", None, &[("accept-encoding", "identity;q=0")])
        .await;
    assert_eq!(refused.status, 406, "body: {}", refused.body);
    assert!(
        refused.raw.is_empty(),
        "a 406 must not carry the representation it is refusing"
    );
    assert_eq!(refused.header("content-encoding"), None);
    // The HTTP/1.1 sibling asserts a `Content-Length: 0` on this answer. The h2 writer may omit
    // the field for a bodyless response, but a length left over from the withheld representation
    // is a defect either way.
    if let Some(len) = refused.header("content-length") {
        assert_eq!(len, "0", "a bodyless 406 kept a stale content-length");
    }
    assert!(
        refused
            .header("vary")
            .is_some_and(|vary| vary.to_ascii_lowercase().contains("accept-encoding")),
        "the refusal depends on Accept-Encoding and must say so, got {:?}",
        refused.header("vary")
    );

    let coded = client
        .request(
            "GET",
            "/big",
            None,
            &[("accept-encoding", "gzip, identity;q=0")],
        )
        .await;
    assert_eq!(coded.status, 200);
    assert_eq!(coded.header("content-encoding"), Some("gzip"));
    // A shape-only pin goes green on a garbled body, so read the member's own framing: the magic
    // at the front, and the trailer's ISIZE (last four bytes, little-endian) reporting the 8,000
    // bytes this body decodes to. A member truncated after its header fails the second check.
    assert!(
        coded.raw.starts_with(&[0x1f, 0x8b]),
        "a body labelled gzip must start with the gzip magic, got {:02x?}",
        &coded.raw[..coded.raw.len().min(4)]
    );
    assert!(
        coded.raw.len() > 18,
        "a gzip member is at least its 10-byte header and an 8-byte trailer, got {}",
        coded.raw.len()
    );
    let isize = u32::from_le_bytes(
        coded.raw[coded.raw.len() - 4..]
            .try_into()
            .expect("the slice is four bytes"),
    );
    assert_eq!(
        isize, 8_000,
        "the gzip trailer must report the decoded length"
    );
    if let Some(len) = coded.header("content-length") {
        assert_eq!(
            len.parse::<usize>().ok(),
            Some(coded.raw.len()),
            "content-length must describe the coded body"
        );
    }

    client.close().await;
    server.stop().await;
}
