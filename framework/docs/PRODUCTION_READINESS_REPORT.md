# Aether Production-Readiness Report

**Report date:** 2026-10-18
**Last revised:** 2026-10-30 (track 105)
**Workspace:** `aether-web-framework/framework`
**Toolchain:** rustc 1.98.1 (48a229cea 2026-09-01), cargo 1.98.1, edition 2024

> **This file holds a dated record and statements that are maintained, and it now says which is
> which.** §1, §4, §6, §7 and §10 describe the 2026-10-18 tree; §2 is its 2026-10-18 summary
> with the dated re-measurements appended to it; §3, §5, §8 and §9 are kept current; §11–§18
> are dated addenda describing the tree of their own date. A coordinate inside a
> dated section is written as "line N of <file>", optionally anchored to a commit, and is not
> re-checked; a coordinate in §3/§5/§8/§9 names the code as it is now and is pinned in
> `cite_check.py`, so moving that code fails the gate. The row-by-row authority for status remains
> [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md), which is where track 103's
> *Secret management* refusal is recorded.
>
> Until track 104 this file carried a `cite-check: snapshot` marker. The marker said the text was
> the unedited 2026-10-18 record; twelve commits after it (tracks 94 through 103) had edited the
> file, §3, §5, §8 and §9 among them, so it exempted live claims from the only check that
> would have caught a stale one — and all five live coordinates it hid had drifted. The
> register's B22 keeps that finding.

This report records what was actually changed, tested, and measured. Where a claim could not
be verified in this environment it is marked as such.

---

## 1. Environment

| Item | Value |
|---|---|
| OS | Windows 10.0.26200 (x64) |
| CPU | Intel Core i7-8565U, 4 cores / 8 logical threads, 1.80 GHz |
| RAM | 16,976,576,512 bytes (~15.8 GiB) |
| Runtime | Tokio multi-threaded work-stealing; not thread-per-core |
| Load generator | Python 3.14.7 raw sockets, loopback |
| Server build | `cargo build --release`, default profile for the external probe; workspace profile `lto="fat"`, `codegen-units=1`, `strip=true`, `panic="unwind"` for framework binaries |

The machine is a laptop, not a bare-metal benchmark host. Absolute throughput numbers are not
comparable with published framework benchmarks.

---

## 2. Executive summary

> **Re-measured 2026-10-20:** `cargo test --workspace` now passes **395 tests, 0 failures,
> 1 ignored**, and `cargo build --workspace --release` finishes with no warnings. **Re-measured
> again 2026-10-21 after the HTTP/2 stage: 426 passed, 0 failed, 1 ignored**, and once more the same
> day after request-scoped extensions landed: **445 passed, 0 failed, 1 ignored**; after §13
> (WebSocket upgrade requests inside the middleware chain, plus its review remediation):
> 452; after the handoff-edge round that settled forked chains, abandoned chains and the two
> header stores (same day): 455; after the four behaviour tests this audit opened (§14,
> 2026-10-22): 458; and after the `101` protocol-correctness fix that one of those tests led to
> (§15, same day): 459; and after PROB-0014 closed — the two HTTP/2 configuration wire tests plus one
> derivation unit test: **462 passed, 0 failed, 1 ignored**; after PROB-0008 closed — the
> capacity-based connection memory charge, three `app.rs` unit tests plus one `scheduling.rs` wire
> test (2026-10-23): 466; and after the two `aether-websocket` tests for the documented idle
> deadline, added later the same day: 468; and after the response-side write-block charge, the
> HTTP/2 block price and the handler-error body fix — three `app.rs` unit tests, one `scheduling`,
> one `http2_e2e` and one `http_e2e` wire test: **474 passed, 0 failed, 1 ignored**
> (271 unit + 198 integration across 15 targets + 5 doc tests; `http_e2e.rs` 46 raw-socket tests,
> `http2_e2e.rs` 30, `tls_e2e.rs` 9, `extensions_e2e.rs` 6, `scheduling.rs` 16). The counts in
> the bullets below are the 2026-10-18 snapshot and are deliberately left as written; §3 and §5
> carry the earlier state and §11 carries this one.

- **Correctness/security work is verified.** `cargo test --workspace` passes **182 tests, 0
  failures**, including **13 checked-in raw-socket end-to-end tests** in
  `framework/tests/tests/http_e2e.rs` (routing, pipelining, chunked bodies, duplicate headers,
  multipart, static ranges, CORS, limits, panic isolation, total deadline, graceful shutdown,
  and WebSocket upgrade/echo/close). An earlier external probe passed **86/86** checks against
  a release build, including smuggling attempts, oversized/malformed requests, and slow-client
  timeout.
- **Newly completed capabilities in this round:** RFC 7578 multipart parsing, RFC 9110
  `Range`/`If-Range` static-file serving, duplicate request-header preservation, testable
  server lifecycle with programmatic shutdown + SIGTERM, and full RFC 6455 WebSocket support
  (handshake validation, bounded connection driver, `app.ws` integration).
- **Highest-impact missing behavior implemented in earlier rounds:** chunked request decoding
  (with trailers and limits), dynamic static-directory serving (strong ETags, 304, traversal
  protection), JWT claim/algorithm validation (in the standalone `aether-security` crate — not
  reachable through `aether`), a real 405 `Allow` union, and release panic
  isolation (`panic = "unwind"`).
- **Performance (release build, hello-world route, measured before the multipart/WebSocket
  additions):** median **8.5k req/s at concurrency 1**, **22.5k req/s at concurrency 8**,
  **20.2k req/s at concurrency 64**; server CPU **0.09–0.13 ms per request**; idle RSS
  **6.9 MiB**, peak RSS **8.8 MiB**; zero failed requests. Multipart and WebSocket were not
  benchmarked, and no new optimization is claimed.
- **Earlier three-framework comparison is inconclusive:** Aether ~23.9k/20.8k, Axum
  ~25.3k/17.8k, Actix ~24.8k/25.6k req/s (plaintext/JSON), with 33–59 % run-to-run noise.
  **No framework is claimed to be universally faster.**
- **Binary size (fresh release build after WebSocket integration):** examples are
  **2,129,408 B** and **2,164,736 B** (~2.03/2.06 MiB), +92,160/+93,696 B over the
  pre-WebSocket build. Switching from `panic = "unwind"` to `"abort"` saved ~266–333 KB but
  was rejected because it turns a handler panic into a process-wide abort. The 2.8 GB
  `target/` directory is build artifacts, not executable size.
- **Aether speaks HTTP/1.1 and HTTP/2, over plaintext and over TLS, on one listener.** HTTP/3 is
  not implemented and is not claimed. TLS is terminated in-process by `aether::tls`; ALPN then picks
  HTTP/2 (`h2`) or HTTP/1.1 inside TLS, and a cleartext connection is HTTP/1.1 unless it opens with
  the HTTP/2 preface on a listener that called `AetherApp::enable_h2c`. This bullet said "HTTP/2 …
  not implemented" in the 2026-10-18 body; it was corrected on 2026-10-21 after the protocol stage
  landed — **§11 is the record of that stage**, and the matrix row at
  the matrix's HTTP/2 row is the current statement of what the server speaks.

---

## 3. Feature completeness

Full row-by-row matrix: [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).

