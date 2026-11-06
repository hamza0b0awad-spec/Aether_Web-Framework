use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
use aether_core::error::AetherError;
use aether_http::limits::ServerLimits;
use aether_http::method::Method;
use aether_http::response::Response;
use aether_http::url::{form_decode, form_decode_cow};
use aether_middleware::{
    ClientAddr, CorsMiddleware, HostValidation, LoggerMiddleware, RateLimitConfig, RateLimiter,
    SecurityHeadersMiddleware, TrustedProxies,
};
use aether_router::trie::RadixRouter;
use aether_security::CsrfTokens;
use aether_websocket::WebSocket;
use bytes::{Bytes, BytesMut};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::signal;

use crate::config::AetherConfig;
use crate::handler::Handler;
use crate::static_dir::{StaticDirOptions, StaticRequest};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// How long a live WebSocket session is given to end on its own after shutdown is requested.
///
/// The socket holds the same shutdown flag and writes a 1001 close frame on its next `recv`, so
/// a handler sitting in its receive loop — where a WebSocket handler spends almost all of its
/// life — returns within a frame write. The bound exists for the other shape: a handler that
/// never comes back to `recv` would otherwise hold the drain for its own duration, so after this
/// the session is aborted exactly as it was before the frame existed. It is deliberately not
/// `write_timeout`: a peer that cannot take a close frame is gone, and shutdown should not wait
/// a whole write budget to learn that.
const WS_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// The identifier `X-Request-ID` and the logger both carry for one request, plus its numeric
/// half.
///
/// Shared by the frontends so a request is numbered the same whichever protocol it arrived on —
/// an HTTP/2 request that consumed a number would otherwise make the HTTP/1 log look like it
/// skipped ids. The number travels with the request so a redacted error can name the same id
/// without formatting a string on the success path.
pub(crate) fn next_request_id() -> (u64, String) {
    let serial = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    (serial, format!("req_{serial:012x}"))
}

/// Trait converting various types into a unified HTTP Response.
///
/// One rule governs the body: build it with `Bytes::from` (from a `Vec<u8>`, a `String`, or a
/// `&'static [u8]` via `Bytes::from_static`), not with `Bytes::from_owner`. HTTP/2 prices a stream
/// by the allocation its response body retains, and it recovers that allocation with
/// `Bytes::try_into_mut` — which `bytes` never allows for an owner-backed block, even for the sole
/// handle, because handing an owner's buffer back would require copying it. An `from_owner` body is
/// therefore served correctly but held unpriced, and the memory budget under-counts the stream by
/// exactly that heap. Every body this framework produces follows the rule;
/// `every_body_this_framework_produces_is_a_block_the_budget_can_name` keeps them on it.
pub trait IntoResponse: Send {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        Response::text(self)
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        let bytes = Bytes::from(self);
        Response::bytes(bytes, "text/plain; charset=utf-8")
    }
}

impl IntoResponse for (u16, &'static str) {
    fn into_response(self) -> Response {
        Response::status(self.0, self.1)
    }
}

impl IntoResponse for (u16, String) {
    fn into_response(self) -> Response {
        let mut res = Response::status(self.0, aether_http::response::canonical_reason(self.0));
        res.headers
            .push(("Content-Type", "text/plain; charset=utf-8".to_string()));
        res.headers
            .push(("Content-Length", self.1.len().to_string()));
        res.body = Bytes::from(self.1);
        res
    }
}

/// A handler error that decides the HTTP status its own response carries.
///
/// The blanket `Result` response used to bound `E: std::fmt::Display` and answer *every* `Err`
/// with 400, so it guessed a client mistake out of an internal failure and monitoring could not
/// trust a 4xx. This trait is how an error type joins that contract: it states the status, and
/// the framework stops guessing it.
///
/// Two omissions are deliberate and both are load-bearing:
/// * It is **not** [`IntoResponse`], and the blanket does not bound `E: IntoResponse`. `&'static
///   str` and `String` already implement `IntoResponse` as `text/plain` with status 200, so that
///   bound would answer `Ok` on an `Err` — `Err("database is unreachable")` would reach the caller
///   as a 200.
/// * There is **no** blanket impl over `Display`. Restoring one would put the guess back. A
///   foreign error type (`sqlx::Error` and friends) joins by wrapping it in a local type that
///   implements this trait; that is a compile-time error for whoever returns it unwrapped, never a
///   silently wrong status.
pub trait IntoResponseError: Send {
    /// The status the response carries.
    ///
    /// The blanket [`IntoResponse`] impl for `Result<T, E>` accepts only `400..=599`. A value
    /// outside that range answers `500` — with code `E5002`, the canonical phrase and no
    /// detail — rather than publishing a success or a status line a client cannot place.
    fn error_status(&self) -> u16;

    /// The `code` field of the JSON body. Defaults to the class of [`error_status`](Self::error_status)
    /// so a 5xx cannot answer with a 4xx code — the contradiction `E5002`-on-a-400 used to ship.
    fn error_code(&self) -> ErrorCode {
        let status = self.error_status();
        if status >= 500 {
            return ErrorCode::E5002InternalServerError;
        }
        match status {
            404 => ErrorCode::E2002RouteNotFound,
            405 => ErrorCode::E2003MethodNotAllowed,
            413 | 431 => ErrorCode::E3002PayloadLimitExceeded,
            417 => ErrorCode::E3003UnsupportedExpectation,
            _ => ErrorCode::E4003HandlerReturnedError,
        }
    }

    /// The `error` field of the JSON body. `None` redacts it and the status phrase is sent
    /// instead, because an internal message can name a table, a path or a port.
    ///
    /// A redacted message is not lost: a 5xx response hands [`Self::redacted_detail`] to the
    /// [error reporter](crate::diagnostics::set_error_reporter), if one is installed.
    fn error_detail(&self) -> Option<String> {
        None
    }

    /// The message to hand the [error reporter](crate::diagnostics::set_error_reporter) when a
    /// 5xx response redacts it, alongside the request's `X-Request-ID`.
    ///
    /// Defaults to `None`, so an error type that redacts also stays silent unless it opts in.
    /// This is a diagnostics channel, not a wire contract: whatever it returns never reaches the
    /// response body. It is read only when [`Self::error_detail`] returned `None` for a 5xx, so
    /// a 4xx — whose detail is already in the body — is never reported twice.
    fn redacted_detail(&self) -> Option<String> {
        None
    }
}

impl IntoResponseError for &'static str {
    fn error_status(&self) -> u16 {
        400
    }

    fn error_detail(&self) -> Option<String> {
        Some((*self).to_string())
    }
}

impl IntoResponseError for String {
    fn error_status(&self) -> u16 {
        400
    }

    fn error_detail(&self) -> Option<String> {
        Some(self.clone())
    }
}

impl IntoResponseError for AetherError {
    fn error_status(&self) -> u16 {
        match self {
            AetherError::RouteNotFound => 404,
            AetherError::MethodNotAllowed => 405,
            AetherError::UnsupportedExpectation => 417,
            AetherError::BodyLimitExceeded => 413,
            AetherError::HeaderTooLarge => 431,
            AetherError::Timeout => 504,
            AetherError::DependencyCycleDetected(_) | AetherError::Io(_) => 500,
            AetherError::MalformedRequest
            | AetherError::ValidationFailed(_)
            | AetherError::Serialization(_)
            | AetherError::Custom(_) => 400,
        }
    }

    fn error_code(&self) -> ErrorCode {
        AetherError::error_code(self)
    }

    fn error_detail(&self) -> Option<String> {
        if self.error_status() >= 500 {
            return None;
        }
        Some(self.to_string())
    }

    fn redacted_detail(&self) -> Option<String> {
        if self.error_status() >= 500 {
            return Some(self.to_string());
        }
        None
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: IntoResponseError,
{
    fn into_response(self) -> Response {
        match self {
            Ok(val) => val.into_response(),
            Err(err) => {
                // The message is caller-supplied and frequently includes user input (a
                // rejected field, an unknown id). Writing it straight into a JSON template
                // produced malformed bodies for any message containing `"` or a newline, and
                // let the error text break out of the value. serde_json escapes it properly.
                let declared = err.error_status();
                let in_range = (400..=599).contains(&declared);
                let status = if in_range { declared } else { 500 };
                // A type that named a non-error status wrote its message for that status, so
                // neither the message nor its code class survives into the replacing 500.
                let code = if in_range {
                    err.error_code()
                } else {
                    ErrorCode::E5002InternalServerError
                };
                let reason = aether_http::response::canonical_reason(status);
                let detail = if in_range { err.error_detail() } else { None };
                if status >= 500
                    && detail.is_none()
                    && let Some(diagnostic) = err.redacted_detail()
                {
                    crate::diagnostics::report_redacted_error(status, code.code_str(), &diagnostic);
                }
                let detail = detail.unwrap_or_else(|| reason.to_string());
                let body = serde_json::json!({
                    "error": detail,
                    "code": code.code_str(),
                })
                .to_string();
                Response::json_status(status, reason, Bytes::from(body))
            }
        }
    }
}

/// حاوية الحالة العامة المشتركة للتطبيق والآمنة للاستخدام عبر الـ Threads المتعددة (Thread-Safe Dependency Container).
///
/// تستخدم لتطبيق نمط حقن التبعيات (Dependency Injection) بالاعتماد على `TypeId`،
/// مما يسمح بمشاركة اتصالات قواعد البيانات، المخازن المؤقتة، والخدمات المشتركة بين جميع المعالجات.
#[derive(Default, Clone)]
pub struct AppStateContainer {
    items: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl AppStateContainer {
    /// إنشاء حاوية جديدة فارغة لتسجيل التبعيات والخدمات.
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    /// تسجيل خدمة أو كائن حالة جديد داخل الحاوية مع تغليفه بـ `Arc`.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.items.insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// استرجاع مرجع ذري `Arc<T>` للخدمة المطلوبة بناءً على نوعها في وقت التشغيل بـ $O(1)$.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.items
            .get(&TypeId::of::<T>())
            .and_then(|any| any.clone().downcast::<T>().ok())
    }

    /// Does this container hold nothing? A group that registered no state costs its routes
    /// nothing, which is what keeps a scoped registration equal to a plain one.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Field lines received for one header name. Two are inline because a single line is the
/// overwhelming norm and a doubled field (`Cookie`, `Connection`) is the common exception,
/// so materialising a request head allocates per line rather than per line plus one `Vec`
/// per distinct name. `aether-http` already uses `SmallVec` for the borrowed side of the
/// same data structure.
pub(crate) type HeaderLines = smallvec::SmallVec<[String; 2]>;

/// The protocol-independent request fields, exactly the shape [`RequestContext`] owns.
///
/// A protocol frontend builds one from its own framing (HTTP/1.1 bytes, HTTP/2 headers)
/// and the serving core turns it into a `RequestContext` through
/// [`RequestContext::from_parts`]. That is the single place the two frontends are
/// guaranteed to agree: same case-insensitive first-line dedup, same lowercase multi
/// store, same parameter decoding, so a handler cannot see a different request depending
/// on which protocol carried it.
pub(crate) struct RequestParts {
    pub raw_uri: String,
    pub path: String,
    pub query_str: Option<String>,
    pub method: Method,
    /// Captured path parameters, already percent-decoded. The core fills this from the
    /// router match, which is why [`Self::with_params`] replaces rather than merges.
    pub param_map: HashMap<String, String>,
    pub header_multi: HashMap<String, HeaderLines>,
    pub body: Bytes,
    /// The lane a streaming route reads its body from; `None` on every buffered request.
    ///
    /// `Some` means the frontend dispatched this request as soon as its head was complete, so
    /// [`Self::body`] is empty by construction and the body arrives through
    /// [`RequestContext::body_stream`]. Both frontends set it for a `route_stream` route — the
    /// HTTP/1.1 loop reads the socket into the lane, the HTTP/2 pump reads DATA frames into it —
    /// and every other route is buffered, with `body` holding the bytes.
    pub body_stream: Option<crate::request_body::RequestBody>,
    pub request_id: String,
    /// The numeric half of [`Self::request_id`], kept so a redacted error can reconstruct the
    /// id on the failure path instead of copying the string on every request.
    pub request_serial: u64,
    pub peer_addr: Option<SocketAddr>,
    /// Values middleware attached for the handler. Empty until something is inserted, so the
    /// per-request cost is zero allocations.
    pub extensions: crate::extensions::Extensions,
}

impl RequestParts {
    pub(crate) fn with_params(mut self, param_map: HashMap<String, String>) -> Self {
        self.param_map = param_map;
        self
    }
}

/// One `static_dir` registration, shared by the catch-all route and the tree's root route.
struct StaticTree {
    root: Arc<std::path::PathBuf>,
    prefix: Arc<str>,
    options: StaticDirOptions,
}

impl StaticTree {
    async fn answer(&self, req: RequestContext, relative: String) -> Response {
        let if_none_match = req.header("if-none-match").map(str::to_string);
        let if_range = req.header("if-range").map(str::to_string);
        let range = req.header("range").map(str::to_string);
        let accept_encoding = req.header("accept-encoding").map(str::to_string);
        crate::static_dir::serve_from_dir(
            &self.root,
            &self.prefix,
            &StaticRequest {
                relative: &relative,
                path: &req.path,
                if_none_match: if_none_match.as_deref(),
                if_range: if_range.as_deref(),
                range: range.as_deref(),
                accept_encoding: accept_encoding.as_deref(),
            },
            self.options,
        )
        .await
    }
}

/// The per-handler execution guard owned by a serving task. `dispatch` takes it out of
/// the scheduler and completes it when the handler returns.
pub(crate) type HandlerGuard = Option<crate::scheduling::AdmissionGuard>;

/// سياق الطلب (`RequestContext`) الممرر لكل معالج مسار (Route Handler).
///
/// يحتوي على جميع بيانات الطلب المنقاة مع ملكية منضبطة تمنع تسريبات الذاكرة (No `Box::leak`).
/// يوفر دوال مساعدة لاستخراج المعلمات، الترويسات، وفك تشفير الـ JSON بسرعة فائقة وبدون عمليات نسخ للذاكرة.
///
/// يحمل أيضًا خريطة قيم خاصة بهذا الطلب فقط (`Extensions`) تملؤها الطبقات الوسيطة (Middleware)
/// وتقرأها الدوال المعالجة — تُخصَّص عند أول إدراج، وتبقى لكل اتصال HTTP/2 خريطة منفصلة لكل تدفق.
pub struct RequestContext {
    pub raw_uri: String,
    pub path: String,
    pub query_str: Option<String>,
    pub method: Method,
    pub params: HashMap<String, String>,
    /// Every field line of the request, keyed by ASCII-lowercase field name.
    ///
    /// A field name is case-insensitive on the wire (RFC 9110 §4.2), so the store normalises the
    /// name and keeps the lines in wire order; [`Self::header`] and [`Self::header_values`] are
    /// the whole read surface, and there is no write surface. One store is the point: a second,
    /// handler-visible map would let a middleware rewrite what the next handler reads while the
    /// framework kept deciding on the parsed lines, and the two frontends would then have to agree
    /// on two representations instead of one. Middleware that wants to pass a decision to the
    /// handler uses [`Self::extensions_mut`], which is per-request and is not consulted by any
    /// framework check.
    header_multi: HashMap<String, HeaderLines>,
    pub body: Bytes,
    body_stream: Option<crate::request_body::RequestBody>,
    pub request_id: String,
    /// The numeric half of the id above; see [`RequestParts::request_serial`].
    request_serial: u64,
    state: Arc<AppStateContainer>,
    /// Values a [`RouteGroup`] or a mounted [`Router`] registered for this route, consulted
    /// before [`Self::state`]. `None` for every route registered plain, which is what keeps a
    /// scoped route from paying a lookup the app-wide container could not answer anyway.
    scoped_state: Option<Arc<AppStateContainer>>,
    /// The TCP peer that opened the connection, kept private so adding it did not break
    /// construction: this struct already holds private fields, so no caller outside this
    /// module could build one with a literal in the first place.
    peer_addr: Option<SocketAddr>,
    /// Same reason for privacy: the accessors below are the API, and a literal construction of
    /// this struct is impossible outside the crate either way.
    extensions: crate::extensions::Extensions,
}

impl RequestContext {
    /// Build the handler-visible request from protocol-independent parts.
    ///
    /// `state` is not part of [`RequestParts`]: it belongs to the server, not to the
    /// request, so the serving core supplies it.
    pub(crate) fn from_parts(parts: RequestParts, state: &Arc<AppStateContainer>) -> Self {
        RequestContext {
            raw_uri: parts.raw_uri,
            path: parts.path,
            query_str: parts.query_str,
            method: parts.method,
            params: parts.param_map,
            header_multi: parts.header_multi,
            body: parts.body,
            body_stream: parts.body_stream,
            request_id: parts.request_id,
            request_serial: parts.request_serial,
            state: Arc::clone(state),
            scoped_state: None,
            peer_addr: parts.peer_addr,
            extensions: parts.extensions,
        }
    }

    /// Values attached to this request by middleware, keyed by type.
    ///
    /// This is how a middleware passes a per-request decision — an authenticated principal, a
    /// resolved tenant, a rate-limit verdict — to the handler without putting it in shared state,
    /// which is process-wide and would leak one request's answer into another.
    ///
    /// ```text
    /// app.use_middleware(|mut ctx: RequestContext, next| async move {
    ///     ctx.insert_extension(Principal("hamza"));
    ///     next.run(ctx).await
    /// });
    ///
    /// app.get_async("/me", |ctx: RequestContext| async move {
    ///     match ctx.extension::<Principal>() {
    ///         Some(p) => Response::text_dynamic(p.0.to_string()),
    ///         None => Response::status(401, "Unauthorized"),
    ///     }
    /// });
    /// ```
    ///
    /// A request has exactly one owner at a time — the context moves through the chain into the
    /// handler — so nothing here is synchronised, and a value stored by one request is not
    /// reachable from another. That last part is asserted by
    /// `an_extension_set_by_middleware_reaches_the_handler` and
    /// `extensions_do_not_leak_between_requests_on_one_connection`.
    pub fn extensions(&self) -> &crate::extensions::Extensions {
        &self.extensions
    }

    /// The writable map, for `insert`/`remove`/`get_mut` in one place.
    pub fn extensions_mut(&mut self) -> &mut crate::extensions::Extensions {
        &mut self.extensions
    }

    /// Shorthand for `self.extensions().get::<T>()`.
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.extensions.get::<T>()
    }

    /// Shorthand for `self.extensions_mut().insert(value)`.
    pub fn insert_extension<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.extensions.insert(value)
    }

    /// Shorthand for `self.extensions_mut().insert_sized(value, owned_heap)` — the same insert,
    /// with the heap behind the value declared so the request's memory registration can hold it.
    ///
    /// Use it whenever the attached value is bigger than the struct it is stored in: a parsed
    /// document, a buffered report, a decompressed asset. Without the declaration the budget sees
    /// the container and the inline size, which for a `String` is 24 bytes no matter how many
    /// megabytes the `String` points at.
    pub fn insert_extension_sized<T: Send + Sync + 'static>(
        &mut self,
        value: T,
        owned_heap: usize,
    ) -> Option<T> {
        self.extensions.insert_sized(value, owned_heap)
    }

    /// The bytes this request's extension map has asked the scheduler to register for its values.
    /// Zero when the server runs without a scheduling layer.
    pub fn extension_charged_bytes(&self) -> u64 {
        self.extensions.charged_bytes()
    }

    /// Route this request's extension charges at the handler job the scheduler admitted for it.
    /// Called by the serving core between admission and the handler call; the registration is
    /// released with that job, so a payload's bytes cannot outlive the request that made them.
    pub(crate) fn attach_extension_charge(
        &mut self,
        layer: Arc<crate::scheduling::SchedulingLayer>,
        id: aether_scheduler::JobId,
    ) {
        self.extensions.attach_charge(layer, id);
    }

    /// The peer address of the TCP connection that carried this request.
    ///
    /// `None` only for requests synthesized outside the listener (the in-process test
    /// helpers), which is why rate limiting keys off it rather than a `Host`-derived identity.
    /// This is the *transport* peer: behind a proxy it is the proxy, not the end client, so a
    /// deployment that terminates elsewhere must forward the real address itself — and configure
    /// [`AetherApp::enable_trusted_proxies`] so that [`Self::client_addr`] reports the caller.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }

    /// The caller's address, with a configured trusted-proxy chain resolved.
    ///
    /// Equal to [`Self::peer_addr`]'s address when no proxy list is configured, because the
    /// transport peer is then the only address the server can vouch for. The value comes from the
    /// forwarding headers only when the peer is a listed proxy and only up to the first hop that
    /// is not one (see [`aether_middleware::TrustedProxies`]).
    pub fn client_addr(&self) -> Option<std::net::IpAddr> {
        self.extensions
            .get::<ClientAddr>()
            .map(|client| client.0)
            .or_else(|| self.peer_addr.map(|peer| peer.ip()))
    }

