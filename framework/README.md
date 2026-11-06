# Aether Web Framework

A production-oriented Rust web framework built on Tokio. It ships its own HTTP/1.1
connection engine, a segment radix router, security limits, and a small set of built-in
middleware.

**Status:** HTTP/1.1 over plaintext and over TLS on one port, HTTP/2 over TLS when ALPN
negotiates `h2` and over plaintext on a listener that opts into `AetherApp::enable_h2c`, and
RFC 6455 WebSocket over both plaintext and TLS. HTTP/3 and the `Upgrade: h2c` exchange are **not**
implemented. WebSocket is an HTTP/1.1 mechanism: a WebSocket route reached over HTTP/2 answers 400. See
[docs/FEATURE_COMPLETENESS_MATRIX.md](docs/FEATURE_COMPLETENESS_MATRIX.md) for a row-by-row
audit against Axum and Actix Web, including everything that is missing.

## What is verified today

- HTTP/1.1 parsing and framing: Content-Length and chunked request bodies, pipelining,
  keep-alive, exact consumed-byte accounting.
- RFC 9112 smuggling defenses: dual `Content-Length`, `Content-Length` + `Transfer-Encoding`,
  unsupported transfer codings, and HTTP/1.0 chunked requests are rejected.
- Security limits: 8 KiB header budget, 100 header field lines, 2 MiB decoded body (HTTP/1.1 also
  refuses a request whose *wire* bytes pass the two budgets summed, which is what a chunked upload
  hits first — see `AETHER_MAX_BODY_BYTES` below), 10 000 connections,
  5 s per-read idle timeout, a total receive deadline (default 30 s) that returns 408 to
  trickling clients, 10 s write timeout, 30 s handler timeout.
- Radix routing with static > parameter > catch-all precedence, percent-decoded parameters,
  correct 404, and 405 with a complete `Allow` header.
- Per-request panic isolation: a panicking handler returns 500 and the server keeps running
  (release profile uses `panic = "unwind"`).
- CORS (allow-list, credentials, preflight, `Vary: Origin`), security headers, and
  gzip/Brotli compression with q-value negotiation.
- Duplicate request header fields are preserved (`header_values`); `header()` still answers with
  the first line, and `headers()` enumerates one lowercase `(name, first line)` pair per field.
- Multipart/form-data parsing (RFC 7578) with borrowed parts and strict size/boundary limits.
- Dynamic static-file serving (`static_dir`) with strong ETags, conditional GET, byte ranges
  (`206`/`416`, strong `If-Range`), Cache-Control, async disk I/O, and traversal/Windows
  device-name protection.
- Graceful shutdown on Ctrl+C and Unix SIGTERM, plus a programmatic
  `AetherServer::serve_with_shutdown` API for tests and embedders.
- JWT (HS256) validation with algorithm pinning, `exp`/`nbf`/`iat`/`iss`/`aud` checks, and
  constant-time signature verification.
- WebSocket RFC 6455: validated opening handshake, subprotocol negotiation
  (`app.ws_subprotocols(path, &["chat.v2"], ..)`, RFC 6455 §4.2.2 selection echoed in the `101`),
  fragmented message assembly, ping/pong,
  close handshake, bounded frame/message sizes, and `app.ws()` routing. Compression
  (permessage-deflate) is not implemented.
- Typed extractors on the `*_async` route methods: `Path<T>`, `Query<T>`, `Json<T>`,
  `Form<T>`, and `State<T>` for zero to four handler arguments, with typed rejections
  (400/415/500). A handler taking the context directly must now annotate it:
  `|req: RequestContext| async { .. }`.
- Streamed request bodies over both frontends: `app.route_stream(...)` dispatches before the body
  arrives, and the handler reads it with `RequestContext::read_body` or pulls chunks from
  `body_stream()`, bounded by `max_stream_body_bytes` and `max_receive_time`. See
  *Streaming request bodies* below.
- User-defined middleware: `app.use_middleware(...)` app-wide and
  `RouteGroup::use_middleware(...)` per group, composed around the handler with a `Next`
  continuation; middleware runs in registration order and may short-circuit.
- Scoped state and mountable sub-apps: `RouteGroup::manage(...)` registers a value that answers
  for that group's routes before the app-wide container, `Router` collects routes, guards and
  state without knowing a prefix, and `app.mount("/api", router)` (or `group.mount`, or
  `router.mount`) replays it under one. A scope shadows only the types it registers: everything
  else keeps resolving app-wide.
