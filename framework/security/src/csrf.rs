//! # Signed double-submit CSRF tokens
//!
//! A state-changing request is protected only if an attacker who can make the victim's browser
//! send it cannot know what the request must contain. [`CsrfTokens`] issues a `nonce.mac` token in
//! a cookie on a safe response; an unsafe request has to echo the same token in a header. A
//! cross-origin page can neither read the cookie (same-origin policy) nor set a custom header
//! without a CORS preflight this server only answers for configured origins.
//!
//! The MAC is what separates this from plain double-submit. An attacker who controls any sibling
//! subdomain can plant a cookie for the parent domain with a value it chose, so "cookie equals
//! header" alone proves nothing; a planted nonce cannot carry a MAC the attacker cannot compute.
//! Both comparisons — cookie against header, and MAC against nonce — are constant time.
//!
//! The secret is a constructor argument, never a hidden per-process default: a deployment with
//! more than one node has to pass the same secret everywhere, and an API that generated one
//! silently would fail only across nodes.

use std::fmt;

use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};

use crate::constant_time::constant_time_eq;
use crate::jwt::{decode_base64url, encode_base64url};

/// Nonce length in bytes: 128 bits, encoded as 22 base64url characters.
const NONCE_BYTES: usize = 16;
/// HMAC-SHA256 tag length, and the canonical base64url length of that tag.
const MAC_BYTES: usize = 32;
const MAC_CHARS: usize = 43;

/// The `SameSite` value the CSRF cookie is issued with.
///
/// `Lax` is the default because it removes the simplest cross-site form submits, but it is *not*
/// the protection: a top-level `POST` still carries the cookie under `Lax`, which is why the
/// header token exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrfSameSite {
    Lax,
    Strict,
    None,
}

impl CsrfSameSite {
    /// The registered spelling the `Set-Cookie` attribute takes (RFC 6265bis §5.6).
    pub fn as_str(self) -> &'static str {
        match self {
            CsrfSameSite::Lax => "Lax",
            CsrfSameSite::Strict => "Strict",
            CsrfSameSite::None => "None",
        }
    }
}

/// How the CSRF cookie is named, attributed and carved out.
///
/// `Default` is the configuration that is safe to ship: the cookie is `Secure`, `SameSite=Lax`,
/// `Path=/`, carries no `Domain`, and is not `HttpOnly` because a script on the page has to read
/// it to put the same value in the header.
#[derive(Debug, Clone)]
pub struct CsrfConfig {
    /// The cookie the token is issued in. A `__Host-` or `__Secure-` prefix is accepted and
    /// requires `secure`.
    pub cookie_name: String,
    /// The request field an unsafe request must echo the token in. Anything but a `token` is
    /// refused at construction; the stored name is ASCII-lowercase.
    pub header_name: String,
    /// The `Secure` attribute, on by default. Switch it off only for plaintext development.
    pub secure: bool,
    /// The `SameSite` attribute; `None` requires `secure`, as browsers do.
    pub same_site: CsrfSameSite,
    /// `Max-Age` in seconds. `None` is a session cookie. The window starts at issuance and is not
    /// extended while the cookie keeps being reused, so a value has to outlast a page's rendering
    /// plus the user's time to submit its form.
    pub max_age: Option<u64>,
    /// Raw path prefixes that are exempt — third-party webhooks, and only those. A prefix must
    /// start with `/` and may not be `/` itself. Matching is on the encoded request path, so
    /// `/webhooks/` covers the children of that path and `/webhooks` also covers `/webhooksX`.
    /// An exempted route that authenticates by cookie is unprotected again; the list is for
    /// routes that do not.
    pub exempt_prefixes: Vec<String>,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            cookie_name: "aether_csrf".to_owned(),
            header_name: "x-csrf-token".to_owned(),
            secure: true,
            same_site: CsrfSameSite::Lax,
            max_age: None,
            exempt_prefixes: Vec::new(),
        }
    }
}

/// No OS entropy, so no token could be issued for this response.
///
/// A safe response without a token is still a correct response — an existing cookie keeps
/// working — so the serving layer skips the `Set-Cookie` rather than failing the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsrfError;

impl fmt::Display for CsrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no entropy for a CSRF token")
    }
}

impl std::error::Error for CsrfError {}