    /// استخراج قيمة معلمة مسار ديناميكية (Dynamic Route Parameter) مثل `:id` أو `{id}`
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }

    /// استخراج معلمة المسار وتحويلها تلقائياً إلى نوع رقمي أو منطقي محدد (مثل `i32`, `u64`, `bool`).
    pub fn param_parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.param(key).and_then(|v| v.parse::<T>().ok())
    }

    /// استخراج قيمة معلمة استعلام (Query Parameter) من الرابط (مثل `?page=2&filter=active`).
    ///
    /// The value is returned exactly as it appeared on the wire, still percent-encoded.
    /// Use [`Self::query_decoded`] when the value contains escapes (any non-ASCII text, a
    /// space, `&`, `=`, …).
    pub fn query(&self, key: &str) -> Option<&str> {
        let q = self.query_str.as_deref()?;
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == key {
                    return Some(v);
                }
            } else if pair == key {
                return Some("");
            }
        }
        None
    }

    /// استخراج معلمة استعلام مع فك تشفير الـ percent-encoding.
    ///
    /// Decoding follows `application/x-www-form-urlencoded` rules for the key and the value
    /// alike, so `+` is a space and `%D9%85` is `م`: a browser sends a field named `first name`
    /// as `first+name=...`, and that is the name this lookup must accept. Returns `None` when
    /// the key is absent; an absent value (`?flag`) yields `Some("")`.
    pub fn query_decoded(&self, key: &str) -> Option<String> {
        let q = self.query_str.as_deref()?;
        for pair in q.split('&') {
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            if form_decode_cow(k) == key {
                return Some(form_decode(v));
            }
        }
        None
    }

    /// Parse a query parameter into a typed value (e.g. `i32`, `u64`, `bool`, etc.)
    pub fn query_parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.query(key).and_then(|v| v.parse::<T>().ok())
    }

    /// Parse a percent-decoded query parameter into a typed value.
    pub fn query_parse_decoded<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.query_decoded(key).and_then(|v| v.parse::<T>().ok())
    }

    /// Retrieve all query parameters as a Map
    ///
    /// Keys and values are returned raw, exactly as received. See
    /// [`Self::queries_decoded`] for the decoded form.
    pub fn queries(&self) -> HashMap<&str, &str> {
        let mut map = HashMap::new();
        if let Some(ref q) = self.query_str {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    map.insert(k, v);
                } else if !pair.is_empty() {
                    map.insert(pair, "");
                }
            }
        }
        map
    }

    /// Retrieve all query parameters, percent-decoded.
    pub fn queries_decoded(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Some(ref q) = self.query_str {
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (k, v) = match pair.split_once('=') {
                    Some((k, v)) => (k, v),
                    None => (pair, ""),
                };
                map.insert(form_decode(k), form_decode(v));
            }
        }
        map
    }

    /// Parses URL-encoded form data (`application/x-www-form-urlencoded`) from request body into key-value map
    pub fn form_data(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Ok(body_str) = self.body_str() {
            for pair in body_str.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    let decoded_k = form_decode(k);
                    let decoded_v = form_decode(v);
                    map.insert(decoded_k, decoded_v);
                } else if !pair.is_empty() {
                    map.insert(form_decode(pair), String::new());
                }
            }
        }
        map
    }

    /// Retrieve an HTTP header case-insensitively, returning the first field line.
    ///
    /// RFC 9110 §5.2 keeps the wire order of repeated lines, so the first one is the value
    /// every layer of the server must agree on: middleware, handler and the framework's own
    /// checks all resolve a duplicated field to this line.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.lines_for(name)
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    /// Every field the request carried, as `(lowercase name, first line)`, in no particular
    /// order. Repeated lines are reachable through [`Self::header_values`].
    ///
    /// Names are normalised because RFC 9110 §4.2 makes field names case-insensitive; the wire
    /// casing is not retained, so a proxy that must reproduce it byte-for-byte should read the
    /// connection instead. This is the only enumeration surface — the store itself is private,
    /// and there is deliberately no mutable counterpart:
    ///
    /// ```compile_fail,E0609
    /// # use aether::prelude::*;
    /// # fn middleware(mut ctx: RequestContext) {
    /// // No public field to rewrite, so a guard cannot grant or revoke an upgrade by editing
    /// // the request; it has to answer with a response.
    /// ctx.headers.insert("upgrade".to_string(), "websocket".to_string());
    /// # }
    /// ```
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.header_multi.iter().map(|(name, lines)| {
            (
                name.as_str(),
                lines.first().map(String::as_str).unwrap_or(""),
            )
        })
    }

    /// Every field line received for `name`, in wire order.
    ///
    /// RFC 9110 §5.3 treats repeated field lines as equivalent to one comma-joined line
    /// for list-valued fields. A `HashMap<String, String>` cannot represent that, so
    /// `header()` only exposes the first line; this method exposes all of them. Values are
    /// not split on commas, because a comma may legitimately be data (for example inside
    /// a cookie value or a quoted string).
    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.lines_for(name)
            .map(|values| values.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Keys of the multi store are ASCII-lowercase, so a query that is already lowercase
    /// hits directly; only a mixed-case name needs a lowercased `String` to look up.
    fn lines_for(&self, name: &str) -> Option<&HeaderLines> {
        if name.bytes().all(|b| !b.is_ascii_uppercase()) {
            self.header_multi.get(name)
        } else {
            self.header_multi.get(&name.to_ascii_lowercase())
        }
    }

    /// Parse the body as an RFC 7578 `multipart/form-data` payload.
    ///
    /// The returned parts borrow from [`Self::body`], so no file contents are copied.
    /// Returns [`aether_http::multipart::MultipartError::UnsupportedContentType`] when the request
    /// is not a multipart form. The request body is already bounded by the server's configured
    /// body limit before the handler runs.
    pub fn multipart(
        &self,
    ) -> Result<aether_http::multipart::MultipartForm<'_>, aether_http::multipart::MultipartError>
    {
        aether_http::multipart::MultipartForm::parse(
            self.header("content-type").unwrap_or(""),
            &self.body,
        )
    }

    /// قراءة قيمة ملف تعريف الارتباط (Cookie) بالاسم من ترويسة `Cookie`
    ///
    /// The `Cookie` field is not a list header in the `#field` sense, but a receiver still reads
    /// repeated lines as one jar: RFC 9113 §8.2.3 lets an HTTP/2 client split `Cookie` across
    /// fields, and RFC 6265 §5.4 wraps the whole jar in one field for that reason. Reading only
    /// the first line reported a cookie the client sent as absent — and inconsistently, since
    /// [`Self::signed_cookie`] and the CSRF check already read every line. When a name is carried
    /// more than once, the first occurrence wins exactly as it does within one field;
    /// [`Self::signed_cookie`] is the accessor that refuses that ambiguity instead of choosing.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        for line in self.lines_for("cookie")? {
            for part in line.split(';') {
                let trimmed = part.trim();
                if let Some((k, v)) = trimmed.split_once('=')
                    && k.trim() == name
                {
                    return Some(v.trim());
                }
            }
        }
        None
    }

    /// استخراج جسم الطلب (Body) كنص UTF-8 مباشرة بدون أي عمليات نسخ للذاكرة.
    pub fn body_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }

    /// فك تشفير جسم الطلب من صيغة JSON مباشرة إلى بنية بيانات Rust محددة النوع.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }

    /// The lane a streaming route reads its body from, if this request is being streamed.
    ///
    /// `Some` exactly when the route was registered with
    /// [`AetherApp::route_stream`](crate::AetherApp::route_stream) — on both frontends: the HTTP/1.1
    /// loop and the HTTP/2 one dispatch such a route from its head and pump DATA/DATA frames into
    /// the same lane. A handler written for both reads [`Self::read_body`], which takes the lane
    /// when there is one and the buffered body otherwise.
    ///
    /// On a streamed request [`Self::body`] is **empty**: the bytes have not arrived when the
    /// handler starts. Neither `Json<T>` nor `Form<T>` works on such a route for the same reason.
    pub fn body_stream(&mut self) -> Option<&mut crate::request_body::RequestBody> {
        self.body_stream.as_mut()
    }

    /// The whole request body, up to `max` bytes, from the lane if this request is streamed and
    /// from the buffered copy otherwise.
    ///
    /// `max` is the handler's own ceiling: a route that parses a document knows how large a
    /// document it will take. The server-wide ceiling (`max_stream_body_bytes`) still applies
    /// underneath.
    ///
    /// `413` is the answer for [`BodyError::TooLarge`], and it is the handler's to give — the
    /// framework cannot answer for a handler that has not returned yet. A body over `max` is also
    /// abandoned at this point: on HTTP/1.1 the unread remainder cannot be framed as another
    /// request, so the connection closes after the response; on HTTP/2 the remainder belongs to
    /// the one stream, which the peer's own framing closes, and the connection stays usable.
    pub async fn read_body(&mut self, max: usize) -> Result<Bytes, crate::request_body::BodyError> {
        match self.body_stream.as_mut() {
            Some(stream) => stream.read_to_end(max).await,
            None if self.body.len() > max => Err(crate::request_body::BodyError::TooLarge),
            None => Ok(self.body.clone()),
        }
    }

    /// استرجاع مرجع الخدمة أو الحالة المشتركة المحقونة بنظام الـ DI عبر نوعها `T`.
    ///
    /// تُقرأ حاوية المجموعة أو الراوتر المُثبَّت على هذا المسار أولًا ثم حاوية التطبيق: القيمة
    /// الأكثر تحديدًا تفوز، وأي نوع لم تسجّله المجموعة يظلّ ظاهرًا من حالة التطبيق المشتركة.
    /// A route registered without a scope resolves exactly as it did before — one map lookup.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        if let Some(scoped) = &self.scoped_state
            && let Some(value) = scoped.get::<T>()
        {
            return Some(value);
        }
        self.state.get::<T>()
    }

    /// Attach the state container a group or mounted router registered for this route.
    ///
    /// Only the scope's own chain entry calls this, and it calls it before that scope's
    /// middleware run, so a guard registered on the group reads the same value as its handlers.
    pub(crate) fn attach_scoped_state(&mut self, overlay: Arc<AppStateContainer>) {
        self.scoped_state = Some(overlay);
    }

    /// الحصول على المعرّف الفريد لتتبع هذا الطلب (Request Trace ID).
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// The numeric half of [`Self::request_id`], used by the redacted-error report.
    pub(crate) fn request_serial(&self) -> u64 {
        self.request_serial
    }
}

