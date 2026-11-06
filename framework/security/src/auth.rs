//! # Bearer authentication
//!
//! Turns an `Authorization: Bearer <token>` field line into a [`Principal`]: the request named
//! a token, the [`JwtValidator`] verified its signature and the configured claims, and what is
//! left is who the caller claims to be. What this layer answers is *authentication*; a handler
//! that needs *authorisation* reads [`Principal::claims`] and decides for itself.

use std::fmt;
use std::sync::Arc;

use crate::jwt::{JwtValidationOptions, JwtValidator};

/// What a verified token says about the caller of *this* request.
///
/// Inserted into the request extensions by the facade's bearer layer; a handler reads it with
/// `ctx.extension::<Principal>()`. Nothing here is trusted until [`BearerAuth::authenticate`]
/// has returned it — the type is deliberately not constructible from an unverified token by the
/// framework, only by the caller that already holds the claims.
#[derive(Debug, Clone)]
pub struct Principal {
    /// The `sub` claim, when it is a JSON string. A numeric `sub` is legal in the wild and is
    /// left in [`claims`](Self::claims) rather than stringified here, so the caller sees what
    /// the issuer actually signed.
    pub subject: Option<String>,
    /// Every verified claim, for a handler that authorises on more than the subject.
    pub claims: serde_json::Value,
}

impl Principal {
    /// The heap this principal points at, for `RequestContext::insert_extension_sized` pricing.
    ///
    /// The request's memory registration sees the container and the inline size of the value;
    /// a verified token's claims are heap it cannot count. The figure is the bytes of every
    /// string plus the nodes of every array and object, which is the memory a holder of this
    /// value keeps alive — not a serialised length.
    pub fn owned_heap(&self) -> usize {
        fn value_heap(value: &serde_json::Value) -> usize {
            match value {
                serde_json::Value::String(text) => text.len(),
                serde_json::Value::Array(items) => {
                    items.capacity() * std::mem::size_of::<serde_json::Value>()
                        + items.iter().map(value_heap).sum::<usize>()
                }
                serde_json::Value::Object(map) => {
                    map.len()
                        * (std::mem::size_of::<String>() + std::mem::size_of::<serde_json::Value>())
                        + map
                            .iter()
                            .map(|(key, item)| key.len() + value_heap(item))
                            .sum::<usize>()
                }
                _ => 0,
            }
        }

        self.subject.as_ref().map_or(0, String::len) + value_heap(&self.claims)
    }
}

/// The one refusal a bearer check reports.
///
/// It carries no reason on purpose. A caller that could tell "expired" from "forged" would be
/// tempted to tell the client, and that tells an attacker which guess was closer. The reason is
/// visible in the validator's own error when it is called directly; the layer built on this type
/// answers every failure the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerError;

impl fmt::Display for BearerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid bearer credentials")
    }
}

impl std::error::Error for BearerError {}

/// Verifies `Authorization: Bearer <token>` against a shared [`JwtValidator`].
///
/// It wraps the validator and therefore the key, and it implements no `Debug`: the compiled
/// assertion below fails the doc tests if one is ever added.
///
/// ```compile_fail,E0277
/// use aether_security::{BearerAuth, JwtValidationOptions};
/// let auth = BearerAuth::new(b"a shared secret", JwtValidationOptions::default());
/// let _ = format!("{auth:?}");
/// ```
#[derive(Clone)]
pub struct BearerAuth {
    validator: Arc<JwtValidator>,
    options: JwtValidationOptions,
}

impl BearerAuth {
    /// Build from a shared secret and the claim checks the issuer's tokens must satisfy.
    ///
    /// Panics on an empty secret, through [`JwtValidator::new`]: a zero-length HMAC key
    /// verifies every token minted under it, so it is a forgery surface rather than a
    /// hardening gap.
    pub fn new(secret: impl AsRef<[u8]>, options: JwtValidationOptions) -> Self {
        Self {
            validator: Arc::new(JwtValidator::new(secret.as_ref())),
            options,
        }
    }

    /// Build around a validator that other layers already share.
    pub fn with_validator(validator: Arc<JwtValidator>, options: JwtValidationOptions) -> Self {
        Self { validator, options }
    }

