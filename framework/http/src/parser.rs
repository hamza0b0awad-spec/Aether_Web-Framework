//! # Ultra-fast Zero-Copy HTTP Request Parser
//!
//! Borrows all slices (&'req str, &'req [u8]) directly from the socket buffer.
//! Zero string copies, zero heap allocations for standard requests.
//! Hardened with RFC 9112 HTTP/1.1 Request Smuggling (CL.TE, TE.CL, Dual-CL) protection.

use crate::header::{HeaderMap, HeaderView};
use crate::limits::ServerLimits;
use crate::method::Method;
use aether_core::error::AetherError;

/// تمثيل مباشر وسريع لطلب HTTP يستعير شرائح الذاكرة (`&'req str`, `&'req [u8]`) مباشرة من بافر المقبس بدون أي نسخ (Zero-Copy).
#[derive(Debug)]
pub struct RequestView<'req> {
    pub method: Method,
    pub raw_uri: &'req str,
    pub path: &'req str,
    pub query_str: Option<&'req str>,
    pub version: u8,
    /// Minor version from the request line (`HTTP/1.0` -> 0, `HTTP/1.1` -> 1). Defaults
    /// to 1 for any other shape so legacy keep-alive semantics stay unchanged; the
    /// connection loop uses this because HTTP/1.0 closes by default while 1.1 persists.
    pub version_minor: u8,
    pub headers: HeaderMap<'req>,
    pub body: &'req [u8],
}

/// Number of header slots kept on the stack. A head estimated to fit within this budget
/// never touches the heap; a larger one gets a `Vec` of exactly the size being tried.
const STACK_HEADER_SLOTS: usize = 64;

/// RFC 9112 OWS is SP/HTAB only. `str::trim` also strips Unicode whitespace such as
/// U+00A0, and a framing value padded that way would be re-read differently by a
/// strict frontend — a CL.TE/TE.CL smuggling seam. Framing decisions must use this.
fn ows_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == '\t')
}

/// [`ows_trim`]'s rule for a byte slice. Header values stay borrowed bytes until a consumer
/// decides what they mean, and a field read with `str::trim` would accept padding this parser
/// refuses everywhere else.
fn ows_trim_bytes(s: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = s.len();
    while start < end && matches!(s[start], b' ' | b'\t') {
        start += 1;
    }
    while end > start && matches!(s[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    &s[start..end]
}

impl<'req> RequestView<'req> {
    /// Parses a request whose body is framed by `Content-Length` only.
    ///
    /// `Transfer-Encoding` is deliberately rejected here; use [`parse_request`] for the
    /// runtime path that also supports chunked decoding.
    pub fn parse(
        buf: &'req [u8],
        limits: &ServerLimits,
    ) -> Result<Option<(Self, usize)>, AetherError> {
        let Some((head, header_len)) = Self::parse_head(buf, limits, false, limits.max_body_bytes)?
        else {
            return Ok(None);
        };
        let cl = head.content_length.unwrap_or(0);
        let total_needed = header_len
            .checked_add(cl)
            .ok_or(AetherError::BodyLimitExceeded)?;
        if buf.len() < total_needed {
            return Ok(None);
        }
        Ok(Some((
            Self::from_head(head, &buf[header_len..total_needed]),
            total_needed,
        )))
    }

    fn from_head(head: RequestHead<'req>, body: &'req [u8]) -> Self {
        Self {
            method: head.method,
            raw_uri: head.raw_uri,
            path: head.path,
            query_str: head.query_str,
            version: head.version,
            version_minor: head.version_minor,
            headers: head.headers,
            body,
        }
    }

    /// Parses and validates the request head, applying the framing rules that protect
    /// against request smuggling. `allow_transfer_encoding` is false for the legacy
    /// `Content-Length`-only entry point. `declared_body_ceiling` is the ceiling a declared
    /// `Content-Length` is refused over: the buffered entry points pass `max_body_bytes`, and the
    /// streaming one passes none, because which ceiling applies is the route's decision and the
    /// route is resolved after the head.
    fn parse_head(
        buf: &'req [u8],
        limits: &ServerLimits,
        allow_transfer_encoding: bool,
        declared_body_ceiling: usize,
    ) -> Result<Option<(RequestHead<'req>, usize)>, AetherError> {
        let header_budget = limits.max_headers_count.max(1);

        // The slot array is sized to the request rather than to the configured ceiling: a
        // 100-line ceiling would otherwise cost every ordinary request a zeroed 3.2 KiB
        // array. Sizing it smaller must not make `max_headers_count` weaker, so an attempt
        // whose array ran out is reported separately from an attempt that proved the request
        // is over the limit — only the ceiling-sized attempt is allowed to answer 431.
        let estimate = header_slot_estimate(buf, limits.max_header_bytes, header_budget);

        let mut stack_headers = [httparse::EMPTY_HEADER; STACK_HEADER_SLOTS];
        let mut heap_headers: Vec<httparse::Header<'req>> = Vec::new();

        // The ceiling is a second attempt, not a pre-paid cost: it is only tried by a head
        // that outgrew the estimate, so an oversized request pays for both parses.
        let attempts: &[usize] = if estimate < header_budget {
            &[estimate, header_budget]
        } else {
            &[estimate]
        };

        for &size in attempts {
            let slots: &mut [httparse::Header<'req>] = if size <= STACK_HEADER_SLOTS {
                &mut stack_headers[..size]
            } else {
                heap_headers.resize(size, httparse::EMPTY_HEADER);
                &mut heap_headers
            };
            match parse_head_into(
                slots,
                buf,
                limits,
                allow_transfer_encoding,
                declared_body_ceiling,
            )? {
                Attempt::Head(head) => return Ok(head),
                Attempt::SlotsRanOut => {}
            }
        }

        // Reached only when the ceiling-sized array could not hold the head either.
        Err(AetherError::HeaderTooLarge)
    }
}

/// Outcome of one [`parse_head_into`] attempt against a caller-supplied slot array.
// Only the empty variant makes this a "large" enum; the head is the value `parse_head`
// already returned by value before the split, and boxing it would add a heap allocation to
// every request — the exact cost this split exists to remove.
#[allow(clippy::large_enum_variant)]
enum Attempt<'req> {
    /// A complete head, or `None` while the head is still arriving.
    Head(Option<(RequestHead<'req>, usize)>),
    /// The array ran out before the head did. This proves nothing about
    /// `max_headers_count`, so the caller must retry with a larger array rather than answer
    /// 431 — otherwise a request-sized array would turn a legitimate 70-header gRPC call
    /// into a rejection.
    SlotsRanOut,
}

/// The number of field lines already visible in the head, capped at the configured ceiling.
///
/// Counting line terminators is cheaper than zeroing a ceiling-sized array, and the result
/// is only a starting size: an undercount costs one extra parse attempt, never a wrong
/// verdict. The scan stops at the blank line that ends the head, so a buffer holding several
/// pipelined requests is sized by the first one, and it never reads past the byte budget a
/// slowloris head is already refused for.
fn header_slot_estimate(buf: &[u8], max_header_bytes: usize, ceiling: usize) -> usize {
    let scan_to = buf.len().min(max_header_bytes.saturating_add(1));
    let mut lines = 0usize;
    let mut at_line_start_after_lf = false;
    for &b in &buf[..scan_to] {
        if b == b'\n' {
            lines += 1;
            if at_line_start_after_lf {
                break;
            }
            at_line_start_after_lf = true;
        } else if b != b'\r' {
            at_line_start_after_lf = false;
        }
    }
    lines.min(ceiling).max(1)
}

/// The routing facts of one request-target.
struct Target<'uri> {
    /// What the router matches and what handlers see as the request path.
    path: &'uri str,
    query: Option<&'uri str>,
    /// The authority of an absolute-form target. RFC 9112 §3.2.2 requires it to replace the
    /// received `Host` field, so the caller does that once the header list is complete.
    authority: Option<&'uri str>,
}

/// Classify a request-target into one of the four forms RFC 9112 §3.2 defines.
///
/// Anything outside those forms is refused rather than routed as if it were a path: before this
/// ran, `GET hello HTTP/1.1` matched the route `/hello`, and a request-target the client never
/// meant as an origin-form path became a routable one.
fn classify_target<'uri>(raw_uri: &'uri str, method: Method) -> Result<Target<'uri>, AetherError> {
    // §3.2: no whitespace is allowed in a request-target (httparse already splits the request
    // line on it), and a `#` can only ever introduce a fragment, which is never sent to a
    // server. A target that contains one is malformed, not a path with a literal `#`.
    if raw_uri.contains('#') {
        return Err(AetherError::MalformedRequest);
    }

    if method == Method::Connect {
        // §3.2.3: authority-form is the only form a CONNECT request may use.
        authority_form(raw_uri)?;
        return Ok(Target {
            path: raw_uri,
            query: None,
            authority: None,
        });
    }

    if raw_uri == "*" {
        // §3.2.4: the asterisk is only a server-wide OPTIONS target. It stays in `path` as the
        // one value no origin-form path can take, so the serving core can recognise it.
        return if method == Method::Options {
            Ok(Target {
                path: raw_uri,
                query: None,
                authority: None,
            })
        } else {
            Err(AetherError::MalformedRequest)
        };
    }

    if raw_uri.starts_with('/') {
        // §3.2.1: the absolute path, with an optional query.
        let (path, query) = split_target_query(raw_uri);
        return Ok(Target {
            path,
            query,
            authority: None,
        });
    }

    if let Some((scheme, rest)) = raw_uri.split_once("://") {
        // §3.2.2: a server MUST accept the absolute-form. This origin server represents http
        // and https resources; routing any other scheme as a path would turn a hostile target
        // into a route match.
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(AetherError::MalformedRequest);
        }
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..end];
        // An http(s) URI always has an authority, and it is `host [ ":" port ]`: userinfo has
        // no meaning to an origin server (RFC 9110 §4.2.4 treats it as an error), and a value
        // whose host and port cannot be told apart is exactly the ambiguity an allow-list
        // cannot survive. Refuse rather than make a split up.
        if !absolute_authority_is_well_formed(authority) {
            return Err(AetherError::MalformedRequest);
        }
        let tail = &rest[end..];
        let (path, query) = if tail.is_empty() || tail.starts_with('?') {
            // An empty path is `/`, and a query with no path attaches to it.
            ("/", tail.strip_prefix('?'))
        } else {
            split_target_query(tail)
        };
        return Ok(Target {
            path,
            query,
            authority: Some(authority),
        });
    }

    // A bare token, an empty path (`?query` with no `/`), or any other shape is not one of the
    // four forms. §3.2 says to answer 400 rather than try to autocorrect it.
    Err(AetherError::MalformedRequest)
}

