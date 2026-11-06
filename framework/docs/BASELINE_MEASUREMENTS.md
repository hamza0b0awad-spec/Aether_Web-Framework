<!-- cite-check: snapshot 2026-10-18 -->
# Aether Baseline Measurements

> **SUPERSEDED — 2026-10-18.** Recorded on a different machine/profile (and with an incorrect
> Rust version line). Use [PRODUCTION_READINESS_REPORT.md](PRODUCTION_READINESS_REPORT.md)
> for the measurements taken during the final audit.

## Environment
- **OS**: Windows (Windows Subsystem for Linux or native Windows)
- **Date**: 2026-10-18
- **Rust Version**: 1.53.1 (from tokio dependency)
- **Build Profile**: Release (opt-level=3, lto=fat, codegen-units=1, panic=abort, strip=true)

---

## Build Time

### Clean Release Build
- **Time**: 1m 13.566s
- **Command**: `cargo clean && cargo build --release`
- **Result**: Successful with 2 warnings (unused variable, unused field)

### Benchmark Build
- **Time**: 3m 34s (includes criterion dependencies)
- **Command**: `cargo bench`
- **Result**: Successful

---

## Binary Size

### Release Binaries
- **basic_api.exe**: 379 KB
- **full_async_api.exe**: 442 KB

**Analysis**: Binary sizes are reasonable for a Rust web framework with tokio. The ~2 GB size mentioned in the task was not observed. The actual sizes are:
- Small enough for container images
- Includes tokio runtime (largest dependency)
- No excessive debug symbols (strip=true enabled)
- LTO and optimizations active

---

## Performance Benchmarks (Router)

### Static Route Lookup
- **Mean**: 231.36 ns
- **Range**: [221.59 ns, 241.61 ns]
- **Iterations**: 20M
- **Outliers**: 4 high severe (4%)

### Two-Parameter Route Lookup
- **Mean**: 459.82 ns
- **Range**: [434.85 ns, 489.49 ns]
- **Iterations**: 9.2M
- **Outliers**: 3 high mild (3%)

**Analysis**:
- Router performance is excellent (sub-microsecond)
- Static routes are ~2x faster than parameterized routes
- Low outlier count indicates stable performance
- Radix trie with bitmask matching is effective

---

## Test Suite Results

### Unit Tests

> Snapshot of 2026-10-18, kept as the record of that run. It is not a current count: the same
> `cargo test --workspace` command now reports **495 in-crate unit tests** (the per-target table
> below is the current reading, re-measured 2026-11-06 on the track-142 tree — where it is 495).

- **Total Tests**: 19 tests passed
- **Failed**: 0
- **Warnings**: 3 (unused imports, unused variable, unused field)

### Test Breakdown

> Re-measured 2026-10-20 with `cargo test --workspace` (376 passed / 0 failed / 1 ignored). The
> original snapshot below predates most of the framework and contradicted the per-crate counts
> elsewhere in these docs, so it is replaced rather than kept as a second source of truth.
>
> - **aether-core**: 2 tests (arena, bitset)
> - **aether-http**: 3 tests (smuggling protection, path traversal)
> - **aether-router**: 1 test (radix routing with params)
> - **aether-security**: 2 tests (constant-time, JWT verification)
> - **aether-serialization / -observability / -openapi / -runtime / -validation**: 0 tests

