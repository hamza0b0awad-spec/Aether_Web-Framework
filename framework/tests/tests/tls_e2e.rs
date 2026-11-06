//! TLS end-to-end tests over real TCP sockets and a real rustls client.
//!
//! These exist because every claim in `aether::tls` is a claim about a handshake: that one port
//! answers both plaintext and TLS, that ALPN is what decides the framing, that a reload reaches
//! the next client without a restart, and that a connection the server cannot serve is closed
//! rather than answered in the wrong protocol. Unit tests of the certificate loader cannot show
//! any of that, so each test here completes a TLS handshake against `AetherServer`.
//!
//! Certificates are minted per run by `rcgen` (a throwaway CA plus one leaf). Nothing in this
//! file is secret, and nothing is committed to the repository.

use aether::prelude::*;
use aether::tls::{TlsAcceptor, TlsCertSet, TlsIdentity};
use rustls::pki_types::CertificateDer;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

    async fn stop(self) {
        let Self {
            shutdown,
            task,
            addr: _,
        } = self;
        drop(shutdown.expect("shutdown sender"));
        match tokio::time::timeout(Duration::from_secs(10), task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => panic!("server returned an error: {error}"),
            Ok(Err(join)) => panic!("server task failed: {join}"),
            Err(_) => panic!("server did not finish graceful shutdown within 10s"),
        }
    }
}

