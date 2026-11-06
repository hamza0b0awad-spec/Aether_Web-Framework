//! The connection transport layer: one socket, several possible framings.
//!
//! A server that speaks HTTP/1.1, HTTP/2 and TLS on the same port cannot decide what a
//! connection *is* at accept time — the first bytes say so. This module owns the two things
//! that follow from that: a type-erased byte stream the serving loop can be written once
//! over, and the peek-based classifier that picks a protocol without consuming anything.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

/// A connection's byte stream, whichever framing the transport put around it.
///
/// An enum rather than a generic parameter because this type reaches the public API: a
/// WebSocket handler is `WebSocket<Stream>`, so one signature serves plaintext, TLS, and
/// (later) any other framing, instead of pinning the framework to `TcpStream` or pushing a
/// box onto every connection.
#[expect(
    clippy::large_enum_variant,
    reason = "boxing the TLS stream costs an allocation per connection, which this type's doc rule refuses"
)]
pub enum Stream {
    Plain(TcpStream),
    Tls(tokio_rustls::server::TlsStream<TcpStream>),
}

impl Stream {
    /// The address of the peer that opened the TCP connection.
    ///
    /// Always the transport peer, never anything the client asserted in a header: behind a
    /// TLS-terminating proxy this is still the proxy, which is why rate limiting documented
    /// the same caveat before TLS existed here.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Stream::Plain(s) => s.peer_addr(),
            Stream::Tls(s) => s.get_ref().0.peer_addr(),
        }
    }

    /// The protocol ALPN selected for this connection, when it is a TLS one.
    ///
    /// This is the only trustworthy answer to "what may I speak on this connection": a
    /// `Connection: close` header on a TLS connection negotiated as `h2` is a framing error,
    /// not a hint, so the HTTP/2 frontend must key its close semantics here instead.
    pub fn negotiated_alpn(&self) -> Option<&[u8]> {
        match self {
            Stream::Plain(_) => None,
            Stream::Tls(s) => s.get_ref().1.alpn_protocol(),
        }
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Why a connection was closed before any request was served.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The peer opened the connection and sent nothing.
    Empty,
    /// The first byte arrived, then nothing, inside `read_timeout`.
    TimedOut,
    /// The socket failed while being inspected.
    Io,
    /// A TLS record arrived but the server has no certificate configured.
    NoTlsConfigured,
    /// Plaintext arrived on a server configured to require TLS.
    PlaintextRefused,
    /// The handshake did not complete or was rejected.
    Handshake,
    /// The connection opened with the HTTP/2 preface on a server that has not opted into
    /// serving HTTP/2 over cleartext ([`crate::app::AetherApp::enable_h2c`]).
    ///
    /// Answering a h2c preface with an HTTP/1 status line would be a lie the client cannot
    /// parse, so the honest response is a closed connection until the operator asks for h2c.
    Http2NotSupported,
}

/// Which framing loop an established connection belongs to.
///
/// One decision, made by the layer that actually looked at the bytes: ALPN for a TLS connection,
/// the preface plus the operator's h2c opt-in for a plaintext one. Producing it here rather than
/// re-deriving it from the stream is what keeps the transport and the serving loop from
/// disagreeing about the same socket — a plaintext HTTP/2 connection has no ALPN to read, so any
/// "is this h2?" question asked of the stream afterwards can only answer *no*.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Frontend {
    Http1,
    Http2,
}

