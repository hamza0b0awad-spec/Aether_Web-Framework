//! # Aether Security Crate

pub mod auth;
pub mod constant_time;
pub mod cookie;
pub mod csp;
pub mod csrf;
pub mod jwt;
pub mod smuggling;

pub use auth::{BearerAuth, BearerError, Principal};
pub use constant_time::constant_time_eq;
pub use cookie::{CookieNameError, CookieSigner, SignedCookie};
pub use csp::{CspNonce, InvalidNonce, NONCE_BYTES, csp_hash_sha256};
pub use csrf::{CsrfConfig, CsrfError, CsrfSameSite, CsrfToken, CsrfTokens};
pub use jwt::{JwtValidationOptions, JwtValidator, VerifiedJwt};
pub use smuggling::audit_request_desync;
