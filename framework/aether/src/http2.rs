//! The HTTP/2 frontend: `h2`'s stream machinery wired onto the same [`ServiceCore`] that
//! answers HTTP/1.
//!
//! Scope, stated exactly: HTTP/2 is reachable **over TLS with ALPN `h2`**, and over plaintext
//! only when the operator opts in with [`crate::app::AetherApp::enable_h2c`] (RFC 9113 §3.3
//! *prior knowledge* — cleartext HTTP/2 has no negotiation, so nothing but that flag tells this
//! server the client meant it). A WebSocket upgrade cannot arrive here on either path.
//!
//! Two facts about `h2` shape the whole design:
//!
//! * `server::Connection` is neither `Clone` nor a `Future`. `accept()` *is* the driver:
//!   operating on a `SendStream` or `RecvStream` while nobody polls the connection makes no
//!   progress at all. So this loop must never await a handler — every stream goes to its own
//!   task and the loop returns to `accept()` immediately.
//! * The connection must keep being polled until it reports closed, and only then be dropped.
//!   Shutdown therefore starts with `graceful_shutdown()` (a GOAWAY), keeps accepting and
//!   polling while in-flight streams finish, and releases the connection afterwards.
//!
//! What is deliberately left to `h2`: the stream state machine, HPACK, framing, flow-control
//! accounting, PING and SETTINGS. What is ours: the translation. The pseudo-header view `h2`
//! hands over becomes the same `RequestParts` HTTP/1 produces, connection-specific fields never
//! reach the wire, and both frontends answer one request identically because they call one
//! implementation.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use h2::RecvStream;
use h2::server::SendResponse;
use http::{HeaderName, HeaderValue, StatusCode};

use aether_http::limits::ServerLimits;
use aether_http::method::Method;
use aether_http::parser::{Expectation, classify_expectation};
use aether_http::response::Response as AetherResponse;
use aether_http::response::{
    BodyFraming, BodyStream, canonical_reason, http_date_now, sanitize_field_value,
    status_forbids_body,
};
use aether_scheduler::JobId;

use crate::app::{HeaderLines, RequestParts};
use crate::request_body::{BodyError, BodyWriter};
use crate::serve::{Head, PeerWatch, ServeOutcome, ServiceCore, Version};
use crate::transport::Stream;

/// What one live HTTP/2 stream reserves against the scheduler's memory budget before it has
/// buffered anything: the `h2` stream record, its flow-control accounting, and the cost of a
/// request head and a response head.
///
/// Deliberately not the largest body the stream could reach. Body bytes are charged for the
/// block that holds them ([`StreamCharge::buffer_set`]): the request as it arrives, the response
/// when the stream is handed the body ([`price_response_body`]). That is the rule HTTP/1 already
/// applies to an idle keep-alive socket: a connection pays for what it holds, not for what it
/// might be asked to hold.
const STREAM_BASE_MEMORY: u64 = 16 * 1024;

/// Field names HTTP/2 does not allow on the wire. RFC 9113 §8.2.2 forbids connection-specific
/// fields in both directions, and the two directions are enforced by different code.
///
/// Outbound: this list drops them, because `h2` answers one with a `malformed headers` user error
/// that resets the stream — an HTTP/1 handler that legitimately put `Connection: close` on a 504
/// must not destroy the same response over h2 by doing it again.
///
/// Inbound needs no check here: `h2` rejects the field before the handler ever sees the head.
/// Proven with a HEADERS block serialised by hpack and framed by hand, which is the only way to
/// get it onto the wire at all (python-h2 silently drops these names even with outbound validation
/// off, so a client library reports a false 200). `connection`, `keep-alive`, `transfer-encoding`,
/// `upgrade` and a non-`trailers` `te` each came back as RST_STREAM with no HEADERS frame, while
/// `te: trailers` reached the handler untouched.
const FORBIDDEN_FIELDS: [&str; 6] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "transfer-encoding",
    "upgrade",
];

/// Serve one HTTP/2 connection until peer or server is done with it.
///
/// `conn_job` is the scheduler registration of the *connection*. Each accepted stream re-prices
/// it with what that stream actually holds, the way the HTTP/1 loop re-prices its read buffer.
/// The connection's guard itself stays owned by the caller, which is the only thing that can
/// say whether the job completed or was cancelled.
pub(crate) async fn serve(
    stream: Stream,
    core: Arc<ServiceCore>,
    peer: SocketAddr,
    conn_job: Option<JobId>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let limits = Arc::clone(&core.limits);
    // The handshake reads the 24-byte preface and the peer's first SETTINGS frame. Bounding it
    // with the same budget the HTTP/1 loop gives a connection that has not spoken yet is what
    // stops a client that finishes the transport setup — TLS and ALPN, or a plaintext preface on
    // an h2c port — and then says nothing from holding this task, its permit and its scheduler
    // registration forever. Dropping the future here is safe by construction: the stream goes
    // with it and the connection is over, so no half-consumed framing is reused.
    let mut conn = match tokio::time::timeout(
        limits.read_timeout,
        builder(&limits).handshake::<Stream, Bytes>(stream),
    )
    .await
    {
        Ok(Ok(conn)) => conn,
        // The preface never arrived, or the peer is not speaking HTTP/2. Nothing was served and
        // there is no framing left to answer; the socket goes with the handshake.
        Ok(Err(_)) | Err(_) => return,
    };

    let mut streams: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let mut served_any = false;
    let mut draining = false;
    // Fixed when draining starts, so a stream that keeps the loop busy cannot extend the budget.
    let mut drain_deadline: Option<Instant> = None;

    loop {
        while streams.try_join_next().is_some() {}

        if !draining && *shutdown.borrow_and_update() {
            conn.graceful_shutdown();
            draining = true;
            drain_deadline = Some(Instant::now() + drain_budget(&limits));
        }

        // A connection with a stream in flight is not idle, even when the client sends nothing:
        // that stream's task may be waiting for send capacity, and `accept()` is what polls the
        // connection that grants it. The idle timers below are therefore only for a connection
        // whose stream tasks have all finished — a response that keeps making progress may
        // outlive `keep_alive_timeout`, and one that stops making it is bounded by
        // `write_timeout` on the stream instead.
        let accepted = if !streams.is_empty() || draining {
            let drain_at = drain_deadline.unwrap_or_else(Instant::now);
            tokio::select! {
                biased;
                _ = shutdown.changed(), if !draining => {
                    conn.graceful_shutdown();
                    draining = true;
                    drain_deadline = Some(Instant::now() + drain_budget(&limits));
                    continue;
                }
                // Reap a finished stream so the idle timers resume the moment the last one ends.
                // `JoinSet::join_next` is cancel safe; the `Some(_)` pattern disables the branch,
                // rather than spinning, when the set is empty.
                Some(_) = streams.join_next() => continue,
                // The budget is absolute: an in-flight stream accepted during shutdown does not
                // buy the connection more time.
                _ = tokio::time::sleep_until(drain_at.into()), if draining => break,
                got = conn.accept() => got,
            }
        } else {
            // Before the first request this connection is a slowloris candidate and
            // `read_timeout` bounds it; between keep-alive requests it is an idle socket on
            // `keep_alive_timeout`.
            let wait = if served_any {
                limits.keep_alive_timeout
            } else {
                limits.read_timeout
            };
            tokio::select! {
                biased;
                _ = shutdown.changed(), if !draining => {
                    conn.graceful_shutdown();
                    draining = true;
                    drain_deadline = Some(Instant::now() + drain_budget(&limits));
                    continue;
                }
                got = tokio::time::timeout(wait, conn.accept()) => match got {
                    Ok(got) => got,
                    // Nobody answered in the whole idle window.
                    Err(_) => break,
                },
            }
        };

        match accepted {
            // The peer closed or the connection faulted: there is nothing further to drive.
            None | Some(Err(_)) => break,
            Some(Ok((request, mut response))) => {
                served_any = true;
                let core = Arc::clone(&core);
                streams.spawn(async move {
                    let mut charge = Charge::acquire(&core, conn_job);
                    if let Charge::Refused = charge {
                        // The budget will not cover a stream. Refusing it with a protocol-valid
                        // 503 is the whole point of accounting: the connection, its already
                        // running streams, and the handler slots they hold stay untouched.
                        send(
                            &mut response,
                            Method::Get,
                            over_budget(),
                            core.limits.write_timeout,
                            None,
                        )
                        .await;
                        return;
                    }
                    serve_request(core, peer, request, response, &mut charge).await;
                });
            }
        }
    }

    // In-flight streams get what is left of the drain budget — or a fresh one when the loop ended
    // for another reason; stragglers are aborted. Their `StreamCharge` releases on drop either
    // way, so the budget never keeps accounting for work that stopped running.
    let budget = match drain_deadline {
        Some(deadline) => deadline.saturating_duration_since(Instant::now()),
        None => drain_budget(&limits),
    };
    if tokio::time::timeout(budget, async {
        while streams.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        streams.shutdown().await;
    }
}

/// How long a draining connection may take, mirroring the listener's own bound.
fn drain_budget(limits: &ServerLimits) -> Duration {
    limits.keep_alive_timeout.min(Duration::from_secs(30))
}

/// The settings this server advertises. Each maps a limit the framework already enforces on
/// HTTP/1 onto the protocol mechanism that enforces it for nothing over HTTP/2.
fn builder(limits: &ServerLimits) -> h2::server::Builder {
    let mut b = h2::server::Builder::new();
    // RFC 9113 §6.5.2: the peer must not open more streams than this, and `h2` resets the
    // overflow with REFUSED_STREAM before any task or buffer exists. The per-stream memory
    // re-pricing is the second bound, the one that reacts to what streams actually hold.
    b.max_concurrent_streams(limits.max_concurrent_streams);
    // The header byte budget, advertised *and* used as the decoder's cap: an oversized header
    // list earns a 431 from `h2` itself, before this module sees the request.
    b.max_header_list_size(header_list_cap(limits.max_header_bytes));
    b
}

/// The header budget as the `u32` SETTINGS field HTTP/2 needs, saturated rather than wrapped.
///
/// A cast would take `AETHER_MAX_HEADER_BYTES=4294967296` to `0`, and `h2` reads a zero cap as
/// "no header list is acceptable": the HTTP/1 frontend honours the configured value while every
/// h2 request is refused, on the same listener and the same config. Above the ceiling the only
/// honest cap is the ceiling itself.
fn header_list_cap(max_header_bytes: usize) -> u32 {
    max_header_bytes.min(u32::MAX as usize) as u32
}

/// Turns a peer's `RST_STREAM` into the core's disconnect signal.
///
/// `SendResponse::poll_reset` is only valid before the response head is sent, which is exactly
/// the window a handler runs in. A reset — or an error the reset is reported as once the
/// connection is dead — is the HTTP/2 form of a client walking away: the stream can never be
/// answered, so the handler has nobody left to answer.
struct ResetWatch<'a> {
    response: &'a mut SendResponse<Bytes>,
    settled: bool,
}

impl crate::serve::PeerWatch for ResetWatch<'_> {
    fn poll_gone(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.settled {
            return Poll::Pending;
        }
        match self.response.poll_reset(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => {
                self.settled = true;
                Poll::Ready(())
            }
        }
    }
}

