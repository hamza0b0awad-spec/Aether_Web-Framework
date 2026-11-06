//! # Aether Web Framework
//!
//! High-performance Rust web framework with a Tokio multi-threaded runtime, zero-copy
//! request parsing, chunked and Content-Length body framing, and rich developer diagnostics.
//!
//! HTTP/1.1 and HTTP/2 on one listener, over plaintext and over TLS.
//! [`transport::establish`] decides a connection's framing from its first bytes: a TLS record is
//! terminated with rustls ([`tls`]) and ALPN then chooses HTTP/2 (`h2`) or
//! HTTP/1.1; on plaintext a connection is served as HTTP/1.1 unless it opens with the HTTP/2
//! preface and [`AetherApp::enable_h2c`] opted that listener in, which is RFC 9113 §3.3
//! *prior knowledge* — the deprecated `Upgrade: h2c` exchange is deliberately not served. A
//! WebSocket upgrade is an HTTP/1.1 mechanism, so it works over plaintext and TLS but a WebSocket
//! route reached over HTTP/2 answers `400` (RFC 9113 §8.1). HTTP/3 (QUIC) is not implemented.
//! Call [`AetherApp::require_tls`] to refuse plaintext instead of serving it.
//!
//! Request bodies are copied into owned buffers before handlers run (the parser itself borrows
//! from the connection buffer). `aether_core::RequestArena` is available for request-scoped
//! allocation but is not used by the default server path.

pub mod app;
pub mod auth;
mod body_pump;
pub mod config;
mod cookie;
pub mod diagnostics;
pub mod extensions;
pub mod extract;
pub mod handler;
pub(crate) mod http2;
pub mod request_body;
pub mod scheduling;
pub(crate) mod serve;
mod static_dir;
pub mod test_client;
pub mod tls;
pub mod transport;

pub use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
pub use aether_core::error::AetherError;
pub use aether_http::header;
pub use aether_http::method::Method;
pub use aether_http::multipart::{MultipartError, MultipartForm, Part};
pub use aether_http::response::{BodyFraming, BodySource, BodyStream, Chunk, Response};
pub use aether_http::static_files::{generate_etag, mime_from_extension, serve_static};
pub use aether_middleware::{
    CSP_NONCE_PLACEHOLDER, ClientAddr, CompressionConfig, CompressionMiddleware, CorsConfig,
    CorsMiddleware, LoggerMiddleware, SecurityHeadersMiddleware, TrustedProxies,
};
pub use aether_openapi as openapi;
pub use aether_scheduler::{
    NewJob, PolicyKind, ResourceConfig as SchedulingResourceConfig,
    SchedulerMetrics as SchedulingMetrics, SchedulingConfig, Work as SchedWork,
};
pub use aether_security as security;
pub use aether_security::{
    BearerAuth, BearerError, CookieNameError, CookieSigner, CspNonce, CsrfConfig, CsrfError,
    CsrfSameSite, CsrfToken, CsrfTokens, InvalidNonce, JwtValidationOptions, JwtValidator,
    Principal, SignedCookie, VerifiedJwt, audit_request_desync, constant_time_eq, csp_hash_sha256,
};
pub use aether_websocket as websocket;
pub use aether_websocket::{Message as WsMessage, WebSocket, WebSocketLimits, WsError};
pub use app::{
    AetherApp, AetherServer, AppStateContainer, IntoResponse, IntoResponseError, MiddlewareEntry,
    Next, RequestContext, RouteGroup, Router, WsHandlerFn,
};
pub use auth::require_bearer;
pub use config::AetherConfig;
pub use diagnostics::{ErrorReport, ReporterAlreadySet, set_error_reporter};
pub use extensions::Extensions;
pub use extract::{Extension, Form, FromRequest, FromRequestParts, Json, Path, Query, State};
pub use handler::Handler;
pub use request_body::BodyError;
pub use scheduling::{AdmissionGuard, AdmitFailure, QueueDiag, SchedulingLayer, SliceProgress};
pub use static_dir::StaticDirOptions;
pub use test_client::{TestClient, TestRequest, TestResponse};
pub use tls::{TlsAcceptor, TlsAcceptorBuilder, TlsCertSet, TlsError, TlsIdentity};
pub use transport::Stream;

pub mod prelude {
    pub use crate::app::{
        AetherApp, AetherServer, AppStateContainer, IntoResponse, IntoResponseError,
        MiddlewareEntry, Next, RequestContext, RouteGroup, Router, WsHandlerFn,
    };
    pub use crate::auth::require_bearer;
    pub use crate::config::AetherConfig;
    pub use crate::extensions::Extensions;
    pub use crate::extract::{
        Extension, Form, FromRequest, FromRequestParts, Json, Path, Query, State,
    };
    pub use crate::handler::Handler;
    pub use crate::request_body::BodyError;
    pub use crate::static_dir::StaticDirOptions;
    pub use crate::test_client::{TestClient, TestRequest, TestResponse};
    pub use crate::tls::{TlsAcceptor, TlsAcceptorBuilder, TlsCertSet, TlsError, TlsIdentity};
    pub use crate::transport::Stream;
    pub use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
    pub use aether_core::error::AetherError;
    pub use aether_http::method::Method;
    pub use aether_http::multipart::{MultipartError, MultipartForm, Part};
    pub use aether_http::response::{BodyFraming, BodySource, BodyStream, Chunk, Response};
    pub use aether_http::static_files::{generate_etag, mime_from_extension, serve_static};
    pub use aether_middleware::{
        CSP_NONCE_PLACEHOLDER, ClientAddr, CompressionConfig, CompressionMiddleware, CorsConfig,
        CorsMiddleware, HostValidation, LoggerMiddleware, RateLimitConfig, RateLimitCounters,
        RateLimitDecision, RateLimiter, SecurityHeadersMiddleware, TrustedProxies,
    };
    pub use aether_security::{
        BearerAuth, BearerError, CookieNameError, CookieSigner, CspNonce, CsrfConfig, CsrfError,
        CsrfSameSite, CsrfToken, CsrfTokens, JwtValidationOptions, JwtValidator, Principal,
        SignedCookie, csp_hash_sha256,
    };
    pub use aether_websocket::{Message as WsMessage, WebSocket, WebSocketLimits, WsError};
}
