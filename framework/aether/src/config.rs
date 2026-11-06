//! # Aether Production Configuration Engine
//!
//! Type-safe configuration management with environment variable loading,
//! strict validation, and secret sanitization for logs and telemetry.

use std::env;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AetherConfig {
    pub host: String,
    pub port: u16,
    pub environment: String,
    /// The advertised body ceiling, in **decoded** bytes — the number the parser enforces against
    /// the body it has reassembled. HTTP/1.1 applies one bound before it: a request may not exceed
    /// `max_header_bytes + max_body_bytes` on the **wire**, since the framing and the decoded copy
    /// are both resident until the request completes. That is what a chunked upload runs into
    /// first: measured on the defaults, one-byte chunks are refused at 350 880 payload bytes
    /// (2 105 347 wire) although the parser would take 2 097 152 decoded.
    /// [`aether_http::limits::ServerLimits::max_body_bytes`] states the arithmetic and names the
    /// test that pins it.
    pub max_body_bytes: usize,
    /// The ceiling for a body that a route registered with [`AetherApp::route_stream`] consumes as
    /// it arrives. It is a transfer bound, not a memory one: the memory a streamed body occupies is
    /// the connection's registration, re-priced per chunk. Leave it well above
    /// [`AetherConfig::max_body_bytes`] — a streaming route exists to carry the uploads the buffered
    /// path cannot hold, and a declared `Content-Length` above the buffered ceiling is not a reason
    /// to refuse one; this field is the only declared-length bound a streamed request meets.
    ///
    /// [`AetherApp::route_stream`]: crate::AetherApp::route_stream
    pub max_stream_body_bytes: usize,
    /// The request-head budget, and the first term of the wire budget above. HTTP/1 answers an
    /// over-budget section with 431.
    pub max_header_bytes: usize,
    /// Maximum number of header *field lines* one request may carry. HTTP/1 answers an
    /// over-budget section with 431; the HTTP/2 frontend counts the same budget itself, because
    /// `h2` bounds header bytes and not lines. The count is ordinary header lines only — HTTP/1
    /// counts its `Host` line, HTTP/2 does not carry one — and `from_env` refuses `0`, because at
    /// `0` every HTTP/1 request that sends any header at all is refused.
    pub max_headers_count: usize,
    /// The value advertised as `SETTINGS_MAX_CONCURRENT_STREAMS`, and the bound the server
    /// enforces against it: streams over the cap are reset with `REFUSED_STREAM`, not dropped.
    /// The scheduler's per-stream memory budget is a second and independent bound — a stream it
    /// cannot cover is answered `503`, not reset.
    pub max_concurrent_streams: u32,
    pub workers: usize,
    pub max_request_time: std::time::Duration,
    /// The total budget for receiving one request, head and body together, on both frontends.
    /// Independent of [`AetherConfig::max_request_time`], which bounds handler execution: a slow
    /// upload is admitted or refused by this number alone. Over-budget requests receive `408`.
    pub max_receive_time: std::time::Duration,
    pub max_response_time: std::time::Duration,
    /// Maximum idle time a keep-alive connection may wait between requests
    /// before the server closes it.
    pub keep_alive_timeout: std::time::Duration,
    /// Emit the framework's `X-Request-ID` header on every response. Disabling drops the
    /// header; the id itself is still generated, because a handler can read it from the
    /// request context and the error reporter names it, so disabling saves the field, not
    /// the allocation.
    pub x_request_id: bool,
}

impl Default for AetherConfig {
    fn default() -> Self {
        // Read from the server's own limits rather than restated as literals: the two bounds were
        // once only settable on `ServerLimits`, and a duplicated default here could have drifted
        // from the value the parser and the h2 frontend actually enforce.
        let limits = aether_http::limits::ServerLimits::default();
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
            environment: "development".to_string(),
            max_body_bytes: 2 * 1024 * 1024, // 2MB
            max_stream_body_bytes: limits.max_stream_body_bytes,
            max_header_bytes: 8 * 1024, // 8KB
            max_headers_count: limits.max_headers_count,
            max_concurrent_streams: limits.max_concurrent_streams,
            workers: num_cpus::get().max(1),
            max_request_time: Duration::from_secs(30),
            max_receive_time: Duration::from_secs(30),
            max_response_time: Duration::from_secs(10),
            keep_alive_timeout: Duration::from_secs(60),
            x_request_id: true,
        }
    }
}

