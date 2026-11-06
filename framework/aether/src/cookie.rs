//! Signed cookie values, read off the request.
//!
//! [`aether_security::CookieSigner`] does the cryptography; this module adds the one thing a
//! handler cannot do with a `&str`: decide what the request *carried*. A cookie is a repeated,
//! ordered, client-writable field, so "the value under this name" is only well-defined after the
//! duplicates have been counted — and a request that carries a name twice is not asking for the
//! first or the last of them.
//!
//! The setter side deliberately has no helper here: `aether_http::Response::set_cookie` already
//! writes the attributes and escapes what RFC 6265 forbids, and a signed value is base64url on
//! both sides of its one `.`, so nothing needs escaping and the two calls compose without a
//! wrapper. `aether-security` cannot see `Response` anyway — it is the crate `aether-http` is a
//! dependency *of*, not the other way round.
//!
//! ```rust
//! # use aether::prelude::*;
//! # use aether_security::CookieSigner;
//! # fn issued(signer: &CookieSigner) -> Response {
//! let value = signer.value("session", "user-42").expect("a token name");
//! Response::ok().set_cookie("session", &value, true, true, "Lax", Some(3600))
//! # }
//! # let response = issued(&CookieSigner::new(b"at least 32 bytes of deployment secret!!"));
//! # assert!(response.headers.iter().any(|(name, value)| *name == "Set-Cookie"
//! #     && value.starts_with("session=") && value.contains("; Max-Age=3600")));
//! ```

use aether_security::{CookieSigner, SignedCookie};

use crate::app::RequestContext;

impl RequestContext {
    /// The signed cookie `name`, classified before it is verified.
    ///
    /// Returns [`SignedCookie::Present`] only when exactly one `Cookie` field carries this name
    /// (case-sensitive, as RFC 6265 §5.2 stores it) and its MAC verifies for this name, this
    /// secret, and this payload. `Absent` and `Invalid` are not the same decision: the first is a
    /// client that never got a cookie, the second is a client that presented one this server did
    /// not sign — stale after a key rotation, truncated in transit, or planted by whatever can
    /// write the domain. `Ambiguous` covers a name carried more than once, across any number of
    /// `Cookie` lines, which is the cookie-tossing shape [`aether_security::CsrfTokens`] refuses
    /// for the same reason: the two values disagree about who the caller is, and the server has no
    /// way to choose.
    ///
    /// The value is *not* percent-decoded, and it does not need to be — a signed value is
    /// `base64url(payload).base64url(mac)`, characters RFC 6265 lets a cookie carry unchanged, so
    /// what the browser sends back is byte-for-byte what [`CookieSigner::value`] wrote. A cookie
    /// the application set through any other path will simply fail its MAC, which is the point:
    /// this accessor answers for signed cookies, not for the `Cookie` header in general. Use
    /// [`RequestContext::cookie`] when the unsigned value is what you want.
    ///
    /// Nothing here renders an outcome. A handler that answers `401` for all three negatives keeps
    /// the distinction off the wire; leaking "your MAC was right but your name was wrong" is
    /// exactly the oracle `CookieSigner::verify` refuses to give.
    ///
    /// ```rust
    /// # use aether::prelude::*;
    /// # use aether_security::{CookieSigner, SignedCookie};
    /// # fn example(signer: &CookieSigner, req: &RequestContext) -> Response {
    /// match req.signed_cookie(signer, "session") {
    ///     SignedCookie::Present(user_id) => Response::text_dynamic(user_id),
    ///     SignedCookie::Absent => Response::status(401, "Anonymous"),
    ///     SignedCookie::Invalid | SignedCookie::Ambiguous => Response::status(400, "Bad Request"),
    /// }
    /// # }
    /// ```
    pub fn signed_cookie(&self, signer: &CookieSigner, name: &str) -> SignedCookie {
        let mut found: Option<&str> = None;
        for line in self.header_values("cookie") {
            for pair in line.split(';') {
                let Some((key, value)) = pair.trim().split_once('=') else {
                    continue;
                };
                if key.trim() != name {
                    continue;
                }
                if found.is_some() {
                    return SignedCookie::Ambiguous;
                }
                found = Some(value.trim());
            }
        }

        match found {
            None => SignedCookie::Absent,
            Some(value) => match signer.verify(name, value) {
                Some(payload) => SignedCookie::Present(payload),
                None => SignedCookie::Invalid,
            },
        }
    }
}