/// A CA and the leaf it signs, for one DNS name.
struct Pki {
    /// Leaf first, then the CA: the order a chain must be sent in.
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

fn cert_set(pki: &Pki) -> TlsCertSet {
    TlsCertSet::from_pem(&pki.chain_pem, &pki.key_pem).expect("valid material")
}

/// The bytes of a `GET` that must be answered as HTTP/1.1 on this connection.
const PING: &[u8] = b"GET /ping HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n";

/// A rustls client that trusts `anchors`, with ALPN set to HTTP/1.1.
///
/// Failures come back as the message the client saw, because that message — `UnknownIssuer`,
/// `BadSignature`, a broken pipe mid-handshake — is what each test asserts on.
async fn tls_connect(
    addr: SocketAddr,
    domain: &str,
    anchors: &[&CertificateDer<'static>],
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
    let mut roots = rustls::RootCertStore::empty();
    for anchor in anchors {
        roots.add((*anchor).clone()).expect("trust anchor");
    }
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let tcp = TcpStream::connect(addr).await.expect("connect");
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let name =
        rustls::pki_types::ServerName::try_from(domain.to_string()).expect("domain is a dns name");
    connector
        .connect(name, tcp)
        .await
        .map_err(|e| e.to_string())
}

/// Read an entire HTTP/1.1 response with a `Content-Length`, from anything readable.
async fn read_response<R: tokio::io::AsyncRead + Unpin>(io: &mut R) -> String {
    let mut bytes = Vec::new();
    let mut head_end = None;
    let mut expected_body = 0usize;
    loop {
        let mut chunk = [0u8; 1024];
        let n = io.read(&mut chunk).await.expect("read");
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if head_end.is_none()
            && let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(&bytes[..pos]).to_lowercase();
            head_end = Some(pos + 4);
            expected_body = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
        }
        if let Some(end) = head_end
            && bytes.len() >= end + expected_body
        {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn app_with_ping() -> AetherApp {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    app
}

#[tokio::test]
async fn one_port_answers_plaintext_and_tls() {
    let pki = pki("localhost");
    let acceptor = TlsAcceptor::new(cert_set(&pki)).expect("acceptor");
    let alpn = acceptor.alpn().to_vec();
    let server = TestServer::start(app_with_ping().tls(acceptor)).await;

    // Plaintext on the same port the certificate is served on.
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(PING).await.expect("send");
    let plain = read_response(&mut tcp).await;
    assert!(plain.starts_with("HTTP/1.1 200"), "{plain}");
    assert!(plain.ends_with("pong"), "{plain}");

    // And a TLS connection that negotiated HTTP/1.1 through ALPN.
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(&b"http/1.1"[..]),
        "ALPN must select http/1.1, got {:?}",
        tls.get_ref()
            .1
            .alpn_protocol()
            .map(|p| String::from_utf8_lossy(p).into_owned())
    );
    tls.write_all(PING).await.expect("send over tls");
    let secure = read_response(&mut tls).await;
    assert!(secure.starts_with("HTTP/1.1 200"), "{secure}");
    assert!(secure.ends_with("pong"), "{secure}");

    // `h2` is offered first now that the HTTP/2 frontend is wired, and `http/1.1` stays in the
    // list: a client that advertises only HTTP/1 must still be served on this port.
    assert_eq!(alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    server.stop().await;
}

#[tokio::test]
async fn a_client_that_does_not_trust_the_certificate_cannot_get_in() {
    let served = pki("localhost");
    let stranger = pki("localhost");
    let server = TestServer::start(
        app_with_ping().tls(TlsAcceptor::new(cert_set(&served)).expect("acceptor")),
    )
    .await;

    // A CA the server does not use: the handshake must fail, not fall through to plaintext.
    let err = tls_connect(server.addr, "localhost", &[&stranger.ca_der])
        .await
        .expect_err("untrusted chain");
    assert!(
        err.to_string().contains("BadSignature") || err.to_string().contains("UnknownIssuer"),
        "unexpected handshake failure: {err}"
    );

    // The very same port still serves a client that does trust it.
    let mut tls = tls_connect(server.addr, "localhost", &[&served.ca_der])
        .await
        .expect("handshake with the right anchor");
    tls.write_all(PING).await.expect("send");
    assert!(read_response(&mut tls).await.starts_with("HTTP/1.1 200"));
    server.stop().await;
}

#[tokio::test]
async fn require_tls_closes_plaintext_without_answering_it() {
    let pki = pki("localhost");
    let acceptor = TlsAcceptor::new(cert_set(&pki)).expect("acceptor");
    let server = TestServer::start(app_with_ping().tls(acceptor).require_tls(true)).await;

    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(PING).await.expect("send");
    let mut buf = [0u8; 64];
    // Refused means *refused*: the connection is closed, so the read ends at once rather than
    // hanging and rather than yielding an HTTP status a plaintext client could misread as
    // "tls is optional".
    let read = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buf))
        .await
        .expect("closed within the read timeout");
    assert_eq!(
        read.expect("io"),
        0,
        "plaintext must not receive a response"
    );

    // TLS still works while plaintext is refused.
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");
    tls.write_all(PING).await.expect("send");
    assert!(read_response(&mut tls).await.starts_with("HTTP/1.1 200"));
    server.stop().await;
}

#[tokio::test]
async fn a_server_without_tls_closes_a_tls_client_instead_of_hanging() {
    let pki = pki("localhost");
    let server = TestServer::start(app_with_ping()).await;

    let started = std::time::Instant::now();
    let err = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect_err("no certificate configured");
    // The claim being tested is *closed, not hung*: the server saw a TLS record, had no
    // certificate, and shut the connection down. rustls reports that as an io/alert failure of
    // some shape, so the assertion is on the bound and on the failure, not on one message.
    assert!(!err.is_empty(), "handshake must fail with a reason");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the handshake hung for {:?} instead of being closed",
        started.elapsed()
    );

    // And the same port still serves plaintext, because nothing about the failed handshake
    // leaked into the listener.
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(PING).await.expect("send");
    assert!(read_response(&mut tcp).await.starts_with("HTTP/1.1 200"));
    server.stop().await;
}

#[tokio::test]
async fn an_h2c_preface_is_closed_not_answered_in_http1() {
    let server = TestServer::start(app_with_ping()).await;
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    tcp.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .expect("send preface");
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buf))
        .await
        .expect("closed within the read timeout");
    // A 400 or a 505 here would be bytes an h2 client cannot parse as a frame. HTTP/2 over TLS
    // exists, but it is reached through ALPN, and a plaintext preface has no negotiation to
    // consult — so closing is the only honest answer for h2c.
    assert_eq!(read.expect("io"), 0, "h2c must be closed, not answered");
    server.stop().await;
}