/// Route, dispatch and answer one HTTP/2 stream.
///
/// Both frontends answer one request identically because they call one implementation.
async fn serve_request(
    core: Arc<ServiceCore>,
    peer: SocketAddr,
    request: http::Request<RecvStream>,
    mut response: SendResponse<Bytes>,
    charge: &mut Charge,
) {
    let limits = Arc::clone(&core.limits);
    let (parts, mut body) = request.into_parts();

    let Some(method) = Method::from_bytes(parts.method.as_ref().as_bytes()) else {
        // The HTTP/1 request line rejects an unknown method token the same way, and the router
        // cannot hold a route for a method the framework cannot name.
        send(
            &mut response,
            Method::Get,
            AetherResponse::status(400, "Bad Request"),
            limits.write_timeout,
            None,
        )
        .await;
        return;
    };

    // RFC 9113 §8.3.1 gives `:path` the origin form plus two named shapes: the asterisk, which is
    // how a server-wide OPTIONS arrives over HTTP/2, and nothing at all for a CONNECT. A `*`
    // carried by any other method is the same out-of-set request-target the HTTP/1 parser refuses
    // with 400 (RFC 9112 §3.2.4), but it reaches here as a perfectly framed path string, so only
    // this frontend can tell it apart. RFC 9113 §8.1.1 allows a malformed stream to be reset or
    // answered first; answering is the honest branch, since the peer is waiting for a response and
    // a reset code tells it nothing about why.
    if parts.uri.path() == "*" && method != Method::Options {
        send(
            &mut response,
            method,
            AetherResponse::status(400, "Bad Request"),
            limits.write_timeout,
            None,
        )
        .await;
        return;
    }

    // The field-line ceiling is decided before anything else that reads the head, for the two
    // reasons the HTTP/1 parser decides it there. A head this server will not even hold must not be
    // answered with a verdict about a promise it never reached: the same 9-line head carrying
    // `Expect: foo=bar` answers 431 over HTTP/1, and an ordering that reached 417 first would make
    // one request mean two things depending on which frontend it arrived on. It must also not make
    // the client send a body just to learn the answer, so this sits above the body read below.
    let Some(header_multi) = header_store(&parts, &limits) else {
        // More field lines than `max_headers_count`. HTTP/1 answers this with 431, not 400.
        send(
            &mut response,
            method,
            AetherResponse::status(431, "Request Header Fields Too Large"),
            limits.write_timeout,
            None,
        )
        .await;
        return;
    };

    // RFC 9110 §10.1.1 puts `Expect` in the message semantics, not the connection ones, and
    // RFC 9113 §8.2.3 allows an informational response over HTTP/2, so this frontend owes both
    // answers too. The field is read with the same classifier the HTTP/1 parser uses, so one list
    // of expectation tokens cannot mean two things depending on which socket it arrived on.
    let expectation = classify_expectation(
        parts
            .headers
            .get_all("expect")
            .iter()
            .map(|value| value.as_bytes()),
    );
    if expectation == Expectation::Unsupported {
        // Refused from the head, before a single DATA frame is requested: the point of the
        // expectation is that a client may not have to send the body, and dispatching a handler
        // would make the promise moot.
        send(
            &mut response,
            method,
            AetherResponse::status(417, canonical_reason(417)),
            limits.write_timeout,
            None,
        )
        .await;
        return;
    }
    // A stream that ended with its HEADERS frame has no body waiting on the answer, so the interim
    // would be noise. `send_informational` fails on a stream the peer has already reset; that
    // stream has no response to protect, and the final answer below is what a live stream gets.
    if expectation == Expectation::Continue
        && !body.is_end_stream()
        && let Ok(interim) = http::Response::builder()
            .status(StatusCode::CONTINUE)
            .body(())
    {
        let _ = response.send_informational(interim);
    }

    // The route decides how the body is read, and that decision is taken from the head alone: a
    // `route_stream` route is dispatched before its first DATA frame arrives, exactly as the
    // HTTP/1 loop dispatches it. Every other route keeps the buffered path below unchanged.
    if core.streams_route(method, parts.uri.path()) {
        serve_streamed(
            &core,
            peer,
            method,
            parts,
            &mut body,
            &mut response,
            charge,
            &limits,
        )
        .await;
        return;
    }

    // A stream whose body never finishes is the h2 form of a trickling upload: bounded by the
    // same receive timeout that bounds the HTTP/1 receive phase.
    let deadline = Instant::now() + limits.receive_timeout;
    let mut stream_charge = match charge {
        Charge::Held(c) => Some(c),
        _ => None,
    };
    // A CONNECT is answered from the head here too. RFC 9113 §8.5 gives a DATA frame ending the
    // stream the meaning of a TCP FIN, which a client only sends once the tunnel exists, so a
    // conformant CONNECT client leaves the stream half-open — and this server has no tunnel to
    // agree to. Reading a body first would hold the refusal behind the receive deadline.
    let body_bytes = if method == Method::Connect {
        Bytes::new()
    } else {
        match read_body(&mut body, &limits, deadline, stream_charge.as_deref_mut()).await {
            Ok(bytes) => bytes,
            Err(refusal) => {
                send(
                    &mut response,
                    method,
                    refusal.into_response(),
                    limits.write_timeout,
                    None,
                )
                .await;
                return;
            }
        }
    };

    let path = parts.uri.path().to_string();
    let query_str = parts.uri.query().map(str::to_string);
    let raw_uri = match &query_str {
        Some(q) => format!("{path}?{q}"),
        None => path.clone(),
    };

    let (request_serial, request_id) = crate::app::next_request_id();
    let mut watch = ResetWatch {
        response: &mut response,
        settled: false,
    };
    let outcome = core
        .serve(
            Head {
                parts: RequestParts {
                    raw_uri,
                    path,
                    query_str,
                    method,
                    param_map: HashMap::new(),
                    header_multi,
                    body: body_bytes,
                    body_stream: None,
                    request_id,
                    request_serial,
                    peer_addr: Some(peer),
                    extensions: crate::extensions::Extensions::new(),
                },
                version: Version::Http2,
                peer,
                // RFC 9113 §8.1: HTTP/2 has no Upgrade mechanism, so the core answers a WebSocket
                // route with 400 instead of emitting a 101 that would mis-frame the stream.
                allows_upgrade: false,
            },
            &mut watch,
        )
        .await;

    match outcome {
        // The peer reset this stream while the handler ran: the handler was aborted with it,
        // and the stream can never be answered.
        ServeOutcome::ClientGone => {}
        ServeOutcome::Response { response: out, .. } => {
            send(
                &mut response,
                method,
                out,
                limits.write_timeout,
                stream_charge,
            )
            .await
        }
        ServeOutcome::WebSocketUpgrade { .. } => {
            // Unreachable by construction: `allows_upgrade: false` makes the core produce a 400
            // response. Landing here would mean the core changed underneath this frontend, and a
            // raw-stream handover onto a multiplexed stream is not something that can be
            // attempted half-heartedly.
            send(
                &mut response,
                method,
                AetherResponse::status(500, "Internal Server Error"),
                limits.write_timeout,
                None,
            )
            .await;
        }
    }
}

/// Serve one HTTP/2 stream whose route reads its body as it arrives.
///
/// The head decides everything: the route (already resolved by the caller), the declared length,
/// and the lane. The body is never collected — the handler runs against a lane that dispatch pumps
/// while it waits for it, so the resident set is one DATA frame plus whatever `h2` holds for this
/// stream inside its receive window, not the body.
///
/// A body the handler did not read to its end is *not* a connection problem on HTTP/2, unlike
/// HTTP/1: the stream carries its own framing, so the connection keeps serving. The tail is
/// abandoned by dropping the `RecvStream` when this returns — `h2` then ignores DATA for a stream
/// nobody reads and returns its capacity to the connection. RFC 9113 §8.1's `RST_STREAM(NO_ERROR)`
/// is the textbook way to say that, and this frontend deliberately does not send it: see the note
/// at the send site, where the measurement is recorded.
#[expect(
    clippy::too_many_arguments,
    reason = "the frontend's whole state for one stream: core, peer, head, body, response, budget, limits"
)]
async fn serve_streamed(
    core: &Arc<ServiceCore>,
    peer: SocketAddr,
    method: Method,
    parts: http::request::Parts,
    body: &mut RecvStream,
    response: &mut SendResponse<Bytes>,
    charge: &mut Charge,
    limits: &ServerLimits,
) {
    let Some(header_multi) = header_store(&parts, limits) else {
        send(
            response,
            method,
            AetherResponse::status(431, "Request Header Fields Too Large"),
            limits.write_timeout,
            None,
        )
        .await;
        return;
    };

    // A declared length the transfer ceiling forbids is answered *before* dispatch, the same rule
    // HTTP/1 applies: the handler exists for bodies that size, and saying so afterwards would mean
    // having already paid for a body the rule never allows.
    if declared_length(&parts.headers).is_some_and(|len| len > limits.max_stream_body_bytes as u64)
    {
        send(
            response,
            method,
            AetherResponse::status(413, canonical_reason(413)),
            limits.write_timeout,
            None,
        )
        .await;
        return;
    }

    let path = parts.uri.path().to_string();
    let query_str = parts.uri.query().map(str::to_string);
    let raw_uri = match &query_str {
        Some(q) => format!("{path}?{q}"),
        None => path.clone(),
    };
    let deadline = Instant::now() + limits.receive_timeout;
    let (request_serial, request_id) = crate::app::next_request_id();

    let (reader, writer) = crate::request_body::lane();
    // Re-borrowed, not moved: the same registration prices the response once the pump is gone.
    let mut stream_charge = match charge {
        Charge::Held(c) => Some(c),
        _ => None,
    };
    let mut pump = Http2Body::new(
        &mut *body,
        &mut *response,
        writer,
        deadline,
        limits.max_stream_body_bytes as u64,
        stream_charge.as_deref_mut(),
    );
    let outcome = core
        .serve(
            Head {
                parts: RequestParts {
                    raw_uri,
                    path,
                    query_str,
                    method,
                    param_map: HashMap::new(),
                    header_multi,
                    // A streamed route reads the lane; the buffered copy is empty by contract.
                    body: Bytes::new(),
                    body_stream: Some(reader),
                    request_id,
                    request_serial,
                    peer_addr: Some(peer),
                    extensions: crate::extensions::Extensions::new(),
                },
                version: Version::Http2,
                peer,
                allows_upgrade: false,
            },
            &mut pump,
        )
        .await;
    // The pump borrows the stream and the response; both are needed below.
    drop(pump);

    let out = match outcome {
        // The peer reset this stream while the handler ran: the handler was aborted with it.
        ServeOutcome::ClientGone => return,
        ServeOutcome::Response { response: out, .. } => out,
        // Unreachable by construction, as on the buffered path: `allows_upgrade: false` makes the
        // core answer 400 for a WebSocket route.
        ServeOutcome::WebSocketUpgrade { .. } => {
            AetherResponse::status(500, "Internal Server Error")
        }
    };
    // A body that never reached END_STREAM is abandoned when this returns, exactly as the buffered
    // refusal abandons one: dropping the `RecvStream` makes `h2` ignore DATA for a stream nobody
    // reads while handing its capacity back to the connection, so an unused tail costs neither a
    // drain nor the connection's receive window.
    //
    // `RST_STREAM(NO_ERROR)` — RFC 9113 §8.1's own way to end an answered request — is *not* sent,
    // and that is a measured decision: `h2` implements a reset by dropping the stream's queued
    // frames, so a reset issued just after the response is queued destroys the response it was
    // meant to follow. Reintroducing it turned every early answer here into a client-side `Reset`
    // error (`an_unread_h2_body_answers_and_the_connection_stays_usable`).
    send(response, method, out, limits.write_timeout, stream_charge).await;
}

/// The payload length the head promised, when it named one.
///
/// RFC 9113 §8.1.1 makes a `content-length` that does not equal the sum of the stream's DATA frames
/// a malformed request, and the enforcement of that — including here, where the value is read — is
/// the dependency's, not this frontend's: `h2`'s receive path parses the field as digits only (no
/// padding, no overflow), refuses a repeated field whose lines disagree, refuses `END_STREAM` on
/// the head while a non-zero length is still owed, and decrements the promise for every DATA frame
/// so that `END_STREAM` must find it at zero; each of those is a `RST_STREAM(PROTOCOL_ERROR)` on
/// that stream, never an answer. The eight shapes in `http2_e2e.rs`
/// (`a_body_longer_than_its_declared_length_is_reset_over_h2` and its siblings) pin that from
/// bytes, because this function's safety rests on it: it takes the *first* line, which is the only
/// line a request with several can have once the library is done with it.
///
/// What is left for this frontend is the promise read as a budget, i.e. the ceiling answer that
/// goes out *before* dispatch, and a value that is not a length simply names no ceiling — the body
/// is then bounded by the ceiling as it arrives instead.
fn declared_length(headers: &http::HeaderMap) -> Option<u64> {
    headers
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The HTTP/2 pump behind a streamed request body.
///
/// Dispatch polls this while the handler runs, in place of watching the stream for a reset alone.
/// Each poll hands the lane at most one DATA frame and stops the moment the lane is full, so a
/// handler that stops reading stops crediting the peer: `h2`'s stream window — not a queue here —
/// is the backpressure, and memory is one frame plus that window, never the body.
///
/// The frame is charged to the stream's registration for exactly as long as this task holds it,
/// then the lane charges it for as long as it holds it, then the next frame replaces the
/// registration. `h2`'s own receive buffer is outside what this stream can name; it is bounded by
/// the flow-control window, which only moves when this pump credits bytes the handler consumed.
struct Http2Body<'a> {
    body: &'a mut RecvStream,
    response: &'a mut SendResponse<Bytes>,
    writer: BodyWriter,
    /// `max_stream_body_bytes`: the ceiling on the *whole* body, not on one frame.
    cap: u64,
    received: u64,
    deadline: Instant,
    charge: Option<&'a mut StreamCharge>,
    finished: bool,
    /// The stream or the connection failed while the body was arriving; dispatch cancels the
    /// handler, which has nobody left to answer.
    gone: bool,
    /// The peer's `RST_STREAM` has been observed, so the reset poll is never asked again.
    settled: bool,
}