Status counts (136 audited rows, recounted mechanically on 2026-11-04 by the command below; track 108 moved exactly one row — *OpenAPI generation* — from `PARTIALLY_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`, the document being derived from the registered routes now rather than built by hand; track 120 moved *Bounded response memory* from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`, its evidence having described the priced answer since track 74 while the status cell was never moved; track 107 moved *Nested routers / independent sub-apps* out of `NOT_IMPLEMENTED`; and the *Secret management* row still keeps `NOT_IMPLEMENTED` as a documented non-goal, track 103 having changed only its evidence cell):

| Status | Rows |
|---|---|
| VERIFIED_IMPLEMENTED | 123 |
| PARTIALLY_IMPLEMENTED | 3 |
| NOT_IMPLEMENTED | 8 |
| UNWIRED_IMPLEMENTATION | 0 |
| BROKEN | 0 |
| UNVERIFIED | 0 |
| NOT_APPLICABLE | 2 |

The command that produces this, so the next round re-measures instead of re-copying:

```
awk -F'|' '/^\|/ && NF>3 {gsub(/^ +| +$/,"",$3); print $3}' docs/FEATURE_COMPLETENESS_MATRIX.md \
  | grep -E '^(VERIFIED_IMPLEMENTED|PARTIALLY_IMPLEMENTED|NOT_IMPLEMENTED|UNWIRED_IMPLEMENTATION|BROKEN|UNVERIFIED|NOT_APPLICABLE)$' \
  | sort | uniq -c
```

That command is the one this report is held to, and it is why the numbers here move: the paragraph
that used to sit in this spot read 94 / 11 / 22 over 133 rows while the table beside it printed
97 / 12 / 20 / 3 / 0 / 0 / 2 over 134 rows, both labelled 2026-10-21 — two counts of one table in
one section, neither re-derived. It had drifted again by 2026-10-29, when the table said
117 / 6 / 11 and this paragraph said 117 / 5 / 12; both now come from one run of the command above.
The 2026-11-04 recount reads 123 / 3 / 8 / 0 / 0 / 0 / 2 over 136 rows (the 2026-10-31 run read 122 / 3 / 9, the 2026-10-30 run 121 / 4 / 9): tracks 88–100b moved rows
between statuses, track 101 moved *OCSP stapling / revocation* from partial to verified, track 102
flipped *Property-based tests* to verified, track 107 flipped *Nested routers / independent sub-apps* to verified, track 108 flipped *OpenAPI generation* from partial to verified, and the
count includes the row that had lost its closing pipe (131 rows on 2026-10-20 after the TLS work;
the previous 86/11/24/3/2 snapshot of 122–126 rows predated the transport/TLS stage).

The rows not fully delivered include HTTP/3,
HTTP/2 server push, the `Upgrade: h2c` exchange (declined, not overlooked),
and WebSocket compression (8
`NOT_IMPLEMENTED` and 3 `PARTIALLY_IMPLEMENTED` rows on
2026-11-04, re-read from the table above — this paragraph had carried 19/6, then 11/4, then 9/4, each one
a stale copy of the count it sits beside, the same drift the improvement register records; nested
routers and independent sub-apps left this list in track 107 when `Router`, `AetherApp::mount` and
the copy-on-write scope container landed, and response streaming plus the streamed static-file answer left the
list in track 99 when `Response::stream`/`stream_with_length` and the streamed `FileBody` landed — the
completeness-matrix row that carried that work is titled *Streaming / `sendfile`*, and what landed is a
64 KiB userspace read loop (`STATIC_CHUNK` in `framework/aether/src/static_dir.rs`), not the literal
`sendfile(2)` syscall, which stays on `framework/README.md`'s not-implemented list;
request-body streaming left it in track 100b when the HTTP/2 frontend stopped collecting DATA
before dispatch, so both frontends now hand the body to the handler as it arrives; directory
listing and pre-compressed static selection left the list
in track 97 when `StaticDirOptions` gained `directory_listing` and `precompressed`; custom and
per-route middleware left this list when
`route_sync_with`/`route_async_with`/`ws_with` landed in track 94, subprotocol negotiation
left it in track 95 when `ws_subprotocols` and the echoed `Sec-WebSocket-Protocol` line landed,
and graceful-close-on-shutdown left it on 2026-10-27 with the session's `1001` frame;
authentication middleware left it in track 98b when `require_bearer` and the verified `Principal`
landed, and CSRF left it in track 98c when `AetherApp::enable_csrf` and the signed
double-submit landed), and *OpenAPI generation* left the partial list on 2026-10-31 when `RadixRouter::route_listing` made the registered table enumerable and `AetherApp::openapi_spec` derived the document from it; the three that remain are *README / docs accuracy*, *Zero-copy head parsing* and *SIMD-style validation*.
Rate limiting, Host validation and TLS termination left this list on 2026-10-20: the first two are
opt-in `AetherApp` methods with unit and wire tests, TLS is `aether::tls` (rustls, SNI, live
reload) with 9 unit and 9 wire handshake tests. "Typed extractors" was stale in the previous
list — the matrix has had that row as `VERIFIED_IMPLEMENTED` since 2026-10-18.

---

## 4. Changes made

### 4.1 HTTP framing and parser (`http/src/parser.rs`)

- Split head parsing from body framing (`RequestHead` internal type). `RequestView::parse`
  retains its public signature and semantics (Content-Length framing).
- Added public `parse_request`, which returns the request view, an optional owned chunked
  body, and the exact consumed byte count.
- Implemented strict chunked decoding: size line before extensions, ≤16 hex digits, CRLF after
  each chunk, cumulative body budget, last-chunk trailers validated as header tokens, trailer
  count/byte budgets, HTTP/1.0 chunked rejection, exactly one `Transfer-Encoding` header, and
  only the `chunked` coding.
- Preserved and regression-tested smuggling defenses: dual `Content-Length`, `CL`+`TE`,
  malformed/overflowing lengths, unsupported codings, header count (including heap fallback
  above 64 slots) and header-byte budgets.
- 53 crate tests at the time of this change (110 passed plus the 1 ignored on this crate's lib line now, re-measured in the twenty-third reading below), including every-prefix partial input,
  pipelining offsets, exact-limit behavior, and hostile input.
- 2026-10-20: chunked decoding became resumable. `ChunkedBodyDecoder` holds the validated
  offset and the decoded bytes between reads, so a body arriving in pieces is decoded once
  instead of re-decoding the whole buffer after each read; `parse_request_incremental` is the
  connection-loop entry point and `parse_request` is a whole-buffer wrapper. The same change
  added `Expect: 100-continue` (`HTTP/1.1 100 Continue` on a complete head with the body still
  pending; an HTTP/1.0 head earns no interim and is served when its body arrives) and made
  the connection charge the decoder's held bytes to the connection's memory reservation.
- 2026-10-20: the response status line now carries the version of the request it answers
  (`Response::write_http1_to_for_request_version`, wired at the main response site), so a
  HTTP/1.0 client no longer reads `HTTP/1.1` and assume persistence the server has already
  ruled out. Answering 1.0 with 1.1 was legal (RFC 9110 §9.6 — a server must not *exceed* the
  greatest version it supports), so this is an interoperability fix, not a legality one. The
  default `max_headers_count` moved 64 → 100 field lines (hyper's ceiling); `max_header_bytes`
  is the budget that actually bounds per-line work, and the old default rejected legitimate
  clients without buying anything. Interim/error responses written before the request version
  is known (`100 Continue`, 408, 431/413/417/400, the scheduler's 503) still claim `HTTP/1.1`.
- 2026-11-04: `Expect` is now one reading shared by both frontends and both verdicts. The
  expectation is parsed as the RFC 9110 §10.1.1 extension list it is — comma-separated tokens
  spread over repeated field lines, optional whitespace trimmed, empty elements dropped, and any
  token the server cannot meet outweighing `100-continue` — and the refusal and the interim both
  come from that one reading, so a request cannot be refused for one reading of its list and
  answered with an interim for another. A head carrying an unmeetable token is refused
  (`AetherError::UnsupportedExpectation`) while the head is still all the server has seen: the
  HTTP/1 loop answers `417 (Expectation Failed)` with `Connection: close` before a body byte is
  read, and the HTTP/2 frontend takes the same verdict from the decoded header block before
  dispatch, streamed routes included. An HTTP/2 `100-continue` now earns a real informational
  `100` on its stream (RFC 9113 §8.2.3) — the first time this frontend called
  `send_informational`. What predates this change is the HTTP/1 interim itself and its
  per-request latch; what is new is that the latch is pinned, because with the latch removed
  every pre-existing HTTP/1 wire test still passed (measured: 88 of the target's 89) and only
  the piecewise-body test below failed.
  Pinned by `an_unsupported_expectation_is_refused_before_the_body_is_sent` on HTTP/1, by
  `an_unsupported_expectation_is_refused_over_http2_from_the_head`,
  `a_supported_expectation_earns_an_informational_response_over_http2` and
  `a_streamed_h2_route_answers_its_expectation_before_it_dispatches` on HTTP/2, by
  `a_streaming_route_sends_no_interim_for_a_request_that_arrived_whole` for the streamed
  no-interim rule, by `the_field_line_ceiling_outranks_the_expectation_over_http2` for the
  field-line ceiling outranking the expectation, and by the parser unit tests
  `test_unsupported_expectation_is_refused_from_the_head_ahead_of_the_body_budget`,
  `test_header_count_refusal_outranks_the_expectation_refusal` and
  `test_a_zero_header_budget_refuses_the_one_line_head_that_otherwise_parses`. The once-per-request
  latch half of the claim above is pinned only by the *pair*
  `a_body_that_arrives_in_pieces_earns_one_interim_not_one_per_piece` and
  `expect_100_continue_is_answered_before_the_body_arrives`; neither alone asserts the latch across a
  keep-alive pair, so no sentence may credit one of them with it.

### 4.2 Server (`aether/src/app.rs`, new `aether/src/static_dir.rs`)

- Integrates `parse_request`; chunked bodies are delivered as owned `Bytes`, framed bodies are
  copied out of the reusable connection buffer before handler execution.
- Removed a per-request header-map clone (map is moved into `RequestContext`).
- Fixed duplicate `Connection: close` on handler timeout when the client also requested close.
- Added a total receive deadline: each request receive phase is bounded by
  `max_request_time`; incomplete requests receive `408` and the connection closes. The 5 s
  per-read idle timeout remains. **Superseded 2026-10-27 (track 84):** the receive deadline is now
  its own setting, `max_receive_time` / `AETHER_MAX_RECEIVE_TIME_SECS`; `max_request_time` bounds
  handler execution only. **Narrowed again 2026-11-04:** the `408` this bullet promises holds
  while bytes keep arriving; **closed by track 117 (2026-11-04):** a request that stops arriving
  is also answered — the pending read is bounded by the shorter of the idle timeout and the
  receive budget's remainder, and expiry writes `408` + `Connection: close` (PROB-0034). An idle
  keep-alive socket has no request to answer and is still closed silently.
- Added `static_dir` and `static_dir_with_cache`: async file I/O, MIME detection, strong ETag,
  `Cache-Control`, conditional GET, no directory listing, traversal protection. (Both behaviours
  named as absent there stay the default; `static_dir_with_options` opts into a listing and into
  pre-compressed `.br`/`.gz` selection — track 97.) A directory that holds an `index.html` answers
  with that page in either mode since track 141; before it the lookup sat inside the listing
  branch, so only a listing-enabled tree could serve a site's home page.
- Retained earlier fixes: HEAD→GET fallback with body suppression, CORS preflight only when no
  user OPTIONS route exists, preflight requires a routable requested method, percent-decoded
  path parameters, IPv6-aware config, JSON-escaped error bodies, graceful Ctrl+C drain with
  `JoinSet` and a 10 s deadline.
- Added `bind`/`bind_with_config` returning `AetherServer` with `local_addr()` and
  `serve_with_shutdown(future)`, so tests and embedders can bind port 0, discover the assigned
  port, and stop the server deterministically. Unix installs a SIGTERM handler alongside
  Ctrl+C. The banner reports the actual bound port.
- Fixed limits desynchronization when a config was supplied at listen time, and the banner's
  route count now includes WebSocket routes.

### 4.3 Router (`router/src/trie.rs`)

- Fixed 405 `Allow` generation: the method bitmask now accumulates across **all**
  path-matching branches (static, param, catch-all), not just the first.
- Retained duplicate-route detection (panic with diagnostic), catch-all full-path capture, and
  static > param > catch-all backtracking. 24 tests (re-counted 2026-10-26 from the
  `aether-router` lib `test result:` line).
- Fixed the shared-`Param`-level name collapse (PROB-0031, 2026-10-26): a radix dynamic level is
  shared by sibling routes, so `/users/{user}/edit` and `/users/{id}` both descend through one
  `Param` child, and `RouteNode::param_names` keyed the declared name by *method* alone — each
  handler therefore read the other route's capture key, because `reject_duplicate` compares
  method+path at the leaf and the two paths differ. The node now stores the matched route's
  ordered dynamic names where its handler lives and `RadixRouter::resolve_keys` restamps the
  borrowed capture keys at the two `Search::Found` sites; the placeholder pushed while descending
  is the node's own first name, which only the leaf can correct.

### 4.4 Static files (`http/src/static_files.rs`)

- Strong, syntactically valid ETag (`"len-hash"`); removed the invalid embedded `W/`.
- `If-None-Match` supports lists, `*`, and weak comparison per RFC 9110 §13.1.2.
- Hardened `safe_resolve_path`: traversal, backslashes, drive letters, percent/backslash
  escapes; Windows alternate data streams (`:`), reserved device names (CON/PRN/AUX/NUL/
  COM1-9/LPT1-9), and trailing dots/spaces are rejected.
- Added negative and conditional-GET tests.

### 4.5 Panic isolation (`framework/Cargo.toml`)

- Release profile changed from `panic = "abort"` to `"unwind"`, with a comment explaining why:
  the spawned handler's `JoinError` becomes a 500 for that request; abort would kill every
  connection in the process. Verified on the release probe: `/panic` returns 500 and the
  server keeps serving.

### 4.6 WebSocket (`websocket/src/{lib,handshake,connection}.rs`, `aether/src/app.rs`)

- Strict frame validation: RSV bits must be zero; only defined opcodes; control frames must be
  FIN and ≤125 bytes; minimal length encodings enforced; checked length arithmetic.
- Added `ClientFrame`/`parse_client_frame` requiring the client mask and unmasking correctly.
  Public `WsFrameView` layout preserved. 8 parser tests.
- Added RFC 6455 opening handshake validation: GET only, `Upgrade: websocket` and
  `Connection: Upgrade` token matching (case-insensitive, comma lists), version 13 only, and a
  canonical base64 key that decodes to exactly 16 bytes. The accept value uses `ring`'s
  legacy SHA-1; the RFC sample vector is tested.
- Added a bounded connection driver: fragmentation assembly, interleaved control frames,
  automatic pong, close echo/status validation, UTF-8 validation, 16 MiB default message and
  frame caps, and header-first rejection of oversized frames before buffering their payloads.
- Added `AetherApp::ws(path, handler)` and `RouteGroup::ws`; the server writes the 101
  response, passes any pipelined bytes into the driver, and maps failures to 400/426/405 with
  `Sec-WebSocket-Version: 13` or `Allow: GET` where applicable.
- 22 crate tests (8 parser, 4 handshake, 10 driver) plus two raw-socket E2E tests — the
  2026-10-18 snapshot. The crate now holds 31 (8 parser, 4 handshake, 19 driver); the driver's
  later nine are the read and write deadlines of PROB-0030 (2026-10-26) and, on 2026-10-27, the
  going-away tests below.
- **Graceful close on shutdown (2026-10-27).** A session whose server starts shutting down is no
  longer aborted: the socket holds a clone of the same `watch` the serve loop watches, so a
  handler in `recv` sends one close frame with status `1001` and the reason
  `server shutting down`, shuts its write half down so the frame cannot be lost to an RST, and
  returns `Ok(None)` — the handler's loop ends on its own terms. The connection task waits
  `WS_SHUTDOWN_GRACE` (1 s) for that return and aborts only a handler that never comes back to
  `recv`, so shutdown stays bounded. Pinned at the driver by the three crate tests named in §3's
  fifteenth reading and on the wire by
  `shutdown_sends_a_going_away_close_frame_and_ends_the_session` and
  `a_session_that_never_returns_to_recv_is_still_aborted_at_shutdown`.

### 4.7 JWT (`security/src/jwt.rs`)

- Signature check now uses `ring::hmac::verify` (constant time).
- Added `JwtValidationOptions` and `verify`: algorithm pinning (`HS256`), `exp`/`nbf`/`iat`
  with leeway and saturating arithmetic, exact `iss`, and `aud` as string or array.
- `decode_base64url` rejects impossible lengths, non-zero trailing bits, and the standard
  alphabet. Original `verify_signature` retained. 7 tests.
- Added `serde_json` to `aether-security` (already present in the workspace lockfile).

### 4.8 Diagnostics and docs

- Startup banner now states the real engine/allocator: Tokio work-stealing, system allocator,
  configured worker count (not applied).
- Crate docs state HTTP/1.1-only, body copies, and unused arena.
- Root README replaced (it was an unrelated AI Studio template); `framework/README.md` created
  (the manifest referenced a missing README).
- Feature matrix rewritten. Old reports/marketing docs now carry explicit
  SUPERSEDED/NOT-VERIFIED banners: `FINAL_REPORT.md`, `CONSOLIDATED_FINAL_REPORT.md`,
  `BENCHMARKS.md`, `BASELINE_MEASUREMENTS.md`, `ARCHITECTURE.md`.
- Examples no longer interpolate user input into JSON; they build values with
  `serde_json::json!`.

### 4.9 Multipart form data (`http/src/multipart.rs`)

- New RFC 7578 parser exposed as `RequestContext::multipart()`.
- Parts borrow the request body (no file-content copy) and expose name, filename,
  content type, part headers, and bytes.
- Strict grammar: line-start delimiters, closing `--`, transport padding, preamble/epilogue,
  quoted-string parameter unescaping, folded/bare-LF header rejection, and `form-data`
  disposition validation. Filenames are metadata only and never touch the filesystem.
- Bounds: 1024 parts, 16 KiB of headers per part, 64 headers per part, 70-byte boundary (the
  RFC limit). 11 unit tests plus an end-to-end upload test.

### 4.10 Range requests (`http/src/static_files.rs`, `aether/src/static_dir.rs`)

- Added `parse_range_header`, strong-only `if_range_matches`, and
  `serve_static_with_range`.
- Supports one byte range (closed, open-ended, and suffix forms), `206 Partial Content`,
  `416 Range Not Satisfiable` with `Content-Range: bytes */len`, and `Accept-Ranges: bytes`.
- Malformed or multiple ranges are ignored (full 200 response); weak `If-Range` validators
  never match, so a changed representation is never sliced under a stale validator.
- A streamed file answer now plans its status and headers from metadata (`plan_file`), opens
  the payload only for a `200`/`206`, and reads it in 64 KiB chunks (`FileBody`); `HEAD`, `304` and
  `416` never read the file. The validator is weak (`W/"len-mtime"`) and published with
  `Last-Modified`, so `If-Range` matches the date and never the weak tag. 7 `static_files.rs`
  tests plus E2E checks for 206/304/416/If-Range and a live range resume.

### 4.11 Duplicate headers and checked-in end-to-end tests

- `RequestContext` keeps one private store: every field line of the request, keyed by the
  ASCII-lowercase field name and held in wire order. `header_values()` returns every line of
  a name, `header()` returns the first, and `headers()` enumerates one `(lowercase name,
  first line)` pair per field. No public map remains, so there is nothing a middleware can
  rewrite into a second view of the request.
- `Accept-Encoding` is negotiated across repeated field lines per RFC 9110 §5.3.
- Added `framework/tests/tests/http_e2e.rs`: 13 raw-TCP tests that bind port 0, read the real
  address, and exercise the wire protocol, including multipart, ranges, CORS, limits, 408,
  graceful shutdown, and WebSocket. These run in `cargo test --workspace`, so the claims no
  longer depend on an external harness.

---

## 5. Tests and build

### Commands and results

| Command | Result |
|---|---|
| `cargo test --workspace` (final tree) | **182 passed, 0 failed**, exit 0 |
| `cargo check --workspace --all-targets` | clean, exit 0 |
| `cargo build --release --workspace` | success (fresh run in this round; see §7) |
| `python aether_e2e_probe.py` (release probe, earlier round) | **86 checks, 86 passed, 0 failed** |
| Trickle-client probe (8 s receive deadline — then configured as `max_request_time`, earlier round) | `HTTP/1.1 408 Request Timeout` at 8.1 s |
| Startup to first accepted connection (3 runs, earlier round) | 80.9 / 76.6 / 83.0 ms |

**Re-measured on the final tree, 2026-10-21** (the four rows above are the 2026-10-18 snapshot and
are left as written):

| Command | Result |
|---|---|
| `cargo test --workspace` | **474 passed, 0 failed, 1 ignored**, exit 0 (271 unit + 198 integration across 15 targets + 5 doc; re-run three times on 2026-10-23 with identical per-target tallies, the last on the tree this slice ships. Earlier readings the same day: after PROB-0008 closed and after the two `aether-websocket` idle-deadline tests landed — 466 then 468. PROB-0008's four tests had taken the tree from 462, which is what the 2026-10-22 re-run after PROB-0014 and the `--all-targets` clippy fix measured; the `101` fix had taken the tree to 459) |
| `cargo check --workspace --all-targets` | clean, exit 0, zero warning lines (re-confirmed 2026-10-23 on the shipped tree: still exit 0 and still 0 `warning` lines) |
| `cargo build --workspace --release` | `Finished` in 2m 38s, **zero warnings** (that is the 2026-10-21 tree; not re-run since) |
| `cargo clippy --workspace` | exit 0, **0 errors and no lint from the `correctness` group** (re-measured cold on 2026-10-23 on the tree this slice ships: 46 lines beginning `warning` — `grep -cE '^warning'` — across 16 distinct style/pedantic lints such as `large_enum_variant`, `type_complexity`, `result_unit_err`, and 39 warnings in the seven per-crate `generated N warnings` summaries. **Read the caveat below: this number only reproduces from a cold cache.** The earlier "39 lint mentions" was a count on the 2026-10-21 tree, not this method, and is kept here only as history) |
| `cargo clippy --workspace --all-targets` | exit 0, **0 errors** (re-measured 2026-10-23 on the shipped tree from a cold cache: 97 top-level `warning` lines, 114 by the 22 per-target `generated N warnings` summaries; still no `correctness` lint, and `aether-websocket` contributes none). The one correctness error it carried was fixed 2026-10-22, see the note below |

**Re-measured on the final tree, 2026-10-26** (track 73 slice 2 — the four rows above are the
2026-10-21/23 readings and are left as written; both clippy rows are superseded, since the warnings they
counted are gone):

| Command | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | exit **0**, zero lines beginning `warning` or `error` — the first time this workspace has passed a fail-on-anything lint gate. Five lints are refused in-source as `#[expect(lint, reason = …)]` and named in `FEATURE_COMPLETENESS_MATRIX.md` item 16; an `#[expect]` that stops firing is itself a warning, so this exit code is also the proof that all five refusals are still earned |
| `cargo test --workspace` | **513 passed / 0 failed / 1 ignored**, exit 0, 50 per-target `test result:` lines |
| `cargo build --workspace --examples` | exit 0, zero `warning`/`error` lines (one lint batch edited `scheduler/examples/active_trace.rs`, and `cargo test` never compiles an example) |
| `cargo fmt --all -- --check` | exit **1**, 560 diff blocks over 65 files — recorded as red, see item 16 of `FEATURE_COMPLETENESS_MATRIX.md` |

The cache-sensitivity note below does not reach the 2026-10-26 lint row, because that row is an exit code
and not a count: a cached replay that had warnings to report would still hand them to `-D warnings` and
exit non-zero, and the run above was made on a warm cache (0.78 s) with nothing to replay. Counting
`warning` lines is what the note warns about, and this tree no longer has any to count.

**Re-measured on the track-73 slice-3 tree, 2026-10-26 — the `fmt` row is green.** The table above
stays as the slice-2 reading it is, including its red `fmt` row; this slice changed only Markdown, and
the gates were re-executed here anyway:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit **0**, zero diff blocks — the standing format debt is closed by seven per-crate batches (`63b6326..e7644b9`), not by a repo-wide sweep. `FEATURE_COMPLETENESS_MATRIX.md` item 16 records the chain it retired and the two things the green run does not buy (no CI enforces it; the reading is `rustfmt 1.9.0-stable` with no `rustfmt.toml`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit **0**, zero `warning`/`error` lines, re-run on this tree |
| `cargo test --workspace` | **513 passed / 0 failed / 1 ignored**, exit 0, 50 per-target `test result:` lines |
| `cargo build --workspace --examples` | exit 0 |
| `python .project-governance/tools/project_governance_bootstrap.py validate` | exit **1**, the same four standing lines (three `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`) — permanent by track 73 slice 1, and ratcheted |
| `python -B -m unittest discover -s .project-governance/tools/tests` | 42 / 0, exit 0 |

**One gate is no longer red, so the ledger line is the only standing red**, and it is red on purpose:
`validate` refuses ID reuse, and the three recycles are records in an append-only log that this
project will not rewrite. **Nothing pushed.**

**The clippy rows are cache-sensitive.** `cargo clippy` replays cached results per crate and does
not reprint the individual warnings, so the same command on a warm cache reported **9** `^warning`
lines and 2 distinct lints where the cold run reports 46 and 16 — the summaries (`generated N
warnings`) are replayed either way, which is why 39 is the only number that survives both states.
Measured 2026-10-23 by running the command twice on an untouched tree, then again after `touch` on
every `.rs` outside `target/`. Any re-measurement of these two rows must force the recompile first,
or it will report a falsely improved lint count. `cargo test --workspace` has no such sensitivity:
its tallies are printed by the test binaries, and the two runs above produced identical per-target
numbers.

The `--all-targets` failure this table used to record was `clippy::unused_io_amount` in the
single-byte TLS read loop of [`framework/tests/tests/tls_e2e.rs`](../tests/tests/tls_e2e.rs), at its line 452:
discarded the returned count, and that lint is **deny-by-default correctness**, so clippy reported
an error rather than a warning. It was **pre-existing at the `HEAD` of 2026-10-21** — pinned as
`git show 907dfcd^:framework/tests/tests/tls_e2e.rs`, whose line 452 is the `tls.read(&mut byte)`
loop (`907dfcd` is the commit that fixed it, so the bare `git show HEAD:` form this sentence used
stopped reproducing on 2026-10-22 and was replaced on 2026-10-23), it was in test code only, and `cargo test` passed
because rustc does not run the lint. It is now closed: that line reads
`tls.read_exact(&mut byte)`, matching the four `read_exact` calls the same file already used at
402–414, and the EOF that used to spin the loop forever now fails. Re-measured 2026-10-22:
`cargo clippy --workspace --all-targets` exits **0** with zero `error` lines, `cargo clippy
--workspace` is unchanged at **39** warnings, `cargo test --workspace` is **462 passed / 0 failed /
1 ignored**. The scope statement is therefore no longer a compromise: **both lib-scope and
whole-tree clippy are free of correctness lints, and `--all-targets` is usable as a gate.** The
exact warning count is not recorded here on purpose — it moves with the tree and with
how the summary is counted, and the invariant that matters is the one sentence above. The fix is
`read_exact(&mut byte)` in that test, tracked as a separate decision, and this is why
`cargo check --all-targets` and lib-scope clippy are the gates that can actually be used. That last
clause is the 2026-10-22 reading and is now the weakest of the three: since 2026-10-26 the
whole-workspace run with `-- -D warnings` exits 0, so the strictest lint gate is usable and is what the
table above records.

Test breakdown (2026-10-18 snapshot): `aether-http` 68, `aether-middleware` 35, `aether-websocket`
22, `aether-router` 16, `aether-tests` E2E 13, integration 12, `aether-core` 8, `aether-security`
7, `aether-dependency` 1. Re-measured mechanically from the per-target `test result:` lines of the
2026-10-22 run (the same awk-per-`Running`-header method, so the split is derived and not carried
over): unit **262** (`aether-http` 89, `aether-middleware` 59, `aether` 29,
`aether-router` 20, `aether-websocket` 23, `aether-scheduler` 11, `aether-tests` 12,
`aether-core` 8, `aether-security` 7, `aether-dependency` 4), integration **192** across the
remaining targets (`http_e2e` 45, `http2_e2e` 27, `engine` 21, `extractors` 18, `scheduling` 14,
`test_client` 12, `sim` 9, `tls_e2e` 9, `user_middleware` 8, `pump_head_of_line` 8,
`handler_exec_semantics` 7, `extensions_e2e` 6, `memory_reprice` 4, `policy_extension` 2,
`stress` 2), doc **5**. The earlier "doc tests contain no runnable examples" line was stale:
`Doc-tests aether` now passes 5, the `Extensions` examples among them.

Re-measured once more 2026-10-23 from the run that closed PROB-0008, same method: unit **266**
(`aether` 29 → 33, the three new `app.rs` buffer-charge tests), integration **195** (`http2_e2e`
27 → 29 and `scheduling` 14 → 15 — the HTTP/2 target had gained its two PROB-0014 wire tests
between these two readings, which is why this paragraph and the one above disagree), everything
else unchanged, doc **5**. Total for that reading: **466 passed / 0 failed / 1 ignored**.

Re-measured a third time on 2026-10-23, from the run that carries the two `aether-websocket` tests
for the documented idle deadline, same method: unit **268** (`aether-websocket` 23 → 25),
integration **195** across the same 15 targets and doc **5**, total
**468 passed / 0 failed / 1 ignored**.

Re-measured a fourth time the same day, from the run that carries the response-block charge, the
HTTP/2 block price and the handler-error body fix, same method and reproduced on a second
consecutive run with identical per-target tallies: unit **271** (`aether` 33 → 36 — the three new
`app.rs` write-charge tests), integration **198** (`http_e2e` 45 → 46, `http2_e2e` 29 → 30,
`scheduling` 15 → 16), doc **5**, 50 `test result:` lines, total
**474 passed / 0 failed / 1 ignored**. A fifth run the same day, after the pre-commit path
verification had added its one invariant comment to `app.rs`, reproduced the identical per-target
tallies — same 271 / 198 / 5, same 50 result lines, same total. A sixth, on the tree this slice
ships — after the rationale comment that had sat above the `E4003` emission at line 100 of
`aether/src/app.rs` on that tree was deleted, leaving that change a single line — reproduced them again, and the six new tests re-run one
at a time each report `1 passed`. The readings above are kept as dated history of the trees they
measured; the sixth describes the tree in the repository.

Re-measured a seventh time 2026-10-26, same method, on the tree that currently sits in the working
directory: unit **290** (the sixth reading had 271 — `aether` lib 36 → 50, `aether-router` 20 → 22),
integration **211** (`http_e2e` 46 → 51, `http2_e2e` 30 → 37, `scheduling` 16 → 17, `engine` 21,
`extractors` 18, `test_client` 12, `tls_e2e` 9, `sim` 9, `user_middleware` 8, `pump_head_of_line` 8,
`handler_exec_semantics` 7, `extensions_e2e` 6, `memory_reprice` 4, `policy_extension` 2, `stress` 2),
doc **5**, 50 `test result:` lines, total **506 passed / 0 failed / 1 ignored**
(`cargo test --workspace`, 2026-10-26, every `test result:` line parsed). These arrows are the drift
over every track since the sixth reading,
not one track's; the per-track splits are in `.project-governance/WORKING_CONTEXT.md`. The line above
that says the sixth describes the tree in the repository was true until those tracks landed; the
seventh held until track 71 landed.

Re-measured an eighth time 2026-10-26, same method (parsing every `test result:` line of
`cargo test --workspace`), on the tree that carries track 71's router fix: unit **292** (the seventh
reading had 290 — `aether-router` lib 22 → 24, the two new `trie.rs` naming tests), integration
**212** (`extractors` 18 → 19, the one new sibling-route wire test; every other target held its
seventh-reading count), doc **5**, 50 `test result:` lines, total **509 passed / 0 failed /
1 ignored** (`cargo test --workspace`, 2026-10-26, same parser). This describes the tree in the
repository now; the seventh superseded the sixth in the same way.

Re-measured a ninth time 2026-10-26, same method, on the tree that carries track 72's WebSocket
write bound: unit **295** (the eighth reading had 292 — `aether-websocket` lib 25 → 28, the three
write-bound tests), integration **213** (`scheduling` 17 → 18, the wire test that a stalled peer's
session releases its 33,570,816 B registration; every other target held its eighth-reading count),
doc **5**, 50 `test result:` lines, total **513 passed / 0 failed / 1 ignored**. That run's log is
not preserved — it lived in a scratch directory the owner clears — so the number stands on the method
above, and the durable per-target split is the table in
`framework/docs/BASELINE_MEASUREMENTS.md`. The eighth superseded the seventh in the same way.

Re-measured a tenth time 2026-10-26, same method, on the tree that carries track 74's extension
pricing: unit **305** (the ninth reading had 295 — ten new `aether` lib tests, nine in
`extensions.rs` for the charging rules and one in `http2.rs` for the body producers), integration
**216** (213 → 216: `extensions_e2e` 6 → 8 for the live-server readings, `http2_e2e` 37 → 38 for the
same charge taken over HTTP/2), doc **5**, 50 `test result:` lines, total **526 passed / 0 failed /
1 ignored**. The count is reproducible by a rule: 513 plus the thirteen `#[test]`/`#[tokio::test]`
attributes this slice added. This describes the tree in the repository now; the ninth superseded the
eighth in the same way.

Re-measured an eleventh time 2026-10-26, same method, on the tree that carries track 75's
wire-budget naming: unit **305** (the tenth reading's count held — this slice added no unit test, it
renamed nothing in code and moved no bound), integration **217** (216 → 217: `http_e2e` 51 → 52 for
`a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes`; every other target held its
tenth-reading count), doc **5**, 50 `test result:` lines, total **527 passed / 0 failed / 1 ignored**.
The count is reproducible by the same rule: 526 plus the one `#[tokio::test]` attribute this slice
added. This describes the tree in the repository now; the tenth superseded the ninth in the same way.

Re-measured a twelfth time 2026-10-27, same method, on the tree that carries track 83's error
reporter: unit **305** (held — the slice added no unit test), integration **218** (217 → 218: the
new `error_reporting` target's one wire test), doc **6** (5 → 6: the runnable example in
`diagnostics.rs`), 51 `test result:` lines, total **529 passed / 0 failed / 1 ignored**. The count is
reproducible by the same rule: 527 plus the one `#[tokio::test]` attribute and the one doctest this
slice added. This describes the tree in the repository now; the eleventh superseded the tenth in the
same way.

Re-measured a thirteenth time 2026-10-27, same method, on the tree that carries track 84's receive
budget split: unit **305** (held — the slice added no unit test), integration **219** (218 → 219:
`http_e2e`'s `a_slow_upload_is_not_bounded_by_the_handler_timeout`, which trickles a body past a
100 ms handler timeout and is served because the receive budget is its own setting), doc **6**
(held), 51 `test result:` lines, total **530 passed / 0 failed / 1 ignored**. The count is
reproducible by the same rule: 529 plus the one `#[tokio::test]` attribute this slice added. This
describes the tree in the repository now; the twelfth superseded the eleventh in the same way.

Re-measured a fourteenth time 2026-10-27, same method, on the tree that carries track 85's
client-disconnect cancellation: unit **305** (held — the slice added no unit test), integration
**225** (219 → 225: `handler_exec_semantics` 7 → 9 for
`a_pipelined_request_does_not_look_like_a_disconnect` and
`a_client_that_stays_connected_is_not_cancelled`; `http2_e2e` 38 → 40 for
`a_stream_reset_cancels_the_running_handler_and_keeps_the_session` and
`a_dropped_h2_connection_cancels_its_running_handlers`; `tls_e2e` 9 → 11 for
`a_tls_client_that_leaves_cancels_the_running_handler` and
`a_tls_client_that_half_closes_still_gets_its_response`; every other target held), doc **6**
(held), 51 `test result:` lines, total **536 passed / 0 failed / 1 ignored**. The count is
reproducible by the same rule: 530 plus the six `#[tokio::test]` attributes this slice added.

Re-measured a fifteenth time 2026-10-27, same method, on the tree that carries track 86's
going-away close frame: unit **308** (305 → 308: `aether-websocket` 31, three tests —
`a_server_shutdown_closes_a_waiting_session_with_going_away`,
`a_shutdown_that_started_before_the_upgrade_still_closes_the_session` and
`a_session_closed_by_shutdown_refuses_further_sends`), integration **226** (225 → 226:
`http_e2e` 53 → 54, `a_session_that_never_returns_to_recv_is_still_aborted_at_shutdown` added
alongside the rewritten `shutdown_sends_a_going_away_close_frame_and_ends_the_session`, which
replaced `shutdown_aborts_a_live_websocket_session_without_a_close_frame` one for one; every other
target held), doc **6** (held), 51 `test result:` lines, total **540 passed / 0 failed / 1
ignored**. The count is reproducible by the same rule: 536 plus the four `#[tokio::test]`
attributes this slice added net. That reading described the track-86 tree; tracks 87–105 landed
after it and the sixteenth reading below supersedes it.

Re-measured a sixteenth time 2026-10-30, same method, on the tree that carries track 105's
request-target conformance: unit **416**, integration **319** across 27 targets, doc **15** over
16 doc runs, 64 `test result:` lines, total **750 passed / 0 failed / 1 ignored**. The unit total
is larger than the fifteenth reading's 308 mainly because tracks 87–104 added in-crate tests
(`aether-security` 42, `aether-middleware` 76), and track 105 added five `aether-http` parser
tests, one `trie.rs` test and two `aether/src/http2.rs` tests for which host an HTTP/2 request
resolves to, one of them replacing the test that pinned the opposite precedence; integration grew by the new `http_conformance` target (7) and the
targets tracks 87–104 filled in. That reading described the track-105 tree; the seventeenth reading
below supersedes it.

Re-measured a seventeenth time 2026-10-30, same method, on the tree that carries track 106's HTTP/2
request-target conformance: unit **416** (unchanged — track 106 added no in-crate test), integration
**325** across the same 27 targets, doc **15** over 16 doc runs, 64 `test result:` lines, total
**756 passed / 0 failed / 1 ignored**. Integration moved only in `tests/http2_e2e.rs`, 49 → 55: the
server-wide `OPTIONS *` answer re-checked over h2, a `*` on a method that is not `OPTIONS`, a `:path`
outside the forms RFC 9113 §8.3.1 allows, a `CONNECT` answered from the head, and two wire tests that
read the `Host` a handler and the host policy both see after the `:authority` fold. That reading
described the track-106 tree; the eighteenth reading below supersedes it.

Re-measured an eighteenth time 2026-10-30, same method, on the tree that carries track 107's
group-scoped state and mountable sub-apps: unit **420**, integration **340** across 28 targets, doc
**16** over 16 doc runs, 65 `test result:` lines, total **776 passed / 0 failed / 1 ignored**. The
unit total grew 416 → 420 with the four `app.rs` tests of the scoped overlay (it answers its own
type and the app answers the rest, one chain entry for a real scope and none for an empty one, the
overlay's position in the chain, the copy-on-write container), which took the `aether` lib total
92 → 96 and its `app.rs` line 17 → 21. Integration grew 325 → 340 as a new target, 14 wire tests in
`tests/nested_routers.rs` over a real TCP listener, plus one cleartext-h2c mount test in
`tests/http2_e2e.rs` (55 → 56); doc grew 15 → 16 with the `Router` doctest, so the `aether` crate's
doc run is 13 where the two readings above imply 12 — the only doctest either reading added. That reading described the track-107 tree; the nineteenth reading below supersedes it.

Re-measured a nineteenth time 2026-10-31, same method, on the tree that carries track 108's derived OpenAPI
document: unit **430**, integration **349** across 29 targets, doc **16** over 16 doc runs, 66 `test result:`
lines, total **795 passed / 0 failed / 1 ignored**. The unit total grew 420 → 430 with four `trie.rs` tests for
the route listing (each registration comes back with its own declared template, a dynamic level shared by two
routes carries each route's own capture name, a sloppy declaration reports its canonical form, and the listing
does not depend on registration order) and six `aether-openapi` tests for the derived document (a catch-all
becomes a path parameter, the document is independent of input order, colliding `operationId`s stay unique, a
WebSocket registration carries `101`/`400`/`426`, nothing in the document claims a body schema, and a
malformed slot declares no parameter); the `aether-router` lib line moved 26 → 30 and `aether-openapi` 1 → 7.
Integration grew 340 → 349: `router/tests/router_properties.rs` gained its fifth property — the listing is the
table, and each name it reports is the name `find` extracts — and a new target, `tests/openapi_derivation.rs`,
carries eight app-level tests over `AetherApp::openapi_spec`. Doc held at 16. That reading described the
track-108 tree; the twentieth reading below supersedes it.

Re-read 2026-10-31 for track 109, which moved no cargo count at all: it changed governance tools and
documents only, so the nineteenth reading's total was re-measured on this tree rather than replaced
— **795 passed / 0 failed / 1 ignored** — and the per-suite figures the root README prints were
re-derived here too (73 `http_e2e`, 56 `http2_e2e`, 18 `scheduling`, 4 plus 5 property, 8
`openapi_derivation`). What did move sits outside that count: the governance suite went 111 → 112,
because `cite_check.py` gained a second pass that resolves every name the 27 live documents assert
against the tree's own sources, and it reported one phantom API the pinned coordinates had kept
green — see the improvement register's track-109 record.

Re-measured a twentieth time 2026-10-31, same method, on the tree that carries track 110's request
declared-length pins: unit **431**, integration **367** across 29 targets, doc **16** over 16 doc runs,
66 `test result:` lines, total **814 passed / 0 failed / 1 ignored**. The unit total grew 430 → 431 with
one `http/src/parser.rs` test (`test_content_length_surplus_is_left_for_the_next_request`), which exists
because an injected mutation proved the buffered reader's surplus rule was not pinned in its
body-carrying shape. Integration grew 349 → 367 and no new target was needed: `tests/http_e2e.rs`
73 → 78 (the HTTP/1 half of the same rule — the surplus served as the next request, the surplus that
is not a request refused `400` then closed, ASCII OWS padding served, a non-ASCII padding refused, a
repeated length refused even when it agrees), `tests/http2_e2e.rs` 56 → 67 (eight hand-framed
`RST_STREAM(PROTOCOL_ERROR)` observations for a body that does not match its promise, plus the two
served shapes — agreeing repeats, which come back to the handler as `values=["5", "5"]` — and the
streaming-ceiling `413` answered before dispatch), and `tests/streaming_request.rs` 8 → 10 (the same
two HTTP/1 halves over the streamed lane). Doc held at 16. Track 110 found **no defect**: every shape
RFC 9113 §8.1.1 lists is either enforced by this framework's own code, where the wire test proves it
rejects the mutant, or enforced by the `h2` crate's receive path before the head reaches this
framework, which is what the division-of-responsibility note in `framework/aether/src/http2.rs` now
names. That reading described the track-110 tree; the twenty-first reading below supersedes it.

Re-measured a twenty-first time 2026-11-02, same method, on the tree that carries track 114's signed
cookies: unit **446**, integration **378** across 30 targets, doc **19** over 14 doc runs, 63
`test result:` lines, total **843 passed / 0 failed / 1 ignored**. Unit grew 431 → 446 with the fifteen
`aether-security` cookie tests — the tag bound to its cookie name, the four shapes a presented value
comes in, a payload that is not UTF-8, an empty secret, a `Debug` that prints no payload — which took
that crate's lib line 42 → 57. Integration grew 367 → 378 in one new target, `tests/signed_cookies.rs`,
over a real TCP listener. Doc grew 16 → 19: two `aether/src/cookie.rs` doctests and the `compile_fail`
that keeps `CookieSigner` unprintable. The line count fell 66 → 63 because track 111 deleted two crates
that each asked for a unit run and a doc run; the two lanes left the census, the suite kept its tests.
This described the tree until track 115; the twenty-second reading below supersedes it.

Re-measured a twenty-second time 2026-11-03, same method, on the tree that carries track 115's HTTP/2
signed-cookie wire tests: unit **446**, integration **383** across 30 targets, doc **19** over 14 doc
runs, 63 `test result:` lines, total **848 passed / 0 failed / 1 ignored**. Five tests moved and exactly
one target moved with them — `tests/http2_e2e.rs` 67 → 72 — because the work was tests, not code: no
crate gained a unit test, no lane entered or left the census, and the doc lane still holds track 114's
three cookie examples. This described the tree until track 116; the twenty-third reading below
supersedes it.

Re-measured a twenty-third time 2026-11-04, same method, on the tree that carries track 116's
`Expect` work: unit **450** over 19 unit runs, integration **406** across 30 targets, doc **19** over
14 doc runs, 63 `test result:` lines, total **875 passed / 0 failed / 1 ignored**. The cross-check
still agrees: the 63 `running N tests` lines sum to **876**, which is 875 plus the single ignored
test. Unit grew 446 → 450 — four new `aether-http` cases, taking that crate's lib line to 110 passed
plus 1 ignored — and integration grew 383 → 406 in exactly the three targets this track touched:
`tests/http_e2e.rs` 78 → 89, `tests/http2_e2e.rs` 72 → 80, `tests/streaming_request.rs` 10 → 12. Each
prior figure was re-derived by counting `#[test]` attributes in that file at the committed tree and
the same count on this one reproduces the log's number for all three (89, 80, 12), which is what makes
the split a measurement rather than an inference from the total. No lane entered or left the census,
and the doc lane still holds track 114's three cookie examples. This described the tree until track 117; the twenty-fourth reading below
supersedes it.

Re-measured a twenty-fourth time 2026-11-04, same method, on the tree that carries track 117's
receive-timeout answer: unit **450** over 19 unit runs (held), integration **410** across 30 targets,
doc **19** over 14 doc runs, 63 `test result:` lines, total **879 passed / 0 failed / 1 ignored**.
The cross-check agrees: the 63 `running N tests` lines sum to **880**, which is 879 plus the single
ignored test. Integration grew 406 → 410 in exactly one target — `tests/http_e2e.rs` 89 → 93 — the
four tests the fix brought: three shapes of a request that stops arriving (half a head, a body short
of its declared length, a chunked body short of its final chunk), each answered `408` with
`Connection: close`, plus the idle keep-alive socket that must still close with no answer. The unit,
doc and lane counts did not move, because the change is one helper and one read bound in
`aether/src/app.rs`. Every prior figure was re-derived by counting `#[test]` attributes at the
committed tree, and the same count on this one reproduces the log's 93 for `http_e2e`. This described the tree
until track 118; the twenty-fifth reading below supersedes it.

Re-measured a twenty-fifth time 2026-11-04, same method, on the tree that carries track 118's
diagnostic corrections: unit **463** over 19 unit runs, integration **410** across 30 targets,
doc **19** over 14 doc
runs, 63 `test result:` lines, total **892 passed / 0 failed / 1 ignored**.
The cross-check agrees: the 63 `running N tests` lines sum to **893**, which is 892 plus the
single ignored
test. Unit grew 450 -> 463 in exactly two crates: `aether-core`'s lib line 8 -> 20
(eight `error.rs` tests and four `diagnostic.rs` tests over every variant and every code) and
`aether`'s lib line 96 -> 97 (the twelve-row status, code, detail and redaction cross-product).
Integration held at 410 because the only wire change adds assertions inside an existing test,
and the doc lane held because the change writes no example. Every prior figure was re-derived by
counting `#[test]` attributes at the committed tree and the same count on this one reproduces the
log's numbers — `error.rs` 0 -> 8, `diagnostic.rs` 0 -> 4, `app.rs` 21 -> 22, `http_e2e.rs`
93 -> 93. This described the tree until track 123; the twenty-sixth reading below supersedes it.

Re-measured a twenty-sixth time 2026-11-05, same method, on the tree that carries track 123's content-negotiation and CORS-cache corrections: unit **478** over 19 unit runs, integration **414** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **911 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **912**, which is 911 plus the one ignored test. Unit grew 463 -> 478 in exactly one crate: `aether-middleware`'s lib line 76 -> 91 (the content-negotiation cases — identity refused with no coding and with `*;q=0`, a selected coding larger than the original, a body below the size floor, a range, a streamed body, a handler-chosen coding, an empty body and a bodyless status, plus the metadata a refusal strips — and the three CORS refusal variances). Integration grew 410 -> 414 in exactly one target — `tests/http_e2e.rs` 93 -> 97 — the four wire pins this track added: a denied origin still varying on `Origin`, a refused preflight varying on the fields that decided it, an approved preflight for an unroutable method answered without CORS headers, and a refused identity answered `406` on the wire beside the coded answer that replaces a larger-but-acceptable body. No lane entered or left the census, and the doc lane held because no example changed. Every prior figure was re-derived by counting `#[test]` attributes at the committed tree and the same count on this one reproduces the log's numbers — `http_e2e.rs` 93 -> 97, and the fifteen in `middleware/src/compression.rs` and `cors.rs` take the middleware lib line 76 -> 91. This reading describes the tree that carried track 123; the twenty-eighth reading below supersedes it.

Re-measured a twenty-seventh time 2026-11-05, same method, on the tree that carries track 125's five HTTP/2 wire pins: unit **478** over 19 unit runs, integration **419** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **916 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **917**, which is 916 plus the one ignored test. Integration grew 414 -> 419 in exactly one target — `tests/http2_e2e.rs` 80 -> 85 — the five wire pins this track added: a denied origin still varying on `Origin` over h2, a refused preflight varying on the three fields that decided it, an approved preflight for an unroutable method answered without CORS headers, a refused identity answered `406` while the coded answer that replaces a larger-but-acceptable body stays `200` and starts with a complete gzip member, and an approved preflight answered `204` with its allow fields beside a refused origin on the same connection. No lane entered or left the census, and the doc lane held because no example changed. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the tree that carries track 125, and the twenty-eighth reading below supersedes it.

Re-measured a twenty-eighth time 2026-11-05, same method, on the tree that carries track 126's split-cookie-field pins: unit **478** over 19 unit runs, integration **421** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **918 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **919**, which is 918 plus the one ignored test. Integration grew 419 -> 421 in exactly two targets — `tests/http_e2e.rs` 97 -> 98 and `tests/http2_e2e.rs` 85 -> 86 — the two wire pins this track added: a cookie name carried on a second `Cookie` field line is a cookie the client sent, over HTTP/1.1 with the first occurrence still winning across lines and a case-variant name still absent, and the same jar split across repeated fields over h2. No lane entered or left the census, and the doc lane held because no example changed. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the tree that carries track 126, and the twenty-ninth reading below supersedes it.

Re-measured a twenty-ninth time 2026-11-05, same method, on the tree that carries track 127's form-rule key decoding: unit **479** over 19 unit runs, integration **422** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **920 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **921**, which is 920 plus the one ignored test. Unit grew 478 -> 479 in exactly one crate — `aether-http`'s lib line 110 -> 111 — the borrowing form decoder's own test: `form_decode_cow` applies the same rule as `form_decode` and borrows only when the input holds neither `+` nor a `%` escape. Integration grew 421 -> 422 in exactly one target — `tests/http_e2e.rs` 98 -> 99 — the one wire pin this track added: a browser-encoded parameter name (`first+name`) is a name `query_decoded` finds, beside `%20`, a literal `%2B`, a name carrying both encodings, and the raw accessor still answering the raw string. No lane entered or left the census, and the doc lane held because no example changed. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the tree that carries track 127, and the thirtieth reading below supersedes it.

Re-measured a thirtieth time 2026-11-05, same method, on the tree that carries track 128's out-of-root link pin: unit **479** over 19 unit runs, integration **423** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **921 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **922**, which is 921 plus the one ignored test. Unit held at 479 and the doc lane held at 19 over 14 runs, because this track changed no crate source and no example. Integration grew 422 -> 423 in exactly one target — `tests/http_e2e.rs` 99 -> 100 — the one wire pin this track added, and it is a pin rather than a fix: the containment check in `safe_resolve_path` already canonicalises the resolved path and refuses one outside the root, but nothing in the suite exercised it, so a directory junction pointing out of the static root and a directory junction pointing inside it are now both answered on the wire (404 and 200 respectively), and two mutants of that check — dropping the rejection, and comparing the lexical join instead of the canonical target — each turn the test red. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-128 tree, and the thirty-second reading below supersedes it.

Re-measured a thirty-second time 2026-11-05, same method, on the tree that carries track 130's multipart adjacent-delimiter refusal: unit **480** over 19 unit runs, integration **424** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **923 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **924**, which is 923 plus the one ignored test. Unit grew 479 -> 480 in exactly one crate — `aether-http` 111 -> 112 passed — the one unit pin this track added: two MIME delimiter lines with nothing between them describe no part, and the parser now hands that empty part to the header-block refusal instead of slicing its byte range backwards and panicking. Integration held at 424 and the doc lane held at 19 over 14 runs, because the wire pin this track added rides inside an existing `http_e2e` test and no example changed. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-130 tree, and the thirty-third reading below supersedes it.

Re-measured a thirty-third time 2026-11-05, same method, on the tree that carries track 131's compressed-metadata strip: unit **482** over 19 unit runs, integration **425** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **926 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **927**, which is 926 plus the one ignored test. Unit grew 480 -> 482 in exactly one crate — `aether-middleware` 91 -> 93 passed — the two unit pins this track added: a body the middleware replaces loses the strong `ETag`, the integrity digests and the `Accept-Ranges` promise that described the bytes it no longer carries, while a body it leaves alone keeps its validator. Integration grew 424 -> 425 in exactly one target — `tests/http_e2e.rs` 100 -> 101 — the one wire pin this track added: the gzip answer to a static file drops the validator the identity answer keeps, so a compressed cache entry cannot be revalidated against, or range-served from, the stored identity bytes. No lane entered or left the census, and the doc lane held because no example changed. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-131 tree, and the thirty-fourth reading below supersedes it.

Re-measured a thirty-fourth time 2026-11-05, same method, on the tree that carries track 132's empty-secret refusal: unit **485** over 19 unit runs, integration **425** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **929 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **930**, which is 929 plus the one ignored test. Unit grew 482 -> 485 in exactly one crate — `aether-security` 57 -> 60 passed — the three unit pins this track added: the constructor refuses an empty secret, a short non-empty secret is still accepted (the refusal is emptiness, not length), and the bearer layer's constructor reaches that refusal rather than building a validator anyone can sign for. Integration held at 425 and the doc lane held at 19 over 14 runs, because this track changed no example and added no wire test. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-132 tree, and the thirty-fifth reading below supersedes it.

Re-measured a thirty-fifth time 2026-11-06, same method, on the tree that carries track 133's case-insensitive JSON media-type check: unit **485** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **931 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **932**, which is 931 plus the one ignored test. Integration grew 425 -> 427 in exactly one target — `tests/extractors.rs` 19 -> 21 — the two wire pins this track added: a structured-syntax media type is accepted whichever case its `+json` suffix carries, and a request that carries no `Content-Type` is still parsed, as the type's own doc states. Unit and the doc lane held, because this track changed no in-crate test and no example. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-133 tree, and the thirty-sixth reading below supersedes it.

Re-measured a thirty-sixth time 2026-11-06, same method, on the tree that carries track 134's saturating header-budget clamp: unit **487** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **933 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **934**, which is 933 plus the one ignored test. Unit grew 485 -> 487 in exactly one crate — `aether` 97 -> 99 passed — the two unit pins this track added: the h2 header-list ceiling saturates at `u32::MAX` instead of wrapping a large configured budget to zero, checked at the ceiling and just past it. Integration and the doc lane held, because this track changed no wire behaviour a test can reach: the misconfiguration that triggers the wrap reserves a 4 GiB connection budget. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-134 tree, and the thirty-seventh reading below supersedes it.

Re-measured a thirty-seventh time 2026-11-06, same method, on the tree that carries track 136's desync-audit alignment: unit **493** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **939 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **940**, which is 939 plus the one ignored test. Unit grew 487 -> 493 in exactly one crate — `aether-security` 60 -> 66 passed — the six unit pins this track added: the audit refuses a repeated `Transfer-Encoding`, a coding that is not exactly one `chunked`, and a `Content-Length` that is not a decimal number, while the OWS and case spellings the parser accepts stay accepted. Integration and the doc lane held, because the one wire change rides inside the existing facade test and no example changed. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-136 tree, and the thirty-eighth reading below supersedes it.

Re-measured a thirty-eighth time 2026-11-06, same method, on the tree that carries track 137's padded-signature refusal: unit **494** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **940 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **941**, which is 940 plus the one ignored test. Unit grew 493 -> 494 in exactly one crate — `aether-security` 66 -> 67 passed — the one unit pin this track added: a signature segment that names the canonical tag plus trailing base64 padding is refused rather than trimmed into the same credential. Integration and the doc lane held, because the one wire assertion rides inside the existing facade test and no example changed. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-137 tree, and the thirty-ninth reading below supersedes it.

Re-measured a thirty-ninth time 2026-11-06, same method, on the tree that carries track 139's work ordering at the front of the request path: unit **494** over 19 unit runs, integration **428** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **941 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **942**, which is 941 plus the one ignored test. Integration grew 427 -> 428 in exactly one target — `tests/csrf.rs` 9 -> 10 — the one wire pin this track added: the rate limiter's refusal outranks the CSRF gate, so a request over budget is answered `429` rather than `403`, and a safe request the limiter refuses is handed no token. Unit held at 494 and the doc lane held at 19 over 14 runs, because this track changed no in-crate test and no example. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-139 tree, and the fortieth reading below supersedes it.

Re-measured a fortieth time 2026-11-06, same method, on the tree that carries track 141's index page served with listings off: unit **494** over 19 unit runs, integration **429** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **942 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **943**, which is 942 plus the one ignored test. Integration grew 428 -> 429 in exactly one target — `tests/http_e2e.rs` 101 -> 102 — the one wire pin this track added: a directory that holds an `index.html` answers with that page whether listings are on or off, and the tree root answers there too when it ships one. Unit held at 494 and the doc lane held at 19 over 14 runs, because this track changed no in-crate test and no example. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-141 tree.

Re-measured a forty-first time 2026-11-06, same method, on the tree that carries track 142's declared-length ceiling left to the route: unit **495** over 19 unit runs, integration **430** across 30 targets, doc **19** over 14 doc runs in that lane, 63 `test result:` lines, total **944 passed / 0 failed / 1 ignored**. The cross-check agrees: the 63 `running N tests` lines sum to **945**, which is 944 plus the one ignored test. Unit grew 494 -> 495 in exactly one crate - `aether-http` 112 -> 113 passed - and integration grew 429 -> 430 in exactly one target - `tests/streaming_request.rs` 12 -> 13 - the two pins this track added: the streaming head applies no declared-length ceiling of its own, so a `Content-Length` over the buffered `max_body_bytes` parses at the head and streams to a route whose `max_stream_body_bytes` allows it, while the buffered entry points still refuse the same bytes against their own ceiling. The doc lane held at 19 over 14 runs, because this track changed no example. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-142 tree, and the forty-second reading below supersedes it.

Re-measured a forty-second time 2026-11-06, same method, on the tree that carries the 0.2 API pass — PROB-0011's by-value builders and PROB-0005's one header store: unit **495** over 19 unit runs, integration **430** across 30 targets, doc **21** over 15 doc runs, 64 `test result:` lines, total **946 passed / 0 failed / 1 ignored**. The cross-check agrees: the 64 `running N tests` lines sum to **947**, which is 946 plus the single ignored test. The doc lane grew 19 -> 21 in one crate — `aether` 15 -> 17 — the two doctests this slice added: a `no_run` example on `tls_from_files` and the `compile_fail` pin that `RequestContext` has no `headers` field. Its run count went 14 -> 15 because the crate's `compile_fail` example is compiled as a second doctest binary. Unit and integration held: the tests this slice rewrote replaced existing entries — `header_keys_are_lowercased_and_the_first_line_wins` became `repeated_lines_share_one_lowercase_key`, and the header-map upgrade test became `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` — so no test count moved outside the doc lane. No lane entered or left the census. Every prior figure was re-derived from this run's per-target `test result:` lines; this reading describes the track-143 tree.

The checked-in E2E suite covers: routing params, 405/Allow, HEAD fallback, pipelining and
chunked bodies, duplicate headers, multipart uploads, static conditional/range requests, CORS
scoping, 413/431, panic isolation, the total receive deadline, graceful shutdown (including a
`1001` going-away frame to a live WebSocket session), WebSocket
handshake/echo/fragmentation/ping/close, handshake/unmasked-frame rejection, and the scheduler's
memory budget pricing both the block a request body reserves and the block an answer
reserves, on both emitters (since 2026-10-24 the h2 write path registers the allocation behind the
body it hands to the `h2` crate before queueing the response head and releases it with the stream;
what it declines to price was measured 2026-10-26 against `price_response_body` — a `&'static` body
and a block another handle retains correctly stay at the stream's base, a stream holding both a
request and a response block is priced by the response alone under `buffer_set`'s set semantics, and
a sliced body is priced only to the end of the buffer it was sliced from). The one real gap is
`Bytes::from_owner`, which `bytes` reports as never recoverable even for its sole handle — that heap
is invisible to this budget and no zero-copy change here can price it (PROB-0027, closed 2026-10-26
on the rule that replaces it: an HTTP/2 response body is built with `Bytes::from`, never
`Bytes::from_owner`, stated on `IntoResponse` and pinned by
`every_body_this_framework_produces_is_a_block_the_budget_can_name`, which calls the framework's own
producers rather than restating them). The same budget now also sees what a middleware attaches to
the request: an entry in `RequestContext::extensions` costs `EXTENSION_ENTRY_MEMORY` plus its inline
size always, and its heap when the inserter declares it with `insert_extension_sized` (PROB-0016,
closed 2026-10-26; the numbers behind the price are in
`framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs`). PROB-0023 is
closed; its last residual — an error answer that kept a dropped request buffer registered — is
fixed: `read_body` now re-prices the stream to its base on every refusal path. A peer that trickles
flow-control credit is bounded since
2026-10-25: a write interval renews the deadline only once it has moved 1024 bytes, so the same
`max_response_time` is also a floor on consumption (`MIN_PROGRESS_FOR_RENEWAL`,
`framework/aether/src/http2.rs:981`; PROB-0022, closed).
The earlier
external probe additionally covered status reasons, Arabic query decoding, JSON error escaping,
compression negotiation, and traversal probes.

**Not verified:** memory-unsafety under the fuzz campaign — the runner proves no panic, not no undefined behavior, because no ASan/libFuzzer toolchain exists on this host — behavior behind
a real TLS proxy, WebSocket compression (not implemented; subprotocol negotiation is no longer in
this list — track 95 delivered it), and any HTTP/3 behavior
(not implemented). HTTP/2 is **no longer** in this list — it has 86 tests in
`tests/tests/http2_e2e.rs` on the current tree (the per-target `test result:` line,
2026-11-05) and its own addendum
(§11). The external clients those §11 rows were measured with are now checked in beside the server
(`framework/tests/examples/node_h2_interop.cjs`, `h2_handframed_cap.py`), so the rows can be re-run
from the repository; their last run was 2026-10-25, against the tree that carries §15's `101`
refusal. **Platform:** every result in this report, including the TLS
stage, was produced on Windows 10.0.26200 x64 (MSVC). Linux and macOS were not built or tested
here — this host has no WSL and no container runtime — so cross-platform parity for `aether::tls`
is an argument (rustls + `ring` + `tokio-rustls`, no `#[cfg(windows)]` anywhere in the new layer),
not a verified result.

---

## 6. Performance measurements

### 6.1 Release build, keep-alive hello-world

These numbers were measured **before** multipart and WebSocket were added. Those features do
not execute on the ordinary GET path (one extra `Arc` clone per connection and a larger route
table are the only costs), but they have not been re-benchmarked, so no claim is made that they
leave throughput unchanged.

Method: raw-socket Python workers, one connection per worker, sequential keep-alive requests,
8 s measured window after a 1 s warm-up, 3 trials per concurrency. Server CPU from
`Get-Process` deltas; memory from `WorkingSet64`/`PeakWorkingSet64`. Zero request failures in
all runs.

| Concurrency | Trials (req/s) | Median | Server CPU/request | Peak RSS |
|---|---|---|---|---|
| 1 | 8,479 / 9,111 / 7,496 | **8,479** | 0.086–0.103 ms | 6.9 MiB |
| 8 | 22,167 / 22,518 / 22,643 | **22,518** | 0.127–0.131 ms | 7.3 MiB |
| 64 | 22,109 / 15,467 / 20,160 | **20,160** | 0.110–0.124 ms | 8.8 MiB |

At concurrency 8 the server used ~2.8 cores; at concurrency 64 the Python client is visibly
the bottleneck (throughput fell and variance rose). Idle RSS was 6.85 MiB; final working set
after load was 7.8 MiB. Startup was ~77–83 ms.

### 6.2 Three-framework comparison (earlier run in this audit)

| Framework | Plaintext medians | JSON medians |
|---|---|---|
| Aether | ~23.9k req/s | ~20.8k req/s |
| Axum | ~25.3k req/s | ~17.8k req/s |
| Actix Web | ~24.8k req/s | ~25.6k req/s |

Run-to-run variance was **33–59 %**, so these medians do not establish a winner in either
direction. The Aether build used for that run predates the final deadline/parser edits; those
edits do not touch the normal hello-world path except for one `Option<Instant>` check per
read. Wire-size observation from that run: Aether responses carried ~+25 % (plaintext) to
~+35 % (JSON) more bytes than the reference apps because of `X-Request-ID` (+32 B) and
`application/json; charset=utf-8`. This is a concrete, measurable disadvantage and a candidate
for optimization.

### 6.3 Interpretation and rejected optimizations

- **The server is CPU-bound before the client at c=8.** ~0.13 ms of server CPU per hello-world
  request is high; per-request `tokio::spawn` + timeout, request-id formatting, header
  `HashMap` allocation, param map, and body copy are the structural costs. They were
  identified but **not** optimized further because no isolated before/after measurement was
  available and a late restructuring would risk correctness.
- **`panic = "abort"` was rejected** despite saving 266–333 KB: it sacrifices per-request
  panic isolation.
- **`http.client`-based benchmarks were discarded** as client-bound and not representative.
- **Automated Ctrl+C shutdown tests were unreliable** on this Windows environment (process
  exit `0xC000013A`, signal occasionally not delivered). The added programmatic
  `serve_with_shutdown` API and E2E test now verify the drain path without depending on
  console signals.
- **Arena adoption and zero-copy handler views were not attempted.** The measurement does not
  prove they would help end-to-end, and both would require public API changes.

---

## 7. Binary and artifact size

| Artifact | Size |
|---|---|
| `basic_api.exe` (final, with WebSocket) | 2,129,408 B (2.03 MiB) |
| `full_async_api.exe` (final, with WebSocket) | 2,164,736 B (2.06 MiB) |
| Same examples before WebSocket integration (earlier round) | 2,037,248 B / 2,071,040 B |
| Delta attributable to WebSocket integration | +92,160 B / +93,696 B (~4.5 %) |
| Same examples built with `panic="abort"` (controlled experiment, earlier code) | 1,697,280 B / 1,713,152 B |
| Delta attributable to unwinding (approximate) | +265,728 B (+15.7 %) / +332,800 B (+19.4 %) |
| External release probe binary | 2,425,856 B (default profile, not stripped/LTO) |
| `framework/target/` total (after the fresh release build) | 2.8 GB |

The previous "1.4–1.5 GB" figure was the build directory, not an executable. No framework
features were removed to reduce size. Growth versus older reports (~1.56–1.62 MB) comes from
`panic = "unwind"` plus new code (chunked decoding, static_dir, JWT claims, deadline,
multipart, ranges, WebSocket) and `serde_json` use in examples. The final incremental release
build took 44.26 s.

---

## 8. Security and compatibility findings

### Security fixes verified

- Request smuggling: dual CL, CL+TE, unsupported TE, HTTP/1.0 chunked, malformed/overflowing
  lengths are rejected (400); header/body budgets return 431/413.
- Trickle slow clients are bounded by a total receive deadline and receive 408.
- Static-file traversal, Windows ADS/device names, and encoded traversal are blocked.
- Panic isolation returns 500 without terminating the process.
- JWT verification is constant-time with algorithm and claim validation — inside
  `aether-security`, which `aether` does not depend on (2026-10-20 check: zero dependents).
  It is therefore not a capability of the framework a user installs; it is code that builds
  and tests in the workspace and is not yet wired into the facade.
- WebSocket frame parsing rejects reserved bits/opcodes, fragmented control frames, oversized
  control frames, non-minimal lengths, and unmasked client frames; the handshake enforces GET,
  version 13, upgrade tokens, and a canonical 16-byte key. Frame and message sizes are bounded
  before payload buffering.
- Multipart parsing bounds part count, per-part headers, header bytes, and boundary length;
  filenames are never used as filesystem paths.
- Range handling only slices a representation when a strong `If-Range` validator still matches;
  multiple/malformed ranges fall back to the full response.
- Control characters/JSON injection in framework-generated error bodies are escaped.

### Remaining security gaps

1. **TLS is in-process; revocation is enforced only from operator-supplied material.** `aether::tls`
   terminates TLS 1.3/1.2 with SNI and live reload, and since track 101 refuses revocation material
   it cannot validate: `TlsCertSet::with_crl` checks the list's issuer signature and own window and
   refuses a listed leaf serial, and `TlsCertSet::with_ocsp` accepts only a successful, current,
   correctly signed response that names this leaf. Nothing fetches from a CA — refresh is the
   operator's reload — and no peer certificate is validated (server-side only, as for every public
   web server). Self-signed/CA material is the operator's to supply, and `require_tls(true)` is
   opt-in — a listener with a certificate still answers plaintext HTTP/1.1 on the same port unless
   asked not to.
2. **`Host` allow-list is opt-in, not on by default** (`enable_host_validation`). A deployment
   that never calls it still answers any `Host`, which is what every other framework here does
   too — but the redirect/cache decisions that trust this header are then unprotected.
3. ~~**No CSRF protection.**~~ **Closed 2026-10-29 (track 98c).** `AetherApp::enable_csrf`
   takes a `CsrfTokens` built from a shared secret: a safe response issues a signed
   `nonce.mac` cookie and hands the same value to the handler as `CsrfToken`, an unsafe
   method must echo it in one header, and every failure is one empty-bodied `403` refused
   before the handler. Stated limits: expiry is the browser's cookie lifetime (the MAC
   proves issuance, not freshness), and an exempt prefix is unprotected by design.
