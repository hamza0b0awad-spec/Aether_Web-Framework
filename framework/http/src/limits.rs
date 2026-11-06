//! # HTTP Security Limits (RFC9112 & DoS Protection)

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ServerLimits {
    /// The budget a request head may occupy: an HTTP/1 section over it is answered 431, and HTTP/2
    /// counts the same budget against its HEADERS blocks. It is also the first term of the
    /// connection's wire budget — see [`ServerLimits::max_body_bytes`].
    pub max_header_bytes: usize,
    /// The body ceiling, enforced twice and on two different quantities, both deliberately.
    ///
    /// The buffered parser bounds the **decoded** body by this number, on both frontends. A
    /// streamed body is never buffered: it meets
    /// [`ServerLimits::max_stream_body_bytes`] instead. On HTTP/1.1 an earlier bound applies
    /// first: one connection may not buffer more than
    /// `max_header_bytes + max_body_bytes` bytes of **wire** for a request, because while the body
    /// is incomplete every wire byte is resident next to the decoder's copy — which is what makes
    /// that a memory price rather than a stricter body limit.
    ///
    /// A chunked body pays framing on the wire, so the wire bound is the one a chunked client
    /// meets. Measured on the defaults (8 192 + 2 097 152 = a 2 105 344-byte budget), with a 62-byte
    /// request head and one-byte chunks (`1\r\nx\r\n`, six wire bytes per payload byte): 350 879
    /// payload bytes are served and 350 880 are refused with a 413 — 16.7 % of the 2 MiB this field
    /// names, and a fifth of what the parser would have taken. A `Content-Length` body carries no
    /// framing, so it reaches the full cap, and HTTP/2 bounds DATA payloads, so there this number is
    /// the decoded cap exactly. `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes`
    /// pins both sides of that boundary. Raising this field raises the wire budget with it, since the
    /// budget is the sum of the two.
    pub max_body_bytes: usize,
    /// The ceiling for a body that a streaming route consumes as it arrives.
    ///
    /// `max_body_bytes` is the *buffered* ceiling: it exists because a body the server holds is
    /// resident memory, and the wire budget above (`max_header_bytes + max_body_bytes`) is what an
    /// incomplete HTTP/1.1 request may occupy. A streamed body is never held: the handler reads a
    /// chunk, the frontend reads the next one only when the lane is empty, and the bytes leave the
    /// connection buffer as soon as they are framed. So the server needs a *transfer* ceiling
    /// instead — the largest body a route is willing to process — and it is deliberately larger
    /// than `max_body_bytes`, because otherwise streaming would buy nothing.
    ///
    /// Enforced on cumulative payload, so a chunked body meets it on its decoded bytes rather than
    /// on its framing. A declared `Content-Length` over it is refused with `413` before dispatch,
    /// without reading the body — and, just as importantly, a declared length *under* it is not
    /// refused on the way by the buffered ceiling: the head parse leaves the declared-length
    /// decision to the route, so a streaming route receives the uploads this number exists for.
    pub max_stream_body_bytes: usize,
    pub max_headers_count: usize,
    /// How many streams one HTTP/2 connection may have open at once, sent as
    /// `SETTINGS_MAX_CONCURRENT_STREAMS`. A bound the peer can violate cheaply is worth naming:
    /// `h2` resets the overflow with `REFUSED_STREAM` before a task, a body buffer or a handler
    /// permit exists, so the memory re-pricing per stream is the second line of defence rather
    /// than the first.
    pub max_concurrent_streams: u32,
    pub max_connections: usize,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub keep_alive_timeout: Duration,
    pub request_timeout: Duration,
    /// The total budget for receiving one request — head and body together — on both frontends.
    ///
    /// Separate from [`ServerLimits::request_timeout`], which bounds handler execution. One
    /// number used to serve both roles, so a legitimately slow large upload shared its budget
    /// with the handler and could only be admitted by granting every handler the same extension.
    /// On HTTP/1 the wait for the next bytes is capped by the shorter of this budget's remainder
    /// and `read_timeout`, and a request that began arriving and then stopped is answered `408`
    /// when either runs out — a socket idle between keep-alive requests has no request to answer
    /// and is still closed silently. A stalled HTTP/2 streamed body fails its lane at the same
    /// deadline. The handler deadline is counted only after the request has arrived.
    pub receive_timeout: Duration,
    /// How long a TLS client may take to finish the handshake before the connection is
    /// dropped. A handshake is the same slowloris class as a header: the task and its
    /// buffers exist long before any request does, so it needs its own bound.
    pub tls_handshake_timeout: Duration,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_header_bytes: 8 * 1024,      // 8 KB max header budget
            max_body_bytes: 2 * 1024 * 1024, // 2 MB max body
            // 32x the buffered ceiling: a streamed upload is bounded by time and by this number,
            // never by resident memory, so the two limits answer different questions.
            max_stream_body_bytes: 64 * 1024 * 1024,
            // 100 field lines, the same ceiling hyper's `httparse` configuration uses. The
            // real bound on a header section is `max_header_bytes` above; this one exists to
            // stop a per-line work amplification attack, and 64 rejected legitimate clients
            // (gRPC, large cookies, signed JWT headers) without making that cheaper.
            max_headers_count: 100,
            // A deliberate choice, not a protocol default: RFC 9113 leaves this unbounded, and
            // an unbounded setting turns one socket into as many concurrent handlers as the
            // client likes. 100 keeps a single connection's demand inside the scheduler's memory
            // budget while leaving room for the gRPC and browser clients that pipeline
            // aggressively.
            max_concurrent_streams: 100,
            max_connections: 10_000, // 10k concurrent connections limit
            read_timeout: Duration::from_secs(5), // Slowloris mitigation
            write_timeout: Duration::from_secs(10),
            keep_alive_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            receive_timeout: Duration::from_secs(30),
            tls_handshake_timeout: Duration::from_secs(10),
        }
    }
}