    /// Authenticate one request from **every** `Authorization` field line it carried.
    ///
    /// The field is defined as a singleton (RFC 9110 §11.6.2), so zero lines and two lines are
    /// both refusals: with two, a layer in front and this one could disagree about which
    /// credential governs the request. An empty value, a non-`Bearer` scheme, a credential that
    /// is not a `token68`, a bad signature, a failed claim check and an `alg` mismatch all
    /// produce the same [`BearerError`].
    pub fn authenticate(&self, authorization: &[&str]) -> Result<Principal, BearerError> {
        let token = single_bearer_token(authorization)?;
        let verified = self
            .validator
            .verify(token, &self.options)
            .map_err(|_| BearerError)?;
        let claims = verified.claims;
        let subject = claims
            .get("sub")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Ok(Principal { subject, claims })
    }
}

/// The token from exactly one `Authorization` line.
fn single_bearer_token<'a>(authorization: &'a [&'a str]) -> Result<&'a str, BearerError> {
    let [value] = authorization else {
        return Err(BearerError);
    };

    // RFC 9110 §11.1: `credentials = auth-scheme [ 1*SP ( token68 / #auth-param ) ]`. Only a
    // space separates the scheme, and the scheme is case-insensitive (§11.1).
    let (scheme, rest) = value.split_once(' ').ok_or(BearerError)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(BearerError);
    }
    let token = rest.trim_start_matches(' ');
    if !is_token68(token) {
        return Err(BearerError);
    }
    Ok(token)
}