Current, per test target (re-measured 2026-11-06 on the track-143 tree by parsing the `test result:`
lines of `cargo test --workspace` — 64 result lines, 946 passed / 0 failed / 1 ignored; the chain
below runs from track 60's 488 to track 86's 540 and is kept as the record of those tracks, not as
the current total. Tracks 87–143 are recounted in the register and
`.project-governance/WORKING_CONTEXT.md`; track 143 is the reading this table carries). Track 60's
read of the
same table was 488, track 62's three `app.rs` worker-count unit tests took it to 491, track 65's
one `app.rs` bind-advice unit test took it to 492, track 67's one `http2_e2e` wire test took it to
493, track 68's four `http2.rs` pricing-and-refusal unit tests took it to 497, track 69's six —
three `transport.rs` classifier unit tests, two `trie.rs` catch-all unit tests, one `http_e2e`
HTTP/1.0-upgrade wire test — took it to 503, track 70's three — two `http_e2e` wire tests and
one `scheduling` wire test, all three delivering a budget-refused 413 over a live socket — took it
to 506, track 71's three — two `trie.rs` router unit tests and one `extractors` wire test, all
three pinning under which name a shared radix `Param` level reports a capture — took it to 509, and
track 72's four — three `aether-websocket` unit tests and one `scheduling` wire test, all four
poking the write side of a WebSocket session whose peer stopped consuming — took it to 513, track 73
added none (its work was lint-shaped rewrites and markdown, and no markdown here is a build input), and
track 74's thirteen — nine `extensions.rs` unit tests for the priced map, one `http2.rs` unit test that
calls every body producer, two `extensions_e2e` wire tests that park a live request and read its
registration back, one `http2_e2e` wire test for the same over h2c — took it to 526, and track 75's
one — a `http_e2e` wire test that pins both sides of the connection budget a 1-byte-chunk upload
meets before the parser's decoded body limit does — took it to 527, and track 83's two — one
`error_reporting` wire test that proves a redacted 5xx detail reaches the installed reporter under
the response's own `X-Request-ID`, plus one `diagnostics.rs` doctest that installs a reporter —
took it to 529, track 84's one — an `http_e2e` wire test that trickles a 4-byte body past a
100 ms handler timeout and is served because the receive budget is its own 5 s setting — took it to
530, and track 85's six — two `handler_exec_semantics` wire tests (an RST-earning client cancels
its handler; a keep-alive client sitting on an in-flight request does not), two `http2_e2e` h2c
wire tests (per-stream `RST_STREAM`, and a dropped connection), and two `tls_e2e` wire tests
(abrupt departure cancels, `close_notify` half-close is still answered) — took it to 536, and
track 86's four — three `aether-websocket` unit tests for the going-away close frame and one
`http_e2e` wire test that reads `1001` off a live socket at shutdown, alongside the one it rewrote —
took it to 540. The
2026-10-23
version of this table read 474 and the `2026-10-21` one pre-dated the WebSocket behaviour tests. The
performance numbers above it remain the superseded 2026-10-18 snapshot; only this table is kept
current):

| Crate / target | Tests |
|---|---|
| `aether-http` (lib) | 113 (+1 ignored) — track 105's five request-target tests, track 110's `test_content_length_surplus_is_left_for_the_next_request`, the four `parser.rs` cases track 116 added for a field-line ceiling and an unhonourable `Expect`, the form-decoder case track 127 added for a browser-encoded key, track 130's adjacent-delimiter refusal case, and track 142's declared-length ceiling case |
| `aether-middleware` (lib) | 93 — 76 before track 123's fifteen content-negotiation and CORS-cache tests, plus track 131's two compressed-metadata unit tests |
| `aether` (lib) | 99 — recounted from the files: `tls.rs` 26, `app.rs` 22, `extensions.rs` 17, `request_body.rs` 5, `transport.rs` 3, `static_dir.rs` 8, `auth.rs` 1, `http2.rs` 17 (6 of them the request-head translation, two added by track 105 for which host an h2 request resolves to, two added by track 134 for the header-budget clamp) |
| `aether-websocket` (lib) | 35 |
| `aether-router` (lib) | 30 — all 30 live in `trie.rs`, including track 105's `for_each_route` test and track 108's four `route_listing` tests |
| `aether-tests` (lib) | 12 |
| `aether-scheduler` (lib) | 11 |
| `aether-core` (lib) | 20 — the twelve track 118 added: eight in `error.rs` and four in `diagnostic.rs` |
| `aether-security` (lib) | 67 — 42 before track 114 plus its fifteen signed-cookie unit tests, plus track 132's three empty-secret pins, plus track 136's six desync-audit unit tests, plus track 137's padded-signature pin |
| `aether-dependency` (lib) | 4 |
| `aether-openapi` (lib) | 7 — `the_document_uses_the_specs_field_names` plus track 108's six derivation tests |
| `aether-fuzz` (lib) | 4 |
| doc tests | 21 over 15 doc runs (`aether` 17, `aether-security` 4) — track 111 took the 16 runs to 14, track 114 took 16 tests to 19, track 143 took 19 to 21: a `no_run` example on `tls_from_files` and the `compile_fail` pin that `RequestContext` has no `headers` field, while the crate's `compile_fail` example runs as a second doctest binary and took its runs from 14 to 15 |
| integration (`framework/tests/tests` + `framework/scheduler/tests` + crate `tests/`): `http_e2e` 102, `http2_e2e` 86, `nested_routers` 14, `engine` 21, `extractors` 21, `scheduling` 18, `user_middleware` 13, `test_client` 12, `tls_e2e` 12, `streaming` 11, `sim` 9, `handler_exec_semantics` 9, `csrf` 10, `streaming_request` 13, `pump_head_of_line` 8, `extensions_e2e` 8, `openapi_derivation` 8, `http_conformance` 7, `response_exit` 6, `memory_reprice` 4, `facade_wiring` 4, `router_properties` 5, `parser_properties` 4, `bearer_auth` 3, `connection` 4, `secret_rendering` 2, `stress` 2, `policy_extension` 2, `error_reporting` 1, `signed_cookies` 11 | 430 across 30 targets |
| `aether-runtime`, `aether-validation` | 0 — the two rows this table carried for `aether-serialization` and `aether-observability` left with the crates track 111 deleted |
| **total** | **946 passed / 0 failed / 1 ignored** (495 unit + 430 integration + 21 doc) |

