//! Content-Security-Policy nonces and hashes.
//!
//! A CSP without `'unsafe-inline'` blocks every inline `<script>`, which is the point of writing
//! one — and also the reason an application that has one inline bootstrap block ends up putting
//! `'unsafe-inline'` back and losing the protection entirely. The two mechanisms here are how a
//! policy stays strict while a specific inline block is allowed:
//!
//! * [`CspNonce`] is a fresh random value per response. The response names it in the header
//!   (`script-src 'nonce-…'`) and the same value is written on the tags the policy should allow
//!   (`<script nonce="…">`). A static, or a value reused across responses, is what an attacker's
//!   injected markup would also carry, so the freshness is the protection, not the length.
//! * [`csp_hash_sha256`] pins one exact source text instead, for markup the application cannot put
//!   an attribute on.
//!
//! Both render a CSP source expression — the quoting is part of the returned string, because a
//! value without its quotes is not a source expression and is silently ignored by the browser.

use std::fmt;
use std::sync::Arc;

use ring::rand::{SecureRandom, SystemRandom};

use crate::jwt::encode_base64url;

/// Random bytes behind one nonce.
///
/// 16 bytes is the floor at which the value is unpredictable for its whole lifetime by a wide
/// margin (CSP's own examples use 16+), and it still encodes to 22 characters, which costs
/// nothing to emit per response. Length is not what makes a nonce safe — per-response freshness
/// is — so this is not a knob.
pub const NONCE_BYTES: usize = 16;

/// A nonce was handed out with a shape no CSP parser accepts.
///
/// Only reachable through [`CspNonce::from_token`]; [`CspNonce::generate`] cannot produce one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNonce;

impl fmt::Display for InvalidNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "a CSP nonce must be 1-32 base64-value characters (A-Z a-z 0-9 + / - _), per CSP3 §2.3",
        )
    }
}

impl std::error::Error for InvalidNonce {}

/// A per-response CSP nonce.
///
/// Cloning is a refcount bump, not a copy of the token: the serving core hands the same value to
/// the request context (where a handler renders it onto its tags) and to the response (where the
/// header names it), and those two must be the *same* value or the policy blocks the page's own
/// script.
#[derive(Clone, PartialEq, Eq)]
pub struct CspNonce {
    /// `Arc<str>` rather than `String`: one allocation per response, shared by every holder.
    token: Arc<str>,
}

impl CspNonce {
    /// Draw a fresh nonce from the OS entropy source.
    ///
    /// Fails only when the platform's RNG refuses to produce bytes. That is a state in which no
    /// secure token of any kind can be minted, so callers must treat it as a refusal to serve
    /// rather than fall back to a predictable value — a nonce a client can guess is worse than an
    /// error page, because it makes the policy look enforced while allowing injected markup.
    pub fn generate() -> Result<Self, InvalidNonce> {
        let mut bytes = [0u8; NONCE_BYTES];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| InvalidNonce)?;
        Ok(Self {
            token: Arc::from(encode_base64url(&bytes)),
        })
    }

    /// Adopt a token the caller already generated.
    ///
    /// `token` is the bare base64-value, not the quoted CSP source expression. It is checked
    /// against CSP3 §2.3's `base64-value` production because an unchecked token lands inside a
    /// quoted string in the header: a `'` or a space in it would end the source expression and
    /// turn the rest of the token into a *new* source the policy never meant to allow.
    pub fn from_token(token: &str) -> Result<Self, InvalidNonce> {
        let ok = !token.is_empty()
            && token.len() <= 32
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_'));
        if !ok {
            return Err(InvalidNonce);
        }
        Ok(Self {
            token: Arc::from(token),
        })
    }

    /// The bare token, for a `nonce="…"` attribute.
    pub fn as_str(&self) -> &str {
        &self.token
    }

    /// The token as a CSP source expression: `'nonce-…'`, quotes included.
    pub fn source_expression(&self) -> String {
        format!("'nonce-{}'", self.token)
    }
}

impl fmt::Debug for CspNonce {
    /// Prints the token in full, deliberately. A nonce is published on the wire in the same
    /// response it protects and is worthless after that response, so hiding it here would only
    /// make a failing test harder to read. It is not, and must not become, an authentication
    /// secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CspNonce").field(&self.token).finish()
    }
}

impl fmt::Display for CspNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.token)
    }
}

/// The CSP source expression that allows exactly this inline source text: `'sha256-…'`.
///
/// `source` is the text between the tags, byte for byte — a policy hash covers the exact bytes the
/// browser hashes, so a re-indented or re-encoded snippet produces a different, non-matching hash.
/// This is the alternative to a nonce for markup the application cannot add an attribute to.
pub fn csp_hash_sha256(source: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, source.as_bytes());
    format!("'sha256-{}'", encode_base64url(digest.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_nonce_is_a_base64_value_and_not_the_padding_form() {
        let nonce = CspNonce::generate().expect("the platform RNG answers");
        assert_eq!(
            nonce.as_str().len(),
            22,
            "16 bytes encode to 22 base64url chars"
        );
        assert!(
            CspNonce::from_token(nonce.as_str()).is_ok(),
            "a generated token must satisfy the check its own constructor applies"
        );
    }

    #[test]
    fn two_nonces_are_never_the_same() {
        // The whole protection is freshness: a static value would be in every attacker's payload.
        let a = CspNonce::generate().expect("rng");
        let b = CspNonce::generate().expect("rng");
        assert_ne!(a, b);
    }

    #[test]
    fn a_clone_is_the_same_token_even_after_the_original_is_dropped() {
        let nonce = CspNonce::generate().expect("rng");
        let shared = nonce.clone();
        let token = nonce.as_str().to_string();
        drop(nonce);
        assert_eq!(shared.as_str(), token);
    }

    #[test]
    fn the_source_expression_carries_the_quotes_the_header_needs() {
        let nonce = CspNonce::from_token("abc123").expect("a valid token");
        assert_eq!(nonce.source_expression(), "'nonce-abc123'");
        assert_eq!(nonce.to_string(), "abc123");
    }

    #[test]
    fn a_token_that_would_break_out_of_its_quotes_is_refused() {
        // Each of these would either end the source expression early or inject a second source.
        for bad in [
            "",
            "a'b",
            "a b",
            "'nonce-a'",
            "a;b",
            "a\"b",
            &"a".repeat(33),
        ] {
            assert!(
                CspNonce::from_token(bad).is_err(),
                "{bad:?} must not be accepted as a nonce"
            );
        }
        assert!(
            CspNonce::from_token("a_b-c+d/e").is_ok(),
            "the base64-value alphabet"
        );
    }

    #[test]
    fn the_hash_matches_the_known_sha256_of_the_empty_source() {
        // SHA-256("") = 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU= in standard base64; CSP wants
        // base64-value, so the padding goes and +/ become -_.
        assert_eq!(
            csp_hash_sha256(""),
            "'sha256-47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU'"
        );
    }

    #[test]
    fn the_hash_covers_the_exact_bytes_of_the_source() {
        assert_ne!(csp_hash_sha256("alert(1)"), csp_hash_sha256("alert(1) "));
        assert_eq!(csp_hash_sha256("alert(1)"), csp_hash_sha256("alert(1)"));
    }
}