#[tokio::test]
async fn reloading_certificates_reaches_the_next_handshake() {
    let old = pki("localhost");
    let new = pki("localhost");
    let acceptor = TlsAcceptor::new(cert_set(&old)).expect("acceptor");
    let live = acceptor.clone();
    let server = TestServer::start(app_with_ping().tls(acceptor)).await;

    // A client that trusts only the *new* CA cannot connect yet.
    let err = tls_connect(server.addr, "localhost", &[&new.ca_der])
        .await
        .expect_err("not reloaded yet");
    assert!(
        err.contains("UnknownIssuer") || err.contains("BadSignature"),
        "the old certificate must be the one served, so a client of the new CA must fail on \
         the chain of trust, not on something else: {err}"
    );

    live.replace_certificates(vec![TlsIdentity::default_identity(cert_set(&new))])
        .expect("reload without restarting");

    let mut tls = tls_connect(server.addr, "localhost", &[&new.ca_der])
        .await
        .expect("handshake after reload");
    tls.write_all(PING).await.expect("send");
    assert!(read_response(&mut tls).await.ends_with("pong"));

    // The old anchor is now the one that must fail: the swap replaced what is served, it did
    // not add a second trust path.
    let err = tls_connect(server.addr, "localhost", &[&old.ca_der])
        .await
        .expect_err("old ca no longer served");
    assert!(
        err.to_string().contains("UnknownIssuer") || err.to_string().contains("BadSignature"),
        "unexpected failure after reload: {err}"
    );
    server.stop().await;
}

#[tokio::test]
async fn sni_chooses_the_named_identity_and_unknown_names_use_the_default() {
    let default = pki("localhost");
    let named = pki("aether.test");
    let acceptor = TlsAcceptor::builder()
        .insert(TlsIdentity::default_identity(cert_set(&default)))
        .insert(TlsIdentity::for_name("aether.test", cert_set(&named)).expect("a dns name"))
        .build()
        .expect("acceptor");
    let server = TestServer::start(app_with_ping().tls(acceptor)).await;

    // The name the client asks for selects the certificate: each trusts a different CA, so a
    // successful handshake *is* the proof of selection.
    let mut named_client = tls_connect(server.addr, "aether.test", &[&named.ca_der])
        .await
        .expect("sni selection");
    named_client.write_all(PING).await.expect("send");
    assert!(read_response(&mut named_client).await.ends_with("pong"));

    let mut other = tls_connect(server.addr, "localhost", &[&default.ca_der])
        .await
        .expect("default identity for an unlisted name");
    other.write_all(PING).await.expect("send");
    assert!(read_response(&mut other).await.ends_with("pong"));
    server.stop().await;
}