/// Decide what a freshly accepted connection is, and put the right transport around it.
///
/// Both phases are bounded: the first byte by `first_byte_timeout`, the handshake by
/// `handshake_timeout`. Without them a client that completes the TCP handshake and then
/// trickles bytes holds a task, a connection permit and a socket buffer forever — the same
/// slowloris class the header read path already defends against, moved one layer earlier.
///
/// `allow_h2c` is the operator's opt-in to serve the HTTP/2 preface on a plaintext socket. It is
/// off by default because cleartext HTTP/2 has no negotiation step at all: the first 24 bytes
/// *are* the protocol choice, so a proxy in front that rewrites or buffers them, or a client that
/// means `Upgrade: h2c` and gets a live HTTP/2 session it did not ask for, has no handshake to
/// catch the mistake.
pub async fn establish(
    tcp: TcpStream,
    tls: Option<&crate::tls::TlsAcceptor>,
    first_byte_timeout: Duration,
    handshake_timeout: Duration,
    allow_h2c: bool,
) -> Result<(Stream, Frontend), Refused> {
    let greeting = match tokio::time::timeout(first_byte_timeout, classify(&tcp)).await {
        Ok(Ok(g)) => g,
        Ok(Err(_)) => return refuse(tcp, Refused::Io).await,
        Err(_) => return refuse(tcp, Refused::TimedOut).await,
    };
    // The `requires_tls` gate is about *plaintext*, not about HTTP/1: whichever framing a
    // cleartext connection wants, an operator who required TLS refused to terminate it at all.
    let plaintext_forbidden = tls.is_some_and(|acceptor| acceptor.requires_tls());
    match greeting {
        Greeting::Empty => refuse(tcp, Refused::Empty).await,
        Greeting::Http1 => {
            if plaintext_forbidden {
                return refuse(tcp, Refused::PlaintextRefused).await;
            }
            Ok((Stream::Plain(tcp), Frontend::Http1))
        }
        Greeting::Tls => {
            let Some(acceptor) = tls else {
                return refuse(tcp, Refused::NoTlsConfigured).await;
            };
            match tokio::time::timeout(handshake_timeout, acceptor.accept(tcp)).await {
                Ok(Ok(stream)) => {
                    let stream = Stream::Tls(stream);
                    let frontend = if stream.negotiated_alpn() == Some(b"h2".as_slice()) {
                        Frontend::Http2
                    } else {
                        Frontend::Http1
                    };
                    Ok((stream, frontend))
                }
                // The socket went with the handshake attempt; there is nothing left to shut down.
                Ok(Err(_)) => Err(Refused::Handshake),
                Err(_) => Err(Refused::TimedOut),
            }
        }
        Greeting::H2Preface => {
            if plaintext_forbidden {
                return refuse(tcp, Refused::PlaintextRefused).await;
            }
            if !allow_h2c {
                return refuse(tcp, Refused::Http2NotSupported).await;
            }
            Ok((Stream::Plain(tcp), Frontend::Http2))
        }
    }
}

/// Close cleanly rather than letting the socket drop with unread bytes in the receive buffer:
/// on Windows that turns into an RST, which discards anything the client may still be reading.
async fn refuse(mut tcp: TcpStream, reason: Refused) -> Result<(Stream, Frontend), Refused> {
    let _ = tcp.shutdown().await;
    Err(reason)
}

/// What the first bytes of a connection say about the protocol it wants.
#[derive(Debug, PartialEq, Eq)]
pub enum Greeting {
    /// HTTP/1.1 or HTTP/1.0: anything that is not a TLS record or the HTTP/2 preface.
    ///
    /// HTTP/1 is the *fallback* rather than a method-name match, because the parser is what
    /// decides whether a head is well-formed and it answers a bad one with a 400. A server
    /// that classified by method name would silently hang up on clients that deserve a real
    /// HTTP status (and any future method token), which is a worse failure than a 400.
    Http1,
    /// The client is speaking TLS.
    Tls,
    /// The client asked for HTTP/2 directly, without TLS and without an upgrade request.
    H2Preface,
    /// Fewer bytes than the shortest discriminator arrived, and then EOF.
    Empty,
}