pub type BoxFutureResponse = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;
pub type AsyncHandlerFn = Arc<dyn Fn(RequestContext) -> BoxFutureResponse + Send + Sync + 'static>;
pub type WsHandlerFn = Arc<
    dyn Fn(
            RequestContext,
            WebSocket<crate::transport::Stream>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>
        + Send
        + Sync
        + 'static,
>;

/// The continuation passed to user middleware: calling it runs the next middleware in the
/// chain, or the route handler when the chain is exhausted.
pub type Next = Arc<dyn Fn(RequestContext) -> BoxFutureResponse + Send + Sync + 'static>;

/// Type-erased user middleware: receives the request plus the continuation, and produces the
/// response (by calling `next`, by short-circuiting, or both).
pub type MiddlewareEntry =
    Arc<dyn Fn(RequestContext, Next) -> BoxFutureResponse + Send + Sync + 'static>;

/// Type-erase one user middleware closure into a chain entry, mapping whatever it returns into
/// the chain's response type. Shared by the app-wide stack, a group's stack and a single route's.
fn middleware_entry<M, Fut, R>(middleware: M) -> MiddlewareEntry
where
    M: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + Send + 'static,
{
    Arc::new(move |ctx, next| {
        let fut = middleware(ctx, next);
        Box::pin(async move { fut.await.into_response() })
    })
}

/// The chain entry that hands a scope's state container to the request.
///
/// It is a middleware rather than a field the frontend fills in because the scope is known at
/// *registration*, and the request is not: this way the per-request path stays exactly what it
/// was — one `Option` that is `None` for every route without a scope — and the container reaches
/// the scope's own middleware as well as its handler, since the entry sits in front of them.
fn attach_state_entry(overlay: Arc<AppStateContainer>) -> MiddlewareEntry {
    middleware_entry(move |mut ctx, next| {
        ctx.attach_scoped_state(Arc::clone(&overlay));
        async move { next(ctx).await }
    })
}

/// Build the dispatch chain for one route: wrap `handler` with `middleware` so that the
/// first middleware in the list runs first on the request path.
pub(crate) fn wrap_with_middleware(
    handler: AsyncHandlerFn,
    middleware: &[MiddlewareEntry],
) -> AsyncHandlerFn {
    let mut next = handler;
    for mw in middleware.iter().rev() {
        let mw = Arc::clone(mw);
        let inner = next;
        next = Arc::new(move |ctx| {
            let mw = Arc::clone(&mw);
            let inner = Arc::clone(&inner);
            mw(ctx, inner)
        });
    }
    next
}

/// كائن تطبيق Aether الأساسي والمسؤول عن إدارة المسارات، البرمجيات الوسيطة، وحلقة تشغيل الخادم.
pub struct AetherApp {
    router: RadixRouter,
    handlers: HashMap<u32, AsyncHandlerFn>,
    ws_handlers: HashMap<u32, WsHandlerFn>,
    /// The middleware chain a WebSocket route must clear before its handshake is validated.
    /// Stored per route and unfolded at serve time, because the chain's terminal here hands the
    /// request to the socket instead of answering it, so it cannot be pre-bound at registration.
    ws_middleware: HashMap<u32, Arc<Vec<MiddlewareEntry>>>,
    /// The subprotocols a WebSocket route accepts, in the route's preference order. Absent means
    /// the route declared none, so a client's offer is ignored rather than negotiated.
    ws_subprotocols: HashMap<u32, Arc<Vec<String>>>,
    /// Routes registered to consume their body as it arrives. Empty unless `route_stream` is
    /// used, and the HTTP/1 loop checks the emptiness before it looks at any head twice.
    streaming_routes: std::collections::HashSet<u32>,
    next_id: u32,
    limits: ServerLimits,
    state: AppStateContainer,
    cors: Option<CorsMiddleware>,
    compression: Option<aether_middleware::CompressionMiddleware>,
    security: Option<SecurityHeadersMiddleware>,
    host_validation: Option<HostValidation>,
    trusted_proxies: Option<Arc<TrustedProxies>>,
    rate_limit: Option<Arc<RateLimiter>>,
    /// The CSRF layer, app-wide when configured: a per-route opt-in would leave every route the
    /// operator forgot to list unprotected, which is the opposite of what this layer is for.
    csrf: Option<Arc<CsrfTokens>>,
    logger: LoggerMiddleware,
    user_middleware: Vec<MiddlewareEntry>,
    config: AetherConfig,
    websocket_limits: aether_websocket::WebSocketLimits,
    scheduling: Option<Arc<crate::scheduling::SchedulingLayer>>,
    tls: Option<crate::tls::TlsAcceptor>,
    h2c: bool,
}

impl Default for AetherApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AetherApp {
    /// Create a new application instance with default configuration
    pub fn new() -> Self {
        Self::with_config(AetherConfig::default())
    }

    /// Create an application instance with custom configuration
    pub fn with_config(config: AetherConfig) -> Self {
        let limits = Self::limits_for(&config);

        Self {
            router: RadixRouter::new(),
            handlers: HashMap::new(),
            ws_handlers: HashMap::new(),
            ws_middleware: HashMap::new(),
            ws_subprotocols: HashMap::new(),
            streaming_routes: std::collections::HashSet::new(),
            next_id: 1,
            limits,
            state: AppStateContainer::new(),
            cors: None,
            compression: None,
            security: None,
            host_validation: None,
            trusted_proxies: None,
            rate_limit: None,
            csrf: None,
            logger: LoggerMiddleware::new(),
            user_middleware: Vec::new(),
            config,
            websocket_limits: aether_websocket::WebSocketLimits::default(),
            scheduling: None,
            tls: None,
            h2c: false,
        }
    }

    /// Register user middleware that runs around every route registered **after** this call.
    ///
    /// Middleware runs inside the built-in response pipeline: the request reaches middleware
    /// after routing and before the handler, and the response passes through it before CORS,
    /// security headers, and compression are applied. Multiple middlewares run in
    /// registration order on the request path (first registered runs first).
    ///
    /// A middleware may modify the request before calling `next`, short-circuit by returning
    /// a response without calling `next`, or modify the response after `next` returns:
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example(app: AetherApp) -> AetherApp {
    /// app.use_middleware(|req: RequestContext, next: Next| async move {
    ///     let mut res = next(req).await;
    ///     res.headers.push(("X-Processed", "true".to_string()));
    ///     res
    /// })
    /// # }
    /// ```
    pub fn use_middleware<F, Fut, R>(mut self, middleware: F) -> Self
    where
        F: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + Send + 'static,
    {
        self.user_middleware.push(middleware_entry(middleware));
        self
    }

    /// Register managed application state
    pub fn manage<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.state.insert(state);
        self
    }

    /// Enable CORS middleware
    pub fn enable_cors(mut self, cors: CorsMiddleware) -> Self {
        self.cors = Some(cors);
        self
    }

    /// Enable Security Headers middleware
    pub fn enable_security_headers(mut self) -> Self {
        self.security = Some(SecurityHeadersMiddleware::new());
        self
    }

    /// Enable security headers from a configuration the caller built.
    ///
    /// This is how a policy that needs a per-response nonce is installed: when
    /// `content_security_policy` names [`crate::CSP_NONCE_PLACEHOLDER`], the server draws one
    /// nonce per request, substitutes it into the header, and hands the same value to the handler
    /// as [`crate::CspNonce`] in the request extensions — so an inline `<script nonce="…">` and
    /// the header that authorises it can never disagree.
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example(app: AetherApp) -> AetherApp {
    /// let app = app.enable_security_headers_with(SecurityHeadersMiddleware {
    ///     content_security_policy: Some("default-src 'self'; script-src 'self' 'nonce-{nonce}'"),
    ///     ..SecurityHeadersMiddleware::default()
    /// });
    /// app.get("/page", |req: RequestContext| {
    ///     let nonce = req.extension::<CspNonce>().map(CspNonce::as_str).unwrap_or("");
    ///     format!("<script nonce=\"{nonce}\">go()</script>")
    /// })
    /// # }
    /// ```
    pub fn enable_security_headers_with(mut self, middleware: SecurityHeadersMiddleware) -> Self {
        self.security = Some(middleware);
        self
    }

    pub fn enable_compression(mut self) -> Self {
        self.compression = Some(aether_middleware::CompressionMiddleware::default());
        self
    }

    /// Answer only requests whose `Host` matches one of `allowed`; anything else gets a 400
    /// with a fixed body, before routing, so a hostile host never reaches a handler, a static
    /// file, or the cache/redirect logic that trusts this header.
    ///
    /// Panics on an entry it cannot parse (a scheme, a path, an empty name, `*.`) rather than
    /// silently keeping a shorter list: a typo in a security allow-list must not degrade into
    /// "protects less than you meant". `*.example.com` matches a sub-domain at any depth and
    /// never the apex; ports and a trailing root dot are ignored. Behind a proxy the `Host`
    /// still comes from the client, so this works regardless of where TLS terminated.
    pub fn enable_host_validation<I, S>(mut self, allowed: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        match HostValidation::new(allowed) {
            Ok(h) => self.host_validation = Some(h),
            Err(e) => panic!("invalid Host allow-list: {e}"),
        }
        self
    }

    /// Enable the per-client token bucket. Keyed by the TCP peer address
    /// ([`RequestContext::peer_addr`]), so behind a proxy every request shares one bucket —
    /// that is a property of transport identity, not a bug to paper over.
    ///
    /// A client that exhausts its budget gets 429 with a non-zero `Retry-After` and the
    /// connection stays open; the limiter fails *open* (counted on the handle from
    /// [`Self::rate_limiter`]) when it hits its tracked-client ceiling, because rejecting
    /// everyone is worse than not mitigating.
    pub fn enable_rate_limit(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit = Some(Arc::new(RateLimiter::new(config)));
        self
    }

    /// Protect every unsafe request with a signed double-submit token.
    ///
    /// The layer is app-wide rather than per-route: a state-changing route the operator forgot to
    /// guard is exactly the one an attacker uses, so the default has to be "protected" and the
    /// escape hatch has to be explicit (`CsrfConfig::exempt_prefixes`, for webhooks that
    /// authenticate by their own signature and not by an ambient cookie).
    ///
    /// On a safe request the token is issued in a cookie and handed to the handler as
    /// [`aether_security::CsrfToken`] in the request extensions — a form renders that value, and
    /// the browser echoes the cookie with it. An unsafe request must repeat it in the configured
    /// header; a missing, mismatched, duplicated or planted token is refused with `403` and an
    /// empty body before the handler runs.
    ///
    /// `GET`, `HEAD`, `OPTIONS` and `TRACE` are exempt because RFC 9110 §9.2.1 makes them safe.
    /// The token is signed with the secret passed to [`aether_security::CsrfTokens::new`], so a
    /// deployment with more than one node must pass the same secret to all of them.
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example(app: AetherApp) -> AetherApp {
    /// let app = app.enable_csrf(CsrfTokens::new(
    ///     std::env::var("CSRF_SECRET").expect("a shared secret"),
    ///     CsrfConfig::default(),
    /// ));
    /// app.get("/form", |req: RequestContext| {
    ///     let token = req.extension::<CsrfToken>().map(CsrfToken::as_str).unwrap_or("");
    ///     format!("<form method=\"post\"><input name=\"_csrf\" value=\"{token}\"></form>")
    /// })
    /// # }
    /// ```
    pub fn enable_csrf(mut self, csrf: CsrfTokens) -> Self {
        self.csrf = Some(Arc::new(csrf));
        self
    }

    /// Believe forwarding headers (`Forwarded`, `X-Forwarded-For`) from these proxy networks.
    ///
    /// The transport peer is the last hop and is always true. A forwarding header is read only
    /// when that peer is in `proxies`, and the chain is then walked right to left: each entry is
    /// believed while the address to its right is a listed proxy, and the first address that is
    /// not one is the caller. A malformed entry stops the walk. Without a list the headers are
    /// ignored entirely, which is what stops a client naming its own rate-limit bucket.
    ///
    /// The rate limiter keys on the resolved address, and the same value reaches handlers as
    /// [`RequestContext::client_addr`]; [`RequestContext::peer_addr`] keeps reporting the socket.
    ///
    /// Panics on a malformed entry, an empty list, or a `/0` block: the last two describe "trust
    /// everyone", which makes a forwarded identity nothing but the client's own claim.
    pub fn enable_trusted_proxies<I, S>(mut self, proxies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        match TrustedProxies::new(proxies) {
            Ok(proxies) => self.trusted_proxies = Some(Arc::new(proxies)),
            Err(error) => panic!("invalid trusted-proxy list: {error}"),
        }
        self
    }

    /// A handle to the shared limiter, or `None` when rate limiting is disabled.
    ///
    /// Take this *before* `serve` moves the app: `RateLimiter::counters()` then reports
    /// refusals and fail-opens for the life of the server, which is what a metrics route or an
    /// operator endpoint needs. Cloning the `Arc` is how the same bucket is read while the
    /// listener keeps writing to it.
    pub fn rate_limiter(&self) -> Option<Arc<RateLimiter>> {
        self.rate_limit.clone()
    }

    /// Enable per-request access logging to stdout (disabled by default so benchmarks
    /// and hot paths stay silent). `show_timing` appends the handler latency in µs.
    pub fn enable_request_logging(mut self, show_timing: bool) -> Self {
        self.logger = LoggerMiddleware {
            enabled: true,
            show_timing,
        };
        self
    }

    /// Register a synchronous or asynchronous GET route
    pub fn get<F, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Get, path, handler)
    }

    /// Register an asynchronous GET route, optionally with typed extractors.
    ///
    /// The handler runs as an async closure of zero to four arguments. Every argument before
    /// the last must implement `FromRequestParts` (`Path`, `Query`, `State`); the last one
    /// implements `FromRequest` (`Json`, `Form`, `RequestContext`). A handler that takes the
    /// context directly must annotate it: `|req: RequestContext| async { .. }`.
    pub fn get_async<F, Args>(self, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Get, path, handler)
    }

    /// Register a synchronous POST route
    pub fn post<F, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Post, path, handler)
    }

    /// Register an asynchronous POST route. See [`Self::get_async`] for extractor syntax.
    pub fn post_async<F, Args>(self, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Post, path, handler)
    }

    /// Register a synchronous PUT route
    pub fn put<F, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Put, path, handler)
    }

    /// Register an asynchronous PUT route. See [`Self::get_async`] for extractor syntax.
    pub fn put_async<F, Args>(self, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Put, path, handler)
    }

    /// Register a synchronous DELETE route
    pub fn delete<F, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Delete, path, handler)
    }

    /// Register an asynchronous DELETE route. See [`Self::get_async`] for extractor syntax.
    pub fn delete_async<F, Args>(self, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Delete, path, handler)
    }

    /// Register a synchronous PATCH route
    pub fn patch<F, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Patch, path, handler)
    }

    /// Register an asynchronous PATCH route. See [`Self::get_async`] for extractor syntax.
    pub fn patch_async<F, Args>(self, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Patch, path, handler)
    }

    /// Register synchronous handler
    pub fn route_sync<F, R>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync_scoped(method, path, handler, &[]);
        self
    }

    fn route_sync_scoped<F, R>(
        &mut self,
        method: Method,
        path: &str,
        handler: F,
        scoped: &[MiddlewareEntry],
    ) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.router.add_route(method, path, id);
        let handler = Arc::new(handler);
        let handler: AsyncHandlerFn = Arc::new(move |ctx| {
            let h = Arc::clone(&handler);
            Box::pin(async move { h(ctx).into_response() })
        });
        let mut chain = Vec::with_capacity(self.user_middleware.len() + scoped.len());
        chain.extend(self.user_middleware.iter().map(Arc::clone));
        chain.extend(scoped.iter().map(Arc::clone));
        self.handlers
            .insert(id, wrap_with_middleware(handler, &chain));
        self
    }

    /// Register an asynchronous handler with an explicit HTTP method.
    ///
    /// Accepts any handler supported by the [`Handler`] trait: `|req: RequestContext| async`,
    /// `|Path(id): Path<u32>| async`, `|Query(q): Query<T>, Json(b): Json<U>| async`, and so
    /// on. Extraction failures return the extractor's rejection response (400/415/500) and the
    /// handler does not run.
    pub fn route_async<F, Args>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async_scoped(method, path, handler, &[], false);
        self
    }

    fn route_async_scoped<F, Args>(
        &mut self,
        method: Method,
        path: &str,
        handler: F,
        scoped: &[MiddlewareEntry],
        streamed: bool,
    ) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.register_async(method, path, handler, scoped, streamed);
        self
    }

    /// Register an asynchronous route that reads its request body **as it arrives**.
    ///
    /// The body is not buffered before the handler runs: the frontend dispatches as soon as the
    /// head is complete and the handler pulls chunks from
    /// [`RequestContext::body_stream`](RequestContext::body_stream) — or reads the whole thing with
    /// [`RequestContext::read_body`](RequestContext::read_body). This is what makes an upload that
    /// is larger than the server's held memory possible: the resident set is one read window, one
    /// chunk and the decoder's own window, whatever the body's length.
    ///
    /// What that buys, it also costs, and the costs are the contract:
    ///
    /// - [`RequestContext::body`](RequestContext::body) is **empty** on this route. So are the
    ///   `Json<T>` and `Form<T>` extractors, which read it. Read the body yourself.
    /// - The body must arrive inside `receive_timeout` (counted from the head) and must not
    ///   exceed `max_stream_body_bytes`, which is a transfer ceiling rather than a memory price;
    ///   `Content-Length` over it is refused `413` before the handler is dispatched.
    /// - A body the handler did not finish reading costs the rest of the exchange: on HTTP/1.1 the
    ///   connection closes after the response, because the bytes left cannot be framed as another
    ///   request and guessing is request smuggling. On HTTP/2 the stream carries its own framing,
    ///   so the abandoned tail costs that one stream — `h2` ignores DATA for a stream nobody reads
    ///   and hands its window back — and the connection serves the next stream normally.
    /// - Both frontends stream. The HTTP/2 pump credits the peer as the handler takes chunks
    ///   (`release_capacity`), so a body larger than the 65,535-byte default window arrives whole.
    ///
    /// An app that registers no streaming route keeps the buffered path byte for byte: the loop
    /// checks whether this set is empty before it looks at any head twice.
    pub fn route_stream<F, Args>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async_scoped(method, path, handler, &[], true);
        self
    }

    fn register_async<F, Args>(
        &mut self,
        method: Method,
        path: &str,
        handler: F,
        scoped: &[MiddlewareEntry],
        streamed: bool,
    ) where
        F: Handler<Args>,
        Args: 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.router.add_route(method, path, id);
        let mut chain = Vec::with_capacity(self.user_middleware.len() + scoped.len());
        chain.extend(self.user_middleware.iter().map(Arc::clone));
        chain.extend(scoped.iter().map(Arc::clone));
        self.handlers.insert(
            id,
            wrap_with_middleware(crate::handler::erase(handler), &chain),
        );
        if streamed {
            self.streaming_routes.insert(id);
        }
    }

    /// Register a synchronous handler whose chain is the app-wide middleware followed by
    /// `middleware` for **this registration only**.
    ///
    /// A group is the right tool when several routes share a prefix and a guard; this is the
    /// tool for one route. What it does not do is scope by path: the chain is bound to the
    /// registration, so another method on the same path, and a second registration of this one,
    /// are unaffected. `middleware` runs after everything registered with
    /// [`use_middleware`](Self::use_middleware) before this call and obeys the same contract —
    /// calling `next` runs the handler, answering without calling it refuses the request. A
    /// middleware registered app-wide *after* this call does not reach it, exactly as for a
    /// plain route: chains are fixed at registration.
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example(app: AetherApp) -> AetherApp {
    /// app.route_sync_with(
    ///     Method::Get,
    ///     "/admin/stats",
    ///     |req: RequestContext, next: Next| async move {
    ///         if req.header("x-api-key") == Some("secret") {
    ///             next(req).await
    ///         } else {
    ///             Response::status(401, "Unauthorized")
    ///         }
    ///     },
    ///     |_| "stats",
    /// )
    /// # }
    /// ```
    pub fn route_sync_with<F, R, M, Fut, O>(
        mut self,
        method: Method,
        path: &str,
        middleware: M,
        handler: F,
    ) -> Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
        M: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = O> + Send + 'static,
        O: IntoResponse + Send + 'static,
    {
        self.route_sync_scoped(method, path, handler, &[middleware_entry(middleware)]);
        self
    }

    /// Async counterpart of [`route_sync_with`](Self::route_sync_with): `handler` may be any
    /// [`Handler`], extractors included, and the per-route chain is composed the same way.
    pub fn route_async_with<F, Args, M, Fut, O>(
        mut self,
        method: Method,
        path: &str,
        middleware: M,
        handler: F,
    ) -> Self
    where
        F: Handler<Args>,
        Args: 'static,
        M: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = O> + Send + 'static,
        O: IntoResponse + Send + 'static,
    {
        self.route_async_scoped(
            method,
            path,
            handler,
            &[middleware_entry(middleware)],
            false,
        );
        self
    }

    /// Register a WebSocket endpoint at `path` (RFC 6455).
    ///
    /// The handler runs after a successful upgrade and owns the connection until it returns;
    /// use [`WebSocket::recv`] and the send methods. Only `GET` requests are upgraded. The
    /// handshake is validated by the framework (method, `Upgrade`, `Connection`, version 13,
    /// and a canonical 16-byte key), and failures are answered with 400 or 426 before the
    /// handler is invoked.
    ///
    /// Middleware registered before this call runs around the upgrade request on the same
    /// execution path an HTTP route uses: it can stamp [`Extensions`](crate::Extensions) the
    /// handler then reads, and its panics and the configured request timeout are mapped to 500
    /// and 504 for the upgrade request too. The upgrade only proceeds when the chain hands the
    /// terminal's `101` back untouched; a middleware that answers with anything else — whether
    /// it refuses before calling `next` (401 for an auth chain) or replaces the answer after
    /// awaiting it — gets that answer sent and no socket is handed over.
    ///
    /// The chain's *shape* is constrained the same way as for an HTTP route, and nothing about a
    /// WebSocket handover loosens it: `next` consumes the [`RequestContext`], which is neither
    /// `Clone` nor constructible outside this crate, so a guard cannot run the terminal twice for
    /// one request. It can run the chain in another task or race it against its own future — in
    /// both cases the rule is unchanged, and the recorded handoff is only honoured if what the
    /// guard finally returns is the terminal's `101`
    /// (`a_middleware_that_forks_the_chain_still_answers_the_upgrade_request`,
    /// `a_middleware_that_abandons_the_chain_future_refuses_the_upgrade`).
    ///
    /// What differs from an HTTP route: the `101` itself is synthesized by the protocol
    /// frontend, so middleware cannot edit the handshake response, and it cannot revoke an
    /// upgrade by mutating headers either, because the validator reads the parsed header store
    /// rather than the public map ([`RequestContext::headers`]). A refusal has to be a returned
    /// response — and a `101` is not a claim a guard can make: a guard that returns
    /// `Response::status(101, ..)` without letting the terminal run is answered
    /// **`500 Internal Server Error`**. The handoff needs the request context the terminal
    /// records, so such a guard performed no protocol transition, and serving its `101` would put
    /// the client into WebSocket framing on a socket this server still parses as HTTP/1.1. The
    /// refusal is one fresh response line, so the headers and body the guard attached to the claim
    /// are dropped with it; it is scoped to this status, not to this path — the core's ordinary
    /// response exit applies the same refusal to any `101` it did not synthesize for a handoff, so a
    /// handler's claim takes the same answer. The refusal covers every informational status: the
    /// request loop writes `100 Continue` straight to the stream before a response exists, so no
    /// `1xx` reaching the exit came from a transition this server performed, and a `103` or an
    /// invented code is answered with `500` instead of framing the client did not ask for
    /// (`a_hand_built_101_from_a_middleware_is_refused_with_500`,
    /// `a_101_without_a_handoff_is_never_served`,
    /// `every_1xx_from_a_handler_is_refused_not_only_101`). `101` is what the framework
    /// synthesizes when it hands a socket over, not a value a middleware can claim. Once the socket
    /// is handed over, the session is not wrapped by anything.
    ///
    /// Two things have to be true before any of that is reached. The request must be **HTTP/1.1**:
    /// `Upgrade` is a 1.1 mechanism (RFC 9110 §6.7) and the `101` line the frontend writes names that
    /// version, so an `HTTP/1.0` upgrade request is refused with a `400` framed as `HTTP/1.0` and no
    /// session starts (`an_http10_upgrade_request_is_refused_instead_of_switching_protocols`). And
    /// under adaptive scheduling the session's reassembly buffers must fit the memory budget: they
    /// live for the whole negotiated connection, so the upgrade re-prices the socket before the
    /// handshake is written and, if the budget cannot cover them, answers `503` with `Retry-After`
    /// and `Connection: close` and closes the socket — refusing after the `101` is not possible
    /// (`websocket_buffers_are_charged_to_the_memory_budget`).
    ///
    /// Chain shapes are settled by the same rule, and the rule is only ever one thing: the client
    /// gets **the response the chain returns**. `next` cannot run twice — it takes the
    /// [`RequestContext`] by value, and that type is neither `Clone` nor constructible outside this
    /// crate, so a second call is a compile error rather than an untested case. `next` *can* be
    /// moved into `tokio::spawn` or raced with `select!`: if the forked chain completes with the
    /// terminal's `101` while the guard answers 202, the client gets 202 and no socket is handed
    /// over (`a_middleware_that_forks_the_chain_still_answers_the_upgrade_request`). A guard that
    /// drops the chain future also gets its own answer served
    /// (`a_middleware_that_abandons_the_chain_future_refuses_the_upgrade`).
    ///
    /// What a refusal does **not** do is cancel work the guard already started. Dropping the
    /// `JoinHandle` of a spawned chain does not abort that task, and a `select!` the guard wins
    /// leaves the chain running to completion: the client has been answered, the socket has not been
    /// handed over, and no session can start — but the forked middleware keeps executing, including
    /// whatever it does next (a query, a write, a log). Authorising the handoff is a decision about
    /// the *response*, not a kill switch for the *task*; a guard that needs the spawned work stopped
    /// has to abort it itself.
    ///
    /// Note for existing apps: WebSocket routes used to run no middleware at all, so an
    /// auth chain that guarded HTTP routes left every `app.ws` route open. Registering the
    /// guard now genuinely guards those routes as well.
    pub fn ws<F, Fut>(mut self, path: &str, handler: F) -> Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.ws_scoped(path, handler, &[], &[]);
        self
    }

    /// [`ws`](Self::ws) that negotiates a subprotocol with the client.
    ///
    /// `protocols` is this route's declaration in **its** preference order (RFC 6455 §4.2.2
    /// leaves the choice to the server). If the client's `Sec-WebSocket-Protocol` offer names
    /// any of them, the first declared name that the client also offered is echoed in the
    /// `101`, whatever order the client listed them in. Only a declared name is ever echoed, so
    /// a client cannot put its own bytes on the response.
    ///
    /// A client that offers no subprotocol gets the plain upgrade. A client that offers some and
    /// matches none gets **`400 Bad Request`** and no session: a client that asked for a
    /// subprotocol is required to fail the connection when the `101` names none, so a `101`
    /// would only relocate that failure. Routes registered with [`ws`](Self::ws) declare
    /// nothing and ignore any offer, which is what every existing route does.
    ///
    /// The session is not told which subprotocol was selected: the handler already knows the
    /// route it serves, and the client's choice is in the request headers the guard chain sees
    /// ([`RequestContext::header`]) if it needs the raw offer.
    pub fn ws_subprotocols<F, Fut>(mut self, path: &str, protocols: &[&str], handler: F) -> Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.ws_scoped(path, handler, &[], protocols);
        self
    }

    /// [`ws`](Self::ws) whose chain is the app-wide middleware followed by `middleware` for this
    /// registration only — the same per-route contract as
    /// [`route_sync_with`](Self::route_sync_with), applied to the upgrade request. A guard that
    /// answers instead of calling `next` gets that answer sent and no socket is handed over.
    pub fn ws_with<F, Fut, M, MFut, O>(mut self, path: &str, middleware: M, handler: F) -> Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
        M: Fn(RequestContext, Next) -> MFut + Send + Sync + 'static,
        MFut: Future<Output = O> + Send + 'static,
        O: IntoResponse + Send + 'static,
    {
        self.ws_scoped(path, handler, &[middleware_entry(middleware)], &[]);
        self
    }

    /// `ws` with the extra middleware a [`RouteGroup`] contributes, in the same order as
    /// [`route_sync_scoped`](Self::route_sync_scoped) composes it: app middleware first, then
    /// group middleware. `protocols` is the route's subprotocol declaration in preference order;
    /// see [`ws_subprotocols`](Self::ws_subprotocols).
    fn ws_scoped<F, Fut>(
        &mut self,
        path: &str,
        handler: F,
        scoped: &[MiddlewareEntry],
        protocols: &[&str],
    ) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.router.add_route(Method::Get, path, id);
        let mut chain = Vec::with_capacity(self.user_middleware.len() + scoped.len());
        chain.extend(self.user_middleware.iter().map(Arc::clone));
        chain.extend(scoped.iter().map(Arc::clone));
        self.ws_middleware.insert(id, Arc::new(chain));
        if !protocols.is_empty() {
            self.ws_subprotocols.insert(
                id,
                Arc::new(protocols.iter().map(|name| name.to_string()).collect()),
            );
        }
        let handler = Arc::new(handler);
        self.ws_handlers.insert(
            id,
            Arc::new(move |ctx, socket| {
                let h = Arc::clone(&handler);
                Box::pin(async move { h(ctx, socket).await })
            }),
        );
        self
    }

    /// Route Grouping with common prefix
    pub fn group<F>(mut self, prefix: &str, configure: F) -> Self
    where
        F: FnOnce(&mut RouteGroup),
    {
        {
            let mut group = RouteGroup {
                app: &mut self,
                prefix: prefix.to_string(),
                middleware: Vec::new(),
                state: Arc::new(AppStateContainer::new()),
            };
            configure(&mut group);
        }
        self
    }

    /// Mount a [`Router`] — an independently built sub-application — at `prefix`.
    ///
    /// Its routes land under the prefix, its middleware runs after the app-wide stack, and its
    /// managed values answer for its own routes before the container registered with
    /// [`manage`](Self::manage). A router mounted at `""` or `"/"` is the whole application, which
    /// is how a sub-app written in another crate is served on its own.
    pub fn mount(mut self, prefix: &str, router: Router) -> Self {
        {
            let mut group = RouteGroup {
                app: &mut self,
                prefix: prefix.to_string(),
                middleware: Vec::new(),
                state: Arc::new(AppStateContainer::new()),
            };
            for action in router.actions {
                action(&mut group);
            }
        }
        self
    }

    /// Derive an OpenAPI 3.1 document from the routes registered so far.
    ///
    /// The document is built from the router's own listing, so it describes what the server
    /// actually answers rather than what a description file claims: routes added through a
    /// [`group`](Self::group) or a mounted [`Router`] are already in that table with their
    /// prefixes, a [`static_dir`](Self::static_dir) appears as the `{filepath}` catch-all it
    /// registered as, and a [`ws`](Self::ws) route carries the upgrade statuses instead of `200`.
    ///
    /// What it cannot know is anything the route table does not hold. No handler is consulted, so
    /// the document carries no request or response schema, no media type, and none of the statuses
    /// a guard, a middleware or the handler itself produces. `HEAD` and the generic `OPTIONS`
    /// answer are absent for the same reason: the server gives them without a registration.
    ///
    /// Call it after the routes are registered. The output depends on the table alone, so writing
    /// it to a file in a build step and diffing that file in review is what this is for.
    pub fn openapi_spec(&self, title: &str, version: &str) -> aether_openapi::OpenApiSpec {
        let listing = self.router.route_listing();
        let routes: Vec<aether_openapi::RouteDeclaration<'_>> = listing
            .iter()
            .map(|route| aether_openapi::RouteDeclaration {
                method: route.method.as_str(),
                path: &route.path,
                // The upgrade is chosen by the handler behind the registration, not by the
                // method: a WebSocket route is registered as `GET` and answers the handshake.
                websocket: self.ws_handlers.contains_key(&route.handler),
            })
            .collect();
        aether_openapi::OpenApiSpec::from_routes(title, version, &routes)
    }

    /// Register a static byte asset route directly (e.g. css, js, images, icons)
    pub fn static_asset(
        self,
        path: &str,
        content_type: &'static str,
        bytes: &'static [u8],
    ) -> Self {
        self.get(path, move |_| {
            Response::bytes(Bytes::from_static(bytes), content_type)
        })
    }

    /// Register a static file from disk to be served directly on a path.
    ///
    /// The file is read with `tokio::fs` on every request so disk latency never blocks a
    /// runtime worker. A missing or unreadable file returns 404.
    pub fn static_file(self, path: &str, file_path: &'static str) -> Self {
        self.get_async(path, move |req: RequestContext| async move {
            let _ = &req;
            let ext = std::path::Path::new(file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let mime = aether_http::static_files::mime_from_extension(ext);
            let Ok((body, len)) =
                crate::static_dir::open_file(std::path::Path::new(file_path)).await
            else {
                return Response::status(404, "File Not Found");
            };
            // Streamed, not read: the file's size is what a handler would buffer, and the
            // declared length keeps the connection persistent and lets the client show progress.
            Response::stream_with_length(body, mime, len)
        })
    }

    /// Serve a directory tree under `route_prefix` (e.g. `/assets` -> `./public`).
    ///
    /// Requests are resolved through `safe_resolve_path`, so `..`, encoded traversal,
    /// Windows device names, and alternate data streams cannot escape `root_dir`.
    /// Directory listings are disabled, pre-compressed variants are ignored, and unknown
    /// paths return 404; [`Self::static_dir_with_options`] turns either on.
    pub fn static_dir(self, route_prefix: &str, root_dir: impl Into<std::path::PathBuf>) -> Self {
        self.static_dir_with_cache(route_prefix, root_dir, 3600)
    }

    /// Same as [`Self::static_dir`] with an explicit `max-age` for `Cache-Control`.
    pub fn static_dir_with_cache(
        self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf>,
        max_age_secs: u32,
    ) -> Self {
        self.static_dir_with_options(
            route_prefix,
            root_dir,
            StaticDirOptions::with_max_age(max_age_secs),
        )
    }

    /// Same as [`Self::static_dir`], with an explicit [`StaticDirOptions`].
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example(app: AetherApp) -> AetherApp {
    /// app.static_dir_with_options(
    ///     "/assets",
    ///     "./public",
    ///     StaticDirOptions { directory_listing: true, precompressed: true, ..Default::default() },
    /// )
    /// # }
    /// ```
    pub fn static_dir_with_options(
        mut self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf>,
        options: StaticDirOptions,
    ) -> Self {
        self.static_dir_scoped(route_prefix, root_dir, options, &[]);
        self
    }

    /// `static_dir_with_options` with the chain a [`RouteGroup`] contributes, so a tree mounted
    /// inside a group or a mounted [`Router`] is guarded and scoped like the routes beside it.
    fn static_dir_scoped(
        &mut self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf>,
        options: StaticDirOptions,
        scoped: &[MiddlewareEntry],
    ) -> &mut Self {
        let tree = Arc::new(StaticTree {
            root: Arc::new(root_dir.into()),
            prefix: Arc::from(route_prefix.trim_end_matches('/')),
            options,
        });

        let catch_all = Arc::clone(&tree);
        let route = format!("{}/*filepath", tree.prefix);
        self.route_async_scoped(
            Method::Get,
            &route,
            move |req: RequestContext| {
                let tree = Arc::clone(&catch_all);
                async move {
                    let relative = req.param("filepath").unwrap_or("").to_string();
                    tree.answer(req, relative).await
                }
            },
            scoped,
            false,
        );

        // A catch-all needs at least one character after the prefix, so the root of the tree is
        // the one directory the route above can never reach. It needs a route of its own
        // whenever the tree can answer there: a listing, or the page the operator ships as
        // `index.html`. The index is read once, here, because the route table is fixed from
        // this point on.
        let root_has_index = tree.root.join("index.html").is_file();
        if tree.options.directory_listing || root_has_index {
            // `find` trims trailing slashes, so this registration answers both `/assets` (with
            // a redirect when it lists) and `/assets/`.
            let root_route = if tree.prefix.is_empty() {
                "/".to_string()
            } else {
                tree.prefix.to_string()
            };
            let at_root = Arc::clone(&tree);
            self.route_async_scoped(
                Method::Get,
                &root_route,
                move |req: RequestContext| {
                    let tree = Arc::clone(&at_root);
                    async move { tree.answer(req, String::new()).await }
                },
                scoped,
                false,
            );
        }
        self
    }

    /// Set resource limits for every accepted WebSocket session, including both inactivity
    /// policies: `WebSocketLimits::read_timeout` (120s) and `WebSocketLimits::write_timeout` (10s).
    #[must_use]
    pub fn with_websocket_limits(mut self, limits: aether_websocket::WebSocketLimits) -> Self {
        self.websocket_limits = limits;
        self
    }

    /// Serve TLS on the same port as plaintext HTTP/1.1, using an acceptor the application
    /// built (SNI sets, ALPN, reload policy).
    ///
    /// The port is not switched to "https only": a connection whose first bytes are a TLS
    /// record is handed to rustls and every other connection keeps being served as HTTP/1.1,
    /// which is what lets one listener serve `https://` and `http://`. Inside TLS, ALPN decides:
    /// a client that negotiates `h2` is served by the HTTP/2 frontend, one that negotiates
    /// `http/1.1` by this loop. Plaintext HTTP/2 needs [`AetherApp::enable_h2c`] — see
    /// [`crate::transport`]. Call [`AetherApp::require_tls`] to refuse plaintext instead.
    #[must_use]
    pub fn tls(mut self, acceptor: crate::tls::TlsAcceptor) -> Self {
        self.tls = Some(acceptor);
        self
    }

    /// Serve TLS from a certificate bundle and key on disk (PEM, leaf first).
    ///
    /// Every builder step consumes the app and hands it back, so registration and startup are one
    /// expression — the shape a caller reaches for when the app exists only to be served.
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    /// AetherApp::new()
    ///     .get("/", |_| "hello")
    ///     .enable_security_headers()
    ///     .tls_from_files("cert.pem", "key.pem")?
    ///     .run("0.0.0.0:443")
    /// # }
    /// ```
    pub fn tls_from_files(
        mut self,
        cert_path: &str,
        key_path: &str,
    ) -> Result<Self, crate::tls::TlsError> {
        let set = crate::tls::TlsCertSet::from_files(cert_path, key_path)?;
        self.tls = Some(crate::tls::TlsAcceptor::new(set)?);
        Ok(self)
    }

    /// Serve TLS from PEM bytes in memory.
    pub fn tls_from_pem(
        mut self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<Self, crate::tls::TlsError> {
        let set = crate::tls::TlsCertSet::from_pem(cert_pem, key_pem)?;
        self.tls = Some(crate::tls::TlsAcceptor::new(set)?);
        Ok(self)
    }

    /// Refuse connections that arrive as plaintext once TLS is configured. Without an
    /// acceptor this changes nothing: there is no TLS to require.
    #[must_use]
    pub fn require_tls(mut self, require: bool) -> Self {
        if let Some(acceptor) = &mut self.tls {
            acceptor.set_require_tls(require);
        }
        self
    }

    /// The live TLS acceptor, for reloading certificates while the server runs.
    ///
    /// Cloning is cheap and correct here: the acceptor shares the certificate table through an
    /// `Arc`, so [`crate::tls::TlsAcceptor::replace_certificates`] on this handle changes what
    /// the listening server serves.
    pub fn tls_acceptor(&self) -> Option<crate::tls::TlsAcceptor> {
        self.tls.clone()
    }

    /// Serve HTTP/2 on a plaintext connection that opens with the `PRI * HTTP/2.0` preface.
    ///
    /// The `h2c` in this name is cleartext HTTP/2 as RFC 9113 §3.3 defines it — *prior
    /// knowledge*, the client arriving with the preface and nothing else. It is not the §3.1
    /// `Upgrade: h2c` token, and this method cannot make that route exist: no request on this
    /// server is ever switched onto HTTP/2 by a header.
    ///
    /// Off by default, and HTTP/2 over TLS does not need it: inside TLS, ALPN chooses. Cleartext
    /// HTTP/2 has no negotiation at all — RFC 9113 §3.3 calls this *prior knowledge*, so the
    /// client is trusted to have learned elsewhere that this port speaks h2. That asymmetry is
    /// why it is an opt-in rather than a sniff-and-serve rule: a connection that starts with the
    /// preface has no HTTP/1 framing left to fall back to if the server reads it the other way.
    ///
    /// A connection served this way goes through the same core as every other one — routing,
    /// guards, compression, CORS, rate limiting, the body and header budgets, the scheduler's
    /// per-stream memory accounting. What it cannot get is a WebSocket upgrade: HTTP/2 has no
    /// Upgrade mechanism (RFC 9113 §8.1), so a WebSocket route answers `400` and the upgrade has
    /// to come over a separate HTTP/1.1 connection, which this port still serves.
    ///
    /// `Upgrade: h2c` is deliberately **not** implemented: RFC 9113 §3.1 deprecates the `h2c`
    /// upgrade token and `HTTP2-Settings` outright, and RFC 7540 §3.2 requires the request that
    /// asked for the upgrade to be answered as an implicitly half-closed stream 1 — a state the
    /// `h2` server does not synthesize.
    ///
    /// What it opens is cleartext HTTP/2, never cleartext in general: on an acceptor that
    /// [`AetherApp::require_tls`] made strict, every plaintext connection is refused whichever
    /// framing it started with, preface included.
    #[must_use]
    pub fn enable_h2c(mut self) -> Self {
        self.h2c = true;
        self
    }

    /// Start the server, listening on `addr` (for example `127.0.0.1:3000` or `[::1]:3000`).
    pub async fn listen(
        mut self,
        addr: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        apply_listen_addr(&mut self.config, addr)?;
        let config = self.config.clone();
        let server = self.bind_with_config(config).await?;
        server.serve().await
    }

    /// Start the server with a full [`AetherConfig`].
    pub async fn listen_with_config(
        self,
        config: AetherConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let server = self.bind_with_config(config).await?;
        server.serve().await
    }

    /// Build the Tokio runtime this app is configured for (`AetherConfig::workers`, set from
    /// `AETHER_WORKERS` by `AetherConfig::from_env`).
    ///
    /// `listen`, `listen_with_config`, `bind` and `serve` all run on the caller's runtime and
    /// cannot change its worker count; this is the runtime those entry points belong on when the
    /// process owns it. `AETHER_WORKERS=0` parses, so the count is clamped to one rather than
    /// handed to a builder that would reject it.
    pub fn build_runtime(&self) -> std::io::Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(self.config.workers.max(1))
            .enable_all()
            .build()
    }

    /// Serve on a runtime built from this app's configuration, blocking the current thread.
    ///
    /// This is the only entry point that applies the configured worker count. Calling it from
    /// inside an existing runtime returns an error instead of panicking: a runtime cannot be
    /// nested, and that caller should use [`listen`](Self::listen) instead.
    pub fn run(self, addr: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(
                "AetherApp::run builds its own runtime; async code should call listen()".into(),
            );
        }
        let runtime = self.build_runtime()?;
        runtime.block_on(self.listen(addr))
    }

    /// Bind the listening socket without accepting connections yet.
    ///
    /// This is the entry point for tests and for applications that need the real bound port
    /// before serving (bind `127.0.0.1:0` and read [`AetherServer::local_addr`]).
    pub async fn bind(
        mut self,
        addr: &str,
    ) -> Result<AetherServer, Box<dyn std::error::Error + Send + Sync>> {
        apply_listen_addr(&mut self.config, addr)?;
        let config = self.config.clone();
        self.bind_with_config(config).await
    }

    /// Bind the listening socket with an explicit configuration.
    pub async fn bind_with_config(
        mut self,
        config: AetherConfig,
    ) -> Result<AetherServer, Box<dyn std::error::Error + Send + Sync>> {
        self.apply_config(config);
        let addr = self.config.listen_addr();

        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                let mut diag = Diagnostic::new(ErrorCode::E1001ServerStartupFailed, format!("TCP Bind failed: {}", e))
                    .with_context("Address", &addr)
                    .with_hint("Port is already occupied by another running application or requires elevated privileges.")
                    .with_suggestion(port_change_advice(self.config.port));
                for advice in port_holder_advice(self.config.port) {
                    diag = diag.with_suggestion(advice);
                }
                eprintln!("{}", diag.format_terminal(true));
                return Err(Box::new(AetherError::Io(e)));
            }
        };

        let local_addr = listener.local_addr()?;
        Ok(AetherServer {
            app: self,
            listener,
            local_addr,
        })
    }

    /// Apply a configuration object, keeping the derived parser limits in sync.
    ///
    /// `listen_with_config` previously took a config but kept the limits derived from the
    /// config supplied to `with_config`, so body/header limits could silently disagree with
    /// the advertised configuration.
    fn apply_config(&mut self, config: AetherConfig) {
        self.limits = Self::limits_for(&config);
        self.config = config;
    }

    /// Derive the parser/serve limits from a configuration object.
    ///
    /// One place rather than the two it used to have: `with_config` and `apply_config` each
    /// copied a subset of the fields by hand, so a field added to `AetherConfig` could be
    /// silently dropped by one of them and the configured bound would not be the enforced one.
    fn limits_for(config: &AetherConfig) -> ServerLimits {
        ServerLimits {
            max_body_bytes: config.max_body_bytes,
            max_stream_body_bytes: config.max_stream_body_bytes,
            max_header_bytes: config.max_header_bytes,
            max_headers_count: config.max_headers_count,
            max_concurrent_streams: config.max_concurrent_streams,
            request_timeout: config.max_request_time,
            receive_timeout: config.max_receive_time,
            write_timeout: config.max_response_time,
            keep_alive_timeout: config.keep_alive_timeout,
            ..ServerLimits::default()
        }
    }

    /// Enable adaptive tree scheduling: bounded connection admission (503 on
    /// overflow instead of an unbounded blocking wait), per-handler execution
    /// budgets, and the divisible-work driver.
    ///
    /// Opt-in: without this call the serve path is byte-for-byte the default
    /// engine behavior. The connection ceiling is kept in sync with
    /// `ServerLimits::max_connections` regardless of what the resource config says.
    pub fn with_scheduling(self, cfg: aether_scheduler::SchedulingConfig) -> Self {
        self.with_scheduling_resources(cfg, aether_scheduler::ResourceConfig::default())
    }

    /// Enable adaptive scheduling with explicit resource ceilings. See
    /// [`Self::with_scheduling`].
    pub fn with_scheduling_resources(
        mut self,
        cfg: aether_scheduler::SchedulingConfig,
        rcfg: aether_scheduler::ResourceConfig,
    ) -> Self {
        let rcfg = aether_scheduler::ResourceConfig {
            max_connections: self.limits.max_connections,
            ..rcfg
        };
        self.scheduling = Some(crate::scheduling::SchedulingLayer::new(cfg, rcfg));
        self
    }

    /// The scheduling layer, if [`Self::with_scheduling`] was called. Clone it to
    /// drive divisible work from handlers or tests.
    #[must_use]
    pub fn scheduling(&self) -> Option<Arc<crate::scheduling::SchedulingLayer>> {
        self.scheduling.clone()
    }
}

