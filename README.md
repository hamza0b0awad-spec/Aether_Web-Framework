# Aether Web Framework

Aether is a Rust web framework built on Tokio with its own HTTP/1.1 connection engine,
a segment radix router, request-scoped security limits, and built-in CORS, security-header,
and compression middleware.

The Rust workspace lives in [`framework/`](framework/). The audit documents are in
[`framework/docs/`](framework/docs/), including the
[feature-completeness matrix](framework/docs/FEATURE_COMPLETENESS_MATRIX.md) and the
[production-readiness report](framework/docs/PRODUCTION_READINESS_REPORT.md).

## Status

HTTP/1.1 and HTTP/2 over TLS on one port, and cleartext HTTP/2 on a listener that opts into
`AetherApp::enable_h2c`; HTTP/3 and the `Upgrade: h2c` exchange are **not** implemented. The matrix
records verified
behavior against Axum and Actix Web and lists every missing feature; do not rely on older
reports that predate the current audit.

Verified on the current tree (re-measured 2026-11-06 on the track-143 tree with `cargo test --workspace`):
**946 workspace tests pass, 0 fail, 1 ignored**, including
102 checked-in raw-socket end-to-end tests, 86 HTTP/2 wire tests, 18 scheduling wire tests, nine
property tests (parser and router invariants, each shown to fail on a deliberate mutant), eighteen
wire tests and one parser unit test pinning the request declared-length contract on both frontends
(over HTTP/2 a body that does not match its `content-length` is reset with `RST_STREAM`
`PROTOCOL_ERROR` before it reaches a handler; over HTTP/1 the body stops at the declared length and
the surplus is framed as the next message), eight
wire tests for the OpenAPI document `AetherApp::openapi_spec` derives from the app's own route
table, and a
secret-rendering guarantee — the key-holding types cannot be printed at all, the CSRF token's `Debug`
is redacted, and the TLS types' `Debug` carries no private-key bytes — pinned by two tests and three
`compile_fail` doc tests; and thirty-one tests for a signed cookie — fifteen in `aether-security`
and sixteen on the wire, eleven over HTTP/1.1 and five over HTTP/2 — that refuse a value the client wrote, one issued for a different cookie name,
and a name the request carried twice. Chunked request framing,
pipelining, smuggling defenses, 405
`Allow` generation, panic isolation, CORS, compression, duplicate header retention, multipart
uploads, dynamic static files with conditional and range requests, JWT validation, graceful
shutdown (now ending a live WebSocket session with a `1001` going-away frame), RFC 6455
WebSocket upgrade/echo/close, typed extractors (`Path`/`Query`/`Json`/
`Form`/`State`/`Extension`), request-scoped extensions (`Extensions`, per stream on HTTP/2),
user-defined and group-scoped middleware, the `TestClient` integration
harness, and opt-in adaptive scheduling (bounded admission queues, 503 load shedding,
per-kind capacity, baseline-derived work quotas — `aether-scheduler`, design and measured
policy comparison in `framework/docs/`) were exercised with unit tests, negative tests, and
wire-level tests. WebSocket compression and HTTP/3 are not implemented. TLS termination, SNI,
live certificate reload, HTTP/2 negotiated through ALPN `h2`, and cleartext HTTP/2 by prior
knowledge are, and were exercised against
clients that are not ours (Windows curl/Schannel, Python/OpenSSL, Node.js `http2`, python-h2 over
cleartext).

## Quick start

```bash
cd framework
cargo test --workspace
cargo run --release --bin basic_api     # http://127.0.0.1:3000
```

The verified toolchain is Rust 1.98.1 (edition 2024). See
[`framework/README.md`](framework/README.md) for application examples, configuration, and
security notes.

## Repository layout

| Path | Contents |
|---|---|
| `framework/` | Cargo workspace: 16 crates (`aether`, `aether-http`, `aether-router`, `aether-scheduler`, …) |
| `framework/docs/` | Architecture, API, deployment, audit, and benchmark documents |
| `docs/` | Specification and design documents (Arabic and English) |

## License

MIT OR Apache-2.0.