impl<'a> Http2Body<'a> {
    fn new(
        body: &'a mut RecvStream,
        response: &'a mut SendResponse<Bytes>,
        writer: BodyWriter,
        deadline: Instant,
        cap: u64,
        charge: Option<&'a mut StreamCharge>,
    ) -> Self {
        Self {
            body,
            response,
            writer,
            cap,
            received: 0,
            deadline,
            charge,
            finished: false,
            gone: false,
            settled: false,
        }
    }

    /// Keep the stream's registration equal to what it now holds: the lane's chunk, if any.
    fn check_budget(&mut self) -> bool {
        match self.charge.as_mut() {
            Some(charge) => charge.buffer_set(self.writer.held() as u64),
            None => true,
        }
    }

    fn finish_clean(&mut self) {
        if !self.finished {
            self.finished = true;
            self.writer.finish();
        }
    }

    fn fail(&mut self, error: BodyError) {
        if !self.finished {
            self.finished = true;
            self.writer.fail(error);
        }
    }

    /// One step of the pump. `Ready` means the body reached its terminal state.
    fn step(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.finished {
            return Poll::Ready(());
        }
        loop {
            if self.writer.poll_room(cx).is_pending() {
                return Poll::Pending;
            }
            match self.body.poll_data(cx) {
                Poll::Pending => return Poll::Pending,
                // The peer ended the stream: `h2` delivers every DATA frame before this, so what
                // arrived is the whole body.
                Poll::Ready(None) => {
                    self.finish_clean();
                    return Poll::Ready(());
                }
                // The peer reset the stream, or the connection failed under it. The response can
                // no longer be watched from `poll_gone`, and the handler has nobody to answer.
                Poll::Ready(Some(Err(_))) => {
                    self.gone = true;
                    self.fail(BodyError::Aborted);
                    return Poll::Ready(());
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    let taken = chunk.len();
                    // An empty DATA frame carries no payload and no capacity to return.
                    if taken == 0 {
                        continue;
                    }
                    if self.received.saturating_add(taken as u64) > self.cap {
                        self.fail(BodyError::TooLarge);
                        return Poll::Ready(());
                    }
                    self.received += taken as u64;
                    // Crediting the peer is the application's job on HTTP/2 as well: without this
                    // the client stalls one window into an upload, which is the defect
                    // `read_body_buffer` records for the buffered path.
                    if self.body.flow_control().release_capacity(taken).is_err() {
                        self.gone = true;
                        self.fail(BodyError::Aborted);
                        return Poll::Ready(());
                    }
                    self.writer.push(chunk);
                    if !self.check_budget() {
                        self.fail(BodyError::TooLarge);
                        return Poll::Ready(());
                    }
                    // The handler may already have taken the previous chunk, so the lane is free
                    // again; the loop asks for room before the next frame.
                }
            }
        }
    }
}

impl PeerWatch for Http2Body<'_> {
    fn poll_gone(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.gone {
            return Poll::Ready(());
        }
        if self.settled {
            return Poll::Pending;
        }
        match self.response.poll_reset(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => {
                self.settled = true;
                Poll::Ready(())
            }
        }
    }

    fn poll_body(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.step(cx)
    }

    fn body_pending(&self) -> bool {
        !self.finished
    }

    fn body_deadline(&self) -> Option<Instant> {
        (!self.finished).then_some(self.deadline)
    }

    fn expire_body(&mut self) {
        self.fail(BodyError::Timeout);
    }
}

/// Why a request body was not accepted.
#[derive(Debug)]
enum Refusal {
    /// More body than `max_body_bytes`, or more than the memory budget would cover.
    TooLarge,
    /// The body stopped arriving inside `receive_timeout`, or the peer reset the stream.
    Stalled,
}

impl Refusal {
    fn into_response(self) -> AetherResponse {
        match self {
            Refusal::TooLarge => AetherResponse::status(413, canonical_reason(413)),
            Refusal::Stalled => AetherResponse::status(408, "Request Timeout"),
        }
    }
}

/// Read the request body, bounded by the framework's body budget.
///
/// A refusal abandons the stream instead of draining what is left of it: `h2` ends the stream on
/// its side, and the response the caller sends next belongs to that same stream only. That the
/// connection keeps serving afterwards is not assumed here — it is asserted, by
/// `a_body_over_the_budget_is_413_and_the_connection_still_works` and
/// `a_body_over_the_limit_is_413_when_the_limit_is_crossed_mid_stream`.
///
/// A refusal also takes the refused buffer off the stream's registration, since the allocation
/// goes with it; a read that succeeds leaves its block charged for as long as the returned bytes
/// live (`a_refused_upload_stops_registering_the_buffer_it_dropped` pins both halves).
async fn read_body(
    body: &mut RecvStream,
    limits: &ServerLimits,
    deadline: Instant,
    mut charge: Option<&mut StreamCharge>,
) -> Result<Bytes, Refusal> {
    let read = read_body_buffer(body, limits, deadline, charge.as_deref_mut()).await;
    if read.is_err() {
        // Every refusal abandons the buffer that was being filled, and the allocation behind it
        // dies with the local. Re-price the stream down to its base here, at the point the block
        // goes away, rather than leaving the error answer to be written — and the next admission
        // decision to be taken — against memory this stream no longer holds. The answers on this
        // path carry no payload, so today the stale figure lives for one queued head; the rule it
        // breaks is the one every other charge here follows, and an error body would make it real.
        if let Some(charge) = charge {
            let _ = charge.buffer_set(0);
        }
    }
    read
}

/// The receive loop behind [`read_body`]: fill a buffer until the stream ends or a bound refuses
/// it. Split from the re-pricing so the invariant has exactly one exit.
async fn read_body_buffer(
    body: &mut RecvStream,
    limits: &ServerLimits,
    deadline: Instant,
    charge: Option<&mut StreamCharge>,
) -> Result<Bytes, Refusal> {
    let mut buf = BytesMut::new();
    let mut charge = charge;
    loop {
        let chunk = match deadline.checked_duration_since(Instant::now()) {
            None => return Err(Refusal::Stalled),
            Some(remaining) => match tokio::time::timeout(remaining, body.data()).await {
                Err(_elapsed) => return Err(Refusal::Stalled),
                Ok(None) => break,
                // The peer reset the stream, or a frame of theirs was invalid. The response is
                // already unwatchable, but the body it sent is complete as far as we can tell.
                Ok(Some(Err(_))) => return Err(Refusal::Stalled),
                Ok(Some(Ok(bytes))) => bytes,
            },
        };
        if buf.len() + chunk.len() > limits.max_body_bytes {
            return Err(Refusal::TooLarge);
        }
        let taken = chunk.len();
        buf.extend_from_slice(&chunk);
        drop(chunk);
        // Releasing receive capacity is the application's job: `RecvStream::drop` clears the
        // buffer, it does not credit the peer. A server that never calls this reads exactly one
        // receive window (64 KiB by default) and then waits forever for bytes the client is no
        // longer allowed to send — which is why an upload larger than the window used to arrive as
        // a 408 at `receive_timeout` instead of being served or refused on its size.
        if body.flow_control().release_capacity(taken).is_err() {
            return Err(Refusal::Stalled);
        }
        // Charge the block this stream now reserves, the way the HTTP/1 loop re-prices its
        // socket buffer. A budget that cannot cover it sheds this stream, never the server.
        if let Some(c) = charge.as_mut()
            && !c.buffer_set(buf.capacity() as u64)
        {
            return Err(Refusal::TooLarge);
        }
    }
    Ok(buf.freeze())
}

/// Build the two header stores the core reads, from the header block `h2` decoded.
///
/// The rules are copied from the HTTP/1 path on purpose: ASCII-lowercase keys in the multi
/// store, first line wins in the single store, so a handler and a security decision cannot see
/// different header sets depending on which protocol arrived on the wire.
///
/// One job happens here that HTTP/1 does not need: HTTP/2 carries the target in the `:authority`
/// pseudo-header, which `http` folds into the URI rather than the header map. A server that left
/// `host` unset would fail its own `HostValidation` and answer a request that was perfectly formed.
///
/// A client that sends both a `:authority` and a `Host` is not rejected either. RFC 9113 §8.3.1
/// requires the recipient to reach the target through the `:authority` ("MUST NOT use the Host
/// header field to determine the target URI if `:authority` is present") and only *SHOULD* treat a
/// differing `Host` as malformed; replacing the field satisfies the MUST, matches what the HTTP/1
/// path does with an absolute-form target (RFC 9112 §3.2.2), and stays usable behind a proxy,
/// which the same section permits to "retain any Host header field" while rebuilding the
/// authority. One request must not resolve two hosts depending on which protocol carried it.
///
/// `None` means more field lines than `max_headers_count`.
fn header_store(
    parts: &http::request::Parts,
    limits: &ServerLimits,
) -> Option<HashMap<String, HeaderLines>> {
    let mut header_multi: HashMap<String, HeaderLines> =
        HashMap::with_capacity(parts.headers.len());

    let mut lines = 0usize;
    // `HeaderMap::iter` yields one item per value, so repeated lines are counted individually —
    // the per-line work bound `max_headers_count` is meant to be.
    for (name, value) in parts.headers.iter() {
        lines += 1;
        if lines > limits.max_headers_count {
            return None;
        }
        let key = name.as_str().to_ascii_lowercase();
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        header_multi.entry(key).or_default().push(value);
    }

    if let Some(authority) = parts.uri.authority().filter(|a| !a.as_str().is_empty()) {
        // `insert` rather than "only when absent": the authority replaces whatever `Host` lines the
        // client sent, including duplicates, so exactly one host survives and it is the routed one.
        let mut lines: HeaderLines = Default::default();
        lines.push(authority.as_str().to_string());
        header_multi.insert("host".to_string(), lines);
    }

    Some(header_multi)
}

/// Most `h2` is asked to reserve per capacity wait, and the most handed to one `send_data` call.
/// This bounds our step, not the buffer: `h2`'s own `max_send_buffer_size` (400 KiB on the server
/// default) is what bounds one stream's unflushed data.
const WRITE_CHUNK: usize = 64 * 1024;

/// The least payload one write interval must move to renew that interval's deadline.
///
/// Renewal on *any* progress is what made a stream's lifetime unbounded: a peer that grants one
/// byte of flow-control credit before each expiry moves one byte, renews the deadline, and keeps
/// the stream, its memory registration and its `h2` send-buffer leftover alive for as long as it
/// likes, at the cost of one credit per interval. Requiring a minimum turns the same timer into a
/// rate floor — `MIN_PROGRESS_FOR_RENEWAL / write_timeout` of consumed payload — without becoming
/// the absolute per-response cap that would truncate legitimate long downloads. At the default
/// `max_response_time` of 10 s the floor is about 102 bytes per second, and every figure here is
/// the interval bound, not a throughput claim.
const MIN_PROGRESS_FOR_RENEWAL: usize = 1024;