/// A bound Aether server that has not started accepting connections yet.
pub struct AetherServer {
    app: AetherApp,
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl AetherServer {
    /// The address the kernel actually assigned (useful with port `0` in tests).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Serve until `Ctrl+C` (all platforms) or `SIGTERM` (Unix), then drain in-flight requests.
    pub async fn serve(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.serve_with_shutdown(wait_for_shutdown_signal()).await
    }

    /// Serve until `shutdown` resolves, then drain in-flight requests and release the socket.
    ///
    /// The shutdown future is polled on its own task, so it may be a signal listener, a
    /// channel receiver, or any other `Send` future. Once it resolves, the listener is
    /// dropped immediately (new connections are refused) and existing connections are given
    /// up to `min(keep_alive_timeout, 30s)` to finish.
    pub async fn serve_with_shutdown<F>(
        self,
        shutdown: F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let AetherServer {
            app,
            listener,
            local_addr,
        } = self;
        let routes_count = app.handlers.len() + app.ws_handlers.len();
        // What the runtime actually has, not what was asked for: the worker count can only be
        // applied by an entry point that builds the runtime itself (`AetherApp::run`). An app
        // served on the caller's runtime has whatever count that runtime was built with.
        let workers_active = tokio::runtime::Handle::try_current()
            .map_or(0, |handle| handle.metrics().num_workers());
        let banner = ServerBanner {
            env: if app.config.is_production() {
                "production"
            } else {
                "development"
            },
            // The scheme the port actually answers, not the one the operator wishes it had: with
            // TLS configured the same listener also still serves plaintext, which the
            // `+ plaintext` below makes visible rather than hidden - unless `require_tls` is set,
            // in which case every cleartext connection is refused and the banner must not imply
            // otherwise.
            address: match app.tls {
                Some(ref acceptor) if acceptor.requires_tls() => {
                    format!("https://{local_addr} (tls required)")
                }
                Some(_) => format!("https://{local_addr} (+ plaintext http)"),
                None => format!("http://{local_addr}"),
            },
            routes_count,
            workers: workers_active,
        };
        println!("{}", banner.render());
        if workers_active != app.config.workers {
            println!(
                "note: {workers_active} Tokio workers active, but workers={} is configured — \
                 the worker count is applied by AetherApp::run, not on a runtime built by the caller",
                app.config.workers
            );
        }

        let limits = Arc::new(app.limits);
        let ws_limits = app.websocket_limits;
        let logger = Arc::new(app.logger);
        let layer = app.scheduling.clone();
        // TLS is per-connection state, not per-listener: the same port answers plaintext
        // HTTP/1.1, TLS/HTTP-1.1 and TLS/HTTP-2, so the acceptor travels into each connection
        // task and decides nothing until the first bytes arrive.
        let tls = app.tls.clone();
        // Cleartext HTTP/2 is a per-connection decision too: the same port keeps serving HTTP/1.1
        // to every connection that does not open with the preface, so this is only a yes/no on
        // what a `PRI * HTTP/2.0` greeting earns.
        let h2c = app.h2c;
        // `OPTIONS *` asks about the server rather than a path, so its `Allow` value is the
        // union of every route's methods. HEAD is listed only when a non-WebSocket GET route
        // exists: the serving core answers HEAD from the GET handler, while a WebSocket route
        // refuses every non-GET with 405 before the handler can run.
        let mut server_methods = 0u64;
        let mut head_served = false;
        app.router.for_each_route(|method, handler_id| {
            server_methods |= method.bitmask();
            if method == Method::Get && !app.ws_handlers.contains_key(&handler_id) {
                head_served = true;
            }
        });
        let server_allow = RadixRouter::allow_header_for_target(server_methods, head_served);
        // The protocol-independent serving state. HTTP/1.1 and HTTP/2 both call
        // into this, so a request gets the same routing, guards and post-processing
        // whichever protocol carried it.
        let core = Arc::new(crate::serve::ServiceCore {
            router: app.router,
            handlers: app.handlers,
            streaming_routes: app.streaming_routes,
            ws_handlers: app.ws_handlers,
            ws_middleware: app.ws_middleware,
            ws_subprotocols: app.ws_subprotocols,
            state: Arc::new(app.state),
            limits: Arc::clone(&limits),
            cors: app.cors.map(Arc::new),
            compression: app.compression.map(Arc::new),
            security: app.security.map(Arc::new),
            host_validation: app.host_validation.map(Arc::new),
            trusted_proxies: app.trusted_proxies.clone(),
            rate_limit: app.rate_limit.clone(),
            csrf: app.csrf.clone(),
            logger: Arc::clone(&logger),
            request_id: app.config.x_request_id,
            server_allow,
            scheduling: app.scheduling.clone(),
            websocket_limits: ws_limits,
        });
        // With the scheduling layer active, connection admission is its job
        // (bounded queue + 503 overflow); the semaphore stays as the plain
        // blocking limiter only on the default path.
        let conn_semaphore = Arc::new(tokio::sync::Semaphore::new(if layer.is_some() {
            1 << 30
        } else {
            limits.max_connections
        }));
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(async move {
            shutdown.await;
            let _ = shutdown_tx.send(true);
        });

        // Track connection tasks so shutdown can actually wait for them. The previous code
        // dropped the semaphore and printed "Graceful shutdown complete" while every spawned
        // request was still running.
        let mut connections: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

        // حلقة استقبال اتصالات الـ TCP مع تقييد عدد الاتصالات المتزامنة (Semaphore Connection Limit)
        loop {
            // Reap finished connection tasks so the JoinSet does not grow without bound.
            while connections.try_join_next().is_some() {}

            if *shutdown_rx.borrow() {
                eprintln!("⚡ Shutdown requested, no longer accepting connections.");
                break;
            }

            let permit = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => break,
                p = conn_semaphore.clone().acquire_owned() => match p {
                    Ok(p) => p,
                    Err(_) => break,
                },
            };

            let (tcp, peer) = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => break,
                r = listener.accept() => match r {
                    Ok(pair) => pair,
                    Err(e) => {
                        eprintln!("Accept error: {}", e);
                        // Resource-exhaustion accept errors (descriptor limits, the
                        // Windows connection-aborted queue) would otherwise turn the
                        // accept loop into a busy spin that steals CPU from every
                        // in-flight handler. A short pause bounds the retry rate;
                        // one-off transient errors still recover on the next pass.
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                },
            };
            let limits_clone = Arc::clone(&limits);
            let logger_clone = Arc::clone(&logger);
            // One clone per connection of the whole shared serving state: the HTTP/1.1 loop
            // and the HTTP/2 frontend call the same `ServiceCore`, so routing, security and
            // dispatch cannot diverge between protocols.
            let core = Arc::clone(&core);
            let mut conn_shutdown = shutdown_rx.clone();
            let sched_conn = layer.clone();
            let tls_conn = tls.clone();

            // معالجة كل مقبس (Socket Connection) بشكل غير متزامن داخل Tokio Task مستقلة
            connections.spawn(async move {
                let _permit = permit; // تصريح الـ RAII يتحرر تلقائياً عند إغلاق الاتصال
                // Adaptive scheduling: connection admission with a bounded queue.
                // Overflow/timeout gets a protocol-valid 503 before any state exists.
                // Admission comes *before* the transport is established on purpose: the
                // connection's `CONN_BASE_MEMORY` registration is what bounds how many sockets
                // may exist, so a socket that has not spoken yet still has to be counted.
                let mut conn_guard = if let Some(l) = &sched_conn {
                    // Charge the socket's overhead, not the largest body it could ever
                    // buffer; the bytes actually read are charged as they arrive.
                    match l.admit(aether_scheduler::NewJob::connection(CONN_BASE_MEMORY)).await {
                        Ok(g) => Some(g),
                        Err(_) => {
                            // What this connection *is* is decided by its first bytes, not by
                            // the port. Shedding has to know that too: a `503` in plaintext
                            // would be garbage to a client mid-ClientHello. So classify (a peek
                            // costs nothing) and answer only a connection already known to be
                            // HTTP/1.x; every other one is closed, because completing a TLS
                            // handshake in order to refuse a client is exactly the work that
                            // overload shedding exists to avoid.
                            let plaintext = matches!(
                                tokio::time::timeout(limits_clone.read_timeout, crate::transport::classify(&tcp)).await,
                                Ok(Ok(crate::transport::Greeting::Http1))
                            ) && !tls_conn
                                .as_ref()
                                .is_some_and(|acceptor| acceptor.requires_tls());
                            if plaintext {
                                let mut stream = tcp;
                                let mut r = Response::status(503, "Service Unavailable");
                                r.headers.push(("Retry-After", "1".to_string()));
                                r.headers.push(("Connection", "close".to_string()));
                                let mut rej_buf = BytesMut::new();
                                r.write_http1_to(&mut rej_buf);
                                // Bound the rejection write: a client that opens the socket but
                                // never reads would otherwise pin this task and its concurrency
                                // permit forever, exhausting the connection budget.
                                let _ = tokio::time::timeout(
                                    limits_clone.write_timeout,
                                    stream.write_all(&rej_buf),
                                )
                                .await;
                                // The client's request bytes are still unread in the receive
                                // buffer here. Closing without a graceful FIN makes Windows
                                // send an RST, which discards the just-written 503 — shutdown
                                // flushes the response and closes cleanly instead.
                                let _ = stream.shutdown().await;
                            }
                            // TLS, h2c, silent, or plaintext-on-a-tls-only-port: dropping the
                            // socket closes it. `establish` would have shut down gracefully, and
                            // a shed connection has no response the client is still reading.
                            return;
                        }
                    }
                } else {
                    None
                };
                // The same port answers plaintext HTTP/1.1, TLS, and — with `enable_h2c` — a
                // plaintext HTTP/2 preface; the first bytes pick which.
                let (mut stream, frontend) = match crate::transport::establish(
                    tcp,
                    tls_conn.as_ref(),
                    limits_clone.read_timeout,
                    limits_clone.tls_handshake_timeout,
                    h2c,
                )
                .await
                {
                    Ok(ok) => ok,
                    Err(_) => {
                        // A connection that went away before it could be served is finished,
                        // not leaked: say so, or the scheduler keeps charging a socket that is
                        // already closed.
                        if let Some(g) = conn_guard.take() {
                            g.complete();
                        }
                        return;
                    }
                };
                // The transport decided the framing once, from the bytes that actually arrived:
                // ALPN inside TLS, the preface plus the operator's h2c opt-in on a plaintext
                // socket. Asking the stream afterwards is not a fallback but a wrong answer — a
                // plaintext connection has no ALPN to report, so HTTP/2 over cleartext could only
                // ever read as HTTP/1. Handing an `h2` session to the HTTP/1 framing loop would
                // answer a valid multiplexed connection with bytes neither side can parse, so the
                // frontend is picked before any byte of the first request is read.
                if frontend == crate::transport::Frontend::Http2 {
                    let conn_job = conn_guard.as_ref().map(|g| g.id());
                    crate::http2::serve(stream, core, peer, conn_job, conn_shutdown.clone()).await;
                    // The connection finished: served, timed out, or closed by the peer. Same
                    // accounting rule as the HTTP/1 loop's exit — a job that ended on its own
                    // terms completes rather than cancels.
                    if let Some(g) = conn_guard.take() {
                        g.complete();
                    }
                    return;
                }
                let mut conn_buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
                // Carries the in-flight `Transfer-Encoding: chunked` body between reads so a
                // body arriving in pieces is decoded once instead of re-decoded from the
                // start after every read.
                let mut chunked_body = aether_http::parser::ChunkedBodyDecoder::new();
                let mut temp_buf = [0u8; 8192];
                let mut write_buf = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
                // Wall-clock start of the request currently being received. The per-read
                // timeout alone lets a client trickle one byte per interval forever; this
                // bounds the whole receive phase with the configured receive timeout.
                let mut recv_started: Option<Instant> = None;
                let mut served_any = false;
                // Whether the `HTTP/1.1 100 Continue` interim response has been emitted for
                // the buffered path's request currently being received: that head loop reads
                // the same head again after every socket read, so it needs a latch.
                // Reset with every completed request, so a pipelined `Expect` still earns its
                // own interim response.
                // so a pipelined `Expect` still earns its own interim response.
                let mut continue_sent = false;
                // The connection's memory registration, tracked beside `conn_buf` and kept
                // equal to `CONN_BASE_MEMORY` + buffered bytes. See `reprice_conn_memory`.
                let mut conn_charged = CONN_BASE_MEMORY;

                loop {
                    // 1. محاولة تحليل أي بيانات مكتملة متبقية في البافر التراكمي (دعم Pipelining و Keep-Alive الفوري)
                    loop {
                        if conn_buf.is_empty() {
                            break;
                        }

                        // A route registered with `route_stream` is served from its head: the body
                        // arrives while the handler runs, so the buffered parser is never asked
                        // for the whole thing. An app with no streaming route pays one empty-set
                        // check; one that has any pays one extra head parse per request, which is
                        // why the gate is the set and not the parse.
                        let streamed = if core.has_streaming_routes() {
                            aether_http::parser::parse_request_head(&conn_buf, &limits_clone)
                                .ok()
                                .flatten()
                                .filter(|head| core.streams_route(head.view.method, head.view.path))
                                .map(|head| {
                                    let (request_body, writer) = crate::request_body::lane();
                                    let (parts, _) = parts_from_view(
                                        &head.view,
                                        Bytes::new(),
                                        Some(request_body),
                                        peer,
                                    );
                                    let version = if head.view.version_minor == 0 {
                                        crate::serve::Version::Http10
                                    } else {
                                        crate::serve::Version::Http11
                                    };
                                    (
                                        parts,
                                        head.framing,
                                        head.header_len,
                                        writer,
                                        head.view.method,
                                        version,
                                    )
                                })
                        } else {
                            None
                        };
                        if let Some((parts, framing, header_len, writer, method, version)) = streamed
                        {
                            // A declared length over the streaming ceiling is refused before
                            // dispatch. The handler exists for bodies that large, so telling it
                            // afterwards would only mean the refuse path had already paid for a
                            // body it never allows.
                            if let aether_http::parser::BodyFraming::ContentLength(len) = framing
                                && len > limits_clone.max_stream_body_bytes
                            {
                                let mut refusal = Response::status(
                                    413,
                                    aether_http::response::canonical_reason(413),
                                );
                                refusal.headers.push(("Connection", "close".to_string()));
                                write_buf.clear();
                                refusal.write_http1_to(&mut write_buf);
                                let _ = tokio::time::timeout(
                                    limits_clone.write_timeout,
                                    stream.write_all(&write_buf),
                                )
                                .await;
                                let _ = stream.shutdown().await;
                                discard_after_close(&mut stream, &mut temp_buf, &limits_clone)
                                    .await;
                                if let Some(g) = conn_guard.take() {
                                    g.complete();
                                }
                                return;
                            }
                            // RFC 9112 §9.3.3: answer the expectation before the handler runs,
                            // for the same reason the buffered path does — the client is paused
                            // and the handler is about to become the one waiting on it. The
                            // pending check is the buffered path's `Ok(None)`: a head plus a body
                            // that already arrived in full owes no interim answer on either path.
                            // No latch is needed here, unlike the buffered path: this branch
                            // splits the head off before the pump below, so one head is
                            // examined exactly once and the interim decision cannot repeat.
                            if aether_http::parser::awaits_100_continue(&conn_buf, &limits_clone)
                                && aether_http::parser::body_is_pending(
                                    &conn_buf,
                                    header_len,
                                    framing,
                                )
                                && tokio::time::timeout(
                                    limits_clone.write_timeout,
                                    stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n"),
                                )
                                .await
                                .map_or(true, |r| r.is_err())
                            {
                                return;
                            }
                            // Everything left in `conn_buf` is body, and how much of it is framed
                            // is the pump's decision, not this loop's.
                            let _ = conn_buf.split_to(header_len);
                            let deadline = Instant::now() + limits_clone.receive_timeout;
                            let mut pump = crate::body_pump::Http1Body::new(
                                &mut stream,
                                &mut conn_buf,
                                writer,
                                framing,
                                &limits_clone,
                                deadline,
                                &conn_guard,
                                &mut conn_charged,
                            );
                            let outcome = core
                                .serve(
                                    crate::serve::Head {
                                        parts,
                                        version,
                                        peer,
                                        // A streaming route is an HTTP handler, not a
                                        // WebSocket one: an upgrade from here has no route to
                                        // belong to.
                                        allows_upgrade: false,
                                    },
                                    &mut pump,
                                )
                                .await;
                            // The handler answered before it read the whole body. What the peer
                            // already sent can still be settled without touching the socket;
                            // what cannot is a body this connection may not carry past the
                            // response, because the leftovers cannot be framed as another
                            // request (RFC 9112 §6.3).
                            let reusable = pump.settle();
                            drop(pump);
                            match outcome {
                                crate::serve::ServeOutcome::ClientGone => {
                                    let _ = stream.shutdown().await;
                                    return;
                                }
                                crate::serve::ServeOutcome::Response {
                                    response,
                                    close_after_response,
                                } => {
                                    let close = close_after_response || !reusable;
                                    match write_http1_response(
                                        &mut stream,
                                        &conn_buf,
                                        &mut write_buf,
                                        &conn_guard,
                                        &mut conn_charged,
                                        &limits_clone,
                                        version,
                                        method,
                                        response,
                                        close,
                                    )
                                    .await
                                    {
                                        None => return,
                                        Some(Answered::Close) => {
                                            if let Some(g) = conn_guard.take() {
                                                g.complete();
                                            }
                                            // Response, then FIN, then a bounded discard: closing
                                            // over bytes this side never read makes Windows reset
                                            // the connection and destroy the answer the client is
                                            // still reading (see `refuse_over_budget`).
                                            let _ = stream.shutdown().await;
                                            discard_after_close(
                                                &mut stream,
                                                &mut temp_buf,
                                                &limits_clone,
                                            )
                                            .await;
                                            return;
                                        }
                                        Some(Answered::KeepAlive) => {}
                                    }
                                }
                                crate::serve::ServeOutcome::WebSocketUpgrade { .. } => {
                                    // Unreachable by construction: `streams_route` matched a
                                    // route in `streaming_routes`, and a WebSocket registration
                                    // never enters that set. Answering a 101 here would hand a
                                    // raw stream to a handler registered for HTTP.
                                    let _ = stream.shutdown().await;
                                    return;
                                }
                            }
                            // The pump's decoder belongs to the body it just read; the buffered
                            // parser must not resume from a state it never drove.
                            chunked_body = aether_http::parser::ChunkedBodyDecoder::new();
                            if conn_buf.is_empty()
                                && conn_buf.capacity() > 2 * CONN_BUF_INITIAL_CAPACITY
                            {
                                conn_buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
                            }
                            if let Reprice::Updated(value) = reprice_conn_memory(
                                &conn_guard,
                                conn_charged,
                                conn_buffered_charge(&conn_buf, &write_buf),
                                0,
                            ) {
                                conn_charged = value;
                            }
                            served_any = true;
                            continue_sent = false;
                            recv_started = if conn_buf.is_empty() {
                                None
                            } else {
                                Some(Instant::now())
                            };
                            continue;
                        }

                        let parse_result = match aether_http::parser::parse_request_incremental(
                            &conn_buf,
                            &limits_clone,
                            &mut chunked_body,
                        ) {
                            Ok(Some((req, owned_body, consumed))) => {
                                let raw_uri = req.raw_uri.to_string();
                                let path = req.path.to_string();
                                let query_str = req.query_str.map(|s| s.to_string());
                                let method = req.method;
                                let version_minor = req.version_minor;
                                let header_multi = header_store(&req.headers);
                                // Chunked bodies are decoded into an owned buffer; framed
                                // bodies are copied out of the reused connection buffer.
                                let body = match owned_body {
                                    Some(decoded) => Bytes::from(decoded),
                                    None => Bytes::copy_from_slice(req.body),
                                };
                                Ok(Some((
                                    raw_uri,
                                    path,
                                    query_str,
                                    method,
                                    version_minor,
                                    header_multi,
                                    body,
                                    consumed,
                                )))
                            }
                            Ok(None) => Ok(None),
                            Err(err) => Err(err),
                        };

                        match parse_result {
                            Ok(Some((
                                raw_uri,
                                path_str,
                                query_str,
                                method_enum,
                                version_minor,
                                header_multi,
                                body_bytes,
                                consumed,
                            ))) => {
                                let (request_serial, req_id_str) = crate::app::next_request_id();
                                let parts = RequestParts {
                                    raw_uri,
                                    path: path_str.clone(),
                                    query_str,
                                    method: method_enum,
                                    param_map: HashMap::new(),
                                    header_multi,
                                    body: body_bytes,
                                    body_stream: None,
                                    request_id: req_id_str,
                                    request_serial,
                                    peer_addr: Some(peer),
                                    extensions: crate::extensions::Extensions::new(),
                                };
                                let version = match version_minor {
                                    0 => crate::serve::Version::Http10,
                                    _ => crate::serve::Version::Http11,
                                };
                                // A request with the client's own bytes already behind it was
                                // not the last thing to arrive: those bytes are the answer, so
                                // the peer is alive and reading ahead would only re-buffer what
                                // is already buffered.
                                let mut watch = if conn_buf.len() > consumed {
                                    crate::serve::ReadAheadWatch::disarmed()
                                } else {
                                    crate::serve::ReadAheadWatch::on(&mut stream, &mut conn_buf)
                                };
                                let outcome = core
                                    .serve(
                                        crate::serve::Head {
                                            parts,
                                            version,
                                            peer,
                                            // `Upgrade` is an HTTP/1.1 mechanism (RFC 9110 §6.7): a
                                            // 1.0 request carries no version this server is willing
                                            // to switch to, and the handshake the upgrade path writes
                                            // is a hardcoded `HTTP/1.1 101` status line. Accepting
                                            // here would hand a 1.0 client a response naming a version
                                            // it never sent, on a connection it expects to close.
                                            allows_upgrade: version == crate::serve::Version::Http11,
                                        },
                                        &mut watch,
                                    )
                                    .await;
                                match outcome {
                                    crate::serve::ServeOutcome::ClientGone => {
                                        // The handler was cancelled because the peer closed or
                                        // reset its side. No answer can be read and no further
                                        // request can arrive; the connection job is left for
                                        // `Drop`, exactly like the failed-write exits below.
                                        let _ = stream.shutdown().await;
                                        return;
                                    }
                                    crate::serve::ServeOutcome::Response {
                                        response,
                                        close_after_response,
                                    } => {
                                        match write_http1_response(
                                            &mut stream,
                                            &conn_buf,
                                            &mut write_buf,
                                            &conn_guard,
                                            &mut conn_charged,
                                            &limits_clone,
                                            version,
                                            method_enum,
                                            response,
                                            close_after_response,
                                        )
                                        .await
                                        {
                                            // The socket failed mid-write: a keep-alive loop on it
                                            // would silently swallow the next response.
                                            None => return,
                                            Some(Answered::Close) => {
                                                // The connection finished its negotiated life;
                                                // count it complete before leaving the task.
                                                if let Some(g) = conn_guard.take() {
                                                    g.complete();
                                                }
                                                // Graceful FIN: pipelined bytes may still sit unread
                                                // in the receive buffer, and a bare close would make
                                                // Windows RST and discard the response just written.
                                                let _ = stream.shutdown().await;
                                                return;
                                            }
                                            Some(Answered::KeepAlive) => {}
                                        }
                                    }
                                    crate::serve::ServeOutcome::WebSocketUpgrade {
                                        accept,
                                        ctx,
                                        handler,
                                        ws_memory,
                                        protocol,
                                    } => {
                                        // A WS session keeps its reassembly buffers for the
                                        // whole negotiated life, an order of magnitude above
                                        // the HTTP charge. Re-pricing before the 101 is what
                                        // keeps the budget enforceable: once the protocol has
                                        // switched there is no HTTP status left to refuse
                                        // with, so a socket that cannot be covered must not
                                        // be upgraded at all.
                                        if let Reprice::OverBudget = reprice_conn_memory(
                                            &conn_guard,
                                            conn_charged,
                                            conn_buffered_charge(&conn_buf, &write_buf),
                                            ws_memory,
                                        ) {
                                            let mut r = Response::status(503, "Service Unavailable");
                                            r.headers.push(("Retry-After", "1".to_string()));
                                            // The refusal is the last word on this socket: no
                                            // upgrade happened, so there is no framing to keep,
                                            // and a client that reused the connection would parse
                                            // its next request as WebSocket frames.
                                            r.headers.push(("Connection", "close".to_string()));
                                            let mut rej_buf = BytesMut::new();
                                            r.write_http1_to_for_request_version(
                                                &mut rej_buf,
                                                version.minor(),
                                                true,
                                            );
                                            let _ = tokio::time::timeout(
                                                limits_clone.write_timeout,
                                                stream.write_all(&rej_buf),
                                            )
                                            .await;
                                            let _ = stream.shutdown().await;
                                            // A 503 we wrote and closed on is a completed
                                            // connection, exactly like the 400/413/431 rejections
                                            // below: leaving the guard here tallied the same
                                            // refusal under two different counters.
                                            if let Some(g) = conn_guard.take() {
                                                g.complete();
                                            }
                                            return;
                                        }
                                        // RFC 6455 §4.2.2 only allows the response to name a
                                        // subprotocol the client offered, and the core already
                                        // picked one from the route's own declaration — so this
                                        // line never carries client bytes.
                                        let protocol_line = match &protocol {
                                            Some(name) => {
                                                format!("Sec-WebSocket-Protocol: {name}\r\n")
                                            }
                                            None => String::new(),
                                        };
                                        let handshake = format!(
                                            "HTTP/1.1 101 Switching Protocols\r\n\
                                             Upgrade: websocket\r\n\
                                             Connection: Upgrade\r\n\
                                             Sec-WebSocket-Accept: {accept}\r\n\
                                             {protocol_line}\r\n"
                                        );
                                        let handshake_write = async {
                                            stream.write_all(handshake.as_bytes()).await?;
                                            stream.flush().await
                                        };
                                        if tokio::time::timeout(
                                            limits_clone.write_timeout,
                                            handshake_write,
                                        )
                                        .await
                                        .map_or(true, |result| result.is_err())
                                        {
                                            let _ = stream.shutdown().await;
                                            return;
                                        }

                                        // Anything after the request head is already
                                        // WebSocket framing (or garbage) and must not be
                                        // parsed as HTTP.
                                        let _ = conn_buf.split_to(consumed);
                                        let initial = conn_buf.to_vec();
                                        let socket = WebSocket::from_parts_with_shutdown(
                                            stream,
                                            initial,
                                            core.websocket_limits,
                                            conn_shutdown.clone(),
                                        );
                                        logger_clone.log("GET", &path_str, 101, Instant::now());

                                        let mut session = tokio::spawn(handler(ctx, socket));
                                        tokio::select! {
                                            biased;
                                            outcome = &mut session => {
                                                // The WebSocket handler returned on its own
                                                // terms: the connection finished its
                                                // negotiated life, so report completion.
                                                // Without this the guard releases via Drop and
                                                // the job is tallied as a cancellation,
                                                // skewing the adaptive baselines.
                                                // A panicking session is NOT a clean
                                                // completion (mirrors the HTTP JoinError
                                                // mapping on the request path): the guard is
                                                // left so Drop records the cancellation
                                                // instead of feeding a crashed handler to the
                                                // baseline.
                                                if outcome.is_ok() && let Some(g) = conn_guard.take() {
                                                    g.complete();
                                                }
                                            }
                                            _ = conn_shutdown.changed() => {
                                                // Shutdown is a request, not a departure: the
                                                // socket holds a clone of this watch, so a handler
                                                // in `recv` writes a 1001 close frame and ends its
                                                // loop on its own terms. Give it the grace to do
                                                // that; a handler that never returns to `recv` is
                                                // still aborted, which is what keeps shutdown
                                                // bounded. A session that returns is a completion,
                                                // like the HTTP request that finishes during a
                                                // drain; only the abort is a cancellation.
                                                match tokio::time::timeout(WS_SHUTDOWN_GRACE, &mut session).await {
                                                    Ok(outcome) => {
                                                        if outcome.is_ok() && let Some(g) = conn_guard.take() {
                                                            g.complete();
                                                        }
                                                    }
                                                    Err(_) => session.abort(),
                                                }
                                            }
                                        }
                                        return;
                                    }
                                }

                                // Advance the buffer by the exact number of consumed bytes
                                let _ = conn_buf.split_to(consumed);
                                // A block that doubled for one large body stays doubled for the
                                // life of the socket: `split_to` reclaims the front, it does not
                                // shrink the allocation. Once the buffer holds nothing, hand the
                                // block back — otherwise pricing the reservation honestly would
                                // tax an idle keep-alive connection for memory it only needed for
                                // one request. A `Bytes` still frozen out of the old block keeps
                                // it alive and uncharged; that window ends when the request's
                                // copies drop.
                                if conn_buf.is_empty()
                                    && conn_buf.capacity() > 2 * CONN_BUF_INITIAL_CAPACITY
                                {
                                    conn_buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
                                }
                                // Report the read and write blocks together: a request
                                // that peaked at a 2 MiB upload answered by a 2 MiB body
                                // held both copies at once. The write block is normally
                                // back to its initial size here — the response path hands
                                // it over as soon as the bytes are on the wire — so this
                                // costs nothing above the read charge it already carried.
                                if let Reprice::Updated(value) = reprice_conn_memory(
                                    &conn_guard,
                                    conn_charged,
                                    conn_buffered_charge(&conn_buf, &write_buf),
                                    0,
                                ) {
                                    conn_charged = value;
                                }
                                served_any = true;
                                // The next request — pipelined bytes already in the buffer or
                                // a later keep-alive one — gets its own Expect handling.
                                continue_sent = false;
                                // A pipelined head already sitting in the buffer starts its
                                // own deadline now; otherwise the next request starts on read.
                                recv_started = if conn_buf.is_empty() {
                                    None
                                } else {
                                    Some(Instant::now())
                                };
                            }
                            Ok(None) => {
                                // Need more data from the network
                                if recv_started.is_some_and(|t| t.elapsed() > limits_clone.receive_timeout) {
                                    answer_receive_timeout(&mut stream, &mut write_buf, &limits_clone).await;
                                    // We answered the request (408) and closed on our own
                                    // terms, so tally it like every other server-initiated
                                    // exit; only aborts and panics are cancellations.
                                    if let Some(g) = conn_guard.take() {
                                        g.complete();
                                    }
                                    return;
                                }
                                // RFC 9112 §9.3.3: a client that sends `Expect:
                                // 100-continue` pauses after the head and waits before
                                // sending the body. The head is complete here (the parser
                                // only returns `None` for an incomplete body when the head
                                // framed it), so answer it now instead of leaving the client
                                // blocked on a deadline it cannot extend.
                                if !continue_sent
                                    && aether_http::parser::awaits_100_continue(
                                        &conn_buf,
                                        &limits_clone,
                                    )
                                {
                                    continue_sent = true;
                                    if tokio::time::timeout(
                                        limits_clone.write_timeout,
                                        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n"),
                                    )
                                    .await
                                    .map_or(true, |r| r.is_err())
                                    {
                                        // The socket is gone; a keep-alive loop on a dead
                                        // stream would only swallow the next request.
                                        return;
                                    }
                                }
                                break;
                            }
                            Err(err) => {
                                let mut err_res = match err {
                                    AetherError::HeaderTooLarge => Response::status(431, "Request Header Fields Too Large"),
                                    AetherError::BodyLimitExceeded => Response::status(413, aether_http::response::canonical_reason(413)),
                                    // The parser refuses an unmet expectation as soon as the head
                                    // is complete, so this is the branch a paused client is
                                    // waiting on: it arrives before any body byte is read.
                                    AetherError::UnsupportedExpectation => Response::status(417, aether_http::response::canonical_reason(417)),
                                    _ => Response::status(400, "Bad Request"),
                                };
                                err_res.headers.push(("Connection", "close".to_string()));
                                write_buf.clear();
                                err_res.write_http1_to(&mut write_buf);
                                // Bound the error write (a stalled client must not pin
                                // the task/permit) and close with FIN so Windows does
                                // not RST away the response over unread request bytes.
                                let _ = tokio::time::timeout(
                                    limits_clone.write_timeout,
                                    stream.write_all(&write_buf),
                                )
                                .await;
                                let _ = stream.shutdown().await;
                                // A 400/413/431 we wrote and closed on is a completed
                                // connection, exactly like the buffer-limit 413 path that
                                // falls through to the loop tail; leaving the guard here
                                // tallied identical rejections under different counters.
                                if let Some(g) = conn_guard.take() {
                                    g.complete();
                                }
                                return;
                            }
                        }
                    }

                    // A keep-alive socket must not pin the drain: once shutdown is requested,
                    // the loop exits after the response currently in flight is written.
                    if *conn_shutdown.borrow() {
                        break;
                    }

                    // Read next TCP chunk. While a request is half-received (or before the
                    // first response is served) the strict read timeout bounds slowloris
                    // trickling; once the socket is simply idle between keep-alive requests
                    // the configured keep_alive_timeout applies instead. A request that has
                    // started arriving also waits no longer than what is left of its receive
                    // budget: without that cap a read begun just inside the budget could
                    // outlast it by a whole idle timeout, and the expiry would arrive late
                    // enough to look like the wrong clock.
                    let half_received = recv_started.is_some();
                    let read_limit = if half_received || !served_any {
                        let idle = limits_clone.read_timeout;
                        match recv_started {
                            Some(started) => idle.min(
                                limits_clone
                                    .receive_timeout
                                    .saturating_sub(started.elapsed()),
                            ),
                            None => idle,
                        }
                    } else {
                        limits_clone.keep_alive_timeout
                    };
                    let read_result = tokio::select! {
                        biased;
                        _ = conn_shutdown.changed() => break,
                        r = tokio::time::timeout(
                            read_limit,
                            stream.read(&mut temp_buf),
                        ) => r,
                    };

                    match read_result {
                        // The peer closed or its socket failed: there is nobody left to answer.
                        Ok(Ok(0)) | Ok(Err(_)) => break,
                        // A read that times out with a request half-received is a receive
                        // timeout, whichever of the two clocks expired, and RFC 9110 §15.5.9
                        // gives it an answer. Before the first response the socket has no
                        // request to answer — and neither has an idle keep-alive connection —
                        // so those still close silently.
                        Err(_) if half_received => {
                            answer_receive_timeout(&mut stream, &mut write_buf, &limits_clone).await;
                            if let Some(g) = conn_guard.take() {
                                g.complete();
                            }
                            return;
                        }
                        Err(_) => break, // Read timed out with no request in flight (Slowloris protection)
                        Ok(Ok(n)) => {
                            // Refuse the request once its *wire* bytes pass the connection's budget.
                            // This is the memory bound — every byte here is resident beside the
                            // decoder's copy until the request completes — and it counts framing, so
                            // a chunked body meets it before the parser's decoded `max_body_bytes`
                            // does. Both quantities are named on the doc comment of
                            // aether_http::limits::ServerLimits::max_body_bytes.
                            if conn_buf.len() + n > limits_clone.max_header_bytes + limits_clone.max_body_bytes {
                                refuse_over_budget(
                                    &mut stream,
                                    &mut conn_buf,
                                    &mut chunked_body,
                                    &mut write_buf,
                                    &mut temp_buf,
                                    &limits_clone,
                                )
                                .await;
                                break;
                            }
                            if recv_started.is_none() {
                                recv_started = Some(Instant::now());
                            }
                            conn_buf.extend_from_slice(&temp_buf[..n]);
                            // Charge what the socket now reserves — plus the decoded prefix a
                            // chunked body is holding, because the wire copy and the decoder's
                            // copy are both live until the request is served. Both are priced by
                            // capacity: `len` hides the byte a doubling allocator already paid
                            // for. If the budget cannot cover it, this connection sheds the bytes
                            // the same way the static buffer limit does — `refuse_over_budget`:
                            // a 413 the client actually reads, then a close, never a hang.
                            match reprice_conn_memory(
                                &conn_guard,
                                conn_charged,
                                conn_buffered_charge(&conn_buf, &write_buf),
                                chunked_body.body_capacity_bytes() as u64,
                            ) {
                                Reprice::Updated(value) => conn_charged = value,
                                Reprice::OverBudget => {
                                    refuse_over_budget(
                                        &mut stream,
                                        &mut conn_buf,
                                        &mut chunked_body,
                                        &mut write_buf,
                                        &mut temp_buf,
                                        &limits_clone,
                                    )
                                    .await;
                                    break;
                                }
                                Reprice::Unchanged => {}
                            }
                        }
                    }
                }

                let _ = stream.shutdown().await;
                // The connection ended on its own terms (EOF, idle timeout, or clean
                // break): report completion so the job is not tallied as a cancellation.
                // Aborts and panics never reach this line and still release via Drop.
                if let Some(g) = conn_guard {
                    g.complete();
                }
            });
        }

        // Graceful shutdown: stop accepting, finish in-flight requests, then abort stragglers.
        if connections.is_empty() {
            eprintln!("⚡ No active connections; graceful shutdown complete.");
        } else {
            eprintln!("⚡ Draining {} active connection(s)...", connections.len());
            let drain_timeout = limits.keep_alive_timeout.min(Duration::from_secs(30));
            let drained = tokio::time::timeout(drain_timeout, async {
                while connections.join_next().await.is_some() {}
            })
            .await;
            if drained.is_err() {
                eprintln!(
                    "⚡ Graceful shutdown timed out after {:?}; aborting remaining connections.",
                    drain_timeout
                );
                connections.shutdown().await;
            }
            eprintln!("⚡ Graceful shutdown complete.");
        }

        Ok(())
    }
}