/// The token a safe response hands to the page it renders.
///
/// Inserted into the request extensions by the facade's CSRF layer: the value is what the cookie
/// carries — an existing valid one, or the one this response is about to set. A handler that
/// renders a form reads it here instead of parsing the request's own `Cookie` field, so the page
/// and the cookie cannot disagree even when the response replaces a planted cookie.
#[derive(Clone)]
pub struct CsrfToken(String);

impl fmt::Debug for CsrfToken {
    /// The token is a live credential for this response: a form renders it, and a `{:?}` that
    /// carried it into a log would hand the value to everyone who can read the log. Debug exists
    /// so the type can sit in a struct that derives one; [`Self::as_str`] is the deliberate way to
    /// read the text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CsrfToken(<redacted>)")
    }
}

impl CsrfToken {
    /// Wrap a token this layer issued.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token text, exactly what the cookie carries.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The heap this token points at, for extension pricing.
    pub fn owned_heap(&self) -> usize {
        self.0.len()
    }
}

/// Issues and verifies signed double-submit tokens under one shared secret.
///
/// Construction validates the configuration and panics on a value that would silently protect
/// less than it says: a name that is not a token, a `__Host-`/`__Secure-` name with `secure`
/// off, `SameSite=None` without `secure`, an empty or `/` exemption, an empty secret. The same
/// secret must be passed on every node, or a token issued by one is refused by the next.
///
/// The secret is copied into the HMAC key and the type implements no `Debug`, so a configuration
/// struct that derives one cannot print it by accident; the compiled assertion below fails the
/// doc tests if a `Debug` impl is ever added.
///
/// ```compile_fail,E0277
/// use aether_security::{CsrfConfig, CsrfTokens};
/// let csrf = CsrfTokens::new(b"a shared secret", CsrfConfig::default());
/// let _ = format!("{csrf:?}");
/// ```
pub struct CsrfTokens {
    key: hmac::Key,
    rng: SystemRandom,
    config: CsrfConfig,
}