/// Write the core's response onto the stream.
///
/// Returns quietly on any `h2` error: a stream the peer reset, or a connection already closed, is
/// a response nobody is waiting for.
///
/// The body is paced by the peer's send capacity rather than handed to `h2` in one call. That is
/// what makes `write_timeout` meaningful here, and it diverges from HTTP/1 on purpose: HTTP/1
/// wraps the whole write in one `timeout(write_timeout, ...)`, a *total* cap, because one socket
/// carries one response. HTTP/2 multiplexes responses on one connection, so a total per-response
/// cap would cut the wrong unit. Here `write_timeout` is the longest interval the peer may leave
/// this stream with no send progress; the response may take any total time while every wait stays
/// inside the budget. "Progress" is not any progress: an interval renews the deadline only once it
/// has moved [`MIN_PROGRESS_FOR_RENEWAL`] bytes, so the same timer is also a floor on how slowly a
/// peer may consume. A wait that expires resets only this stream. One edge is outside the budget:
/// after the final `send_data` the flush belongs to `h2`, so the buffer's worth of leftover bytes
/// (at most `max_send_buffer_size`, 400 KiB by default) is bounded by the connection's idle timer,
/// not by `write_timeout`.
///
/// The body's block is registered against the connection before the response head is queued, for
/// as long as this stream's task holds it, and released with the stream's [`StreamCharge`]. The
/// slices `h2` may still hold after `write_body` returns are the leftover the paragraph above
/// bounds. A response that writes no payload — HEAD, or a status that forbids one — prices no
/// block: the bytes are dropped when this function returns instead of being held by the stream.
///
/// `charge` is `None` in two situations: this server has no scheduling layer at all, and the
/// framework answers before a handler body exists (the budget-refused 503, the unnameable-method
/// 400, the 413/408 body refusal, 431, the 500 upgrade answer). None of those answers carries a
/// payload, so there is no response block to price here; what each leaves registered is whatever
/// the stream already carried. The 503 and the 400 come before any body was read. The 413/408
/// follows a *refused* read, whose buffer [`read_body`] has already released from the
/// registration. The 431 and the 500 follow a read that *succeeded*, so the request block is
/// still held and still charged — the set-semantics approximation [`price_response_body`]
/// describes, not an over-registration.
async fn send(
    response: &mut SendResponse<Bytes>,
    method: Method,
    mut out: AetherResponse,
    write_timeout: Duration,
    mut charge: Option<&mut StreamCharge>,
) {
    let status = StatusCode::from_u16(out.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = http::Response::builder().status(status);

    for (name, value) in &out.headers {
        let lower = name.to_ascii_lowercase();
        // `content-length` is recomputed from the payload below; a connection-specific field
        // would make `h2` reset the whole stream instead of dropping the one header.
        if FORBIDDEN_FIELDS.contains(&lower.as_str()) || lower == "content-length" {
            continue;
        }
        let Some(name) = HeaderName::from_bytes(name.as_bytes()).ok() else {
            // A field name that is not a valid token cannot go on an HTTP/2 wire at all. The
            // HTTP/1 emitter would have written it raw and desynchronised the socket, so
            // dropping it is the strictly safer half of a divergence worth knowing about.
            continue;
        };
        // RFC 9110 §5.5 forbids CR, LF and NUL inside a field value. The HTTP/1 emitter strips
        // them on the way out; `HeaderValue` would reject the entire field for one byte.
        let sanitized = sanitize_field_value(value);
        let Ok(value) = HeaderValue::from_bytes(sanitized.as_bytes()) else {
            continue;
        };
        builder = builder.header(name, value);
    }

    let body_len = out.body.len();
    let streaming = out.is_streaming();
    // A streamed body's length was declared by its producer, if at all; every other response is
    // measured from `body`. HTTP/2 has no chunked coding to fall back on, so an undeclared stream
    // simply sends no `content-length` and ends with an empty DATA frame.
    let declared_len = if streaming {
        match out.body_framing(1) {
            BodyFraming::ContentLength(len) => Some(len),
            _ => None,
        }
    } else {
        None
    };
    // A status that forbids a body must drop the payload, not only its length — the same rule
    // the HTTP/1 emitter applies, and the reason `status_forbids_body` is shared.
    let forbids_body = status_forbids_body(out.status);
    // HEAD earns the GET's framing, Content-Length included, and no payload (RFC 9110 §9.3.2).
    let carries_payload = !forbids_body && method != Method::Head && (streaming || body_len > 0);
    // Taken before the body is moved out of `out`: a `HEAD` or a body-forbidding status drops the
    // source here, unread, exactly as the HTTP/1 writer never polls it.
    let stream = if streaming { out.take_stream() } else { None };

    let supplied_date = out
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("date"));
    let (mut head, _) = match builder.body(()) {
        Ok(res) => res.into_parts(),
        Err(_) => return,
    };
    if !supplied_date && let Ok(value) = HeaderValue::from_bytes(http_date_now().as_bytes()) {
        head.headers.insert("date", value);
    }
    if !forbids_body {
        // A stream declares its length or it does not: `body` is empty while the payload is still
        // to arrive, so measuring it here would promise the peer zero bytes and then send some.
        let len = if streaming {
            declared_len
        } else {
            Some(body_len as u64)
        };
        if let Some(len) = len {
            head.headers
                .insert("content-length", HeaderValue::from(len));
        }
    }

    // Priced before the head is queued, so whoever holds the response head can also see the
    // registration that covers the body it is about to receive. A stream has no whole body to
    // price yet: each chunk is priced while it is in hand, in `write_stream`.
    let body = if carries_payload && !streaming {
        price_response_body(out.body, charge.as_deref_mut())
    } else {
        out.body
    };
    let mut sender =
        match response.send_response(http::Response::from_parts(head, ()), !carries_payload) {
            Ok(sender) => sender,
            Err(_) => return,
        };
    if let Some(stream) = stream {
        if carries_payload {
            write_stream(&mut sender, stream, write_timeout, charge).await;
        }
    } else if carries_payload {
        write_body(&mut sender, body, write_timeout).await;
    }
}

/// Price the block a response body keeps alive while this stream writes it.
///
/// `Bytes` exposes no capacity, and its length is not the block: a handler can hand over a 1 MiB
/// payload inside a 2 MiB allocation. `try_into_mut` recovers the block without copying only for a
/// handle `bytes` is willing to give the allocation back to — a `Vec` it still holds, or a shared
/// block whose only other handle has been dropped — so `capacity()` is then a number the budget can
/// trust. A body that fails that check is left uncharged rather than copied into fresh memory just
/// to have something to register: a `&'static` slice is not per-stream heap; a block another holder
/// retains belongs to that holder's accounting, and charging it here would price one cached
/// allocation once per stream that reads it; and `bytes` reports an owner-backed block
/// (`Bytes::from_owner`) as never recoverable *even for its sole handle*, because handing an
/// owner's buffer to a `BytesMut` would have to copy it — that one is a real gap, bounded by the
/// handler's choice and not closeable from here without allocating. Hence the rule this function
/// depends on, stated on [`IntoResponse`](crate::app::IntoResponse): an HTTP/2 response body is
/// built with `Bytes::from`, never with `Bytes::from_owner`. A body sliced from a larger
/// buffer is priced from its first byte to the end of that buffer: an unused tail is reported and
/// so priced, an unused prefix is not reported by `Bytes` and so stays unpriced.
///
/// `a_body_the_stream_solely_owns_is_priced_by_its_allocation_not_by_a_copy`,
/// `a_body_sliced_from_a_larger_block_is_priced_only_up_to_its_end`,
/// `the_bodies_left_unpriced_are_the_static_shared_and_owner_backed_ones` and
/// `every_body_this_framework_produces_is_a_block_the_budget_can_name` pin each of those
/// sentences against the function, including that no shape below is copied on the way through.
///
/// Best effort, like the HTTP/1 write buffer: a budget that refuses is not a reason to withhold
/// an answer the handler already produced — the charge is best-effort, the response is not.
///
/// The stream carries one variable block, not a sum: [`StreamCharge::buffer_set`] sets the
/// registration, so pricing the response replaces the inbound buffer's charge. A handler that
/// returns a fresh body *and* keeps the request body alive (buffering it into state, say) is
/// therefore priced by the response block alone — the same shape the inbound reading had while
/// the request was arriving, and the reason this charge is a bound on the stream's live block
/// rather than an exact heap census.
fn price_response_body(body: Bytes, charge: Option<&mut StreamCharge>) -> Bytes {
    let block = match body.try_into_mut() {
        Ok(block) => block,
        Err(body) => return body,
    };
    if let Some(charge) = charge {
        // A refusal leaves the registration where it was and does not shorten the response.
        let _ = charge.buffer_set(block.capacity() as u64);
    }
    block.freeze()
}

/// Price one streamed chunk while this task holds it.
///
/// The same rule as [`price_response_body`] — the block is what the budget is told, not the
/// written length — with one difference forced by a body that lasts: a block this stream does not
/// own (a static slice, or one another handle shares) is priced by its length instead of being
/// left at whatever the previous chunk registered. Length is a floor, never a copy, and the next
/// chunk replaces the registration anyway.
///
/// `None` means the budget refused: the caller sheds the stream, because a body of unknown length
/// has no size at which a refusal can be answered with anything but an end to the transfer.
fn price_stream_chunk(chunk: Bytes, charge: Option<&mut StreamCharge>) -> Option<Bytes> {
    let Some(charge) = charge else {
        return Some(chunk);
    };
    let block = match chunk.try_into_mut() {
        Ok(block) => block,
        Err(chunk) => {
            let len = chunk.len() as u64;
            return charge.buffer_set(len).then_some(chunk);
        }
    };
    let capacity = block.capacity() as u64;
    charge.buffer_set(capacity).then(|| block.freeze())
}

/// Hand the payload to `h2` at the pace its send buffer allows.
///
/// The deadline is renewed only after `send_data` succeeded with data taken from real capacity —
/// never on a wake that produced none. `h2` reports capacity as
/// `min(flow-control window, 400 KiB) - buffered`, and `poll_capacity` returns `Pending`, not
/// `Ready(0)`, when that is zero, so a wake with capacity means the connection either received a
/// window update or reclaimed send-buffer space by writing frames out. A wake that produces
/// nothing keeps the old deadline.
///
/// Moving bytes is necessary for a renewal but not sufficient: [`MIN_PROGRESS_FOR_RENEWAL`] has to
/// be paid for first, and credit accumulated across intervals counts toward it. That is what bounds
/// the lifetime of a stream a peer only trickles, and it leaves the total duration of a stream that
/// consumes normally unbounded, as an interval bound must.
async fn write_body(sender: &mut h2::SendStream<Bytes>, body: Bytes, write_timeout: Duration) {
    let mut deadline = Instant::now() + write_timeout;
    let mut earned = 0usize;
    let _ = pump(
        sender,
        body,
        write_timeout,
        &mut deadline,
        &mut earned,
        true,
    )
    .await;
}

/// Write as much of `remaining` as the stream takes, renewing `deadline` as the bytes are earned.
///
/// `end_after` marks the last slice as the end of the whole response. A streamed body sets it
/// false on every chunk: a chunk boundary is not the end of the message, and a DATA frame flagged
/// END_STREAM would end it.
///
/// `None` means nobody is left to receive the rest — the stream was reset or the connection ended
/// — or that the peer stopped consuming this stream for a whole interval, in which case the stream
/// is reset. `Some(tail)` is the part that did not fit, which is empty whenever the whole input was
/// written.
async fn pump(
    sender: &mut h2::SendStream<Bytes>,
    mut remaining: Bytes,
    write_timeout: Duration,
    deadline: &mut Instant,
    earned: &mut usize,
    end_after: bool,
) -> Option<Bytes> {
    while !remaining.is_empty() {
        let mut capacity = sender.capacity();
        if capacity == 0 {
            sender.reserve_capacity(remaining.len().min(WRITE_CHUNK));
            capacity = match tokio::time::timeout_at(
                (*deadline).into(),
                std::future::poll_fn(|cx| sender.poll_capacity(cx)),
            )
            .await
            {
                // No capacity in the whole interval: the peer has stopped consuming this stream.
                // Shed the stream, never the connection or its other streams.
                Err(_) => {
                    sender.send_reset(h2::Reason::CANCEL);
                    return None;
                }
                // The stream or the connection ended while waiting; nobody is left to receive
                // the rest.
                Ok(None) | Ok(Some(Err(_))) => return None,
                // `poll_capacity` guarantees a ready value greater than zero.
                Ok(Some(Ok(capacity))) => capacity,
            };
        }
        let len = capacity.min(remaining.len()).min(WRITE_CHUNK);
        let slice = remaining.slice(..len);
        let last = end_after && len == remaining.len();
        if sender.send_data(slice, last).is_err() {
            return None;
        }
        remaining = remaining.slice(len..);
        // Renewed here and nowhere else, and only once the interval has earned it: bytes accumulate
        // across waits until the minimum is paid, then the tally restarts and the clock with it.
        *earned += len;
        if *earned >= MIN_PROGRESS_FOR_RENEWAL {
            *earned = 0;
            *deadline = Instant::now() + write_timeout;
        }
    }
    Some(remaining)
}