/// Helper struct for Route Grouping
pub struct RouteGroup<'a> {
    app: &'a mut AetherApp,
    prefix: String,
    middleware: Vec<MiddlewareEntry>,
    /// The state this group registered, shared with the group it was created from until this
    /// group's first [`manage`](Self::manage) copies it out. Routes registered here see these
    /// values ahead of the app-wide container; see [`RequestContext::state`].
    state: Arc<AppStateContainer>,
}

impl<'a> RouteGroup<'a> {
    /// Register middleware that runs around every route registered **inside this group after
    /// this call**, in addition to the app-wide middleware stack. A WebSocket route registered
    /// here gets the same treatment: app middleware first, then this group's, around its
    /// upgrade request. See [`AetherApp::ws`] for what a `101` does and does not expose.
    pub fn use_middleware<F, Fut, R>(&mut self, middleware: F) -> &mut Self
    where
        F: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + Send + 'static,
    {
        self.middleware.push(middleware_entry(middleware));
        self
    }

    /// Register a value in **this group's** state container, visible to the routes and the
    /// middleware registered in this group after this call.
    ///
    /// A route registered here resolves [`State<T>`](crate::extract::State) and
    /// [`RequestContext::state`] against this container first and the app-wide one after, so a
    /// group can carry its own database handle, tenant, or configuration while every type it does
    /// not mention keeps coming from `AetherApp::manage`. A nested group's value shadows the
    /// group it was created from, and both shadow the app.
    ///
    /// Order is the contract, exactly as for [`use_middleware`](Self::use_middleware): a value
    /// reaches the routes registered **after** this call. A route registered before it resolves
    /// the app-wide container as it always did, so an app that manages a type and then opens a
    /// group that manages the same type is not at risk of a half-applied overlay — the overlay
    /// belongs to the routes under it.
    pub fn manage<T: Send + Sync + 'static>(&mut self, state: T) -> &mut Self {
        Arc::make_mut(&mut self.state).insert(state);
        self
    }

    /// Open a group **inside** this one: `prefix` extends this group's, this group's middleware
    /// and state carry in, and what this call registers carries only its own routes.
    ///
    /// A guard registered on the outer group runs before the inner one's, and an inner value
    /// shadows an outer one of the same type while the outer's other types stay visible.
    pub fn group<F>(&mut self, prefix: &str, configure: F) -> &mut Self
    where
        F: FnOnce(&mut RouteGroup<'_>),
    {
        let mut inner = RouteGroup {
            app: &mut *self.app,
            prefix: format!("{}{}", self.prefix, prefix),
            middleware: self.middleware.clone(),
            state: Arc::clone(&self.state),
        };
        configure(&mut inner);
        self
    }

    /// Mount a [`Router`] inside this group: its routes are registered under `prefix`, its guards
    /// run after this group's, and its managed values shadow this group's.
    pub fn mount(&mut self, prefix: &str, router: Router) -> &mut Self {
        let mut inner = RouteGroup {
            app: &mut *self.app,
            prefix: format!("{}{}", self.prefix, prefix),
            middleware: self.middleware.clone(),
            state: Arc::clone(&self.state),
        };
        for action in router.actions {
            action(&mut inner);
        }
        self
    }

    /// The chain this group contributes to one registration: this group's state (when it has
    /// any), then its middleware. App-wide middleware still runs before it, which is what makes
    /// an app-level guard see the request the app defined and only the scoped layers see the
    /// scoped values.
    fn scope(&self) -> Vec<MiddlewareEntry> {
        let overlay = if self.state.is_empty() {
            None
        } else {
            Some(Arc::clone(&self.state))
        };
        let mut chain = Vec::with_capacity(self.middleware.len() + usize::from(overlay.is_some()));
        if let Some(overlay) = overlay {
            chain.push(attach_state_entry(overlay));
        }
        chain.extend(self.middleware.iter().map(Arc::clone));
        chain
    }

    pub fn get<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_sync_scoped(Method::Get, &full_path, handler, &scope);
        self
    }

    pub fn get_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(Method::Get, &full_path, handler, &scope, false);
        self
    }

    pub fn post<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_sync_scoped(Method::Post, &full_path, handler, &scope);
        self
    }

    pub fn post_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(Method::Post, &full_path, handler, &scope, false);
        self
    }

    pub fn put<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_sync_scoped(Method::Put, &full_path, handler, &scope);
        self
    }

    pub fn put_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(Method::Put, &full_path, handler, &scope, false);
        self
    }

    pub fn delete<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_sync_scoped(Method::Delete, &full_path, handler, &scope);
        self
    }

    pub fn delete_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(Method::Delete, &full_path, handler, &scope, false);
        self
    }

    pub fn patch<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_sync_scoped(Method::Patch, &full_path, handler, &scope);
        self
    }

    pub fn patch_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(Method::Patch, &full_path, handler, &scope, false);
        self
    }

    /// Register a synchronous handler with an explicit method, under the group prefix and with
    /// the group's middleware and state. See [`AetherApp::route_sync`].
    pub fn route_sync<F, R>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_sync_scoped(method, &full_path, handler, &scope);
        self
    }

    /// Register an asynchronous handler with an explicit method, under the group prefix. See
    /// [`AetherApp::route_async`].
    pub fn route_async<F, Args>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(method, &full_path, handler, &scope, false);
        self
    }

    /// Serve a directory tree under a path relative to the group prefix, with the group's
    /// middleware and state in front of it. See [`AetherApp::static_dir`].
    pub fn static_dir(
        &mut self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf>,
    ) -> &mut Self {
        let full_prefix = format!("{}{}", self.prefix, route_prefix);
        let scope = self.scope();
        self.app
            .static_dir_scoped(&full_prefix, root_dir, StaticDirOptions::default(), &scope);
        self
    }

    /// Register a streaming route under the group prefix, with the group's middleware.
    /// See [`AetherApp::route_stream`].
    pub fn route_stream<F, Args>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app
            .route_async_scoped(method, &full_path, handler, &scope, true);
        self
    }

    /// Register a WebSocket endpoint under the group prefix. See [`AetherApp::ws`].
    pub fn ws<F, Fut>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app.ws_scoped(&full_path, handler, &scope, &[]);
        self
    }

    /// Register a WebSocket endpoint that negotiates a subprotocol, under the group prefix and
    /// with the group's middleware. See [`AetherApp::ws_subprotocols`].
    pub fn ws_subprotocols<F, Fut>(
        &mut self,
        path: &str,
        protocols: &[&str],
        handler: F,
    ) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        let scope = self.scope();
        self.app.ws_scoped(&full_path, handler, &scope, protocols);
        self
    }
}