- An ergonomic `TestClient` for integration tests: starts the app on an ephemeral loopback
  port, sends real HTTP/1.1 requests, and offers request-builder, JSON/form, and graceful
  shutdown helpers.

The regression suite includes 73 checked-in raw-socket end-to-end tests
(`tests/tests/http_e2e.rs`), 12 TLS wire tests (`tests/tests/tls_e2e.rs`), 8 request-extension wire
tests (`tests/tests/extensions_e2e.rs`) and 49 HTTP/2 wire tests
(`tests/tests/http2_e2e.rs`, counted from the test attributes — `grep -c '#\[\(tokio::\)\?test'`
on 2026-10-30), which cover
TLS with ALPN `h2`, cleartext h2c through the `h2` client, and connections written as raw frames for
the input a well-behaved client will not produce: the advertised concurrency cap, and a head that
stops at the preface. Some of them hand-frame HEADERS/DATA/SETTINGS at the byte level, some also send
a plaintext HTTP/1.1 request on the same port to prove that framing did not steal it, and one holds
four streams on one connection with three of them inside the middleware's hold at once, to prove
per-request state is per stream — so these behaviors are exercised by `cargo test --workspace`
rather than only by an external harness.

## Not implemented (selected)

HTTP/3, HTTP/2 server push, the `Upgrade: h2c` exchange (deprecated by RFC 9113 §3.1; cleartext
HTTP/2 is served by prior knowledge through `enable_h2c` instead), literal
`sendfile(2)`, and WebSocket permessage-deflate. Fetching certificate revocation from a CA is not
implemented — revocation is enforced offline from a CRL or OCSP response the operator supplies.
This list is not exhaustive; the matrix is.

## Streaming request bodies

`route_stream` registers a route whose handler runs as soon as the head has been parsed, before
the body has arrived. The handler pulls the body itself, so an upload costs one chunk of memory
rather than its whole length:

```rust
use aether::prelude::*;
use bytes::Bytes;

app = app.route_stream(Method::Post, "/upload", |mut req: RequestContext| async move {
    match req.read_body(16 * 1024 * 1024).await {
        Ok(body) => Response::json(Bytes::from(format!("{}", body.len()))),
        Err(BodyError::TooLarge) => Response::status(413, "Payload Too Large"),
        Err(_) => Response::status(400, "Bad Request"),
    }
});
```

`read_body(max)` is the whole-body form; `body_stream()` gives the pull loop when a handler wants
to act on each chunk (`while let Some(chunk) = req.body_stream()?.next_chunk().await {}`). On a
streaming route `req.body` is empty and `Json<T>`/`Form<T>` do not work, because both read that
buffered copy. The body is bounded by `max_stream_body_bytes` — a declared `Content-Length` over
it is `413` before the handler runs — and by `max_receive_time` counted from the head. A handler
that answers without consuming the body makes the server close the connection after the response,
since the unread tail cannot be framed as another request; a body that finished arriving keeps
keep-alive. Both frontends stream: on HTTP/2 the pump releases flow-control capacity per DATA
frame, so a body larger than the 65,535-byte default window still arrives whole, and an unread
body costs only its own stream — the connection keeps serving.

## Typed extractors

```rust
use aether::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct CreateUser { name: String }

app = app.post_async("/users/{id}", |Path(id): Path<u32>, Json(body): Json<CreateUser>| async move {
    format!("created user {id} named {}", body.name)
});
```

Extraction failures return a typed rejection (400 for malformed input, 415 for a wrong
`Content-Type`, 500 for missing state or a missing extension) and the handler never runs.

## User middleware

```rust
app = app.use_middleware(|req: RequestContext, next: Next| async move {
    if req.header("x-token").is_none() {
        return Response::status(401, "Unauthorized");
    }
    next(req).await
});
```

Middleware applies to routes registered after the `use_middleware` call. Group-scoped
middleware composes with the app-wide stack (app-wide runs first on the request path).

## Scoped state and sub-applications

`app.manage(value)` is process-wide. A group can register its own, and a route under that group
resolves the group's value first and the app's after — for the types the group registered, and
only those:

```rust
app = app.group("/tenant-a", |group| {
    group.manage(Db::connect(DSN_A));       // this group's routes see this handle
    group.get_async("/rows", |State(db): State<Db>| async move { db.count().await });
});
app = app.get_async("/global", |State(db): State<Db>| async move { db.count().await });
```

A `Router` is the same thing with no prefix attached: it collects routes, guards and state, and
knows nothing about the port or the mount it will eventually be served from.

```rust
let mut api = Router::new();
api.manage(Db::connect(DSN_A));
api.use_middleware(require_tenant_header);
api.get_async("/rows", |ctx: RequestContext| async move { /* .. */ });

app = app.mount("/api", api);                     // /api/rows

let mut v1 = Router::new();                 // a router mounted inside a group
v1.get_async("/rows", |ctx: RequestContext| async move { /* .. */ });
app = app.group("/tenants", |group| { group.mount("/v1", v1); });  // /tenants/v1/rows
```

A mount behaves exactly like the same registrations written inline at that prefix: the app-wide
stack, then the enclosing group's, then the router's, then the handler; a `manage` reaches the
registrations that follow it; and a route registered outside the mount keeps resolving the
app-wide container. `State<T>` and `RequestContext::state::<T>()` both obey that order, over
HTTP/1.1, HTTP/2 and a WebSocket upgrade.

Mounting consumes the router: `mount` takes it by value and `Router` is not `Clone`, so a
feature that has to answer at two prefixes builds two routers, or nests one inside the
other with `Router::mount`.

## Request-scoped extensions

A middleware that decides something the handler needs to know — who the caller is, which tenant,
which feature flag — has a place to put it that is neither process-wide state nor a header:

```rust
use aether::prelude::*;

#[derive(Clone)]
struct Principal(String); // any 'static + Send + Sync

app = app.use_middleware(|mut req: RequestContext, next: Next| async move {
    if let Some(token) = req.header("authorization") {
        req.insert_extension(Principal(token.to_string()));
    }
    next(req).await
});

// As a typed argument. This form requires `Clone`, because the map holds one owner and hands a
// copy onward — the same bound Axum's `Extension<T>` carries.
app = app.get_async("/whoami", |Extension(p): Extension<Principal>| async move {
    format!("hello {}", p.0)
});

// Or borrowed from the context, which also works for a value deliberately left non-`Clone`.
app = app.get_async("/whoami2", |req: RequestContext| async move {
    match req.extension::<Principal>() {
        Some(p) => Response::text_dynamic(p.0.clone()),
        None => Response::text_dynamic("anonymous".to_string()),
    }
});
```

The map travels with one request: it is per stream on HTTP/2 and per request on a keep-alive
HTTP/1.1 connection, and it is boxed only when something is inserted, so requests that never use it
allocate nothing. `Debug` prints a count and never the values.

## Testing utilities

```rust
let mut app = AetherApp::new();
app = app.get("/hello", |_| "hello");

let client = TestClient::start(app).await;
let res = client.get("/hello").send().await;
assert_eq!(res.status, 200);
assert_eq!(res.text(), "hello");
client.shutdown().await;
```

## Quick start

```rust
use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();
    app = app.enable_security_headers();

    app = app.get("/", |_req| Response::text("hello"));
    app = app.get("/users/{id}", |req| {
        let id = req.param("id").unwrap_or("unknown");
        let body = serde_json::json!({ "id": id }).to_string();
        Response::json(bytes::Bytes::from(body))
    });

    app.listen("0.0.0.0:3000").await
}
```

A `Cargo.toml` for an application:

```toml
[dependencies]
aether = { path = "path/to/aether/framework/aether" }
tokio = { version = "1", features = ["full"] }
bytes = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

The workspace is Rust 2024 edition; the verified toolchain is rustc 1.98.1.

## Configuration

`AetherConfig::from_env()` reads:

| Variable | Default | Meaning |
|---|---|---|
| `AETHER_HOST` | `0.0.0.0` | Bind host (IPv6 literals are bracketed) |
| `AETHER_PORT` | `3000` | Bind port |
| `AETHER_ENV` | `development` | Environment name |
| `AETHER_MAX_BODY_BYTES` | `2097152` | Decoded request body budget. HTTP/1.1 refuses a request whose **wire** bytes pass `AETHER_MAX_HEADER_BYTES + this`, so a chunked upload — framing costs 6 wire bytes per payload byte at 1-byte chunks — meets that bound first: on the defaults it is served to 350 879 payload bytes and refused at 350 880. HTTP/2 counts DATA payloads, so there the number is the decoded cap |
| `AETHER_MAX_HEADER_BYTES` | `8192` | Request head budget |
| `AETHER_WORKERS` | CPU count | Tokio worker threads. Applied when the app builds its own runtime (`AetherApp::run`, as `basic_api` does); on `listen`/`serve` the caller's runtime decides the count and the banner says so |
| `AETHER_MAX_REQUEST_TIME_SECS` | `30` | Per-handler timeout |
| `AETHER_MAX_RECEIVE_TIME_SECS` | `30` | Total receive budget for one request, head and body, on both frontends. Independent of the handler timeout: a slow upload is admitted or refused by this number alone, and an over-budget request receives `408` |
| `AETHER_MAX_RESPONSE_TIME_SECS` | `10` | Response write timeout |
| `AETHER_KEEP_ALIVE_TIMEOUT_SECS` | `60` | Idle keep-alive bound before a connection is closed |
| `AETHER_MAX_HEADERS_COUNT` | `100` | Header **field lines** per request; HTTP/1 answers an over-budget section with 431, the HTTP/2 frontend counts the same budget. `from_env` rejects `0` |
| `AETHER_MAX_CONCURRENT_STREAMS` | `100` | Value advertised as `SETTINGS_MAX_CONCURRENT_STREAMS` and enforced against that advertisement (over-cap streams are reset with `REFUSED_STREAM`, no GOAWAY). `from_env` rejects `0` |

Every row above needs two things to have any effect: the process must **read** the environment
(`AetherConfig::from_env()`, not `AetherConfig::default()` — `AetherApp::new()` uses the default),
and it must **act** on it. `basic_api` did the second without the first until the 2026-10-25 banner
check, when it was switched to `from_env()`; now `AETHER_WORKERS` and `AETHER_PORT` both reach it, so
a pinned one-core server is one environment variable away.

`from_env` is the only guard against `0`: a directly constructed `AetherConfig { max_headers_count: 0, .. }`
reaches both frontends as 0 and every request that carries a header line is refused. The stream cap
is the protocol-level bound; the scheduler's per-stream memory budget is a second, independent one
that answers `503` on a stream it cannot cover.

Every field above is also a field of `AetherConfig`, and one derivation (`AetherApp::limits_for`)
turns the config into the `ServerLimits` the frontends enforce — so the number an operator sets is
the number that bites. `max_connections`, `read_timeout` and `tls_handshake_timeout` stay on
`ServerLimits::default()` and are not reachable from `AetherConfig`.

`AetherConfig::listen_addr()` formats IPv6 hosts correctly.

## Static files

```rust
// Serves ./public/** at /files/** with a one-hour Cache-Control.
app = app.static_dir_with_cache("/files", "./public", 3600);
```

Directories return 404 unless `StaticDirOptions::directory_listing` is on, and a
pre-compressed `.br`/`.gz` sibling is served when `precompressed` is set. Files are streamed from
disk in 64 KiB chunks rather than read into memory, so a `HEAD`, `304` or `416` never opens the
payload. Single and suffix byte ranges are supported with `206`, `416`, `Content-Range`, and
`If-Range` matching the published `Last-Modified` date (the ETag is a weak metadata validator and
does not satisfy `If-Range`). `.`/`..` traversal, percent-encoded
traversal, Windows alternate data streams, reserved device names, and trailing dots/spaces are
rejected.

## WebSocket

```rust
app = app.ws("/ws", |_req, mut socket| async move {
    while let Ok(Some(message)) = socket.recv().await {
        if let WsMessage::Text(text) = message {
            if socket.send_text(&text).await.is_err() {
                break;
            }
        }
    }
});
```

The framework validates the opening handshake and writes `101 Switching Protocols` before the
handler runs; invalid handshakes receive 400, unsupported versions 426 (advertising 13), and
non-GET methods 405. `WebSocket` exposes `recv`, `send`/`send_text`/`send_binary`, `ping`, and
`close`, with default 16 MiB frame/message limits. When the server shuts down, a session that is
in `recv` is sent one close frame with status `1001` ("going away") and the handler's loop ends on
it; a handler that never returns to `recv` is aborted after a 1 s grace, so shutdown stays bounded.

The upgrade request itself is an ordinary request: the middleware registered before the route
runs around it, so a guard can stamp [`Extensions`](#request-scoped-extensions) that the handler
reads, refuse the upgrade with its own response (401 for an auth chain), or panic and be mapped to
500 like any other handler. The refusal works whether the guard answers before calling `next` or
after awaiting it — the upgrade proceeds only if the chain returns the terminal's `101` untouched,
so a later 403, a 500 from a post-`next` panic, or a 504 from a guard that stalls on the way back
out is served instead and no socket is handed over. The same rule holds for a guard that runs the
chain in a separate task or races it with `select`: whichever response the guard returns is the
answer, so a forked chain that completes with `101` while the guard returns 202 gets 202 and no
handover. A guard cannot call `next` twice — `RequestContext` is not `Clone` and `next` consumes it.
What a guard cannot do is edit that `101` or revoke the upgrade by mutating headers:
`RequestContext` exposes no mutable header store (one store, read through
`header`/`header_values`/`headers`), and the handshake response is synthesized by the protocol
frontend. Neither does a refusal cancel work
the guard already started: dropping a spawned chain's `JoinHandle` does not abort it, so the client
is answered and the socket is not handed over while the forked middleware keeps running to
completion — handoff authorisation is a rule about the response, not a kill switch.
Two limits stay: the CORS/security-header/compression post-pass does not apply to the `101` the
frontend **synthesizes** on a granted upgrade, and nothing wraps the session after the socket is
handed over. The `101` a middleware returns *itself*, with the terminal never reached, is a different
thing — it claims a protocol transition that never happened, so the serving core refuses it with
`500 Internal Server Error` instead of sending it. Measured shape of the refusal
(`a_hand_built_101_from_a_middleware_is_refused_with_500`): the guard's `101` was given `Upgrade`,
`Connection`, `Sec-WebSocket-Accept` and a JSON body, and the client receives `HTTP/1.1 500` with
nothing but `X-Request-ID`, `Date` and `Content-Length: 0`, no body, no session started, and the same
socket still answering a following `GET` with `200`. The same test then upgrades through the same
middleware to prove the granted path is untouched (`101` with the RFC 6455 accept vector, session
started, frame echoed). `101` is what the framework emits when it hands a socket over, not a value a
middleware can claim — and the rule belongs to the status, not to the route or the author: the serving
core replaces **any** `101` it did not synthesize for a handoff, so a handler's claim on an ordinary
route is refused the same way (`a_101_without_a_handoff_is_never_served`), and over HTTP/2 the answer
is the same `500` instead of the protocol error `h2` raised for such a head
(`a_101_from_a_middleware_is_refused_over_h2`). The refusal is one status, not the informational
class, and that boundary is tested rather than asserted: a `103` or a `418` on a WS route still travel
the ordinary path, and widening the condition to the whole 1xx class makes the test fail
`left: 500, right: 103`.
Only two refusals skip the chain, and neither is a request decision: a non-`GET` method on a WS route
(`405`) and a WS route reached over HTTP/2 (`400`, RFC 7540 §8.1 forbids the mechanism there). A plain
`GET` with no handshake headers on a WS route does run the chain, and its `400` comes from the
validator afterwards. **Neither refusal had a wire test until the 2026-10-21 audit named the gap** —
the non-`GET` case had none at all, and the HTTP/2 tests asserted the status and connection survival
without proving the chain was skipped. Both now have one
(`a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain`, and the HTTP/2 test extended
with the same counter), and each was falsified: disabling the HTTP/2 transport gate makes that test
fail `left: 1, right: 0` while the status stays `400`, so the status assertion alone was blind to it.
One correction the falsification forced: the `405` is produced by the **router**, not by the serving
core — `app.ws` registers the path for `GET` only, so a non-`GET` never reaches the WebSocket branch
of `serve.rs`. That branch's own `405` is a second line of defence, normally unreachable through the
public API; when it was made reachable in a scratch build it did answer `405` before the chain, and
disabling it there let a `POST` be upgraded (`101` on the wire). So the guard is real, and the
attribution in the earlier draft was wrong.

## Security notes

- `enable_security_headers()` is **opt-in** and adds HSTS, `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, and `X-XSS-Protection: 0`.
  CSP is configurable but off by default because a wrong policy breaks applications.