4. **Rate limiting is opt-in** (`enable_rate_limit`, keyed on the TCP peer — or, once
   `enable_trusted_proxies` names the proxies in front, on the forwarded caller the chain
   resolves to, so one bucket per client rather than per proxy). **Authentication is opt-in**:
   `require_bearer(&BearerAuth)` verifies an HS256 token from `Authorization: Bearer` and attaches
   a `Principal`, registered per route, per group or app-wide (track 98b) — it is off unless a
   route asks for it, so a public route stays public.
5. **~~Generic handler errors map to 400, not 500~~ — closed 2026-10-24.** The blanket bound is now
   `E: IntoResponseError`, so the error type declares its status: `AetherError::Io` answers 500 and
   `Timeout` 504 with the internal message redacted, while `&'static str`/`String` keep the 400 they
   documented. **~~What remains: a redacted 5xx detail is dropped rather than logged~~ — closed
   2026-10-27.** The detail now reaches an optional process-wide reporter
   (`aether::set_error_reporter`, receiving `ErrorReport { request_id, status, code, detail }`)
   under the same `X-Request-ID` the response carries; PROB-0009 is closed.
6. **~~Graceful shutdown aborts WebSocket sessions rather than sending a close frame~~ —
   closed 2026-10-27.** A session gets one `1001` going-away frame and ends on it; only a handler
   that never returns to `recv` is aborted, after a 1 s grace. HTTP connections drain properly,
   and `SIGTERM`/programmatic shutdown already existed.