/// RFC 7235 §2.1 `token68`: the JWT alphabet, with any `=` padding only at the end.
fn is_token68(token: &str) -> bool {
    let bytes = token.as_bytes();
    let body_len = bytes.iter().position(|&b| b == b'=').unwrap_or(bytes.len());
    body_len > 0
        && bytes[..body_len].iter().all(|&b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
        })
        && bytes[body_len..].iter().all(|&b| b == b'=')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::encode_base64url;
    use ring::hmac;

    const SECRET: &[u8] = b"bearer-auth-test-secret-0123456789";
    const NOW: u64 = 1_000_000;

    fn mint(secret: &[u8], header: &str, payload: &str) -> String {
        let signed = format!(
            "{}.{}",
            encode_base64url(header.as_bytes()),
            encode_base64url(payload.as_bytes())
        );
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
        let tag = hmac::sign(&key, signed.as_bytes());
        format!("{signed}.{}", encode_base64url(tag.as_ref()))
    }

    fn auth() -> BearerAuth {
        BearerAuth::new(
            SECRET,
            JwtValidationOptions {
                now_unix: Some(NOW),
                ..JwtValidationOptions::default()
            },
        )
    }

    #[test]
    fn a_verified_token_yields_its_subject_and_claims() {
        let token = mint(
            SECRET,
            r#"{"alg":"HS256"}"#,
            r#"{"sub":"alice","role":"admin","exp":2000000}"#,
        );
        let principal = auth().authenticate(&[&format!("Bearer {token}")]).unwrap();
        assert_eq!(principal.subject.as_deref(), Some("alice"));
        assert_eq!(principal.claims["role"], "admin");
    }

    #[test]
    fn the_scheme_is_case_insensitive_and_extra_spaces_are_a_separator() {
        let token = mint(SECRET, r#"{"alg":"HS256"}"#, r#"{"sub":"a"}"#);
        for credential in [
            format!("Bearer {token}"),
            format!("bearer {token}"),
            format!("BEARER {token}"),
            format!("Bearer   {token}"),
        ] {
            assert!(
                auth().authenticate(&[&credential]).is_ok(),
                "credential {credential} must authenticate"
            );
        }
    }

    #[test]
    fn every_malformed_credential_is_the_same_refusal() {
        let good = mint(SECRET, r#"{"alg":"HS256"}"#, r#"{"sub":"a"}"#);
        let other = mint(SECRET, r#"{"alg":"HS256"}"#, r#"{"sub":"b"}"#);
        let cases: Vec<Vec<String>> = vec![
            vec![],
            vec![String::new()],
            vec!["Bearer".into()],
            vec!["Bearer ".into()],
            vec![format!("Basic {good}")],
            vec![format!("Token {good}")],
            vec![format!(" {good}")],
            vec![format!("Bearer {good} {other}")],
            vec![format!("Bearer {good},{other}")],
            vec!["Bearer ...".into()],
            vec!["Bearer a=b".into()],
            // Two credentials are not a singleton field: whichever layer picked would decide.
            vec![format!("Bearer {good}"), format!("Bearer {other}")],
        ];
        for case in cases {
            let borrowed: Vec<&str> = case.iter().map(String::as_str).collect();
            assert_eq!(
                auth().authenticate(&borrowed).err(),
                Some(BearerError),
                "credential {case:?} must be refused"
            );
        }
    }

    #[test]
    fn a_forged_expired_or_mis_labelled_token_is_refused() {
        let now = NOW;
        let payload = format!(r#"{{"sub":"a","exp":{}}}"#, now + 100);
        let forged = mint(b"another-secret", r#"{"alg":"HS256"}"#, &payload);
        let expired = mint(
            SECRET,
            r#"{"alg":"HS256"}"#,
            &format!(r#"{{"sub":"a","exp":{}}}"#, now - 3600),
        );
        let not_yet = mint(
            SECRET,
            r#"{"alg":"HS256"}"#,
            &format!(r#"{{"sub":"a","nbf":{}}}"#, now + 3600),
        );
        // Correctly HMAC-signed, but the header names an algorithm the validator was not told to
        // accept — the confusion attack the `expected_alg` pin exists for.
        let none = mint(SECRET, r#"{"alg":"none"}"#, &payload);
        let swapped = mint(SECRET, r#"{"alg":"HS512"}"#, &payload);

        for token in [forged, expired, not_yet, none, swapped] {
            let credential = format!("Bearer {token}");
            assert_eq!(
                auth().authenticate(&[&credential]).err(),
                Some(BearerError),
                "token {token} must be refused"
            );
        }
    }

    #[test]
    fn the_credential_must_be_a_token68() {
        for token in ["eyJhbGciOiJIUzI1NiJ9.e30.abc-_DEF", "abc", "a=", "a=="] {
            assert!(is_token68(token), "{token} is a token68");
        }
        for token in ["", " a", "a b", "a,b", "a=b", "=a", "a b=c", "a\tb"] {
            assert!(!is_token68(token), "{token:?} is not a token68");
        }
    }

    #[test]
    fn the_issuer_and_audience_pins_apply_to_the_bearer_layer() {
        let auth = BearerAuth::new(
            SECRET,
            JwtValidationOptions {
                issuer: Some("aether".into()),
                audience: Some("api".into()),
                now_unix: Some(NOW),
                ..JwtValidationOptions::default()
            },
        );
        let good = mint(
            SECRET,
            r#"{"alg":"HS256"}"#,
            r#"{"sub":"a","iss":"aether","aud":["web","api"]}"#,
        );
        assert!(auth.authenticate(&[&format!("Bearer {good}")]).is_ok());

        let wrong_issuer = mint(
            SECRET,
            r#"{"alg":"HS256"}"#,
            r#"{"sub":"a","iss":"evil","aud":"api"}"#,
        );
        assert_eq!(
            auth.authenticate(&[&format!("Bearer {wrong_issuer}")])
                .err(),
            Some(BearerError)
        );
    }

    #[test]
    fn the_owned_heap_counts_the_subject_and_nested_claims() {
        let flat = Principal {
            subject: Some("alice".into()),
            claims: serde_json::json!({"role": "admin"}),
        };
        assert!(flat.owned_heap() >= "alice".len() + "role".len() + "admin".len());

        let nested = Principal {
            subject: None,
            claims: serde_json::json!({"scopes": ["read", "write"], "meta": {"tenant": "acme"}}),
        };
        assert!(nested.owned_heap() > flat.owned_heap() - "alice".len());

        let numbers = Principal {
            subject: None,
            claims: serde_json::json!({"exp": 1}),
        };
        assert!(numbers.owned_heap() < flat.owned_heap());
    }

    #[test]
    fn a_shared_validator_can_back_the_layer() {
        let validator = Arc::new(JwtValidator::new(SECRET));
        let auth = BearerAuth::with_validator(
            validator,
            JwtValidationOptions {
                now_unix: Some(NOW),
                ..JwtValidationOptions::default()
            },
        );
        let token = mint(SECRET, r#"{"alg":"HS256"}"#, r#"{"sub":"a"}"#);
        assert!(auth.authenticate(&[&format!("Bearer {token}")]).is_ok());
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn the_bearer_layer_refuses_an_empty_secret() {
        // The layer has no key handling of its own: this pins that construction reaches the
        // validator's refusal instead of building a validator anyone can sign for.
        BearerAuth::new(b"", JwtValidationOptions::default());
    }
}
