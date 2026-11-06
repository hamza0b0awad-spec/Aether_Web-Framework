//! # Bearer authentication middleware
//!
//! [`require_bearer`] turns an [`aether_security::BearerAuth`] into one chain entry: the request
//! must carry exactly one well-formed `Authorization: Bearer <token>` whose signature and claims
//! verify, and the verified [`Principal`](aether_security::Principal) is attached to the request
//! before `next` runs. Every failure — no field, another scheme, a malformed credential, a
//! forged, expired or wrongly labelled token — is the same `401` with
//! `WWW-Authenticate: Bearer error="invalid_token"`.
//!
//! ```rust,no_run
//! use aether::prelude::*;
//!
//! let auth = BearerAuth::new(b"a shared secret", JwtValidationOptions::default());
//! let app = AetherApp::new();
//! let app = app.route_sync_with(Method::Get, "/me", require_bearer(&auth), |ctx: RequestContext| {
//!     match ctx.extension::<Principal>() {
//!         Some(principal) => Response::text_dynamic(
//!             principal.subject.clone().unwrap_or_else(|| "unnamed".into()),
//!         ),
//!         None => Response::status(500, "authenticated request carried no principal"),
//!     }
//! });
//! # let _ = app;
//! ```
//!
//! It composes like any other middleware: `route_sync_with`, `route_async_with`, `ws_with` and
//! [`RouteGroup::use_middleware`](crate::app::RouteGroup::use_middleware) all accept it, and a
//! route registered without it is public. Registration order is the chain order, so an
//! app-wide `use_middleware(require_bearer(..))` protects the routes registered after it and not
//! the ones already registered — the per-route form is what leaves public routes alone.
//!
//! Authentication is not authorisation: a route that admits every valid token and decides by
//! `Principal::claims` (a `role`, a `scope`, a tenant) checks those claims itself. Replay is not
//! mitigated — a stolen token is good until `exp` — and revocation, refresh tokens and
//! asymmetric algorithms are out of scope; the issuer signs, this server verifies.

use aether_http::response::Response;
use aether_security::BearerAuth;

use crate::app::{BoxFutureResponse, Next, RequestContext};

/// Middleware that admits a request only with a valid bearer token and attaches its
/// [`Principal`](aether_security::Principal).
///
/// The check reads every `Authorization` field line, so a request carrying two credentials is
/// refused rather than resolved by whichever layer looks first, and it runs before the handler,
/// so nothing on the route executes for an unauthenticated caller. Because the token is a header
/// and not a cookie, a browser does not attach it to a cross-site request on its own.
///
/// ```
/// use aether::prelude::*;
///
/// let auth = BearerAuth::new(b"secret", JwtValidationOptions::default());
/// let app = AetherApp::new();
/// let app = app.get("/public", |_| "open");
/// let app = app.route_sync_with(Method::Get, "/private", require_bearer(&auth), |_| "closed");
/// # let _ = app;
/// ```
pub fn require_bearer(
    auth: &BearerAuth,
) -> impl Fn(RequestContext, Next) -> BoxFutureResponse + Send + Sync + 'static {
    let auth = auth.clone();
    move |mut ctx, next| match auth.authenticate(&ctx.header_values("authorization")) {
        Ok(principal) => {
            let owned_heap = principal.owned_heap();
            ctx.insert_extension_sized(principal, owned_heap);
            next(ctx)
        }
        Err(_) => Box::pin(std::future::ready(refusal())) as BoxFutureResponse,
    }
}

/// The one answer an unauthenticated request gets, whatever was wrong with its credential.
fn refusal() -> Response {
    Response::status(401, "Unauthorized")
        .header("WWW-Authenticate", "Bearer error=\"invalid_token\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_the_scheme_without_a_reason() {
        let response = refusal();
        assert_eq!(response.status, 401);
        assert_eq!(
            response.headers,
            vec![("WWW-Authenticate", "Bearer error=\"invalid_token\"".into())]
        );
        assert!(response.body.is_empty());
    }
}