impl AetherConfig {
    /// Loads configuration automatically from environment variables with sensible defaults.
    pub fn from_env() -> Result<Self, String> {
        let mut cfg = Self::default();

        if let Ok(host) = env::var("AETHER_HOST") {
            cfg.host = host;
        }

        if let Ok(port_str) = env::var("AETHER_PORT") {
            cfg.port = port_str.parse::<u16>().map_err(|_| {
                format!(
                    "Invalid AETHER_PORT value '{}': Must be a valid port (1-65535)",
                    port_str
                )
            })?;
        }

        if let Ok(env_str) = env::var("AETHER_ENV") {
            cfg.environment = env_str;
        }

        if let Ok(body_str) = env::var("AETHER_MAX_BODY_BYTES") {
            cfg.max_body_bytes = body_str
                .parse::<usize>()
                .map_err(|_| format!("Invalid AETHER_MAX_BODY_BYTES: '{}'", body_str))?;
        }

        if let Ok(stream_str) = env::var("AETHER_MAX_STREAM_BODY_BYTES") {
            let bytes = stream_str
                .parse::<usize>()
                .map_err(|_| format!("Invalid AETHER_MAX_STREAM_BODY_BYTES: '{}'", stream_str))?;
            if bytes == 0 {
                return Err(
                    "Invalid AETHER_MAX_STREAM_BODY_BYTES: '0' (a streaming route could not read a body)"
                        .to_string(),
                );
            }
            cfg.max_stream_body_bytes = bytes;
        }

        // These four were configurable as struct fields but silently ignored by `from_env`,
        // which made the documented environment interface a lie.
        if let Ok(header_str) = env::var("AETHER_MAX_HEADER_BYTES") {
            cfg.max_header_bytes = header_str
                .parse::<usize>()
                .map_err(|_| format!("Invalid AETHER_MAX_HEADER_BYTES: '{}'", header_str))?;
        }

        if let Ok(workers_str) = env::var("AETHER_WORKERS") {
            cfg.workers = workers_str
                .parse::<usize>()
                .map_err(|_| format!("Invalid AETHER_WORKERS: '{}'", workers_str))?;
        }

        if let Ok(secs_str) = env::var("AETHER_MAX_REQUEST_TIME_SECS") {
            let secs = secs_str
                .parse::<u64>()
                .map_err(|_| format!("Invalid AETHER_MAX_REQUEST_TIME_SECS: '{}'", secs_str))?;
            cfg.max_request_time = Duration::from_secs(secs);
        }

        if let Ok(secs_str) = env::var("AETHER_MAX_RECEIVE_TIME_SECS") {
            let secs = secs_str
                .parse::<u64>()
                .map_err(|_| format!("Invalid AETHER_MAX_RECEIVE_TIME_SECS: '{}'", secs_str))?;
            if secs == 0 {
                return Err(
                    "Invalid AETHER_MAX_RECEIVE_TIME_SECS: '0' (no request could finish arriving)"
                        .to_string(),
                );
            }
            cfg.max_receive_time = Duration::from_secs(secs);
        }

        if let Ok(secs_str) = env::var("AETHER_MAX_RESPONSE_TIME_SECS") {
            let secs = secs_str
                .parse::<u64>()
                .map_err(|_| format!("Invalid AETHER_MAX_RESPONSE_TIME_SECS: '{}'", secs_str))?;
            cfg.max_response_time = Duration::from_secs(secs);
        }

        if let Ok(secs_str) = env::var("AETHER_KEEP_ALIVE_TIMEOUT_SECS") {
            let secs = secs_str
                .parse::<u64>()
                .map_err(|_| format!("Invalid AETHER_KEEP_ALIVE_TIMEOUT_SECS: '{}'", secs_str))?;
            cfg.keep_alive_timeout = Duration::from_secs(secs);
        }

        // The two bounds the HTTP frontends enforce. A request whose `Host` line alone crosses
        // `AETHER_MAX_HEADERS_COUNT` would fail every request, and a stream cap of zero would
        // advertise a connection that refuses everything it is offered, so `0` is rejected here
        // rather than passed down to be discovered at the first request.
        if let Ok(count_str) = env::var("AETHER_MAX_HEADERS_COUNT") {
            let count = count_str
                .parse::<usize>()
                .map_err(|_| format!("Invalid AETHER_MAX_HEADERS_COUNT: '{}'", count_str))?;
            if count == 0 {
                return Err("Invalid AETHER_MAX_HEADERS_COUNT: '0' (every request carries at least one header line)".to_string());
            }
            cfg.max_headers_count = count;
        }

        if let Ok(streams_str) = env::var("AETHER_MAX_CONCURRENT_STREAMS") {
            let streams = streams_str
                .parse::<u32>()
                .map_err(|_| format!("Invalid AETHER_MAX_CONCURRENT_STREAMS: '{}'", streams_str))?;
            if streams == 0 {
                return Err(
                    "Invalid AETHER_MAX_CONCURRENT_STREAMS: '0' (advertises a connection that refuses every stream)"
                        .to_string(),
                );
            }
            cfg.max_concurrent_streams = streams;
        }

        Ok(cfg)
    }

    pub fn is_production(&self) -> bool {
        self.environment.eq_ignore_ascii_case("production")
            || self.environment.eq_ignore_ascii_case("prod")
    }

    /// `host:port` suitable for `TcpListener::bind`. IPv6 literals are bracketed, which the
    /// naive `format!("{}:{}")` did not do — `::1:3000` is not a parseable socket address.
    pub fn listen_addr(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl fmt::Display for AetherConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AetherConfig(env={}, addr={}, max_body={}B, workers={})",
            self.environment,
            self.listen_addr(),
            self.max_body_bytes,
            self.workers
        )
    }
}

// Fallback if num_cpus is not included
mod num_cpus {
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
}
