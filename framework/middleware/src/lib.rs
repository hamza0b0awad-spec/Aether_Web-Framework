//! # Aether Middleware Pipeline Crate

pub mod compression;
pub mod cors;
pub mod forwarded;
pub mod host_validation;
pub mod logger;
pub mod rate_limit;
pub mod security;

pub use compression::{CompressionConfig, CompressionMiddleware};
pub use cors::{CorsConfig, CorsMiddleware};
pub use forwarded::{ClientAddr, TrustedProxies};
pub use host_validation::HostValidation;
pub use logger::LoggerMiddleware;
pub use rate_limit::{RateLimitConfig, RateLimitCounters, RateLimitDecision, RateLimiter};
pub use security::{CSP_NONCE_PLACEHOLDER, SecurityHeadersMiddleware};