/// One registration step a [`Router`] will perform when it is mounted.
type MountAction = Box<dyn FnOnce(&mut RouteGroup<'_>) + Send>;

/// A **sub-application**: routes, middleware and state collected independently of any server,
/// then mounted at a prefix by [`AetherApp::mount`] or by another [`Router::mount`].
///
/// A `Router` is the unit of delivery for a feature: it holds everything an app holds per scope
/// — a prefix-relative set of routes, its own guards, its own managed values — and knows nothing
/// about the port, the TLS configuration or the limits it eventually runs under. Mounting replays
/// its registrations into a [`RouteGroup`] scoped to the mount point, so a mounted sub-app behaves
/// exactly like a group written inline at the same prefix.
///
/// ```rust
/// # use aether::prelude::*;
/// # struct Db(usize);
/// # fn example(app: AetherApp) -> AetherApp {
/// let mut api = Router::new();
/// api.manage(Db(7));
/// api.use_middleware(|ctx, next| async move { next(ctx).await });
/// api.get_async("/health", |ctx: RequestContext| async move {
///     format!("pool={}", ctx.state::<Db>().map(|d| d.0).unwrap_or(0))
/// });
/// app.mount("/api", api)
/// # }
/// ```
///
/// What a mount point contributes:
///
/// - **Paths.** A route registered at `/health` under a router mounted at `/api` is served at
///   `/api/health`. The concatenation is the same rule [`AetherApp::group`] uses, so a prefix
///   written with or without its trailing slash reaches the same route.
/// - **Middleware.** The app-wide stack, then the enclosing group's, then this router's, then the
///   handler — the order a group already produces, and a router mounted inside a group inherits
///   that group's guards.
/// - **State.** [`Router::manage`] values answer for this router's routes before the app-wide
///   container, and an inner mount shadows an outer one of the same type. Types nobody in the
///   chain registers keep resolving app-wide, so mounting never hides the app's own services.
///
/// Order inside a router is the same contract as inside a group: `manage` and `use_middleware`
/// reach the registrations that follow them, not the ones already collected. Mounting consumes the
/// router — `Router` is not `Clone` — so a feature that has to answer at two prefixes builds two
/// routers, or nests one inside the other with [`Router::mount`].
#[derive(Default)]
pub struct Router {
    actions: Vec<MountAction>,
}

impl Router {
    /// An empty sub-application, ready to be filled and mounted.
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
        }
    }

    /// Run `action` against the mount scope when this router is mounted — the escape hatch for a
    /// registration this type does not name. Everything here is one of these, so anything a
    /// [`RouteGroup`] can do, a router can do.
    pub fn add<F>(&mut self, action: F) -> &mut Self
    where
        F: FnOnce(&mut RouteGroup<'_>) + Send + 'static,
    {
        self.actions.push(Box::new(action));
        self
    }

    /// State for every route this router contributes; see [`RouteGroup::manage`].
    pub fn manage<T: Send + Sync + 'static>(&mut self, state: T) -> &mut Self {
        self.actions.push(Box::new(move |group| {
            group.manage(state);
        }));
        self
    }

    /// A guard around every route registered in this router after this call; see
    /// [`RouteGroup::use_middleware`].
    pub fn use_middleware<F, Fut, R>(&mut self, middleware: F) -> &mut Self
    where
        F: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + Send + 'static,
    {
        self.actions.push(Box::new(move |group| {
            group.use_middleware(middleware);
        }));
        self
    }

    /// Register a synchronous route under the mount point. See [`RouteGroup::get`].
    pub fn get<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.get(&path, handler);
        }));
        self
    }

    /// Register an asynchronous route under the mount point. See [`RouteGroup::get_async`].
    pub fn get_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.get_async(&path, handler);
        }));
        self
    }

    pub fn post<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.post(&path, handler);
        }));
        self
    }

    pub fn post_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.post_async(&path, handler);
        }));
        self
    }

    pub fn put<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.put(&path, handler);
        }));
        self
    }

    pub fn put_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.put_async(&path, handler);
        }));
        self
    }

    pub fn delete<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.delete(&path, handler);
        }));
        self
    }

    pub fn delete_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.delete_async(&path, handler);
        }));
        self
    }

    pub fn patch<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.patch(&path, handler);
        }));
        self
    }

    pub fn patch_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.patch_async(&path, handler);
        }));
        self
    }

    /// Register a route with an explicit method under the mount point. See
    /// [`RouteGroup::route_sync`].
    pub fn route_sync<F, R>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.route_sync(method, &path, handler);
        }));
        self
    }

    /// Register an asynchronous route with an explicit method. See [`RouteGroup::route_async`].
    pub fn route_async<F, Args>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.route_async(method, &path, handler);
        }));
        self
    }

    /// A route that reads its request body as it arrives. See [`RouteGroup::route_stream`] and
    /// [`AetherApp::route_stream`] for what streaming costs and buys.
    pub fn route_stream<F, Args>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.route_stream(method, &path, handler);
        }));
        self
    }

    /// A WebSocket endpoint under the mount point. See [`RouteGroup::ws`].
    pub fn ws<F, Fut>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let path = path.to_string();
        self.actions.push(Box::new(move |group| {
            group.ws(&path, handler);
        }));
        self
    }

    /// A WebSocket endpoint that negotiates a subprotocol. See [`RouteGroup::ws_subprotocols`].
    pub fn ws_subprotocols<F, Fut>(
        &mut self,
        path: &str,
        protocols: &[&str],
        handler: F,
    ) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<crate::transport::Stream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let path = path.to_string();
        let protocols: Vec<String> = protocols.iter().map(|name| name.to_string()).collect();
        self.actions.push(Box::new(move |group| {
            let names: Vec<&str> = protocols.iter().map(String::as_str).collect();
            group.ws_subprotocols(&path, &names, handler);
        }));
        self
    }

    /// Serve a directory tree under a path relative to the mount point. See
    /// [`RouteGroup::static_dir`].
    pub fn static_dir(
        &mut self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf> + Send + 'static,
    ) -> &mut Self {
        let route_prefix = route_prefix.to_string();
        let root_dir = root_dir.into();
        self.actions.push(Box::new(move |group| {
            group.static_dir(&route_prefix, root_dir);
        }));
        self
    }

    /// A nested group inside this router, at a path relative to the mount point. See
    /// [`RouteGroup::group`].
    pub fn group<F>(&mut self, prefix: &str, configure: F) -> &mut Self
    where
        F: FnOnce(&mut RouteGroup<'_>) + Send + 'static,
    {
        let prefix = prefix.to_string();
        self.actions.push(Box::new(move |group| {
            group.group(&prefix, configure);
        }));
        self
    }

    /// Mount another router inside this one, at a path relative to where this router is mounted.
    /// Its routes, guards and state nest inside this router's, exactly as a group nests inside a
    /// group. See [`RouteGroup::mount`].
    pub fn mount(&mut self, prefix: &str, router: Router) -> &mut Self {
        let prefix = prefix.to_string();
        self.actions.push(Box::new(move |group| {
            group.mount(&prefix, router);
        }));
        self
    }
}