/// `path?query` without allocating, leaving an empty query (`/users?`) as `Some("")`.
fn split_target_query(target: &str) -> (&str, Option<&str>) {
    match target.find('?') {
        Some(idx) => (&target[..idx], Some(&target[idx + 1..])),
        None => (target, None),
    }
}

/// `uri-host ":" port`, the only target a CONNECT request may carry (RFC 9112 §3.2.3).
fn authority_form(target: &str) -> Result<(), AetherError> {
    if target.is_empty() || target.contains(['/', '?', '@']) {
        return Err(AetherError::MalformedRequest);
    }
    // rsplit, not split: a bracketed IPv6 host is full of colons and the port is last.
    let Some((host, port)) = target.rsplit_once(':') else {
        return Err(AetherError::MalformedRequest);
    };
    if host.is_empty() || port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AetherError::MalformedRequest);
    }
    Ok(())
}

/// `host [ ":" port ]`, with a bracketed IPv6 host, for an http(s) absolute-form target.
///
/// The host part is left otherwise unchecked: RFC 9112 does not require an origin server to
/// validate the authority's DNS form, and the host allow-list - which does normalise it - only
/// runs when an operator configured one. What is refused here is the shape that cannot be split
/// unambiguously, because a mismatch between this server and an upstream proxy about which side
/// of a stray `@` or colon is the host is a routing bypass in the making.
fn absolute_authority_is_well_formed(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let tail = &rest[end + 1..];
        return match tail.strip_prefix(':') {
            None => tail.is_empty(),
            Some(port) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
        };
    }
    if authority.matches(':').count() > 1 {
        // An unbracketed IPv6 literal: valid as a URI host, but not as an authority this
        // parser can split into host and port.
        return false;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
        }
        None => true,
    }
}

/// One parse attempt, including every framing rule, against the supplied header slots.
fn parse_head_into<'req>(
    slots: &mut [httparse::Header<'req>],
    buf: &'req [u8],
    limits: &ServerLimits,
    allow_transfer_encoding: bool,
    declared_body_ceiling: usize,
) -> Result<Attempt<'req>, AetherError> {
    let mut req = httparse::Request::new(slots);

    let header_len = match req.parse(buf) {
        Ok(status) => match status {
            httparse::Status::Complete(header_len) => header_len,
            httparse::Status::Partial => {
                if buf.len() > limits.max_header_bytes {
                    return Err(AetherError::HeaderTooLarge);
                }
                return Ok(Attempt::Head(None));
            }
        },
        // httparse reports an exhausted slot array distinctly from a malformed head. At the
        // configured ceiling that is the limit error the server answers as 431; at a smaller
        // array it only says this attempt could not hold the head.
        Err(httparse::Error::TooManyHeaders) => return Ok(Attempt::SlotsRanOut),
        Err(_) => return Err(AetherError::MalformedRequest),
    };

    if header_len > limits.max_header_bytes {
        return Err(AetherError::HeaderTooLarge);
    }

    let method_str = req.method.ok_or(AetherError::MalformedRequest)?;
    let method = Method::from_bytes(method_str.as_bytes()).ok_or(AetherError::MalformedRequest)?;

    let raw_uri = req.path.ok_or(AetherError::MalformedRequest)?;
    let version = req.version.unwrap_or(1);
    let version_minor = request_line_version_minor(&buf[..header_len]);

    // RFC 9112 §3.2 defines four request-target forms; only a target in one of them may be
    // routed, and absolute-form has to replace the Host field it arrives with (below).
    let target = classify_target(raw_uri, method)?;
    // فصل المسار عن معلمات الاستعلام بدون تخصيص: /users?page=1 -> path="/users", query="page=1"
    let (path, query_str) = (target.path, target.query);

    let mut headers = HeaderMap::new();
    let mut content_length: Option<usize> = None;
    let mut te_count: usize = 0;
    let mut te_chunked: usize = 0;
    let mut te_other = false;
    let mut host_count: usize = 0;

    for h in req.headers.iter() {
        headers.push(HeaderView {
            name: h.name,
            value: h.value,
        });

        if h.name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                // الحماية من هجمات Request Smuggling: تم رصد تكرار Content-Length
                return Err(AetherError::MalformedRequest);
            }

            let val_str =
                std::str::from_utf8(h.value).map_err(|_| AetherError::MalformedRequest)?;
            let trimmed = ows_trim(val_str);
            if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
                return Err(AetherError::MalformedRequest);
            }
            let parsed_len = trimmed
                .parse::<usize>()
                .map_err(|_| AetherError::MalformedRequest)?;
            content_length = Some(parsed_len);
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            te_count += 1;
            // Only an exact `chunked` coding is supported. Anything else
            // (`gzip`, `identity`, lists) cannot be framed and must be refused.
            let value = std::str::from_utf8(h.value).map_err(|_| AetherError::MalformedRequest)?;
            if ows_trim(value).eq_ignore_ascii_case("chunked") {
                te_chunked += 1;
            } else {
                te_other = true;
            }
        } else if h.name.eq_ignore_ascii_case("host") {
            host_count += 1;
        }
    }

    // RFC 9112 §3.2 makes more than one Host field a 400 for every request, not only for
    // HTTP/1.1. This runs before the absolute-form replacement, which would otherwise collapse
    // the duplicates `validate_head` looks for into the one value the target supplied.
    if host_count > 1 {
        return Err(AetherError::MalformedRequest);
    }

    // RFC 9112 بند 6.3: حظر الجمع بين Transfer-Encoding و Content-Length (منع ثغرات CL.TE و TE.CL)
    // The array is ceiling-sized on the attempt that gets here, so this can only decide the
    // case `max_headers_count` below 1 (the budget is floored at 1 slot to keep httparse's
    // own precondition) — the count limit stays enforced either way. That one case is what
    // `test_a_zero_header_budget_refuses_the_one_line_head_that_otherwise_parses` pins: deleting
    // this check leaves the 431 verdict to [`RequestView::parse_head`]'s ceiling attempt above,
    // which a zero budget still reaches, but only for a head with more than one field line.
    if headers.len() > limits.max_headers_count {
        return Err(AetherError::HeaderTooLarge);
    }
    if te_count > 0 && content_length.is_some() {
        return Err(AetherError::MalformedRequest);
    }
    if te_count > 0 && !allow_transfer_encoding {
        return Err(AetherError::MalformedRequest);
    }
    // RFC 9110 §10.1.1: a server that receives an expectation-extension it cannot meet answers
    // `417 (Expectation Failed)` rather than processing the request. This sits below the grammar
    // and header-count refusals above and above the body-length refusal below, and that ordering
    // is the whole claim: a head that cannot be framed is malformed whatever it promised, and a
    // request already refused on its expectation must not be answered with a body-size verdict it
    // never reached. Placed here rather than in [`validate_framed_head`] because every reader of a
    // head - the buffered loop, the streaming loop and the `Content-Length`-only legacy entry
    // point - passes through this function, so none can serve an unmet promise.
    if expectation_of(&headers) == Expectation::Unsupported {
        return Err(AetherError::UnsupportedExpectation);
    }
    // A declared length is refused against the ceiling the *caller* names: the buffered path's
    // `max_body_bytes`, or none for the streaming entry point, whose route applies
    // `max_stream_body_bytes` once it is known. Refusing here against the buffered ceiling would
    // make a streaming route reject the uploads it exists to carry.
    if let Some(cl) = content_length
        && cl > declared_body_ceiling
    {
        return Err(AetherError::BodyLimitExceeded);
    }

    if let Some(authority) = target.authority {
        // RFC 9112 §3.2.2: an origin server "MUST ignore the received Host header field (if
        // any) and instead use the host information of the request-target". Replacing the
        // field, rather than routing around it, is what makes every consumer below - the host
        // allow-list, CORS, a handler reading `Host` - see the same authority.
        //
        // §3.2 still requires a Host field on every HTTP/1.1 request, so its absence is a 400
        // even when the target carries the authority; the replacement would hide that.
        if version == 1 && version_minor >= 1 && host_count == 0 {
            return Err(AetherError::MalformedRequest);
        }
        headers.retain(|h| !h.name.eq_ignore_ascii_case("host"));
        headers.push(HeaderView {
            name: crate::header::standard_headers::HOST,
            value: authority.as_bytes(),
        });
    }

    Ok(Attempt::Head(Some((
        RequestHead {
            method,
            raw_uri,
            path,
            query_str,
            version,
            version_minor,
            headers,
            content_length,
            te_count,
            te_chunked,
            te_other,
        },
        header_len,
    ))))
}

/// Parsed head plus the framing metadata needed to decide how to read the body.
struct RequestHead<'req> {
    method: Method,
    raw_uri: &'req str,
    path: &'req str,
    query_str: Option<&'req str>,
    version: u8,
    version_minor: u8,
    headers: HeaderMap<'req>,
    content_length: Option<usize>,
    te_count: usize,
    te_chunked: usize,
    te_other: bool,
}

/// What the `Expect` field of one request asks the server to do before the body is read.
///
/// This is the single reading of the field on both frontends: the HTTP/1 parser feeds it the
/// borrowed header lines of a head, the HTTP/2 bridge feeds it the `http::HeaderMap` of a
/// HEADERS frame. Neither can therefore answer an expectation differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expectation {
    /// No `Expect` field at all, or only empty list elements. Nothing is owed.
    Absent,
    /// Every non-empty element names `100-continue`, which this server supports.
    Continue,
    /// At least one element names an expectation this server cannot meet.
    Unsupported,
}

/// Classify the `Expect` field values of one request.
///
/// The value is a comma-separated list of expectation tokens, so a value that merely *contains*
/// the token (`100-continue-x`, `x100-continue`) or quotes it (`"100-continue"`) is a different
/// expectation. Empty elements are skipped the way RFC 9110 §5.5's `#` rule skips them, so
/// `Expect: , 100-continue` still reads as one expectation, while a list that mixes a supported
/// and an unsupported element reads as unsupported: the field promises that *every* element is
/// met before the body is sent, so answering the interim for half of it would be a lie.
pub fn classify_expectation<'a, I>(values: I) -> Expectation
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut saw_continue = false;
    let mut saw_unsupported = false;
    for value in values {
        for element in value.split(|byte| *byte == b',') {
            let element = ows_trim_bytes(element);
            if element.is_empty() {
                continue;
            }
            if element.eq_ignore_ascii_case(b"100-continue") {
                saw_continue = true;
            } else {
                saw_unsupported = true;
            }
        }
    }
    if saw_unsupported {
        Expectation::Unsupported
    } else if saw_continue {
        Expectation::Continue
    } else {
        Expectation::Absent
    }
}