7. **~~Cookies are parsed but not signed/encrypted~~ — the signed half closed 2026-11-02
   (track 114).** `aether_security::CookieSigner` issues
   `base64url(payload).base64url(HMAC-SHA256)` and binds the cookie **name** into the tag, so a
   value this server signed for `theme` cannot be presented as `session`;
   `RequestContext::signed_cookie` answers `Present`, `Absent`, `Invalid` or `Ambiguous`, and a name
   carried twice — on one `Cookie` line or across two — is refused instead of resolved by taking
   one. Fifteen unit tests in `security/src/cookie.rs`, eleven wire tests in
   `tests/tests/signed_cookies.rs` and five more over the HTTP/2 frontend in `tests/tests/http2_e2e.rs`
   (track 115, 2026-11-03: that frontend builds its own header store, so these are what prove the value it
   hands the accessor is the value the peer sent — a `session` whose separator arrived as `%2E` is refused,
   not decoded — that a name carried twice is still refused when the two halves come as two separate fields,
   and that four streams in flight on one connection each answer for their own cookie. Four of the five run
   on cleartext prior-knowledge h2c; the fifth round-trips the issued cookie and refuses a foreign MAC on TLS
   with ALPN `h2`, and the duplicate shapes are not claimed over TLS), two doctests, and one `compile_fail`
   assertion that the signer never gains a `Debug`. **Encryption is still not implemented:** the payload is base64url, so the
   browser reads it — this is integrity, not confidentiality, and anything the client must not see
   belongs in server state behind an opaque id.