/// The one header store the core reads, built once for both request paths.
///
/// A field value holding a non-UTF-8 byte must stay visible: `httparse` accepts it, and discarding
/// the whole line would also discard a `close` directive or the `Origin` the CORS decision is made
/// on. `into_owned` reserves exactly the value's length — the `Display` path `to_string()` takes on
/// a `Cow` starts from a zero-capacity `String` and grows, charging one or two extra allocations
/// per line for the same bytes.
///
/// Repeated field lines share one lowercase key and keep wire order inside it, so `Cookie` and
/// `cookie` are correctly merged and `header()` resolves a duplicated field to its first line
/// (RFC 9110 §5.2). The name is normalised rather than kept in wire casing: a store keyed by two
/// spellings would let `X-Dup` and `x-dup` answer differently depending on which accessor asked.
fn header_store(headers: &[aether_http::header::HeaderView<'_>]) -> HashMap<String, HeaderLines> {
    let mut multi: HashMap<String, HeaderLines> = HashMap::with_capacity(headers.len());
    for h in headers {
        let owned = String::from_utf8_lossy(h.value).into_owned();
        multi
            .entry(h.name.to_ascii_lowercase())
            .or_default()
            .push(owned);
    }
    multi
}

/// Own everything a parsed head borrows from the connection buffer.
///
/// The streamed path hands that buffer to the body pump and advances it while the handler runs, so
/// nothing the head borrowed can outlive this call. `body_stream: Some` means the body arrives as
/// the handler reads it; the `body` copy is then empty by contract.
fn parts_from_view(
    view: &aether_http::parser::RequestView<'_>,
    body: Bytes,
    body_stream: Option<crate::request_body::RequestBody>,
    peer: std::net::SocketAddr,
) -> (RequestParts, u8) {
    let header_multi = header_store(&view.headers);
    let (request_serial, request_id) = crate::app::next_request_id();
    (
        RequestParts {
            raw_uri: view.raw_uri.to_string(),
            path: view.path.to_string(),
            query_str: view.query_str.map(str::to_string),
            method: view.method,
            param_map: HashMap::new(),
            header_multi,
            body,
            body_stream,
            request_id,
            request_serial,
            peer_addr: Some(peer),
            extensions: crate::extensions::Extensions::new(),
        },
        view.version_minor,
    )
}

/// First field-line value for `name`, in wire order. Keys of the multi map are stored
/// ASCII-lowercase, so `name` must be given lowercase.
pub(crate) fn header_first(headers: &HashMap<String, HeaderLines>, name: &str) -> Option<String> {
    headers.get(name).and_then(|values| values.first().cloned())
}

/// Every field line for `name`, joined into the single value RFC 9110 §5.2 says the repeated
/// lines mean.
///
/// The forwarding fields are list-valued and append-only along a proxy chain, so the last
/// element of the joined value is the hop nearest this server. Reading only the first line
/// instead would let a client that plants its own `X-Forwarded-For` line decide the answer
/// before the proxy's appended line is ever looked at.
pub(crate) fn header_joined(headers: &HashMap<String, HeaderLines>, name: &str) -> Option<String> {
    let lines = headers.get(name)?;
    let mut iter = lines.iter();
    let mut joined = iter.next()?.clone();
    for line in iter {
        joined.push_str(", ");
        joined.push_str(line);
    }
    Some(joined)
}

/// The connection's buffers are charged for the blocks they reserve as they grow (see
/// `reprice_conn_memory` and `conn_buffered_charge`), so the socket's base charge is only its
/// task and buffer overhead.
use crate::scheduling::{CONN_BASE_MEMORY, CONN_BUF_INITIAL_CAPACITY, WRITE_BUF_INITIAL_CAPACITY};

/// Outcome of re-pricing a connection's memory registration.
pub(crate) enum Reprice {
    /// Nothing to do, or scheduling is off and no registration exists to change.
    Unchanged,
    /// Registration now equals this many bytes.
    Updated(u64),
    /// The budget refused the growth: the caller must shed these bytes.
    OverBudget,
}

/// The buffered part of a connection's charge: what its read buffer reserves beyond the
/// block that [`CONN_BASE_MEMORY`] already pays for.
pub(crate) fn conn_buffer_charge(buf: &BytesMut) -> u64 {
    (buf.capacity().saturating_sub(CONN_BUF_INITIAL_CAPACITY)) as u64
}

/// The write-side half of a connection's buffered charge: the block its response buffer
/// reserves beyond the [`WRITE_BUF_INITIAL_CAPACITY`] every connection starts with.
///
/// A 5 MiB response is serialized into this buffer before `write_all` hands it to the socket,
/// and `clear()` afterwards reclaims the bytes, not the block — so without this the same
/// connection is priced for the body it *read* and free for the one it *wrote*.
fn write_buffer_charge(buf: &BytesMut) -> u64 {
    (buf.capacity().saturating_sub(WRITE_BUF_INITIAL_CAPACITY)) as u64
}

/// Everything currently buffered on this side of the socket: the read block and the response
/// block. Both are real RSS for as long as the connection lives, so both are what the memory
/// budget has to cover.
fn conn_buffered_charge(read: &BytesMut, write: &BytesMut) -> u64 {
    conn_buffer_charge(read).saturating_add(write_buffer_charge(write))
}

/// Hand back a response buffer that a large answer made grow. `clear()` reclaims the length,
/// never the block, so without this a socket that once served a 5 MiB body keeps that
/// reservation — and its charge — for the rest of its keep-alive life. The headroom up to
/// twice [`WRITE_BUF_INITIAL_CAPACITY`] stays, so a connection that answers many small
/// requests does not reallocate per response.
fn release_oversized_write_buf(buf: &mut BytesMut) {
    if buf.capacity() > 2 * WRITE_BUF_INITIAL_CAPACITY {
        buf.clear();
        *buf = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
    }
}

/// The write-buffer block a streaming body keeps between chunks.
///
/// Releasing down to [`WRITE_BUF_INITIAL_CAPACITY`] after every chunk would trade one allocation
/// and one copy per chunk for a charge that flips; holding a block up to this size makes the
/// second chunk onwards allocation-free at the framework's 64 KiB chunk size, and the block is
/// exactly what the budget is told while it is held. A chunk larger than this still shrinks the
/// buffer back once it is on the wire, so one oversized chunk does not reserve its block for the
/// rest of the stream.
const STREAM_BUF_KEEP: usize = 128 * 1024;

/// A chunk that produced no byte, this many times in a row, is a source that is spinning rather
/// than pausing: nothing has been written, so there is no I/O to yield the task back to the
/// reactor. The limit ends such a body instead of letting it occupy a worker forever. Shared with
/// the HTTP/2 writer, which faces the same source.
pub(crate) const MAX_EMPTY_CHUNKS: u32 = 1024;

/// Release the write block after a streamed chunk, keeping a chunk-sized one for the next.
fn release_stream_write_buf(buf: &mut BytesMut) {
    buf.clear();
    if buf.capacity() > STREAM_BUF_KEEP {
        *buf = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
    }
}

/// Write one buffer to the socket under the connection's write deadline.
///
/// `false` means the client did not take the bytes: no timeout-free path exists here, because a
/// peer that stops reading must not pin a task for the rest of its life.
async fn write_buf_to_socket(
    stream: &mut crate::transport::Stream,
    buf: &BytesMut,
    write_timeout: Duration,
) -> bool {
    tokio::time::timeout(write_timeout, async {
        stream.write_all(buf).await?;
        stream.flush().await
    })
    .await
    .is_ok_and(|result| result.is_ok())
}

/// What a written HTTP/1 response leaves for the connection to do.
enum Answered {
    /// The message is complete and the socket may carry another request.
    KeepAlive,
    /// The response was the last thing this connection may carry.
    Close,
}

/// Serialise `response` for HTTP/1 and write it, body included.
///
/// One implementation for both request paths: a buffered request and a streamed one differ in
/// how the request body arrived, not in how the answer leaves. `None` means the socket failed
/// mid-write and the caller must end the connection task.
#[allow(clippy::too_many_arguments)]
async fn write_http1_response(
    stream: &mut crate::transport::Stream,
    conn_buf: &BytesMut,
    write_buf: &mut BytesMut,
    conn_guard: &Option<crate::scheduling::AdmissionGuard>,
    conn_charged: &mut u64,
    limits_clone: &ServerLimits,
    version: crate::serve::Version,
    method_enum: Method,
    mut response: Response,
    close_after_response: bool,
) -> Option<Answered> {
    // Framing is decided before the head is serialised: it is
    // what puts `Transfer-Encoding: chunked` on the wire, and
    // a stream of unknown length answered to HTTP/1.0 has no
    // framing field at all — the close *is* the end of the
    // message (RFC 9112 §6.3), so the socket cannot outlive
    // the body it carries.
    let framing = response.body_framing(version.minor());
    let mut close_after_response =
        close_after_response || framing == aether_http::response::BodyFraming::UntilClose;
    // HTTP/1 keeps the socket's fate in a header, so the
    // frontend — not the core — states it. A response that
    // already carries a `Connection` field (a 504 from the
    // handler timeout, the 1.0 rejection path) keeps its
    // own wording; the check is on the wire casing the
    // emitter used, matching what the client reads.
    if !response
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("connection"))
    {
        if close_after_response {
            response.add_header("Connection", "close");
        } else if version == crate::serve::Version::Http10 {
            // A persisted HTTP/1.0 connection must advertise it.
            response.add_header("Connection", "keep-alive");
        }
    }
    write_buf.clear();
    // HEAD carries the GET headers (including
    // Content-Length) but no payload, whether the route
    // was explicit or fell back. The status line echoes
    // the request version: a 1.0 client that read
    // `HTTP/1.1` would assume persistence the server has
    // already decided against (RFC 9110 §9.6 permits
    // either, so this is the interoperable choice, not a
    // legality fix).
    response.write_http1_to_for_request_version(
        write_buf,
        version.minor(),
        method_enum != Method::Head,
    );
    // The block this answer needs is priced before the
    // bytes go to the socket, because a client that stops
    // reading leaves the whole serialized response in
    // `write_buf` for as long as the write takes. A budget
    // that refuses is not a reason to withhold an answer
    // the handler already produced: the charge is
    // best-effort, the response is not.
    if write_buffer_charge(write_buf) > 0
        && let Reprice::Updated(value) = reprice_conn_memory(
            conn_guard,
            *conn_charged,
            conn_buffered_charge(conn_buf, write_buf),
            0,
        )
    {
        *conn_charged = value;
    }
    let write_future = async {
        stream.write_all(write_buf).await?;
        stream.flush().await
    };
    // A mid-write IO error (client gone) must end the
    // keep-alive loop; continuing would silently swallow
    // later pipelined responses on a dead socket.
    if tokio::time::timeout(limits_clone.write_timeout, write_future)
        .await
        .map_or(true, |result| result.is_err())
    {
        return None;
    }
    // The bytes are on the wire, so the block that held
    // them is dead weight: hand it back and report the drop
    // before the next decision is priced.
    if write_buffer_charge(write_buf) > 0 {
        release_oversized_write_buf(write_buf);
        if let Reprice::Updated(value) = reprice_conn_memory(
            conn_guard,
            *conn_charged,
            conn_buffered_charge(conn_buf, write_buf),
            0,
        ) {
            *conn_charged = value;
        }
    }
    // The head is on the wire, so the body must follow it.
    // From here the framing is a promise the writer cannot
    // renegotiate: every way a body can fail — a source
    // error, a file that shrank, more bytes than the
    // declared length — ends the *connection* instead of the
    // message, because a client cannot be told mid-body that
    // the length it read is wrong. A `HEAD` and a status that
    // forbids a body never poll the source at all.
    let carries_body =
        method_enum != Method::Head && framing != aether_http::response::BodyFraming::None;
    if carries_body && let Some(mut body) = response.take_stream() {
        let declared = match framing {
            aether_http::response::BodyFraming::ContentLength(len) => Some(len),
            _ => None,
        };
        let chunked = framing == aether_http::response::BodyFraming::Chunked;
        let mut sent: u64 = 0;
        let mut empty_polls = 0u32;
        loop {
            let chunk = match body.next_chunk().await {
                None => break,
                Some(Ok(chunk)) if chunk.is_empty() => {
                    // A zero-length *frame* is the chunked
                    // terminator, never data, so an empty
                    // chunk cannot be forwarded. A source that
                    // keeps answering empty without producing
                    // a byte is spinning, and this loop would
                    // spin with it.
                    empty_polls += 1;
                    if empty_polls > MAX_EMPTY_CHUNKS {
                        close_after_response = true;
                        break;
                    }
                    continue;
                }
                Some(Ok(chunk)) => {
                    empty_polls = 0;
                    chunk
                }
                Some(Err(_)) => {
                    close_after_response = true;
                    break;
                }
            };
            // Never one byte past a declared length: the head
            // already said how many follow, and the extra
            // would be read as the next response.
            let chunk = match declared {
                Some(len) => {
                    let room = len.saturating_sub(sent);
                    if room == 0 {
                        close_after_response = true;
                        break;
                    }
                    if chunk.len() as u64 > room {
                        close_after_response = true;
                        chunk.slice(..room as usize)
                    } else {
                        chunk
                    }
                }
                None => chunk,
            };
            sent += chunk.len() as u64;
            write_buf.clear();
            if chunked {
                aether_http::response::encode_chunk(write_buf, &chunk);
            } else {
                write_buf.extend_from_slice(&chunk);
            }
            // Priced before it is written, exactly like the
            // head: a chunk the budget cannot cover is a body
            // this connection may not hold, and after the
            // head the only honest answer is to close.
            match reprice_conn_memory(
                conn_guard,
                *conn_charged,
                conn_buffered_charge(conn_buf, write_buf),
                0,
            ) {
                Reprice::OverBudget => {
                    close_after_response = true;
                    break;
                }
                Reprice::Updated(value) => *conn_charged = value,
                Reprice::Unchanged => {}
            }
            if !write_buf_to_socket(stream, write_buf, limits_clone.write_timeout).await {
                return None;
            }
            release_stream_write_buf(write_buf);
            if let Reprice::Updated(value) = reprice_conn_memory(
                conn_guard,
                *conn_charged,
                conn_buffered_charge(conn_buf, write_buf),
                0,
            ) {
                *conn_charged = value;
            }
            if declared.is_some_and(|len| sent >= len) {
                break;
            }
        }
        // The terminator is sent only for a body that ended
        // on its own terms. A truncated chunked body stays
        // unterminated on purpose: that is how the client is
        // told the payload is incomplete.
        if chunked && !close_after_response {
            write_buf.clear();
            aether_http::response::encode_last_chunk(write_buf);
            if !write_buf_to_socket(stream, write_buf, limits_clone.write_timeout).await {
                return None;
            }
        }
        // A source that ended before its declared length
        // leaves the head's promise unmet; closing is what
        // keeps the next response from being read as the rest
        // of this body.
        if declared.is_some_and(|len| sent < len) {
            close_after_response = true;
        }
        release_stream_write_buf(write_buf);
        if let Reprice::Updated(value) = reprice_conn_memory(
            conn_guard,
            *conn_charged,
            conn_buffered_charge(conn_buf, write_buf),
            0,
        ) {
            *conn_charged = value;
        }
    }
    // The connection finished its negotiated life. The caller reports the completion and sends the
    // FIN: pipelined bytes may still sit unread in the receive buffer, and a bare close would make
    // Windows reset and discard the response just written.
    if close_after_response {
        return Some(Answered::Close);
    }
    Some(Answered::KeepAlive)
}

