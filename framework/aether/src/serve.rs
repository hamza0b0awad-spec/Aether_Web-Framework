//! The protocol-independent request-serving core.
//!
//! HTTP/1.x and HTTP/2 both stop here: routing, security rejections, CORS preflight,
//! handler dispatch under the scheduling layer, and the response post-processing that
//! decides `X-Request-ID`, CORS, security headers, compression and the close decision.
//! Keeping it in one place is what makes the two protocol frontends answer the same
//! request identically.
//!
//! What deliberately stays outside: framing, socket writes, keep-alive draining and the
//! WebSocket raw-stream handoff, all of which are protocol-specific.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::BytesMut;
use tokio::io::{AsyncRead, ReadBuf};

use aether_http::limits::ServerLimits;
use aether_http::method::Method;
use aether_http::response::Response;
use aether_http::url::percent_decode;
use aether_middleware::{
    ClientAddr, CompressionMiddleware, CorsMiddleware, HostValidation, LoggerMiddleware,
    RateLimitDecision, RateLimiter, SecurityHeadersMiddleware, TrustedProxies,
};
use aether_router::trie::{MatchResult, RadixRouter};
use aether_security::{CspNonce, CsrfToken, CsrfTokens};
use aether_websocket::{HandshakeError, WebSocketLimits};

use crate::app::{
    AppStateContainer, AsyncHandlerFn, HandlerGuard, MiddlewareEntry, RequestContext, RequestParts,
    WsHandlerFn, connection_has_token, header_first, header_joined, header_value_multi,
    wrap_with_middleware,
};
use crate::transport::Stream;

/// Everything a protocol frontend needs to serve a request, shared across connections.
pub(crate) struct ServiceCore {
    pub router: RadixRouter,
    pub handlers: HashMap<u32, AsyncHandlerFn>,
    /// The routes registered to consume their request body as it arrives. Empty for an app that
    /// never calls `route_stream`, and the HTTP/1 loop reads that emptiness first: a route that is
    /// not streaming keeps the buffered path byte for byte, including its cost.
    pub streaming_routes: std::collections::HashSet<u32>,
    pub ws_handlers: HashMap<u32, WsHandlerFn>,
    /// The middleware each registered WebSocket route carries, in app-then-group order.
    pub ws_middleware: HashMap<u32, Arc<Vec<MiddlewareEntry>>>,
    /// The subprotocols each registered WebSocket route accepts, in preference order. A route
    /// absent here declared none, so any client offer is ignored rather than refused.
    pub ws_subprotocols: HashMap<u32, Arc<Vec<String>>>,
    pub state: Arc<AppStateContainer>,
    pub limits: Arc<ServerLimits>,
    pub cors: Option<Arc<CorsMiddleware>>,
    pub compression: Option<Arc<CompressionMiddleware>>,
    pub security: Option<Arc<SecurityHeadersMiddleware>>,
    pub host_validation: Option<Arc<HostValidation>>,
    pub trusted_proxies: Option<Arc<TrustedProxies>>,
    pub rate_limit: Option<Arc<RateLimiter>>,
    /// The signed double-submit layer, app-wide: the gate below refuses an unsafe request that
    /// cannot echo a token this secret signed, and a safe response hands the page one.
    pub csrf: Option<Arc<CsrfTokens>>,
    pub logger: Arc<LoggerMiddleware>,
    /// Whether the response carries the framework's `X-Request-ID` (the config's
    /// `x_request_id`).
    pub request_id: bool,
    /// The `Allow` value for `OPTIONS *`: the union of every registered route's methods, with
    /// `HEAD` only when a non-WebSocket `GET` route serves it. Computed once when the server is
    /// built, because the answer cannot change while it runs.
    pub server_allow: String,
    pub scheduling: Option<Arc<crate::scheduling::SchedulingLayer>>,
    pub websocket_limits: WebSocketLimits,
}

/// The protocol-independent view of a parsed request head.
pub(crate) struct Head {
    pub parts: RequestParts,
    /// `0` = HTTP/1.0, `1` = HTTP/1.1, `2` = HTTP/2. Only the frontends' close semantics
    /// read it; the core otherwise treats every version the same.
    pub version: Version,
    /// The transport peer, used for rate limiting and exposed to handlers.
    pub peer: SocketAddr,
    /// Whether the transport can hand a raw stream over to another protocol. HTTP/2 has no
    /// `Upgrade` mechanism (RFC 7540 §8.1 forbids forwarding connection-specific fields), so
    /// its frontend sets `false` and a registered WS route answers `400` rather than
    /// emitting a 101 that would mis-frame the stream.
    pub allows_upgrade: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    Http10,
    Http11,
    Http2,
}

impl Version {
    pub(crate) fn minor(self) -> u8 {
        match self {
            Version::Http10 => 0,
            Version::Http11 => 1,
            Version::Http2 => 2,
        }
    }
}