/// The HTTP/2 client magic (RFC 9113 §3.4) that a h2c connection must open with.
pub const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Classify a connection from its leading bytes *without consuming them*.
///
/// `peek` is deliberate: the bytes are still in the kernel receive buffer afterwards, so the
/// HTTP/1 parser sees the request exactly as it would have without sniffing, and a TLS
/// handshake starts from the ClientHello rather than a stream with its head eaten.
///
/// Because a peek reports from the socket's *first unread byte* and never advances it, a second
/// peek of the same connection returns the same leading bytes again — so what a peek yields
/// replaces the probe prefix, it does not extend it, and a connection whose discriminator arrives
/// in several segments is only recognised once the later segments have shown up. Both of those
/// are why the loop counts waits rather than bytes: re-peeking a socket that has stopped sending
/// would otherwise be a hot loop on one worker, and a bound is what turns it into the fallback
/// the classifier already documents.
pub async fn classify(stream: &TcpStream) -> io::Result<Greeting> {
    // 3 bytes is the minimum that separates a TLS record (0x16 0x03 xx) from a method token,
    // and 24 is what the HTTP/2 preface needs. One buffer covers both.
    let mut probe = [0u8; 24];
    let mut filled = 0usize;
    let mut waits = 0usize;
    let mut scratch = [0u8; 24];
    loop {
        // `peek`, not `read`: a TLS ClientHello must still be in the socket for rustls, and an
        // HTTP/1 request head must still be there for the parser.
        let got = stream.peek(&mut scratch).await?;
        if got == 0 {
            // EOF, or a client that opened the socket and said nothing. Whatever arrived
            // (if anything) is still classified from the bytes we do have.
            return Ok(decide(&probe[..filled]));
        }
        if got > filled {
            // The bytes this peek reported start at the same socket offset as the ones already
            // in the probe, so only the new tail has to be copied.
            probe[filled..got].copy_from_slice(&scratch[filled..got]);
            filled = got;
            waits = 0;
            if let Some(g) = decide_final(&probe[..filled]) {
                return Ok(g);
            }
            continue;
        }
        // Nothing new arrived. A client that sent a partial discriminator and then paused is
        // still worth waiting for — a 24-byte preface can be split by anything between the two
        // stacks — but only [`CLASSIFY_RETRY_LIMIT`] times, and then the prefix in hand decides.
        waits += 1;
        if waits > CLASSIFY_RETRY_LIMIT {
            return Ok(decide(&probe[..filled]));
        }
        tokio::time::sleep(CLASSIFY_RETRY_SLACK).await;
    }
}

/// How many times [`classify`] waits for a stalled discriminator before answering from the
/// bytes it has. Four waits of [`CLASSIFY_RETRY_SLACK`] cost a connection at most 100 ms, which
/// is well inside every `first_byte_timeout` the framework ships with; the bound exists because
/// a non-consuming peek is always ready, so waiting *without* a bound would burn a worker.
const CLASSIFY_RETRY_LIMIT: usize = 4;

/// The pause between two peeks of a connection that has not yet sent enough to decide.
const CLASSIFY_RETRY_SLACK: Duration = Duration::from_millis(25);

/// The earliest answer that cannot be revised by more bytes.
fn decide_final(buf: &[u8]) -> Option<Greeting> {
    if buf.len() >= 3 && buf[0] == 0x16 && buf[1] == 0x03 {
        return Some(Greeting::Tls);
    }
    if buf.len() >= 24 {
        return Some(if buf[..24] == *H2_PREFACE {
            Greeting::H2Preface
        } else {
            decide(buf)
        });
    }
    // A first byte that cannot start a TLS record and is not `P` (the only preface start) is
    // already known not to be h2 or TLS.
    if !buf.is_empty() && buf[0] != 0x16 && buf[0] != b'P' {
        return Some(decide(buf));
    }
    None
}