/// Drain a streaming body into DATA frames, pricing each chunk while it is in hand.
///
/// The registration follows the chunk, not the body: it is set to the block the source just
/// handed over, and cleared the moment that chunk is fully queued, so a source that pauses
/// between chunks holds no budget while it pauses. That is what makes a streamed response cost
/// one chunk instead of its length.
///
/// Every ending is visible. A source that ends sends an empty DATA frame with END_STREAM — the
/// only way to end a response whose last payload frame was already queued. A source that fails, a
/// budget that refuses a chunk, or a source that spins on empty chunks resets the stream: `h2`
/// has no truncated-but-terminated body to offer, and a reset is how a client learns the payload
/// is incomplete.
async fn write_stream(
    sender: &mut h2::SendStream<Bytes>,
    mut body: BodyStream,
    write_timeout: Duration,
    mut charge: Option<&mut StreamCharge>,
) {
    let mut deadline = Instant::now() + write_timeout;
    let mut earned = 0usize;
    let mut empty_polls = 0u32;
    loop {
        let chunk = match body.next_chunk().await {
            None => break,
            Some(Ok(chunk)) if chunk.is_empty() => {
                // A chunk boundary is not a pause: a source answering empty without producing a
                // byte is spinning, and nothing is written for the reactor to yield on.
                empty_polls += 1;
                if empty_polls > crate::app::MAX_EMPTY_CHUNKS {
                    sender.send_reset(h2::Reason::CANCEL);
                    return;
                }
                continue;
            }
            Some(Ok(chunk)) => {
                empty_polls = 0;
                chunk
            }
            Some(Err(_)) => {
                sender.send_reset(h2::Reason::CANCEL);
                return;
            }
        };
        let Some(chunk) = price_stream_chunk(chunk, charge.as_deref_mut()) else {
            sender.send_reset(h2::Reason::CANCEL);
            return;
        };
        if pump(
            sender,
            chunk,
            write_timeout,
            &mut deadline,
            &mut earned,
            false,
        )
        .await
        .is_none()
        {
            return;
        }
        // The chunk is queued, so its block is h2's to write and no longer this stream's to hold.
        if let Some(charge) = charge.as_deref_mut() {
            let _ = charge.buffer_set(0);
        }
    }
    let _ = sender.send_data(Bytes::new(), true);
}

/// A response the frontend produces when the scheduler refuses to size a new stream.
fn over_budget() -> AetherResponse {
    let mut response = AetherResponse::status(503, "Service Unavailable");
    response.add_header("Retry-After", "1");
    response
}

/// One live stream's registration against the scheduler's memory budget, and the three states
/// acquiring it can end in.
pub(crate) enum Charge {
    /// Scheduling is not configured for this server: nothing to re-price, nothing to shed on.
    Untracked,
    Held(StreamCharge),
    /// The budget would not cover the stream's reservation. The caller answers 503 and drops the
    /// stream before it can allocate a body buffer.
    Refused,
}

impl Charge {
    fn acquire(core: &Arc<ServiceCore>, conn_job: Option<JobId>) -> Charge {
        let Some(layer) = core.scheduling.as_ref() else {
            return Charge::Untracked;
        };
        // Without a connection job there is no registration to grow: the connection was accepted
        // on the unscheduled path, so a stream on it is accounted like one there.
        let Some(conn_job) = conn_job else {
            return Charge::Untracked;
        };
        if layer.adjust_memory(conn_job, STREAM_BASE_MEMORY as i64) {
            Charge::Held(StreamCharge {
                layer: Arc::clone(layer),
                conn_job,
                held: STREAM_BASE_MEMORY,
            })
        } else {
            Charge::Refused
        }
    }
}

/// Held by the stream task, so a handler panic, a client reset, or the drain timeout all release
/// the registration through `Drop` — the no-orphan rule the connection and handler guards follow.
pub(crate) struct StreamCharge {
    layer: Arc<crate::scheduling::SchedulingLayer>,
    conn_job: JobId,
    held: u64,
}

impl StreamCharge {
    /// Re-price this stream so its registration equals [`STREAM_BASE_MEMORY`] plus `buffered`
    /// — the block the body buffer currently *reserves*, not the bytes written into it.
    /// `false` means the budget refused, and the caller sheds the stream.
    ///
    /// A per-frame delta of the written length under-priced exactly the way HTTP/1 did before
    /// `conn_buffer_charge`: `Vec` growth is `max(2 * capacity, needed)`, so 1.5 MiB of body
    /// lives in a 2 MiB block and only the block is RSS.
    fn buffer_set(&mut self, buffered: u64) -> bool {
        let want = STREAM_BASE_MEMORY.saturating_add(buffered);
        if want == self.held {
            return true;
        }
        let delta = if want > self.held {
            (want - self.held) as i64
        } else {
            -((self.held - want) as i64)
        };
        if self.layer.adjust_memory(self.conn_job, delta) {
            self.held = want;
            true
        } else {
            false
        }
    }
}

