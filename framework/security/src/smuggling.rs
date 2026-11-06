//! # HTTP Request Smuggling & Desync Detection (RFC 9112 Section 6.3)
//!
//! Refuses a header set that two HTTP/1 parsers could frame differently: a `Content-Length`
//! alongside a `Transfer-Encoding`, a repeated framing field, a `Content-Length` that is not a
//! decimal number, or a `Transfer-Encoding` that is not exactly the one `chunked` coding.
//!
//! These are the server's own framing rules, so `Ok(())` names a head the server would accept
//! on framing grounds. Two limits follow from the input. This is an audit helper, not the
//! enforcement point: the parser makes the same refusals on every request whatever a caller
//! does with this result. And it sees headers only, so it cannot refuse `Transfer-Encoding` on
//! an HTTP/1.0 request; the chunked *body* is decoded and validated while parsing, not here.

use aether_core::error::AetherError;
use aether_http::header::HeaderMap;

pub fn audit_request_desync(headers: &HeaderMap<'_>) -> Result<(), AetherError> {
    let mut content_length: Option<&[u8]> = None;
    let mut transfer_encoding: Option<&[u8]> = None;
    let mut transfer_encoding_fields = 0usize;

    for h in headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                // Dual Content-Length headers -> Smuggling attack!
                return Err(AetherError::MalformedRequest);
            }
            content_length = Some(h.value);
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding_fields += 1;
            transfer_encoding = Some(h.value);
        }
    }

    // CL.TE or TE.CL conflict: one parser reads a body the other does not.
    if content_length.is_some() && transfer_encoding.is_some() {
        return Err(AetherError::MalformedRequest);
    }

    if let Some(value) = content_length {
        let value = ows_str(value)?;
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AetherError::MalformedRequest);
        }
        // A length no `usize` can hold cannot delimit a body, and the parser refuses it too.
        value
            .parse::<usize>()
            .map_err(|_| AetherError::MalformedRequest)?;
    }

    if let Some(value) = transfer_encoding {
        // Duplicates are refused rather than merged: an upstream that keeps the first field
        // and a downstream that keeps the last disagree about the framing the moment the two
        // differ, and even two `chunked` codings cannot both apply.
        if transfer_encoding_fields > 1 {
            return Err(AetherError::MalformedRequest);
        }
        // Exactly one coding, and it must be `chunked`: a list (`gzip, chunked`) or any other
        // coding declares a body this server cannot frame.
        if !ows_str(value)?.eq_ignore_ascii_case("chunked") {
            return Err(AetherError::MalformedRequest);
        }
    }

    Ok(())
}

/// A field value as text with its optional whitespace removed, or a malformed verdict when it
/// is not UTF-8.
fn ows_str(value: &[u8]) -> Result<&str, AetherError> {
    let value = std::str::from_utf8(value).map_err(|_| AetherError::MalformedRequest)?;
    Ok(value.trim_matches(|c| c == ' ' || c == '\t'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_http::header::HeaderView;

    fn audit(pairs: &[(&'static str, &'static [u8])]) -> Result<(), AetherError> {
        let headers = HeaderMap::from_vec(
            pairs
                .iter()
                .map(|(name, value)| HeaderView { name, value })
                .collect(),
        );
        audit_request_desync(&headers)
    }

    fn malformed(result: Result<(), AetherError>) -> bool {
        matches!(result, Err(AetherError::MalformedRequest))
    }

    /// A head whose framing is unambiguous stays accepted, however the one coding it names
    /// is spelled: the parser applies the same case-insensitive, OWS-trimmed rule.
    #[test]
    fn a_head_with_one_unambiguous_framing_is_accepted() {
        assert!(audit(&[]).is_ok());
        assert!(audit(&[("host", b"localhost"), ("accept", b"*/*")]).is_ok());
        assert!(audit(&[("Content-Length", b"0")]).is_ok());
        assert!(audit(&[("content-length", b"5")]).is_ok());
        assert!(audit(&[("content-length", b"\t 5 ")]).is_ok());
        assert!(audit(&[("content-length", b"0005")]).is_ok());
        assert!(audit(&[("transfer-encoding", b"chunked")]).is_ok());
        assert!(audit(&[("Transfer-Encoding", b"Chunked")]).is_ok());
        assert!(audit(&[("transfer-encoding", b"\tCHUNKED ")]).is_ok());
    }

    /// Whichever field comes first, a length plus a transfer coding is the CL.TE / TE.CL
    /// disagreement this audit exists to flag.
    #[test]
    fn a_length_and_a_transfer_coding_together_are_refused() {
        assert!(malformed(audit(&[
            ("content-length", &b"5"[..]),
            ("transfer-encoding", &b"chunked"[..]),
        ])));
        assert!(malformed(audit(&[
            ("transfer-encoding", &b"chunked"[..]),
            ("content-length", &b"5"[..]),
        ])));
    }

    #[test]
    fn a_repeated_content_length_is_refused() {
        assert!(malformed(audit(&[
            ("content-length", &b"5"[..]),
            ("content-length", &b"5"[..]),
        ])));
        // The name is compared case-insensitively, so a case change is not an escape.
        assert!(malformed(audit(&[
            ("Content-Length", &b"5"[..]),
            ("content-length", &b"6"[..]),
        ])));
    }

    /// The parser refuses repeated `Transfer-Encoding` fields rather than merging them, and
    /// two `chunked` codings are invalid whether merged or not: `chunked` is the framing
    /// itself, so a second one has nowhere to go. An audit that accepted this head would
    /// return `Ok(())` for a head the server answers with a 400.
    #[test]
    fn a_repeated_transfer_encoding_is_refused() {
        assert!(malformed(audit(&[
            ("transfer-encoding", &b"chunked"[..]),
            ("transfer-encoding", &b"chunked"[..]),
        ])));
        assert!(malformed(audit(&[
            ("transfer-encoding", &b"gzip"[..]),
            ("transfer-encoding", &b"chunked"[..]),
        ])));
    }

    /// Only exactly one `chunked` coding is frameable. A list, another coding, an empty
    /// value, or bytes that are not text all declare a framing this server cannot read.
    #[test]
    fn a_coding_other_than_a_single_chunked_is_refused() {
        for value in [
            &b"gzip"[..],
            &b"identity"[..],
            &b"deflate"[..],
            &b"chunked, gzip"[..],
            &b"gzip, chunked"[..],
            &b"chunked, chunked"[..],
            &b"chunked;q=1"[..],
            &b""[..],
            &b"chunked gzip"[..],
            &b"\xFF"[..],
        ] {
            assert!(
                malformed(audit(&[("transfer-encoding", value)])),
                "{value:?} is not exactly one chunked coding"
            );
        }
    }

    /// A length the parser cannot turn into a number cannot delimit a body either, so the
    /// audit refuses it for the same reason the parser does. The overflow case is included:
    /// the digits are well formed but no `usize` can hold them.
    #[test]
    fn a_content_length_that_is_not_a_decimal_number_is_refused() {
        for value in [
            &b"abc"[..],
            &b"5, 5"[..],
            &b"+5"[..],
            &b"-1"[..],
            &b" "[..],
            &b"0x10"[..],
            &b"5 5"[..],
            &b"\xFF"[..],
            &b"99999999999999999999999999"[..],
        ] {
            assert!(
                malformed(audit(&[("content-length", value)])),
                "{value:?} is not a decimal length"
            );
        }
    }
}