/// The expectation one set of header lines makes. The refusal in [`parse_head_into`] and
/// [`awaits_100_continue`] both read the field through here, so a request cannot be refused for
/// one reading of its list and answered with an interim for another.
fn expectation_of(headers: &HeaderMap<'_>) -> Expectation {
    classify_expectation(
        headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("expect"))
            .map(|h| h.value),
    )
}

/// True when a *complete* request head has arrived, the body it frames is still incomplete,
/// and the client asked for an interim response with `Expect: 100-continue`.
///
/// RFC 9112 §9.3.3 keeps `Expect` semantics in HTTP/1.1 and later and forbids a 1xx for an
/// HTTP/1.0 request, so a supported expectation on that version is answered by serving the
/// request rather than by an interim response. The check is head-only by design: the whole point
/// of the expectation is to let a client skip sending a body the server would reject, so
/// answering it after buffering the body would defeat it.
///
/// A head whose expectation this server cannot meet never reads true here: [parse_head_into]
/// refuses it with `AetherError::UnsupportedExpectation` before any body is read, and that
/// refusal is the answer the client needs, not an interim one.
pub fn awaits_100_continue(buf: &[u8], limits: &ServerLimits) -> bool {
    // No declared-length ceiling: the interim answer is owed to a client that paused, and on a
    // streaming route the length it promises is the route's ceiling's decision, not this one's.
    match RequestView::parse_head(buf, limits, true, usize::MAX) {
        Ok(Some((head, _))) => {
            expectation_of(&head.headers) == Expectation::Continue
                && head.version == 1
                && head.version_minor >= 1
        }
        _ => false,
    }
}
/// True while the body that a complete head frames has not all arrived.
///
/// The interim answer exists because a client paused to wait for it, so a request that is already
/// whole owes nothing: the buffered loop reaches [`awaits_100_continue`] only after
/// [`parse_request_incremental`] returned `Ok(None)`, which is this same rule for a loop that
/// reads the body before dispatching. A frontend that dispatches from the head knows only the
/// framing and how many bytes it holds, so the rule is stated here in those terms.
///
/// A chunked body is reported pending. Its completeness belongs to [`ChunkedBodyDecoder`], the one
/// place the chunk framing rules live, and a second scan here could only disagree with it.
pub fn body_is_pending(buf: &[u8], header_len: usize, framing: BodyFraming) -> bool {
    match framing {
        BodyFraming::None => false,
        BodyFraming::ContentLength(len) => buf.len() < header_len.saturating_add(len),
        BodyFraming::Chunked => true,
    }
}

/// One decoded request: the borrowed head view, the owned body when the framing produced one
/// (a chunked body is assembled outside the buffer), and how many buffer bytes it consumed.
pub type ParsedRequest<'req> = (RequestView<'req>, Option<Vec<u8>>, usize);

/// How a request body is framed on the wire, decided from the head alone.
///
/// A streaming frontend dispatches as soon as the head is complete, so this is the only thing it
/// knows about the body it is about to receive. Every rule that decides it — the `Transfer-Encoding`
/// checks, the duplicate-framing refusal — lives in [`body_framing`], which
/// [`parse_request_incremental`] also uses, so a streamed and a buffered request cannot disagree
/// about where their body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFraming {
    /// No body: neither `Content-Length` nor `Transfer-Encoding` was present.
    None,
    /// Exactly this many payload bytes follow the head.
    ContentLength(usize),
    /// `Transfer-Encoding: chunked`.
    Chunked,
}

/// A complete request head whose body may still be arriving.
#[derive(Debug)]
pub struct StreamingHead<'req> {
    /// The parsed head. Its `body` is empty by construction: the body has not been delivered.
    pub view: RequestView<'req>,
    /// Bytes of the buffer the head occupies; the body starts here.
    pub header_len: usize,
    pub framing: BodyFraming,
}

/// Validate the parts of the head that decide framing, exactly as the buffered path does.
///
/// RFC 9112 §3.2: every HTTP/1.1 request must carry exactly one Host field. A missing Host leaves
/// virtual-host routing undefined; a duplicated one is a smuggling vector, because an upstream that
/// keeps the first value and a downstream that keeps the last disagree on the target. HTTP/1.0
/// requests predate the rule and stay accepted.
fn validate_head(head: &RequestHead<'_>) -> Result<(), AetherError> {
    if head.version == 1 && head.version_minor >= 1 {
        let mut host_values = head
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("host"));
        let Some(first) = host_values.next() else {
            return Err(AetherError::MalformedRequest);
        };
        if host_values.next().is_some() {
            return Err(AetherError::MalformedRequest);
        }
        if ows_trim(std::str::from_utf8(first.value).unwrap_or("")).is_empty() {
            return Err(AetherError::MalformedRequest);
        }
    }
    Ok(())
}

/// The framing the head declares, refusing the combinations RFC 9112 §6.1 forbids: `chunked` must
/// be the sole coding, it exists only in HTTP/1.1, and duplicate `Transfer-Encoding` fields are
/// refused rather than merged.
fn body_framing(head: &RequestHead<'_>) -> Result<BodyFraming, AetherError> {
    if head.te_count == 0 {
        return Ok(match head.content_length.unwrap_or(0) {
            0 => BodyFraming::None,
            len => BodyFraming::ContentLength(len),
        });
    }
    if head.version != 1
        || head.version_minor < 1
        || head.te_count != 1
        || head.te_chunked != 1
        || head.te_other
    {
        return Err(AetherError::MalformedRequest);
    }
    Ok(BodyFraming::Chunked)
}

/// Validate the head and decide how its body is framed.
///
/// The `Expect` promise is honoured one level below, inside the head parser, because every reader
/// of a head passes there; this function is the shared remainder, so a streamed and a buffered
/// request cannot disagree about framing.
fn validate_framed_head(head: &RequestHead<'_>) -> Result<BodyFraming, AetherError> {
    validate_head(head)?;
    body_framing(head)
}

/// Parse a complete request head without waiting for its body.
///
/// This is the streaming entry point: the head carries everything routing and the security gates
/// need, and [`StreamingHead::framing`] says how the body that follows is delimited. The same
/// validation runs as in [`parse_request_incremental`], so a head accepted here is a head that
/// path would also have accepted, with one deliberate difference: this function applies no
/// declared-length ceiling. Which ceiling a `Content-Length` must respect — the buffered
/// `max_body_bytes` or the route's `max_stream_body_bytes` — is the *route's* question, and the
/// route is resolved after the head parses. A caller that does not route the head to a streaming
/// route parses it again through [`parse_request_incremental`], which applies the buffered
/// ceiling; a streaming caller applies `max_stream_body_bytes` itself before dispatch.
///
/// `Ok(None)` means the head itself is not complete yet.
pub fn parse_request_head<'req>(
    buf: &'req [u8],
    limits: &ServerLimits,
) -> Result<Option<StreamingHead<'req>>, AetherError> {
    let Some((head, header_len)) = RequestView::parse_head(buf, limits, true, usize::MAX)? else {
        return Ok(None);
    };
    let framing = validate_framed_head(&head)?;
    Ok(Some(StreamingHead {
        view: RequestView::from_head(head, &[]),
        header_len,
        framing,
    }))
}

/// Parses a request whose body may still be arriving, decoding a chunked body through
/// caller-owned [`ChunkedBodyDecoder`] state.
///
/// This is the connection-loop entry point: the decoder remembers how much of the current
/// chunked body has already been validated, so a body that arrives over many reads is
/// decoded once rather than re-decoded from the start after every read. A
/// `Content-Length`-framed request leaves the decoder untouched.
pub fn parse_request_incremental<'req>(
    buf: &'req [u8],
    limits: &ServerLimits,
    decoder: &mut ChunkedBodyDecoder,
) -> Result<Option<ParsedRequest<'req>>, AetherError> {
    let Some((head, header_len)) =
        RequestView::parse_head(buf, limits, true, limits.max_body_bytes)?
    else {
        return Ok(None);
    };
    let framing = validate_framed_head(&head)?;

    if framing == BodyFraming::None {
        return Ok(Some((RequestView::from_head(head, &[]), None, header_len)));
    }

    if let BodyFraming::ContentLength(cl) = framing {
        let total = header_len
            .checked_add(cl)
            .ok_or(AetherError::BodyLimitExceeded)?;
        if buf.len() < total {
            return Ok(None);
        }
        return Ok(Some((
            RequestView::from_head(head, &buf[header_len..total]),
            None,
            total,
        )));
    }

    // Resume where the last read stopped. The remembered offset is unusable when it points
    // before this head (the decoder has not seen this request yet, or a pipelined request
    // was consumed and the buffer moved under it) or past the end of the buffer: either way,
    // start over rather than decode from a stale position.
    if decoder.pos < header_len || decoder.pos > buf.len() {
        decoder.reset();
        decoder.pos = header_len;
    }
    decoder.feed(buf, limits)?;
    if !decoder.complete {
        return Ok(None);
    }

    let consumed = decoder.pos;
    let body = decoder.take_body();
    // The body belongs to the caller now. Clearing the decoder here (rather than trusting
    // the caller to reset it) means a finished body can never be reported twice — a second
    // parse of the same buffer would otherwise short-circuit and hand back an empty body.
    decoder.reset();
    Ok(Some((
        RequestView::from_head(head, &[]),
        Some(body),
        consumed,
    )))
}

/// Parses a request, decoding a `Transfer-Encoding: chunked` body into an owned buffer.
///
/// Returns the request view, an owned body when the request was chunked (`None` means the
/// body is borrowed from `buf` and available as `view.body`), and the exact number of
/// consumed bytes so pipelined requests stay framed.
///
/// A whole-buffer convenience over [`parse_request_incremental`] for callers that hold the
/// complete request. A connection receiving a chunked body in pieces must use the
/// incremental form, or it re-decodes every buffered byte on every read.
pub fn parse_request<'req>(
    buf: &'req [u8],
    limits: &ServerLimits,
) -> Result<Option<ParsedRequest<'req>>, AetherError> {
    parse_request_incremental(buf, limits, &mut ChunkedBodyDecoder::default())
}

/// Maximum bytes permitted for a chunk-size line (including extensions) or a trailer line.
const MAX_CHUNK_LINE: usize = 1024;