/// A frontend's answer to "has the peer gone?", polled while a handler runs.
///
/// Dispatch parks the connection task inside the handler deadline, so the frontend cannot keep
/// reading the socket it owns. It hands over this arm instead. An implementation that sees the
/// peer still sending settles into `Pending` for good: a peer with more bytes to send is not
/// gone, and the request deadline remains the bound for it.
pub(crate) trait PeerWatch: Send {
    fn poll_gone(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    /// Advance a streamed request body. `Ready` means the body reached its terminal state — a
    /// clean end or a failure — so dispatch stops polling this arm and waits for the handler.
    ///
    /// The default is "this frontend has no body to pump", which every frontend without one
    /// keeps: `body_pending` answers `false`, so this is never polled.
    fn poll_body(&mut self, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }

    /// Whether a streamed body is still owed to the handler.
    fn body_pending(&self) -> bool {
        false
    }

    /// The instant the body must have finished arriving by, measured from its head.
    fn body_deadline(&self) -> Option<Instant> {
        None
    }

    /// The body's own deadline fired: fail the lane so the handler can answer, and stop pumping.
    fn expire_body(&mut self) {}
}

/// Watches a peer by reading ahead into the connection's own buffer.
///
/// Peeking the socket cannot answer for TLS: an abrupt close surfaces inside rustls as an EOF
/// while a record is expected, and a clean `close_notify` is an encrypted record whose outer type
/// is application data — neither is visible on the socket. A read through the transport is the
/// only observation that separates them, and it is safe here because the bytes are not dropped:
/// they are appended to the frontend's buffer, where the next request is parsed from.
///
/// Only a read **error** means the peer is gone. End of stream means the peer will send no more
/// requests, which is exactly what a client that shuts down its write half and waits for the
/// response looks like — the framework answers such a client, so cancelling on EOF would drop a
/// response that has a reader. A connection that ends cleanly is still detected at write time, as
/// it always was; this watch is what catches the abrupt case earlier.
pub(crate) struct ReadAheadWatch<'a> {
    stream: Option<(&'a mut Stream, &'a mut BytesMut)>,
    /// Kept small: this only has to tell "some bytes" from "none", and the loop reads the rest.
    scratch: [u8; 1024],
    settled: bool,
}

impl<'a> ReadAheadWatch<'a> {
    pub(crate) fn on(stream: &'a mut Stream, pending: &'a mut BytesMut) -> Self {
        Self {
            stream: Some((stream, pending)),
            scratch: [0u8; 1024],
            settled: false,
        }
    }

    /// Stop watching: more of the client's bytes were already buffered behind this request.
    pub(crate) fn disarmed() -> Self {
        Self {
            stream: None,
            scratch: [0u8; 1024],
            settled: true,
        }
    }

    /// Whether this watch has reached its final state (an error, or EOF with nothing behind it).
    /// A caller that re-creates one per poll uses this to stop asking the socket.
    pub(crate) fn is_settled(&self) -> bool {
        self.settled
    }
}

impl PeerWatch for ReadAheadWatch<'_> {
    fn poll_gone(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.settled {
            return Poll::Pending;
        }
        let Some((stream, pending)) = self.stream.as_mut() else {
            self.settled = true;
            return Poll::Pending;
        };
        let mut buf = ReadBuf::new(&mut self.scratch);
        match Pin::new(&mut **stream).poll_read(cx, &mut buf) {
            Poll::Pending => Poll::Pending,
            // A failed read is a socket the peer can neither read from nor write to: an RST on a
            // plaintext connection, or rustls finding the stream ended without a `close_notify`.
            Poll::Ready(Err(_)) => {
                self.settled = true;
                Poll::Ready(())
            }
            // End of stream is a peer that will send no more requests, not one that cannot read:
            // a client may shut down its write half and still wait for the response. The write
            // path remains the detector for that connection, exactly as it was before this watch.
            Poll::Ready(Ok(())) => {
                self.settled = true;
                if !buf.filled().is_empty() {
                    // A live client with more to send — a pipelined request — is not gone, and
                    // its bytes must survive for the next parse.
                    pending.extend_from_slice(buf.filled());
                }
                Poll::Pending
            }
        }
    }
}

/// Which connection-side event ended a poll of [`PeerWatch`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectionWake {
    /// The peer left: the handler was aborted and there is nobody to answer.
    Gone,
    /// A streamed body reached its terminal state; the handler still owes an answer.
    BodyHandedOver,
}

/// What the caller must do next with what the core produced.
#[expect(
    clippy::large_enum_variant,
    reason = "boxing the upgrade context costs an allocation per upgrade to shrink a per-call stack value"
)]
pub(crate) enum ServeOutcome {
    /// Serve `response`, then apply the frontend's own `Connection`/keep-alive rules using
    /// `close_after_response`.
    Response {
        response: Response,
        close_after_response: bool,
    },
    /// The request is a valid WebSocket upgrade the transport can take over.
    ///
    /// `ws_memory` is the *total* the connection should be charged once the session starts
    /// (base + buffers + reassembly). The caller must re-price its registration to cover it
    /// *before* writing the `101`: once the protocol has switched there is no HTTP status
    /// left to refuse with, so a socket the budget cannot cover must not be upgraded.
    WebSocketUpgrade {
        accept: String,
        ctx: RequestContext,
        handler: WsHandlerFn,
        ws_memory: u64,
        /// The subprotocol the route agreed with this client, if the route declares any and the
        /// client offered one. The frontend echoes it in the `101`.
        protocol: Option<String>,
    },
    /// The peer left while the handler was running: the handler was aborted, no response was
    /// produced, and there is nobody left to write one to. The frontend closes what remains of
    /// the connection without writing a byte.
    ClientGone,
}

impl ServiceCore {
    /// True when some route on this app reads its body as it arrives. An app that registers none
    /// pays one empty-set check per request, and the frontend never looks at a head twice.
    pub(crate) fn has_streaming_routes(&self) -> bool {
        !self.streaming_routes.is_empty()
    }