/// A client frame: RFC 6455 §5.1 requires every client-sent frame to be masked.
fn masked_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Read one unmasked server frame (the driver never masks, as RFC 6455 requires of a server).
async fn read_server_frame<R: tokio::io::AsyncRead + Unpin>(io: &mut R) -> (u8, Vec<u8>) {
    let mut two = [0u8; 2];
    io.read_exact(&mut two).await.expect("frame header");
    let mut len = (two[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        io.read_exact(&mut ext).await.expect("extended length");
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        io.read_exact(&mut ext).await.expect("extended length");
        len = usize::try_from(u64::from_be_bytes(ext)).expect("frame fits in memory");
    }
    let mut payload = vec![0u8; len];
    io.read_exact(&mut payload).await.expect("frame payload");
    (two[0] & 0x0f, payload)
}

#[tokio::test]
async fn websocket_upgrade_and_echo_run_over_tls() {
    // `WebSocket` is handed the transport, not a `TcpStream`, which is the whole reason the
    // public handler signature became `WebSocket<Stream>`: an upgrade must work after a TLS
    // handshake instead of only on plaintext.
    let mut app = AetherApp::new();
    app = app.ws("/wss", |_req, mut socket| async move {
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

    let pki = pki("localhost");
    let server =
        TestServer::start(app.tls(TlsAcceptor::new(cert_set(&pki)).expect("acceptor"))).await;
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");

    tls.write_all(
        b"GET /wss HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
          Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await
    .expect("send upgrade");

    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tls.read_exact(&mut byte).await.expect("read upgrade head");
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(
        head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "the accept vector must survive the TLS layer: {head}"
    );

    tls.write_all(&masked_frame(0x1, b"over tls", [9, 8, 7, 6]))
        .await
        .expect("send frame");
    let (opcode, payload) = read_server_frame(&mut tls).await;
    assert_eq!((opcode, payload.as_slice()), (0x1, &b"over tls"[..]));

    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice(b"bye");
    tls.write_all(&masked_frame(0x8, &close, [1, 1, 1, 1]))
        .await
        .expect("send close");
    let (opcode, echoed) = read_server_frame(&mut tls).await;
    assert_eq!((opcode, echoed), (0x8, close));
    server.stop().await;
}

#[tokio::test]
async fn a_connection_that_never_speaks_is_closed_by_the_read_timeout() {
    // `read_timeout` is 5s by default; the classification peek is bounded by it, so a client
    // that opens the socket and stays silent cannot pin a task, a permit and a buffer.
    let pki = pki("localhost");
    let server =
        TestServer::start(app_with_ping().tls(TlsAcceptor::new(cert_set(&pki)).expect("acceptor")))
            .await;
    let started = std::time::Instant::now();
    let mut tcp = TcpStream::connect(server.addr).await.expect("connect");
    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(12), tcp.read(&mut buf))
        .await
        .expect("closed well within the timeout window");
    assert_eq!(read.expect("io"), 0, "silent connection must be dropped");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(3_500),
        "closed after {elapsed:?}, which is early enough to also fail a legitimate slow client"
    );

    // The listener is unharmed.
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake after a timed-out connection");
    tls.write_all(PING).await.expect("send");
    assert!(read_response(&mut tls).await.ends_with("pong"));
    server.stop().await;
}

/// CONTRACT: the receive-budget answer belongs to the serving loop, not to the plaintext
/// frontend. Track 117 fixed the silent close of a buffered request whose bytes stopped arriving
/// and pinned it on cleartext sockets only, so nothing asserted that the same `408` reaches a TLS
/// client — where the answer has to be TLS application data and the end has to be a
/// `close_notify` rather than a bare FIN. The budget is 600 ms against the 5 s idle timeout, so a
/// server that closes on the idle timeout cannot answer inside the wrap, and the lower bound
/// rejects a server that answers at the first pause instead of at the budget.
#[tokio::test]
async fn a_partial_request_over_tls_is_answered_408_and_closed() {
    let config = AetherConfig {
        max_receive_time: Duration::from_millis(600),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.get("/ping", |_| "pong");
    app = app.post("/upload", |req| format!("got:{}", req.body.len()));

    let pki = pki("localhost");
    let acceptor = TlsAcceptor::new(cert_set(&pki)).expect("acceptor");
    let server = TestServer::start(app.tls(acceptor)).await;
    let head = b"POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\n";

    async fn answer(mut tls: tokio_rustls::client::TlsStream<TcpStream>) {
        let started = std::time::Instant::now();
        let text = tokio::time::timeout(Duration::from_millis(1500), read_response(&mut tls))
            .await
            .expect("the receive budget, not the 5 s idle timeout, must bound a stopped request");
        let elapsed = started.elapsed();
        assert!(
            text.starts_with("HTTP/1.1 408"),
            "a stopped request over TLS was answered with {:?}",
            &text[..40.min(text.len())]
        );
        assert!(
            text.to_ascii_lowercase()
                .contains("\r\nconnection: close\r\n"),
            "the 408 over TLS must close the connection: {text:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(400),
            "answered after {elapsed:?}, earlier than the 600 ms receive budget"
        );
        let mut rest = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(2), tls.read(&mut rest))
            .await
            .expect("the server did not close after its 408")
            .expect("the TLS connection must end with a close_notify, not a truncated record");
        assert_eq!(
            n, 0,
            "bytes arrived after the 408 that promised `Connection: close`"
        );
    }

    // A complete head whose body never starts.
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");
    tls.write_all(head).await.expect("send head");
    answer(tls).await;

    // Three of five body bytes, then silence.
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");
    tls.write_all(head).await.expect("send head");
    tls.write_all(b"abc").await.expect("send partial body");
    answer(tls).await;

    // The refusal belongs to the connection, not the listener.
    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake after a refused connection");
    tls.write_all(PING).await.expect("send");
    assert!(read_response(&mut tls).await.ends_with("pong"));
    server.stop().await;
}

/// CONTRACT: the transport beneath TLS is watched too, so a TLS client that vanishes mid-handler
/// is not an exception. Only an *abrupt* end counts: dropping the stream sends no `close_notify`,
/// which rustls reports as an EOF where a record was expected. A `close_notify` is the clean
/// form — "I will send no more" — and a client that sends it may still be reading, so it is the
/// subject of the next test instead.
#[tokio::test]
async fn a_tls_client_that_leaves_cancels_the_running_handler() {
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
    app = app.get("/ping", |_| "pong");

    let pki = pki("localhost");
    let server =
        TestServer::start(app.tls(TlsAcceptor::new(cert_set(&pki)).expect("acceptor"))).await;

    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");
    tls.write_all(b"GET /slow HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("send over tls");
    for _ in 0..200 {
        if started.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        started.load(Ordering::SeqCst),
        "the handler never started, so the departure would measure nothing"
    );
    // No `shutdown()`: the client vanishes without a `close_notify`, which is the abrupt end a
    // crash or a dropped link produces.
    drop(tls);

    // Past the handler's own 400 ms sleep: an uncancelled handler has finished by now.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the handler ran to completion after its TLS client left"
    );

    // Only that connection was affected.
    let mut fresh = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake after a departed client");
    fresh.write_all(PING).await.expect("send");
    assert!(read_response(&mut fresh).await.ends_with("pong"));
    server.stop().await;
}

/// CONTRACT: a TLS client that closes only its sending half — a `close_notify`, then no more
/// application data — is not gone. It still reads the connection, so its response must be
/// written. RFC 8446 dropped the requirement to reply to `close_notify` by closing immediately
/// for exactly this reason: reacting to it as a departure truncates responses a client is
/// waiting for.
#[tokio::test]
async fn a_tls_client_that_half_closes_still_gets_its_response() {
    let finished = Arc::new(AtomicUsize::new(0));
    let finished_handler = Arc::clone(&finished);
    let mut app = AetherApp::new();
    app = app.get_async("/slow", move |_: RequestContext| {
        let finished = Arc::clone(&finished_handler);
        async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            finished.fetch_add(1, Ordering::SeqCst);
            Response::text("late")
        }
    });

    let pki = pki("localhost");
    let server =
        TestServer::start(app.tls(TlsAcceptor::new(cert_set(&pki)).expect("acceptor"))).await;

    let mut tls = tls_connect(server.addr, "localhost", &[&pki.ca_der])
        .await
        .expect("handshake");
    tls.write_all(b"GET /slow HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("send over tls");
    // `shutdown()` writes the `close_notify`; the read half stays open for the answer.
    tls.shutdown().await.expect("close_notify");

    let response = read_response(&mut tls).await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the half-closing client was answered with {response:?}"
    );
    assert!(
        response.ends_with("late"),
        "the half-closing client did not get the handler's body: {response:?}"
    );
    assert_eq!(
        finished.load(Ordering::SeqCst),
        1,
        "the handler must run to completion for a client that is still reading"
    );
    server.stop().await;
}