8. **~~CSP is an opt-in static header, with no nonce/hash support~~ — closed 2026-10-29
   (track 96).** `SecurityHeadersMiddleware` substitutes `CSP_NONCE_PLACEHOLDER` (the `{nonce}`
   source expression) with a fresh `aether_security::CspNonce` on every dispatched response and puts
   the same token in the request extensions, so an inline `<script nonce="…">` and the header cannot
   disagree, and `csp_hash_sha256` produces the `'sha256-…'` form. Track 96's register record lists
   its unit and wire tests. Track 140 narrowed the draw to requests that will reach a handler: an
   answer the framework writes itself — a security refusal, a 404/405, a preflight, the generic
   `OPTIONS`, the h2 upgrade refusal — carries the policy with the nonce expression removed and
   pays no entropy draw. What survives of this row is the first clause: the whole thing is
   opt-in, and an app that configures no policy sends no `Content-Security-Policy` header.
9. **The receive deadline covers body upload** — by design, and it is its own setting. A body that
   has not finished arriving inside `ServerLimits::receive_timeout` is refused: on a buffered
   route the server answers `408` itself — from the parser's need-more-data branch while bytes
   keep arriving, and since track 117 also from the read that times out, whose wait is capped by
   `min(read_timeout, what is left of the budget)` — on a streamed one the deadline fails the
   lane and `BodyError::Timeout` reaches the handler's read (which is what track 100 bounds a
   slow upload with). Before track 117 a buffered body that stopped arriving at the socket
   entirely was closed at the 5 s per-read timeout with no answer (measured 2026-11-04: EOF at
   5.005 s with no body byte sent, 5.003 s with three of five, 5.002 s even at the default 30 s
   budget); PROB-0034 recorded that, and track 117 answered it. An idle keep-alive socket has no
   request to answer and is still closed silently. A legitimately slow large upload needs a
   larger `AETHER_MAX_RECEIVE_TIME_SECS`, and the second clause as this list used to state it —
   "configurable only through the shared request-timeout setting" — is false:
   `AetherConfig::max_receive_time` is separate from `max_request_time`, which bounds handler
   execution (the two were split in track 84), and a `0` receive window is refused at parse time
   because no request could finish arriving.
10. **WebSocket compression is not implemented.** (Subprotocol negotiation was delivered in track
    95, 2026-10-29.)
11. **~~WebSocket sessions have no post-upgrade read/write deadline~~ — both deadlines exist;
    what the row priced was wrong.** `WebSocketLimits::read_timeout` (default 120 s, delivered as
    F7 on 2026-10-19) bounds each socket wait in `recv`, and `write_timeout` (default 10 s) bounds
    how slowly a peer may consume an outgoing frame — the write half was PROB-0030, closed by
    track 72 on 2026-10-26. `AetherApp::with_websocket_limits` sets both app-wide, so the "only the
    101 response is covered" clause is stale. What remains true is the consequence the row named
    without pricing: an idle peer holds its connection slot and its permit until `read_timeout`
    fires, so the cost of a stalled client is the configured window, not zero — and no deadline at
    all is `None`, which an operator has to ask for deliberately.

### Public API compatibility

No public function signature was removed in the 0.1 line. 0.2 changed two surfaces on purpose, both
tabulated in `docs/11_migration_0_1_to_0_2.md`: the 38 `AetherApp` registration methods take `self`
and return `Self` so calls chain, and `RequestContext`'s public `headers` map was removed in favour
of one private store read through `header`, `header_values` and `headers`. Preserved:
`RequestView::parse`, `WsFrameView` layout, `JwtValidator::verify_signature`,
`ServerLimits`, `AetherConfig`, and every `RequestContext` accessor.

Additions: `http::parse_request`; `aether::AetherApp::{static_dir, static_dir_with_cache,
ws}`; `aether::{AetherServer, bind, bind_with_config}`, `AetherServer::{local_addr, serve,
serve_with_shutdown}`; `RequestContext::{header_values, multipart}`;
`http::multipart::{MultipartForm, Part, MultipartError}`;
`http::static_files::{parse_range_header, if_range_matches, serve_static_with_range,
RangeDecision}`; `websocket::{ClientFrame, parse_client_frame, validate_handshake,
accept_for_key, HandshakeError, WebSocket, WebSocketLimits, WsError, Message}`;
`JwtValidationOptions`, `JwtValidator::verify`, `VerifiedJwt`.

Behavioral changes (not signature breaks) that embedders must know:

- `Transfer-Encoding: chunked` is now decoded instead of rejected.
- WebSocket and JWT parsing are stricter.
- Static ETags are now strong (`"…"`) instead of a malformed weak-looking string.
- A panicking handler now yields 500 under the release profile (previously the process
  aborted).
- CORS preflight now checks that the requested method is routable.
- `AETHER_MAX_HEADER_BYTES`, `AETHER_WORKERS`, and the two time variables are now read by
  `from_env`. `AETHER_WORKERS` was displayed and ignored for a long time; it is now applied by
  `AetherApp::run`, the entry point that builds the runtime from the configuration (track 62),
  while `listen`/`bind`/`serve` still run on the caller's runtime and the banner prints that
  runtime's active worker count with a note when it differs from the configured one.
- Response `Connection: close` is emitted once, not twice.
- `RequestContext::header()` returns the first field line; `header_values()` returns every line
  of a name in wire order, and `headers()` enumerates one lowercase `(name, first line)` pair per
  field. The 0.1 public map keyed by wire casing is gone; a middleware that must hand a decision
  to the handler uses `extensions_mut()`, which no framework check reads.
- Static handlers now answer satisfiable `Range` requests with 206 and unsatisfiable ones with
  416 instead of always returning 200.
- `framework/middleware/src/timeout.rs` (unreferenced, uncompilable dead file) was deleted
  with owner approval; no module or API referenced it.

Dependency changes: `aether-security` now depends on `serde_json`; `aether-websocket` now
depends on `ring` and `tokio`; `aether` depends on `aether-websocket`. All were already in the
lockfile. No dependency was removed.

---

## 9. Remaining work (priority order)

1. ~~Typed error mapping (`IntoResponse for AetherError`, 500 for internal errors).~~
   **Delivered 2026-10-24 (D9), in the framework's own shape.** The bound is a local trait,
   `E: IntoResponseError` (`framework/aether/src/app.rs:123-166`), not `IntoResponse for
   AetherError`:
   re-bounding the blanket to `E: IntoResponse` was measured to answer an `Err` with **200**,
   because the same blanket serves `&'static str`, whose response is a successful plain-text one.
   The error type declares its status — `&'static str`/`String` at
   `framework/aether/src/app.rs:168-186`, `AetherError` at `framework/aether/src/app.rs:188-222`
   (`Io`/`DependencyCycleDetected` 500, `Timeout` 504) — the blanket impl at
   `framework/aether/src/app.rs:224-265` builds the JSON body with its `code` derived from the status class, and a 5xx redacts its
   message while handing it to the reporter (PROB-0009). Pinned by
   `a_handler_error_answers_400_and_says_so_in_the_body` and
   `typed_handler_errors_answer_their_own_class`.
2. ~~Separate receive/upload timeout from the handler timeout; client-disconnect cancellation.~~
   **Delivered in two halves.** The receive deadline became its own setting on 2026-10-27
   (`max_receive_time`, track 84); a running handler is cancelled when its client is observably
   gone as of 2026-10-27 (track 85): HTTP/1 on a read error — an RST, or rustls's
   end-without-`close_notify` — and HTTP/2 on a per-stream `RST_STREAM` or a dead connection.
   A clean HTTP/1 EOF stays answerable by design (RFC 8446 half-close) and is still caught at
   write time. Pinned by `client_disconnect_cancels_the_running_handler`,
   `a_stream_reset_cancels_the_running_handler_and_keeps_the_session`,
   `a_dropped_h2_connection_cancels_its_running_handlers`,
   `a_tls_client_that_leaves_cancels_the_running_handler`, and in the not-gone direction by
   `a_pipelined_request_does_not_look_like_a_disconnect`,
   `a_client_that_stays_connected_is_not_cancelled` and
   `a_tls_client_that_half_closes_still_gets_its_response`.
3. ~~Streaming request bodies.~~ **Delivered on both frontends** — HTTP/1.1 on 2026-10-29 in track 100, HTTP/2 on
   2026-10-30 in track 100b — together with response streaming and the streamed static-file answer
   in track 99 (`FileBody` reads its range in 64 KiB chunks inside userspace; the literal
   `sendfile(2)` syscall is still not implemented). `AetherApp::route_stream`
   dispatches a route from its parsed head, before the body is read; the handler pulls chunks from
   `RequestContext::body_stream()` or reads the whole body with `read_body(max)`. The body is
   bounded by `max_stream_body_bytes` (a declared length over it is `413` before dispatch; a
   chunked body that crosses it fails the read) and by `receive_timeout` counted from the head,
   its held bytes are charged to the connection's memory registration chunk by chunk, and
   `Expect: 100-continue` is answered before the body. An answer that leaves the body unfinished
   closes the connection after the response, so the tail cannot be read as a next request; a body
   that finished its framing keeps it alive. Ten socket tests in
   `framework/tests/tests/streaming_request.rs` (eight when this section was written, plus track 110's
   pair that pins where a streamed lane stops relative to its declared length). On HTTP/2 the same route set branches in
   `serve_request` to `serve_streamed` + `Http2Body`: DATA is pulled into the same lane as it
   arrives, capacity is returned per frame so a body larger than the 65,535-byte window completes,
   the same ceiling and deadline apply, and an unread body is abandoned by dropping the
   `RecvStream` — `h2` ignores further DATA and returns the capacity, so the connection serves the
   next stream (`RST_STREAM(NO_ERROR)` is not sent: `h2` drops the stream's queued response frames
   on a reset). Six h2 socket tests in `framework/tests/tests/http2_e2e.rs`.
4. Custom middleware **delivered 2026-10-18** (app-wide and group-scoped `use_middleware` with a
   `Next` continuation). The dedicated **per-route** middleware/guard API — `route_sync_with`,
   `route_async_with`, `ws_with` — is **delivered 2026-10-29** (track 94), with five
   `user_middleware` and two `http_e2e` wire tests; an empty-prefix group remains the
   prefix-scoped form and produces the same wire result.
