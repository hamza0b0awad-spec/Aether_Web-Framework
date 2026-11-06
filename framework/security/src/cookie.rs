//! # Name-bound signed cookie values
//!
//! A cookie is attacker-writable: anything on the request's own domain (a sibling subdomain, a
//! XSS payload, a `Set-Cookie` injected through an open redirect) can choose its value. So a
//! handler that reads a session id, a user id or a role out of a cookie is trusting a byte string
//! the client picked, unless that byte string carries proof it left this server unchanged.
//!
//! [`CookieSigner`] provides that proof: the value a response sets is
//! `base64url(payload).base64url(HMAC-SHA256)`, and a request that carries it back either
//! verifies or does not. The MAC covers the cookie **name** as well as the payload, so a value
//! issued for `theme` cannot be replayed as `role=admin` under a different name — the transplant
//! the CSRF layer defends its own cookie against, but with the namespace of many cookies sharing
//! one key.
//!
//! This is integrity, not confidentiality: the payload is base64url, not encrypted, so anything
//! the handler puts in it is readable by the client. Never sign a secret the browser must not see
//! (see [`Self::value`]'s note), and treat this as the answer to "did I write this byte string",
//! not to "can I hide this byte string".
//!
//! The key is a constructor argument, never a per-process default, for the same reason as
//! [`crate::csrf::CsrfTokens`]: a deployment behind a load balancer has to pass the same secret to
//! every node or a cookie issued by one is refused by the next.
//!
//! [`CookieSigner`] implements no `Debug`, so a configuration struct that derives one cannot print
//! the key by accident.
//!
//! ```compile_fail,E0277
//! use aether_security::CookieSigner;
//! let signer = CookieSigner::new(b"a shared secret of at least 32 bytes!!");
//! let _ = format!("{signer:?}");
//! ```

use std::fmt;

use ring::hmac;

use crate::csrf::is_token;
use crate::jwt::{decode_base64url, encode_base64url};

/// HMAC-SHA256 tag length, and the canonical unpadded base64url length of that tag.
const MAC_BYTES: usize = 32;
const MAC_CHARS: usize = 43;

/// A cookie name that no `Cookie` field could carry: not an RFC 6265 §4.1.1 `token`.
///
/// The name is part of the MAC input and part of the wire, so a name outside the token grammar
/// cannot be both set and read back: [`Response::set_cookie`](aether_http::response::Response::set_cookie)
/// percent-encodes it while a request lookup compares it literally. Refusing it here turns that
/// into a compile-visible error at the call site instead of a cookie that never comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieNameError;

impl fmt::Display for CookieNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cookie name is not an RFC 6265 token")
    }
}

impl std::error::Error for CookieNameError {}

/// What a signed cookie lookup found on the request.
///
/// The four outcomes answer different questions and are deliberately not collapsed: "the client
/// sent nothing" is a redirect-to-login decision, "the client sent something that did not verify"
/// is a forged-or-stale decision (and the one that should clear the cookie), and "the client sent
/// it twice" is an attack attempt that must not resolve by taking the first or the last.
/// `Ambiguous` follows the same rule as [`crate::csrf::CsrfTokens::cookie_value`] for the same
/// reason: sibling-subdomain cookie tossing makes a duplicated name untrustworthy, and picking one
/// of the two is a coin flip the server should not take.
#[derive(Clone, PartialEq, Eq)]
pub enum SignedCookie {
    /// Exactly one cookie with this name, and its MAC verifies for this name and secret.
    Present(String),
    /// No cookie with this name.
    Absent,
    /// A cookie with this name, but nothing readable at the value: wrong MAC, wrong shape, wrong
    /// charset, or payload bytes that are not UTF-8.
    Invalid,
    /// More than one cookie with this name, across any number of `Cookie` lines.
    Ambiguous,
}

impl SignedCookie {
    /// The verified payload, or `None` for the three negative outcomes.
    pub fn payload(&self) -> Option<&str> {
        match self {
            SignedCookie::Present(payload) => Some(payload),
            _ => None,
        }
    }
}

impl fmt::Debug for SignedCookie {
    /// Renders the outcome without the payload: a signed cookie commonly holds a session or user
    /// identifier, and `Debug` output reaches logs and error reports.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignedCookie::Present(_) => f.write_str("Present(<redacted>)"),
            SignedCookie::Absent => f.write_str("Absent"),
            SignedCookie::Invalid => f.write_str("Invalid"),
            SignedCookie::Ambiguous => f.write_str("Ambiguous"),
        }
    }
}

/// Signs and verifies cookie payloads under one shared secret.
///
/// The secret is copied into the HMAC key and never kept here. It should be at least 32 random
/// bytes from the deployment's configuration; an empty secret panics, because a signer that would
/// refuse every cookie it is supposed to accept is a startup bug, not a request-time one.
pub struct CookieSigner {
    key: hmac::Key,
}

