# Aether Web Framework — Complete Framework Specification (English)

Welcome to the comprehensive, production-grade technical specification and functional documentation for the **Aether Web Framework** in Rust.

Aether is an ultra-high-throughput, zero-copy, memory-safe asynchronous web framework engineered with strict RFC 9112 conformance, deterministic routing, and zero lock-contention hot paths.

---

## 📑 Complete Functional Table of Contents

1. [Architecture & Request Execution Pipeline](#1-architecture--request-execution-pipeline)
2. [Socket Layer & Adaptive Two-Tier TCP Framing](#2-socket-layer--adaptive-two-tier-tcp-framing)
3. [Zero-Copy HTTP/1.1 Parser & RFC 9112 Security Engine](#3-zero-copy-http11-parser--rfc-9112-security-engine)
4. [Deterministic Radix Trie Router & Method Bitmask Matching](#4-deterministic-radix-trie-router--method-bitmask-matching)
5. [Request Context & Memory Architecture (`RequestContext`)](#5-request-context--memory-architecture-requestcontext)
6. [Response Construction & Zero-Allocation Serialization (`Response`)](#6-response-construction--zero-allocation-serialization-response)
7. [Dependency Injection & Shared Application State (`AppStateContainer`)](#7-dependency-injection--shared-application-state-appstatecontainer)
8. [Middleware Subsystems (CORS, Security Headers, Tracing, Logger)](#8-middleware-subsystems-cors-security-headers-tracing-logger)
9. [Resource Limits, Concurrency Semaphores & Slowloris Mitigation (`ServerLimits`)](#9-resource-limits-concurrency-semaphores--slowloris-mitigation-serverlimits)
10. [Error Handling Subsystem (`AetherError`)](#10-error-handling-subsystem-aethererror)
11. [End-to-End Developer Examples](#11-end-to-end-developer-examples)

---

## 1. Architecture & Request Execution Pipeline

Aether executes requests along an explicit, non-blocking asynchronous pipeline designed to minimize cache-misses and eliminate global locking:

```text
 Client TCP Connection
          │
          ▼
┌─────────────────────────────────────────┐
│ 1. Concurrency Semaphore (acquire_owned)│  -> Limits concurrent sockets (Default: 10,000)
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 2. Two-Tier Persistent Read Buffer      │  -> BytesMut accumulation with Read Timeout
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 3. Zero-Copy Incremental Parser         │  -> `parse_request_incremental` (`framework/http/src/parser.rs:748`): RFC 9112 validation & Smuggling defense
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 4. Radix Trie Router ($O(K)$ Depth)     │  -> 64-bit Bitmask method matching + Path Params
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 5. Middleware Pipeline Execution        │  -> CORS / Security Headers / Zero-overhead Logger
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 6. Async Route Handler Invocation       │  -> User application code with RequestContext
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 7. Direct Buffer Serialization          │  -> `write_http1_to` direct to BytesMut writer
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 8. Non-blocking Socket Write & Flush    │  -> Write Timeout applied; RAII buffer cleanup
└─────────────────────────────────────────┘
```

---

## 2. Socket Layer & Adaptive Two-Tier TCP Framing

### The Problem:
TCP is a byte-stream protocol. A single HTTP request may arrive across multiple fragmented TCP packets, or multiple HTTP requests may arrive clustered together in a single read (HTTP Pipelining / Keep-Alive).

### The Aether Solution:
- **Fast-Path (Stack-Read):** Reads into a local 8 KB stack buffer.
- **Accumulator Buffer (`BytesMut`):** If `parse_request_incremental` returns `Ok(None)` (partial request), the bytes are safely preserved in `conn_buf`, and the next socket chunk is awaited (`framework/aether/src/app.rs:2639` for the call, `app.rs:3038-3059` for the timeout, which is `keep_alive_timeout` once the socket is idle between requests).
- **Zero-Copy Slicing (`split_to`):** Once a request of length $N$ is consumed, `conn_buf.split_to(consumed)` advances the buffer in $O(1)$ time without memory reallocation, immediately evaluating any remaining pipelined bytes.

---

## 3. Zero-Copy HTTP/1.1 Parser & RFC 9112 Security Engine

The parser constructs a borrowed `RequestView<'req>` directly over socket bytes:

### Security Hardening (RFC 9112 Conformance):
1. **Dual Content-Length Attack Prevention:** If multiple `Content-Length` headers or comma-separated lists are received, the request is rejected with `400 Bad Request`.
2. **CL.TE / TE.CL Desync Prevention:** Combining `Transfer-Encoding` with `Content-Length` is strictly forbidden and rejected immediately.
3. **Integer Overflow & Negative Range Protection:** All payload lengths are safely validated against `usize::MAX` and `ServerLimits::max_body_bytes`. That is the *decoded* bound; HTTP/1.1 refuses a request earlier against a *wire* budget of `max_header_bytes + max_body_bytes`, so a chunked body meets the socket bound before the parser meets the decoded one (`docs/07_limits_and_dos_mitigation.md` §4, pinned by a test).
4. **Header Limits:** Header block size is capped by `max_header_bytes` (returns `431 Request Header Fields Too Large` on violation).
5. **Request-Target Forms (RFC 9112 §3.2):** Only a target in one of the four defined forms is routed — `/path`, `http(s)://authority/path`, the `CONNECT` authority, and `*` for server-wide `OPTIONS`. Anything else is `400`, not a path: before this rule `GET hello` matched the route `/hello`. An absolute-form target supplies the `Host` (§3.2.2), replacing the received field so the host allow-list, CORS and handlers all read the same authority, while a missing `Host` on HTTP/1.1 stays a `400` and more than one `Host` field is a `400` on every version. A `CONNECT` that carries a well-formed authority is answered `501 Not Implemented`: this framework establishes no tunnel and RFC 9110 §9.3.6 offers `405` or `501` to a server without one, while a router `404` would blame the named host and port for a capability the server itself lacks. Security decisions still come first — a `Host` outside the allow-list is a `400` before that capability answer. The same set is what HTTP/2's `:path` may carry (RFC 9113 §8.3.1): a server-wide `OPTIONS` arrives as `:path` `*` and gets the identical `204` + `Allow`, a `*` on any other method is answered `400` rather than routed as a path, an empty, absolute-form or slash-less `:path` is malformed (§8.1.1) and costs only its stream a `PROTOCOL_ERROR` reset, and a `CONNECT` — which omits `:scheme` and `:path` entirely and ends its stream only once a tunnel exists (§8.5) — is answered `501` from the head instead of waiting for a body that conformant clients never send. One asymmetry belongs above this layer: `h2` removes a `#` fragment from `:path` before the request is handed over, so HTTP/1's `#` refusal has no HTTP/2 counterpart, and both protocols still identify the same target resource.

---

## 4. Deterministic Radix Trie Router & Method Bitmask Matching

### Algorithm:
- **Radix Trie with Segment Sorting:** Routes are indexed as hierarchical segment trees.
- **Deterministic Precedence:**
  1. **Static Segments** (`/users/me`) -> Checked first.
  2. **Parameterized Segments** (`/users/{id}`) -> Checked second. Braces are the only accepted
     parameter form; a `/users/:id` route stays a *literal* segment (see `router/src/trie.rs:126`).
  3. **Catch-All Wildcards** (`/static/*filepath`) -> Checked last.
- **Zero-Allocation Stack Traversal:** Path splitting uses `SmallVec<[&str; 8]>` on the CPU stack.
- **64-bit Method Bitmask:** Checking whether a route supports `GET`, `POST`, `PUT`, `DELETE`, etc., is executed in a **single CPU instruction**. Each method owns one bit of the node's `allowed_methods: u64` (`framework/http/src/method.rs:25-43` numbers the bits, `framework/router/src/node.rs:42-44` is the test):
  ```rust
  (self.allowed_methods & method.bitmask()) != 0
  ```

---

## 5. Request Context & Memory Architecture (`RequestContext`)

`RequestContext` is passed to every route handler with strictly scoped lifetimes, preventing memory leaks (eliminated all `Box::leak` calls):

### Key Functions & APIs:

| Function | Signature | Description |
| :--- | :--- | :--- |
| `param` | `pub fn param(&self, key: &str) -> Option<&str>` | Retrieves route parameter value (from `{id}`, key `id`). |
| `param_parse` | `pub fn param_parse<T: FromStr>(&self, key: &str) -> Option<T>` | Parses a route parameter into a typed primitive (`u32`, `i64`, `bool`). |
| `query` | `pub fn query(&self, key: &str) -> Option<&str>` | Retrieves query string parameter (`?page=2`). |
| `header` | `pub fn header(&self, name: &str) -> Option<&str>` | Case-insensitive header lookup. |
| `cookie` | `pub fn cookie(&self, name: &str) -> Option<&str>` | Extracts cookie value by name from `Cookie:` header, exactly as the client wrote it. |
| `signed_cookie` | `pub fn signed_cookie(&self, signer: &CookieSigner, name: &str) -> SignedCookie` | Reads a signed cookie, separating `Present` from `Absent`, `Invalid` and `Ambiguous`; the name is bound into the MAC. |
| `body_str` | `pub fn body_str(&self) -> Result<&str, Utf8Error>` | Zero-copy UTF-8 view of request body. |
| `json` | `pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error>` | Direct JSON deserialization from body bytes. |
| `state` | `pub fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>>` | Retrieves typed shared state from the DI container. |
| `request_id` | `pub fn request_id(&self) -> &str` | Accesses the unique trace identifier for the request. |

---

## 6. Response Construction & Zero-Allocation Serialization (`Response`)

### Response Constructors:
- `Response::ok()`: an empty `200 OK`. It takes no body — the body-carrying constructors below set
  `Content-Type` and `Content-Length` themselves.
- `Response::text(&'static str)` / `Response::text_dynamic(impl Into<String>)`: `text/plain`.
- `Response::json(Bytes)` / `Response::json_status(u16, &'static str, impl Into<Bytes>)`: attaches
  `Content-Type: application/json`. **It does not serialize anything** — the caller passes the bytes
  (`serde_json::to_string(&data).unwrap().into()`), which is why the type the argument resolves to
  has to be `Bytes` and no `Response::json(&Serializable)` exists.
- `Response::status(code, text)`: Creates a custom status response (e.g. `201 Created`, `404 Not Found`).

### Direct Buffer Serialization (`write_http1_to`):
Writes headers and body directly into `BytesMut` using `bytes::BufMut::writer`, completely bypassing intermediate `String` or `format!()` heap allocations.

---

## 7. Dependency Injection & Shared Application State (`AppStateContainer`)

Aether includes a thread-safe DI container keyed by `TypeId`. The registration method on the app is
`manage`, and a handler reads the value back by type through `ctx.state::<T>()` (an `Arc<T>`):

```rust
let mut app = AetherApp::new();
app = app.manage(DatabasePool::new("postgres://..."));
app = app.manage(RedisCache::new("redis://..."));

app = app.get_async("/data", |ctx: RequestContext| async move {
    let db = ctx.state::<DatabasePool>().expect("DatabasePool registered");
    let _ = db;
    Response::text("Data fetched")
});
```

There is no public `app.state_container` field and no `AetherApp::with_state(..)`; `manage` is the
registration path.

---

## 8. Middleware Subsystems

- **`CorsMiddleware`**: Configurable CORS policies (Origins, Methods, Headers, Max Age).
- **`SecurityHeadersMiddleware`**: Injects HSTS, X-Frame-Options (`DENY`), X-Content-Type-Options (`nosniff`), and CSP.
- **`LoggerMiddleware`**: Zero-cost abstraction (`#[inline(always)]`), disabled on hot-paths by default to prevent `stdout` global mutex contention.

---

## 9. Resource Limits, Concurrency Semaphores & Slowloris Mitigation (`ServerLimits`)

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,     // 8 KB default
    pub max_body_bytes: usize,       // 2 MB default, decoded body bytes (HTTP/1.1 also bounds wire bytes)
    pub max_headers_count: usize,    // 100 header field lines default
    pub max_concurrent_streams: u32, // 100 open HTTP/2 streams per connection
    pub max_connections: usize,      // 10,000 concurrent sockets
    pub read_timeout: Duration,      // 5s Slowloris mitigation
    pub write_timeout: Duration,     // 10s socket flush timeout
    pub keep_alive_timeout: Duration,// 60s idle timeout
    pub request_timeout: Duration,   // 30s handler budget
    pub tls_handshake_timeout: Duration, // 10s TLS handshake bound
}
```

---

## 10. Error Handling Subsystem (`AetherError`)

The variants are `HeaderTooLarge`, `BodyLimitExceeded`, `MalformedRequest`, `RouteNotFound`,
`MethodNotAllowed`, `ValidationFailed(String)`, `DependencyCycleDetected(String)`, `Timeout`,
`Io`, `Serialization(String)`, `Custom(String)` (`framework/core/src/error.rs:5-19`). There is no
`PayloadTooLarge` variant. What the parse loop does with them is a three-arm match at
`framework/aether/src/app.rs:2991-2999`:

- `AetherError::HeaderTooLarge` -> Returns `431 Request Header Fields Too Large`.
- `AetherError::BodyLimitExceeded` -> Returns `413 Content Too Large`.
- any other parse error -> Returns `400 Bad Request`.
- every one of the three is written with `Connection: close` (`app.rs:3000`), so the socket is not
  reused after a framing error.
- `AetherError::Timeout` is not answered by that match: a handler that overruns
  `request_timeout` gets `504 Gateway Timeout` plus `Connection: close` from the serve layer
  (`framework/aether/src/serve.rs:972-977`), and its task is aborted.

---

## 11. End-to-End Developer Examples

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct UserProfile {
    id: u64,
    username: String,
}

#[derive(Deserialize)]
struct CreateUserDto {
    username: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    app = app.get("/", |_ctx| Response::text("Aether Core Engine Running"));

    app = app.get_async("/users/{id}", |ctx: RequestContext| async move {
        let user_id = ctx.param_parse::<u64>("id").unwrap_or(0);
        let profile = UserProfile {
            id: user_id,
            username: format!("user_{}", user_id),
        };
        Response::json(serde_json::to_string(&profile).unwrap().into())
    });

    app = app.post_async("/users", |ctx: RequestContext| async move {
        match ctx.json::<CreateUserDto>() {
            Ok(dto) => {
                let profile = UserProfile {
                    id: 101,
                    username: dto.username,
                };
                Response::json_status(
                    201,
                    "Created",
                    serde_json::to_string(&profile).unwrap(),
                )
            }
            Err(_) => Response::status(400, "Bad Request"),
        }
    });

    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```

This block was compile-checked on 2026-10-22: `cargo check` in a throwaway crate whose only
dependency is `aether` by path, with `serde`, `serde_json` and `tokio` alongside it — exit 0, no
warnings. It replaces an example that did not compile and could not have: `aether::context` and
`http::response::Response` are not our paths (the prelude re-exports `aether_http::response::Response`),
`Response::ok(body)` takes no body, `Response::json(&serializable)` and
`Response::status(..).with_json(..)` do not exist, `/users/:id` is a *literal* segment because the
trie only treats `{name}` as a parameter, and `app.get` with an `async move` closure is rejected —
`get_async`/`post_async` are the async forms. `main` returns `Box<dyn Error + Send + Sync>` because
that is what `listen` fails with; `Box<dyn Error>` alone does not compile with `?`.