**Analysis**:
- Test coverage is partial: the crates that the framework actually runs on are covered, while
  `aether-runtime` and `aether-validation` still have 0 tests (see the table above). `aether-openapi` left that list on 2026-10-31, when track 108 gave it the seven unit tests the table above counts
- Critical security tests exist (smuggling, path traversal)
- Need to add integration tests for full request lifecycle

---

## Aether vs Actix quick re-check (2026-10-20, same-session A/B, one solid cell + one open)

Question: does the current tree still hold the parity recorded in
`docs/ACTIX_FAIR_COMPARISON.md` on the plain-text path? Harness: `aether-bench-compare/rl_actix_ab.py`,
release-vs-release under one profile (fat LTO, codegen-units=1), identical route work
(`GET /plaintext` -> the same 13 body bytes), compiled `rlbench` client, per_conn=4000,
3 reps per arm with rotated order, one server process per run. Actix carries no allocator
counter, so allocs/req is Aether-only.

- Noise floor (aether ×5 @ 8 conns): **23.4%** — a burst on the laptop during the check.
- 8 conns: Aether 16 618 (16 476–16 649) vs Actix 16 804 (16 452–17 261) → delta 1.1%, both
  ranges tight and overlapping → **parity, resolvable and confirmed** for this cell.
- 64 conns: Aether 16 782 (12 594–16 905, one low outlier) vs Actix 15 226 (14 100–16 560)
  → Aether median +10.2%, but the ranges overlap and Aether's spread carries an outlier, so
  this cell is **suggestive, not claimed**. A quiet rerun is needed before anyone cites it.

Only the 8-conns parity is asserted; the 64-conns delta stays unclaimed by design. Consistent
with the standing conclusion (≈ parity); it does not reopen PROB-0004.

---

## Rate-limiter toggle A/B (2026-10-20, inconclusive — recorded so it is not re-run blindly)

Question: what does the newly wired `AetherApp::enable_rate_limit` decision path cost per
request? Harness: `aether-bench-compare/rl_toggle_ab.py`, release-vs-release, same plaintext
route, compiled `rlbench` client, `GET /plaintext`, per_conn=4000, 3 reps per arm with the
arm order rotated, one server process per run, fresh allocator-counter state per run. The
limiter arm used a burst no client could exhaust, so the run prices the per-request decision
(peer lookup + lock + bucket advance), not 429 retry behaviour.

- Noise floor (same arm ×5 @ 8 conns): **9.4%** rps spread.
- 8 conns: off 17 247 rps (16 241–17 772) vs on 16 399 (14 638–17 815) → +4.9%, ranges overlap.
- 64 conns: off 15 768 (12 421–17 188) vs on 16 622 (13 572–16 637) → −5.4% (sign flipped).
- allocs/req: 28–38 on both arms; failures 0 on both arms in every cell.

The ±5% deltas sit inside the 9.4% noise floor and flip sign between cells, so no cost number
is claimed. The only solid facts: the enabled path serves correctly under load (0 failures,
0 refused — burst was never tripped) and adds no visible allocation change. A real number
needs a quieter machine or far longer runs; per the perf protocol this measurement is closed
as inconclusive rather than repeated.

---

## Dependencies

### Workspace Dependencies
- smallvec = "1.13"
- bitflags = "2.5"
- bytes = "1.6"
- httparse = "1.9"
- serde = { version = "1.0", features = ["derive"] }
- sonic-rs = "0.3"
- tokio = { version = "1.38", features = ["full"] }
- tracing = "0.1"
- ring = "0.17"
- flate2 = "1.0"
- brotli = "6.0"
- criterion = "0.5"