/// Extracts the HTTP minor version from the request line (`HTTP/1.0` -> 0). httparse only
/// reports the major version, so the raw first line is scanned; anything else defaults to 1,
/// preserving the pre-1.1 semantics the server already implements.
///
/// The scan is restricted to the FIRST line, terminated by LF (RFC 9110 §3.1.1 tolerates
/// bare-LF request lines, which httparse accepts). Splitting on `\r` instead would make a
/// LF-only request line swallow the entire header block, letting a later header value that
/// contains `HTTP/1.` — e.g. a Referer URL — decide the version and flip keep-alive
/// semantics. Within the line the LAST `HTTP/1.` token wins, because a URI could contain
/// an earlier look-alike.
fn request_line_version_minor(head_bytes: &[u8]) -> u8 {
    let line = head_bytes
        .split(|b| *b == b'\n')
        .next()
        .unwrap_or(head_bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    line.windows(b"HTTP/1.".len())
        .rposition(|w| w == b"HTTP/1.")
        .and_then(|i| line.get(i + b"HTTP/1.".len()))
        .filter(|c| c.is_ascii_digit())
        .map_or(1, |c| c - b'0')
}

fn find_crlf(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|w| w == b"\r\n")
}

fn is_token_char(b: u8) -> bool {
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
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Incremental decode state for one `Transfer-Encoding: chunked` request body.
///
/// A chunked body arrives over many reads. Without this state the connection loop would
/// re-decode every buffered byte after every read — O(bytes-so-far) per read, so a body
/// arriving in 8 KiB slices costs ~130x the bytes it carries in copying — and would throw
/// away the partially decoded payload each time. The decoder keeps the validated offset
/// and the decoded bytes, so each wire byte is parsed exactly once.
///
/// The framing rules (hex size lines, extensions, chunk-trailing CRLF, trailer fields and
/// their budgets) live only in `ChunkedBodyDecoder::feed`, which is private and reached only
/// through [`parse_request_incremental`]; `Content-Length`-framed requests never touch it.
#[derive(Default)]
pub struct ChunkedBodyDecoder {
    /// Absolute offset in the connection buffer of the first wire byte not yet validated.
    /// Zero means the request head has not been seen yet; the parser seeds it with the
    /// header length.
    pos: usize,
    /// Decoded payload accumulated so far.
    body: Vec<u8>,
    /// Data bytes still owed by the chunk whose size line has already been validated.
    chunk_remaining: usize,
    /// A size line has been validated and its framing is not finished yet: `chunk_remaining`
    /// bytes of payload, then the chunk's trailing CRLF. This has to be a separate state,
    /// because `chunk_remaining == 0` cannot tell "between chunks" from "payload in, CRLF
    /// not yet buffered" — reading the latter as a size line would treat the chunk's own
    /// CRLF as an empty (and illegal) size.
    in_chunk: bool,
    /// Set by the zero-size chunk: everything after it is trailer fields.
    in_trailers: bool,
    /// Trailer field bytes and count spent since the zero-size chunk.
    trailer_bytes: usize,
    trailer_count: usize,
    /// The terminating empty trailer line has been validated: the body is complete.
    complete: bool,
}

impl ChunkedBodyDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the current body. Called when a completed body is handed to the caller and
    /// whenever the remembered offset no longer belongs to the request at the front of the
    /// buffer.
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Bytes held by a partially decoded body. These are live memory beside the wire copy
    /// still sitting in the read buffer, which is why the connection loop charges them.
    pub fn body_bytes(&self) -> usize {
        self.body.len()
    }

    /// Bytes the decoder's accumulator has been handed by the allocator. A `Vec` grows by
    /// doubling, so this reaches the scheduler where [`Self::body_bytes`] would hide the
    /// difference: a body of 1.5 MiB sits in a 2 MiB block, and both blocks are equally real
    /// in RSS. A decoder that never saw a byte reports 0, so this is only ever larger than
    /// `body_bytes()` while a body is genuinely in flight.
    pub fn body_capacity_bytes(&self) -> usize {
        self.body.capacity()
    }

    fn take_body(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }

    /// True once the zero-size chunk and its trailer section have been validated: no further
    /// byte can belong to this body.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Streaming mode: validate `buf` from its **front**, hand the newly decoded payload to `out`,
    /// and return how many wire bytes at the front of `buf` the caller may now drop.
    ///
    /// This is the same state machine as [`Self::feed`], so a streamed and a buffered chunked body
    /// can never disagree about what a valid chunk is. The difference is only in who keeps the
    /// payload: here it leaves as soon as it is decoded, and the caller advances its buffer by the
    /// returned count, which keeps resident memory at "one wire window + one decoded window"
    /// instead of "the whole body".
    ///
    /// The returned count is always a point the decoder can resume from: it is either the start of
    /// a line it has not fully buffered yet, or a point inside a chunk whose remaining length it
    /// still remembers.
    pub fn stream_feed(
        &mut self,
        buf: &[u8],
        limits: &ServerLimits,
        out: &mut Vec<u8>,
    ) -> Result<usize, AetherError> {
        self.feed(buf, limits)?;
        let consumed = self.pos;
        out.extend_from_slice(&self.body);
        self.body.clear();
        // The caller drops exactly `consumed` bytes from the front, so the next call starts at
        // offset zero again. `in_chunk`, `chunk_remaining` and the trailer state carry the rest.
        self.pos = 0;
        Ok(consumed)
    }

    /// Validate and decode `data[self.pos..]`, leaving `pos` at the first byte that is not
    /// yet fully validated so the next read resumes there. `Ok(())` with `complete == false`
    /// simply means "more data needed"; a malformed or over-budget body is `Err`, and the
    /// whole message framing must then be refused rather than decoded twice.
    fn feed(&mut self, data: &[u8], limits: &ServerLimits) -> Result<(), AetherError> {
        loop {
            if self.complete {
                return Ok(());
            }

            // Inside a chunk whose size line already passed validation: copy what arrived,
            // then validate the CRLF that terminates the chunk framing.
            if self.in_chunk {
                let avail = data.len() - self.pos;
                let take = avail.min(self.chunk_remaining);
                if take > 0 {
                    self.body
                        .extend_from_slice(&data[self.pos..self.pos + take]);
                    self.pos += take;
                    self.chunk_remaining -= take;
                }
                if self.chunk_remaining > 0 || data.len() - self.pos < 2 {
                    return Ok(());
                }
                if &data[self.pos..self.pos + 2] != b"\r\n" {
                    return Err(AetherError::MalformedRequest);
                }
                self.pos += 2;
                self.in_chunk = false;
                continue;
            }

            // A size or trailer line must be buffered whole before it can be validated.
            let line_end = match find_crlf(&data[self.pos..]) {
                Some(rel) => self.pos + rel,
                None => {
                    if data.len() - self.pos > MAX_CHUNK_LINE {
                        return Err(AetherError::MalformedRequest);
                    }
                    return Ok(());
                }
            };
            let line = &data[self.pos..line_end];
            if line.len() > MAX_CHUNK_LINE {
                return Err(AetherError::MalformedRequest);
            }

            if self.in_trailers {
                if line.is_empty() {
                    self.pos = line_end + 2;
                    self.complete = true;
                    return Ok(());
                }
                self.trailer_bytes += line.len() + 2;
                self.trailer_count += 1;
                if self.trailer_bytes > limits.max_header_bytes
                    || self.trailer_count > limits.max_headers_count
                {
                    return Err(AetherError::HeaderTooLarge);
                }
                // Trailer fields must be well-formed `token ":" OWS value`; they are not
                // merged into the request headers, so they cannot influence routing.
                let Some(colon) = line.iter().position(|&b| b == b':') else {
                    return Err(AetherError::MalformedRequest);
                };
                if colon == 0 || !line[..colon].iter().all(|&b| is_token_char(b)) {
                    return Err(AetherError::MalformedRequest);
                }
                // RFC 9112 §6.5: a trailer must not carry a field that affects message
                // framing or routing. The head has already been validated without these
                // values, so honouring them here would let a client smuggle a second
                // Content-Length/Transfer-Encoding/Host past the parser.
                let name = &line[..colon];
                if name.eq_ignore_ascii_case(b"content-length")
                    || name.eq_ignore_ascii_case(b"transfer-encoding")
                    || name.eq_ignore_ascii_case(b"host")
                {
                    return Err(AetherError::MalformedRequest);
                }
                self.pos = line_end + 2;
                continue;
            }

            let size_part = line.split(|&b| b == b';').next().unwrap_or(b"");
            let size_part = size_part.trim_ascii();
            if size_part.is_empty()
                || size_part.len() > 16
                || !size_part.iter().all(|b| b.is_ascii_hexdigit())
            {
                return Err(AetherError::MalformedRequest);
            }
            let size_str =
                std::str::from_utf8(size_part).map_err(|_| AetherError::MalformedRequest)?;
            let size =
                u64::from_str_radix(size_str, 16).map_err(|_| AetherError::MalformedRequest)?;
            if size > limits.max_body_bytes as u64 {
                return Err(AetherError::BodyLimitExceeded);
            }
            let size = size as usize;
            self.pos = line_end + 2;

            if size == 0 {
                self.in_trailers = true;
                continue;
            }
            // The budget is checked on the declaration, so an over-limit total is refused
            // before the bytes that would exceed it are buffered.
            if self
                .body
                .len()
                .checked_add(size)
                .ok_or(AetherError::BodyLimitExceeded)?
                > limits.max_body_bytes
            {
                return Err(AetherError::BodyLimitExceeded);
            }
            self.chunk_remaining = size;
            self.in_chunk = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smuggling_dual_content_length_rejected() {
        let limits = ServerLimits::default();
        let raw =
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nContent-Length: 10\r\n\r\nhello";
        let result = RequestView::parse(raw, &limits);
        assert!(matches!(result, Err(AetherError::MalformedRequest)));
    }

    /// The other half of the same promise: this entry point frames the body from the declared
    /// length, not from whatever the buffer happens to hold, and reports only that much consumed
    /// so the surplus stays the next message. The serve path reads through
    /// `parse_request_incremental` (whose wire tests are in `tests/tests/http_e2e.rs`), so without
    /// this the rule here would be pinned by nothing — and this is public API.
    #[test]
    fn test_content_length_surplus_is_left_for_the_next_request() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\r\nhello\
                    GET / HTTP/1.1\r\nHost: t\r\n\r\n";
        let (view, consumed) = RequestView::parse(raw, &limits)
            .expect("parses")
            .expect("the promised body is present");
        assert_eq!(
            view.body, b"hello",
            "the body is the promise, not the buffer"
        );
        assert!(
            raw[consumed..].starts_with(b"GET / HTTP/1.1"),
            "the surplus must still be on the wire as the next request, got {:?}",
            std::str::from_utf8(&raw[consumed..]).unwrap_or("<non-utf8>")
        );
    }

    #[test]
    fn test_smuggling_cl_te_conflict_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let result = RequestView::parse(raw, &limits);
        assert!(matches!(result, Err(AetherError::MalformedRequest)));
    }

    /// `Transfer-Encoding` alone (with no `Content-Length`) must also be rejected while
    /// chunked support is absent — otherwise the body would be silently treated as empty
    /// and the trailing chunk data misparsed as the next pipelined request.
    #[test]
    fn test_smuggling_te_only_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    /// Obfuscated `Transfer-Encoding` spellings must not slip past the check.
    #[test]
    fn test_smuggling_obfuscated_transfer_encoding_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding : chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nTRANSFER-ENCODING: chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: identity, chunked\r\n\r\n"
                .as_slice(),
        ] {
            assert!(
                matches!(
                    RequestView::parse(raw, &limits),
                    Err(AetherError::MalformedRequest)
                ),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    /// A non-numeric or negative `Content-Length` must be rejected, not parsed as zero.
    #[test]
    fn test_malformed_content_length_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: abc\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: -5\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5x\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: \r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5 5\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(
                    RequestView::parse(raw, &limits),
                    Err(AetherError::MalformedRequest)
                ),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    /// An astronomically large `Content-Length` must not overflow `header_len + cl`.
    #[test]
    fn test_content_length_overflow_is_rejected_not_wrapped() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 18446744073709551615\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        // Just past usize::MAX on 64-bit: parse::<usize> fails, still rejected.
        let raw2 =
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 99999999999999999999999999\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw2, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    /// `max_headers_count` must be enforced and reported as 431, not a generic 400.
    #[test]
    fn test_header_count_limit_is_enforced() {
        let limits = ServerLimits {
            max_headers_count: 4,
            max_header_bytes: 64 * 1024,
            ..ServerLimits::default()
        };

        let build = |n: usize| {
            let mut raw = String::from("GET / HTTP/1.1\r\n");
            for i in 0..n {
                raw.push_str(&format!("X-Header-{i}: value\r\n"));
            }
            raw.push_str("\r\n");
            raw.into_bytes()
        };

        // At the limit: accepted.
        let at_limit = build(4);
        let ok = RequestView::parse(&at_limit, &limits).unwrap();
        assert!(ok.is_some());

        // One past the limit: 431.
        let over_limit = build(5);
        assert!(
            matches!(
                RequestView::parse(&over_limit, &limits),
                Err(AetherError::HeaderTooLarge)
            ),
            "configured max_headers_count must be enforced"
        );
    }

    /// The limit must be honoured above the 64-slot stack array too, via the heap fallback.
    #[test]
    fn test_header_count_limit_above_stack_slots() {
        let limits = ServerLimits {
            max_headers_count: 100,
            max_header_bytes: 64 * 1024,
            ..ServerLimits::default()
        };

        let build = |n: usize| {
            let mut raw = String::from("GET / HTTP/1.1\r\n");
            for i in 0..n {
                raw.push_str(&format!("X-Header-{i}: value\r\n"));
            }
            raw.push_str("\r\n");
            raw.into_bytes()
        };

        let at_limit = build(100);
        let ok = RequestView::parse(&at_limit, &limits).unwrap();
        let (req, _) = ok.expect("100 headers should parse under a 100-header budget");
        assert_eq!(req.headers.len(), 100);

        let over_limit = build(101);
        assert!(matches!(
            RequestView::parse(&over_limit, &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// The *default* budget is 100 field lines — the ceiling hyper's `httparse`
    /// configuration uses. The old default of 64 rejected legitimate clients (gRPC metadata,
    /// large cookie jars, JWT-heavy headers) without making the per-line work it protects
    /// against any cheaper; `max_header_bytes` is what actually bounds that.
    #[test]
    fn test_default_header_budget_is_a_hundred_field_lines() {
        let limits = ServerLimits::default();
        assert_eq!(limits.max_headers_count, 100);

        let build = |n: usize| {
            let mut raw = String::from("GET / HTTP/1.1\r\n");
            for i in 0..n {
                raw.push_str(&format!("X-H{i}: v\r\n"));
            }
            raw.push_str("\r\n");
            raw.into_bytes()
        };

        // Within the default 8 KiB byte budget, so only the count can be deciding here.
        let hundred = build(100);
        let (req, _) = RequestView::parse(&hundred, &limits)
            .unwrap()
            .expect("100 field lines must parse with the default limits");
        assert_eq!(req.headers.len(), 100);

        assert!(matches!(
            RequestView::parse(&build(101), &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// An array smaller than the head must be reported as *the array* running out, never as
    /// the configured limit. That distinction is what lets `parse_head` size the first attempt
    /// to the request and still answer 431 only from a ceiling-sized attempt.
    #[test]
    fn test_undersized_slot_array_reports_the_array_not_the_limit() {
        let limits = ServerLimits::default();
        let raw = b"GET / HTTP/1.1\r\nHost: t\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n";

        let mut too_few = [httparse::EMPTY_HEADER; 2];
        assert!(matches!(
            parse_head_into(&mut too_few, raw, &limits, false, limits.max_body_bytes),
            Ok(Attempt::SlotsRanOut)
        ));

        // The same bytes, same rule, enough slots: parsed, with the framing metadata intact.
        let mut enough = [httparse::EMPTY_HEADER; STACK_HEADER_SLOTS];
        let Ok(Attempt::Head(Some((head, header_len)))) =
            parse_head_into(&mut enough, raw, &limits, false, limits.max_body_bytes)
        else {
            panic!("an array that holds the head must parse it");
        };
        assert_eq!(head.headers.len(), 4);
        assert_eq!(
            &raw[..header_len],
            b"GET / HTTP/1.1\r\nHost: t\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n"
        );
    }

    /// The stack/heap boundary must not change what a client is allowed to send: 65 field
    /// lines is legal under the default 100-line ceiling and now sizes the array instead of
    /// inheriting it.
    #[test]
    fn test_head_just_above_the_stack_slots_parses() {
        let limits = ServerLimits::default();
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        for i in 0..65 {
            raw.push_str(&format!("X-H{i}: v\r\n"));
        }
        raw.push_str("\r\n");

        let (req, _) = RequestView::parse(raw.as_bytes(), &limits)
            .unwrap()
            .expect("65 field lines must parse under the default 100-line ceiling");
        assert_eq!(req.headers.len(), 65);
    }

    /// The estimate is the only new thing on the request path, so its contract is stated:
    /// it counts the head, stops at the blank line that ends it, and never scans past the
    /// byte budget a head is already refused for.
    #[test]
    fn test_header_slot_estimate_scans_one_head() {
        let one_head = b"GET / HTTP/1.1\r\nHost: t\r\nA: 1\r\n\r\nGET /x HTTP/1.1\r\nHost: t\r\nB: 2\r\nC: 3\r\nD: 4\r\n\r\n";
        // Only the first head is scanned: three field lines plus the terminator line, all
        // counted, and the pipelined second head never enters the estimate.
        assert_eq!(header_slot_estimate(one_head, 8192, 100), 4);

        // A bare-LF head terminates the scan the same way.
        let lf_only = b"GET / HTTP/1.1\nA: 1\n\nGET / HTTP/1.1\nB: 2\n\n";
        assert_eq!(header_slot_estimate(lf_only, 8192, 100), 3);

        // No terminator at all: the scan is bounded by the byte budget, so a slowloris
        // buffer cannot make the estimate a second full pass over everything sent.
        let partial = vec![b'a'; 4096];
        assert_eq!(header_slot_estimate(&partial, 128, 100), 1);

        // Whatever the head holds, the estimate never exceeds the ceiling it is a
        // shortcut for, and never drops below the one slot httparse needs.
        assert_eq!(header_slot_estimate(one_head, 8192, 1), 1);
        assert_eq!(
            header_slot_estimate(b"GET / HTTP/1.1\r\n\r\n", 8192, 100),
            2
        );
    }

    /// Exceeding the header byte budget must yield 431 whether or not the head is complete.
    #[test]
    fn test_header_byte_budget_enforced() {
        let limits = ServerLimits {
            max_header_bytes: 128,
            ..ServerLimits::default()
        };

        // Complete but oversized.
        let big = format!("GET / HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(4096));
        assert!(matches!(
            RequestView::parse(big.as_bytes(), &limits),
            Err(AetherError::HeaderTooLarge)
        ));

        // Incomplete and already oversized (Slowloris shape).
        let partial = format!("GET / HTTP/1.1\r\nX-Pad: {}", "a".repeat(4096));
        assert!(matches!(
            RequestView::parse(partial.as_bytes(), &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// A `Content-Length` above the body budget must be refused before any body is read.
    #[test]
    fn test_body_limit_enforced_from_content_length() {
        let limits = ServerLimits {
            max_body_bytes: 10,
            ..ServerLimits::default()
        };

        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 11\r\n\r\n0123456789a";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        let ok = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\n0123456789";
        let (req, consumed) = RequestView::parse(ok, &limits).unwrap().unwrap();
        assert_eq!(req.body, b"0123456789");
        assert_eq!(consumed, ok.len());
    }

    /// The streaming head applies no declared-length ceiling; the buffered paths apply
    /// `max_body_bytes`. Which ceiling a `Content-Length` must fit is the receiving route's
    /// decision, and a streaming route's ceiling is deliberately larger — so the head parse
    /// cannot be the place that refuses the length, or a streamed route could carry past the
    /// buffered ceiling only with chunked framing.
    #[test]
    fn test_streaming_head_leaves_the_declared_length_ceiling_to_the_route() {
        let limits = ServerLimits {
            max_body_bytes: 10,
            max_stream_body_bytes: 4096,
            ..ServerLimits::default()
        };
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 100\r\n\r\n";

        let head = parse_request_head(raw, &limits)
            .expect("the head is well formed")
            .expect("the head is complete");
        assert_eq!(head.framing, BodyFraming::ContentLength(100));

        // The buffered paths still refuse the same bytes against their own ceiling.
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));
        // The interim answer is owed to a paused client whose length the route may allow.
        assert!(!awaits_100_continue(raw, &limits));
        let expect =
            b"POST / HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nContent-Length: 100\r\n\r\n";
        assert!(awaits_100_continue(expect, &limits));
    }

    /// Partial requests must report "need more data" rather than an error.
    #[test]
    fn test_partial_requests_request_more_data() {
        let limits = ServerLimits::default();

        // Headers not terminated yet.
        assert!(
            RequestView::parse(b"GET / HTTP/1.1\r\nHost: x\r\n", &limits)
                .unwrap()
                .is_none()
        );

        // Headers complete, body short.
        assert!(
            RequestView::parse(
                b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\nabc",
                &limits
            )
            .unwrap()
            .is_none()
        );

        // Empty buffer.
        assert!(RequestView::parse(b"", &limits).unwrap().is_none());
    }

    /// The consumed-byte count must be exact, or pipelining desynchronises the stream.
    #[test]
    fn test_consumed_offset_is_exact_for_pipelining() {
        let limits = ServerLimits::default();
        let raw = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";

        let (req1, consumed1) = RequestView::parse(raw, &limits).unwrap().unwrap();
        assert_eq!(req1.path, "/a");

        let (req2, consumed2) = RequestView::parse(&raw[consumed1..], &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req2.path, "/b");
        assert_eq!(consumed1 + consumed2, raw.len());
    }

    /// Path and query must be split without allocating, and `+`/`%` left untouched here —
    /// percent-decoding is a routing-layer concern, not a framing one.
    #[test]
    fn test_path_query_split() {
        let limits = ServerLimits::default();

        let (req, _) = RequestView::parse(b"GET /users?page=1&x=2 HTTP/1.1\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/users");
        assert_eq!(req.query_str, Some("page=1&x=2"));
        assert_eq!(req.raw_uri, "/users?page=1&x=2");

        let (req, _) = RequestView::parse(b"GET /users? HTTP/1.1\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/users");
        assert_eq!(req.query_str, Some(""));

        let (req, _) = RequestView::parse(b"GET /users HTTP/1.1\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/users");
        assert_eq!(req.query_str, None);
    }

    fn host_values<'a>(req: &RequestView<'a>) -> Vec<&'a [u8]> {
        req.headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("host"))
            .map(|h| h.value)
            .collect()
    }

    /// RFC 9112 §3.2.2: the absolute-form is accepted, its path and query are what routing
    /// sees, and its authority replaces the Host field - including when the two disagree, which
    /// is the case the host allow-list would otherwise decide on the wrong value.
    #[test]
    fn test_absolute_form_routes_on_its_own_path_and_replaces_host() {
        let limits = ServerLimits::default();

        let raw = b"GET http://example.com/a/b?x=1 HTTP/1.1\r\nHost: other.example\r\n\r\n";
        let (req, _) = RequestView::parse(raw, &limits).unwrap().unwrap();
        assert_eq!(req.raw_uri, "http://example.com/a/b?x=1");
        assert_eq!(req.path, "/a/b");
        assert_eq!(req.query_str, Some("x=1"));
        assert_eq!(
            host_values(&req),
            vec![b"example.com".as_slice()],
            "the target's authority is the only Host field"
        );

        // An uppercase scheme and a port are the same form; an absent path is `/`.
        let raw = b"GET HTTPS://example.com:8443 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let (req, _) = RequestView::parse(raw, &limits).unwrap().unwrap();
        assert_eq!(req.path, "/");
        assert_eq!(req.query_str, None);
        assert_eq!(host_values(&req), vec![b"example.com:8443".as_slice()]);

        // A query with no path still attaches to `/`.
        let raw = b"GET http://example.com?x=2 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let (req, _) = RequestView::parse(raw, &limits).unwrap().unwrap();
        assert_eq!(req.path, "/");
        assert_eq!(req.query_str, Some("x=2"));

        // A bracketed IPv6 authority (with and without a port) is the same form.
        let raw = b"GET http://[::1]:8080/a HTTP/1.1\r\nHost: [::1]\r\n\r\n";
        let (req, _) = RequestView::parse(raw, &limits).unwrap().unwrap();
        assert_eq!(req.path, "/a");
        assert_eq!(host_values(&req), vec![b"[::1]:8080".as_slice()]);
    }

    /// §3.2: the four forms are exhaustive. A target outside them is a 400, not a path that
    /// happens to match a route - `GET hello HTTP/1.1` used to route to `/hello`.
    #[test]
    fn test_targets_outside_the_four_forms_are_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"GET hello HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET ?x=1 HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET * HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET http:///x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET http://u@t/x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET http://t:abc/x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET http://[::1/x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET http://::1/x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET ftp://t/x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"GET /x#frag HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"CONNECT /x HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"CONNECT t HTTP/1.1\r\nHost: t\r\n\r\n".as_slice(),
            b"OPTIONS t:443 HTTP/1.1\r\nHost: t:443\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(
                    RequestView::parse(raw, &limits),
                    Err(AetherError::MalformedRequest)
                ),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    /// §3.2.4: the asterisk is a server-wide OPTIONS target and stays distinguishable from
    /// every origin-form path; §3.2.3: a CONNECT authority-form is the only other target with
    /// no `/`.
    #[test]
    fn test_asterisk_and_authority_forms_are_carried_through() {
        let limits = ServerLimits::default();

        let (req, _) = RequestView::parse(b"OPTIONS * HTTP/1.1\r\nHost: t\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "*");
        assert_eq!(req.query_str, None);

        let (req, _) =
            RequestView::parse(b"CONNECT t:443 HTTP/1.1\r\nHost: t:443\r\n\r\n", &limits)
                .unwrap()
                .unwrap();
        assert_eq!(req.method, Method::Connect);
        assert_eq!(req.path, "t:443");
        assert_eq!(req.query_str, None);
    }

    /// §3.2 requires a Host field on every HTTP/1.1 request, and the absolute-form replacement
    /// must not hide its absence. HTTP/1.0 predates the rule, so it gets the authority as its
    /// Host instead of a rejection.
    #[test]
    fn test_absolute_form_does_not_excuse_a_missing_host() {
        let limits = ServerLimits::default();

        assert!(matches!(
            parse_request(b"GET http://example.com/x HTTP/1.1\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));

        let (req, _, _) = parse_request(b"GET http://example.com/x HTTP/1.0\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(host_values(&req), vec![b"example.com".as_slice()]);
    }

    /// §3.2 makes two Host fields a 400 for every request; the absolute-form replacement must
    /// not be able to collapse them first.
    #[test]
    fn test_duplicate_host_is_refused_on_http10_too() {
        let limits = ServerLimits::default();
        assert!(matches!(
            RequestView::parse(b"GET / HTTP/1.0\r\nHost: a\r\nHost: b\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    /// Garbage on the wire must be rejected cleanly, never panic.
    #[test]
    fn test_malformed_requests_do_not_panic() {
        let limits = ServerLimits::default();
        let hostile: [&[u8]; 12] = [
            b"\x00\x00\x00",
            b"\r\n\r\n",
            b"GET",
            b"GET ",
            b"GET / ",
            b"GET / HTTP/1.1",
            b"NOTAMETHOD / HTTP/1.1\r\n\r\n",
            b"GET /\x00 HTTP/1.1\r\n\r\n",
            b"GET / HTTP/9.9\r\n\r\n",
            b"GET / HTTP/1.1\r\nBad Header No Colon\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: \xc3\x28\r\n\r\n",
            &[0xFF; 64],
        ];
        for raw in hostile {
            let _ = RequestView::parse(raw, &limits);
        }
    }

    /// Non-standard methods must not be silently coerced into a standard one.
    #[test]
    fn test_unknown_method_rejected() {
        let limits = ServerLimits::default();
        assert!(matches!(
            RequestView::parse(b"PROPFIND / HTTP/1.1\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    // ------------------------------------------------------------------ chunked bodies

    fn chunked(body: &str) -> Vec<u8> {
        body.as_bytes().to_vec()
    }

    #[test]
    fn test_chunked_single_and_multi_chunk_decoded() {
        let limits = ServerLimits::default();

        let raw = b"POST /upload HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let (req, owned, consumed) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(req.path, "/upload");
        assert_eq!(owned.as_deref(), Some(b"hello".as_slice()));
        assert!(
            req.body.is_empty(),
            "owned bodies must not alias the connection buffer"
        );
        assert_eq!(consumed, raw.len());

        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let (_, owned, consumed) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"abcde".as_slice()));
        assert_eq!(consumed, raw.len());
    }

    /// Chunk extensions are legal and must be ignored, not treated as part of the size.
    #[test]
    fn test_chunk_extensions_ignored() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5;name=value;x=y\r\nhello\r\n0\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"hello".as_slice()));
    }

    /// Every strict prefix of a chunked request must request more data, never error or
    /// report a short body.
    #[test]
    fn test_chunked_partial_at_every_prefix_requests_more() {
        let limits = ServerLimits::default();
        let raw = chunked(
            "POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\nX-T: 1\r\n\r\n",
        );

        for cut in 0..raw.len() {
            assert!(
                parse_request(&raw[..cut], &limits).unwrap().is_none(),
                "prefix of {} bytes must be incomplete",
                cut
            );
        }
        let (req, owned, consumed) = parse_request(&raw, &limits).unwrap().unwrap();
        assert_eq!(req.body.len(), 0);
        assert_eq!(owned.as_deref(), Some(b"hello world".as_slice()));
        assert_eq!(consumed, raw.len());
    }

    /// Consumed bytes must include the terminating chunk and trailers so a pipelined
    /// request behind a chunked body stays framed.
    #[test]
    fn test_chunked_consumed_offset_is_exact_for_pipelining() {
        let limits = ServerLimits::default();
        let raw = b"POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nbody\r\n0\r\nTrailer: yes\r\n\r\nGET /b HTTP/1.1\r\nHost: t\r\n\r\n";

        let (req1, owned, consumed) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(req1.path, "/a");
        assert_eq!(owned.as_deref(), Some(b"body".as_slice()));

        let (req2, owned2, consumed2) = parse_request(&raw[consumed..], &limits).unwrap().unwrap();
        assert_eq!(req2.path, "/b");
        assert!(owned2.is_none());
        assert_eq!(consumed + consumed2, raw.len());
    }

    /// The legacy entry point must keep rejecting Transfer-Encoding; only `parse_request`
    /// opts into chunked decoding.
    #[test]
    fn test_legacy_parse_still_rejects_transfer_encoding() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_chunked_with_content_length_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_duplicate_and_unsupported_transfer_encoding_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: gzip\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: identity\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked, gzip\r\n\r\n0\r\n\r\n".as_slice(),
        ] {
            let res = parse_request(raw, &limits);
            assert!(
                matches!(res, Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}, got {:?}",
                String::from_utf8_lossy(raw),
                res.map(|o| o.map(|(_, b, c)| (b, c)))
            );
        }
    }

    #[test]
    fn test_chunked_http10_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_chunked_malformed_framing_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            // Non-hex size.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n".as_slice(),
            // Signed / 0x-prefixed size.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n+5\r\nhello\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0x5\r\nhello\r\n".as_slice(),
            // Chunk data not followed by CRLF.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhelloXX".as_slice(),
            // Size over 16 hex digits (would overflow a reasonable usize).
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n11111111111111111\r\n".as_slice(),
            // Trailer without a colon.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nBadTrailer\r\n\r\n".as_slice(),
            // Obsolete line folding / space before the trailer colon.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-T : 1\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(parse_request(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn test_chunked_body_limit_enforced() {
        let limits = ServerLimits {
            max_body_bytes: 10,
            ..ServerLimits::default()
        };

        // Declared chunk size alone exceeds the budget: reject before buffering data.
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n11\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        // Individually legal chunks that together exceed the budget.
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n6\r\nghijkl\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        // Exactly at the budget is accepted.
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"0123456789".as_slice()));
    }

    #[test]
    fn test_chunked_trailers_are_bounded() {
        let limits = ServerLimits {
            max_header_bytes: 100,
            max_headers_count: 2,
            ..ServerLimits::default()
        };

        let raw =
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nA: 1\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"".as_slice()));

        let padding = "a".repeat(200);
        let too_large = format!(
            "POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Padding: {padding}\r\n\r\n"
        );
        assert!(matches!(
            parse_request(too_large.as_bytes(), &limits),
            Err(AetherError::HeaderTooLarge)
        ));

        let too_many = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n";
        assert!(matches!(
            parse_request(too_many, &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// Mixed case and surrounding OWS are legal for the `chunked` token.
    #[test]
    fn test_chunked_token_is_case_insensitive() {
        let limits = ServerLimits::default();
        let raw =
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: Chunked \r\n\r\n1\r\nx\r\n0\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"x".as_slice()));
    }

    /// Regression: scanning for the version token used to split the head on `\r`, so a
    /// bare-LF request line (which httparse accepts) made the "first line" the whole
    /// header block, and a later header value containing `HTTP/1.` — e.g. a Referer URL —
    /// decided the connection's keep-alive semantics.
    #[test]
    fn test_version_minor_reads_only_the_first_line() {
        assert_eq!(
            request_line_version_minor(b"GET / HTTP/1.0\nHost: x\n\n"),
            0
        );
        assert_eq!(
            request_line_version_minor(b"GET / HTTP/1.1\nHost: x\n\n"),
            1
        );
        assert_eq!(
            request_line_version_minor(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n"),
            0
        );

        let crlf = b"GET /x?y=1 HTTP/1.1\r\nReferer: http://e/HTTP/1.0/page\r\n\r\n";
        assert_eq!(request_line_version_minor(crlf), 1);
        let lf_only = b"GET /x?y=1 HTTP/1.1\nReferer: http://e/HTTP/1.0/page\n\n";
        assert_eq!(request_line_version_minor(lf_only), 1);

        // Missing or non-numeric version tokens fall back to 1.
        assert_eq!(request_line_version_minor(b"GET / weird\r\n\r\n"), 1);
        assert_eq!(request_line_version_minor(b"GET / HTTP/1.x\r\n\r\n"), 1);
    }

    /// RFC 9112 §3.2: an HTTP/1.1 request must carry exactly one Host field. Zero leaves
    /// routing undefined; two is a smuggling vector (first-vs-last divergence). HTTP/1.0
    /// predates the rule and must stay accepted.
    #[test]
    fn test_http11_requires_exactly_one_host() {
        let limits = ServerLimits::default();

        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nUser-Agent: x\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost: a\r\nhost: b\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));

        let (req, _, _) = parse_request(b"GET / HTTP/1.1\r\nHost: example\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/");

        assert!(
            parse_request(b"GET / HTTP/1.0\r\n\r\n", &limits)
                .unwrap()
                .is_some()
        );
    }

    /// RFC 9112 §6.5: trailers must not carry framing- or routing-critical fields. The
    /// head was already validated without them, so accepting these names would smuggle a
    /// second Content-Length/Transfer-Encoding/Host past the parser. Ordinary trailers
    /// remain legal.
    #[test]
    fn test_chunked_rejects_framing_trailer_names() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nContent-Length: 99\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nTRANSFER-ENCODING: chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nHost: evil\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\ncontent-length: 0\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(parse_request(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }

        let ok = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Checksum: abc\r\n\r\n";
        assert!(parse_request(ok, &limits).unwrap().is_some());
    }

    /// T1-a (CL.TE vector): framing values may be padded only by RFC 9112 OWS
    /// (SP/HTAB). `str::trim` additionally strips Unicode whitespace such as
    /// U+00A0, so `Transfer-Encoding: <NBSP>chunked` framed the body as chunked
    /// for Aether while strict frontends see a non-`chunked` token and fall back
    /// to Content-Length. Both spellings must be refused.
    #[test]
    fn test_framing_headers_accept_only_ascii_ows() {
        let limits = ServerLimits::default();
        let cases = [
            // NBSP-prefixed/suffixed chunked -> must NOT be treated as chunked
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: \xc2\xa0chunked\r\n\r\n0\r\n\r\n"
                .as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\xc2\xa0\r\n\r\n0\r\n\r\n"
                .as_slice(),
            // NBSP-padded Content-Length -> must NOT parse as the number
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: \xc2\xa05\r\n\r\nhello".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\xc2\xa0\r\n\r\nhello".as_slice(),
        ];
        for raw in cases {
            assert!(
                matches!(
                    parse_request(raw, &limits),
                    Err(AetherError::MalformedRequest)
                ),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
        // OWS (SP/HTAB) continues to be accepted.
        let ok = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length:\t5 \r\n\r\nhello";
        assert!(parse_request(ok, &limits).unwrap().is_some());
    }

    /// T1-d: the exactly-one-Host rule must also require a non-empty field value
    /// (RFC 9112 §3.2.2 origin-form); `Host: ` is not a valid authority.
    #[test]
    fn test_http11_rejects_empty_host_value() {
        let limits = ServerLimits::default();
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost: \r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost:\t\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
        assert!(
            parse_request(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", &limits)
                .unwrap()
                .is_some()
        );
    }

    /// `Expect: 100-continue` must be recognised from the head alone, before the body
    /// arrives, so the server can answer it while the client is still waiting.
    #[test]
    fn test_await_100_continue_only_for_a_complete_head_with_pending_body() {
        let limits = ServerLimits::default();
        // Head complete, `Content-Length` bytes still missing: this is the interim case.
        let pending =
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n";
        assert!(awaits_100_continue(pending, &limits));
        assert!(parse_request(pending, &limits).unwrap().is_none());

        // The whole request already arrived: no interim response is owed, and the caller
        // never reaches the pending-body branch anyway.
        let complete =
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello";
        assert!(parse_request(complete, &limits).unwrap().is_some());

        // Head itself is still incomplete.
        assert!(!awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: 100-cont",
            &limits
        ));
        // A malformed head must not become an interim response.
        assert!(!awaits_100_continue(b"NOT A REQUEST\r\n\r\n", &limits));
    }

    /// RFC 9112 §9.3.3 keeps Expect semantics in HTTP/1.1 and later, and the value is a
    /// token list, so only an exact `100-continue` expectation counts.
    #[test]
    fn test_await_100_continue_requires_11_and_exact_expectation_token() {
        let limits = ServerLimits::default();
        assert!(!awaits_100_continue(
            b"PUT /f HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
        assert!(awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
        assert!(awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nEXPECT: 100-Continue\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
        // A list that mixes the supported token with one this server cannot meet is refused, not
        // answered: `Expect` promises every element is met before the body is sent.
        assert!(!awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: other, 100-continue\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
        // A token that merely contains it is a different expectation.
        assert!(!awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: 100-continue-x\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
        assert!(!awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: x100-continue\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
        assert!(!awaits_100_continue(
            b"PUT /f HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\r\n",
            &limits
        ));
    }

    /// Every shape the `Expect` value can arrive in is decided here, by the one function both
    /// frontends call, so no frontend has to re-read the list rule to answer.
    #[test]
    fn test_classify_expectation_reads_the_value_as_a_list_of_tokens() {
        let classify = |values: &[&[u8]]| classify_expectation(values.iter().copied());
        assert_eq!(classify(&[]), Expectation::Absent);
        assert_eq!(
            classify(&[b"100-continue".as_slice()]),
            Expectation::Continue
        );
        // The field name and the token are both case-insensitive.
        assert_eq!(
            classify(&[b"100-CONTINUE".as_slice()]),
            Expectation::Continue
        );
        // OWS around an element is not part of the token (RFC 9112 §5.6.1).
        assert_eq!(
            classify(&[b"  100-continue \t".as_slice()]),
            Expectation::Continue
        );
        // RFC 9110 §5.5's `#` rule skips empty elements, so a stray comma is not a second
        // expectation and a list of nothing but commas is no expectation at all.
        assert_eq!(
            classify(&[b", 100-continue".as_slice()]),
            Expectation::Continue
        );
        assert_eq!(
            classify(&[b"100-continue,,".as_slice()]),
            Expectation::Continue
        );
        assert_eq!(classify(&[b",,".as_slice()]), Expectation::Absent);
        // Two fields naming the same expectation are still one expectation.
        assert_eq!(
            classify(&[b"100-continue".as_slice(), b"100-continue".as_slice()]),
            Expectation::Continue
        );
        assert_eq!(classify(&[b"foo=bar".as_slice()]), Expectation::Unsupported);
        // A token that merely contains the supported one, or quotes it, is a different
        // expectation; so is padding that OWS does not cover.
        assert_eq!(
            classify(&[b"100-continue-x".as_slice()]),
            Expectation::Unsupported
        );
        assert_eq!(
            classify(&[b"x100-continue".as_slice()]),
            Expectation::Unsupported
        );
        assert_eq!(
            classify(&[b"\"100-continue\"".as_slice()]),
            Expectation::Unsupported
        );
        assert_eq!(
            classify(&["\u{a0}100-continue".as_bytes()]),
            Expectation::Unsupported
        );
        // Half a promise is not a met one: a list containing an unsupported element refuses even
        // when the supported token is in it.
        assert_eq!(
            classify(&[b"100-continue, foo".as_slice()]),
            Expectation::Unsupported
        );
        assert_eq!(
            classify(&[b"foo".as_slice(), b"100-continue".as_slice()]),
            Expectation::Unsupported
        );
    }

    /// RFC 9110 §10.1.1 makes the refusal a verdict about the head, so it arrives before the body
    /// is read and before any verdict about that body - and the grammar refusals still arrive
    /// before it, because a head that cannot be framed has no promise to judge yet.
    #[test]
    fn test_unsupported_expectation_is_refused_from_the_head_ahead_of_the_body_budget() {
        let limits = ServerLimits::default();
        let pending = b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\n";
        assert!(matches!(
            parse_request(pending, &limits),
            Err(AetherError::UnsupportedExpectation)
        ));
        // The paused client is not answered with an interim on the way to the refusal.
        assert!(!awaits_100_continue(pending, &limits));

        let with_body =
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\nhello";
        assert!(matches!(
            parse_request(with_body, &limits),
            Err(AetherError::UnsupportedExpectation)
        ));
        // The streaming reader and the `Content-Length`-only legacy reader refuse identically: they
        // parse the same head, so neither can dispatch a handler on an unmet promise.
        assert!(matches!(
            parse_request_head(with_body, &limits),
            Err(AetherError::UnsupportedExpectation)
        ));
        assert!(matches!(
            RequestView::parse(with_body, &limits),
            Err(AetherError::UnsupportedExpectation)
        ));

        // The expectation verdict outranks the body budget: the request never reached the point
        // where its size matters.
        let one_byte = ServerLimits {
            max_body_bytes: 1,
            ..ServerLimits::default()
        };
        assert!(matches!(
            parse_request(with_body, &one_byte),
            Err(AetherError::UnsupportedExpectation)
        ));
        // ...and the budget still stands on its own when the promise is met.
        assert!(matches!(
            parse_request(
                b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello",
                &one_byte
            ),
            Err(AetherError::BodyLimitExceeded)
        ));

        // HTTP/1.0 forbids the interim response, not the refusal.
        assert!(matches!(
            parse_request(
                b"PUT /f HTTP/1.0\r\nExpect: foo\r\nContent-Length: 5\r\n\r\n",
                &limits
            ),
            Err(AetherError::UnsupportedExpectation)
        ));
        // Grammar first: a head that cannot be framed is malformed whatever it expected, so the
        // `Content-Length` plus `Transfer-Encoding` request still answers 400, not 417.
        assert!(matches!(
            parse_request(
                b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: foo\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n",
                &limits
            ),
            Err(AetherError::MalformedRequest)
        ));
        // A request that is not a request at all is not an expectation failure either.
        assert!(!awaits_100_continue(b"NOT A REQUEST\r\n\r\n", &limits));
    }

    /// A head this server will not even hold is `431` whatever it promised: the capacity verdict is
    /// reached before the expectation is judged. Measured shape of the claim, stated precisely — on
    /// the shipped sizing that verdict comes from `parse_head`'s ceiling-sized attempt, which sits
    /// above every check in `parse_head_into`; the count check inside `parse_head_into` is the
    /// belt-and-braces that answers when the array is not capped to the ceiling. Injecting the
    /// reordering alone is therefore behaviourally identical on the shipped tree (measured: this test
    /// stayed green), and injecting it on top of an uncapped estimate turns this verdict into `417`
    /// and fails here (measured, at this file's `over_limit` assertion).
    #[test]
    fn test_header_count_refusal_outranks_the_expectation_refusal() {
        let tight = ServerLimits {
            max_headers_count: 1,
            ..ServerLimits::default()
        };
        let over_limit =
            b"PUT /f HTTP/1.1\r\nHost: t\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\n";
        assert!(
            matches!(
                parse_request(over_limit, &tight),
                Err(AetherError::HeaderTooLarge)
            ),
            "a head past max_headers_count must answer 431, not 417"
        );
        assert!(!awaits_100_continue(over_limit, &tight));

        assert!(matches!(
            parse_request(over_limit, &ServerLimits::default()),
            Err(AetherError::UnsupportedExpectation)
        ));
    }

    /// The shape the count check inside [`parse_head_into`] is the only authority for. The sizing
    /// above caps the slot array at `max_headers_count.max(1)`, so with any nonzero budget a head
    /// that reaches that check can never hold more entries than the budget allows — a head past a
    /// nonzero budget is refused by `parse_head`'s ceiling attempt instead. A zero budget means
    /// "no field line is acceptable", and httparse still completes a one-line head into the single
    /// slot the budget is floored to, so nothing but that check refuses it.
    #[test]
    fn test_a_zero_header_budget_refuses_the_one_line_head_that_otherwise_parses() {
        let zero = ServerLimits {
            max_headers_count: 0,
            ..ServerLimits::default()
        };
        let one_line = b"PUT /f HTTP/1.1\r\nHost: t\r\n\r\n";
        assert!(
            matches!(
                parse_request(one_line, &zero),
                Err(AetherError::HeaderTooLarge)
            ),
            "a zero header budget must refuse even the single field line httparse can fit"
        );

        // The same head under the default budget is served, so the refusal above is the budget's
        // doing and not the shape of the head.
        assert!(matches!(
            parse_request(one_line, &ServerLimits::default()),
            Ok(Some(_))
        ));
    }

    /// The whole point of the resumable decoder: a body that arrives in pieces must be
    /// decoded from where the last read stopped, so the bytes are neither lost nor
    /// duplicated, whatever the slice size.
    #[test]
    fn test_incremental_chunked_decode_resumes_at_the_last_read() {
        let limits = ServerLimits::default();
        let raw: Vec<u8> = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n3\r\nabc\r\n0\r\nx: 1\r\n\r\n"
            .to_vec();
        for step in [1usize, 2, 3, 7, 13, 64, 4096] {
            let mut decoder = ChunkedBodyDecoder::new();
            let mut buf: Vec<u8> = Vec::new();
            let mut completed = None;
            for piece in raw.chunks(step) {
                buf.extend_from_slice(piece);
                let parsed = parse_request_incremental(&buf, &limits, &mut decoder).unwrap_or_else(
                    |error| panic!("legal body rejected at step {step}: {error:?}"),
                );
                if let Some((view, owned, consumed)) = parsed {
                    assert!(
                        view.body.is_empty(),
                        "chunked body must not borrow the buffer"
                    );
                    completed = Some((owned.unwrap(), consumed));
                    break;
                }
            }
            let (body, consumed) = completed.expect("chunked body never completed");
            assert_eq!(
                body,
                b"helloabc".to_vec(),
                "resumed decode lost or doubled bytes"
            );
            assert_eq!(
                consumed,
                raw.len(),
                "consumed offset must cover the trailers"
            );
        }
    }

    /// A chunk bigger than one read is held in the decoder, not re-parsed: the decoded
    /// prefix grows monotonically while the body is incomplete, nothing is served early,
    /// and the decoder is empty again once the body has been handed to the caller.
    #[test]
    fn test_incremental_decoder_holds_partial_chunk_progress() {
        let limits = ServerLimits::default();
        let raw: Vec<u8> = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n"
            .to_vec();
        let body_start = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let mut decoder = ChunkedBodyDecoder::new();
        let mut held = Vec::new();
        for end in body_start..raw.len() {
            assert!(
                parse_request_incremental(&raw[..end], &limits, &mut decoder)
                    .unwrap()
                    .is_none(),
                "an incomplete chunked body was served early"
            );
            held.push(decoder.body_bytes());
        }
        assert!(
            held.windows(2).all(|w| w[1] >= w[0]),
            "decoded progress went backwards: {held:?}"
        );
        assert!(
            held.iter().max() == Some(&10),
            "the chunk payload was never decoded into the held body: {held:?}"
        );

        let (_, body, _) = parse_request_incremental(&raw[..], &limits, &mut decoder)
            .unwrap()
            .expect("complete chunked body");
        assert_eq!(body.unwrap(), b"0123456789".to_vec());
        assert_eq!(decoder.body_bytes(), 0, "body was not handed over");
    }

    /// Two chunked requests on one connection share the decoder, and the second one is
    /// parsed from a buffer whose front was already consumed — the remembered offset is
    /// stale and must not be trusted.
    #[test]
    fn test_incremental_decoder_restarts_for_the_next_request_on_the_connection() {
        let limits = ServerLimits::default();
        let first: Vec<u8> = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n".to_vec();
        let second: Vec<u8> = b"POST /x HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\n\r\n".to_vec();

        let mut decoder = ChunkedBodyDecoder::new();
        let (_, body, consumed) = parse_request_incremental(&first, &limits, &mut decoder)
            .unwrap()
            .expect("first request");
        assert_eq!(body.unwrap(), b"abc".to_vec());
        assert_eq!(consumed, first.len());

        let mut conn: Vec<u8> = first[consumed..].to_vec();
        conn.extend_from_slice(&second);
        let (_, body, consumed) = parse_request_incremental(&conn, &limits, &mut decoder)
            .expect("second request parses")
            .unwrap_or_else(|| panic!("second request never completed after a buffer shift"));
        assert_eq!(body.unwrap(), b"hi".to_vec());
        assert_eq!(consumed, second.len());
    }

    /// A caller that hands over a *shorter* buffer than the one the decoder already read
    /// through must not decode from a position past the end of it. Restarting is safe
    /// because such a buffer still contains the whole head plus every byte it claims to
    /// hold, so the decoder simply re-reads the smaller prefix.
    #[test]
    fn test_incremental_decoder_ignores_an_offset_past_a_shorter_buffer() {
        let limits = ServerLimits::default();
        let head: Vec<u8> =
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let raw: Vec<u8> = [head.as_slice(), b"3\r\nabc\r\n4\r\nwxyz\r\n0\r\n\r\n"].concat();

        let mut decoder = ChunkedBodyDecoder::new();

        // One whole chunk decoded: the remembered offset now sits at the next size line.
        let first = head.len() + 8;
        assert!(
            parse_request_incremental(&raw[..first], &limits, &mut decoder)
                .unwrap()
                .is_none(),
            "a body with a chunk still to come is not complete"
        );

        // A shorter buffer must not be decoded from the stale offset.
        let short = head.len() + 4;
        assert!(
            parse_request_incremental(&raw[..short], &limits, &mut decoder)
                .unwrap()
                .is_none(),
            "a truncated buffer can never complete the body"
        );

        let (_, body, consumed) = parse_request_incremental(&raw, &limits, &mut decoder)
            .unwrap()
            .unwrap_or_else(|| panic!("the full buffer never completed the body"));
        assert_eq!(body.unwrap(), b"abcwxyz".to_vec());
        assert_eq!(consumed, raw.len());
    }
}
