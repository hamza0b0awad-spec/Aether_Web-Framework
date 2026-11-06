//! Property tests for the HTTP/1.1 request parser.
//!
//! These state what the hand-written unit tests can only sample: that *no* byte sequence
//! panics the parser, that *every* generated well-formed request round-trips its fields and
//! its consumed length, that *every* proper prefix of a request is incomplete rather than
//! prematurely complete, and that a chunked body decodes to the same payload however its wire
//! bytes are split across reads. Each property names, in the track-102 register entry, the
//! mutant it is known to kill.

use aether_http::limits::ServerLimits;
use aether_http::method::Method;
use aether_http::parser::{
    BodyFraming, ChunkedBodyDecoder, RequestView, awaits_100_continue, body_is_pending,
    parse_request, parse_request_head,
};
use proptest::prelude::*;

fn limits() -> ServerLimits {
    ServerLimits::default()
}

// ---------------------------------------------------------------------------------------------
// A generated, well-formed request: the fixture every round-trip property starts from.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ValidRequest {
    method: Method,
    path: String,
    query: Option<String>,
    minor: u8,
    host: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn method_strategy() -> impl Strategy<Value = Method> {
    prop_oneof![
        Just(Method::Get),
        Just(Method::Post),
        Just(Method::Put),
        Just(Method::Delete),
        Just(Method::Patch),
        Just(Method::Head),
    ]
}

/// Segments are ASCII tokens with no `/`, `?`, whitespace or control bytes, so the generated
/// request line is unambiguous by construction rather than by the parser's good will.
fn path_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec("[A-Za-z0-9._~-]{1,12}", 0..=4).prop_map(|segments| {
        if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", segments.join("/"))
        }
    })
}

fn valid_request_strategy() -> impl Strategy<Value = ValidRequest> {
    (
        method_strategy(),
        path_strategy(),
        prop::option::of("[A-Za-z0-9._~=&%+-]{1,30}"),
        0u8..=1,
        "[a-z0-9.-]{1,20}",
        prop::collection::vec("[A-Za-z0-9._~:/=+-]{0,40}", 0..=5),
        prop::collection::vec(any::<u8>(), 0..=512),
    )
        .prop_map(|(method, path, query, minor, host, values, body)| {
            let headers = values
                .into_iter()
                .enumerate()
                .map(|(i, value)| (format!("x-h{i}"), value))
                .collect();
            ValidRequest {
                method,
                path,
                query,
                minor,
                host,
                headers,
                body,
            }
        })
}

