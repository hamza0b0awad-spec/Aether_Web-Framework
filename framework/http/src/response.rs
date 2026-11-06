//! # Aether Response Builder
//!
//! Pre-formats HTTP/1.1 response status lines and headers directly into
//! output bytes slices to eliminate temporary allocations.

use bytes::{Bytes, BytesMut};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// One chunk of a streamed response body.
pub type Chunk = std::io::Result<Bytes>;

/// A pull-based source of response-body chunks.
///
/// The writer asks for one chunk, sends it, and only then asks for the next, so a producer cannot
/// outrun the client: what a streamed response holds is one chunk plus whatever the socket or the
/// HTTP/2 send buffer has not drained. `None` ends the body. An empty chunk is legal and skipped by
/// the writer, because a zero-length *frame* is the chunked terminator, never data.
pub trait BodySource: Send + 'static {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>>;
}

/// A streaming response body, boxed so one `Response` type carries both shapes.
pub struct BodyStream {
    source: Box<dyn BodySource>,
}

impl BodyStream {
    pub fn new(source: impl BodySource) -> Self {
        Self {
            source: Box::new(source),
        }
    }

    /// A body made of pieces already in memory, sent one piece per write.
    pub fn from_chunks(chunks: impl IntoIterator<Item = Bytes>) -> Self {
        struct Chunks(std::vec::IntoIter<Bytes>);
        impl BodySource for Chunks {
            fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
                let next = self.0.next();
                Box::pin(async move { next.map(Ok) })
            }
        }
        Self::new(Chunks(chunks.into_iter().collect::<Vec<_>>().into_iter()))
    }

    /// The next chunk, or `None` at the end of the body.
    pub async fn next_chunk(&mut self) -> Option<Chunk> {
        self.source.next_chunk().await
    }
}

/// A `BodyStream` is itself a source, so a stream can be nested, stored, or handed on without
/// being unwrapped first.
impl BodySource for BodyStream {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
        Box::pin(BodyStream::next_chunk(self))
    }
}

impl fmt::Debug for BodyStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BodyStream")
    }
}

/// How a response's payload is framed on an HTTP/1 wire (RFC 9112 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFraming {
    /// The status forbids a body (1xx, 204, 304): neither length nor coding may be sent.
    None,
    /// `Content-Length: n` — the framing whenever the size is known up front.
    ContentLength(u64),
    /// `Transfer-Encoding: chunked` — an HTTP/1.1 stream of unknown length.
    Chunked,
    /// No framing field: the body ends when the connection closes, the only option an HTTP/1.0
    /// stream of unknown length has (RFC 9112 §6.3).
    UntilClose,
}

pub struct Response {
    pub status: u16,
    pub status_text: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Bytes,
    /// When set, the writers ignore `body` and drain this source instead. The constructors keep
    /// `body` empty whenever a stream is attached, and the head writer decides the framing from
    /// the declared length (if any) and the status.
    pub stream: Option<BodyStream>,
}