impl CookieSigner {
    /// Build from the shared secret every node of the deployment passes.
    ///
    /// Panics on an empty secret.
    pub fn new(secret: impl AsRef<[u8]>) -> Self {
        let secret = secret.as_ref();
        assert!(
            !secret.is_empty(),
            "the cookie signing secret must not be empty"
        );
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, secret),
        }
    }

    /// The `Set-Cookie` value to send for `name` carrying `payload`.
    ///
    /// Both halves are base64url (`A-Za-z0-9-_`, unpadded), so the value is inside RFC 6265's
    /// `cookie-value` grammar already and the attribute encoder has nothing to escape: what the
    /// browser stores is byte-for-byte what [`Self::verify`] reads back. An empty `payload` is
    /// legal and signs the empty string; a name that is not a token is refused.
    ///
    /// `payload` is not encrypted. Anything that must stay away from the client belongs in server
    /// state with an opaque id in the cookie, not here.
    pub fn value(&self, name: &str, payload: &str) -> Result<String, CookieNameError> {
        if !is_token(name) {
            return Err(CookieNameError);
        }
        let encoded = encode_base64url(payload.as_bytes());
        let mac = hmac::sign(&self.key, &self.mac_input(name, &encoded));
        Ok(format!("{encoded}.{}", encode_base64url(mac.as_ref())))
    }

    /// The payload `name` was issued with, or `None` unless this exact name carries this exact
    /// payload under this exact secret.
    ///
    /// Every failure mode answers `None` with the same shape: a truncated value, a padded or
    /// standard-alphabet (`+`/`/`) tag, a tag of the wrong length, a payload that is not UTF-8 and
    /// a valid signature over a different name. Distinguishing them would tell an attacker which
    /// part of the value he got right, and the handler cannot act on the difference anyway.
    ///
    /// The tag goes through `ring`'s `hmac::verify`, which is constant time; a hand-rolled prefix
    /// check would leak the matching bytes.
    pub fn verify(&self, name: &str, presented: &str) -> Option<String> {
        if !is_token(name) {
            return None;
        }
        let (encoded, mac) = presented.split_once('.')?;
        // The `=` and post-decode-length guards below are implied by the 43-character rule: a
        // padded tag is never 43 characters that also decode to 32 bytes. They document the wire.
        if mac.len() != MAC_CHARS || mac.contains('=') {
            return None;
        }
        let mac = decode_base64url(mac).ok()?;
        if mac.len() != MAC_BYTES {
            return None;
        }
        hmac::verify(&self.key, &self.mac_input(name, encoded), &mac).ok()?;
        let payload = decode_base64url(encoded).ok()?;
        String::from_utf8(payload).ok()
    }

    /// `name`, a NUL separator, then the encoded payload.
    ///
    /// The separator is what makes the boundary recoverable: base64url's alphabet sits inside the
    /// token alphabet a cookie name is written in, so a bare `name || payload` concatenation can
    /// be split more than one way, and one cookie's valid value would be another's. NUL is not a
    /// character either of them may contain.
    fn mac_input(&self, name: &str, encoded: &str) -> Vec<u8> {
        let mut input = Vec::with_capacity(name.len() + 1 + encoded.len());
        input.extend_from_slice(name.as_bytes());
        input.push(0);
        input.extend_from_slice(encoded.as_bytes());
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"cookie-test-secret-0123456789abcdef";

    fn signer() -> CookieSigner {
        CookieSigner::new(SECRET)
    }

    fn is_b64url(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
    }

    #[test]
    fn an_issued_value_is_two_base64url_halves() {
        let value = signer().value("session", "user-42").expect("token name");
        let (encoded, mac) = value.split_once('.').expect("one separator");
        assert_eq!(mac.len(), MAC_CHARS);
        assert!(encoded.bytes().all(is_b64url), "{encoded}");
        assert!(mac.bytes().all(is_b64url), "{mac}");
    }

    #[test]
    fn a_signed_payload_round_trips() {
        let signer = signer();
        for payload in [
            "",
            "user-42",
            "sess/with=space;and,comma",
            "عربي-🔑",
            "\n\t",
        ] {
            let value = signer.value("session", payload).expect("token name");
            assert_eq!(signer.verify("session", &value).as_deref(), Some(payload));
        }
    }

    #[test]
    fn a_signed_value_needs_no_percent_encoding() {
        // Whatever the payload, the wire value stays inside `cookie-value`: the base64url halves
        // are what a browser stores, so `Response::set_cookie`'s encoder has nothing to escape and
        // `RequestContext::cookie`'s literal comparison reads the exact bytes back.
        let value = signer()
            .value("session", "a=b; c,d \"\\ é")
            .expect("token name");
        assert!(
            value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
            "{value}"
        );
        assert!(!value.contains('='), "{value}");
        assert!(
            !value
                .bytes()
                .any(|b| matches!(b, b';' | b',' | b'"' | b'\\' | b' ')),
            "{value}"
        );
    }

    #[test]
    fn a_payload_change_invalidates_the_mac() {
        let signer = signer();
        let value = signer.value("session", "user-42").expect("token name");
        let mac = value.split_once('.').expect("separator").1;
        let forged = format!("{}.{}", encode_base64url(b"user-43"), mac);
        assert_eq!(signer.verify("session", &forged), None);
        assert_eq!(signer.verify("session", &value).as_deref(), Some("user-42"));
    }

    #[test]
    fn a_one_bit_mac_change_invalidates() {
        let signer = signer();
        let value = signer.value("session", "user-42").expect("token name");
        let mut flipped: Vec<char> = value.chars().collect();
        let last = flipped.len() - 1;
        flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
        assert_eq!(
            signer.verify("session", &flipped.iter().collect::<String>()),
            None
        );
    }

    #[test]
    fn a_value_issued_for_one_name_is_refused_under_another() {
        // The transplant this whole type exists to stop: read `role=admin` off a cookie the app
        // signed for `theme`, or move a low-privilege session id under a privileged name.
        let signer = signer();
        let admin = signer.value("role", "admin").expect("token name");
        assert_eq!(signer.verify("role", &admin).as_deref(), Some("admin"));
        assert_eq!(signer.verify("theme", &admin), None);
        assert_eq!(signer.verify("Role", &admin), None);
    }

    #[test]
    fn the_mac_input_separates_name_from_payload() {
        // Pinned at the byte level: base64url's alphabet is a subset of the cookie-name token
        // alphabet, so a bare concatenation of the two can be split more than one way. The NUL is
        // not a character a token may contain, which is what makes the boundary recoverable.
        let signer = signer();
        assert_eq!(
            signer.mac_input("session", "YnVzZXI"),
            b"session\0YnVzZXI".to_vec()
        );
        assert_eq!(signer.mac_input("a", ""), b"a\0".to_vec());
    }

    #[test]
    fn a_different_secret_refuses_the_value() {
        let issued = CookieSigner::new(SECRET)
            .value("session", "user-42")
            .expect("token name");
        assert_eq!(
            CookieSigner::new(b"another-secret-entirely-0123456789").verify("session", &issued),
            None
        );
    }

    #[test]
    fn malformed_shapes_are_all_refused_the_same_way() {
        let signer = signer();
        let value = signer.value("session", "user-42").expect("token name");
        let mac = value.split_once('.').expect("separator").1.to_owned();
        let encoded = value.split_once('.').expect("separator").0.to_owned();
        for candidate in [
            String::new(),
            encoded.clone(),
            format!(".{mac}"),
            format!("{encoded}."),
            format!("{encoded}.{mac}.extra"),
            format!("{encoded}.{mac}="),
            format!("{encoded}+{mac}"),
            format!("{encoded}.{}", mac.replace('-', "+")),
            mac.clone(),
            format!("{}.{}", encode_base64url(b"x"), &mac[..8]),
        ] {
            assert_eq!(signer.verify("session", &candidate), None, "{candidate:?}");
        }
    }

    #[test]
    fn a_payload_that_is_not_utf8_is_refused() {
        // Signed here by hand, so the value carries a real MAC over bytes that are not text: a
        // `Vec<u8>` answer would accept it and then push the failure to a `String` boundary.
        let signer = signer();
        let encode_and_sign = |bytes: &[u8]| {
            let encoded = encode_base64url(bytes);
            let mac = hmac::sign(&signer.key, &signer.mac_input("sx", &encoded));
            format!("{encoded}.{}", encode_base64url(mac.as_ref()))
        };
        assert_eq!(signer.verify("sx", &encode_and_sign(&[0xff, 0xfe])), None);
        assert_eq!(
            signer.verify("sx", &encode_and_sign(b"ok")).as_deref(),
            Some("ok"),
            "the same construction over text verifies, so the refusal above is the UTF-8 boundary"
        );
    }

    #[test]
    fn a_non_token_name_is_refused_on_both_sides() {
        let signer = signer();
        for name in ["", "bad name", "a=b", "a;b", "café"] {
            assert_eq!(
                signer.value(name, "x"),
                Err(CookieNameError),
                "set side accepted {name:?}"
            );
            assert_eq!(
                signer.verify(name, "anything.mac"),
                None,
                "read side accepted {name:?}"
            );
        }
    }

    #[test]
    fn an_empty_payload_signs_and_verifies() {
        let signer = signer();
        let value = signer.value("session", "").expect("token name");
        assert!(value.starts_with('.'), "{value}");
        assert_eq!(signer.verify("session", &value).as_deref(), Some(""));
    }

    #[test]
    fn the_signed_halves_are_stable_across_calls() {
        // Unlike a CSRF token, a signed cookie value is not a nonce: the same payload under the
        // same name and key produces the same bytes, which is what lets a response reuse a
        // `Vary`-friendly cache entry and what makes this deterministic enough to test on a wire.
        let signer = signer();
        assert_eq!(
            signer.value("session", "user-42").expect("token name"),
            signer.value("session", "user-42").expect("token name")
        );
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn an_empty_secret_panics() {
        CookieSigner::new("");
    }

    #[test]
    fn debug_never_prints_the_payload() {
        let signed = SignedCookie::Present("session-id-42".to_owned());
        let rendered = format!("{signed:?}");
        assert_eq!(rendered, "Present(<redacted>)");
        assert!(!rendered.contains("session-id-42"));
    }
}