impl CsrfTokens {
    /// Build from the shared secret and the cookie/header policy.
    ///
    /// The secret should be at least 32 random bytes from the deployment's configuration; it is
    /// copied into the HMAC key and never kept here. Panics on the invalid configurations listed
    /// on the type, so a typo in a security setting fails at startup rather than at the first
    /// request.
    pub fn new(secret: impl AsRef<[u8]>, config: CsrfConfig) -> Self {
        let secret = secret.as_ref();
        assert!(!secret.is_empty(), "the CSRF secret must not be empty");
        assert!(
            is_token(&config.cookie_name),
            "CSRF cookie name {:?} is not a token",
            config.cookie_name
        );
        assert!(
            is_token(&config.header_name),
            "CSRF header name {:?} is not a token",
            config.header_name
        );
        if config.cookie_name.starts_with("__Host-") || config.cookie_name.starts_with("__Secure-")
        {
            assert!(
                config.secure,
                "CSRF cookie name {:?} requires secure=true; a browser drops the cookie otherwise",
                config.cookie_name
            );
        }
        if config.same_site == CsrfSameSite::None {
            assert!(
                config.secure,
                "SameSite=None requires secure=true; a browser drops the cookie otherwise"
            );
        }
        for prefix in &config.exempt_prefixes {
            assert!(
                prefix.starts_with('/') && prefix != "/",
                "CSRF exempt prefix {prefix:?} must start with '/' and must not be '/' \
                 (that would exempt every path)"
            );
        }
        let config = CsrfConfig {
            header_name: config.header_name.to_ascii_lowercase(),
            ..config
        };
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, secret),
            rng: SystemRandom::new(),
            config,
        }
    }

    /// The cookie the token is issued in.
    pub fn cookie_name(&self) -> &str {
        &self.config.cookie_name
    }

    /// The request field the token must be echoed in, ASCII-lowercase.
    pub fn header_name(&self) -> &str {
        &self.config.header_name
    }

    /// The `Secure` attribute the cookie is issued with.
    pub fn secure(&self) -> bool {
        self.config.secure
    }

    /// The `SameSite` attribute, in its registered spelling.
    pub fn same_site(&self) -> &'static str {
        self.config.same_site.as_str()
    }

    /// The `Max-Age` in seconds, or `None` for a session cookie.
    pub fn max_age(&self) -> Option<u64> {
        self.config.max_age
    }

    /// True when `path` is under one of the configured exempt prefixes.
    pub fn is_exempt(&self, path: &str) -> bool {
        self.config
            .exempt_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix.as_str()))
    }

    /// Draw a fresh `nonce.mac` token for a safe response.
    ///
    /// The nonce is 128 random bits; the MAC is HMAC-SHA256 over the nonce's base64url text, so
    /// the token is two base64url strings with a `.` between them and nothing in it needs
    /// escaping in a cookie or a header.
    pub fn issue(&self) -> Result<String, CsrfError> {
        let mut nonce = [0u8; NONCE_BYTES];
        self.rng.fill(&mut nonce).map_err(|_| CsrfError)?;
        let nonce = encode_base64url(&nonce);
        let mac = hmac::sign(&self.key, nonce.as_bytes());
        Ok(format!("{nonce}.{}", encode_base64url(mac.as_ref())))
    }

    /// The one value of the configured cookie, or `None` when it is absent or carried twice.
    ///
    /// Two cookies with the same name are a cookie-tossing attempt (a sibling subdomain can set a
    /// parent-domain cookie): whichever one a layer picked first, the other layer could pick the
    /// other, so neither is trusted. The refusal costs the attacker only a denial of service it
    /// could already cause by phishing a cookie onto the domain.
    pub fn cookie_value<'a>(&self, cookies: &'a [String]) -> Option<&'a str> {
        let mut found = None;
        for line in cookies {
            for pair in line.split(';') {
                let Some((name, value)) = pair.trim().split_once('=') else {
                    continue;
                };
                if name.trim() != self.config.cookie_name {
                    continue;
                }
                if found.is_some() {
                    return None;
                }
                found = Some(value.trim());
            }
        }
        found
    }

    /// The one value of the configured cookie when its MAC verifies, or `None`.
    ///
    /// This is the test for "the request already carries a token this server issued": a safe
    /// response reuses it rather than rotating, so two tabs opened before either response keep
    /// the same cookie and the token the user's open form carries. An attacker-planted cookie
    /// fails the MAC here and is replaced by a fresh token instead of being trusted.
    pub fn valid_cookie<'a>(&self, cookies: &'a [String]) -> Option<&'a str> {
        self.cookie_value(cookies)
            .filter(|token| self.mac_verifies(token))
    }

    /// True when a request carries exactly one header line whose value equals the cookie and
    /// whose token verifies under this secret.
    ///
    /// The field is treated as a singleton: two lines are the same ambiguity as two cookies, and
    /// a value that differs from the cookie in any byte — including only the last one — is
    /// refused by the constant-time comparison.
    pub fn checks(&self, cookies: &[String], headers: &[String]) -> bool {
        let Some(cookie) = self.valid_cookie(cookies) else {
            return false;
        };
        let [header] = headers else {
            return false;
        };
        constant_time_eq(cookie.as_bytes(), header.trim().as_bytes())
    }

    /// True when `cookie` and `header` are the same token and that token's MAC verifies.
    ///
    /// The equality is checked in constant time and the tag through `ring`'s constant-time
    /// verify; a caller that compared them itself would leak the matching prefix.
    pub fn verifies(&self, cookie: &str, header: &str) -> bool {
        constant_time_eq(cookie.as_bytes(), header.as_bytes()) && self.mac_verifies(cookie)
    }

    fn mac_verifies(&self, token: &str) -> bool {
        let Some((nonce, mac)) = token.split_once('.') else {
            return false;
        };
        if nonce.is_empty() || mac.len() != MAC_CHARS || mac.contains('=') {
            return false;
        }
        let Ok(mac) = decode_base64url(mac) else {
            return false;
        };
        if mac.len() != MAC_BYTES {
            return false;
        }
        hmac::verify(&self.key, nonce.as_bytes(), &mac).is_ok()
    }
}