/// Discard what the peer already sent, after the answer and the FIN have gone out.
///
/// A response written before the request body was read leaves bytes in the socket's receive queue.
/// Dropping the socket over them makes Windows reset it and destroy the answer the client is still
/// reading, so the frontend half-closes first and only then throws the leftovers away. The wait is
/// bounded by `read_timeout`, the same bound a request that has not finished arriving gets, and
/// the discard allocates nothing: every pass reads into the caller's fixed read buffer.
async fn discard_after_close(
    stream: &mut crate::transport::Stream,
    temp_buf: &mut [u8],
    limits: &ServerLimits,
) {
    let _ = tokio::time::timeout(limits.read_timeout, async {
        loop {
            match stream.read(temp_buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}

/// Answer a receive phase that ran out, and close.
///
/// Two clocks expire while a request is arriving: the parser noticing that the receive budget has
/// passed after a read returned, and the read itself timing out — the per-read idle bound, or
/// what is left of the receive budget when that is shorter. Both are one event to the peer, a
/// request it started and never finished, and RFC 9110 §15.5.9 gives it one answer. The write is
/// bounded by `write_timeout` and the close is a FIN, for the same reasons as the other early
/// refusals; the response carries no body (the h2 body goes to the h2 crate), so this cannot grow
/// `write_buf` past its initial block and nothing here is charged or released.
async fn answer_receive_timeout(
    stream: &mut crate::transport::Stream,
    write_buf: &mut BytesMut,
    limits: &ServerLimits,
) {
    let mut err_res = Response::status(408, "Request Timeout");
    err_res.headers.push(("Connection", "close".to_string()));
    write_buf.clear();
    err_res.write_http1_to(write_buf);
    let _ = tokio::time::timeout(limits.write_timeout, stream.write_all(&write_buf[..])).await;
    let _ = stream.shutdown().await;
}

/// Refuse the request this connection can no longer hold, and make sure the client reads the
/// refusal.
///
/// A client that trips a byte budget is usually still *sending*. Measured 2026-10-26 with the
/// default limits: a chunked upload of 400 000 payload bytes in one-byte chunks is 2,400,000 wire
/// bytes against a connection budget of `max_header_bytes + max_body_bytes` = 2,105,344, so
/// hundreds of kilobytes are still queued in this socket's receive queue when the `413` is
/// written. Closing over unread received bytes makes Windows reset the connection, and the client
/// then reads **nothing** for a request this server had already decided about (`os error 10053`
/// after ~140 ms, where a smaller overshoot that finished sending before the refusal did read its
/// `413`). So the answer goes out first, then the FIN — a client holding its answer knows the
/// exchange is over and stops queueing more — and only then does the socket discard whatever the
/// peer already sent.
///
/// Draining is bounded three ways so it cannot become the attack the budget exists to prevent: the
/// refused blocks are handed back *before* the wait, so those megabytes stop being resident
/// immediately (the guard's registration is released a moment later, at the loop tail, when the
/// task returns); the discard allocates nothing, because every pass reads into the caller's fixed
/// 8 KiB read buffer and throws it away; and the wait is bounded by the same `read_timeout` the
/// receive loop already applies to a request that has not finished arriving, so a client that
/// keeps sending junk pins a task for no longer than one that trickles a legitimate request.
///
/// Three tests, one per claim. `an_over_budget_chunked_upload_is_still_answered_with_413` reads
/// the status line of a 2.4 MB-wire upload refused by the static bound, and fails with `os error
/// 10053` (or reads an empty response) without the drain.
/// `a_buffer_limit_refusal_finishes_the_exchange_without_waiting_for_the_client` holds its half of
/// the socket open after the refusal and must still see the FIN inside 2 s, which is what pins the
/// FIN-before-drain order. `a_memory_budget_refusal_reaches_a_client_that_is_still_sending` pins
/// the same refusal at the re-price site — its status line, `Connection: close`, that the handler
/// never ran, and the completed-not-cancelled tallying; measured on this tree it also caught the
/// missing drain in 3 of 4 repeats, so it is the weaker of the two delivery tests and is not the
/// one the drain is claimed on. That no bytes are resident during the drain is argued from dropping
/// a `BytesMut`, not measured: no test samples RSS.
async fn refuse_over_budget(
    stream: &mut crate::transport::Stream,
    conn_buf: &mut BytesMut,
    chunked_body: &mut aether_http::parser::ChunkedBodyDecoder,
    write_buf: &mut BytesMut,
    temp_buf: &mut [u8],
    limits: &ServerLimits,
) {
    let mut err_res = Response::status(413, aether_http::response::canonical_reason(413));
    err_res.headers.push(("Connection", "close".to_string()));
    write_buf.clear();
    err_res.write_http1_to(write_buf);
    // The refusal makes these bytes worthless, so drop them before waiting rather than holding a
    // full request buffer through the drain.
    *conn_buf = BytesMut::new();
    *chunked_body = aether_http::parser::ChunkedBodyDecoder::new();
    let _ = tokio::time::timeout(limits.write_timeout, stream.write_all(&write_buf[..])).await;
    let _ = stream.shutdown().await;
    let _ = tokio::time::timeout(limits.read_timeout, async {
        loop {
            match stream.read(temp_buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}

/// Keep a live connection's memory registration equal to `CONN_BASE_MEMORY` plus the bytes
/// it currently holds, plus `extra` — per-request headroom the socket does not hold yet but
/// the next phase (e.g. a WebSocket session's reassembly buffers) will.
/// Reserving `max_header_bytes + max_body_bytes` for the whole life of
/// every socket made the memory budget the real connection ceiling — a 256 MiB budget
/// admitted 126 sockets while `max_connections` said 10 000, and 99 % of them were idle
/// keep-alive sockets holding a few hundred bytes.
///
/// Callers pass what the buffer *reserves*, not what it has written: `BytesMut` grows by
/// doubling, so a 1.5 MiB body sits in a 2 MiB block and only the block is real RSS.
pub(crate) fn reprice_conn_memory(
    guard: &Option<crate::scheduling::AdmissionGuard>,
    charged: u64,
    buffered: u64,
    extra: u64,
) -> Reprice {
    let Some(g) = guard else {
        return Reprice::Unchanged;
    };
    let want = CONN_BASE_MEMORY
        .saturating_add(buffered)
        .saturating_add(extra);
    if want == charged {
        return Reprice::Updated(charged);
    }
    let delta = if want > charged {
        (want - charged) as i64
    } else {
        -((charged - want) as i64)
    };
    if g.adjust_memory(delta) {
        Reprice::Updated(want)
    } else {
        Reprice::OverBudget
    }
}

/// Combine repeated field lines for a list-valued header (RFC 9110 §5.3).
pub(crate) fn header_value_multi<'a>(
    headers: &'a HashMap<String, HeaderLines>,
    name: &str,
) -> Option<std::borrow::Cow<'a, str>> {
    // `header_multi` is keyed by the lowercased field name, so a caller that already passes a
    // lowercase literal - every framework caller does - needs no temporary to look it up.
    let hit = match headers.get(name) {
        Some(values) => values,
        None => headers.get(&name.to_ascii_lowercase())?,
    };
    // One field line is the common case and is already a contiguous string in the store;
    // joining it would copy it for nothing.
    match hit.len() {
        1 => Some(std::borrow::Cow::Borrowed(&hit[0])),
        _ => Some(std::borrow::Cow::Owned(hit.join(", "))),
    }
}

/// True when the comma-separated `Connection` header lists `token` (RFC 9110 §5.3, §7.3.1).
/// The whole field value must NOT be compared atomically: clients legitimately send
/// `Connection: keep-alive, close` or `Connection: close, TE`, and an atom check would
/// silently miss the `close` directive, wrongly keeping the socket alive. The token match is
/// case-insensitive with surrounding OWS trimmed, exactly as the ABNF `connection = #token`
/// grammar requires.
pub(crate) fn connection_has_token(
    headers_multi: &HashMap<String, HeaderLines>,
    token: &str,
) -> bool {
    let Some(values) = headers_multi.get("connection") else {
        return false;
    };
    values.iter().any(|v| {
        v.split(',')
            .map(|t| t.trim_matches(|c| c == ' ' || c == '\t'))
            .any(|t| t.eq_ignore_ascii_case(token))
    })
}

/// The first `Try:` line of an E1001 bind failure: move to a free port instead.
///
/// The alternative port is derived from the one that failed, so the advice is about the
/// configuration the process actually used.
fn port_change_advice(port: u16) -> String {
    format!(
        "Change port via AETHER_PORT environment variable (e.g. AETHER_PORT={})",
        port.saturating_add(1)
    )
}

/// The `Try:` lines that find and stop whatever holds this port.
///
/// Both halves of the old line were wrong in different ways. It named `lsof` and `xargs kill`,
/// commands that do not exist on Windows, which is the only platform this tree builds and runs on,
/// and it named port 3000 even when a different port failed — so the one instruction a reader could
/// not ignore was one that reader's shell cannot execute, about a port they never configured.
///
/// The Windows pair filters `netstat` to `LISTENING` rows because that is what makes it a holder:
/// the first version of this advice printed `netstat -ano | findstr :<port>`, and against a server
/// that had just been probed it returned two rows — the listener plus a `TIME_WAIT` client
/// connection to that port whose PID column is 0. A reader following that output would have been
/// told to kill PID 0, which is the System process.
fn port_holder_advice(port: u16) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("Find what holds it: netstat -ano | findstr LISTENING | findstr :{port}"),
            "Stop it: taskkill /PID <pid> /F, the PID being the last column above".into(),
        ]
    } else {
        vec![format!(
            "Kill conflicting process: lsof -ti:{port} | xargs kill -9"
        )]
    }
}

/// Parse a `listen` address into the configuration's host and port.
///
/// Splitting on the first `:` broke every IPv6 literal (`[::1]:3000` became host `[` and
/// port ``). Parse the bracketed form explicitly and otherwise take the last colon, which
/// is the port in both `host:port` and bare `::1:3000`.
///
/// A non-empty but unparseable port is a configuration mistake and must surface, not
/// silently fall back to the default port — the server would otherwise start listening
/// somewhere the caller never asked for.
fn apply_listen_addr(config: &mut AetherConfig, addr: &str) -> Result<(), String> {
    let (host, port) = if let Some(rest) = addr.strip_prefix('[') {
        match rest.split_once("]:") {
            Some((h, p)) => (Some(h), p),
            None => (Some(rest.trim_end_matches(']')), ""),
        }
    } else if addr.matches(':').count() > 1 {
        // An unbracketed address holding more than one colon is an IPv6 literal and its
        // port is unrecoverable: `::1:3000` is itself a valid address, so splitting on the
        // last colon would bind `::` on port 1 — somewhere the caller never asked for.
        // A port with an IPv6 literal must use the bracketed form handled above.
        (Some(addr), "")
    } else {
        match addr.rsplit_once(':') {
            Some((h, p)) => (Some(h), p),
            // A colon-less address is all digits only in the conventional bare-port
            // form (`"3000"`); otherwise it is a host and the port stays default.
            None if !addr.is_empty() && addr.bytes().all(|b| b.is_ascii_digit()) => (None, addr),
            None => (Some(addr), ""),
        }
    };

    if let Some(h) = host
        && !h.is_empty()
    {
        config.host = h.to_string();
    }
    if !port.is_empty() {
        let p: u16 = port.parse().map_err(|_| {
            format!(
                "Aether: invalid port '{port}' in listen address '{addr}'; expected a number in 0..=65535"
            )
        })?;
        config.port = p;
    }
    Ok(())
}

/// Wait for the platform shutdown signal: `Ctrl+C` everywhere, plus `SIGTERM` on Unix.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(sig) => sig,
            Err(e) => {
                eprintln!("Failed to install SIGTERM handler: {}", e);
                match signal::ctrl_c().await {
                    Ok(_) => eprintln!("\n⚡ Received Ctrl+C, initiating graceful shutdown..."),
                    Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
                }
                return;
            }
        };

        tokio::select! {
            result = signal::ctrl_c() => {
                match result {
                    Ok(_) => eprintln!("\n⚡ Received Ctrl+C, initiating graceful shutdown..."),
                    Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
                }
            }
            _ = sigterm.recv() => {
                eprintln!("\n⚡ Received SIGTERM, initiating graceful shutdown...");
            }
        }
    }

    #[cfg(not(unix))]
    {
        match signal::ctrl_c().await {
            Ok(_) => eprintln!("\n⚡ Received Ctrl+C, initiating graceful shutdown..."),
            Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> (String, u16) {
        let c = AetherConfig::default();
        (c.host, c.port)
    }

    fn listen(addr: &str) -> (String, u16) {
        let mut c = AetherConfig::default();
        apply_listen_addr(&mut c, addr).expect("address must parse");
        (c.host, c.port)
    }

    #[test]
    fn listen_addr_forms_select_the_requested_host_and_port() {
        let (dh, dp) = defaults();
        assert_eq!(listen("127.0.0.1:8080"), ("127.0.0.1".to_string(), 8080));
        assert_eq!(listen("[::1]:8080"), ("::1".to_string(), 8080));
        assert_eq!(listen("[::1]"), ("::1".to_string(), dp));
        assert_eq!(listen("0.0.0.0"), ("0.0.0.0".to_string(), dp));
        assert_eq!(listen("3000"), (dh.clone(), 3000));
    }

    #[test]
    fn bare_ipv6_literal_keeps_its_address_instead_of_losing_a_group() {
        let (_, dp) = defaults();
        // `::1` once split into host `::` and port 1, binding somewhere else entirely.
        assert_eq!(listen("::1"), ("::1".to_string(), dp));
        assert_eq!(listen("::"), ("::".to_string(), dp));
        // `::1:3000` is itself a valid address, so an unbracketed port is not recoverable.
        assert_eq!(listen("::1:3000"), ("::1:3000".to_string(), dp));
    }

    fn active_workers(app: &AetherApp) -> usize {
        let runtime = app.build_runtime().expect("runtime");
        runtime.block_on(async { tokio::runtime::Handle::current().metrics().num_workers() })
    }

    /// `workers` reached the banner and nothing else: `AETHER_WORKERS` was parsed, displayed and
    /// ignored. The count given to `build_runtime` is now the count Tokio starts.
    #[test]
    fn build_runtime_starts_the_configured_number_of_workers() {
        let config = AetherConfig {
            workers: 3,
            ..AetherConfig::default()
        };
        assert_eq!(active_workers(&AetherApp::with_config(config)), 3);
    }

    #[test]
    fn zero_workers_is_served_by_one_instead_of_a_builder_error() {
        let config = AetherConfig {
            workers: 0,
            ..AetherConfig::default()
        };
        assert_eq!(active_workers(&AetherApp::with_config(config)), 1);
    }

    /// `run` builds a runtime, so it must refuse to be called from inside one — the alternative
    /// is Tokio's "Cannot start a runtime from within a runtime" panic in the caller's face.
    #[test]
    fn run_refuses_to_nest_inside_an_existing_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let error = runtime
            .block_on(async { AetherApp::new().run("127.0.0.1:0") })
            .expect_err("run must refuse a nested runtime");
        assert!(error.to_string().contains("listen"), "{error}");
    }

    /// `AetherConfig` is what an operator sets, `ServerLimits` is what the frontends enforce. The
    /// one derivation between them must carry every configurable bound: a field dropped here is a
    /// number that is configured, documented, and never used.
    #[test]
    fn config_derivation_carries_the_header_and_stream_bounds() {
        let server_default = ServerLimits::default();
        let from_default = AetherApp::limits_for(&AetherConfig::default());
        // The whole struct, field by field: a default that quietly moved would fail here rather
        // than only showing up as a changed limit in production.
        assert_eq!(from_default.max_body_bytes, server_default.max_body_bytes);
        assert_eq!(
            from_default.max_header_bytes,
            server_default.max_header_bytes
        );
        assert_eq!(
            (
                from_default.max_headers_count,
                from_default.max_concurrent_streams
            ),
            (
                server_default.max_headers_count,
                server_default.max_concurrent_streams
            ),
            "the default configuration must not restate a bound the server already carries"
        );
        assert_eq!(from_default.request_timeout, server_default.request_timeout);
        assert_eq!(from_default.receive_timeout, server_default.receive_timeout);
        assert_eq!(from_default.write_timeout, server_default.write_timeout);
        assert_eq!(
            from_default.keep_alive_timeout,
            server_default.keep_alive_timeout
        );
        // Fields `AetherConfig` does not carry must survive the derivation untouched.
        assert_eq!(from_default.max_connections, server_default.max_connections);
        assert_eq!(from_default.read_timeout, server_default.read_timeout);
        assert_eq!(
            from_default.tls_handshake_timeout,
            server_default.tls_handshake_timeout
        );

        let configured = AetherConfig {
            max_headers_count: 7,
            max_concurrent_streams: 3,
            max_receive_time: Duration::from_secs(2),
            ..AetherConfig::default()
        };
        let from_configured = AetherApp::limits_for(&configured);
        assert_eq!(
            (
                from_configured.max_headers_count,
                from_configured.max_concurrent_streams
            ),
            (7, 3)
        );
        assert_eq!(
            (
                from_configured.receive_timeout,
                from_configured.request_timeout
            ),
            (Duration::from_secs(2), server_default.request_timeout),
            "the receive budget is its own field, and the handler budget is untouched"
        );
        assert_eq!(
            from_configured.max_body_bytes,
            server_default.max_body_bytes
        );

        // The second site, not just the first: `bind_with_config` re-derives through
        // `apply_config`, which used to keep the limits of whichever config the app was built
        // with. Both now go through the same function, and this is what shows it.
        let mut app = AetherApp::new();
        app.apply_config(configured.clone());
        assert_eq!(
            (
                app.limits.max_headers_count,
                app.limits.max_concurrent_streams
            ),
            (7, 3),
            "`apply_config` left the limits it derived at construction in place"
        );
        assert_eq!(app.config.max_concurrent_streams, 3);
    }

    #[test]
    fn non_numeric_port_is_an_error_not_a_silent_default() {
        let mut c = AetherConfig::default();
        assert!(apply_listen_addr(&mut c, "127.0.0.1:http").is_err());
        assert!(apply_listen_addr(&mut c, "127.0.0.1:70000").is_err());
    }

    /// The E1001 hint once ordered every reader to run `lsof … | xargs kill -9` — commands that do
    /// not exist on the only platform this tree builds — about port 3000 whatever had actually
    /// failed. Asserted by content, not by shape: the commands named, and the port carried.
    #[test]
    fn bind_advice_names_the_failed_port_and_a_command_this_platform_has() {
        let advice = port_holder_advice(8123).join("\n");
        assert!(
            advice.contains("8123"),
            "advice lost the failing port: {advice}"
        );
        assert!(
            !advice.contains("3000"),
            "advice hard-codes a port: {advice}"
        );
        if cfg!(windows) {
            assert!(
                advice.contains("netstat") && advice.contains("taskkill"),
                "Windows advice names no command Windows has: {advice}"
            );
            assert!(
                !advice.contains("lsof"),
                "Windows advice says lsof: {advice}"
            );
            // Unfiltered, `findstr :PORT` also matches TIME_WAIT clients connecting TO that port,
            // and their PID column is 0 — the advice would send the reader to kill the System
            // process. Measured against a live server on 3215, which is where this came from.
            assert!(
                advice.contains("LISTENING"),
                "Windows advice can name a connection, not a holder: {advice}"
            );
        } else {
            assert!(
                advice.contains("lsof"),
                "non-Windows advice lost lsof: {advice}"
            );
        }
        let change = port_change_advice(8123);
        assert!(
            change.contains("AETHER_PORT=8124"),
            "alternative port: {change}"
        );
    }

    fn multi(pairs: &[(&str, &[&str])]) -> HashMap<String, HeaderLines> {
        let mut map: HashMap<String, HeaderLines> = HashMap::new();
        for (name, values) in pairs {
            map.insert(
                (*name).to_string(),
                values.iter().map(|v| (*v).to_string()).collect(),
            );
        }
        map
    }

    #[test]
    fn duplicated_single_value_fields_resolve_to_the_first_wire_line() {
        let headers = multi(&[("origin", &["https://a.example", "https://b.example"])]);
        assert_eq!(
            header_first(&headers, "origin").as_deref(),
            Some("https://a.example")
        );
        assert_eq!(header_first(&headers, "missing"), None);
    }

    #[test]
    fn connection_tokens_are_read_per_token_not_per_line() {
        assert!(connection_has_token(
            &multi(&[("connection", &["keep-alive, close"])]),
            "close"
        ));
        assert!(connection_has_token(
            &multi(&[("connection", &["\u{FFFD}close", "close"])]),
            "close"
        ));
        assert!(!connection_has_token(
            &multi(&[("connection", &["keep-alive"])]),
            "close"
        ));
    }

    /// More field lines than the inline capacity must spill to the heap without losing one:
    /// a list-valued field is comma-joined across *every* line (RFC 9110 §5.3).
    #[test]
    fn lines_beyond_the_inline_capacity_are_still_all_read() {
        let headers = multi(&[
            (
                "accept-encoding",
                &["gzip", "br", "zstd", "deflate", "identity"],
            ),
            ("cookie", &["a=1", "b=2", "c=3", "d=4", "e=5", "f=6"]),
        ]);
        assert_eq!(
            header_value_multi(&headers, "accept-encoding").as_deref(),
            Some("gzip, br, zstd, deflate, identity")
        );
        assert_eq!(header_first(&headers, "cookie").as_deref(), Some("a=1"));
        assert!(connection_has_token(
            &multi(&[("connection", &["keep-alive", "Upgrade", "close"])]),
            "close"
        ));
    }

    /// A connection that has read nothing must still cost exactly its base: the 8 KiB block
    /// allocated before the first byte is what `CONN_BASE_MEMORY` pays for, so pricing
    /// capacity may not push the idle charge (and therefore the admitted-connection count at
    /// the default budget) above what it was while len was charged.
    #[test]
    fn fresh_buffer_adds_nothing_to_the_base_charge() {
        let buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
        assert_eq!(buf.capacity(), CONN_BUF_INITIAL_CAPACITY);
        assert_eq!(conn_buffer_charge(&buf), 0);
    }

    /// `bytes` hands out a block, not a length: growing an 8 KiB buffer in 8 KiB reads to
    /// hold 1.5 MiB reserves 2 MiB. The registration must follow the block, which is the
    /// memory the process really holds.
    #[test]
    fn buffer_charge_follows_the_reserved_block_not_the_bytes_written() {
        let mut buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
        for _ in 0..(1_572_864 / 8192) {
            buf.extend_from_slice(&[7u8; 8192]);
        }
        assert_eq!(buf.len(), 1_572_864);
        assert_eq!(
            buf.capacity(),
            2 * 1_024 * 1024,
            "the doubling measured for bytes 1.12.1 is what this pricing assumes"
        );
        assert_eq!(
            conn_buffer_charge(&buf),
            (2 * 1_024 * 1024 - CONN_BUF_INITIAL_CAPACITY) as u64,
            "reserved bytes below the written length were not priced"
        );
        assert!(conn_buffer_charge(&buf) > buf.len() as u64);
    }

    /// The block that doubled for one body stays doubled: `split_to` reclaims the front, it
    /// does not shrink the allocation. So an emptied oversized buffer is replaced, otherwise
    /// an honest capacity price would tax an idle keep-alive socket forever for a single
    /// earlier upload — and the RSS is real until the block is handed back.
    #[test]
    fn emptied_oversized_buffer_is_released_when_the_request_is_done() {
        let mut buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
        // The read loop's shape matters: 8 KiB per read doubles the block, while one big
        // `extend_from_slice` grows it to fit exactly and leaves nothing to reclaim.
        for _ in 0..(1_572_864 / 8192) {
            buf.extend_from_slice(&[7u8; 8192]);
        }
        let _ = buf.split_to(1_572_864);
        assert!(buf.is_empty());
        assert!(
            buf.capacity() > 2 * CONN_BUF_INITIAL_CAPACITY,
            "precondition: split_to reclaims the front, it does not shrink the block (got {})",
            buf.capacity()
        );

        if buf.is_empty() && buf.capacity() > 2 * CONN_BUF_INITIAL_CAPACITY {
            buf = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
        }
        assert_eq!(conn_buffer_charge(&buf), 0);
    }

    /// The response side starts where the read side finished: a connection that has only ever
    /// written small answers must cost nothing above its base, or the honest capacity price
    /// would tax every idle keep-alive socket for a block it never grew.
    #[test]
    fn fresh_write_buffer_adds_nothing_to_the_base_charge() {
        let buf = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
        assert_eq!(buf.capacity(), WRITE_BUF_INITIAL_CAPACITY);
        assert_eq!(write_buffer_charge(&buf), 0);
        let read = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
        assert_eq!(conn_buffered_charge(&read, &buf), 0);
    }

    /// A 2 MiB answer is serialized into `write_buf` before `write_all` hands it to the socket,
    /// and the block it reserves is larger than the bytes it holds. Both blocks are the
    /// connection's real RSS, so the charge is the sum, not the read block alone.
    #[test]
    fn a_large_answer_is_priced_by_the_block_it_reserved() {
        let mut write = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
        for _ in 0..(1_572_864 / 1024) {
            write.extend_from_slice(&[7u8; 1024]);
        }
        assert_eq!(write.len(), 1_572_864);
        assert_eq!(
            write.capacity(),
            2 * 1_024 * 1024,
            "the doubling measured for bytes 1.12.1 is what this pricing assumes"
        );
        assert_eq!(
            write_buffer_charge(&write),
            (2 * 1_024 * 1024 - WRITE_BUF_INITIAL_CAPACITY) as u64
        );
        assert!(write_buffer_charge(&write) > write.len() as u64);

        let mut read = BytesMut::with_capacity(CONN_BUF_INITIAL_CAPACITY);
        read.extend_from_slice(&[1u8; 4096]);
        assert_eq!(
            conn_buffered_charge(&read, &write),
            conn_buffer_charge(&read) + write_buffer_charge(&write),
            "one of the two buffers was priced out of the registration"
        );
    }

    /// `clear()` reclaims the length, never the block, so the release has to replace the
    /// buffer. Under the doubled headroom the block is kept: a connection answering many
    /// small responses must not reallocate per answer.
    #[test]
    fn oversized_write_buffer_is_handed_back_and_small_ones_are_kept() {
        let mut write = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
        for _ in 0..(1_572_864 / 1024) {
            write.extend_from_slice(&[7u8; 1024]);
        }
        write.clear();
        assert!(write.capacity() > 2 * WRITE_BUF_INITIAL_CAPACITY);

        let kept = write.capacity();
        let mut small = BytesMut::with_capacity(WRITE_BUF_INITIAL_CAPACITY);
        small.extend_from_slice(b"HTTP/1.1 200 OK\r\n\r\nok");
        small.clear();
        release_oversized_write_buf(&mut small);
        assert_eq!(
            small.capacity(),
            WRITE_BUF_INITIAL_CAPACITY,
            "a buffer inside the headroom must survive untouched"
        );

        release_oversized_write_buf(&mut write);
        assert_eq!(write.capacity(), WRITE_BUF_INITIAL_CAPACITY);
        assert_eq!(write_buffer_charge(&write), 0);
        assert!(kept > 2 * WRITE_BUF_INITIAL_CAPACITY);
    }

    struct Shadowed(&'static str);
    struct AppOnly(&'static str);
    struct NeverRegistered;

    fn bare_ctx(state: &Arc<AppStateContainer>) -> RequestContext {
        RequestContext::from_parts(
            RequestParts {
                raw_uri: "/x".to_string(),
                path: "/x".to_string(),
                query_str: None,
                method: Method::Get,
                param_map: HashMap::new(),
                header_multi: HashMap::new(),
                body: Bytes::new(),
                body_stream: None,
                request_id: "t".to_string(),
                request_serial: 1,
                peer_addr: None,
                extensions: crate::extensions::Extensions::new(),
            },
            state,
        )
    }

    fn app_container() -> Arc<AppStateContainer> {
        let mut container = AppStateContainer::new();
        container.insert(Shadowed("app"));
        container.insert(AppOnly("app"));
        Arc::new(container)
    }

    /// The resolution rule of a scoped route: the scope's own value wins for the type it
    /// registered, and every type it never mentions still comes from the app.
    ///
    /// Both halves are the contract. Answering the shadowed type from the app container would
    /// make a mount a no-op; answering an unshadowed type from the overlay's absence would make
    /// a mount hide the application's own services, which is the failure mode that turns a
    /// sub-app into a wall instead of a feature.
    #[test]
    fn the_scoped_overlay_answers_its_own_type_and_the_app_answers_the_rest() {
        let app = app_container();
        let mut ctx = bare_ctx(&app);
        assert_eq!(ctx.state::<Shadowed>().unwrap().0, "app");
        assert_eq!(ctx.state::<AppOnly>().unwrap().0, "app");

        let mut overlay = AppStateContainer::new();
        overlay.insert(Shadowed("scoped"));
        ctx.attach_scoped_state(Arc::new(overlay));

        assert_eq!(ctx.state::<Shadowed>().unwrap().0, "scoped");
        assert_eq!(
            ctx.state::<AppOnly>().unwrap().0,
            "app",
            "a scope that never mentions a type must not hide the app's value for it"
        );
        assert!(ctx.state::<NeverRegistered>().is_none());
    }

    /// A group that registered neither state nor middleware contributes no chain entry, so its
    /// routes are compiled to exactly the chain a plain registration gets. With state alone the
    /// entry is the overlay and it sits **in front of** the group's own middleware, which is what
    /// lets a guard read the same value as the handler it guards.
    #[test]
    fn a_scope_contributes_one_entry_for_state_and_none_for_nothing() {
        let mut app = AetherApp::new();
        app = app.group("/bare", |g| {
            assert!(
                g.scope().is_empty(),
                "an empty group must not add a layer to its routes"
            );
        });
        app = app.group("/state", |g| {
            g.manage(Shadowed("scoped"));
            assert_eq!(g.scope().len(), 1, "state alone costs one entry");
        });
        app.group("/both", |g| {
            g.manage(Shadowed("scoped"));
            g.use_middleware(|ctx, next| async move { next(ctx).await });
            assert_eq!(g.scope().len(), 2, "state plus one guard is two entries");
        });
    }

    /// The overlay entry runs before the scope's middleware, and the app-wide stack still runs
    /// before the overlay. Reordering these two is invisible to any test that only reads state in
    /// a handler, so the chain itself is driven here.
    #[tokio::test]
    async fn the_overlay_reaches_the_scope_middleware_and_not_the_app_stack() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let mut app = AetherApp::new();
        app = app.manage(Shadowed("app"));
        app = app.use_middleware({
            let seen = Arc::clone(&seen);
            move |ctx: RequestContext, next: Next| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.lock()
                        .unwrap()
                        .push(ctx.state::<Shadowed>().unwrap().0);
                    next(ctx).await
                }
            }
        });
        let scoped_id = app.next_id;
        app = app.group("/g", |g| {
            g.manage(Shadowed("scoped"));
            g.use_middleware({
                let seen = Arc::clone(&seen);
                move |ctx: RequestContext, next: Next| {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.lock()
                            .unwrap()
                            .push(ctx.state::<Shadowed>().unwrap().0);
                        next(ctx).await
                    }
                }
            });
            g.get_async("/x", {
                let seen = Arc::clone(&seen);
                move |ctx: RequestContext| {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.lock()
                            .unwrap()
                            .push(ctx.state::<Shadowed>().unwrap().0);
                        "ok"
                    }
                }
            });
        });

        let chain = app.handlers.get(&scoped_id).expect("the scoped route");
        chain(bare_ctx(&app_container())).await;
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec!["app", "scoped", "scoped"],
            "the app stack must see the app's value, the scope's guard and handler the scope's"
        );
    }

    /// A nested group shares its parent's container until it registers a value of its own, and
    /// the first write copies out instead of editing the shared map. Mutating in place would
    /// publish the inner value to every route of the outer group — a leak one level up that no
    /// wire test of the inner route can see.
    #[test]
    fn a_child_group_copies_the_container_out_on_its_first_value() {
        let app = AetherApp::new();
        app.group("/outer", |outer| {
            outer.manage(Shadowed("outer"));
            let outer_state = Arc::as_ptr(&outer.state);
            outer.group("/inner", |inner| {
                assert_eq!(
                    Arc::as_ptr(&inner.state),
                    outer_state,
                    "a child must share its parent's container, not clone it per level"
                );
                inner.manage(Shadowed("inner"));
                assert_ne!(
                    Arc::as_ptr(&inner.state),
                    outer_state,
                    "registering a value must not write through the shared handle"
                );
            });
            assert_eq!(
                outer.state.get::<Shadowed>().unwrap().0,
                "outer",
                "the inner value leaked into the outer group"
            );
        });
    }

    /// `IntoResponseError` picks the status in this crate and `AetherError::error_code` picks the
    /// code in `aether-core`. Their agreement was documented on `error_code` and checked nowhere:
    /// `typed_handler_errors_answer_their_own_class` covers four variants on the wire, and the
    /// rest could drift one variant at a time. This walks every variant through the *conversion*
    /// and reads the status, the code in the JSON body, the detail and the redaction together —
    /// the four facts that made PROB-0009 a contradiction rather than a typo.
    #[test]
    fn every_aether_error_answers_the_status_class_its_code_names() {
        let expected: [(AetherError, u16, &str); 12] = [
            (AetherError::HeaderTooLarge, 431, "E3002"),
            (AetherError::BodyLimitExceeded, 413, "E3002"),
            (AetherError::MalformedRequest, 400, "E3001"),
            (AetherError::UnsupportedExpectation, 417, "E3003"),
            (AetherError::RouteNotFound, 404, "E2002"),
            (AetherError::MethodNotAllowed, 405, "E2003"),
            (
                AetherError::ValidationFailed("bad".to_string()),
                400,
                "E4002",
            ),
            (
                AetherError::DependencyCycleDetected("cycle".to_string()),
                500,
                "E5001",
            ),
            (AetherError::Timeout, 504, "E5002"),
            (
                AetherError::Io(std::io::Error::other("disk says no")),
                500,
                "E5002",
            ),
            (
                AetherError::Serialization("missing".to_string()),
                400,
                "E4001",
            ),
            (AetherError::Custom("nope".to_string()), 400, "E4003"),
        ];

        for (error, status, code) in expected {
            assert_eq!(error.error_status(), status, "{error:?}");
            assert_eq!(error.error_code().code_str(), code, "{error:?}");
            assert_eq!(
                error.to_diagnostic().code.code_str(),
                code,
                "the developer's diagnostic and the wire body must name one code: {error:?}"
            );

            let internal = status >= 500;
            assert_eq!(
                internal,
                code.starts_with("E5"),
                "a {} status carries {}: {error:?}",
                if internal { "5xx" } else { "4xx" },
                code
            );
            assert_eq!(
                error.error_detail().is_some(),
                !internal,
                "a 5xx body must redact its detail and a 4xx must send it: {error:?}"
            );
            assert_eq!(
                error.redacted_detail().is_some(),
                internal,
                "only a redacted 5xx detail reaches the error reporter: {error:?}"
            );

            let response = Result::<&'static str, AetherError>::Err(error).into_response();
            assert_eq!(response.status, status);
            let body = String::from_utf8_lossy(&response.body);
            assert!(
                body.contains(&format!("\"code\":\"{code}\"")),
                "the answer's own code is missing from its body: {body}"
            );
            if internal {
                assert!(
                    !body.contains("disk says no"),
                    "an internal detail reached the wire: {body}"
                );
            }
        }
    }
}