/// The final verdict for a byte prefix. A prefix shorter than the discriminators can only be
/// HTTP/1: the alternatives need 3 and 24 bytes respectively to tell apart at all.
fn decide(buf: &[u8]) -> Greeting {
    if buf.is_empty() {
        return Greeting::Empty;
    }
    if buf.len() >= 3 && buf[0] == 0x16 && buf[1] == 0x03 {
        return Greeting::Tls;
    }
    if buf.len() >= H2_PREFACE.len() {
        return if buf[..H2_PREFACE.len()] == *H2_PREFACE {
            Greeting::H2Preface
        } else {
            Greeting::Http1
        };
    }
    Greeting::Http1
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// One connected pair on an ephemeral loopback port. A real socket is the only instrument
    /// that has `peek`: the classifier's whole subject is what a non-consuming read reports.
    async fn pair() -> (TcpStream, TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let client = TcpStream::connect(addr).await.expect("connect");
        let (server, _) = listener.accept().await.expect("accept");
        (client, server)
    }

    /// A preface that arrives in two segments is still a preface.
    ///
    /// This is the shape the classifier used to get wrong: a second `peek` re-reports the bytes
    /// the first one already put in the probe, because MSG_PEEK does not advance the socket. The
    /// probe was then filled with a duplicated head, 24 bytes looked nothing like the preface,
    /// and a legitimate h2c client was handed to the HTTP/1 parser — which answers `PRI` with a
    /// 400 on a socket the client believes is HTTP/2.
    ///
    /// The classifier has to run *while* the preface is still arriving, so it is spawned and the
    /// rest of the preface is written only after checking that no answer exists yet. Writing both
    /// halves and then classifying proves nothing: the first peek would see all 24 bytes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_preface_arriving_in_two_segments_is_still_recognised_as_http2() {
        let (mut client, server) = pair().await;
        client
            .write_all(&H2_PREFACE[..1])
            .await
            .expect("write the first preface byte");

        let (tx, mut rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(classify(&server).await);
        });
        // Past the first peek, inside the wait budget: 25 ms x 4 retries.
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            rx.try_recv().is_err(),
            "the classifier settled before the rest of the preface arrived: either it answered \
             from a probe it filled without waiting, or the split this test exists to make did \
             not happen, and either way the assertion below would be measuring one segment"
        );

        client
            .write_all(&H2_PREFACE[1..])
            .await
            .expect("write the rest");
        let greeting = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("classification never settled")
            .expect("classifier task died")
            .expect("peek");
        assert_eq!(
            greeting,
            Greeting::H2Preface,
            "a preface split across segments must still be HTTP/2"
        );
    }

    /// The classifier waits for a stalled discriminator, then answers — it does not loop.
    ///
    /// A peek on a socket that still holds a byte is always ready, so a wait without a bound is
    /// a hot loop on one worker. The bound is what this asserts, by finishing: `timeout` runs on
    /// another worker, so a spinning classifier would be pre-empted by the deadline instead of
    /// by the runtime.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_client_that_pauses_mid_discriminator_is_answered_from_what_it_sent() {
        let (client, server) = pair().await;
        let mut client = client;
        client.write_all(b"P").await.expect("write one byte");

        let started = std::time::Instant::now();
        let greeting = tokio::time::timeout(Duration::from_secs(2), classify(&server))
            .await
            .expect("a stalled discriminator must fall back, not spin")
            .expect("peek");
        let elapsed = started.elapsed();

        assert_eq!(
            greeting,
            Greeting::Http1,
            "one `P` cannot be told from a method token"
        );
        assert!(
            elapsed < Duration::from_millis(900),
            "the fallback is bounded by the retry budget, not by the test: {elapsed:?}"
        );
        // The socket is dropped with the client still connected; nothing was consumed.
    }

    /// Classifying consumes nothing, so the protocol that was picked still sees every byte.
    ///
    /// Without this, the loop above could be rewritten as a `read` and every assertion here
    /// would still pass while the HTTP/1 parser lost its request line and rustls lost its
    /// ClientHello. The read is timed because a classifier that ate the bytes would otherwise
    /// leave this test waiting on a socket that can never deliver them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn classifying_leaves_the_bytes_for_the_protocol_that_uses_them() {
        let (mut client, mut server) = pair().await;
        client
            .write_all(H2_PREFACE.as_slice())
            .await
            .expect("write the preface");

        assert_eq!(classify(&server).await.expect("peek"), Greeting::H2Preface);
        let mut carried = [0u8; 24];
        tokio::time::timeout(Duration::from_secs(5), server.read_exact(&mut carried))
            .await
            .expect("the bytes classifying left behind never arrived")
            .expect("read after classifying");
        assert_eq!(
            &carried,
            H2_PREFACE.as_slice(),
            "the preface must still be in the socket"
        );
    }
}