    /// Whether the route a parsed head resolves to is a streaming one.
    ///
    /// The frontend asks *before* the body arrives, which is the whole point: it must decide
    /// whether to hand the handler a lane instead of a buffer. The same lookup runs again inside
    /// [`Self::serve`], which is what makes the two answers impossible to disagree — routing is
    /// deterministic for a method and path.
    pub(crate) fn streams_route(&self, method: Method, path: &str) -> bool {
        match self.router.find(method, path) {
            MatchResult::Found { handler_id, .. } => self.streaming_routes.contains(&handler_id),
            _ => false,
        }
    }

    /// Route, guard, dispatch and post-process one request head.
    pub(crate) async fn serve(&self, head: Head, watch: &mut dyn PeerWatch) -> ServeOutcome {
        let Head {
            mut parts,
            version,
            peer,
            allows_upgrade,
        } = head;

        // Both are clones, not moves, because the same value has to reach the handler: `parts`
        // carries the original into the `RequestContext` and the error reporter, and the core
        // keeps the copy that becomes the response's `X-Request-ID`.
        let path_str = parts.path.clone();
        let method_enum = parts.method;
        let req_id_str = parts.request_id.clone();
        // Read from the store `parts` owns instead of a deep clone: every use below happens
        // before `parts` moves into the dispatch, and the values that have to outlive that
        // move are already owned Strings. Cloning the header map costs the table plus one
        // allocation per stored field, on every request of every protocol.
        //
        // The single store the helpers below read is still the one that ends up inside the
        // `RequestContext`, so a handler and a security decision can never see different
        // header sets for one request.
        let header_multi = &parts.header_multi;

        let start_time = self.logger.start();
        // Only read what a configured feature will look at: with no compression and no CORS
        // layer these strings are produced for every request and then dropped unread, and
        // each one is a heap allocation on a hot path.
        let accept_encoding = if self.compression.is_some() {
            // Owned, not borrowed: the choice has to outlive the move of `parts` into the
            // dispatch. Only a compression-enabled app pays for it.
            header_value_multi(header_multi, "accept-encoding").map(std::borrow::Cow::into_owned)
        } else {
            None
        };
        let origin = if self.cors.is_some() {
            header_first(header_multi, "origin")
        } else {
            None
        };

        // Connection-close semantics are an HTTP/1.x concept only. In HTTP/2 a `Connection`
        // field is invalid (RFC 7540 §8.1), and honouring `close` there would let any client
        // drop a multiplexed connection that carries other live streams — so on h2 the
        // frontend owns the close decision and the core never sets it.
        let mut close_after_response = match version {
            Version::Http2 => false,
            Version::Http10 | Version::Http11 => {
                let close_requested = connection_has_token(header_multi, "close");
                // RFC 9112 §6.1 / RFC 9110 §9.6: HTTP/1.0 connections are closed after one
                // response unless the client explicitly asked for keep-alive; HTTP/1.1
                // persists unless the client asks to close.
                let keepalive_requested =
                    version == Version::Http10 && connection_has_token(header_multi, "keep-alive");
                close_requested || (version == Version::Http10 && !keepalive_requested)
            }
        };

        // 2. Route match against the radix trie.
        let mut match_result = self.router.find(method_enum, &path_str);

        // RFC 9110 §9.3.2 requires every general-purpose server to support HEAD. When no
        // HEAD route is registered, serve the GET handler and let the frontend drop the
        // payload, preserving its headers.
        if method_enum == Method::Head && !matches!(match_result, MatchResult::Found { .. }) {
            let get_result = self.router.find(Method::Get, &path_str);
            if matches!(get_result, MatchResult::Found { .. }) {
                match_result = get_result;
            }
        }

        // The caller's address, resolved once: the limiter keys on it, and a configured proxy
        // list makes it visible to handlers through the extension that backs `client_addr()`.
        // With no list, the peer is the only address the server can vouch for and nothing is
        // attached, so a request pays nothing for a feature it does not use.
        let client_ip = match &self.trusted_proxies {
            Some(trusted) => {
                let forwarded = header_joined(header_multi, "forwarded");
                let x_forwarded_for = header_joined(header_multi, "x-forwarded-for");
                let client =
                    trusted.client_ip(peer.ip(), forwarded.as_deref(), x_forwarded_for.as_deref());
                parts.extensions.insert(ClientAddr(client));
                client
            }
            None => peer.ip(),
        };

        // 3. Security rejections: host allow-list, rate limit, CSRF. All run before the handler,
        // so a request they refuse never reaches a handler, a static file, or the WebSocket
        // upgrade path. The route match above is the one routing work a refusal still pays for:
        // it is a deterministic, side-effect-free lookup whose result the CORS, OPTIONS and
        // dispatch branches below all reuse. No CSP nonce is drawn for a refusal: no dispatched
        // markup can name one, so the policy goes out with its nonce expression removed. Among the
        // checks the order is
        // by cost: the host allow-list is a header comparison, the limiter takes a lock, and CSRF
        // is the only one that does cryptographic work — verifying the cookie's HMAC and, on a
        // safe request without one, minting a token from OS entropy. A request the two cheaper
        // checks refuse never reaches it, leaving a flood to the limiter rather than to HMAC work.

        // The host value is only needed by the validator, so it is read inside its arm:
        // an app with no host allow-list should not pay a String per request for a check
        // it never runs.
        let rejection: Option<Response> = match &self.host_validation {
            Some(hv) => {
                let host_header = header_first(header_multi, "host");
                if hv.accepts(host_header.as_deref()) {
                    None
                } else {
                    hv.reject(host_header.as_deref())
                }
            }
            None => None,
        }
        .or_else(|| match &self.rate_limit {
            Some(rl) => match rl.check(client_ip, Instant::now()) {
                RateLimitDecision::Allowed => None,
                RateLimitDecision::Limited { retry_after } => {
                    // Never a zero: a client told to wait gets a real interval, so round
                    // up rather than truncate.
                    let secs = retry_after
                        .as_secs()
                        .saturating_add(u64::from(retry_after.subsec_nanos() != 0))
                        .max(1);
                    let mut res = Response::status(429, "Too Many Requests");
                    res.add_header("Retry-After", secs.to_string());
                    Some(res)
                }
            },
            None => None,
        });

        // CSRF is decided here in one place: an unsafe request is checked against the token the
        // browser echoes, and a safe one is handed the token its page must render. Computing
        // both from the same cookie means a request can never be checked against a token it was
        // not itself issued. It runs only when the two cheaper checks have not already refused:
        // a refused request has no page to protect and no cookie to refresh.
        let (csrf_refused, csrf_token, csrf_minted) = match &self.csrf {
            Some(csrf) if rejection.is_none() && !csrf.is_exempt(&path_str) => {
                let cookies = header_multi
                    .get("cookie")
                    .map_or(&[][..], |lines| lines.as_slice());
                if is_safe_method(method_enum) {
                    match csrf.valid_cookie(cookies) {
                        // Reuse the token already in the browser: two tabs opened before either
                        // response keep one cookie, and a form already on the user's screen
                        // keeps working. Rotating here would invalidate every open form.
                        Some(token) => (false, Some(token.to_owned()), false),
                        // No token, or one this server did not sign (a planted cookie): issue a
                        // fresh one. The `Set-Cookie` replaces the planted value.
                        None => match csrf.issue() {
                            Ok(token) => (false, Some(token), true),
                            // No OS entropy is no token, not a failed response: a safe response
                            // without one is still correct, and the next unsafe request is the
                            // one that reports the absence.
                            Err(_) => (false, None, false),
                        },
                    }
                } else {
                    let headers = header_multi
                        .get(csrf.header_name())
                        .map_or(&[][..], |lines| lines.as_slice());
                    if csrf.checks(cookies, headers) {
                        (false, None, false)
                    } else {
                        // Empty body, no reason, no token echoed, no cookie refreshed: nothing a
                        // page can distinguish, so nothing an attacker can iterate on.
                        (true, None, false)
                    }
                }
            }
            _ => (false, None, false),
        };
        // The token to hand the browser, only for the request that had none of its own.
        let csrf_cookie = csrf_minted.then(|| csrf_token.clone()).flatten();
        if let Some(token) = csrf_token {
            // Priced at dispatch: the map is built before the handler job is admitted, and the
            // attach is when every pre-admission entry is registered against that job.
            let heap = token.len();
            parts.extensions.insert_sized(CsrfToken::new(token), heap);
        }

        let rejection =
            rejection.or_else(|| csrf_refused.then(|| Response::status(403, "Forbidden")));

        // 3b. Server-wide OPTIONS (RFC 9112 §3.2.4, RFC 9110 §9.3.7 — and RFC 9113 §8.3.1 gives
        // HTTP/2 the same target, as a `:path` of just `*`). It names no resource, so the router
        // has nothing to match and the generic per-path responder below would answer 404. Its
        // capabilities are the union of what any route dispatches to, computed when the server was
        // built: the same 204 + `Allow` a routable path gets, widened to the whole server.
        let asterisk_options = match (method_enum, path_str.as_str()) {
            (Method::Options, "*") => {
                let mut res = Response::status(204, "No Content");
                res.add_header("Allow", self.server_allow.clone());
                Some(res)
            }
            _ => None,
        };

        // 3c. CONNECT. HTTP/1 accepts only an authority-form target for it (§3.2.3); HTTP/2 carries
        // the same host:port in `:authority` and no `:path` (RFC 9113 §8.5). This framework has no
        // tunnel, so RFC 9110 §9.3.6 decides: such a server says 405 or 501, and 501 is the honest
        // one — a 404 would blame the named host and port for a capability this server lacks.
        let connect_refusal =
            (method_enum == Method::Connect).then(|| Response::status(501, "Not Implemented"));

        // 4. CORS preflight. Only when the application has not registered its own OPTIONS
        // route for this path — the previous blanket 204 made user-registered OPTIONS
        // handlers unreachable.
        let mut preflight: Option<Response> = None;
        // Kept nested rather than collapsed into a three-condition chain: the body is a
        // 30-line block with its own comments, and re-indenting it to satisfy a style lint
        // would make the CORS preflight decision harder to read in review.
        #[expect(clippy::collapsible_if, reason = "nesting kept for a commented block")]
        if method_enum == Method::Options
            && path_str != "*"
            && !matches!(match_result, MatchResult::Found { .. })
        {
            if let Some(ref cors_mw) = self.cors {
                let requested_method = header_first(header_multi, "access-control-request-method");
                let requested_headers =
                    header_first(header_multi, "access-control-request-headers");

                if origin.is_some() && requested_method.is_some() {
                    // A preflight only succeeds if the actual request would have been
                    // routable.
                    let routable = requested_method
                        .as_deref()
                        .and_then(|m| Method::from_bytes(m.as_bytes()))
                        .map(|m| {
                            matches!(self.router.find(m, &path_str), MatchResult::Found { .. })
                        })
                        .unwrap_or(false);

                    let mut r = Response::status(204, "No Content");
                    // Run the middleware even when the requested method is not routable:
                    // the refusal is decided by the same request fields as an approval, and
                    // only the middleware knows which fields those are.
                    let allowed = cors_mw.apply_preflight(
                        &mut r,
                        origin.as_deref(),
                        requested_method.as_deref(),
                        requested_headers.as_deref(),
                    ) && routable;
                    preflight = Some(if allowed {
                        r
                    } else {
                        let mut refused = Response::status(403, "Forbidden");
                        // `Vary` is the one field that survives a refusal: without it a shared
                        // cache can store this 403 and replay it for a request that would have
                        // been approved (RFC 9111 §4.1).
                        for (name, value) in &r.headers {
                            if name.eq_ignore_ascii_case("vary") {
                                refused.headers.push((*name, value.clone()));
                            }
                        }
                        refused
                    });
                }
            }
        }

        // 5. Generic OPTIONS. RFC 9110 §9.3.7 makes OPTIONS a question about the target's
        // capabilities, so a routable path must answer it even when no OPTIONS handler was
        // registered — the alternatives are a `405` for a method the server actually supports,
        // or a `404` for a path that exists. A registered OPTIONS route is `Found` and never
        // reaches here, and the CORS preflight above already answered its own case; this only
        // runs when the router reports the path exists but the method is not registered.
        let options_fallback = match (method_enum, &match_result) {
            (Method::Options, MatchResult::MethodNotAllowed { allowed_bitmask }) => {
                let mut res = Response::status(204, "No Content");
                res.add_header("Allow", self.allow_header(&path_str, *allowed_bitmask));
                Some(res)
            }
            _ => None,
        };

        // A policy that names `{nonce}` gets a fresh value only where a handler is about to run,
        // because only dispatched markup can render it. Every answer the framework writes on its
        // own — a security refusal, a routing 404 or 405, a preflight, the generic OPTIONS, an
        // upgrade the frontend forbids — skips the OS-entropy draw and goes out with the nonce
        // expression removed (the middleware's documented fail-closed rendering). An app whose
        // policy names no placeholder pays nothing either way.
        let mut csp_nonce: Option<CspNonce> = None;

        let is_preflight = preflight.is_some();
        let mut response = if let Some(r) = rejection
            .or(asterisk_options)
            .or(connect_refusal)
            .or(preflight)
            .or(options_fallback)
        {
            r
        } else {
            match match_result {
                MatchResult::Found { handler_id, params } => {
                    // Only two answers this branch writes are not dispatched: a WebSocket route's
                    // 405, and an upgrade the frontend forbids (the h2 400). Neither has a chain
                    // or a handler that could render a nonce. On HTTP/1.1 a GET without upgrade
                    // fields does reach the chain, so its 400 is a dispatched answer.
                    let reaches_dispatch = !self.ws_handlers.contains_key(&handler_id)
                        || (method_enum == Method::Get && allows_upgrade);
                    if reaches_dispatch
                        && let Some(security) = &self.security
                        && security.needs_nonce()
                    {
                        csp_nonce = match CspNonce::generate() {
                            Ok(nonce) => Some(nonce),
                            // No OS entropy means no unpredictable token, and a guessable nonce is
                            // worse than an error page: it makes the policy look enforced while
                            // allowing injected markup. Refuse the request instead of serving a
                            // page that claims protection.
                            Err(_) => {
                                return ServeOutcome::Response {
                                    response: Response::status(500, "Internal Server Error"),
                                    close_after_response,
                                };
                            }
                        };
                    }
                    let mut param_map = HashMap::with_capacity(params.len());
                    for p in params {
                        // Routing stays on the raw path so `%2F` cannot change segment
                        // boundaries; only the captured value is decoded.
                        param_map.insert(p.key.to_string(), percent_decode(p.value));
                    }
                    if let Some(ws_handler_ref) = self.ws_handlers.get(&handler_id) {
                        // Two layers with two responsibilities, deliberately not merged:
                        // the *router* decides which methods a registered route accepts, and
                        // `app.ws` registers `GET` only, so a non-`GET` is answered `405` +
                        // `Allow` before this branch ever runs. This guard is the *serving
                        // core's* own promise that nothing which slipped past an earlier check
                        // reaches the upgrade path. It is not decorative: with the route also
                        // registered for `POST`, removing it handed the `POST` a `101`.
                        if method_enum != Method::Get {
                            let mut res = Response::status(405, "Method Not Allowed");
                            // `HEAD` is *not* in this list, unlike an HTTP route's: this
                            // branch is what refuses a non-`GET`, and the `HEAD`-serves-`GET`
                            // rule below never reaches a WebSocket route. `OPTIONS` is, because
                            // the generic responder answers it before this branch can.
                            res.add_header("Allow", "GET, OPTIONS");
                            res
                        } else if !allows_upgrade {
                            Response::status(400, "Bad Request")
                        } else {
                            let ws = self.websocket_limits;
                            // An upgrade request is still a request, so the route's middleware
                            // has to run for it: otherwise an auth chain that guards every HTTP
                            // route leaves every `app.ws` route wide open.
                            let handed_off = Arc::new(Mutex::new(None::<RequestContext>));
                            // The terminal cannot answer — the `101` is synthesized by the
                            // frontend — so it takes the request out of the response path and
                            // returns the one status that means "the chain let me through".
                            let terminal: AsyncHandlerFn = {
                                let handed_off = Arc::clone(&handed_off);
                                Arc::new(move |ctx| {
                                    *handed_off.lock().unwrap() = Some(ctx);
                                    Box::pin(async { Response::status(101, "Switching Protocols") })
                                })
                            };
                            let middleware = self
                                .ws_middleware
                                .get(&handler_id)
                                .cloned()
                                .unwrap_or_default();
                            let guard = if middleware.is_empty() {
                                terminal
                            } else {
                                wrap_with_middleware(terminal, middleware.as_slice())
                            };
                            let Some(chain_response) = self
                                .dispatch(
                                    &guard,
                                    parts.with_params(param_map),
                                    &self.state,
                                    watch,
                                    csp_nonce.as_ref(),
                                )
                                .await
                            else {
                                return ServeOutcome::ClientGone;
                            };
                            // Identity is the sentinel's status, not a flag set inside the
                            // terminal: a guard that calls `next` and then replaces its answer
                            // (a post-check refusal, a panic after `next`, a timeout after
                            // `next`) produces something other than `101`, and that answer is
                            // the response. A bare flag would have upgraded the socket anyway
                            // and thrown the guard's answer away.
                            let handoff = handed_off
                                .lock()
                                .unwrap()
                                .take()
                                .filter(|_| chain_response.status == 101);
                            match handoff {
                                // A `101` with no recorded handoff was not produced by the
                                // terminal, so no protocol transition happened: the guard built
                                // it. Serving it would tell the client to start WebSocket framing
                                // on a socket this server still parses as HTTP/1, so the claim is
                                // replaced by an honest 5xx. Every other status the chain returns
                                // is served as written; a `101` from outside this branch meets the
                                // same refusal at the core's single response exit.
                                None if chain_response.status == 101 => {
                                    Response::status(500, "Internal Server Error")
                                }
                                None => chain_response,
                                Some(ctx) => {
                                    let upgrade = ctx.header_values("upgrade").join(", ");
                                    let connection = ctx.header_values("connection").join(", ");
                                    let sec_version =
                                        ctx.header("sec-websocket-version").map(str::to_string);
                                    let key = ctx.header("sec-websocket-key").map(str::to_string);

                                    match aether_websocket::validate_handshake(
                                        "GET",
                                        if upgrade.is_empty() {
                                            None
                                        } else {
                                            Some(&upgrade)
                                        },
                                        if connection.is_empty() {
                                            None
                                        } else {
                                            Some(&connection)
                                        },
                                        sec_version.as_deref(),
                                        key.as_deref(),
                                    ) {
                                        Ok(accept) => {
                                            // RFC 6455 §4.2.2: the server picks at most one of
                                            // the client's offered subprotocols and echoes it.
                                            // A route that declares none ignores the offer, which
                                            // is the pre-negotiation behaviour every existing
                                            // route keeps. A route that declares some must refuse
                                            // an offer it cannot meet: a client that asked for a
                                            // subprotocol is required to fail the connection when
                                            // the `101` names none, so answering `101` would only
                                            // move that failure somewhere the client cannot
                                            // explain. Only a route-declared name is ever echoed.
                                            let mut unmatched = false;
                                            let protocol = match self
                                                .ws_subprotocols
                                                .get(&handler_id)
                                            {
                                                None => None,
                                                Some(supported) => {
                                                    let declared: Vec<&str> = supported
                                                        .iter()
                                                        .map(String::as_str)
                                                        .collect();
                                                    let offered_values =
                                                        ctx.header_values("sec-websocket-protocol");
                                                    let offered = if offered_values.is_empty() {
                                                        None
                                                    } else {
                                                        Some(offered_values.join(", "))
                                                    };
                                                    match aether_websocket::select_subprotocol(
                                                        offered.as_deref(),
                                                        &declared,
                                                    ) {
                                                        aether_websocket::ProtocolSelection::Agreed(
                                                            name,
                                                        ) => Some(name.to_string()),
                                                        aether_websocket::ProtocolSelection::NoMatch => {
                                                            unmatched = true;
                                                            None
                                                        }
                                                        aether_websocket::ProtocolSelection::NotOffered => {
                                                            None
                                                        }
                                                    }
                                                }
                                            };
                                            if unmatched {
                                                Response::status(400, "Bad Request")
                                            } else {
                                                // A WS session keeps its reassembly buffers for
                                                // the whole negotiated life (default 16 MiB
                                                // message + 16 MiB frame), an order of magnitude
                                                // above the HTTP charge. This is the session's own
                                                // headroom on top of what the connection already
                                                // holds; the frontend adds it to its re-pricing.
                                                let ws_memory = ws.max_message_bytes as u64
                                                    + ws.max_frame_bytes as u64
                                                    + 16 * 1024;
                                                return ServeOutcome::WebSocketUpgrade {
                                                    accept,
                                                    ctx,
                                                    // `WsHandlerFn` already *is* an
                                                    // `Arc<dyn Fn..>` and the map holds one per
                                                    // route, so the entry is `&Arc<WsHandlerFn>`
                                                    // and cloning it is the cheap refcount bump
                                                    // the frontend needs.
                                                    handler: ws_handler_ref.clone(),
                                                    ws_memory,
                                                    protocol,
                                                };
                                            }
                                        }
                                        Err(HandshakeError::UnsupportedVersion) => {
                                            let mut res = Response::status(426, "Upgrade Required");
                                            res.add_header("Sec-WebSocket-Version", "13");
                                            res
                                        }
                                        Err(_) => Response::status(400, "Bad Request"),
                                    }
                                }
                            }
                        }
                    } else if let Some(handler_ref) = self.handlers.get(&handler_id) {
                        let Some(response) = self
                            .dispatch(
                                handler_ref,
                                parts.with_params(param_map),
                                &self.state,
                                watch,
                                csp_nonce.as_ref(),
                            )
                            .await
                        else {
                            return ServeOutcome::ClientGone;
                        };
                        response
                    } else {
                        Response::status(500, "Internal Server Error")
                    }
                }
                MatchResult::MethodNotAllowed { allowed_bitmask } => {
                    let allow_header = self.allow_header(&path_str, allowed_bitmask);
                    let mut res = Response::status(405, "Method Not Allowed");
                    res.add_header("Allow", allow_header);
                    res
                }
                MatchResult::NotFound => Response::status(404, "Not Found"),
            }
        };

        // A `101` is only true when the frontend synthesizes it for a handoff that happened, and
        // every response reaching this point came from somewhere else: a guard, a handler, the
        // router. Serving such a claim would move the client's framing while this server keeps
        // parsing the same bytes as HTTP/1 or HTTP/2 — and `h2` does not merely mis-serve it, it
        // resets the stream with a protocol error, so the peer gets no answer at all. The answer
        // is the one the WebSocket branch above already gives a guard's `101`, applied here so it
        // holds for both protocols and for every author of the claim.
        //
        // The refusal covers every informational status, not just `101`. The only `1xx` this
        // server writes by itself — `100 Continue` — goes straight to the stream in the HTTP/1
        // read loop before a response exists, so no `1xx` arriving here marks a transition this
        // server performed.
        if (100..200).contains(&response.status) {
            response = Response::status(500, "Internal Server Error");
        }

        self.post_process(
            &mut response,
            &mut close_after_response,
            PostFix {
                req_id: req_id_str,
                origin: origin.as_deref(),
                accept_encoding: accept_encoding.as_deref(),
                is_preflight,
                method: method_enum,
                path: &path_str,
                logger_start: start_time,
                response_can_close: version != Version::Http2,
                // The bare token, never the quoted source expression: the middleware owns the
                // quoting, so a policy that writes `{nonce}` unquoted is rendered the same way.
                csp_nonce: csp_nonce.as_ref().map(CspNonce::as_str),
                // A token to hand the browser, only when this request had none it was issued.
                csrf: csrf_cookie.as_deref(),
            },
        );

        ServeOutcome::Response {
            response,
            close_after_response,
        }
    }