5. ~~Typed extractors and request extensions.~~ **Delivered.** Extractors 2026-10-18
   (`Path`/`Query`/`Json`/`Form`/`State`); request-scoped extensions landed 2026-10-21
   (`aether/src/extensions.rs` + `RequestContext` accessors + the `Extension<T>` argument,
   17 unit + 8 HTTP/1.1 wire + 4 HTTP/2 wire tests — the HTTP/2 half is what proves the state is
   per stream and not per connection). Since 2026-10-26 the map is priced by the scheduler's memory
   budget: every entry costs `EXTENSION_ENTRY_MEMORY` plus its inline size, and an inserter that
   holds a heap allocation declares it with `insert_extension_sized` so the budget registers that
   too (PROB-0016, closed 2026-10-26). What that pricing still cannot see is stated in §12 and in remaining-work item
   17 — a value grown in place through `get_mut`, and a heap payload nobody declared.
   `Extension<T>` still requires `Clone` because the map has
   one owner (Axum's `Extension<T>` has the same bound).
6. ~~HTTP/2~~ **Delivered 2026-10-20/21** (over TLS by ALPN, over cleartext by opt-in
   `enable_h2c`, one serving core behind both frontends — see §11). **HTTP/3** is what remains: a
   QUIC connection engine, not a patch on this one.
7. WebSocket permessage-deflate (owner-deferred) and subprotocol negotiation — **negotiation
   delivered 2026-10-29** (track 95: `ws_subprotocols` on the app and group, RFC 6455 §4.2.2
   selection, the echoed `Sec-WebSocket-Protocol` line, `400` when a declared route cannot meet
   the offer; five wire tests). The graceful
   close on shutdown was delivered 2026-10-27 (see §4.6).
8. CSRF middleware. TLS termination landed on 2026-10-20 (`aether::tls`, rustls, same-port SNI,
   live reload); bearer authentication and the forwarded-client identity (tracks 98b and 98a)
   made cookie-less APIs authenticable and rate limiting correct behind a proxy, and the
   cookie-authenticated form arrived on 2026-10-29 (track 98c, see item 3 above). OCSP/CRL
   enforcement landed on 2026-10-30 (track 101: `with_crl`, and `with_ocsp` only after the response
   proves it covers this chain, is current and is signed; both offline, refreshed by reload). (Host
   validation and rate limiting landed on 2026-10-20 too.)
9. Performance work driven by isolated before/after measurements: request-id formatting,
   header map allocation, handler spawn/timeout structure, and wire-size reduction.
10. ~~Run the fuzz target for a sustained period.~~ **Closed 2026-10-30 (track 102b):** the
    in-repo runner (12 seeds, seeded mutation, outcome buckets, crash artifacts) completed
    15 843 328 executions in 120.011 s at seed 20261030 with 0 crashes, and caught a
    deliberately injected chunk-boundary panic at execution 6 507 with an artifact. Re-run on
    2026-10-30 over the track-106 parser: **16 089 088 executions in 120.006 s** (134 069/s),
    corpus 12 → 711, 709 outcome buckets, 0 crashes.

---

## 10. Evidence and cleanup

**What is checked in is the evidence.** Everything below is in the repository and survives any
cleanup of scratch files:

- `framework/tests/tests/http_e2e.rs` — 102 raw-socket HTTP/1.1 end-to-end tests (recounted
  2026-11-05 on the track-132 tree from the target's `test result:` line; 52 after track 75 added the wire-budget boundary
  test that serves a 1-byte-chunk upload at 350,879 payload bytes and refuses it at 350,880, then 53
  after track 84's receive-budget test that trickles a body past the handler timeout, then 73 across
  tracks 86–108, 78 after track 110's five declared-length tests, 89 after track 116's `Expect` shapes, 93 after tracks 117 and 118's receive-budget and diagnostic wire tests, 97 after track 123's four cache and negotiation pins, 98 after track 126's split-`Cookie`-field read, 99 after track 127's form-rule key decoding, 100 after track 128's out-of-root link pin, and 101 after track 131's compressed-static validator pin);
  15 of them register a
  WebSocket route *and* install middleware (re-counted mechanically from the file on 2026-10-31 by the
  rule the root README states — a test counts when its own body, or a same-file builder it calls,
  registers a chain through `use_middleware`, `route_sync_with`, `route_async_with` or `ws_with`
  *and* a WebSocket route: the §13 upgrade-inside-the-chain cases, the behaviour tests this audit
  opened in §14, the two the `101` fix produced in §15, and the two per-route cases track 94 added,
  which reach the route through `ws_with` rather than `use_middleware`. The 13 this bullet carried
  pre-dated those two).
- `framework/tests/tests/tls_e2e.rs` — 12 TLS handshake/wire tests (recounted 2026-11-05 on the
  track-132 tree: 11 after track 85's abrupt-departure and `close_notify`-half-close pair, then 12
  after track 129's receive-budget pin, which trickles a body over TLS past the deadline and reads
  the 408 back).
- `framework/tests/tests/http2_e2e.rs` — 86 HTTP/2 wire tests (recounted 2026-11-05 from the test
  attributes: 40 on 2026-10-27, three added by tracks 86–99, track 100b's six request-streaming
  tests, six request-target tests in track 106, one mounted-sub-app test in track 107 and eleven
  declared-length tests in track 110, 72 after track 115's five signed-cookie reads, 80 after track 116's `Expect` shapes 85 after track 125's five CORS and identity pins and 86 after track 126's split-`Cookie`-field read), over TLS with ALPN `h2`, over
  cleartext `h2c` through the `h2` client, and over connections written as raw frames where a library
  client cannot produce the input the server must survive. The three shapes count out at 30
  `H2Client::connect` over TLS, 26 `H2Client::connect_plain` over cleartext and 12 that frame the
  socket themselves — 9 through the `handframed` helper (track 106's malformed-`:path` reset plus
  track 110's eight shared `lying_length_is_reset` cases) and 3 that write raw bytes or share the
  concurrency-cap helper. Several hand-frame HEADERS/DATA/SETTINGS at
  the byte level, several also send a plaintext HTTP/1.1 request to show the same port is still
  served, and one holds four streams on one connection with three inside the middleware's hold at
  once. Earlier versions of this bullet said 27, then 29, then 49 with a per-transport split; the split was
  not re-derived as the file grew, so this bullet states the count and what it was counted from.
- `framework/tests/tests/extensions_e2e.rs` — 8 wire tests for request-scoped extensions over
  HTTP/1.1, including the keep-alive case where a second request on one connection must not see the
  first request's values, and, since the budget priced extensions, one that asserts a declared
  payload's registration grows by exactly its container plus its declared heap and one that shows an
  undeclared value is still charged its container while the answer keeps being served.
- `framework/aether/src/extensions.rs` — 17 unit tests for the type-map itself.
- `framework/tests/examples/h2_interop_server.rs` — the server the external clients below are run
  against. It mints its certificate **in memory** and writes nothing to disk.
- `framework/aether/src/http2.rs` — 12 unit tests: 5 for the request-head translation, 2 for
  response-body pricing over a scheduled connection, 1 for the drain deadline, and 4 added 2026-10-26
  that drive the pricing step and the refusal path directly (three `price_response_body` body shapes
  and one `read_body` registration case).
- Suite as of 2026-10-23: `cargo test --workspace` — **474 passed, 0 failed, 1 ignored** (271 unit +
  198 integration across 15 checked-in targets + 5 doc tests; the 2026-10-18
  figure of 182 described the tree before the TLS and HTTP/2 stages, 455 was the same tree
  before §14's three tests, 458 before §15's `101` fix, 459 before PROB-0014's three tests, 462
  before PROB-0008's four, 466 before the two `aether-websocket` idle-deadline tests and 468 before
  this slice's six).

**What is scratch, and how to remake it.** Every probe/client script lives *outside* the repository,
in a temporary directory the owner clears (`%TEMP%` on Windows; the current and earlier rounds used
two different ones), and is disposable — the numbers they produced are transcribed into §11 and the
feature matrix, so deleting the scripts loses no finding. To reproduce an external run after deletion:

```bash
cargo build --release -p aether-tests --example h2_interop_server
# three listeners, one build; each prints its own CA between CA-PEM-BEGIN / CA-PEM-END
./target/release/examples/h2_interop_server.exe 18451 strict   # tls + require_tls + enable_h2c
./target/release/examples/h2_interop_server.exe 18450 tls       # tls + ALPN
./target/release/examples/h2_interop_server.exe 18453 h2c       # cleartext prior knowledge
# then: a non-Rust client against each port, re-extracting the CA from that process's stdout
```

- The `Upgrade`-style clients used were Node.js 24 `http2` (nghttp2 stack) and python-h2 + hyperframe
  + hpack; neither is vendored here, both are ordinary `npm`/`pip` installs.
- Probe project kept outside the repo as a downstream compile check: a sibling directory named
  `aether-probe` beside this repository.
- All probe servers were stopped after measurement.

The probe project and temporary harnesses are outside the repository and are not part of the
delivered framework. The `target/` directory was retained; it is build output, not source.

---

## 11. Addendum — the HTTP/2 verification round, 2026-10-21

This section is dated separately because the body of this report is the 2026-10-18 record and is
not edited. It covers the protocol stage delivered after that date: HTTP/2 over TLS by ALPN and
over cleartext by prior-knowledge opt-in, and the focused pre-commit review the owner asked for.

**The defect found, and why the suite had not found it.** `h2` credits the peer for received bytes
only when the application releases the capacity; `RecvStream::drop` clears the buffer and credits
nothing. `framework/aether/src/http2.rs::read_body` consumed the request body and released nothing,
so any upload larger than one receive window (64 KiB by default) could not be served: the server
read one window, then waited for bytes the client was no longer permitted to send, and answered 408
at `request_timeout`. Downloads were unaffected, which is why a 5 MiB flow-controlled response had
already been measured as intact and nothing looked wrong.

The fix is the release call itself. The regression evidence is
`a_body_larger_than_the_receive_window_arrives_whole` (256 KiB in, echoed byte-for-byte) and
`a_body_over_the_limit_is_413_when_the_limit_is_crossed_mid_stream` (256 KiB against a 128 KiB
budget, refused at the crossing and the connection then serving the next request). The pre-existing
413 test used a 16-byte limit and a 64-byte body — the limit is crossed inside the first window, so
that path never needed a credit. Stated as a rule for the next round: a body test that stays inside
one receive window does not test flow control at all.

**Two probe corrections, recorded because both were nearly believed.**

- python-h2 reported "blocked by flow control after 49152 bytes … window is 16383" with zero credit
  observed, against a server that a strict nghttp2 client streamed 1 MiB through in 16 ms. The probe
  was reporting its own window accounting. Only wire-parsed SETTINGS values and HTTP status codes
  from it were used.
- The same blind spot existed in this suite's own harness: `read_body` in `tests/http2_e2e.rs` never
  released capacity for **response** bodies either, so the new 256 KiB test failed with
  `UnexpectedEof` after 60 s and looked like a server bug. It was the client. The rule is symmetric,
  and the harness now obeys it.

**External clients, against the final release binary** (same `h2_interop_server` example, three
listeners on one build: `strict` = TLS + `require_tls(true)` + `enable_h2c`, plain TLS + ALPN, and
cleartext `h2c`):

| Check | Result 2026-10-21 | Re-run 2026-10-23 | Re-run 2026-10-25, from the repository |
|---|---|---|---|
| Node.js `http2` over TLS | 16/16 on both TLS ports | 24/24, twice (fresh client, same behaviours + ALPN/no-ALPN + wire-decoded SETTINGS) | 24/24, twice, exit 0 — same client, now committed as `node_h2_interop.cjs` |
| ALPN | `h2` negotiated; `http/1.1` negotiated when offered; served with no ALPN offered at all | all three re-run | all three |
| Cleartext on the strict port (raw sockets) | 3/3 closed without an answer: HTTP/1.1 request, h2 preface + SETTINGS, silent connection | 3/3, same three probes | 3/3, 0 bytes read on each |
| Oversized header list | 431 over the advertised `MAX_HEADER_LIST_SIZE`; 431 over the per-line budget at the default 100 lines | both 431 | both 431, connection still serving |
| Oversized body | 413 at 2,146,304 bytes against the 2 MiB default, mid-stream, connection still serving | 413, echoed 0 bytes, next request 200 | 413, echoed 0 bytes, next request 200 |
| Flow control | 1 MiB upload → 200; 5 MiB download → intact | 200 with 1 MiB echoed whole; 5,242,880 B received | 200 with the 1,048,621-byte echo; 5,242,880 B received |
| Multiplexing | 50 concurrent streams (10 behind a 300 ms handler) on one TCP socket | 50/50 in 324 ms (serialised it would be ~3 s) | 50/50 in 325 ms |
| Concurrency cap, hand-framed (python, raw frames) | 110 streams on one connection → 100 answered, 10 `RST_STREAM` code 7, no GOAWAY | 100 × `200`, 10 × `RST_STREAM(7)`, 0 GOAWAY, then `/ping` → 200 `pong` | same, PASS twice, 212 frames decoded |
| Cancellation | stream reset mid-handler, connection kept serving | `RST_STREAM` inside `/slow`, next request 200 | 200 |
| WebSocket route over h2 | 400, both TLS and cleartext | 400 over TLS and 400 over cleartext | 400 over TLS and 400 over cleartext |

Every row was re-run **2026-10-23 on the current tree** — the one that carries the
WebSocket-middleware change, the `101` refusal, the capacity-based connection pricing and the two
WebSocket idle-deadline tests — against `cargo build --release -p aether-tests --example
h2_interop_server` (exit 0, 3,958,784-byte artifact, `Finished release in 1m 51s`).

**Re-run 2026-10-25, from the repository: the reproducibility qualifier is closed.** It had two
halves. The first fell on 2026-10-23. The second was that the clients were scratch files, so a
reviewer could read the commands but not run them. Register item B8 allowed either answer — commit
the files or keep only the commands — and the files are now committed beside the server they drive:
`framework/tests/examples/node_h2_interop.cjs` and `framework/tests/examples/h2_handframed_cap.py`.
Every row above was re-run from those paths against a fresh release build (`Finished release profile
in 3m 16s`, 3,979,776-byte artifact) on the tree that also carries §15's core-level `101` refusal,
**twice each**: Node **24/24 both times**, exit 0; the hand-framed probe **PASS both times** with
100 × `200`, 10 × `RST_STREAM(7)`, 0 GOAWAY and a post-storm `200 pong`.

**How to reproduce it.** Server and both clients are in `framework/tests/examples/`. The server
prints its issuing CA between `CA-PEM-BEGIN`/`CA-PEM-END` and a `READY <scheme>://<addr> mode=<mode>`
line once it is listening; the certificate is minted per process, so a CA from an earlier run belongs
to a dead server.

```
cargo build --release -p aether-tests --example h2_interop_server
node   framework/tests/examples/node_h2_interop.cjs   framework/target/release/examples/h2_interop_server.exe
python framework/tests/examples/h2_handframed_cap.py  framework/target/release/examples/h2_interop_server.exe 18976
```

Both exit non-zero on a failed assertion (the Node client exits 2 on a runner error). The Node side
needs nothing but Node's builtins — 24.18.0 was the runtime for every run in this table; the Python
side needs `hyperframe` 5.2 and `hpack` 3.0. The
extension is `.cjs`, not `.js`, because the repository root's `package.json` then declared
`"type": "module"`, so a `.js` file inside this tree was parsed as ESM and its `require` was
undefined — which is how the first attempt to run the committed copy failed. That manifest is gone:
track 111 deleted it with the AI-Studio site on 2026-11-02, and the file keeps the extension it was
run with (re-measured here: Node v24.18.0 parses a `.js` file in this tree as CommonJS now, and
`node --check` exits 0 on this one). Recording the commands would not have caught the original
failure; it is the argument for committing the files.

`node_h2_interop.cjs` (282 lines, Node builtins only — `http2`, `tls`, `net`, `child_process`) spawns
the three listeners itself on ports 18943/18944/18945 and asserts: ALPN `h2`; `SETTINGS` **read off
the wire by decoding the frame** (`MAX_CONCURRENT_STREAMS`=100 at id `0x3`,
`MAX_HEADER_LIST_SIZE`=8192 at id `0x6`); `/ping` 200; the WS route 400 over TLS *and* over cleartext;
1 MiB upload echoed whole; 5 MiB download byte-exact; 50 concurrent streams; 60×200-byte headers →
431 and 120 header lines → 431 with the connection still serving; 2,146,304 B → 413 with the
connection still serving; `RST_STREAM` mid-handler; 110 streams through Node's cap-aware client;
the three cleartext-on-`strict` refusals; TLS with no ALPN and with `http/1.1` only; and an
`Upgrade: h2c` request. `h2_handframed_cap.py` (151 lines, `hyperframe` 5.2 + `hpack` 3.0) is the one
check a library client cannot make: it ignores the advertised cap and puts 110 `HEADERS` frames on
the wire at once, then decodes the server's frames. The cap check is the one that needs `0x7`
`REFUSED_STREAM` read from a `RST_STREAM` frame rather than inferred from a client callback.

Two things that re-run taught, both in the "the probe measured itself" class:

- **Node's `remoteSettings` was right here, but only the frame decode proves it.** The client reported
  `maxHeaderListSize=8192`; the SETTINGS frame parsed off the TLS stream carries id `0x6` = 8192. The
  first version of this probe asserted id `0x5` and printed `undefined` — `0x5` is `MAX_FRAME_SIZE`.
  The docs' rule stands: read the frame, not the library's summary.
- **A cap-aware client cannot reproduce the cap row.** 110 streams through Node came back 110/110
  answered with zero refusals, because Node queues the overflow against the advertised 100 — the only
  evidence such a run gives is the two-wave wall clock (651 ms for 110 × 300 ms handlers on
  2026-10-23, 759 ms on 2026-10-25). The refusal
  itself needs a client that ignores `SETTINGS`, which is why the hand-framed script exists.

Measured here for the first time, and consistent with the code comment at
`framework/aether/src/app.rs` (`enable_h2c`, which states that `Upgrade: h2c` is deliberately not
implemented): Node 24.18's cleartext `http2.connect()` opens with the **`PRI * HTTP/2.0` preface** —
prior knowledge, not the upgrade exchange — and an explicit `Upgrade: h2c` HTTP/1.1 request to the
same listener is answered `HTTP/1.1 200 OK` with no `101` and no protocol switch. Linux and macOS
remain unrun (`PROB-0012`); these are Windows/MSVC results.

Committing the two client scripts under `framework/tests/examples/` — which would turn this section
from "recorded from one machine, re-runnable only by whoever still has the scratch" into a gate —
is registered as **B8's open decision** and has not been done here.


**Bounds and containment added to the checked-in suite** (5 new wire tests; `http2_e2e.rs` was 24
then and 27 after the extension tests below — 29 on the 2026-10-23 tree, recount with
`grep -c tokio::test tests/tests/http2_e2e.rs`): over-cap streams hand-framed against the advertised limit → 100 answered, overflow reset with
`REFUSED_STREAM`, no GOAWAY, connection still serving; a header list over the per-line budget → 431;
a panicking handler → 500 on its stream, 200 on the next, same connection; a peer that vanishes
mid-body → the scheduler registration settles at 0; an unusable `:method` token → 400 on that stream.

**Full suite, 2026-10-21:** `cargo test --workspace` — **426 passed, 0 failed, 1 ignored** for the
HTTP/2 stage alone; **445 passed, 0 failed, 1 ignored** after §12; 452 after §13 and
after §13 including its review remediation.

**What is still not verified, stated plainly.** Linux and macOS have never run this suite
(`PROB-0012` stays open). `max_concurrent_streams` and `max_headers_count` were advertised and
enforced but not settable; as of 2026-10-22 they are `AetherConfig` fields and environment
variables, and both bounds are now pinned at a configured value as well as at the default
(`PROB-0014` closed). Receive
window sizes are left at `h2`'s 64 KiB defaults: no measurement justifies changing them, so they were
not changed. Server push and the `Upgrade: h2c` exchange remain deliberate non-goals. No performance
claim is made in this addendum — the flow-control fix changes what is servable, not a measured
throughput number, and no before/after benchmark was run for it.

---

## 12. Addendum — request-scoped extensions, 2026-10-21

**Why this was the next item.** With the protocol stage closed, the parity gaps were re-derived from
the code rather than from the stale "remaining work" lists: `AetherApp::use_middleware` and the typed
extractors already existed, and the one genuinely missing Axum/Actix-parity mechanism was a place for
a middleware to hand a per-request value to the handler. Until now the only shared mutable surface was
`manage`/`state`, which is process-wide — correct for a pool, wrong for a principal.

**What was built.** `framework/aether/src/extensions.rs`: a `TypeId`-keyed map, boxed on first insert
so an unused request allocates nothing, holding non-`Clone` `Send + Sync + 'static` values, reachable
through `RequestContext::{extensions, extensions_mut, extension, insert_extension}` and as the
`Extension<T>` handler argument. It rides `RequestParts` into `RequestContext`, so both frontends
populate it the same way — `http2.rs` and the HTTP/1.1 path each construct one `RequestContext`, and
extraction happens inside the shared core, which is why no separate HTTP/2 extractor test was needed
(the HTTP/2 tests below do exercise the map on that frontend).

**Evidence (19 new tests, 426 → 445).**
- 8 unit tests in `extensions.rs` — type-keying, overwrite returns the previous value, `remove` hands
  over ownership, `get_mut` writes through, `Debug` prints a count and never a value, and the fresh
  map is asserted to have *not allocated*.
- 6 wire tests in `tests/tests/extensions_e2e.rs` (HTTP/1.1): middleware→handler, two middlewares on
  one map (which fails if the chain copied the context), group scoping, the keep-alive non-leak, and
  both halves of the `Extension<T>` extractor (value present; absent → 500 rejection, handler never
  runs).
- 3 wire tests in `tests/tests/http2_e2e.rs`: value reaches a handler over TLS/ALPN `h2`; **four
  concurrent streams** each holding the middleware chain for 100 ms and each reading only its own
  value, with the fourth (unmarked) reading none — the test also asserts the four finished inside
  280 ms, so the isolation claim cannot pass by streams having run one after another; and the same
  request answered identically over HTTP/1.1 and h2c on one port.
- 2 doc tests: the `Extensions` examples (borrow form and `Extension<T>` argument form) compile.

**Review method, stated as §6 requires.** No independent agent was used for this slice. It was
self-reviewed against the two designs it imitates (`http::Extensions`, Actix's `req.extensions()`) and
against the coherence constraint that decides the framework's version of "typed error mapping" —
neither needs a second implementation's opinion, and the falsification attempts that matter here are
in the tests: the non-`Clone` value kills a cloning context, the 280 ms bound kills a sequential
"concurrency" test, and the unmarked fourth stream kills a connection-scoped map.

**Limits recorded, not hidden.** `Extension<T>` requires `Clone` (Axum's does too) because the map has
one owner; a non-`Clone` value is still reachable by borrow through `req.extension::<T>()`. Extension
payloads **are** priced by the scheduler memory budget since 2026-10-26 — each entry its container and
inline size, plus the heap an inserter declares — and the residue that pricing still cannot see is
listed in §16 and in remaining-work item 17 of `FEATURE_COMPLETENESS_MATRIX.md`. No performance claim is made: the map is not allocated for requests
that never insert, which is the only cost statement this addendum supports, and it comes from
`a_fresh_map_holds_nothing_and_has_not_allocated`, not from a benchmark.

---

## 13. Addendum — WebSocket upgrade requests inside the middleware chain (PROB-0017), 2026-10-21

**Why this was next.** §12 delivered the mechanism a guard needs to hand a principal to a handler,
and while documenting it the question "which routes does the chain actually wrap?" had to be answered
from the code, not from the docs. `AetherApp::ws` inserted into `ws_handlers`, a map that
`wrap_with_middleware` never touched (it is applied at the two HTTP route-registration sites only),
and `ServiceCore::serve` answered an upgrade by returning `ServeOutcome::WebSocketUpgrade` without
dispatching anything. So a WebSocket route ran **no** user middleware at all — not to stamp an
extension, not to reject a request. The security reading is the one that matters: an `app
.use_middleware` auth chain guarded every HTTP route in the application and left every `app.ws`
route open. `RouteGroup::ws` had the same shape a second time: it called `AetherApp::ws` and dropped
the group's own scoped middleware on the floor.

**Measured before it was changed.** A characterization wire test was written first and run against
the unmodified tree: with one middleware that inserts an extension and one HTTP route on the same
server reading it, the HTTP route answered `stamped` and the upgraded handler reported `absent`. That
test is not in the tree any more — it was inverted into the seven tests below once the chain was
wired in — but it is the evidence that PROB-0017 was real rather than inferred from reading.

**What was built.** Each `ws` registration now captures the middleware that applies to it (app
middleware, then the group's, in the same order the HTTP routes compose) into
`ServiceCore::ws_middleware`. On an upgrade request the core folds that list around a terminal and
dispatches it through the **same** `dispatch` path as any other handler — so admission, the request
timeout and panic mapping all apply. The terminal does not answer: a `101` is synthesized by the
protocol frontend, so the terminal takes the request out of the response path and returns the single
status that means "the chain let me through". The upgrade then proceeds **only** if the chain hands
that `101` back untouched; any other status — a refusal before `next`, a 403 returned after awaiting
`next`, a 500 from a panic on the way back out, a 504 from a guard that stalls there — is served as
the answer and no socket is handed over. When the guard does pass, the handshake is validated against
the **post-middleware** context and the session starts with that context, extensions included.
`framework/aether/src/serve.rs` (the upgrade branch) and `framework/aether/src/app.rs`
(`ws`/`ws_scoped`, `RouteGroup::ws`).

**Evidence (10 wire tests, 445 → 450 → 452 → 455).** In `tests/tests/http_e2e.rs`:
`a_websocket_upgrade_runs_the_app_middleware_chain` (control HTTP route and upgraded handler read the
same stamped value), `a_middleware_can_refuse_a_websocket_upgrade` (401 instead of 101, the handler's
counter stays 0, and the same connection then serves the next request — a refusal must not damage the
socket), `a_panicking_middleware_answers_the_upgrade_request_with_500` (panic before **and** after
`next`, both 500, while the same server still upgrades a request whose chain completed),
`a_middleware_that_answers_after_next_refuses_the_upgrade` (403 after `await`ing the chain, handler
counter stays 0, connection still serves), `a_middleware_hanging_after_next_times_the_upgrade_request_out`
(`max_request_time` 150 ms → 504 with `Connection: close`, no `101`, no orphaned socket),
`group_middleware_wraps_the_upgrade_request_of_a_group_websocket_route`, and
`middleware_registered_after_a_websocket_route_does_not_wrap_it`, which pins snapshot-at-registration
semantics rather than a live list.

**One existing expectation was changed, and why.**
`tests/tests/scheduling.rs::panicking_websocket_session_is_tallied_as_cancelled` asserted
`completed == 0`. An upgrade request is now itself a dispatched unit of work, so the run measures
`admitted 2, completed 1, cancelled 1`: the one completion is the upgrade chain, and the panicking
*session* is still the cancellation. The assertion is now `completed == 1` with that explanation —
it is not weakened, because a session that tallied itself as a completion would make it 2 and still
fail. That test failing on the first full-suite run after the change is what surfaced this; the fix
was not assumed harmless.

**Review method, stated as §6 requires.** The first cut of this slice was self-reviewed only, and the
two tests that now cover post-`next` refusal and post-`next` timeout did not exist: the terminal set a
"route reached" flag, so a guard that awaited `next` and *then* answered had its answer discarded
while the socket was upgraded anyway. An independent adversarial review agent was then run over the
diff, and that is what found it (its finding 1), together with the related claim that a middleware
cannot revoke an upgrade by editing headers because the validator reads the private parsed store
(its finding 4, the same two-store defect as `PROB-0005`, closed in 0.2 by deleting the public map).
Both were checked against the code before
being accepted, and the fix was then falsified rather than assumed: replacing the status guard with
the old always-hand-off behaviour reproduced the bug exactly —
`a_middleware_that_answers_after_next_refuses_the_upgrade`,
`a_middleware_hanging_after_next_times_the_upgrade_request_out` and the post-`next` half of
`a_panicking_middleware_answers_the_upgrade_request_with_500` all failed with `left: 101`, `right:
403 / 504 / 500` — and all pass with the guard in place. The other review findings were declined with
reasons, not silently: an Axum-shaped `Response`-returning ws handler stays a recorded redesign
(item 18) rather than a quiet API break.

The design
alternatives were weighed in-tree: giving `ws` handlers a `Response`-returning signature (the Axum
shape, where the upgrade *is* the response and middleware then edits it for free) would move socket
handover and the WS memory re-pricing into the response path and change every `app.ws` handler in
existence — a redesign of `ServeOutcome`, not a patch, and it is recorded as remaining-work item 18
instead of being done quietly. The falsification attempts that shaped what is here: the counter test
kills "the handler ran anyway", the same-connection follow-up kills "the refusal broke keep-alive",
the group test kills "app-level capture is enough", the late-registration test kills "the chain is
looked up live", the inverted scheduling test kills "the change is cost-free to the tallies", and the
three status-guard tests kill "a guard that answers after `next` is respected".

**Handoff-edge round (same day, 452 → 455).** The owner accepted the defect fix but ruled the
WebSocket-middleware surface open until four specific cases were settled, so they were settled one at
a time rather than declared covered:

1. **Double `next`.** Not expressible, so no test was written for it — and "not expressible" was
   measured, not read off the signatures: a scratch middleware that calls `next(req)` twice on the
   same request fails to compile with `error[E0382]: use of moved value: req`, move occurs because
   `req` has type `aether::RequestContext`, which does not implement the `Copy` trait. A second call
   is therefore a compile error, which is a stronger guarantee than a runtime test. The scratch file
   was deleted after capturing the error; nothing about it is checked in. The handover slot itself is
   `Option<RequestContext>`, so even a hypothetical second terminal visit cannot duplicate a
   session — it overwrites the pending request.
2. **A chain run in a separate task, and a chain future dropped.** Both are now wire tests, and the
   supported behaviour is stated in the `ws` docs and on `RequestContext::headers`: what the client
   receives is *the response the guard returns*, and nothing else.
   `a_middleware_that_forks_the_chain_still_answers_the_upgrade_request` spawns `next(req)`, lets it
   complete with the terminal's `101`, and returns 202 — the client gets 202 and the handler never
   starts. `a_middleware_that_abandons_the_chain_future_refuses_the_upgrade` builds the chain future
   and drops it: the client gets the guard's 503 and no session starts. Measuring that case corrected
   the explanation this round would otherwise have shipped with — the handover slot is filled
   *synchronously when `next` is called* (the terminal's closure body records the request before
   returning its future), so an abandoned chain does reach the terminal. The refusal comes from the
   status identity, not from the terminal being unreachable, which is exactly why the guard compares
   statuses instead of flagging that the terminal ran. Falsified: with the status guard
   replaced by `.filter(|_| true)` (the old always-hand-off behaviour), the fork test fails
   `left: 101, right: 202` and the abandoned test fails `left: 101, right: 503`; both pass again once
   the guard is restored.
3. **The header store.** Settled as a contract, not as an accident: what a guard reads is the store
   the handshake validator is fed from, and a guard cannot rewrite it — refusal has to be a returned
   response. At the time the map itself was public and writes to it were handler-visible but not
   framework-visible; the two directions are now pinned by
   `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` (renamed from the header-map test when
   the map was deleted). Making the map write-through to the parsed store was the alternative, with
   a real cost — it would let a guard revoke an upgrade by mutation, which is the behaviour the
   review found surprising. That was exactly the open owner decision `PROB-0005`; 0.2 closed it the
   other way (2026-11-06), by deleting the public map and reading one store through
   `header`/`header_values`/`headers`.
   Writing this test also corrected a wrong belief carried in the earlier docs: `allows_upgrade` is a
   **transport capability flag**, not a header scan, so a plain non-upgrade `GET` on a WS route does
   run the chain and the `400` comes from the validator afterwards; only the HTTP/2 transport case and
   a non-`GET` method are refused before the chain. §14 measured *which* component refuses the
   non-`GET` case, and it is the router, not the WebSocket branch.
4. **Re-run on the final tree.** `cargo test --workspace` → **455 passed / 0 failed / 1 ignored**
   (262 unit + 188 integration across 15 targets + 5 doc), `cargo build --workspace --release` clean.
   "Final" meant that stage's tree; §5 and §10 carry the current counts, and §14 the tests that moved
   them. Windows only; `PROB-0012` remains open and no performance claim is made here.

**Limits recorded, not hidden.** A guard refuses by returning a response, not by mutating the
request: `RequestContext` exposes no mutable header store at all (one store, read through
`header`/`header_values`/`headers`; `PROB-0005` closed in 0.2), and a middleware that keeps
status `101` while editing the response around it has those edits dropped — the CORS / security-header
/ compression pass does not apply to the handshake line either. Nothing wraps the session after the
socket is handed over (that remains item 9). A WS route reached over HTTP/2 still answers 400
*before* middleware, exactly as a non-`GET` method does — RFC 7540 §8.1 forbids the `Upgrade`
mechanism there, so there is nothing for a guard to grant. An upgrade request now consumes a handler
admission slot briefly; under a full handler queue it can be refused with 503 like any other route,
which is the intended back-pressure but has no dedicated test. **Behavioural break for embedders:** a
chain that previously guarded only HTTP routes now genuinely guards `app.ws` routes too, so an app
that relied on a WS endpoint being open behind its auth middleware starts returning that middleware's
refusal. Windows only, as everywhere in this report: `PROB-0012` (Linux/macOS) remains open. No
performance claim is made — the added work is one short chain execution per upgrade, and no benchmark
was run for it.

## 14. Addendum — the four behaviour tests this audit opened, 2026-10-22

Gate 5 found four behaviours the documentation asserted that no test exercised: two pre-chain
WebSocket refusals, the hand-built-`101` edge, and shutdown-abort of a live session. The owner
approved writing them before any commit was pinned, with the condition that each test assert the
behaviour rather than a status code, and fail when the fault it targets is present. All four are in
now, and each was falsified by introducing its target fault in a scratch build and reading the
failure. Nothing here changed framework code: every patch below was reverted and the restored files
were checked (`cmp` against a pre-falsification copy, then a full re-run).

| Test | Target fault, injected | Measured failure |
|---|---|---|
| `a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain` | WS branch made reachable for `POST` (register the route for `Post` as well as `Get` in `app.ws`) **and** the `method_enum != Method::Get` gate in the WebSocket branch (`aether/src/serve.rs`) changed to `if false` | `left: 101, right: 405` — the framework upgraded a `POST`. With the same route registration and the guard restored, the test passes on `405` with the chain counter at `0` |
| `a_websocket_route_reached_over_h2_answers_400` | the `!allows_upgrade` transport gate (`aether/src/serve.rs`) → `false` | `left: 1, right: 0` on the middleware counter while the status **stayed `400`** — the assertion this round added is the only one that sees the fault |
| `a_websocket_route_reached_over_h2c_answers_400` | same patch, cleartext h2c | same failure, `left: 1, right: 0` |
| the §14 pin then named `a_hand_built_101_from_a_middleware_is_not_an_upgrade` (it became `a_hand_built_101_from_a_middleware_is_refused_with_500` when the fix landed, §15) | the candidate fix itself: `None if chain_response.status == 101 => Response::status(500, ..)` | `left: 500, right: 101`, and **only** this test moved — the other 12 tests selected with it (the whole WebSocket-chain family plus the upgrade/echo test) stayed green, so the candidate fix is surgical. §15 records why this was read as *test sensitivity*, not as proof of the fix's consequences |
| `shutdown_aborts_a_live_websocket_session_without_a_close_frame` | `session.abort()` → detach (drop the abort, keep the task alive) | the client's post-shutdown read timed out: "the connection stayed open after graceful shutdown" |

Two claims the falsification corrected, rather than confirmed:

1. **The `405` for a non-`GET` on a WS route is the router's, not the serving core's.** `app.ws`
   registers the path for `GET` only [`framework/aether/src/app.rs`](../aether/src/app.rs), `self.router.add_route(Method::Get, path, id)` in `ws`,
   the router has no HEAD→GET fallback
   [`framework/router/src/node.rs`](../router/src/node.rs), `RouteNode::get_handler`, and a method mismatch on a
   matched path yields `MatchResult::MethodNotAllowed`
   [`framework/router/src/trie.rs`](../router/src/trie.rs), `RadixRouter::leaf_result`, which `serve.rs` answers
   (`405` + `Allow`) in the `MatchResult::MethodNotAllowed` arm of
   [`framework/aether/src/serve.rs`](../aether/src/serve.rs), without dispatching. So the WebSocket
   branch's own `405` is a **second line of defence that normal
   registration cannot reach**. Making it reachable proved it is not decorative: with the route
   registered for `POST`, the guard answered `405` before the chain and removing it handed a `POST`
   a `101`.
2. **The hand-built `101` is already body-less.** The HTTP layer refuses a body and a
   `Content-Length` for any 1xx ([`framework/http/src/response.rs`](../http/src/response.rs), lines 292-304 on that
   tree — both gated by `status_forbids_body` at its line 338 and unit-tested at lines 730-748),
   so what the client receives is `HTTP/1.1 101` with the ordinary post-pass headers (`Date`,
   `X-Request-ID`), no `Sec-WebSocket-Accept`, no body, and a connection that still serves HTTP/1.1 —
   the earlier draft's "post-pass included, body and all" was an assumption the wire did not support.

**Suite after this round.** `cargo test --workspace` → **458 passed / 0 failed / 1 ignored**
(262 unit + 191 integration across 15 targets + 5 doc tests), `cargo check --workspace --all-targets`
clean. The `cargo clippy --workspace --all-targets` gate is still red for the same pre-existing
reason recorded in §5 (`clippy::unused_io_amount` at line 452 of `tests/tests/tls_e2e.rs`), and the release
build was not re-run for these three tests. Windows only: `PROB-0012` (Linux/macOS) stays open, and
no performance claim is made — these tests add server-side assertions, not measured throughput.

**Still open after this section.** The behaviour decision for the hand-built `101` (this section
reproduced it; the owner ruled the fix must be a separate, reviewable change, not something folded
into documentation or commit-splitting work) — **taken and implemented in §15**. The duplicate method
gate is documented as defence in depth, and the owner ruled on 2026-10-22 to **keep** it, each layer
owning its stated responsibility (§15). Shutdown still aborts without a close frame — tested, not
improved: a conformant client sees a TCP
FIN where RFC 6455 §7.1.2 would prefer a `/close`, and changing that is a protocol-behaviour
decision of its own. **Closed 2026-10-27 (track 86):** that decision was taken and implemented — a
session now sends `1001` and ends on it (§4.6, known gap 6).

## 15. Addendum — the hand-built `101` is refused: the protocol-correctness fix, 2026-10-22

§14 reproduced a fault and deliberately stopped there, because the owner ruled that the behaviour
decision had to be its own reviewable change: *"إذا كان إصلاحه ضروريًا لصحة البروتوكول، فلا تصفه
بأنه مجرد تحسين تجميلي"* — and, rejecting my earlier inference, that a test which moves when a
candidate patch is injected proves **the test's sensitivity, not the correctness of the patch's
consequences**. This section is that change.

**The decision (owner, 2026-10-22).** `101 Switching Protocols` asserts that a protocol transition
happened. A middleware that returns one while the terminal never ran performed no transition, so it
must not be allowed to announce one: the claim is refused with `500 Internal Server Error`.

**The implementation** — one match arm, in the WebSocket branch of the shared serving core, before
the fall-through that used to send the claim
[`framework/aether/src/serve.rs`](../aether/src/serve.rs) (`None if chain_response.status == 101 =>
Response::status(500, "Internal Server Error")`). The handoff is still authorized by the recorded
request context filtered on the sentinel status, so the only response this arm can reach is a `101`
with no handoff behind it.

The four conditions the owner attached to acceptance, each measured:

1. **Applies only to a middleware `101` that never ran the upgrade path.** A guard that calls `next`
   records the context and takes the `Some(ctx)` arm untouched. Proof in the same test as the refusal:
   the same route, with the same middleware, still answers `HTTP/1.1 101` with the RFC 6455 accept
   vector `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`, starts the session, and the client reads the echoed frame
   back [`framework/tests/tests/http_e2e.rs`](../tests/tests/http_e2e.rs),
   `a_hand_built_101_from_a_middleware_is_refused_with_500`).
2. **HTTP/2 and h2c unaffected.** Over those transports a WS route is refused `400` before the chain
   (`allows_upgrade` is a transport capability), so the arm is unreachable there; measured by the
   `http2_e2e` target holding at **27 passed** across the change, including both
   `a_websocket_route_reached_over_h2…answers_400` counter tests.
3. **No body and no contradictory headers on the `500`.** The refusal is a freshly constructed
   response. The injected guard was given a `101` carrying `Upgrade: websocket`,
   `Connection: Upgrade`, `Sec-WebSocket-Accept: not-a-real-accept-token` and a JSON body, and the
   wire shows none of it: `HTTP/1.1 500 Internal Server Error` with exactly `X-Request-ID`, `Date`
   and `Content-Length: 0`, an empty body, no session started, and the same socket still answering a
   following `GET /ping` with `200`.
4. **The test fails if the old behaviour returns.** Deleting the arm reproduces
   `left: 101, right: 500`; with the arm in place the test passes. Both directions were run.

**Scope is tested, not asserted — and since 2026-10-25 the refusal is no longer scoped to the handoff
arm.** The rule moved to the core's single response exit in `aether/src/serve.rs`, so **any** `101`
the frontend did not synthesize for a handoff is answered `500`, on every route and on both protocols:
a handler's claim on an ordinary route takes the same answer as a guard's, and over HTTP/2 the client
receives that `500` instead of the stream reset `h2` produced for such a head. What
`a_101_without_a_handoff_is_never_served` still pins is the boundary of the *class*: a `103`
returned on a WebSocket route is still `103`, and a `418` is still `418`. That renamed test is now
both a detector and a guard — it fails without the core check (`left: 101, right: 500`) and fails if
the condition widens to `status < 200` (`left: 500, right: 103`) — and the HTTP/2 half is its own
wire test, `a_101_from_a_middleware_is_refused_over_h2`.

**One deliberate deviation from the drafted option.** The register's candidate (a) read "downgrade …
**and log it**". There is no extra log line, and not for lack of a place to put one: the response
post-pass in `aether/src/serve.rs` already records `response.status` per request through
`LoggerMiddleware` (`if self.logger.enabled { self.logger.log(…, response.status, …) }`, disabled by
default on the hot path), so the fabricated upgrade reaches the access log as the `500` the client
gets. A separate `eprintln!` — the only other diagnostics surface in this crate — would write to
stderr once per request on a path a client drives through an application-owned guard, which is an
unbounded-write risk rather than a diagnostic gain. A distinct signal (once-per-route, or a counter)
is its own decision with its own measurement. Recorded as a deviation rather than dropped silently.

**Suite and build after this change.** `cargo test --workspace` → **459 passed / 0 failed / 1
ignored**, recounted mechanically from the per-target `test result:` lines: 262 unit + **192**
integration across 15 checked-in targets + 5 doc; `http_e2e` 44 → **45** (one pin replaced by a
contract test, one scope test added), `http2_e2e` 27 unchanged. `cargo check --workspace
--all-targets` clean, `cargo build --workspace` finished clean. `cargo clippy --workspace` is still
**39** diagnostics and still carries no correctness-class lint, so this change added none; `cargo
clippy --workspace --all-targets` is still **red** for the same pre-existing reason recorded in §5
(`clippy::unused_io_amount`, line 452 of `tests/tests/tls_e2e.rs`), which this round did not touch — the
release build was not re-run.

**Re-run after the last edit.** The final change to the code was comment-only: the short note above
the WebSocket method gate was replaced by the two-layer responsibility comment Decision 2 asks for
(`aether/src/serve.rs`). Because a comment in a linted file moves coordinates, the whole gate was run
again on that tree and every figure above is from it: `cargo test --workspace` → 459 / 0 failed / 1
ignored (262 + 192 + 5, `http_e2e` 45, `http2_e2e` 27), `cargo check --workspace --all-targets` clean,
`cargo build --workspace` clean,
`cargo clippy --workspace` → 39 diagnostics and 0 errors (the per-crate `generated N warnings`
summaries are 7, so 46 lines total), `cargo clippy --workspace --all-targets` → still exactly one
error, the same pre-existing line 452 of `tls_e2e.rs`. The `serve.rs` locations went from 442/458 to
445/461 —
the fourth shift §5's invariant note has recorded — and none of the four (`87`, `192`, `445`, `461`)
falls inside the upgrade branch (239–346). That is the invariant form, and the coordinate is only its
snapshot.

**Independence of the tests, from §14 and this one.** Each of the five runs green alone
(`-- --exact <name>`), the two files run green with `--test-threads=1` (deterministic order) and with
the default parallel harness, and the only process-global mutable the serving path exposes
(`REQUEST_COUNTER`, `aether/src/app.rs`, which numbers `X-Request-ID`) is asserted as `.is_some()`
rather than as a value. Ports come from `127.0.0.1:0`, every test builds its own `App`, and every
counter is a test-local `Arc<AtomicUsize>`.

**Limits of this section.** Windows only — `PROB-0012` (Linux/macOS) stays open, and nothing here was
measured for performance; the change replaces a response object on a path no benchmark reaches, which
is a code-reading statement, not a measurement. Still open by decision, not oversight: a
middleware-authored `101` on a **non**-WebSocket route (register item A6), and ~~whether graceful
shutdown should send a WS close frame instead of aborting the session task~~ — **decided
2026-10-27 (track 86)**: it does, and only a handler that never returns to `recv` is aborted (§4.6).

**Independent agents:** none were used for this change; the falsifications were run in this
worktree, reverted here, and verified by grep plus a full workspace re-run.

---

## 16. Addendum — extension payloads priced, and the owner-backed residue (PROB-0016, PROB-0027), 2026-10-26

The roadmap asked for track 74 to close these two *by measurement, not argument*. Both closed, and
the measurement is what decided the shape of the first one.

**What was measured.** `framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs` is the probe
that produced every number below; it is a standalone crate (`cargo run --release`) whose map shapes are
the ones `extensions.rs` builds, and it prices each case with a wrapper allocator that tags every block
with the epoch that allocated it. Frees of blocks belonging to another epoch — this run excluded
16 777 472 B of them, mostly the JSON fixtures the probe builds between cases and the runtime's own lazy
work — are counted separately and never enter a row, which is what a raw before/after counter cannot
do: it charges whichever case happens to trigger the foreign free. Integrity of the run: 0 full probe
chains, and every case freed everything it allocated.

| Question | Reading |
| --- | --- |
| Container cost per entry, **the shape that ships** (`Entry { value, price, charged }`) | 1 entry **264 B**, 2 entries 268 B, 4 entries 684 B, 8 entries 1 288 B, 16 entries **2 492 B** → (2 492 − 264)/15 ≈ **148 B marginal** |
| Container cost per entry, the shape before this track (`Box<dyn Any>` as the value) | 1 entry 200 B, 2 entries 204 B, 4 entries 492 B, 8 entries 904 B, 16 entries 1 724 B → ≈ 101 B marginal — **the number the first draft of the constant was taken from** |
| A 1 MiB payload held in the map | `String` in map **1 048 860 B**, `Vec<u8>` in map 1 048 860 B, `Vec<String>` 2 228 508 B |
| Wire bytes → parsed JSON kept in the map | 989 → 7 915 B (**8.00×**), 15 921 → 112 821 B (7.09×), 259 937 → 1 793 869 B (6.90×), 4 237 633 → 28 727 357 B (**6.78×**) |
| What the budget already priced | `CONN_BASE_MEMORY` 32 768 B, `STREAM_BASE_MEMORY` 16 384 B, body and write buffers by capacity |

Both container rows come from one run of one artifact, and the second row is why the first one exists.
The constant was drafted at 128 from the pre-pricing shape's ~101-byte marginal cost, and the same track
that measured it gave `Entry` two more `u64` fields — 16 bytes more per slot, and the measured container
moves with it: first entry 200 → 264 B, marginal ~101 → ~148 B. A container price measured against a
shape the code no longer has is a guess wearing a measurement, so the constant was re-derived against the
shape that ships before it was committed. (16 and 24 entries read the same peak, i.e. the table did not grow between them; the marginal
figure is therefore the 2-to-16 slope, not a promise about each individual insertion.) The first entry costing 64 B more rather than the 16 B per slot it works out to is a shape
this probe measures and does not explain — it does not have to: the price covers the measured peak, not a
model of where the peak comes from.

**The option that measurement refused.** The roadmap's cheaper alternative was to charge the map's
*capacity* and document the values as unpriced. Capacity is 264 B for a map's first entry and ~148 B per
further entry; the retained payload reachable through one middleware is 6.78–8.00× the body it parsed, so
at the default 2 MiB `max_body_bytes` a single request can keep ~14 MB alive in the map. Charging capacity
would have registered a number two orders of magnitude off while letting the docs claim the gap was closed
— a fake closure, refused on the numbers rather than on taste.

**What shipped instead.** `EXTENSION_ENTRY_MEMORY = 160` (`framework/aether/src/scheduling.rs`) prices
the container and is charged for every entry unconditionally, together with `size_of::<T>()` — the inline
handle, which is what a value that owns nothing actually costs. 160 rounds the ~148-byte marginal up, and
the residual on a map's first entry is a one-time ~104 bytes (264 − 160), stated rather than smoothed away.
Heap a value points at is charged only when the inserter declares it: `Extensions::insert_sized(value,
owned_heap)` and `RequestContext::insert_extension_sized(value, owned_heap)`. The charge rides the request's
own handler registration, attached in `dispatch` between admission and the handler call, so it is released
with the request by construction and cannot orphan. A budget that refuses the increase registers nothing and
**still serves the answer** — the same best-effort contract `price_response_body` follows — and the
refusal counts in `metrics.memory_shed`. Three things stay invisible to it, stated rather than implied
to be handled: a heap nobody declared, a value grown through `get_mut`, and the tail a `Vec` reserves
beyond its length.

**The HTTP/2 half, verified rather than assumed.** `dispatch` is the shared core's handler runner, so
h2 needed no second implementation — and `a_declared_extension_is_priced_on_the_http_2_path_and_released_with_the_stream`
proves it on a live server: two routes over one h2c connection, each handler sampling the layer from
inside the server, differing by 524 472 B (160 + 24 + 524 288), which the test computes from the
constants rather than writing out. Deleting the `attach_extension_charge` line in `serve.rs` makes that
assertion report `left: 0`, which is the reading that says the test measures the wiring and not the
arithmetic.

**PROB-0027 closed on a rule, not on a copy.** An owner-backed block (`Bytes::from_owner`) can never
be recovered zero-copy by `bytes` 1.12.1 — `owned_is_unique` answers `false` even for the sole handle —
so the three candidate closes were: ask the owner for its capacity (a public-API break for whoever
implements the trait), copy the body to price it (allocating exactly what the charge exists to bound),
or hold to the rule. The rule is documented on `IntoResponse` and in `price_response_body`: an HTTP/2
response body is built with `Bytes::from`, never `Bytes::from_owner`. It is a statement about code this
repository controls, because `every_body_this_framework_produces_is_a_block_the_budget_can_name` calls
the actual producers — `IntoResponse for String`, `IntoResponse for (u16, String)`,
`Response::text_dynamic`, `Response::html_dynamic`, `Response::file` (the fs-read asset shape, which is
also what `static_dir`, the embedded-asset route and the compression middleware's `Bytes::from(Vec)`
produce) — and asserts each one's registration rises by its block without the payload moving. Mutating
`IntoResponse for String` to `Bytes::from_owner` makes that test fail by name, which is what distinguishes
the rule from an expectation.

**Gates on this tree.** `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --
-D warnings` clean; `cargo test --workspace` **526 passed / 0 failed / 1 ignored** over 50
`test result:` lines; `cargo build --workspace --examples` clean; `.project-governance` tool tests
`Ran 42 tests ... OK`. `validate` remains **red** on the four pre-existing recycled-ID findings
(PROB-0007, PROB-0017, REQ-0001 ×3 and the open-vs-history line it produces) — registered as
PROB-0019, not caused by this slice, and recorded as failed rather than worked around.

**No throughput claim.** These are allocation-accounting numbers measured by a counting allocator on
one Windows/MSVC host. Nothing here is a request-per-second or latency result.

## 17. Addendum — the body limit is enforced twice, on two quantities, and only one of them was named (PROB-0028), 2026-10-26

PROB-0028 read as: `max_body_bytes` is advertised as a 2 MiB body cap, while what HTTP/1.1 actually
applies first is a budget over the **wire** bytes a connection has buffered. The measurement confirmed
the mismatch and the reason for it, and located the two enforcements:

- the parser bounds the **decoded** body — the declared `Content-Length` at line 262 of
  [framework/http/src/parser.rs](../http/src/parser.rs),
  the cumulative chunked decode at its lines 647 and 664, and HTTP/2's DATA payloads at line 432 of
  [framework/aether/src/http2.rs](../aether/src/http2.rs);
- the connection bounds the **wire** at line 2330 of
  [framework/aether/src/app.rs](../aether/src/app.rs):
  `conn_buf.len() + n > max_header_bytes + max_body_bytes`, which on the defaults is a
  2,105,344-byte budget, answered 413 with `Connection: close`.

Only HTTP/1.1 has both, because only HTTP/1.1 buffers framing and the decoded copy at the same time;
on HTTP/2 the advertised number *is* the decoded cap. A chunked body pays six wire bytes per payload
byte (`1\r\nx\r\n`) plus a fixed 62-byte head, so the reachable payload ceiling is arithmetic the test
measured rather than derived: **350,879 payload bytes served with 2,105,341 on the wire, 350,880
refused with 2,105,347** — 16.7 % of the 2 MiB the field is named after, and a fifth of what the
parser would have accepted. `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes`
([framework/tests/tests/http_e2e.rs](../tests/tests/http_e2e.rs):1654)
holds both halves on two live connections and asserts the served one reaches the handler whole. Three
mutants were run against it, each with its red pre-declared: disabling the guard's wire comparison
answers the refused request 200 — so the refusal is the guard's, not the parser's, which would have
taken that body; `FITS` at 350 880 fails the served half with a 413; `FITS` at 350 873 fails the
refused half with a 200, so the boundary is that byte and not something earlier. Both files were
restored from backups and the diff re-read as 8/3 and 63/0, which is the check that the mutations left.

**What changed is the name, not the bound.** The two quantities are now stated together on
`ServerLimits::max_body_bytes` and `AetherConfig::max_body_bytes`, in the guard's own comment, in
`framework/README.md`'s `AETHER_MAX_BODY_BYTES` row and security-limits bullet, in the matrix's
Body-budget row, and in the human-facing set — `docs/07` §4 (which also had the correct sum but no
distinction), both `ServerLimits`/`AetherConfig` blocks in `docs/09`, the RFC-9112 hardening items in
`docs/00` in both languages, and the CL check in `docs/04`, whose two `parser.rs` coordinates were
dead (`129-184` → `207-265`, `594` → `689`) and are re-derived.

**Two changes refused, with reasons.** Moving the bound from wire to decoded bytes was refused: the
wire bound is the memory price — every buffered wire byte is resident next to the decoder's copy until
the request completes (that is how PROB-0008 prices connection memory) — and a decoded-only check
would let a framing-heavy request hold more than the budget registers. Renaming the field to say
"wire" was refused too, because it is one field read as two quantities on two frontends; and adding a
separate framing-overhead cap was refused because the wire bound already prices framing, so a second
cap would refuse legitimate clients for a cost the budget has already taken. The follow-up that would
still be worth having is a knob that lets an operator name the two budgets separately instead of only
as a sum; it is not registered as a defect, since nothing measured here shows the sum being wrong.

**Gates on this tree.** `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo doc --workspace --no-deps` clean of **warnings**, which it was not
before this slice: seven phantom links are gone — two redundant `[`size_of`]` targets introduced by
track 74's doc edits, one `MultipartError` path that named nothing, one link to the private
`ChunkedBodyDecoder::feed` in `parser.rs` (all four in crates this slice touched), plus three
pre-existing ones in crates it did not (`benchmarks`' `[job_count]` run line, `runtime`'s
`aether::AetherApp` link into a non-dependency, `scheduler`'s unqualified `SchedulerEngine::admit`);
`cargo test --workspace` **527 passed / 0 failed /
1 ignored** over 50 `test result:` lines (305 unit + 217 integration + 5 doc, `http_e2e` 52) — a reading
of the pre-commit tree; the same ladder re-run on the committed tree is §18 and it was **red**;
`cargo build --workspace --examples` clean. `.project-governance` tool tests: **`Ran 42 tests`, exit 0
— after this track's own closure broke one of them.**
`test_recycling_ratchet`'s forged-reuse sensitivity test hard-coded `PROB-0028` as "an ID that is open
today"; closing it left the forged reuse printing only `[RECYCLED ID]`, which the test reported as a
detector failure. The fixture now derives its pair from the live ledger (one ID allocated once and
closed, one allocated once and still open) and refuses to run when no such pair exists;
`STANDING_RECYCLED` and `STANDING_OPEN_ON_RECYCLED` are untouched, and the repaired test still rejects
all four mutated detectors (no `[OPEN VS HISTORY]` at all, one for every recycled ID, a recycled line
skipped when its ID is closed, and every `[RECYCLED ID]` line dropped). `validate` remains **red** on
the same four pre-existing recycled-ID findings (PROB-0019) — recorded as failed, not worked around.

**No throughput claim.** The wire/decoded boundary is a byte count read off a live socket, not a rate.
Nothing in this section is a request-per-second or latency result.

## 18. Addendum — a memory-release test that measured the harness instead of the server (PROB-0033), 2026-10-27

§17's green count came from the tree before its own commits. Re-running the same ladder on the
committed tree returned `TEST=101`: `a_declared_extension_is_priced_on_the_http_2_path_and_released_with_the_stream`
failed at line 1487 of `framework/tests/tests/http2_e2e.rs` on `e81efed` — the assertion sits at line 1502 after
this section's own edits — with `a declared extension outlived its HTTP/2 stream` (`left: 32768,
right: 0`) while passing 3/3 run alone: a load-only failure, and the report keeps it red rather than
folding it into the 527.

The cause was the test, not the server. Both memory-release tests cited as evidence in this report —
the §16-era extension-pricing test and the older finished-stream test, in
`framework/tests/tests/http2_e2e.rs` — took their baseline as `layer.resources().registered_memory_bytes`
immediately after the client handshake. The connection's own charge — `CONN_BASE_MEMORY`, 32 KiB,
line 61 of `framework/aether/src/scheduling.rs` — is admitted by the server's accept task, not by the client's
handshake, so under suite load the sample read 0 and the settle assertion then waited for a stream's
charge to fall to a baseline that never included the connection. The third test of that shape in the
same file (line 888) already did it correctly: wait for `CONN_BASE_MEMORY`, then assert it.

Both raced sites now wait and assert the same way. The assertions are shown to have teeth rather than
assumed to: with the handler's admission guard deliberately leaked in `serve.rs`'s `dispatch`
(`std::mem::forget`), both tests fail at the settle assertion (65552 vs 32768, and 540856 vs 524472) —
so the baseline is exactly the connection's charge and a leaked stream registration is caught. After
the fix, three consecutive `cargo test --workspace` runs each read **527 passed / 0 failed / 1
ignored** over 50 `test result:` lines, and `cargo fmt --all -- --check` / `cargo clippy --workspace
--all-targets -- -D warnings` remain clean (0 bytes / 0 warnings). One fixed-pause wait of the same
class remains in that file — line 1531, a `150 ms` sleep before the stream's charges are read — and is
recorded as unverified rather than rewritten on suspicion; the file's other two `150 ms` occurrences
are an elapsed-time assertion (line 1251) and a sleep inside a bounded poll loop (line 841). Nothing in
this section is a rate or latency result.