- TLS is built in (`rustls`, pinned to the **ring** provider so Windows needs no external
  toolchain): `AetherApp::tls`, `tls_from_files`, `tls_from_pem`, optional `require_tls(true)`,
  SNI, and live certificate reload through `TlsAcceptor::replace_certificates`. Revocation is
  enforced offline from material the operator supplies: `TlsCertSet::with_crl` refuses a leaf a
  signed, current CRL lists, and `TlsCertSet::with_ocsp` staples only a successful, current,
  correctly signed OCSP response that names this chain. Nothing fetches from a CA, so revoked
  material is refused at load or reload time rather than discovered later.
- The total receive deadline is its own setting (`AETHER_MAX_RECEIVE_TIME_SECS`), separate from the
  handler timeout (`AETHER_MAX_REQUEST_TIME_SECS`): a legitimately slow large upload no longer
  shares its budget with handler execution. Pinned in both directions by
  `slow_trickle_receives_408_within_the_total_receive_deadline` and
  `a_slow_upload_is_not_bounded_by_the_handler_timeout`.
- Cookies are parsed but not signed or encrypted.
- The server answers HTTP/1.1 requests as HTTP/1.1 — including HTTP/1.0 requests, which get a
  `HTTP/1.1` status line — and HTTP/2 only on a connection that asked for it: one whose ALPN
  negotiated `h2`, or, on a listener with `enable_h2c`, one that opened with the cleartext
  preface. `Frontend` is decided once, in `transport::establish`, and the serving loop dispatches
  on it rather than guessing from the stream afterwards.