    /// Run a handler with the same admission, timeout, disconnect and panic mapping on every
    /// protocol.
    ///
    /// `None` means the peer left first: the handler was aborted and there is no response to
    /// write, which the caller turns into [`ServeOutcome::ClientGone`].
    async fn dispatch(
        &self,
        handler: &AsyncHandlerFn,
        parts: RequestParts,
        state: &Arc<AppStateContainer>,
        watch: &mut dyn PeerWatch,
        nonce: Option<&CspNonce>,
    ) -> Option<Response> {
        let handler = Arc::clone(handler);
        // Handler execution admission (opt-in scheduling): bounded concurrent handlers,
        // 503 on overflow. Handlers are indivisible — never preempted.
        let body_mem = parts.body.len() as u64 + 16_384;
        let guard: HandlerGuard = match &self.scheduling {
            Some(l) => match l.admit(aether_scheduler::NewJob::handler(body_mem)).await {
                Ok(g) => Some(g),
                Err(_) => {
                    let mut r = Response::status(503, "Service Unavailable");
                    r.headers.push(("Retry-After", "1".to_string()));
                    return Some(r);
                }
            },
            None => None,
        };
        // Built after admission so the map can be pointed at this request's registration: a value
        // a middleware attaches is charged against the job that is live while the value is live,
        // and the charge disappears when the job is released.
        let mut ctx = RequestContext::from_parts(parts, state);
        if let (Some(l), Some(g)) = (&self.scheduling, &guard) {
            ctx.attach_extension_charge(Arc::clone(l), g.id());
        }
        // After the charge is attached, so the entry is priced against this request's job. The
        // value is the same token the response header will carry — a handler that renders a
        // different one renders a value the policy does not allow.
        if let Some(nonce) = nonce {
            ctx.extensions_mut().insert(nonce.clone());
        }
        let request_serial = ctx.request_serial();
        let mut task_handle = tokio::spawn(async move {
            // Success must count as completed so handler runs feed the adaptive baselines;
            // on panic or abort the guard still releases via Drop.
            //
            // The scope has to live inside the spawned task: a task-local does not cross
            // `tokio::spawn`, and the handler's error conversion runs in here.
            let result = crate::diagnostics::within_request(request_serial, handler(ctx)).await;
            if let Some(g) = guard {
                g.complete();
            }
            result
        });
        let abort_handle = task_handle.abort_handle();
        // The handler deadline is built once and reused: a body that streams in must not restart
        // the handler's budget on every pump step.
        let handler_deadline = tokio::time::sleep(self.limits.request_timeout);
        tokio::pin!(handler_deadline);
        // The body's own budget: `receive_timeout` counted from the head, the same one the
        // buffered path applies to a request that has not finished arriving. It fails the lane
        // rather than aborting the handler, because the handler is still the party that answers.
        let mut body_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = watch
            .body_deadline()
            .map(|at| Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(at))));
        let body_pending = std::sync::atomic::AtomicBool::new(watch.body_pending());
        // One future for the connection side: the peer's departure, the body's progress and the
        // body's deadline all borrow the watch, so they are polled together rather than as three
        // select arms competing for the same `&mut`.
        let outcome = loop {
            let connection = std::future::poll_fn(|cx| {
                if body_pending.load(Ordering::Relaxed)
                    && let Some(deadline) = body_deadline.as_mut()
                    && deadline.as_mut().poll(cx).is_ready()
                {
                    body_deadline = None;
                    body_pending.store(false, Ordering::Relaxed);
                    watch.expire_body();
                }
                // Gone first: once the peer has left, a body it is still owing can never arrive,
                // and a response would be written to a socket nobody is reading.
                if watch.poll_gone(cx).is_ready() {
                    return Poll::Ready(ConnectionWake::Gone);
                }
                if body_pending.load(Ordering::Relaxed) && watch.poll_body(cx).is_ready() {
                    body_pending.store(false, Ordering::Relaxed);
                    body_deadline = None;
                    return Poll::Ready(ConnectionWake::BodyHandedOver);
                }
                Poll::Pending
            });
            tokio::select! {
                biased;
                wake = connection => {
                    if wake == ConnectionWake::Gone {
                        abort_handle.abort();
                        return None;
                    }
                    // The body reached its terminal state (or its deadline fired) and the handler
                    // is still running: keep waiting for it under the handler deadline.
                }
                result = &mut task_handle => break Some(result),
                _ = &mut handler_deadline => break None,
            }
        };
        match outcome {
            Some(Ok(r)) => Some(r),
            Some(Err(_)) => Some(Response::status(500, "Internal Server Error")),
            None => {
                abort_handle.abort();
                let mut r = Response::status(504, "Gateway Timeout");
                r.headers.push(("Connection", "close".to_string()));
                Some(r)
            }
        }
    }

    /// The `Allow` value for a 405 or an OPTIONS answer on `path`.
    ///
    /// The registered bits are the starting point, not the answer: `OPTIONS` is served on every
    /// routable path, and `HEAD` rides along with `GET` — except on a WebSocket route, whose
    /// upgrade refuses every non-`GET` before the handler, so listing an inherited `HEAD` there
    /// would advertise a method that is answered `405`.
    fn allow_header(&self, path: &str, allowed_bitmask: u64) -> String {
        let is_websocket = matches!(
            self.router.find(Method::Get, path),
            MatchResult::Found { handler_id, .. } if self.ws_handlers.contains_key(&handler_id)
        );
        let head_served = !is_websocket && allowed_bitmask & Method::Get.bitmask() != 0;
        aether_router::trie::RadixRouter::allow_header_for_target(allowed_bitmask, head_served)
    }

    /// The response post-processing every protocol applies, plus the close decision it can
    /// derive from the response itself. Emitting the HTTP/1 `Connection` header stays in
    /// that frontend.
    fn post_process(
        &self,
        response: &mut Response,
        close_after_response: &mut bool,
        fix: PostFix<'_>,
    ) {
        // One request has one identifier: the value here is the one the error reporter names,
        // so a handler or middleware that published its own must not leave a second value
        // beside it.
        if self.request_id {
            response
                .headers
                .retain(|(name, _)| !name.eq_ignore_ascii_case("x-request-id"));
            response.headers.push(("X-Request-ID", fix.req_id));
        }

        if !fix.is_preflight
            && let Some(ref c) = self.cors
        {
            c.apply_for_origin(response, fix.origin);
        }
        // A token is issued on a safe response that had none, and never on a refusal: the
        // refusal's job is to leave the browser with exactly what it had. Not `HttpOnly` by
        // design — a script on the page has to read the cookie to echo it in the header.
        if !fix.is_preflight
            && response.status < 400
            && let (Some(token), Some(csrf)) = (fix.csrf, &self.csrf)
        {
            response.add_cookie(
                csrf.cookie_name(),
                token,
                false,
                csrf.secure(),
                csrf.same_site(),
                csrf.max_age(),
            );
        }
        if let Some(ref s) = self.security {
            s.apply_with_nonce(response, fix.csp_nonce);
        }
        if let Some(ref comp) = self.compression {
            comp.apply(response, fix.accept_encoding);
        }
        // A handler or framework error can advertise `Connection: close` itself (e.g. the
        // 504 that aborts a timed-out handler). The close decision above was computed from
        // the *request* alone, so without this the server would emit `Connection: close`
        // and then keep reading on a socket it just told the client to abandon — a framing
        // split. On HTTP/2 the field carries no meaning, so the response cannot close it.
        if !*close_after_response
            && fix.response_can_close
            && response.headers.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("connection")
                    && v.split(',')
                        .map(|t| t.trim_matches(|c| c == ' ' || c == '\t'))
                        .any(|t| t.eq_ignore_ascii_case("close"))
            })
        {
            *close_after_response = true;
        }

        if self.logger.enabled {
            self.logger.log(
                &format!("{:?}", fix.method),
                fix.path,
                response.status,
                fix.logger_start,
            );
        }
    }
}

/// Inputs to [`ServiceCore::post_process`].
struct PostFix<'a> {
    /// The id the core generated for this request, moved rather than copied into the
    /// response's `X-Request-ID` field.
    req_id: String,
    origin: Option<&'a str>,
    accept_encoding: Option<&'a str>,
    is_preflight: bool,
    method: Method,
    path: &'a str,
    logger_start: Instant,
    /// `false` on HTTP/2, where a `Connection` field in a response is invalid and a
    /// response can never decide to close a multiplexed connection.
    response_can_close: bool,
    /// This request's CSP nonce, if its policy asked for one.
    csp_nonce: Option<&'a str>,
    /// The CSRF token this response must set as a cookie, when the request carried none.
    csrf: Option<&'a str>,
}

/// RFC 9110 §9.2.1 safe methods: a page may make the browser send one cross-origin without the
/// target's consent, so a token check on them protects nothing and breaks every link and probe.
fn is_safe_method(method: Method) -> bool {
    matches!(
        method,
        Method::Get | Method::Head | Method::Options | Method::Trace
    )
}