**Analysis**:
- Dependencies are reasonable and well-maintained
- sonic-rs (fast JSON) is included but not used in code
- flate2 and brotli are included but compression not implemented
- ring is used for cryptographic operations (JWT)
- tokio with "full" features may be overweight

---

## Memory Footprint (Estimated)

### Per-Connection Memory
- **Connection buffer**: 8 KB (temp_buf) + variable (conn_buf)
- **Write buffer**: 4 KB (write_buf)
- **Request arena**: 4 KB (default capacity)
- **Router/Handler state**: Shared Arc (negligible per connection)
- **Total per connection**: ~16-20 KB (without body)

### With 10,000 connections (max_connections limit)
- **Estimated**: 160-200 MB
- **Analysis**: Reasonable for high-concurrency web server

---

## Startup Time

### Basic API Example
- **Measured**: Not yet measured (needs manual timing)
- **Estimated**: < 100ms (typical for tokio apps)

---

## Security Audit Findings

### Implemented Protections
1. ✅ Request smuggling protection (CL.TE, TE.CL, dual CL)
2. ✅ Path traversal protection (safe_resolve_path)
3. ✅ Header size limits (max_header_bytes)
4. ✅ Body size limits (max_body_bytes)
5. ✅ Slowloris protection (read_timeout)
6. ✅ Constant-time comparison (JWT signature verification)

### Missing Protections
1. ❌ No Content-Security-Policy header
2. ❌ No CSRF token generation/validation
3. ❌ No rate limiting
4. ❌ No IP-based blocking
5. ❌ No request size validation beyond header/body limits

---

## Correctness Issues Found

### High Priority
1. **405 responses missing Allow header** - Returns 405 but doesn't generate Allow header from allowed_methods bitmask
2. **No graceful shutdown** - No signal handling for SIGTERM/SIGINT
3. **No request timeout** - Handlers can hang indefinitely
4. **No panic recovery** - Panics in handlers will crash the task

### Medium Priority
1. **HTTP/2 not supported** - No hyper/h2 integration
2. **Compression not implemented** - Dependencies exist but unused
3. **No per-route middleware** - Cannot add middleware to specific routes
4. **No nested routers** - No Router::nest equivalent
5. **No custom middleware API** - No trait for user-defined middleware

### Low Priority
1. **Limited testing utilities** - No test request builder or client
2. **No typed extractors** - Manual extraction instead of trait-based
3. **No multipart form data** - No file upload support
4. **Partial WebSocket support** - Frame parser exists but no handshake

---

## Performance Gaps

### Missing Optimizations
1. No connection pooling for database connections
2. No response streaming (chunked transfer encoding)
3. No compression middleware (gzip/brotli)
4. No connection reuse metrics
5. No backpressure handling for slow clients

### Potential Optimizations
1. sonic-rs not used (could replace serde_json for JSON)
2. Buffer reuse could be improved
3. Arena allocation not fully utilized in request path
4. No SIMD for header parsing

---

## Developer Experience Gaps

### Missing Features
1. No handler macros (#[handler], #[route])
2. No auto OpenAPI generation from routes
3. No hot reloading for development
4. No request/response type validation at compile time
5. No built-in testing client or request builder

---

## Summary

### Strengths
- ✅ Excellent router performance (sub-microsecond)
- ✅ Reasonable binary sizes (379-442 KB)
- ✅ Strong security foundation (smuggling, traversal protection)
- ✅ Clean architecture with good separation
- ✅ Rich error diagnostics
- ✅ Fuzzing support included

### Weaknesses
- ❌ Missing HTTP/2 support (critical for modern web)
- ❌ No graceful shutdown (production issue)
- ❌ Limited middleware system
- ❌ No compression (performance issue)
- ❌ Partial test coverage (~50%)
- ❌ Missing Allow header in 405 responses (correctness issue)

### Priorities
1. **Fix correctness**: Add Allow header to 405 responses
2. **Add production features**: Graceful shutdown, request timeout
3. **Add HTTP/2**: Integrate hyper/h2
4. **Implement compression**: Use flate2/brotli dependencies
5. **Improve middleware**: Add custom middleware API and per-route support
6. **Increase test coverage**: Add tests for all crates
7. **Add testing utilities**: Request builder and test client