impl ValidRequest {
    fn raw_uri(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{}", self.path, q),
            None => self.path.clone(),
        }
    }

    fn wire(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(
            format!(
                "{} {} HTTP/1.{}\r\n",
                self.method.as_str(),
                self.raw_uri(),
                self.minor
            )
            .as_bytes(),
        );
        out.extend_from_slice(format!("Host: {}\r\n", self.host).as_bytes());
        out.extend_from_slice(format!("Content-Length: {}\r\n", self.body.len()).as_bytes());
        for (name, value) in &self.headers {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

fn header_pairs(view: &RequestView<'_>) -> Vec<(String, String)> {
    view.headers
        .iter()
        .map(|h| {
            (
                h.name.to_string(),
                String::from_utf8(h.value.to_vec()).expect("generated headers are ASCII"),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. Arbitrary bytes never panic any entry point.
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        // Integration tests have no crate source directory for proptest to drop a
        // `.proptest-regressions` file beside, and every case here is fully generated: a
        // shrunk counterexample is already printed in the failure message.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn an_arbitrary_byte_sequence_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let l = limits();

        if let Ok(Some((view, consumed))) = RequestView::parse(&bytes, &l) {
            prop_assert!(consumed <= bytes.len());
            // `path` and `query_str` are the two halves of the request target, split at the
            // first `?` — the split must lose nothing and invent nothing.
            let query_len = view.query_str.map_or(0, |q| q.len() + 1);
            prop_assert_eq!(view.path.len() + query_len, view.raw_uri.len());
        }

        if let Ok(Some((view, owned, consumed))) = parse_request(&bytes, &l) {
            prop_assert!(consumed <= bytes.len());
            match &owned {
                // A decoded (chunked) body lives outside the buffer, so the borrowed view must
                // not claim any part of it.
                Some(_) => prop_assert!(view.body.is_empty()),
                // A borrowed body is a slice of the caller's buffer, never of separate storage.
                None => {
                    let start = bytes.as_ptr() as usize;
                    let body = view.body.as_ptr() as usize;
                    prop_assert!(body >= start && body + view.body.len() <= start + bytes.len());
                }
            }
        }

        if let Ok(Some(head)) = parse_request_head(&bytes, &l) {
            prop_assert!(head.header_len <= bytes.len());
            prop_assert!(head.view.body.is_empty());
        }

        let _ = awaits_100_continue(&bytes, &l);
        // The pending-body rule is that check's companion on the frontend that dispatches from the
        // head, and it is handed a length the caller read from the same buffer: it must survive an
        // arbitrary buffer, a header length past the end, and a content length that cannot be added.
        for framing in [BodyFraming::None, BodyFraming::Chunked] {
            let _ = body_is_pending(&bytes, bytes.len() / 2, framing);
        }
        let _ = body_is_pending(&bytes, bytes.len() + 1, BodyFraming::ContentLength(usize::MAX));

        // The streaming decoder must also survive arbitrary input: it is the one entry point
        // that keeps state across calls, so a panic there would take a live connection down.
        let mut decoder = ChunkedBodyDecoder::new();
        let mut out = Vec::new();
        let mut rest: &[u8] = &bytes;
        for _ in 0..=bytes.len() {
            let Ok(consumed) = decoder.stream_feed(rest, &l, &mut out) else {
                break;
            };
            prop_assert!(consumed <= rest.len());
            if consumed == 0 || decoder.is_complete() {
                break;
            }
            rest = &rest[consumed..];
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2. A well-formed request round-trips every field and its exact consumed length.
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_valid_request_round_trips_through_the_parser(request in valid_request_strategy()) {
        let l = limits();
        let wire = request.wire();

        // Both public entry points must agree. `RequestView::parse` is the Content-Length-only
        // path used by embedded callers and by the fuzz target; `parse_request` is the runtime
        // path that also decodes chunked bodies.
        let (view_cl, consumed_cl) = RequestView::parse(&wire, &l)
            .expect("generated request must parse")
            .expect("must be complete");
        prop_assert_eq!(consumed_cl, wire.len());
        prop_assert_eq!(view_cl.method, request.method);
        prop_assert_eq!(view_cl.path, request.path.as_str());
        prop_assert_eq!(view_cl.query_str, request.query.as_deref());
        prop_assert_eq!(view_cl.body, request.body.as_slice());

        let (view, owned, consumed) =
            parse_request(&wire, &l).expect("generated request must parse").expect("must be complete");

        prop_assert_eq!(consumed, wire.len(), "a complete request must consume exactly its bytes");
        prop_assert!(owned.is_none(), "a Content-Length body is borrowed, not copied");
        prop_assert_eq!(view.method, request.method);
        let raw_uri = request.raw_uri();
        prop_assert_eq!(view.raw_uri, raw_uri.as_str());
        prop_assert_eq!(view.path, request.path.as_str());
        prop_assert_eq!(view.query_str, request.query.as_deref());
        // `version` carries httparse's version number, which is the minor one (HTTP/1.1 -> 1,
        // HTTP/1.0 -> 0); `version_minor` is the parser's own scan and must agree with it.
        prop_assert_eq!(view.version, request.minor);
        prop_assert_eq!(view.version_minor, request.minor);
        prop_assert_eq!(view.body, request.body.as_slice());

        let mut expected = vec![
            ("Host".to_string(), request.host.clone()),
            ("Content-Length".to_string(), request.body.len().to_string()),
        ];
        expected.extend(request.headers.iter().cloned());
        prop_assert_eq!(header_pairs(&view), expected);

        // The streaming entry point must agree with the buffered one about where the head ends
        // and how the body that follows is framed.
        let head = parse_request_head(&wire, &l)
            .expect("generated head must parse")
            .expect("head must be complete");
        prop_assert_eq!(head.header_len, wire.len() - request.body.len());
        prop_assert_eq!(head.view.path, request.path.as_str());
        match head.framing {
            BodyFraming::None => prop_assert!(request.body.is_empty()),
            BodyFraming::ContentLength(len) => prop_assert_eq!(len, request.body.len()),
            BodyFraming::Chunked => prop_assert!(false, "no generated request declared chunked"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Every proper prefix of a well-formed request is incomplete.
//
// This is the property a streaming server lives on: a parser that answers "complete" one byte
// early hands a handler a half-received body and then misreads the rest of the body as the
// next pipelined request.
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_proper_prefix_of_a_request_is_incomplete(request in valid_request_strategy()) {
        let l = limits();
        let wire = request.wire();
        for cut in 0..wire.len() {
            let prefix = &wire[..cut];
            let buffered = RequestView::parse(prefix, &l);
            prop_assert!(
                matches!(buffered, Ok(None)),
                "RequestView::parse answered {:?} for a {}-byte prefix of a {}-byte request",
                buffered,
                cut,
                wire.len()
            );
            let streamed = parse_request(prefix, &l);
            prop_assert!(
                matches!(streamed, Ok(None)),
                "parse_request answered {:?} for a {}-byte prefix of a {}-byte request",
                streamed,
                cut,
                wire.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 4. A chunked body decodes identically however its wire is split across reads.
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct ChunkedCase {
    head: Vec<u8>,
    body_wire: Vec<u8>,
    payload: Vec<u8>,
    steps: Vec<usize>,
}

fn chunked_case_strategy() -> impl Strategy<Value = ChunkedCase> {
    prop::collection::vec(1usize..=64, 0..=8)
        .prop_flat_map(|sizes| {
            let total: usize = sizes.iter().sum();
            (
                Just(sizes),
                prop::collection::vec(any::<u8>(), total..=total),
                prop::collection::vec(1usize..=23, 0..=64),
                any::<bool>(),
            )
        })
        .prop_map(|(sizes, payload, steps, with_trailer)| {
            let mut body_wire = Vec::new();
            let mut offset = 0;
            for (i, size) in sizes.iter().enumerate() {
                if i % 3 == 0 {
                    body_wire.extend_from_slice(format!("{size:x};n={i}\r\n").as_bytes());
                } else {
                    body_wire.extend_from_slice(format!("{size:x}\r\n").as_bytes());
                }
                body_wire.extend_from_slice(&payload[offset..offset + size]);
                body_wire.extend_from_slice(b"\r\n");
                offset += size;
            }
            body_wire.extend_from_slice(b"0\r\n");
            if with_trailer {
                body_wire.extend_from_slice(b"X-Sum: 1\r\n");
            }
            body_wire.extend_from_slice(b"\r\n");

            let head =
                b"POST /upload HTTP/1.1\r\nHost: h.test\r\nTransfer-Encoding: chunked\r\n\r\n"
                    .to_vec();
            ChunkedCase {
                head,
                body_wire,
                payload,
                steps,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_chunked_body_decodes_the_same_however_its_wire_is_split(case in chunked_case_strategy()) {
        let l = limits();
        let mut wire = case.head.clone();
        wire.extend_from_slice(&case.body_wire);

        // The buffered entry point: one decode of the whole message.
        let (_, body, consumed) =
            parse_request(&wire, &l).expect("generated chunked request must parse").expect("must be complete");
        prop_assert_eq!(consumed, wire.len());
        prop_assert_eq!(body.as_deref(), Some(case.payload.as_slice()));

        let head = parse_request_head(&wire, &l).expect("head must parse").expect("head must be complete");
        prop_assert_eq!(head.framing, BodyFraming::Chunked);

        // The streaming entry point: the same body arrives in arbitrary steps, with the
        // decoder asked to hand back everything it has validated after each one.
        let mut decoder = ChunkedBodyDecoder::new();
        let mut out = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut offset = 0;
        for step in &case.steps {
            let end = (offset + step).min(case.body_wire.len());
            buf.extend_from_slice(&case.body_wire[offset..end]);
            offset = end;
            let consumed = decoder
                .stream_feed(&buf, &l, &mut out)
                .expect("a prefix of a valid body is never malformed");
            prop_assert!(consumed <= buf.len());
            buf.drain(..consumed);
        }
        // Deliver whatever is left in one piece: the steps only decide *where* the reads fall.
        buf.extend_from_slice(&case.body_wire[offset..]);
        let consumed = decoder
            .stream_feed(&buf, &l, &mut out)
            .expect("the tail of a valid body is never malformed");
        buf.drain(..consumed);

        prop_assert!(decoder.is_complete(), "the body must be complete once every wire byte arrived");
        prop_assert!(buf.is_empty(), "a completed body must consume every wire byte");
        prop_assert_eq!(out, case.payload);
    }
}