/// RFC 9110 §5.6.2 `token`: the character set of a cookie name and of a field name.
pub(crate) fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|b| {
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
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"csrf-test-secret-0123456789abcdef";

    fn tokens() -> CsrfTokens {
        CsrfTokens::new(SECRET, CsrfConfig::default())
    }

    fn cookie(value: &str) -> Vec<String> {
        vec![format!("aether_csrf={value}")]
    }

    fn header(value: &str) -> Vec<String> {
        vec![value.to_owned()]
    }

    fn is_b64url(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
    }

    #[test]
    fn an_issued_token_is_two_base64url_halves() {
        let token = tokens().issue().expect("OS entropy");
        let (nonce, mac) = token.split_once('.').expect("one separator");
        assert_eq!(nonce.len(), 22, "16 bytes encode to 22 characters");
        assert_eq!(mac.len(), MAC_CHARS);
        assert!(nonce.bytes().all(is_b64url), "{nonce}");
        assert!(mac.bytes().all(is_b64url), "{mac}");
    }

    #[test]
    fn a_fresh_token_verifies_once_it_is_echoed() {
        let tokens = tokens();
        let token = tokens.issue().expect("OS entropy");
        assert!(tokens.checks(&cookie(&token), &header(&token)));
    }

    #[test]
    fn every_issue_draws_a_new_nonce() {
        let tokens = tokens();
        assert_ne!(
            tokens.issue().expect("OS entropy"),
            tokens.issue().expect("OS entropy")
        );
    }

    #[test]
    fn a_missing_or_ambiguous_pair_is_refused() {
        let tokens = tokens();
        let token = tokens.issue().expect("OS entropy");

        assert!(!tokens.checks(&[], &header(&token)), "no cookie");
        assert!(!tokens.checks(&cookie(&token), &[]), "no header");
        assert!(
            !tokens.checks(&cookie(&token), &[token.clone(), token.clone()]),
            "two header lines"
        );
        assert!(
            !tokens.checks(
                &[
                    format!("aether_csrf={token}"),
                    format!("aether_csrf={token}")
                ],
                &header(&token)
            ),
            "two cookie lines"
        );
        assert!(
            !tokens.checks(
                &[format!("aether_csrf={token}; aether_csrf={token}")],
                &header(&token)
            ),
            "the same name twice in one line"
        );
    }

    #[test]
    fn a_mismatched_pair_is_refused_including_a_one_byte_near_miss() {
        let tokens = tokens();
        let token = tokens.issue().expect("OS entropy");
        let other = tokens.issue().expect("OS entropy");
        assert!(
            !tokens.checks(&cookie(&other), &header(&token)),
            "the cookie and header name different tokens"
        );

        let mut near = token.clone();
        let last = near.pop().expect("a non-empty token");
        near.push(if last == 'A' { 'B' } else { 'A' });
        assert!(
            !tokens.checks(&cookie(&token), &header(&near)),
            "a one-byte difference on the header side"
        );
        assert!(
            !tokens.checks(&cookie(&near), &header(&near)),
            "a one-byte difference that also breaks the MAC"
        );
    }

    #[test]
    fn a_token_signed_by_another_secret_is_refused() {
        let mine = tokens();
        let theirs = CsrfTokens::new(b"a-different-secret", CsrfConfig::default());
        let planted = theirs.issue().expect("OS entropy");
        assert!(
            !mine.checks(&cookie(&planted), &header(&planted)),
            "cookie tossing: the planted token fails the MAC"
        );
    }

    #[test]
    fn only_a_cookie_this_secret_signed_is_reused() {
        let tokens = tokens();
        let token = tokens.issue().expect("OS entropy");
        assert_eq!(tokens.valid_cookie(&cookie(&token)), Some(token.as_str()));

        let theirs = CsrfTokens::new(b"a-different-secret", CsrfConfig::default());
        let planted = theirs.issue().expect("OS entropy");
        assert_eq!(
            tokens.valid_cookie(&cookie(&planted)),
            None,
            "a planted cookie must be replaced, not trusted"
        );
        assert_eq!(tokens.valid_cookie(&cookie("nonsense")), None);
        assert_eq!(tokens.valid_cookie(&[]), None);
        assert_eq!(
            tokens.valid_cookie(&[format!("aether_csrf={token}; aether_csrf={token}")]),
            None
        );
        assert_eq!(
            tokens.valid_cookie(&[format!("other={token}")]),
            None,
            "a different cookie name holds nothing for this layer"
        );
    }

    #[test]
    fn the_extension_hands_the_token_over_byte_for_byte() {
        let token = tokens().issue().expect("OS entropy");
        let extension = CsrfToken::new(token.clone());
        assert_eq!(extension.as_str(), token);
        assert_eq!(extension.owned_heap(), token.len());
    }

    #[test]
    fn a_token_that_is_not_nonce_dot_mac_is_refused() {
        let tokens = tokens();
        let forged_mac = "A".repeat(MAC_CHARS);
        let long_mac = "A".repeat(MAC_CHARS + 1);
        for token in [
            "attacker.attacker".to_owned(),
            "no-separator".to_owned(),
            ".mac-only".to_owned(),
            "nonce.".to_owned(),
            format!("nonce.{forged_mac}"),
            format!("nonce.{long_mac}"),
            format!("nonce.{forged_mac}="),
        ] {
            assert!(
                !tokens.checks(&cookie(&token), &header(&token)),
                "{token:?} must not verify"
            );
        }
    }

    #[test]
    fn the_stored_names_are_the_safe_defaults() {
        let tokens = tokens();
        assert_eq!(tokens.cookie_name(), "aether_csrf");
        assert_eq!(tokens.header_name(), "x-csrf-token");
        assert!(tokens.secure());
        assert_eq!(tokens.same_site(), "Lax");
        assert_eq!(tokens.max_age(), None);
    }

    #[test]
    fn the_header_name_is_stored_lowercase() {
        let tokens = CsrfTokens::new(
            SECRET,
            CsrfConfig {
                header_name: "X-CSRF-Token".to_owned(),
                ..CsrfConfig::default()
            },
        );
        assert_eq!(tokens.header_name(), "x-csrf-token");
    }

    #[test]
    fn a_host_prefixed_cookie_is_accepted_when_secure() {
        let tokens = CsrfTokens::new(
            SECRET,
            CsrfConfig {
                cookie_name: "__Host-csrf".to_owned(),
                ..CsrfConfig::default()
            },
        );
        assert_eq!(tokens.cookie_name(), "__Host-csrf");
        assert!(tokens.secure());
    }

    #[test]
    fn exemptions_match_the_raw_prefix() {
        let tokens = CsrfTokens::new(
            SECRET,
            CsrfConfig {
                exempt_prefixes: vec!["/webhooks/".to_owned(), "/login".to_owned()],
                ..CsrfConfig::default()
            },
        );
        assert!(tokens.is_exempt("/webhooks/stripe"));
        assert!(tokens.is_exempt("/login"));
        assert!(
            tokens.is_exempt("/login/again"),
            "a prefix is not an exact match"
        );
        assert!(
            !tokens.is_exempt("/webhooks"),
            "the trailing slash is part of the prefix"
        );
        assert!(!tokens.is_exempt("/admin"));
    }

    #[test]
    #[should_panic(expected = "is not a token")]
    fn a_cookie_name_that_is_not_a_token_is_refused() {
        CsrfTokens::new(
            SECRET,
            CsrfConfig {
                cookie_name: "with space".to_owned(),
                ..CsrfConfig::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "is not a token")]
    fn a_header_name_that_is_not_a_token_is_refused() {
        CsrfTokens::new(
            SECRET,
            CsrfConfig {
                header_name: "x-csrf;token".to_owned(),
                ..CsrfConfig::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "requires secure=true")]
    fn a_host_prefixed_cookie_requires_secure() {
        CsrfTokens::new(
            SECRET,
            CsrfConfig {
                cookie_name: "__Host-csrf".to_owned(),
                secure: false,
                ..CsrfConfig::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "requires secure=true")]
    fn same_site_none_requires_secure() {
        CsrfTokens::new(
            SECRET,
            CsrfConfig {
                same_site: CsrfSameSite::None,
                secure: false,
                ..CsrfConfig::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "must not be '/'")]
    fn a_root_exemption_is_refused() {
        CsrfTokens::new(
            SECRET,
            CsrfConfig {
                exempt_prefixes: vec!["/".to_owned()],
                ..CsrfConfig::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "must start with '/'")]
    fn an_exemption_without_a_slash_is_refused() {
        CsrfTokens::new(
            SECRET,
            CsrfConfig {
                exempt_prefixes: vec!["webhooks".to_owned()],
                ..CsrfConfig::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn an_empty_secret_is_refused() {
        CsrfTokens::new(b"", CsrfConfig::default());
    }
}