impl Drop for StreamCharge {
    fn drop(&mut self) {
        if self.held != 0 {
            self.layer.adjust_memory(self.conn_job, -(self.held as i64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppStateContainer, AsyncHandlerFn};
    use crate::scheduling::{CONN_BASE_MEMORY, SchedulingLayer};

    use aether_middleware::LoggerMiddleware;
    use aether_router::trie::RadixRouter;
    use aether_scheduler::{NewJob, ResourceConfig, SchedulingConfig};
    use aether_websocket::WebSocketLimits;

    /// A configured budget above the SETTINGS ceiling must clamp, never wrap: `4294967296 as u32`
    /// is `0`, and a zero cap makes `h2` refuse every header list while the HTTP/1 frontend on the
    /// same listener honours the configured value.
    #[test]
    fn the_h2_header_cap_saturates_instead_of_wrapping() {
        assert_eq!(header_list_cap(8 * 1024), 8 * 1024);
        assert_eq!(header_list_cap(u32::MAX as usize), u32::MAX);
        assert_eq!(header_list_cap(usize::MAX), u32::MAX);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_h2_header_cap_does_not_wrap_just_past_the_ceiling() {
        assert_eq!(header_list_cap(u32::MAX as usize + 1), u32::MAX);
        assert_eq!(header_list_cap(4 * 1024 * 1024 * 1024), u32::MAX);
    }

    fn parts(headers: &[(&str, &str)], uri: &str) -> http::request::Parts {
        let mut builder = http::Request::builder().uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(()).expect("request head").into_parts().0
    }

    /// The server-wide `Allow` value for a core built from `router`, computed the same way
    /// `app.rs` computes it: the union of every route's methods, with `HEAD` implied by a
    /// plain `GET` route. No route in these tests is a WebSocket route.
    fn allow_for(router: &RadixRouter) -> String {
        let mut methods = 0u64;
        router.for_each_route(|method, _| methods |= method.bitmask());
        RadixRouter::allow_header_for_target(methods, true)
    }

    /// Every repeated line is retained under one lowercase key, so the first line (which
    /// `RequestContext::header` reports) and the full list come from the same store — the same
    /// rule the HTTP/1 parser applies, which is what stops a handler and a security decision from
    /// seeing different header sets depending on the protocol.
    #[test]
    fn repeated_lines_share_one_lowercase_key() {
        let multi = header_store(
            &parts(
                &[
                    ("X-Marker", "first"),
                    ("x-marker", "second"),
                    ("Accept", "text/html"),
                ],
                "https://aether-h2.test/p",
            ),
            &ServerLimits::default(),
        )
        .expect("inside the line budget");

        assert_eq!(
            multi["x-marker"]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["first", "second"],
            "a repeated field line must not be silently merged or dropped"
        );
        assert_eq!(
            multi["accept"]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["text/html"]
        );
        assert!(
            !multi.contains_key("X-Marker"),
            "keys are stored lowercase, so a handler cannot miss on case"
        );
    }

    /// HTTP/2 carries the target in `:authority`, which the `http` crate folds into the URI and
    /// not into the header map. Without this fold, `HostValidation` would answer 421 to a
    /// perfectly valid h2 request.
    #[test]
    fn authority_becomes_the_host_the_policy_reads() {
        let multi = header_store(
            &parts(&[], "https://aether-h2.test:8443/p"),
            &ServerLimits::default(),
        )
        .expect("inside the line budget");

        assert_eq!(
            multi["host"].iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["aether-h2.test:8443"]
        );
    }

    /// A client that sent both — which RFC 9113 §8.3.1 says a conformant client must not — gets
    /// the `:authority`, which is the host the HTTP/1 path resolves from an absolute-form target.
    /// The `Host` lines, including a duplicate of them, collapse into that one value.
    #[test]
    fn the_authority_beats_an_explicit_host() {
        let multi = header_store(
            &parts(
                &[("host", "explicit.test"), ("host", "other.test")],
                "https://aether-h2.test/p",
            ),
            &ServerLimits::default(),
        )
        .expect("inside the line budget");

        assert_eq!(
            multi["host"].iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["aether-h2.test"],
            "the Host lines the client sent must not survive alongside the routed authority"
        );
    }

    /// With no `:authority` on the request there is no routed host to prefer, so the `Host` field
    /// stands as sent.
    #[test]
    fn a_host_without_an_authority_stands() {
        let multi = header_store(
            &parts(&[("host", "explicit.test")], "/relative-only"),
            &ServerLimits::default(),
        )
        .expect("inside the line budget");

        assert_eq!(
            multi["host"].iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["explicit.test"]
        );
    }

    #[test]
    fn a_request_head_over_the_line_budget_is_refused() {
        let limits = ServerLimits {
            max_headers_count: 2,
            ..ServerLimits::default()
        };
        assert!(
            header_store(
                &parts(
                    &[("a", "1"), ("b", "2"), ("c", "3")],
                    "https://aether-h2.test/p"
                ),
                &limits
            )
            .is_none(),
            "the per-line bound has to bite on repeated lines too, not only on distinct names"
        );
    }

    /// A request with no authority and no `Host` gets neither: inventing one would let the host
    /// policy pass a request that names no server at all.
    #[test]
    fn a_head_with_no_host_and_no_authority_invents_neither() {
        let multi = header_store(
            &parts(&[("x", "1")], "/relative-only"),
            &ServerLimits::default(),
        )
        .expect("inside the line budget");
        assert!(!multi.contains_key("host"), "got {:?}", multi.get("host"));
    }

    /// The e2e drain test goes through `TestServer`, where the listener's watchdog is armed with
    /// the same budget and force-aborts the connection at it, so it cannot say which timer fired.
    /// Here `serve` is driven directly with no listener in the path: the connection's own drain
    /// deadline (and the post-loop budget derived from it) is the only timer that can end it.
    ///
    /// A cut that lands inside "one budget" cannot be checked by one run: any constant cut time
    /// fits some window. The case therefore runs at 200, 300 and 600 ms with disjoint acceptance
    /// windows — `[200, 300)`, `[300, 450)` and `[600, 750)` — so no constant can satisfy all
    /// three. The lowest budget also catches a floor that only raises small budgets: a 400 ms
    /// floor measured 423 ms, and even a 300 ms floor cuts past the window's top (measured
    /// 318 ms); a floor below ~270 ms fits inside that window's slack and is not claimed. The two
    /// that spend a second budget land outside every window: after an early exit the post-loop
    /// timeout adds one more budget (~2×), and a deadline re-armed on each iteration cuts at
    /// ~1.75× or later, since the wake has to finish the delayed read first. An append of ~100 ms
    /// after the deadline is caught by the lowest window, but a 50 ms append still fits inside
    /// every window and is not claimed: the windows leave ~70 ms of slack over the measured
    /// deadline-to-observation overhead.
    ///
    /// The re-armed shape needs a loop iteration during the drain, so each case carries a second
    /// stream whose read is delayed to 75% of the budget: its task finishes *during* the drain and
    /// wakes the loop. That stream also pins the other half of the claim — it can only finish
    /// while `conn.accept()` keeps being polled, which the exit-and-wait-for-the-post-loop-timeout
    /// shape stops doing. Both bodies are sized against h2's 65,535-byte default stream window
    /// (the client's connection window is raised explicitly), so a 0.4.x release that changed
    /// that default would invalidate the stall this test relies on.
    ///
    /// One limitation is recorded rather than tested: for a connection with live streams, routing
    /// a draining connection into the idle branch is observationally identical, because
    /// `drain_budget` is `keep_alive_timeout.min(30 s)` and every budget here is below the cap —
    /// the idle timer and the drain timer expire at the same instant. The routing is pinned by
    /// the source until a test drives a draining connection that accepted no request, where the
    /// idle branch would wait `read_timeout` instead.
    #[tokio::test]
    async fn the_connection_cuts_a_stalled_stream_at_its_own_drain_deadline() {
        let low = drain_case(Duration::from_millis(200)).await;
        assert!(
            (Duration::from_millis(200)..Duration::from_millis(300)).contains(&low),
            "a 200 ms budget cut at {low:?}, outside the one-budget window"
        );
        let fast = drain_case(Duration::from_millis(300)).await;
        assert!(
            (Duration::from_millis(300)..Duration::from_millis(450)).contains(&fast),
            "a 300 ms budget cut at {fast:?}, outside the one-budget window"
        );
        let slow = drain_case(Duration::from_millis(600)).await;
        assert!(
            (Duration::from_millis(600)..Duration::from_millis(750)).contains(&slow),
            "a 600 ms budget cut at {slow:?}, outside the one-budget window"
        );
    }

    /// One connection, two streams, cut off by a drain signal sent after `2 × budget + 150 ms` of
    /// live streaming. The stalled stream never releases capacity past its first window; the
    /// progressing stream (128 KiB, more than one stream window) is read only after 75% of the
    /// budget. Returns how long the connection took to end after the signal.
    async fn drain_case(budget: Duration) -> Duration {
        const STALLED_BODY: usize = 1024 * 1024;
        const PROGRESSING_BODY: usize = 128 * 1024;
        let progress_delay = budget * 3 / 4;

        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/stall-body", 0);
        router.add_route(Method::Get, "/progress-body", 1);
        let mut handlers: HashMap<u32, AsyncHandlerFn> = HashMap::new();
        handlers.insert(
            0,
            Arc::new(|_ctx: crate::app::RequestContext| {
                Box::pin(async {
                    AetherResponse::bytes(
                        Bytes::from(vec![b'x'; STALLED_BODY]),
                        "application/octet-stream",
                    )
                })
            }),
        );
        handlers.insert(
            1,
            Arc::new(|_ctx: crate::app::RequestContext| {
                Box::pin(async {
                    AetherResponse::bytes(
                        Bytes::from(vec![b'y'; PROGRESSING_BODY]),
                        "application/octet-stream",
                    )
                })
            }),
        );
        let server_allow = allow_for(&router);
        let core = Arc::new(ServiceCore {
            router,
            handlers,
            ws_handlers: HashMap::new(),
            ws_middleware: HashMap::new(),
            ws_subprotocols: HashMap::new(),
            streaming_routes: std::collections::HashSet::new(),
            state: Arc::new(AppStateContainer::default()),
            limits: Arc::new(ServerLimits {
                keep_alive_timeout: budget,
                // The stream's own per-stream bound must outlast this case, so a cut at the
                // drain deadline cannot be mistaken for `write_timeout` expiring.
                write_timeout: Duration::from_secs(10),
                ..ServerLimits::default()
            }),
            cors: None,
            compression: None,
            security: None,
            host_validation: None,
            trusted_proxies: None,
            rate_limit: None,
            csrf: None,
            logger: Arc::new(LoggerMiddleware::default()),
            request_id: true,
            server_allow,
            scheduling: None,
            websocket_limits: WebSocketLimits::default(),
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local address");
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let connection = tokio::spawn(async move {
            let (tcp, peer) = listener.accept().await.expect("accept");
            serve(Stream::Plain(tcp), core, peer, None, shutdown_rx).await;
        });

        let tcp =
            tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
                .await
                .expect("connect timed out")
                .expect("connect");
        // The connection window must hold both streams' first windows at once: the stalled stream
        // keeps its 65,535-byte share and never releases it, so the default 65,535-byte connection
        // window would leave the progressing stream no room to move at all. The stream-level
        // window stays at h2's default, which is what keeps the first stream stalled.
        let mut builder = h2::client::Builder::new();
        builder.initial_connection_window_size(4 * 1024 * 1024);
        let (mut sender, driver) =
            tokio::time::timeout(Duration::from_secs(2), builder.handshake::<_, Bytes>(tcp))
                .await
                .expect("h2 handshake timed out")
                .expect("h2 handshake");
        let driver = tokio::spawn(async move {
            let _ = driver.await;
        });

        let request = http::Request::builder()
            .method("GET")
            .uri(format!("http://{addr}/stall-body"))
            .body(())
            .expect("request");
        // The client never releases this stream's receive capacity, so its handler task parks on
        // `poll_capacity` from the first window on.
        let (stalled_response, stalled_hold) =
            sender.send_request(request, true).expect("send request");
        let stalled_response = tokio::time::timeout(Duration::from_secs(2), stalled_response)
            .await
            .expect("response head timed out")
            .expect("response head");
        assert_eq!(stalled_response.status(), 200);
        let mut stalled_body = stalled_response.into_body();

        let request = http::Request::builder()
            .method("GET")
            .uri(format!("http://{addr}/progress-body"))
            .body(())
            .expect("request");
        // Requested before shutdown but not read until `progress_delay` after it: it cannot
        // finish early, and afterwards it can only finish while `conn.accept()` is still being
        // polled. Finishing mid-drain is the point; finishing before the drain would leave a
        // sliding deadline indistinguishable from an absolute one.
        let (progressing_response, progressing_hold) =
            sender.send_request(request, true).expect("send request");
        let progressing_response =
            tokio::time::timeout(Duration::from_secs(2), progressing_response)
                .await
                .expect("response head timed out")
                .expect("response head");
        assert_eq!(progressing_response.status(), 200);
        let mut progressing_body = progressing_response.into_body();

        // The stalled stream must outlive the keep-alive timer: that timer is for an idle
        // connection, and this one has live streams. Waiting 2× the budget also leaves the old
        // shape (idle break at one budget, then a second post-loop budget) finished with 150 ms
        // to spare, so a pass here is evidence that the idle branch was not taken.
        tokio::time::sleep(budget * 2 + Duration::from_millis(150)).await;
        assert!(
            !connection.is_finished(),
            "a connection with live streams was cut before its drain was signalled"
        );
        assert!(
            !driver.is_finished(),
            "the client driver died on its own, so the cut below would prove nothing"
        );

        let started = Instant::now();
        shutdown_tx.send(true).expect("signal shutdown");
        let read_progress = async {
            tokio::time::sleep(progress_delay).await;
            let mut delivered = 0usize;
            while let Some(chunk) = progressing_body.data().await {
                let chunk = chunk.expect(
                    "the drain cut a stream that still had credit instead of letting it finish",
                );
                delivered += chunk.len();
                progressing_body
                    .flow_control()
                    .release_capacity(chunk.len())
                    .expect("release capacity");
            }
            delivered
        };
        let (connection_outcome, progress_outcome) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(2), connection),
            tokio::time::timeout(Duration::from_secs(2), read_progress),
        );
        connection_outcome
            .expect("the connection task outlived its drain deadline")
            .expect("connection task");
        let cut = started.elapsed();
        // Read before the next await, so on this single-threaded runtime the driver task cannot
        // have been scheduled since the connection ended: this is the cut's cause, not cleanup.
        assert!(
            !driver.is_finished(),
            "the connection ended because the client side went away, not at its own deadline"
        );
        let delivered_progress = progress_outcome
            .expect("the progressing stream did not finish inside the drain window");
        assert_eq!(
            delivered_progress, PROGRESSING_BODY,
            "the drain stopped driving the connection before a stream with credit finished"
        );

        // Only the initial 65,535-byte stream window ever moved; the rest never got credit, so
        // the stream was cut mid-write rather than finished. `delivered > 0` keeps the
        // `delivered < 1 MiB` assertion from passing vacuously when nothing arrived at all.
        drop(stalled_hold);
        let mut delivered = 0usize;
        let clean_end = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(chunk) = stalled_body.data().await {
                match chunk {
                    Ok(chunk) => delivered += chunk.len(),
                    Err(_) => return false,
                }
            }
            true
        })
        .await
        .expect("the cut stream never ended after its connection was gone");
        assert!(
            !clean_end,
            "the cut stream ended cleanly as if the whole body had been written"
        );
        assert!(
            delivered > 0 && delivered < STALLED_BODY,
            "delivered {delivered} bytes of a 1 MiB body that was supposed to be cut mid-write"
        );

        drop(progressing_hold);
        drop(sender);
        driver.abort();
        let _ = driver.await;
        cut
    }

    /// A route that answers with `body` bytes held in a `body + spare` allocation. The spare
    /// capacity is what separates a length-based charge from a block-based one: only the block
    /// is what the allocator reserves.
    fn body_router(body: usize, spare: usize) -> (RadixRouter, HashMap<u32, AsyncHandlerFn>) {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/body", 0);
        let mut handlers: HashMap<u32, AsyncHandlerFn> = HashMap::new();
        handlers.insert(
            0,
            Arc::new(move |_ctx: crate::app::RequestContext| {
                Box::pin(async move {
                    let mut block = Vec::with_capacity(body + spare);
                    block.resize(body, b'x');
                    AetherResponse::bytes(Bytes::from(block), "application/octet-stream")
                })
            }),
        );
        (router, handlers)
    }

    /// A core whose frontend and handler admissions share one scheduling layer, so every
    /// registration the test reads is the sum of the jobs actually running.
    fn scheduled_core(
        layer: &Arc<SchedulingLayer>,
        router: RadixRouter,
        handlers: HashMap<u32, AsyncHandlerFn>,
        write_timeout: Duration,
    ) -> Arc<ServiceCore> {
        let server_allow = allow_for(&router);
        Arc::new(ServiceCore {
            router,
            handlers,
            ws_handlers: HashMap::new(),
            ws_middleware: HashMap::new(),
            ws_subprotocols: HashMap::new(),
            streaming_routes: std::collections::HashSet::new(),
            state: Arc::new(AppStateContainer::default()),
            limits: Arc::new(ServerLimits {
                write_timeout,
                // Short enough that a test which drops the connection never waits on a stream
                // that is parked on send capacity; no test here relies on the idle timer.
                keep_alive_timeout: Duration::from_millis(300),
                ..ServerLimits::default()
            }),
            cors: None,
            compression: None,
            security: None,
            host_validation: None,
            trusted_proxies: None,
            rate_limit: None,
            csrf: None,
            logger: Arc::new(LoggerMiddleware::default()),
            request_id: true,
            server_allow,
            scheduling: Some(Arc::clone(layer)),
            websocket_limits: WebSocketLimits::default(),
        })
    }

    /// One served h2c connection, with the connection job registered on the test's layer.
    struct ScheduledConn {
        addr: std::net::SocketAddr,
        send: h2::client::SendRequest<Bytes>,
        driver: tokio::task::JoinHandle<()>,
        connection: tokio::task::JoinHandle<()>,
        shutdown: tokio::sync::watch::Sender<bool>,
    }

    impl ScheduledConn {
        async fn close(self) {
            let _ = self.shutdown.send(true);
            self.driver.abort();
            let _ = self.driver.await;
            tokio::time::timeout(Duration::from_secs(2), self.connection)
                .await
                .expect("connection task outlived its shutdown")
                .expect("connection task");
        }
    }

    async fn scheduled_connection(core: Arc<ServiceCore>, job: JobId) -> ScheduledConn {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local address");
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let connection = tokio::spawn(async move {
            let (tcp, peer) = listener.accept().await.expect("accept");
            serve(Stream::Plain(tcp), core, peer, Some(job), shutdown_rx).await;
        });

        let tcp =
            tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
                .await
                .expect("connect timed out")
                .expect("connect");
        let builder = h2::client::Builder::new();
        let (send, driver) =
            tokio::time::timeout(Duration::from_secs(2), builder.handshake::<_, Bytes>(tcp))
                .await
                .expect("h2 handshake timed out")
                .expect("h2 handshake");
        let driver = tokio::spawn(async move {
            let _ = driver.await;
        });
        ScheduledConn {
            addr,
            send,
            driver,
            connection,
            shutdown,
        }
    }

    /// Registration moves inside the stream task, so a just-ended stream can be seen a few
    /// scheduling points after its reset reaches the client.
    async fn registered_until(layer: &SchedulingLayer, want: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let got = layer.resources().registered_memory_bytes;
            if got == want {
                return got;
            }
            assert!(
                Instant::now() < deadline,
                "registration stuck at {got}, wanted {want}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The block a response body keeps alive is registered while the stream holds it, priced
    /// from the allocation rather than from the payload length, and released when the stream
    /// ends.
    ///
    /// The body is 1 MiB inside a 1.25 MiB allocation on purpose: a charge computed from
    /// `len()` would register 1 MiB and fail the exact assertion. The head can only reach the
    /// client after the stream task has priced the body, so the reading is not a sample of a
    /// race. The client never reads, and 1 MiB cannot fit its 65,535-byte stream window, so the
    /// task is still parked on send capacity when the shutdown drain expires and the connection
    /// aborts it in place — the harshest exit a `StreamCharge` has to release on. The reset path
    /// is covered against the same registration by
    /// `a_reader_that_stops_gets_a_stream_reset_without_killing_the_connection`.
    ///
    /// A body this stream does not uniquely own is not priced, and that limit is deliberate:
    /// static bytes are not per-stream heap, and a block shared with another holder is not this
    /// stream's to charge; copying either into fresh memory just to have a block to register
    /// would allocate exactly what the charge exists to bound.
    #[tokio::test]
    async fn a_response_body_is_registered_by_its_block_while_the_stream_holds_it() {
        const BODY: usize = 1024 * 1024;
        const SPARE: usize = 256 * 1024;
        const BLOCK: u64 = (BODY + SPARE) as u64;
        // The premise of the parked task, pinned as arithmetic rather than trusted from a comment:
        // a client that never calls `release_capacity` can hand back at most its default 65,535-byte
        // stream window, so a body larger than that cannot be written and the stream task can only
        // end when the drain aborts it.
        const _: () = assert!(BODY > 65_535);

        let layer = SchedulingLayer::new(SchedulingConfig::default(), ResourceConfig::default());
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let (router, handlers) = body_router(BODY, SPARE);
        // No write deadline races the reading below: while the body is held, the task is parked
        // on the client's 65,535-byte stream window.
        let core = scheduled_core(&layer, router, handlers, Duration::from_secs(30));
        let mut conn = scheduled_connection(core, conn_guard.id()).await;

        let request = http::Request::builder()
            .method("GET")
            .uri(format!("http://{}/body", conn.addr))
            .body(())
            .expect("request");
        let (response, hold) = conn
            .send
            .send_request(request, true)
            .expect("stream accepted");
        let response = tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("response head timed out")
            .expect("response head");
        assert_eq!(response.status(), 200);

        let during = layer.resources().registered_memory_bytes;
        assert_eq!(
            during,
            CONN_BASE_MEMORY + STREAM_BASE_MEMORY + BLOCK,
            "a held response body must be registered by its block, not by its length"
        );

        // Both client halves stay alive: dropping either would reset the stream from this side
        // and end the server's task through `poll_capacity`. The client socket also stays open,
        // so the only thing that can end the parked task is the drain's own abort — the one exit
        // no other test drives while a response body is registered. `keep_alive_timeout` is the
        // drain budget here, so this ends in about 300 ms, not at `write_timeout`.
        let body = response.into_body();
        let _ = conn.shutdown.send(true);
        tokio::time::timeout(Duration::from_secs(2), conn.connection)
            .await
            .expect("the connection's drain outlived its budget")
            .expect("connection task");
        assert_eq!(
            layer.resources().registered_memory_bytes,
            CONN_BASE_MEMORY,
            "the stream's base and the body's block stayed registered after the stream was aborted"
        );
        drop(body);
        drop(hold);
        conn.driver.abort();
        let _ = conn.driver.await;

        drop(conn_guard);
        assert_eq!(
            layer.resources().registered_memory_bytes,
            0,
            "the connection's base registration outlived its guard"
        );
    }

    /// A body the budget refuses to price is still sent in full. The charge is best-effort —
    /// the same policy the HTTP/1 write buffer follows — so a refusal may leave the bytes
    /// unregistered but must never withhold an answer the handler already produced.
    ///
    /// The budget covers the connection (32 KiB), the stream base (16 KiB) and the handler job
    /// (16 KiB), and is two orders of magnitude below the body's block. Three separate readings
    /// pin the policy: `memory_shed` proves the block was actually offered and refused (without
    /// it, the exact registration below is equally produced by pricing that never ran), the
    /// registration proves the refused growth was not applied as phantom memory, and the full
    /// 512 KiB read proves the refusal was not treated as a reason to shed the answer.
    #[tokio::test]
    async fn a_response_body_over_the_budget_is_sent_anyway() {
        const BODY: usize = 512 * 1024;
        const SPARE: usize = 64 * 1024;
        const BLOCK: u64 = (BODY + SPARE) as u64;

        let resources = ResourceConfig {
            memory_budget_bytes: 128 * 1024,
            ..ResourceConfig::default()
        };
        assert!(BLOCK > resources.memory_budget_bytes);
        let layer = SchedulingLayer::new(SchedulingConfig::default(), resources);
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let (router, handlers) = body_router(BODY, SPARE);
        let core = scheduled_core(&layer, router, handlers, Duration::from_secs(5));
        let mut conn = scheduled_connection(core, conn_guard.id()).await;

        let request = http::Request::builder()
            .method("GET")
            .uri(format!("http://{}/body", conn.addr))
            .body(())
            .expect("request");
        let (response, hold) = conn
            .send
            .send_request(request, true)
            .expect("stream accepted");
        let response = tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("response head timed out")
            .expect("response head");
        assert_eq!(response.status(), 200);

        let during = layer.resources().registered_memory_bytes;
        assert_eq!(
            during,
            CONN_BASE_MEMORY + STREAM_BASE_MEMORY,
            "a refused body must not be registered as if the growth had been applied"
        );
        // Without this, the reading above is vacuous: a pricing step that never ran produces the
        // same registration as one the budget refused. Only an attempted grow that the budget
        // could not cover bumps `memory_shed`, so it is what proves the refusal happened here.
        assert!(
            layer.metrics().memory_shed > 0,
            "the body's block was never offered to the budget, so nothing was refused"
        );

        let mut body = response.into_body();
        let mut received = 0usize;
        loop {
            let next = tokio::time::timeout(Duration::from_secs(5), body.data())
                .await
                .expect("a body the budget only refused to price must still arrive");
            let Some(chunk) = next else { break };
            let chunk = chunk.expect("the response was cut although only its charge was refused");
            received += chunk.len();
            body.flow_control()
                .release_capacity(chunk.len())
                .expect("release capacity");
        }
        assert_eq!(received, BODY, "the whole body must arrive");

        let settled = registered_until(&layer, CONN_BASE_MEMORY).await;
        assert_eq!(settled, CONN_BASE_MEMORY);

        drop(hold);
        conn.close().await;
        drop(conn_guard);
        assert_eq!(layer.resources().registered_memory_bytes, 0);
    }

    /// A [`StreamCharge`] with its stream base registered — exactly what [`Charge::acquire`]
    /// produces for an admitted connection, built directly so the pricing step can be driven on a
    /// body of the test's choosing. Nothing else moves the registration while such a charge is
    /// alive, so every reading below is exact rather than polled.
    fn stream_charge(layer: &Arc<SchedulingLayer>, job: JobId) -> StreamCharge {
        assert!(
            layer.adjust_memory(job, STREAM_BASE_MEMORY as i64),
            "the stream base must fit the test budget"
        );
        StreamCharge {
            layer: Arc::clone(layer),
            conn_job: job,
            held: STREAM_BASE_MEMORY,
        }
    }

    /// The two halves of the pricing rule, tested on the function that implements them instead of
    /// on the wire: a body the stream solely owns is priced from its *allocation*, and pricing it
    /// never copies it. `as_ptr` is what makes the second half observable — a `try_into_mut` that
    /// fell back to a deep copy would keep every registration number right and still allocate the
    /// payload the charge exists to bound.
    #[tokio::test]
    async fn a_body_the_stream_solely_owns_is_priced_by_its_allocation_not_by_a_copy() {
        const BODY: usize = 128 * 1024;
        const SPARE: usize = 512 * 1024;

        let layer = SchedulingLayer::new(SchedulingConfig::default(), ResourceConfig::default());
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let mut charge = stream_charge(&layer, conn_guard.id());
        let base = CONN_BASE_MEMORY + STREAM_BASE_MEMORY;
        assert_eq!(layer.resources().registered_memory_bytes, base);

        let mut block = Vec::with_capacity(BODY + SPARE);
        block.resize(BODY, b'x');
        let capacity = block.capacity();
        let first_byte = block.as_ptr();
        assert!(capacity >= BODY + SPARE);

        let priced = price_response_body(Bytes::from(block), Some(&mut charge));

        assert_eq!(priced.len(), BODY, "the payload must survive the pricing");
        assert_eq!(
            priced.as_ptr(),
            first_byte,
            "pricing a solely-owned block must reuse it, never copy the payload"
        );
        assert_eq!(
            layer.resources().registered_memory_bytes,
            base + capacity as u64,
            "the registration is the allocation, not the written length"
        );

        drop(priced);
        drop(charge);
        assert_eq!(layer.resources().registered_memory_bytes, CONN_BASE_MEMORY);
        drop(conn_guard);
        assert_eq!(layer.resources().registered_memory_bytes, 0);
    }

    /// A body sliced out of a larger buffer is priced from its first byte to the end of that
    /// buffer — and the direction of the slice decides how much that covers.
    ///
    /// `Bytes` reports capacity as the region its handle still reaches, so dropping the *tail* is
    /// priced in full (it lies after the payload, inside what the handle reports) while dropping
    /// the *prefix* is not: those bytes are still reserved by the allocation this stream keeps
    /// alive, and no handle left here can name them. The gap is the prefix, it is bounded by the
    /// allocation the handler chose, and pricing it would mean copying the payload to find out how
    /// far the block started before it — which is what the charge exists to avoid.
    #[tokio::test]
    async fn a_body_sliced_from_a_larger_block_is_priced_only_up_to_its_end() {
        const BLOCK: usize = 256 * 1024;
        const SLICE: usize = 192 * 1024;
        const EDGE: usize = BLOCK - SLICE;

        let layer = SchedulingLayer::new(SchedulingConfig::default(), ResourceConfig::default());
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let mut charge = stream_charge(&layer, conn_guard.id());
        let base = CONN_BASE_MEMORY + STREAM_BASE_MEMORY;

        // A tail-dropping slice: the payload starts EDGE bytes into the allocation.
        let big = Bytes::from(vec![b'z'; BLOCK]);
        let allocation = big.as_ptr();
        let sliced = big.slice(EDGE..);
        drop(big);
        // The premise, pinned rather than assumed: this handle owns the block alone, which is what
        // lets the pricing reclaim the allocation instead of leaving the body unpriced.
        assert!(sliced.is_unique(), "the slice must own its block alone");
        assert_eq!(sliced.len(), SLICE);

        let priced = price_response_body(sliced, Some(&mut charge));
        assert_eq!(
            priced.len(),
            SLICE,
            "the pricing must not widen the payload"
        );
        assert_eq!(
            priced.as_ptr(),
            allocation.wrapping_add(EDGE),
            "and must not move it out of the allocation it was sliced from"
        );
        assert_eq!(
            layer.resources().registered_memory_bytes,
            base + SLICE as u64,
            "the {EDGE}-byte prefix this block still reserves is priced with it only if a handle \
             can name it, and none can"
        );

        // Back to the starting registration, so the second reading below is exact too.
        drop(priced);
        assert!(
            charge.buffer_set(0),
            "shrinking the registration always fits"
        );
        assert_eq!(layer.resources().registered_memory_bytes, base);

        // A head-keeping slice: the unused tail lies after the payload, inside what the handle
        // reports, so the whole allocation is priced even though the body is only part of it.
        let big = Bytes::from(vec![b'z'; BLOCK]);
        let head = big.slice(..SLICE);
        drop(big);
        assert!(head.is_unique(), "the slice must own its block alone");
        let priced = price_response_body(head, Some(&mut charge));
        assert_eq!(priced.len(), SLICE, "the payload must survive the pricing");
        assert_eq!(
            layer.resources().registered_memory_bytes,
            base + BLOCK as u64,
            "a body whose block extends past its last byte is priced by the block"
        );

        drop(priced);
        drop(charge);
        drop(conn_guard);
    }

    /// The bodies [`price_response_body`] leaves unregistered, listed exhaustively: a `&'static`
    /// slice, a block another handle still holds, and an owner-backed block. Each keeps its bytes
    /// where they are — none of them is copied into fresh memory to have something to register —
    /// and for two of the three that is the right answer, not a gap: static bytes are not per-stream
    /// heap, and a block another handle retains belongs to that holder's accounting — charging it
    /// here would price one cached allocation once per stream that reads it.
    ///
    /// The third is the honest residue. `Bytes::from_owner` answers `false` to `is_unique` for
    /// every handle, including the sole one, because handing an owner's buffer to a `BytesMut`
    /// would have to copy it. So a handler that wraps its own 64 KiB in `from_owner` keeps heap
    /// this budget cannot see, and no zero-copy change to this function can price it.
    #[tokio::test]
    async fn the_bodies_left_unpriced_are_the_static_shared_and_owner_backed_ones() {
        const ASSET: usize = 4 * 1024;
        const SHARED: usize = 64 * 1024;
        const OWNED: usize = 64 * 1024;
        static BLOCK: [u8; ASSET] = [7u8; ASSET];

        let layer = SchedulingLayer::new(SchedulingConfig::default(), ResourceConfig::default());
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let mut charge = stream_charge(&layer, conn_guard.id());
        let base = CONN_BASE_MEMORY + STREAM_BASE_MEMORY;

        let static_body = Bytes::from_static(&BLOCK);
        let priced = price_response_body(static_body, Some(&mut charge));
        assert_eq!(
            priced.as_ptr(),
            BLOCK.as_ptr(),
            "a static body must not be copied"
        );
        assert_eq!(
            layer.resources().registered_memory_bytes,
            base,
            "and must not be registered: this stream allocated nothing for it"
        );
        drop(priced);

        let holder: Vec<u8> = vec![b'q'; SHARED];
        let shared_ptr = holder.as_ptr();
        let shared = Bytes::from(holder);
        let retained = shared.clone();
        let priced = price_response_body(shared, Some(&mut charge));
        assert!(
            !priced.is_unique(),
            "the premise: another handle keeps this block alive"
        );
        assert_eq!(
            priced.as_ptr(),
            shared_ptr,
            "a shared block must not be copied"
        );
        assert_eq!(
            layer.resources().registered_memory_bytes,
            base,
            "a block another holder owns is not this stream's to price"
        );
        assert_eq!(retained.len(), SHARED);
        drop(priced);
        drop(retained);

        let owned = "y".repeat(OWNED);
        let owned_ptr = owned.as_ptr();
        let body = Bytes::from_owner(owned);
        assert!(
            !body.is_unique(),
            "bytes reports an owner-backed block as never unique, even for its sole handle"
        );
        let priced = price_response_body(body, Some(&mut charge));
        assert_eq!(
            priced.as_ptr(),
            owned_ptr,
            "pricing must not replace the owner's buffer with a copy of it"
        );
        assert_eq!(
            priced.len(),
            OWNED,
            "the payload survives, unpriced: the budget cannot see this heap"
        );
        assert_eq!(layer.resources().registered_memory_bytes, base);

        drop(priced);
        drop(charge);
        assert_eq!(layer.resources().registered_memory_bytes, CONN_BASE_MEMORY);
        drop(conn_guard);
        assert_eq!(layer.resources().registered_memory_bytes, 0);
    }

    /// The rule the unpriced owner-backed residue is acceptable *because* of: every body this
    /// framework produces is a block the stream owns alone, and so a block
    /// [`price_response_body`] can name without copying it. Tested by calling the producers rather
    /// than restating them — `IntoResponse for String`, `IntoResponse for (u16, String)`,
    /// `Response::text_dynamic`, `Response::html_dynamic` and `Response::file` (the fs-read asset
    /// shape, which is also what `static_dir` and the embedded-asset route hand over, and what the
    /// compression middleware's `Bytes::from(finished_vec)` produces). None of them wraps a foreign
    /// owner, which is what makes "an HTTP/2 response body must not be built with
    /// `Bytes::from_owner`" a rule about code in this repository rather than a hope about what a
    /// handler author chooses.
    #[tokio::test]
    async fn every_body_this_framework_produces_is_a_block_the_budget_can_name() {
        const BODY: usize = 64 * 1024;

        let layer = SchedulingLayer::new(SchedulingConfig::default(), ResourceConfig::default());
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let mut charge = stream_charge(&layer, conn_guard.id());
        let base = CONN_BASE_MEMORY + STREAM_BASE_MEMORY;

        let asset = std::env::temp_dir().join(format!(
            "aether-h2-body-producers-{}-{BODY}.bin",
            std::process::id()
        ));
        std::fs::write(&asset, vec![b'k'; BODY]).expect("temp asset writable");

        // Each producer called the way a route calls it, with the pointer it put into the response
        // where the source string is still identifiable after the move.
        let text: String = "m".repeat(BODY);
        let text_ptr = text.as_ptr();
        let string_body = <String as crate::app::IntoResponse>::into_response(text).body;

        let pair: String = "n".repeat(BODY);
        let pair_ptr = pair.as_ptr();
        let tuple_body =
            <(u16, String) as crate::app::IntoResponse>::into_response((200, pair)).body;

        let dynamic = AetherResponse::text_dynamic("p".repeat(BODY)).body;
        let html = AetherResponse::html_dynamic("q".repeat(BODY)).body;
        let file = AetherResponse::file(&asset)
            .expect("temp asset readable")
            .body;
        std::fs::remove_file(&asset).expect("temp asset removable");

        for (producer, body, first_byte) in [
            ("IntoResponse for String", string_body, text_ptr),
            ("IntoResponse for (u16, String)", tuple_body, pair_ptr),
            ("Response::text_dynamic", dynamic, std::ptr::null()),
            ("Response::html_dynamic", html, std::ptr::null()),
            ("Response::file", file, std::ptr::null()),
        ] {
            assert_eq!(body.len(), BODY, "{producer} must carry the payload");
            let priced = price_response_body(body, Some(&mut charge));
            if !first_byte.is_null() {
                assert_eq!(
                    priced.as_ptr(),
                    first_byte,
                    "{producer}'s buffer must be reused, not replaced by a copy of the payload"
                );
            }
            let registered = layer.resources().registered_memory_bytes;
            assert!(
                registered >= base + BODY as u64,
                "{producer} produced a body pricing left at {registered} — a response body the \
                 stream owns alone is a block the budget can name"
            );
            drop(priced);
            assert!(
                charge.buffer_set(0),
                "clearing the stream's block leaves its base charge"
            );
            assert_eq!(layer.resources().registered_memory_bytes, base);
        }

        drop(charge);
        drop(conn_guard);
    }

    /// One `h2` stream whose request body the test feeds from the *server* side, over an in-memory
    /// duplex, plus the two tasks that keep the connection state machines moving.
    ///
    /// `read_body` is what a stream task calls, and a refusal's registration is released before any
    /// wire test could sample it — the answers on that path carry no payload, so nothing parks the
    /// task. Driving the function itself, on a connection the test owns, is the only shape in which
    /// "the buffer was dropped, and so is its charge" is a reading rather than a race.
    async fn duplex_upload(
        chunks: Vec<usize>,
        end_stream: bool,
    ) -> (
        RecvStream,
        tokio::task::JoinHandle<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let (body_tx, body_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server_io)
                .await
                .expect("server handshake");
            let (request, _response) = conn
                .accept()
                .await
                .expect("connection alive")
                .expect("request head");
            let _ = body_tx.send(request.into_body());
            // Accepting is what drives this connection: a task that slept here instead would leave
            // the test's `data()` wait hanging.
            while conn.accept().await.is_some() {}
        });
        let client = tokio::spawn(async move {
            let (mut send, conn) = h2::client::handshake(client_io)
                .await
                .expect("client handshake");
            let request = http::Request::builder()
                .method("POST")
                .uri("/upload")
                .body(())
                .expect("request");
            let (_response, mut body) = send.send_request(request, false).expect("send request");
            for len in chunks {
                body.send_data(Bytes::from(vec![b'x'; len]), false)
                    .expect("inside the windows both sides advertise");
            }
            if end_stream {
                body.send_data(Bytes::new(), true).expect("end the stream");
            }
            // The send stream and the response future stay alive to here, so nothing resets this
            // stream from the sending side while the test reads it.
            let _ = conn.await;
        });
        (
            body_rx.await.expect("the server handed the stream over"),
            server,
            client,
        )
    }

    /// A refused upload drops the buffer it was filling, and its registration goes with it.
    ///
    /// Both directions are pinned in one run, because one of them cannot catch the obvious mutant:
    /// a refusal that resets to the stream base is only right if a read that *succeeded* still
    /// carries its block. Without the second half, a `buffer_set(0)` on every exit — which would
    /// leave every served upload unregistered — passes.
    #[tokio::test]
    async fn a_refused_upload_stops_registering_the_buffer_it_dropped() {
        const UNDER: usize = 4 * 1024;
        const OVER: usize = 8 * 1024;

        let layer = SchedulingLayer::new(SchedulingConfig::default(), ResourceConfig::default());
        let conn_guard = layer
            .admit(NewJob::connection(CONN_BASE_MEMORY))
            .await
            .expect("connection admitted");
        let limits = ServerLimits {
            max_body_bytes: (3 * UNDER) / 2,
            ..ServerLimits::default()
        };
        assert!(
            OVER > limits.max_body_bytes - UNDER,
            "the second chunk must cross the limit"
        );
        let deadline = || Instant::now() + Duration::from_secs(30);

        let mut charge = stream_charge(&layer, conn_guard.id());
        let base = CONN_BASE_MEMORY + STREAM_BASE_MEMORY;
        let (mut body, server, client) = duplex_upload(vec![UNDER, OVER], false).await;
        let read = read_body(&mut body, &limits, deadline(), Some(&mut charge)).await;
        assert!(
            matches!(read, Err(Refusal::TooLarge)),
            "the second chunk goes over the body limit, so the read must refuse the stream"
        );
        drop(read);
        assert_eq!(
            layer.resources().registered_memory_bytes,
            base,
            "the refused buffer is gone, so the block it reserved must not stay registered"
        );

        drop(body);
        server.abort();
        client.abort();
        drop(charge);
        assert_eq!(layer.resources().registered_memory_bytes, CONN_BASE_MEMORY);

        // The other direction: the same function, on a body it accepts, keeps the block charged
        // for as long as the returned bytes live.
        let mut charge = stream_charge(&layer, conn_guard.id());
        let (mut body, server, client) = duplex_upload(vec![UNDER], true).await;
        let bytes = read_body(&mut body, &limits, deadline(), Some(&mut charge))
            .await
            .expect("a body inside the limit is read");
        assert_eq!(bytes.len(), UNDER);
        let during = layer.resources().registered_memory_bytes;
        assert!(
            during >= base + UNDER as u64,
            "a buffer this stream still holds must stay registered: {during} for a {UNDER}-byte body"
        );

        drop(bytes);
        drop(body);
        server.abort();
        client.abort();
        drop(charge);
        assert_eq!(layer.resources().registered_memory_bytes, CONN_BASE_MEMORY);
        drop(conn_guard);
        assert_eq!(layer.resources().registered_memory_bytes, 0);
    }

    /// HTTP/2 sends `:status` and no reason phrase, so this string is not on an h2 wire. It is
    /// still the value an HTTP/1 caller of the same refusal would see, and it must be the one
    /// phrase the framework names for 413 (RFC 9110 §15.5.14).
    #[test]
    fn a_body_refusal_names_the_canonical_413_phrase() {
        let response = Refusal::TooLarge.into_response();
        assert_eq!(response.status, 413);
        assert_eq!(response.status_text, canonical_reason(413));
    }
}