## Build, test, run

```bash
cd framework
cargo test --workspace          # see the count paragraph below; 0 failures
cargo build --release --workspace
cargo run --release --bin basic_api
cargo run --release --bin scheduling_demo   # adaptive scheduling demo (see docs/SCHEDULING_COMPARISON.md)
```

The suite (re-measured 2026-11-06 on the track-143 tree) is **946 tests, 0 failures, 1 ignored** — 495 unit +
430 integration + 21 doc tests over 64 `test result:` lines — with
the integration half spread over 30 checked-in targets: 102 raw-socket end-to-end tests in
`tests/tests/http_e2e.rs`, 86 HTTP/2 wire tests in `tests/tests/http2_e2e.rs`, 14 nested-state and
mounted-sub-app wire tests in `tests/tests/nested_routers.rs`, 12 TLS wire tests in
`tests/tests/tls_e2e.rs`, 8 request-extension wire tests in `tests/tests/extensions_e2e.rs`, 8 wire
tests in `tests/tests/openapi_derivation.rs` (the document an `AetherApp` derives from its own route
table, tracked by track 108), 11 signed-cookie wire tests in `tests/tests/signed_cookies.rs` (what the
client sent is not what this server issued, and five of the same classifications re-asked over HTTP/2 in
`tests/tests/http2_e2e.rs`, where the request's fields are built into the request's one header store: the value the
peer sent has to arrive untouched — a separator written `%2E` is refused rather than decoded — and a name
carried twice has to stay refused whichever field shape it came in), and
1 error-reporting wire test in `tests/tests/error_reporting.rs` (a redacted 5xx detail reaches an
installed `aether::set_error_reporter` under the response's own `X-Request-ID`), 2 credential-rendering
tests in `security/tests/secret_rendering.rs` (the CSRF token's `Debug` is redacted and a refusal echoes
no credential; `JwtValidator`, `BearerAuth` and `CsrfTokens` have no `Debug`, which three `compile_fail`
doc tests enforce),
19 extractor tests; 50 integration tests put at least one middleware in the chain (15 in
`http_e2e.rs`, 13 in `user_middleware.rs`, 8 in `http2_e2e.rs`, 8 in `extensions_e2e.rs`, 3 in
`nested_routers.rs`, 3 in `bearer_auth.rs` — counted by a rule rather than by eye: a `#[test]` or
`#[tokio::test]` function counts when its own body, or a same-file function it calls transitively,
registers a chain through `use_middleware`, `route_sync_with`, `route_async_with` or `ws_with`, which
is how a chain written in the test or in that file's own `ext_app` / `marked_app` / `extractor_app`
builder both get counted; earlier lines
here said "20 (8 + 12)", then "27", then 32 with 5 in `http2_e2e.rs`, then 34 with 7, none of which
survived a re-run — re-run the count per file rather than re-copy it. This re-run moved
`http2_e2e.rs`'s 7 to 8 by the same checkable route — the 3 WebSocket/`101` transport tests plus the 4
extension tests, plus track 107's `a_mounted_router_answers_with_its_own_state_over_h2c`, which is the
8th — and it is *not* `a_peer_that_vanishes_mid_body_leaves_no_registration_behind`, whose
app comes from `build_app(None)`, which installs no middleware. The 13 this paragraph carried for
`http_e2e.rs` is 15 under the rule: `per_route_middleware_wraps_the_upgrade_request_of_its_websocket_route`
and `a_per_route_guard_refuses_a_websocket_upgrade_before_the_handover` register through `ws_with`
rather than `use_middleware`, which is the call an eye-count looks for); 57 tests for the protocol-independent
`aether-scheduler` crate (11 unit + 46 in `scheduler/tests/`), and 18 scheduling wire tests in
`tests/tests/scheduling.rs`; no external harness is required to reproduce the server-level checks.
Track 102 added the two property suites (`http/tests/parser_properties.rs` 4,
`router/tests/router_properties.rs` 4 when it landed and 5 on this tree — track 108 put the listing
property there) and four `aether-fuzz` unit tests, and ran the in-repo fuzz
runner (deterministic mutation over the committed corpus, crash artifacts on disk) for
**15 843 328 executions in 120.011 s** at seed 20261030 with 0 crashes — a runner that was proved
to fire by catching a deliberately injected chunk-boundary panic at execution 6 507. Track 106 re-ran
it over its own parser change: **16 089 088 executions in 120.006 s** (134 069/s), corpus 12 → 711,
709 outcome buckets, 0 crashes.
Earlier versions of this paragraph carried 468 / 45 / 29, then 513 / 50 / 34 from the 2026-10-26
track-72/73 recount, then 540 from the 2026-10-27 track-86 tree, then the 736 / 63 / 26 reading of the
track-103 tree — which six later tracks had already moved to 750, then 756, then 776, then 795, then
814 (track 108's 19 = 4 `route_listing` unit + 6 `aether-openapi` unit + 1 router property + 8 derivation
wire tests; track 109 moved none, its work being a documentation gate; track 110's 19 = 11 `http2_e2e`
+ 5 `http_e2e` + 2 `streaming_request` wire tests + 1 `parser.rs` unit test, all of them pinning the
request declared-length contract) while this paragraph still said 736; track 114's 29 = 15
`security/src/cookie.rs` unit + 11 `tests/signed_cookies.rs` wire + 2 `aether/src/cookie.rs` doctests +
1 `compile_fail` took it to 843; track 115's 5 = five wire tests that read a signed cookie through the
HTTP/2 frontend's own header store, appended to `tests/http2_e2e.rs`, took it to 848; tracks 116-118
added the 44 that `Expect`, the receive budget and the diagnostic work brought — recounted per
track in the improvement register — and track 123's 19 = 15 `aether-middleware` unit tests + 4 `http_e2e` wire tests took it to 911 on this tree; the counts above are re-derived
from the current tree's per-target `test result:` lines. The middleware-chain count was re-read
per app on 2026-10-30: it moved with `user_middleware.rs` (8 → 13 when track 94 added the
per-route cases, two of which install their chain through the per-route API rather than
`use_middleware`), and `extensions_e2e.rs`'s eighth arrives through its `extractor_app` builder. Track
107 re-read the whole set through the rule above, which is what added `nested_routers.rs` (3) and
`bearer_auth.rs` (3, each installing its layer through the per-route API) and moved `http_e2e.rs` to 15.

## Documentation

- [Feature-completeness matrix](docs/FEATURE_COMPLETENESS_MATRIX.md)
- [Adaptive Tree Scheduling design](docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md) and
  [measured policy comparison](docs/SCHEDULING_COMPARISON.md) — opt-in admission control,
  baseline-derived work quotas, and per-kind capacity via `AetherApp::with_scheduling`
- [Production-readiness report](docs/PRODUCTION_READINESS_REPORT.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Deployment](docs/DEPLOYMENT.md)
- Older reports (`FINAL_REPORT.md`, `CONSOLIDATED_FINAL_REPORT.md`) predate the current audit
  and may contain stale claims.

## License

MIT OR Apache-2.0.