/// Percent-encodes a Set-Cookie component. `is_value=false` applies the RFC 7230
/// token set (cookie names must be tokens); `is_value=true` applies the RFC 6265
/// cookie-value octet set, which keeps `=` but forbids `;`, `,`, `"`/`\`, space
/// and control characters. Bytes outside the allowed set (including every byte of
/// a non-ASCII sequence) become `%XX` with uppercase hex.
fn encode_cookie_component(s: &str, is_value: bool) -> String {
    fn allowed(b: u8, is_value: bool) -> bool {
        if is_value {
            b == 0x21
                || (0x23..=0x2B).contains(&b)
                || (0x2D..=0x3A).contains(&b)
                || (0x3C..=0x5B).contains(&b)
                || (0x5D..=0x7E).contains(&b)
        } else {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        }
    }
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        if allowed(*b, is_value) {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

impl Response {
    pub fn ok() -> Self {
        Self {
            status: 200,
            status_text: "OK",
            headers: Vec::with_capacity(6),
            body: Bytes::new(),
            stream: None,
        }
    }

    pub fn json(body: Bytes) -> Self {
        let mut r = Self::ok();
        r.headers.push((
            "Content-Type",
            "application/json; charset=utf-8".to_string(),
        ));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = body;
        r
    }

    /// Creates a JSON response with a custom HTTP status code and status text
    pub fn json_status(status: u16, status_text: &'static str, body: impl Into<Bytes>) -> Self {
        let b = body.into();
        let mut r = Self::status(status, status_text);
        r.headers.push((
            "Content-Type",
            "application/json; charset=utf-8".to_string(),
        ));
        r.headers.push(("Content-Length", b.len().to_string()));
        r.body = b;
        r
    }

    pub fn text(body: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers
            .push(("Content-Type", "text/plain; charset=utf-8".to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = Bytes::from_static(body.as_bytes());
        r
    }

    pub fn html(body: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers
            .push(("Content-Type", "text/html; charset=utf-8".to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = Bytes::from_static(body.as_bytes());
        r
    }

    /// Creates an HTML response from a dynamic owned String
    pub fn html_dynamic(body: impl Into<String>) -> Self {
        let b = body.into();
        let bytes = Bytes::from(b.into_bytes());
        Self::bytes(bytes, "text/html; charset=utf-8")
    }

    /// Creates a Plain Text response from a dynamic owned String
    pub fn text_dynamic(body: impl Into<String>) -> Self {
        let b = body.into();
        let bytes = Bytes::from(b.into_bytes());
        Self::bytes(bytes, "text/plain; charset=utf-8")
    }

    /// Reads and serves a file directly from the filesystem with automatic MIME type detection.
    ///
    /// Blocking: performs `std::fs::read` synchronously. Call it outside runtime workers
    /// (startup, build, tests); async routes use `App::static_file`/`static_dir` instead.
    pub fn file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read(path)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mime = crate::static_files::mime_from_extension(ext);
        Ok(Self::bytes(Bytes::from(content), mime))
    }

    pub fn bytes(body: Bytes, content_type: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers.push(("Content-Type", content_type.to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = body;
        r
    }

    pub fn status(status: u16, msg: &'static str) -> Self {
        Self {
            status,
            status_text: msg,
            headers: Vec::with_capacity(4),
            body: Bytes::new(),
            stream: None,
        }
    }

    /// Creates an HTTP redirect response (301 Moved Permanently, 302 Found, 307 Temporary Redirect, or 308 Permanent Redirect)
    pub fn redirect(location: impl Into<String>, status: u16) -> Self {
        let mut r = Self::status(status, canonical_reason(status));
        r.headers.push(("Location", location.into()));
        r
    }

    /// A streamed body of unknown length.
    ///
    /// HTTP/1.1 frames it with `Transfer-Encoding: chunked`; HTTP/1.0 has no chunked coding, so
    /// the connection is closed to mark the end of the body. HTTP/2 has no chunked coding at all
    /// and needs none: the source's chunks become DATA frames. Nothing buffers the whole body, so
    /// what this response holds is one chunk plus what the wire has not drained.
    pub fn stream(source: impl BodySource, content_type: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers.push(("Content-Type", content_type.to_string()));
        r.stream = Some(BodyStream::new(source));
        r
    }

    /// A streamed body whose length is known in advance.
    ///
    /// `Content-Length` framing keeps the connection persistent and lets a client show progress.
    /// A source that ends early or runs long cannot be repaired mid-body, so the writer closes the
    /// connection rather than let the next response be read against the wrong boundary.
    pub fn stream_with_length(
        source: impl BodySource,
        content_type: &'static str,
        len: u64,
    ) -> Self {
        let mut r = Self::stream(source, content_type);
        r.headers.push(("Content-Length", len.to_string()));
        r
    }

    /// Whether the payload comes from a [`BodySource`] rather than `body`.
    pub fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    /// Take the stream out for the writer that drains it. The head must be serialised before this
    /// call: afterwards the response no longer knows it was streaming.
    pub fn take_stream(&mut self) -> Option<BodyStream> {
        self.stream.take()
    }

    /// How this response's payload must be framed for the request version `minor`.
    ///
    /// The framing is derived, never trusted: a `Content-Length` in `headers` is honoured only when
    /// a stream is attached (it is then the declared length of that stream), and every other
    /// response is framed from `body.len()` exactly as before.
    pub fn body_framing(&self, minor: u8) -> BodyFraming {
        if status_forbids_body(self.status) {
            return BodyFraming::None;
        }
        if self.stream.is_some() {
            if let Some(len) = self.declared_content_length() {
                return BodyFraming::ContentLength(len);
            }
            return if minor == 0 {
                BodyFraming::UntilClose
            } else {
                BodyFraming::Chunked
            };
        }
        BodyFraming::ContentLength(self.body.len() as u64)
    }

    /// The length a streaming response declared for itself, if it declared a usable one.
    fn declared_content_length(&self) -> Option<u64> {
        self.headers.iter().find_map(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
    }

    /// Attaches a secure Cookie to the response (`Set-Cookie` header)
    ///
    /// Name and value are percent-encoded wherever RFC 6265 forbids the byte
    /// (`;`, `,`, whitespace, `"`/`\` in the value; any non-token byte in the
    /// name), so a caller-supplied string can never inject additional cookie
    /// attributes. Legitimate values round-trip byte-identically. `SameSite`
    /// accepts only the three registered spellings; anything else falls back
    /// to `Lax`.
    pub fn set_cookie(
        mut self,
        name: &str,
        value: &str,
        http_only: bool,
        secure: bool,
        same_site: &str,
        max_age_secs: Option<u64>,
    ) -> Self {
        self.add_cookie(name, value, http_only, secure, same_site, max_age_secs);
        self
    }

    /// As [`Self::set_cookie`], for an owner that holds `&mut Response` — the serving pipeline's
    /// own post-processing being the one that has no value to consume and return.
    pub fn add_cookie(
        &mut self,
        name: &str,
        value: &str,
        http_only: bool,
        secure: bool,
        same_site: &str,
        max_age_secs: Option<u64>,
    ) {
        let same_site = if same_site.eq_ignore_ascii_case("strict") {
            "Strict"
        } else if same_site.eq_ignore_ascii_case("none") {
            "None"
        } else {
            "Lax"
        };
        let mut cookie_str = format!(
            "{}={}; SameSite={}; Path=/",
            encode_cookie_component(name, false),
            encode_cookie_component(value, true),
            same_site
        );
        if http_only {
            cookie_str.push_str("; HttpOnly");
        }
        if secure {
            cookie_str.push_str("; Secure");
        }
        if let Some(age) = max_age_secs {
            cookie_str.push_str(&format!("; Max-Age={}", age));
        }
        self.headers.push(("Set-Cookie", cookie_str));
    }

    /// Adds a custom header (builder pattern)
    pub fn header(mut self, key: &'static str, val: impl Into<String>) -> Self {
        self.headers.push((key, val.into()));
        self
    }

    /// Adds a custom header in-place
    pub fn add_header(&mut self, key: &'static str, val: impl Into<String>) {
        self.headers.push((key, val.into()));
    }

    /// Formats the complete HTTP/1.1 payload into a reusable buffer with zero intermediate String allocations.
    ///
    /// This function is the sole authority for message framing: any `Content-Length` present in
    /// `self.headers` is ignored and exactly one is derived from `self.body`. Emitting a second
    /// `Content-Length` from a constructor or middleware would be a response-splitting vector, so
    /// the invariant is enforced here rather than trusted at every call site.
    ///
    /// The one exception is a response that carries a [`BodyStream`]: its declared
    /// `Content-Length`, when it has one, is the stream's length and is emitted verbatim, and the
    /// payload itself is *not* written here — only the built-in frontends drain a stream, one
    /// chunk at a time, after this call.
    ///
    /// Header values are sanitised for the same reason. `Location` and `Set-Cookie` routinely
    /// carry caller- or client-derived data, and RFC 9110 §5.5 forbids CR, LF and NUL inside a
    /// field value — a value containing them would let an attacker inject whole response headers.
    pub fn write_http1_to(&self, out: &mut BytesMut) {
        self.write_http1_response(out, 1, true);
    }

    /// As [`Self::write_http1_to`], but optionally omits the body bytes.
    ///
    /// A `HEAD` response must carry the header fields — including the `Content-Length` — that
    /// the equivalent `GET` would have produced, while sending no content (RFC 9110 §9.3.2).
    /// Passing `false` preserves the framing headers and drops only the payload.
    pub fn write_http1_to_with_body(&self, out: &mut BytesMut, include_body: bool) {
        self.write_http1_response(out, 1, include_body);
    }

    /// Serialise the response with the version token of the request it answers.
    ///
    /// `minor` is the `HTTP/1.<minor>` of the *request*. RFC 9110 §9.6 permits answering an
    /// HTTP/1.0 request with `HTTP/1.1` (1.1 is the greatest version this server supports),
    /// so nothing here is illegal either way — but the version token is what a client keys
    /// its own framing and persistence assumptions off, and every mainstream implementation
    /// echoes the request. A 1.0 client that reads `HTTP/1.1` may hold a connection the
    /// server already decided to close, so the echo is the interoperable choice.
    pub fn write_http1_to_for_request_version(
        &self,
        out: &mut BytesMut,
        minor: u8,
        include_body: bool,
    ) {
        // Only 1.0 and 1.1 exist on this listener; anything else is clamped to 1.1 rather
        // than emitting a version token no client can parse.
        self.write_http1_response(out, if minor == 0 { 0 } else { 1 }, include_body);
    }

    fn write_http1_response(&self, out: &mut BytesMut, minor: u8, include_body: bool) {
        use std::io::Write;
        {
            let mut writer = bytes::BufMut::writer(&mut *out);
            let _ = write!(
                writer,
                "HTTP/1.{} {} {}\r\n",
                minor, self.status, self.status_text
            );

            let mut has_date = false;
            for (k, v) in &self.headers {
                if k.eq_ignore_ascii_case("content-length") {
                    continue;
                }
                // Framing is decided below, from the body shape: a caller-set
                // `Transfer-Encoding` would either duplicate ours or contradict the framing the
                // payload actually needs, and two framing fields put the client in
                // TE.CL-desync territory.
                if k.eq_ignore_ascii_case("transfer-encoding") {
                    continue;
                }
                if k.eq_ignore_ascii_case("date") {
                    has_date = true;
                }
                let _ = write!(writer, "{}: ", k);
                let _ = write_sanitized_field_value(&mut writer, v);
                let _ = write!(writer, "\r\n");
            }

            if !has_date {
                write_http_date_now(&mut writer);
            }

            // RFC 9110 §15.3.5 (204) and §15.4.5 (304): these responses carry no body, so a
            // Content-Length must not be emitted. Same for 1xx informational responses. A
            // stream of unknown length is chunked on HTTP/1.1 and close-delimited on HTTP/1.0.
            match self.body_framing(minor) {
                BodyFraming::None | BodyFraming::UntilClose => {}
                BodyFraming::ContentLength(len) => {
                    let _ = write!(writer, "Content-Length: {}\r\n", len);
                }
                BodyFraming::Chunked => {
                    let _ = write!(writer, "Transfer-Encoding: chunked\r\n");
                }
            }

            let _ = write!(writer, "\r\n");
        }
        // A status that forbids a body must drop the payload too, not just the
        // Content-Length header: stray bytes after the head would be parsed by the
        // client as the start of the next pipelined response, desynchronising the
        // connection for every request that follows on the same socket.
        //
        // A streamed body is drained by the caller one chunk at a time; serialising it here
        // would buffer exactly what the stream exists to avoid.
        if include_body && self.stream.is_none() && !status_forbids_body(self.status) {
            out.extend_from_slice(&self.body);
        }
    }
}

/// Writes a header field value, dropping the bytes that RFC 9110 §5.5 forbids inside one.
/// The common case (a clean value) is a single `write_str` with no per-byte work.
fn write_sanitized_field_value<W: std::io::Write>(w: &mut W, value: &str) -> std::io::Result<()> {
    match sanitize_field_value(value) {
        std::borrow::Cow::Borrowed(clean) => w.write_all(clean.as_bytes()),
        std::borrow::Cow::Owned(clean) => w.write_all(clean.as_bytes()),
    }
}

/// Strip the bytes RFC 9110 §5.5 forbids inside a field value: CR, LF and NUL.
///
/// Shared with the HTTP/2 frontend, which has no way to write an unsanitized value at all —
/// `h2` rejects the whole field. One implementation means a handler cannot get a response-split
/// byte through on one protocol and a refused stream on the other.
pub fn sanitize_field_value(value: &str) -> std::borrow::Cow<'_, str> {
    if !value
        .bytes()
        .any(|b| b == b'\r' || b == b'\n' || b == b'\0')
    {
        return std::borrow::Cow::Borrowed(value);
    }
    std::borrow::Cow::Owned(
        value
            .chars()
            .filter(|c| !matches!(c, '\r' | '\n' | '\0'))
            .collect(),
    )
}

/// True for status codes that must not carry a message body.
pub fn status_forbids_body(status: u16) -> bool {
    matches!(status, 204 | 304) || (100..200).contains(&status)
}

/// Append one chunk of a `Transfer-Encoding: chunked` body (RFC 9112 §7.1).
///
/// The chunk size is hex, then CRLF, the bytes, CRLF. An empty slice is skipped rather than
/// encoded: a zero-length chunk *terminates* the body, so the writer's end-of-body signal is
/// `None` from the source and never an empty chunk.
pub fn encode_chunk(out: &mut BytesMut, chunk: &[u8]) {
    use std::io::Write;
    if chunk.is_empty() {
        return;
    }
    let mut writer = bytes::BufMut::writer(out);
    let _ = write!(writer, "{:x}\r\n", chunk.len());
    let _ = writer.write_all(chunk);
    let _ = writer.write_all(b"\r\n");
}

/// The zero-length chunk that ends a chunked body (RFC 9112 §7.1).
///
/// A chunked body that stops without this last chunk is truncated, and a client is entitled to
/// treat the response as failed. The writer therefore sends it only when the source ended
/// normally; on an error it closes the connection instead and never fabricates a terminator.
pub fn encode_last_chunk(out: &mut BytesMut) {
    out.extend_from_slice(b"0\r\n\r\n");
}

/// The IANA-registered reason phrase for a status code (RFC 9110 §15).
///
/// Callers that build a response from a bare numeric status must not invent a phrase — a
/// `404 Custom` status line confuses clients, intermediaries and log analysis alike.
pub fn canonical_reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        // RFC 9110 §15 permits an empty reason phrase for unregistered codes; clients key off
        // the numeric status, so a blank phrase is correct where a guessed one would be wrong.
        _ => "",
    }
}

/// Howard Hinnant's civil_from_days: converts days since 1970-01-01 into (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// Formats a UNIX timestamp as an IMF-fixdate string (RFC 9110 §5.6.7), allocation-light.
pub fn format_http_date(unix_secs: u64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = (unix_secs % 86_400) as u32;
    let (h, mi, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday, which is index 4 in a Sunday-first week.
    let weekday = WEEKDAYS[(((days % 7) + 7 + 4) % 7) as usize];

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        weekday,
        d,
        MONTHS[(m - 1) as usize],
        y,
        h,
        mi,
        s
    )
}

/// Current time as an IMF-fixdate string.
pub fn http_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_http_date(secs)
}

/// Writes the `Date` field, reusing one formatted value per UNIX second per thread.
///
/// IMF-fixdate has one-second granularity (RFC 9110 §5.6.7), so every response in the same
/// second is byte-identical in this field. Caching it removes a calendar computation, a
/// `format!`, and a heap allocation from each response. The cache is thread-local: a process-
/// wide `Mutex` put every response behind one lock and bounced the cached string between
/// cores, while a miss now costs one `format!` per second per thread that writes responses.
/// A clock that steps back (NTP) regenerates the string for the new, earlier second.
fn write_http_date_now<W: std::io::Write>(writer: &mut W) {
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<Option<(u64, String)>> = const { RefCell::new(None) };
    }

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    CACHE.with(|cache| {
        let mut slot = cache.borrow_mut();
        if slot.as_ref().is_none_or(|(s, _)| *s != secs) {
            *slot = Some((secs, format_http_date(secs)));
        }
        // Populated immediately above; the borrow cannot be re-entered while held.
        let date = slot
            .as_ref()
            .expect("date cache populated above")
            .1
            .as_str();
        let _ = write!(writer, "Date: {date}\r\n");
    });
}

#[cfg(test)]
fn write_http_date_to(out: &mut BytesMut) {
    write_http_date_now(&mut bytes::BufMut::writer(out));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The status line must carry the version of the request being answered, while the
    /// version-agnostic entry points keep claiming 1.1.
    #[test]
    fn test_status_line_echoes_the_request_version() {
        let res = Response::status(200, "OK");

        let mut v0 = BytesMut::new();
        res.write_http1_to_for_request_version(&mut v0, 0, true);
        assert!(
            v0.starts_with(b"HTTP/1.0 200 OK\r\n"),
            "HTTP/1.0 request answered with {:?}",
            String::from_utf8_lossy(&v0[..24])
        );

        let mut v1 = BytesMut::new();
        res.write_http1_to_for_request_version(&mut v1, 1, true);
        assert!(v1.starts_with(b"HTTP/1.1 200 OK\r\n"));

        // A version this server does not speak is clamped to 1.1 rather than emitting a
        // token no client can parse.
        let mut weird = BytesMut::new();
        res.write_http1_to_for_request_version(&mut weird, 9, true);
        assert!(weird.starts_with(b"HTTP/1.1 200 OK\r\n"));

        let mut plain = BytesMut::new();
        res.write_http1_to(&mut plain);
        assert!(plain.starts_with(b"HTTP/1.1 200 OK\r\n"));

        // Echoing 1.0 changes only the version token: framing stays Content-Length and a
        // HEAD answer still carries no payload.
        let mut head = BytesMut::new();
        res.write_http1_to_for_request_version(&mut head, 0, false);
        let text = String::from_utf8_lossy(&head).into_owned();
        assert!(text.starts_with("HTTP/1.0 200 OK\r\n"), "{text}");
        assert!(text.ends_with("\r\n\r\n"), "{text}");
        assert!(!text.contains("Transfer-Encoding"), "{text}");
    }

    #[test]
    fn test_http_date_matches_known_fixtures() {
        // RFC 9110 §5.6.7 example
        assert_eq!(format_http_date(784111777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(format_http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(format_http_date(86399), "Thu, 01 Jan 1970 23:59:59 GMT");
        assert_eq!(format_http_date(86400), "Fri, 02 Jan 1970 00:00:00 GMT");
        // Leap-day and month/year boundary handling
        assert_eq!(
            format_http_date(1709164800),
            "Thu, 29 Feb 2024 00:00:00 GMT"
        );
        assert_eq!(
            format_http_date(1709251200),
            "Fri, 01 Mar 2024 00:00:00 GMT"
        );
        assert_eq!(
            format_http_date(1740787200),
            "Sat, 01 Mar 2025 00:00:00 GMT"
        );
    }

    #[test]
    fn test_cached_date_write_matches_uncached_format() {
        // The cached writer must emit exactly what format_http_date produces for the
        // current second. Sampling before/after tolerates the tick crossing the second
        // boundary mid-write; any other value is a cache/format divergence.
        let now_secs = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let before = now_secs();
        let mut out = BytesMut::new();
        write_http_date_to(&mut out);
        let after = now_secs();

        let line = std::str::from_utf8(&out).unwrap();
        let value = line
            .strip_prefix("Date: ")
            .and_then(|v| v.strip_suffix("\r\n"))
            .expect("output must be a single Date field line");
        let allowed = [
            format_http_date(before),
            format_http_date(after),
            format_http_date(before + 1),
        ];
        assert!(
            allowed.iter().any(|a| a == value),
            "cached date {value:?} not a valid current fixdate (window {before}..{after})"
        );

        // Repeated writes stay byte-identical inside one second (the cache working),
        // and always parse back as a valid fixdate.
        let mut again = BytesMut::new();
        write_http_date_to(&mut again);
        assert_eq!(out, again, "Date width must be stable");
    }

    #[test]
    fn test_cached_date_single_emission_per_response() {
        let mut out = BytesMut::new();
        Response::text("hi").write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();
        let head = text.split("\r\n\r\n").next().unwrap();
        let count = head
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("date:"))
            .count();
        assert_eq!(count, 1);
        assert!(
            head.lines().last().unwrap().starts_with("Content-Length:"),
            "Date must slot between the fields and Content-Length, {head:?}"
        );
    }

    #[test]
    #[ignore = "perf microbench; run with cargo test --release -p aether-http -- --ignored --nocapture"]
    fn bench_date_write_paths() {
        use std::io::Write;
        use std::time::Instant;

        let n = 200_000usize;
        let mut buf = BytesMut::with_capacity(128);

        let uncached = |buf: &mut BytesMut| {
            buf.clear();
            let mut w = bytes::BufMut::writer(&mut *buf);
            let _ = write!(w, "Date: {}\r\n", http_date_now());
        };
        let cached = |buf: &mut BytesMut| {
            buf.clear();
            let mut w = bytes::BufMut::writer(&mut *buf);
            write_http_date_now(&mut w);
        };

        // Warm caches (time-of-day page, allocator) before the first measurement.
        for _ in 0..1000 {
            uncached(&mut buf);
            cached(&mut buf);
        }

        for round in 0..3 {
            let t0 = Instant::now();
            for _ in 0..n {
                uncached(&mut buf);
            }
            let t_unc = t0.elapsed();
            let t1 = Instant::now();
            for _ in 0..n {
                cached(&mut buf);
            }
            let t_cch = t1.elapsed();
            println!(
                "round{round}: uncached {:.3}µs/op  cached {:.3}µs/op  ratio x{:.2}",
                t_unc.as_secs_f64() * 1e6 / n as f64,
                t_cch.as_secs_f64() * 1e6 / n as f64,
                t_unc.as_secs_f64() / t_cch.as_secs_f64()
            );
        }
        assert!(buf.starts_with(b"Date: "));
    }

    #[test]
    fn test_write_http1_emits_exactly_one_content_length() {
        // A constructor or middleware that pushed its own stale Content-Length must not
        // be able to produce a second one: that is a response-splitting vector.
        let mut res = Response::json(Bytes::from_static(b"{\"ok\":true}"));
        res.headers.push(("Content-Length", "9999".to_string()));
        res.headers.push(("Content-Length", "0".to_string()));

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .count();
        assert_eq!(
            count, 1,
            "must emit exactly one Content-Length, got: {}",
            text
        );
        assert!(text.contains(&format!("Content-Length: {}", res.body.len())));
        assert!(!text.contains("9999"));
    }

    #[test]
    fn test_write_http1_includes_date_once() {
        let res = Response::text("hi");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("date:"))
            .count();
        assert_eq!(count, 1);
        assert!(
            text.contains("GMT\r\n"),
            "date must be IMF-fixdate: {}",
            text
        );
    }

    #[test]
    fn test_user_supplied_date_is_preserved_not_duplicated() {
        let res = Response::text("hi").header("Date", "Thu, 01 Jan 1970 00:00:00 GMT");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("date:"))
            .count();
        assert_eq!(count, 1);
        assert!(text.contains("Date: Thu, 01 Jan 1970 00:00:00 GMT"));
    }

    #[test]
    fn test_204_and_304_carry_no_content_length() {
        for status in [204u16, 304] {
            let res = Response::status(status, "x");
            let mut out = BytesMut::new();
            res.write_http1_to(&mut out);
            let text = std::str::from_utf8(&out).unwrap();
            assert!(
                !text.to_ascii_lowercase().contains("content-length:"),
                "{} must not carry Content-Length, got: {}",
                status,
                text
            );
        }
        assert!(status_forbids_body(204));
        assert!(status_forbids_body(304));
        assert!(status_forbids_body(100));
        assert!(!status_forbids_body(200));
        assert!(!status_forbids_body(404));
    }

    /// Regression: suppressing only the `Content-Length` while still writing `self.body`
    /// leaked stray bytes onto the wire for 204/304/1xx. With no framing header, the next
    /// request's response parser reads them as the start of a new message, desynchronising
    /// the whole keep-alive connection.
    #[test]
    fn test_bodyless_status_drops_stray_payload() {
        for status in [204u16, 304, 100, 199] {
            let mut res = Response::status(status, "x");
            res.body = Bytes::from_static(b"LEAKED");
            let mut out = BytesMut::new();
            res.write_http1_to(&mut out);
            let text = std::str::from_utf8(&out).unwrap();
            assert!(
                !text.contains("LEAKED"),
                "{status} must not emit any body bytes, got: {text}"
            );

            // Same via the HEAD-suppression path: headers kept, payload dropped.
            let mut out2 = BytesMut::new();
            res.write_http1_to_with_body(&mut out2, false);
            assert!(!std::str::from_utf8(&out2).unwrap().contains("LEAKED"));
        }

        // Control: a body-permitting status must still emit its payload.
        let mut res = Response::status(200, "OK");
        res.body = Bytes::from_static(b"KEEP");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        assert!(std::str::from_utf8(&out).unwrap().contains("KEEP"));
    }

    #[test]
    fn test_json_status_has_single_correct_content_length() {
        // Regression: json_status used to inherit "Content-Length: 0" from status()
        // and then push a second, real one.
        let res = Response::json_status(404, "Not Found", Bytes::from_static(b"{\"e\":1}"));
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .count();
        assert_eq!(count, 1, "got: {}", text);
        assert!(text.contains("Content-Length: 7"), "got: {}", text);
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    /// A caller- or client-derived header value must not be able to inject extra headers.
    #[test]
    fn test_header_value_crlf_injection_is_neutralised() {
        let malicious = "/ok\r\nSet-Cookie: session=attacker\r\nX-Injected: yes";
        let res = Response::redirect(malicious, 302);

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let header_section = text.split("\r\n\r\n").next().unwrap();
        // Stripping CR/LF leaves the attacker's text inline inside the Location value, so the
        // assertion has to be about line structure, not substring absence.
        let lines: Vec<&str> = header_section.split("\r\n").collect();
        assert_eq!(
            lines.len(),
            4,
            "expected status + Location + Date + Content-Length only, got: {:#?}",
            lines
        );
        assert!(lines[0].starts_with("HTTP/1.1 302 Found"));
        assert!(lines[1].starts_with("Location: /ok"), "got: {}", lines[1]);
        assert!(
            !lines.iter().skip(1).any(|l| l.starts_with("Set-Cookie:")),
            "injected Set-Cookie became a real header: {:#?}",
            lines
        );
        assert!(
            !lines.iter().skip(1).any(|l| l.starts_with("X-Injected:")),
            "injected X-Injected became a real header: {:#?}",
            lines
        );
        assert!(!header_section.contains("\n\n"));
    }

    #[test]
    fn test_header_value_nul_and_lone_lf_are_stripped() {
        let mut res = Response::ok();
        res.add_header("X-A", "before\0after");
        res.add_header("X-B", "line1\nline2");
        res.add_header("X-C", "line1\rline2");

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();
        let header_section = text.split("\r\n\r\n").next().unwrap();

        assert!(!header_section.contains('\0'));
        // status + X-A + X-B + X-C + Date + Content-Length, and nothing more.
        let lines: Vec<&str> = header_section.split("\r\n").collect();
        assert_eq!(lines.len(), 6, "got: {:#?}", lines);
        assert!(lines.contains(&"X-A: beforeafter"), "{:#?}", lines);
        assert!(lines.contains(&"X-B: line1line2"), "{:#?}", lines);
        assert!(lines.contains(&"X-C: line1line2"), "{:#?}", lines);
    }

    /// Clean values must pass through untouched, including UTF-8.
    #[test]
    fn test_clean_header_values_are_untouched() {
        let mut res = Response::ok();
        res.add_header("X-Arabic", "مرحبا بالعالم");
        res.add_header("X-Token", "abc.def-ghi_jkl~mno+pqr/stu=vwx");

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        assert!(text.contains("X-Arabic: مرحبا بالعالم"));
        assert!(text.contains("X-Token: abc.def-ghi_jkl~mno+pqr/stu=vwx"));
    }

    #[test]
    fn test_canonical_reason_phrases() {
        assert_eq!(canonical_reason(200), "OK");
        assert_eq!(canonical_reason(204), "No Content");
        assert_eq!(canonical_reason(301), "Moved Permanently");
        assert_eq!(canonical_reason(404), "Not Found");
        assert_eq!(canonical_reason(405), "Method Not Allowed");
        assert_eq!(canonical_reason(413), "Content Too Large");
        assert_eq!(canonical_reason(431), "Request Header Fields Too Large");
        assert_eq!(canonical_reason(500), "Internal Server Error");
        assert_eq!(canonical_reason(504), "Gateway Timeout");
        // Unregistered codes get an empty phrase rather than a fabricated one.
        assert_eq!(canonical_reason(599), "");
        assert_eq!(canonical_reason(0), "");
    }

    #[test]
    fn test_redirect_uses_canonical_reason() {
        let res = Response::redirect("/new", 308);
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();
        assert!(
            text.starts_with("HTTP/1.1 308 Permanent Redirect\r\n"),
            "got: {}",
            text
        );

        // Previously any non-redirect status fell through to "See Other".
        let res = Response::redirect("/x", 200);
        assert_eq!(res.status_text, "OK");
    }

    /// T1-b: the writer is the sole framing authority (Content-Length derived from
    /// the body). A caller-set `Transfer-Encoding` would co-exist with that
    /// Content-Length on the wire — a response-desync vector — so it must not be
    /// emitted.
    #[test]
    fn test_caller_transfer_encoding_never_emitted() {
        let res = Response::text_dynamic("hi").header("Transfer-Encoding", "chunked");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = String::from_utf8_lossy(&out).to_lowercase();
        assert!(
            !text.contains("transfer-encoding"),
            "TE leaked into a Content-Length-framed response: {text}"
        );
        assert!(text.contains("content-length: 2"), "got: {text}");
    }

    /// T1-c: RFC 6265 forbids `;`, `,`, whitespace, `"` and `\` inside the
    /// cookie-value and requires a token name/attribute — unfiltered callers
    /// could inject extra Set-Cookie attributes (e.g. turn a session cookie into
    /// `sid=x; Admin=1`). They must be percent-encoded; legitimate values stay
    /// byte-identical.
    #[test]
    fn test_set_cookie_encodes_injection_characters() {
        let res = Response::ok().set_cookie("sid", "a=b; Admin=1", true, true, "Lax", None);
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .expect("Set-Cookie present");
        assert_eq!(
            value.as_str(),
            "sid=a=b%3B%20Admin=1; SameSite=Lax; Path=/; HttpOnly; Secure"
        );

        // benign value unchanged
        let res = Response::ok().set_cookie("sid", "abc-123~x", false, false, "Strict", Some(60));
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .unwrap();
        assert_eq!(
            value.as_str(),
            "sid=abc-123~x; SameSite=Strict; Path=/; Max-Age=60"
        );

        // attribute-position injection through same_site is neutralised
        let res = Response::ok().set_cookie("sid", "v", false, false, "Lax; Admin=1", None);
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .unwrap();
        assert!(
            !value.contains("Admin"),
            "same_site injected an attribute: {value}"
        );

        // name-position injection is neutralised
        let res = Response::ok().set_cookie("sid=evil; x", "v", false, false, "Lax", None);
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .unwrap();
        assert!(
            !value.contains("; x=") && !value.starts_with("sid=evil;"),
            "name injected attributes: {value}"
        );
    }

    /// Both frontends write field values through one sanitizer, so a handler cannot get a
    /// response-splitting byte through on HTTP/1 and a refused stream on HTTP/2.
    #[test]
    fn test_sanitize_field_value_strips_the_forbidden_bytes() {
        assert_eq!(sanitize_field_value("plain"), "plain");
        assert_eq!(
            sanitize_field_value("a\r\nb\0c"),
            "abc",
            "CR, LF and NUL must not survive into a field value"
        );
        // The filter runs over `char`s, so a multi-byte value stays valid UTF-8 after stripping.
        assert_eq!(sanitize_field_value("mantiq\rsalāmًا 🌍"), "mantiqsalāmًا 🌍");
        assert!(
            matches!(sanitize_field_value("clean"), std::borrow::Cow::Borrowed(_)),
            "a value with nothing to strip must not allocate"
        );
    }

    /// One chunk, one frame: the length is hex, the payload is verbatim, and an empty chunk is
    /// no frame at all because a zero-length one *is* the terminator (RFC 9112 §7.1).
    #[test]
    fn test_chunk_encoding_is_the_wire_format() {
        let mut out = BytesMut::new();
        encode_chunk(&mut out, b"hello");
        encode_chunk(&mut out, b"");
        encode_chunk(&mut out, &[0xff; 16]);
        encode_last_chunk(&mut out);
        assert_eq!(
            out,
            Bytes::from_static(
                b"5\r\nhello\r\n10\r\n\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\r\n0\r\n\r\n"
            )
        );
    }

    /// A declared length is the framing, and the head promises exactly that many bytes.
    #[test]
    fn test_a_stream_is_framed_by_its_declared_length() {
        let response = Response::stream_with_length(
            BodyStream::from_chunks([Bytes::from_static(b"abc")]),
            "text/plain",
            6,
        );
        assert_eq!(response.body_framing(1), BodyFraming::ContentLength(6));
        let mut wire = BytesMut::new();
        response.write_http1_to_for_request_version(&mut wire, 1, true);
        let head = String::from_utf8_lossy(&wire);
        assert!(head.contains("Content-Length: 6\r\n"), "{head}");
        assert!(!head.contains("Transfer-Encoding"), "{head}");
        // The head writer never drains the source: that is the whole point of a stream.
        assert!(head.ends_with("\r\n\r\n"), "{head}");
    }

    /// An undeclared stream is chunked on 1.1 and close-delimited on 1.0 — the only two legal
    /// framings for a body whose end is not known when the head is written.
    #[test]
    fn test_an_undeclared_stream_is_chunked_on_one_one_and_delimited_by_close_on_one_zero() {
        let response = Response::stream(
            BodyStream::from_chunks([Bytes::from_static(b"abc")]),
            "text/plain",
        );
        assert_eq!(response.body_framing(1), BodyFraming::Chunked);
        assert_eq!(response.body_framing(0), BodyFraming::UntilClose);

        let mut one_one = BytesMut::new();
        response.write_http1_to_for_request_version(&mut one_one, 1, true);
        let head = String::from_utf8_lossy(&one_one);
        assert!(head.contains("Transfer-Encoding: chunked\r\n"), "{head}");
        assert!(!head.contains("Content-Length"), "{head}");

        let mut one_zero = BytesMut::new();
        response.write_http1_to_for_request_version(&mut one_zero, 0, true);
        let head = String::from_utf8_lossy(&one_zero);
        assert!(!head.contains("Transfer-Encoding"), "{head}");
        assert!(!head.contains("Content-Length"), "{head}");
    }

    /// A status that forbids a body carries neither framing nor payload, whatever stream was
    /// attached to it — the same rule the emitters apply to a buffered body.
    #[test]
    fn test_a_body_forbidding_status_ignores_a_stream() {
        let mut response = Response::status(204, "No Content");
        response.stream = Some(BodyStream::from_chunks([Bytes::from_static(b"nope")]));
        assert_eq!(response.body_framing(1), BodyFraming::None);
        let mut wire = BytesMut::new();
        response.write_http1_to_for_request_version(&mut wire, 1, true);
        let head = String::from_utf8_lossy(&wire);
        assert!(!head.contains("Transfer-Encoding"), "{head}");
        assert!(!head.contains("Content-Length"), "{head}");
        assert!(head.ends_with("\r\n\r\n"), "{head}");
    }

    /// A `HEAD` earns the GET's framing and no payload, so the stream is never drained.
    #[test]
    fn test_a_head_writes_the_streams_framing_and_no_payload() {
        let response = Response::stream_with_length(
            BodyStream::from_chunks([Bytes::from_static(b"abc")]),
            "text/plain",
            3,
        );
        let mut wire = BytesMut::new();
        response.write_http1_to_for_request_version(&mut wire, 1, false);
        let head = String::from_utf8_lossy(&wire);
        assert!(head.contains("Content-Length: 3\r\n"), "{head}");
        assert!(head.ends_with("\r\n\r\n"), "{head}");
        assert!(
            response.is_streaming(),
            "serialising a head must not consume it"
        );
    }
}
