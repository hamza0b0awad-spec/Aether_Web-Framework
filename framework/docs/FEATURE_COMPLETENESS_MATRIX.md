# Aether Feature-Completeness Matrix

**Audit date:** 2026-10-18
**Aether version:** workspace `0.2.0` (Rust 1.98.1, edition 2024)
**Scope:** the implementation in `framework/` is the source of truth. Nothing below is marked
implemented because a type, config field, or dependency exists; each `VERIFIED_IMPLEMENTED`
row was exercised by tests or a live server check.

**Status codes**

| Code | Meaning |
|---|---|
| `VERIFIED_IMPLEMENTED` | Behavior exists and was exercised by a test or live request. |
| `PARTIALLY_IMPLEMENTED` | Works, but a documented or important part of the behavior is missing. |
| `NOT_IMPLEMENTED` | No implementation. |
| `UNWIRED_IMPLEMENTATION` | Code and tests exist in a workspace crate that no shipped crate depends on, so the behavior is unreachable through `aether`. |
| `BROKEN` | Implemented but produces incorrect behavior. |
| `UNVERIFIED` | Cannot be verified in this environment. |
| `NOT_APPLICABLE` | Not a framework responsibility, or provided by the ecosystem for all frameworks. |

**Evidence used**

- `cargo test --workspace` — **946 tests, 0 failed, 1 ignored** on the current tree (re-measured
  2026-11-06 on the track-143 tree: 495 in-crate, 430 integration across 30 targets, 21 doc over 15 doc
  runs; the same per-target recount was 474 on 2026-10-23, twice, identical, and the 467 tests added
  since are the slices recorded per track in
  `.project-governance/WORKING_CONTEXT.md`. The
  2026-10-21 tree measured 455, the four behaviour tests took it to 458, the `101` fix — which
  replaced one pin with a contract test and added a scope test — took it to 459, PROB-0014's
  three tests (two `http2_e2e` wire tests, one `aether` derivation unit test) took it to 462,
  PROB-0008's capacity pricing — three `app.rs` unit tests plus one `scheduling` wire test — took
  it to 466, the two `aether-websocket` idle-deadline tests
  (`silent_peer_is_failed_by_the_idle_deadline`, `a_ping_resets_the_idle_deadline`) took it to
  468, and the response-block price plus the h2 block price plus the handler-error body test —
  three `app.rs` unit tests, one `scheduling`, one `http2_e2e` and one `http_e2e` wire test — took
  it to 474. The recount track 60 published was 488, track 62's three `app.rs` worker-count tests took it to
  491 and track 65's one `app.rs` bind-advice test took it to 492; track 67's one `http2_e2e`
  paced-reader test took it to 493, track 68's four `http2.rs` unit tests — the response-block
  pricing shapes and the refused-upload registration — took it to 497, track 69's six — three
  `transport.rs` classifier tests, two `trie.rs` catch-all-position tests and one `http_e2e`
  HTTP/1.0-upgrade wire test — took it to 503, and track 70's three — two `http_e2e` wire tests and
  one `scheduling` wire test, each driving an over-budget chunked upload over a live socket so the
  413 must arrive rather than merely be written — took it to 506, and track 71's three — two
  `trie.rs` unit tests and one `extractors` wire test, each pinning that a radix `Param` level
  shared by two sibling routes reports each route's own capture name, in declaration order — took it
  to 509, and track 72's four — three `aether-websocket` unit tests and one `scheduling` wire test,
  each driving a session whose peer stops consuming so the write side, not the read side, is what
  releases the slot and the registered buffers — took it to 513, track 73 added none (its work was
  lint-shaped rewrites and markdown), and track 74's thirteen — nine `extensions.rs` unit tests for the
  priced map, one `http2.rs` unit test that calls every body producer this framework has, two
  `extensions_e2e` wire tests that park a live request and read its registration back, one `http2_e2e`
  wire test for the same over h2c — took it to 526, and track 75's one — a `http_e2e` wire test that
  uploads a 1-byte-chunk body up to one payload byte below the connection's wire budget and one byte
  past it, so the served half and the refused half are both measured rather than derived — took it to
  527, and track 83's two — one `error_reporting` wire test that drives four handlers over a live
  socket and proves a redacted 5xx detail reaches the installed reporter under the response's own
  `X-Request-ID`, plus one `diagnostics.rs` doctest that installs a reporter — took it to
  529, and track 84's one — an `http_e2e` wire test that trickles a 4-byte body past a 100 ms
  handler timeout and is served because the receive budget is its own setting — took it to
  530, and track 85's six — two `handler_exec_semantics` wire tests (one cancelling a handler whose
  client earned an RST, one holding a handler that a keep-alive client sits on) plus two
  `http2_e2e` h2c wire tests (a per-stream `RST_STREAM` and a dropped connection) plus two
  `tls_e2e` wire tests (an abrupt TLS departure cancels, a `close_notify` half-close is still
  answered) — took it to 536, and track 86's four — three `aether-websocket` unit tests (a waiting
  session closed with 1001 and its write half shut down, a shutdown that started before the
  upgrade still seen, and a shutdown-closed socket refusing later sends) plus one `http_e2e` wire
  test added alongside the one it rewrote — took it to 540, and track 89's six — five `response_exit`
  wire tests (an error status outside `400..=599` answering `500`, a handler's `X-Request-ID` not
  surviving beside the framework's, every 1xx from a handler refused, the canonical `413` phrase on
  both the handler-error and the head-refusal path, and the config flag gating the header) plus one
  `http2` unit test that names the same `413` phrase — took it to 546, and track 90's three —
  the `aether-runtime` crate's first tests, over a real socket: a request split across two
  reads answered once, two pipelined requests in one read answered twice, and a malformed
  request answered `400` with `Connection: close` — took it to 549, and track 91's five — one
  `aether-openapi` unit test that pins the document's spec-named keys (`operationId`, not the
  Rust field name `operation_id`, which is what the generator emitted until the facade test read
  its own output) plus four `facade_wiring` tests that reach the security and OpenAPI crates
  only through `aether::` (a precomputed HS256 token verified, a token signed with another key
  rejected, the constant-time comparison, the desync audit on an ambiguous and an unambiguous
  head, and a generated OpenAPI document) — took it to 554, track 93 added none (its work was a
  serial guard inside `tests/scheduling.rs`, a test-side change that introduces no test), and
  track 94's eight — five `user_middleware` wire tests (a per-route chain covering one registration
  only, the same chain running inside the app stack and inside its group, a guard whose handler
  never runs, composition with extractors, and a per-route chain agreeing with an empty-prefix
  group) plus two `http_e2e` WebSocket wire tests (a per-route middleware wrapping the upgrade
  request, and a per-route guard refusing the upgrade before any socket is handed over) plus one
  `app.rs` doc test that compiles the guard example — took it to 562, and track 95's nine — four
  `aether-websocket` handshake unit tests (the route's declared order beating the client's, OWS and
  case sensitivity, "no offer" kept distinct from "no match", and a name being echoed only when the
  route declared it) plus five `http_e2e` wire tests (a negotiated name echoed from the route's
  declaration over two connections, a route without a declaration ignoring the offer, an unmatched
  offer refused with `400` while the socket keeps serving HTTP, a client that offers nothing still
  upgraded, and the group registration path negotiating too) — took it to 571, and track 96's
  twenty-one — one `trie.rs` unit test pinning the `Allow` value a target really answers (a `GET`
  route's `HEAD`, a `POST` route's absence of it, `OPTIONS` always, no duplication), seven `csp.rs`
  unit tests for the nonce and hash primitives (a generated token's shape, two draws never equal, a
  clone surviving the original, the source expression carrying its quotes, a token that would break
  out of those quotes refused, and the known SHA-256 of the empty source), six `security.rs` unit
  tests for the policy renderer (a policy without the placeholder emitted verbatim, the placeholder
  substituted, and the expression removed rather than emitted empty when there is no nonce), four
  `http_e2e` wire tests for the generic `OPTIONS` answer and the `Allow` on both a `405` and a
  WebSocket route's refusal, two `http_e2e` wire tests for the nonce (the header and the handler
  rendering the same value, a second response drawing a different one, and no nonce attached when
  the policy names none) and one `app.rs` doc test — took it to 592, and track 97's twelve — seven
  `static_dir` unit tests (a name the resolver would refuse is never listed, the `.br`/`.gz`
  siblings are not entries of their own, a path segment escapes every non-unreserved byte, a
  hostile file name is rendered as text, directories list with a trailing slash and a parent link,
  the root listing has no parent link, and a redirect names the directory once) plus one
  `accepts_identity` unit test (only an explicit `identity;q=0` or a bare `*;q=0` refuses the
  stored representation) plus three `http_e2e` wire tests (`directory_listing_is_opt_in_and_never_widens_the_resolver`,
  `a_directory_with_an_index_page_is_still_that_page`, `precompressed_variants_are_selected_by_accept_encoding`)
  plus one `app.rs` doc test for `static_dir_with_options` — took it to 604, and track 98a's ten
  `forwarded` unit tests plus three `http_e2e` wire tests took it to 617, and track 98b's eight
  `aether-security/src/auth.rs` unit tests, one facade unit test, three `bearer_auth` wire tests
  and two doc tests took it to 631, and track 98c's twenty `csrf.rs` unit tests plus nine
  `csrf` wire tests plus one `enable_csrf` doc test took it to 661. Every
  number comes from the same per-target `test result:` recount, not from a doc; the chain above is
  history). On the track-107 tree the split is
  **420 unit tests** in the crates (four of them the `aether-fuzz` campaign's own tests), **340 integration tests
  across 28 checked-in targets** (`http_e2e` 73 raw-socket wire tests — the
  WebSocket-middleware/upgrade-gate family, which now covers a non-`GET` refused before the chain, a
  hand-built `101` refused with `500`, that refusal's scope, an `HTTP/1.0` upgrade refused with `400`
  before any session starts, a per-route chain on a WS route, a per-route guard refusing the upgrade,
  subprotocol negotiation and its `400` refusal, shutdown sending a 1001 going-away close frame,
  the generic `OPTIONS` answer with its `Allow`, and a CSP nonce reaching both the header and the
  handler; plus the two
  buffer-limit-413
  delivery tests, one where the client half-closes after uploading and one where it neither shuts
  down nor drops its write half, plus one that pins the wire boundary itself — the same 1-byte-chunk
  framing served at 350,879 payload bytes and refused with `413` at 350,880;
  `http2_e2e` 56 (40 when this breakdown was taken, plus three streaming tests in track 99, six
  streamed-request tests in track 100b, six request-target tests in track 106 and one mounted-sub-app state
  test in track 107), whose connections are built three ways — 30 `H2Client::connect` over TLS with ALPN
  `h2`, 22 `H2Client::connect_plain` over cleartext h2c (including the two disconnect-cancellation
  tests), and 4 that hand-frame the socket: the
  preface-only close, the two concurrency-cap tests that share one overflow helper and the
  malformed-`:path` reset test, which reads the raw frame stream for its `RST_STREAM` —
  `scheduler/tests/engine.rs` 21, `extractors` 19, `scheduling` 18, **`nested_routers` 14** (new target,
  track 107), `test_client` 12, `tls_e2e` 12, `streaming` 11, `streaming_request` 8, `csrf` 9,
  `bearer_auth` 3, `sim` 9, `user_middleware` 13, `pump_head_of_line` 8, `handler_exec_semantics` 9,
  **`response_exit` 5**, **`extensions_e2e` 8**, **`facade_wiring` 4**, `memory_reprice` 4,
  `stress` 2,
  `policy_extension` 2, `error_reporting` 1, `connection` 3 in the `aether-runtime` crate, the two property suites — `http/tests/parser_properties.rs` 4 and `router/tests/router_properties.rs` 4 — `secret_rendering` 2 in `aether-security`, and `http_conformance` 7 in track 105)
  and **16 doc tests** over 14 doc runs. Track 102 took the total from 718 to 730: eight property tests in two new targets and four `aether-fuzz` unit tests. Track 103 took it from 730 to 736: one `tls.rs` unit test, two `secret_rendering` integration tests in a new target, and three `compile_fail` doc tests in `aether-security`. Track 105 took it from 736 to 750: five `aether-http` parser unit tests, one `trie.rs` unit test for `for_each_route`, seven raw-socket tests in the new `http_conformance` target, and two `aether/src/http2.rs` host-precedence unit tests replacing the one that pinned the opposite precedence (5 + 1 + 7 + 2 - 1 = 14). Track 106 took it from 750 to 756: six `http2_e2e` wire tests — the server-wide `OPTIONS *` answer over h2, a `*` on a method that is not `OPTIONS`, a `:path` outside the forms RFC 9113 allows, a `CONNECT` answered from the head, and the two `:authority`-over-`Host` tests (6, all integration; the in-crate total is unchanged). Track 107 took it from 756 to 776: four `app.rs` unit tests (the overlay answers its own type and the app the rest, one chain entry for a scope and none for an empty one, the overlay's position in the chain, the copy-on-write container), fourteen `nested_routers` wire tests in a new target, one `http2_e2e` cleartext-h2c mount test and one `Router` doctest (4 + 15 + 1 = 20). Track 108 took it from 776 to 795: four `trie.rs` unit tests for the route listing, six `aether-openapi` unit tests for the derived document, one `router_properties` property test, and eight app-level tests in the new `tests/openapi_derivation.rs` target (4 + 6 + 1 + 8 = 19). Track 109 moved no cargo count at all — its work was a governance tool and the documents it corrected. Track 110 took it from 795 to 814: eleven `http2_e2e` wire tests pinning RFC 9113 §8.1.1 on the request side (eight `RST_STREAM(PROTOCOL_ERROR)` resets for a body that does not match its `content-length`, plus a served pair of agreeing repeats, a served body with no declared length at all, and the streaming-ceiling `413` answered before dispatch), five `http_e2e` raw-socket tests pinning the HTTP/1 half of the same rule (surplus past the declared length framed as the next request, surplus that is not a request refused `400` then closed, OWS padding served, a non-ASCII-padded length refused, a repeated length refused even when it agrees), two `streaming_request` lane tests for the streamed HTTP/1 path, and one `parser.rs` unit test that the buffered body is the promise and not the buffer (11 + 5 + 2 + 1 = 19). At that reading `http_e2e` was 78, `http2_e2e` 67 — the same three ways, 30 `H2Client::connect` over TLS with ALPN `h2`, 25 `connect_plain` over cleartext h2c, and 12 that frame the socket themselves (9 through `handframed`, of which 8 are track 110's shared `lying_length_is_reset` helper, plus the 3 that write raw bytes or share the concurrency-cap helper) — and `streaming_request` 10. That reading supersedes the per-target breakdown above it, which is the
  older one its own parenthesis says so. Track 111 moved no cargo count at all: the two crates it
  deleted carried no `#[test]`, no test target and no doc example, so the totals above are unchanged
  and only the doc-test lane shrank, from 16 runs to 14, because those two crates no longer ask for a
  run. Track 112 moved no cargo count: its work is a PowerShell sweep tool and the two documents
  that name it. Track 113 moved none: prose only. Track 114 took the total from 814 to 843: fifteen unit
  tests in `security/src/cookie.rs` (the name-bound tag, the four refusal shapes, a payload that is not
  UTF-8, an empty secret, a `Debug` that prints no payload), eleven wire tests in a new target,
  `tests/signed_cookies.rs`, and three doctests — two in `aether/src/cookie.rs` and one `compile_fail`
  in `security/src/cookie.rs` (15 + 11 + 2 + 1 = 29). Track 115 took the total from 843 to 848:
  five wire tests appended to `tests/http2_e2e.rs`, which read that same accessor through the HTTP/2
  frontend's own header store — one target's line moved (67 → 72), no target was added, and no unit or
  doc test changed, because track 115 wrote no production code.
  Inside the 96-test `aether` lib total, the per-file split recounted from the files (`cargo test -p
  aether --lib -- --list`) is `tls.rs` 26 TLS unit tests (15 of them revocation, and one pinning that the derived `Debug` renders no private-key bytes), `app.rs` 21,
  `extensions.rs` 17, `request_body.rs` 5, `transport.rs` 3, `static_dir.rs` 8, `auth.rs` 1 and
  `http2.rs` 15 HTTP/2 unit tests (6 request-head
  translation, 2 response-body pricing over a scheduled connection, 1 drain deadline, 4 added 2026-10-26
  that drive the pricing step and the refusal path directly (three body-shape cases for
  `price_response_body` and one `read_body` registration case), 1 more the same day —
  `every_body_this_framework_produces_is_a_block_the_budget_can_name`, which calls the producers instead
  of describing them, and 1 in track 89 naming the canonical `413` phrase a refusal carries) — earlier
  versions of this line said 9 TLS
  unit tests, then 5 `http2.rs` tests, then 10 TLS tests and a 68-test lib total at track 100b — each
  stale when read, not a different definition.
- An independent-client run, 2026-10-20: `cargo run -p aether-tests --example h2_interop_server`
  (certificate minted in memory, nothing written to disk) served to **Node.js 24.18's built-in
  `http2`** (nghttp2 stack, not rustls/h2). Observed: ALPN `h2`, GET/HEAD/POST with body+header+query
  all answered correctly, 4 streams on one connection served concurrently (318 ms wall for one 300 ms
  handler plus three fast ones), a 5 MiB response transferred intact (flow control), 60 field lines
  in one head decoded (HPACK dynamic table), a request head over `max_header_bytes` answered **431**
  while the connection kept serving, and 120 concurrent slow streams all completed — the client
  queued at the advertised stream limit rather than being reset.
  The **SETTINGS** frame is read off the wire, not from a client API: it advertises
  `MAX_CONCURRENT_STREAMS=100` and `MAX_HEADER_LIST_SIZE=8192` and nothing else. `headerTableSize`,
  `maxFrameSize` and `initialWindowSize` reported by an earlier version of this document were that
  client's *assumed protocol defaults*, not values this server sent, and they are recorded here as a
  correction rather than silently dropped.
- Independent-client runs **against the final release binary**, 2026-10-21, after the receive
  flow-control fix below. Same example, `--release`, three listeners on one build: `strict`
  (`tls` + `require_tls(true)` + `enable_h2c()`) on 18451, TLS+ALPN on 18450, cleartext `h2c` on
  18453.
  * Node.js `http2` against **both** TLS ports: **16/16 checks** each — ALPN `h2`; GET/POST; HPACK
    under the limit; **431** over the advertised `MAX_HEADER_LIST_SIZE`; **413** for a 3 MiB body
    with the same connection then answering 200; a 5 MiB response intact; 50 concurrent streams on
    one TCP socket (10 of them on a 300 ms handler, `sameTcpSocket=true`); `te: trailers` reaching
    the handler; a WebSocket route answered **400**; a stream reset mid-handler with the connection
    still serving; SETTINGS read off the wire (`maxConcurrentStreams=100`, `maxHeaderListSize=8192`);
    HTTP/1.1 over TLS negotiated by ALPN (`alpn=http/1.1`) **and** served when the client offers no
    ALPN at all (`alpn=false`).
  * Raw sockets against the strict port — **3/3 refused**: a cleartext HTTP/1.1 request, a cleartext
    `PRI * HTTP/2.0` preface followed by SETTINGS, and a cleartext connection that says nothing each
    read **0 bytes**, i.e. closed without an answer. The same build on the permissive port answered
    the identical plaintext HTTP/1.1 request with `200 pong`, so the refusal is `require_tls`, not a
    probe artefact.
  * Strict flow-controlled uploads against the cleartext port: a 1 MiB body streamed 16 KiB at a
    time under real backpressure arrived whole and answered **200** (the full exchange closed 105 ms
    after the request on the re-run, 64 ms after the last byte was written); a 3 MiB body was
    answered **413** at 2,146,304 bytes — the limit crossed mid-stream, on a stream that had already
    had hundreds of frames accepted.
  * Hand-framed concurrency check against the cleartext port (python, raw frames — a well-behaved
    client obeys the advertised cap, so this is the only way to test it): 110 HEADERS opened on one
    connection → **100 answered, 10 RST_STREAM with error code 7 (`REFUSED_STREAM`), no GOAWAY**,
    i.e. the overflow is shed per stream and the connection is not punished.
    Re-run notes: the binary is rebuilt from the final tree and each listener mints its own
    in-memory CA, so every run above re-extracts the CA from that process's stdout rather than
    reusing a previous one.
- **Re-run 2026-10-25, from the repository.** The clients are no longer scratch files: they are
  committed beside the server as `framework/tests/examples/node_h2_interop.cjs` and
  `framework/tests/examples/h2_handframed_cap.py`, and every row of §11's table was re-run from those
  paths against a fresh release build, twice each — Node **24/24 both times** (exit 0) and the
  hand-framed cap **100 × `200` / 10 × `RST_STREAM(7)` / 0 GOAWAY / 200 `pong` after** both times.
  The `.cjs` extension was load-bearing then: the root `package.json` declared `"type": "module"`,
  so a `.js` file in this tree parsed as ESM and its `require` was undefined. Track 111 deleted that
  manifest with the AI-Studio site it belonged to (2026-11-02) and the file keeps its name - it is
  what every run above executed, and an explicit extension does not depend on a root manifest that
  may or may not be there. Re-measured on the deletion: Node v24.18.0 parses a `.js` file in this
  tree as CommonJS now (`require: function`) and `node --check` exits 0 on the `.cjs` itself.
- **The defect those last two checks found and closed (2026-10-21).** `h2` credits the peer for
  received bytes only when the application calls `RecvStream::flow_control().release_capacity(n)`;
  `RecvStream::drop` clears the buffer and does *not* credit. The HTTP/2 frontend consumed the body
  and released nothing, so a server that compiled, passed every test and answered a 5 MiB *download*
  correctly could never serve an *upload* larger than one receive window (64 KiB): it read one
  window, waited for bytes the client was no longer allowed to send, and answered 408 at the
  receive deadline (then still `request_timeout`). `framework/aether/src/http2.rs::read_body` now
  releases what it takes. The
  regression tests are `a_body_larger_than_the_receive_window_arrives_whole` and
  `a_body_over_the_limit_is_413_when_the_limit_is_crossed_mid_stream`; the 413 case had a test
  before this (`a_body_over_the_budget_is_413_and_the_connection_still_works`) but with a 16-byte
  limit and a 64-byte body, which crosses the limit inside the first window and so never needed a
  credit. Recording why it escaped: a check that only exercises the first window is not a test of
  flow control, and the same blind spot was then found in this suite's own client harness —
  `read_body` in `http2_e2e.rs` had to release capacity too, because the rule applies to response
  bodies as well, and without it a 256 KiB response stalled the *server* and surfaced as
  `UnexpectedEof` after 60 s.
  Probe honesty, same lesson: python-h2 reported "blocked by flow control after 49152 bytes,
  window 16383" against a server that a strict nghttp2 client streamed 1 MiB through in 16 ms. That
  probe was measuring its own window accounting, not the server.
- **Byte-level bounds (2026-10-21).** Three assertions no well-behaved client can produce, because
  each of them requires the client to ignore what the server advertised, so they are hand-framed:
  opening 105 streams against the advertised `MAX_CONCURRENT_STREAMS=100` answers 100 and resets the
  overflow with `REFUSED_STREAM` (code 7) and no GOAWAY, and the connection still serves the next
  stream (`streams_over_the_advertised_concurrency_cap_are_reset_not_silently_dropped`); 120 field
  lines, small enough to pass the byte bound `h2` enforces, are answered **431** by this frontend's
  own per-line budget and the connection survives
  (`a_header_list_over_the_field_line_budget_is_431`); a POST that stops mid-body and loses its
  peer at the transport leaves **0** bytes of scheduler registration behind
  (`a_peer_that_vanishes_mid_body_leaves_no_registration_behind`). A panicking handler costs one
  stream — 500 on that stream, 200 on the next, same connection
  (`a_panicking_handler_costs_one_stream_and_not_the_connection`).
- An independent cleartext run, 2026-10-21: the same example with `... h2_interop_server 18446 h2c`
  served to **python-h2 3.2.0 + hyperframe + hpack** (no Rust code on the client side) — 20/20 checks:
  GET/HEAD/POST with body+header+query, PING ACK, a fast stream answered in 2 ms while a 300 ms
  handler was still running on the same socket, a 5 MiB flow-controlled download intact, 431 over
  `max_header_list_size` with the connection surviving, a WebSocket route answered with 400, a client
  that stops after the preface closed by the `read_timeout` bound, and no PUSH_PROMISE observed on
  seven connections.
  Connection-specific *request* fields could not be tested with python-h2 at all:
  `validate_outbound_headers=False` silences its errors but not its filter, and its encoder drops
  `connection`, `keep-alive`, `transfer-encoding` and `upgrade` before they reach the wire, so a 200
  from it measures the client. Those fields were therefore sent as HEADERS blocks serialised by hpack
  and framed by hand — 8/8: each of `connection`, `keep-alive`, `transfer-encoding`, `upgrade` and a
  non-`trailers` `te` came back as RST_STREAM with no HEADERS frame at all (`h2` refuses them before
  any handler runs), `te: trailers` reached the handler untouched, and a head with no such field was
  served normally.
- An earlier raw-socket end-to-end probe against a release build: 86/86 checks covering framing,
  HEAD, routing, 405/Allow, Arabic percent-decoding, JSON errors, compression, CORS,
  chunked uploads, pipelining, panic isolation, static files, conditional GET, oversized and
  malformed requests, and slow-client timeout. The probe lives outside the repository; the
  checked-in suite supersedes it for regression evidence.
- Direct code inspection of every crate in the workspace.
- Reference-framework columns describe **documented built-in behavior** of Axum 0.8 and
  Actix Web 4.x as of the audit date. They were not independently re-tested here, and
  first-party external crates are called out rather than counted as built-in.

**Aether speaks HTTP/1.1 and HTTP/2, and no HTTP/3.** HTTP/1.1 runs over plaintext and over TLS on
one port; HTTP/2 runs **inside TLS, selected by ALPN `h2`**, on that same port, and **over cleartext
only when the application opts in** with `AetherApp::enable_h2c` (RFC 9113 §3.3 prior knowledge — the
same `ServiceCore`, the same limits, the same memory accounting). Off by default, so an h2c preface
against a server that has not opted in is closed rather than answered in HTTP/1.1 framing.
`Upgrade: h2c` is **not** implemented and is a deliberate non-goal, documented under its own row.
ALPN advertises `h2` then `http/1.1`, so a client that offers only HTTP/1.1 is still served by the
HTTP/1.1 loop. Server push is not implemented. Windows-only verification — see the Linux row in
`PROB-0012`.

---

## HTTP/1.1 and framing

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| HTTP/1.1 request parsing | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `http/src/parser.rs`; 106 crate tests passing (1 ignored) plus 4 property tests in `http/tests/parser_properties.rs`; fuzz target |
| Strict header/request-target validation | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | All four RFC 9112 §3.2 request-target forms are classified before routing: origin-form is split into path+query, absolute-form is accepted and its authority **replaces** the `Host` field (§3.2.2), authority-form is accepted only for `CONNECT` — and answered `501 Not Implemented` (RFC 9110 §9.3.6 offers `405` or `501` to a server with no tunnel, and a router `404` would blame the named host and port for a capability this server lacks) — and `*` only for `OPTIONS`; host validation and rate limiting are still decided before that capability answer. A target outside those forms (`GET hello`, `GET ?x=1`, `GET *`, `GET ftp://t/x`, a fragment, `CONNECT /x`, `OPTIONS t:443`) is `400` — before track 105, `GET hello` matched the route `/hello` and `GET ?x=1` matched `/`. More than one `Host` field is `400` on HTTP/1.0 as well as 1.1, and a missing `Host` stays `400` even when absolute-form supplied an authority. Five parser unit tests plus seven raw-socket tests in `tests/http_conformance.rs`; `OPTIONS *` answers `204` with the union of every route's methods. HTTP/2 resolves the same host: a non-empty `:authority` replaces a `Host` field rather than losing to it (RFC 9113 §8.3.1 offers reject-or-use-authority, and replacing matches HTTP/1), proved by two `header_store` unit tests |
| Content-Length framing | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Exact consumed-offset and partial-read tests, and since track 110 the whole promise measured on the wire on both frontends. HTTP/1 takes the declared length as the body's bound and no further: the surplus is left framed as the next message, so `a_body_longer_than_its_declared_length_leaves_the_surplus_for_the_next_request` gets `200` for the promised six bytes and then a real `200 pong` for the `GET` behind them, while `bytes_past_the_declared_length_that_are_not_a_request_are_400_and_close` shows the other half — surplus that cannot parse is a `400` and a closed connection, never a body. The same pair runs over the streamed lane (`a_streamed_lane_stops_at_its_declared_length_and_the_surplus_is_the_next_request`, `a_streamed_lane_whose_surplus_is_not_a_request_is_400_and_close`) and one `parser.rs` unit test pins the buffered shape (`test_content_length_surplus_is_left_for_the_next_request`). HTTP/2 delegates the promise to the `h2` crate, which enforces RFC 9113 §8.1.1 by resetting the stream — eight shapes pinned in `tests/http2_e2e.rs`, with the two served ones beside them |
| Chunked request decoding | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Single/multi chunk, extensions, trailers, pipelining tests; live upload |
| Chunked decode is incremental (resumes per read) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `ChunkedBodyDecoder`; resume-at-every-slice tests and a live body split across reads. hyper and actix-web likewise decode chunked framing as bytes arrive rather than re-reading the buffer |
| Chunked size/trailer bounds | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 1024-byte line limit, body budget, trailer count/byte budgets |
| Reject dual `Content-Length` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Parser tests and live smuggling probes. The two frontends differ on purpose, and track 110 pinned both. HTTP/1 refuses **any** second `Content-Length` line, even one that agrees (`a_repeated_content_length_is_refused_on_http_1_even_when_it_agrees` → `400` and the connection closes), which is stricter than the RFC 9110 §6.3.2.2 "same value is fine" rule and is what keeps a front-ending proxy from ever having a different reading than this server. HTTP/2 follows RFC 9113 §8.2.1: agreeing repeats are served with both values intact (`matching_repeated_content_lengths_are_served_over_h2` reads back `values=["5", "5"]` on a 5-byte body) and a disagreeing pair never reaches the handler — the `h2` crate resets it (`conflicting_repeated_content_lengths_are_reset_over_h2`). That reset is what makes the frontend's own ceiling check safe: it reads the *first* line only, and after the library is done with a request, the first line is the only line a multi-valued one can have |
| Reject `Content-Length` + `Transfer-Encoding` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Parser tests; unsupported codings rejected. Over HTTP/1 a `content-length` beside a `transfer-encoding` is a `400`; over HTTP/2 the same combination is a reset stream (`a_content_length_beside_transfer_encoding_is_reset_over_h2`), and a body that overgrows or underfills its declared length is reset whether the promise was digits (`a_non_numeric_content_length_is_reset_over_h2` for `abc`), padded (`a_padded_content_length_is_reset_over_h2` for `" 5 "` — HTTP/1 accepts ASCII padding because httparse trims it, `an_ows_padded_content_length_is_served_on_http_1`, and refuses a non-ASCII one, `a_non_ascii_padded_content_length_is_refused_on_http_1`), or unparseably large (`an_unparseably_large_content_length_is_reset_over_h2`) |
| `Expect: 100-continue` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `HTTP/1.1 100 Continue` written on a complete head with the body still pending; refused for HTTP/1.0; one interim response per request. Track 116 made the field's *list* semantics and the refusal the same on both frontends: commas and repeated field lines are one list (RFC 9110 §5.5), empty elements drop, OWS is trimmed, and the element must equal `100-continue` case-insensitively — `100-continue-x` or a quoted `"100-continue"` is a different expectation, and a list that mixes a supported and an unsupported element is one promise that cannot be met, so it is refused `417` (`E3003`) from the head, above the `max_body_bytes` check it would otherwise trip and before dispatch, so no body byte is ever asked for. `an_unsupported_expectation_is_refused_before_the_body_is_sent`, `a_mixed_expectation_list_is_refused_rather_than_half_answered`, `repeated_expectation_fields_are_one_list`, `an_unmet_expectation_outranks_the_body_budget_it_would_have_breached`, `an_empty_expectation_list_owes_neither_an_interim_nor_a_refusal`, `an_unsupported_expectation_is_refused_on_http_10_too`, `an_expectation_token_that_merely_contains_the_supported_one_is_refused`, `a_websocket_upgrade_cannot_be_built_on_an_unmet_expectation`, `expect_100_continue_is_not_sent_for_10_or_a_complete_request`, `expect_100_continue_is_answered_before_the_body_arrives`, `a_supported_expectation_is_answered_even_on_a_path_with_no_route` on HTTP/1, the same three shapes over HTTP/2 (`an_unsupported_expectation_is_refused_over_http2_from_the_head`, `a_mixed_expectation_list_is_refused_over_http2_too`, `repeated_expectation_fields_are_one_list_over_http2_too`, `a_supported_expectation_earns_an_informational_response_over_http2`, `a_stream_that_already_ended_its_body_earns_no_informational_over_http2`, `an_empty_expectation_list_owes_nothing_over_http2`) because RFC 9110 puts `Expect` in message semantics and RFC 9113 §8.2.3 allows a `1xx` frame, the streamed lane on each frontend (`a_streaming_route_refuses_an_unsupported_expectation_before_dispatch` on HTTP/1, `a_streamed_h2_route_answers_its_expectation_before_it_dispatches` over HTTP/2 — the latter also pinning that a streamed route earns exactly one interim before its first DATA frame) and the legacy runtime loop (`an_unsupported_expectation_is_refused_by_the_legacy_loop`), since a rule that answers `417` on one frontend and `400` on another is two rules. The interim is per request, not per partial read: `a_body_that_arrives_in_pieces_earns_one_interim_not_one_per_piece` sends a head, then three of its ten body bytes, then the rest, and requires one `100` across the three reads that see the same complete head. Capacity still outranks the promise — a head carrying an unsupported expectation whose field count passes `max_headers_count` answers `431`, not `417` (`test_header_count_refusal_outranks_the_expectation_refusal`, whose doc comment names which of the two refusals the shipped sizing actually reaches) |
| Keep-alive | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Connection loop, idle timeout; live multi-request connection |
| Pipelining (exact consumed-byte accounting) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Two-request pipeline test; live pipelined GETs |
| HTTP/1.0 request handling | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 1.0 requests parse; close-by-default, explicit keep-alive opt-in (advertised on the response), chunked-on-1.0 rejection and a status line that echoes `HTTP/1.0` are all implemented and wire-tested (including HEAD over 1.0: version echoed, `Content-Length` kept, no payload). The deviation this row used to carry — a WebSocket `101` claiming `HTTP/1.1` to a 1.0 client, "unreachable in practice" — is closed: `allows_upgrade` is a `Version::Http11` test, so a 1.0 upgrade request is refused with `400` framed as `HTTP/1.0` and no session starts (`an_http10_upgrade_request_is_refused_instead_of_switching_protocols`) |
| Response body streaming / chunked responses | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `Response::stream(source, content_type)` and `Response::stream_with_length(source, content_type, len)` take a `BodySource` — a pull-based `next_chunk()` returning `Option<Result<Bytes, io::Error>>` — and the bytes are written after the head, so a handler never holds the whole body. `BodyStream` is the boxed form, `BodyStream::from_chunks` adapts an iterator, and `BodyStream` is itself a `BodySource`. Framing is derived once, by `Response::body_framing(minor)` from the status and the declared length: HTTP/1.1 sends `Transfer-Encoding: chunked` for an undeclared stream and `Content-Length` for a declared one; HTTP/1.0 has no chunked coding, so an undeclared stream is close-delimited and forces `Connection: close` (RFC 9112 §6.3); a `HEAD` and every body-forbidding status (`1xx`/`204`/`304`) drop the source unpolled. The chunk framing is RFC 9112 §7.1 (`encode_chunk`/`encode_last_chunk` in `framework/http/src/response.rs`), and an empty chunk from a source is skipped rather than mistaken for a second terminator. A source that fails, ends early or overruns a declared length ends the **connection** without a terminator, because framing cannot be renegotiated after the head; a source that yields nothing for 1024 polls is treated as broken (`MAX_EMPTY_CHUNKS`), so a spinning source cannot pin a connection. Each chunk is priced against the memory budget while it is in hand and released once it is on the socket, on both emitters. Over HTTP/2 the same body becomes DATA frames — no `transfer-encoding`, and no `content-length` at all when the length is undeclared — with the write deadline renewed only on real progress. A streamed body is also what the compression middleware cannot touch: the coding would have to be declared on a head written before the bytes exist, so `apply` leaves a stream alone instead of advertising an encoding for a body it never saw. Pinned by 11 wire tests in `framework/tests/tests/streaming.rs`, five `response.rs` unit tests (`test_chunk_encoding_is_the_wire_format`, `test_a_stream_is_framed_by_its_declared_length`, `test_an_undeclared_stream_is_chunked_on_one_one_and_delimited_by_close_on_one_zero`, `test_a_body_forbidding_status_ignores_a_stream`, `test_a_head_writes_the_streams_framing_and_no_payload`) and three `http2_e2e` tests (`a_streamed_body_arrives_whole_with_no_framing_field`, `a_declared_stream_length_is_the_length_the_peer_is_told`, `a_head_over_h2_never_polls_the_stream`). The request side is the *Streaming request bodies* row, delivered for HTTP/1.1 in track 100 |
| Connection framing decided per connection, not per listener | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | `aether/src/transport.rs::classify` peeks the first bytes (`0x16 0x03` → TLS, 24-byte h2c preface → HTTP/2 when `enable_h2c` is set and closed otherwise, anything else → HTTP/1.1) under `read_timeout`, and `establish` returns the `Frontend` the serving loop dispatches on, so one listener answers `http://`, `https://`, ALPN-`h2` and opted-in cleartext h2. Nothing is decided by port number (`one_port_answers_plaintext_and_tls`, `an_h2c_preface_is_closed_not_answered_in_http1`, `h2c_multiplexes_without_stealing_the_port_from_http1`). Axum/Actix bind one protocol per listener |
| HTTP/2 over TLS | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether/src/http2.rs` bridges `h2` 0.4.20 onto the same `ServiceCore` HTTP/1.1 uses (`serve.rs`), reached only when ALPN negotiates `h2`. Ours: the head→`RequestParts` translation (`:authority` folded into `host`, and preferred over a `Host` the client sent), lowercase dedup, per-line budget, one task per stream because `h2::server::Connection` is neither `Clone` nor a `Future` and the accept loop must never await a handler, per-stream memory re-pricing through the scheduler, **releasing receive flow-control credit for every body byte it consumes** (see the defect above: `h2` does not credit a peer when a stream is dropped), response writing with connection-specific fields stripped, `keep_alive_timeout`/`read_timeout`/`receive_timeout`/`request_timeout` enforcement, a handshake bounded by `read_timeout`, and GOAWAY-drain shutdown. Left to `h2`: framing, HPACK, the stream state machine, flow-control *accounting*, PING/SETTINGS, and refusing connection-specific fields on the way in. Verified by 80 tests (`tests/http2_e2e.rs`, the target's `running N tests` and `test result:` lines on 2026-11-04 — the row said 16, then 27, then 29, then 36, then 37, then 40, then 49, then 55, then 56, then 67, then 78, then 79, then 80 as the file grew; the count is re-derived from the target's own `test result:` line, not carried forward). The forms RFC 9112 §3.2 allows a request-target are enforced here too, not only on the HTTP/1 request line: `:path` `*` is the server-wide `OPTIONS` and answers `204` + `Allow`, a `*` on any other method is answered `400` instead of routed, an empty or absolute or slash-less `:path` costs only its stream a `PROTOCOL_ERROR` reset, and a `CONNECT` — which carries no `:scheme`/`:path` at all — is answered `501` from the head, before any wait for a body a conformant client does not send (RFC 9113 §8.3.1, §8.1.1, §8.5). Track 110 made the §8.1.1 half of that sentence measured rather than assumed, and named the division: the request `content-length` promise is enforced by the `h2` receive path before the head reaches this framework — a body whose DATA frames do not sum to the declared length, a non-numeric value, a padded value, two lengths that disagree, a head that ends on `END_STREAM` while its length still promises bytes, an unparseably large length and a `content-length` beside `transfer-encoding` each cost the stream a reset with `PROTOCOL_ERROR`, the connection stays up, and the connection's next request is served — while what this frontend owns is the answer to a length over `max_stream_body_bytes`, which is a `413` before dispatch, and the body it hands the handler. Two served shapes are pinned beside the resets: agreeing repeats, which reach the handler as two values, and a streamed body with no declared length at all and by the independent clients above, which now live in the repository |
(`framework/tests/examples/node_h2_interop.cjs`, `h2_handframed_cap.py`) and were re-run against the
release binary on 2026-10-25: reproducible evidence, though still not part of `cargo test`. Not implemented: server push, which RFC 9113 §3.6 and §10.8 deprecates and no mainstream client uses |
| HTTP/2 over cleartext (h2c), prior knowledge | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED | `AetherApp::enable_h2c` opts the listener into RFC 9113 §3.3 prior-knowledge HTTP/2 over the same port as HTTP/1.1: `transport::establish` recognises the 24-byte preface and returns `Frontend::Http2`, and the identical `ServiceCore` serves it — identical limits, host policy, rate limiting, memory accounting and shutdown. Off by default, and a preface against a server that has not opted in is closed rather than answered in HTTP/1.1 framing, because an h2 client cannot parse a status line (`h2c_answers_a_post_on_a_plaintext_socket_when_enabled`, `h2c_multiplexes_without_stealing_the_port_from_http1`, `a_websocket_route_reached_over_h2c_answers_400`, `h2c_is_still_refused_when_tls_is_required`, `a_cleartext_connection_that_stops_at_the_preface_is_closed`, `streams_over_the_advertised_concurrency_cap_are_reset_not_silently_dropped`, `a_header_list_over_the_field_line_budget_is_431`, `a_peer_that_vanishes_mid_body_leaves_no_registration_behind`; then 20/20 live checks from python-h2, 8/8 from a hand-framed HEADERS probe, and the 2026-10-21 run against the final release binary — 3/3 cleartext connections refused on a `require_tls` port that had `enable_h2c` on, and 1 MiB / 3 MiB flow-controlled uploads answered 200 / 413 — the python-h2 and HEADERS probes there were scratch scripts, not committed and not re-run since 2026-10-21, while the committed `node_h2_interop.cjs` was re-run over cleartext on 2026-10-25). The name is prior-knowledge-only: turning this on cannot open a cleartext door on a port that requires TLS, and it never answers an `Upgrade: h2c` request. **Not implemented and deliberately not planned:** the `Upgrade: h2c` exchange itself — see the next row |
| HTTP/2 upgrade over cleartext (`Upgrade: h2c`) | NOT_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | A deliberate non-goal, not a gap. RFC 9113 §3.1 deprecates the `h2c` upgrade token and the `HTTP2-Settings` header outright, and RFC 7540 §3.2 requires the *request that asked for the upgrade* to be answered as an implicitly half-closed stream 1 — a state the `h2` 0.4 server does not synthesise, so implementing it means hand-writing the switch over from the HTTP/1 loop. Clients that still send it (old curl, old okhttp) are served cleartext HTTP/2 by prior knowledge instead, which is the path the spec kept |
| HTTP/3 | NOT_IMPLEMENTED | PARTIALLY_IMPLEMENTED | NOT_IMPLEMENTED | Axum only via ecosystem; no QUIC anywhere in Aether |

## Routing

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Static routes | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Radix trie tests |
| Path parameters (`{id}`) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Param nodes, percent-decoded, tested |
| Catch-all (`/*filepath`) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Full-path capture via pointer offset; tests. A catch-all declared before the last segment is refused **at registration** with a panic (`a_non_terminal_catchall_panics_at_registration`): it consumes the rest of the path, so anything declared after it can never match, and the handler would silently run for the whole prefix instead |
| Precedence: static > param > catch-all | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Backtracking tests |
| 404 handling | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Unit + live checks |
| 405 with complete `Allow` header | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Accumulated method bitmask across all path-matching branches; regression test + live check |
| HEAD via GET fallback | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Live HEAD checks; body suppressed while length preserved |
| Route groups (prefix) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `RouteGroup` in `aether/src/app.rs` |
| Duplicate-route detection | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Registration panics with a diagnostic instead of silently shadowing |
| Nested routers / independent sub-apps | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Prefix grouping (`RouteGroup`) plus group-scoped middleware, and — since 2026-10-30 (track 107) — the two halves this row said were missing: a scope of its own state and a mountable sub-app. `RouteGroup::manage` and `Router::manage` register values into the scope's own `AppStateContainer`, which `RequestContext::state::<T>` consults **before** the app container for exactly the types that scope registered and never for the rest (`a_scope_that_does_not_register_a_type_keeps_the_app_value`, `two_groups_can_register_the_same_type_and_answer_for_their_own_routes`), and `Router::new()` collects registrations that `app.mount("/api", router)` or `group.mount("/v1", router)` replays into a scope, so a sub-app carries its own guards, state, `route_stream`, `ws`, `static_dir` and nested `group`/`mount` without knowing where it will be mounted. The scope travels as one synthetic `MiddlewareEntry` at the head of the route's scoped chain, so `serve.rs` is unchanged and a route registered outside any scope pays nothing (`RequestContext::scoped_state` is `None`). Order is app middleware → scope state → scope middleware → handler (`the_chain_order_is_app_then_group_then_router_then_handler`), a nested child shadows its parent per type and keeps the parent's other types (`a_nested_group_shadows_its_parent_and_keeps_its_other_types`, copy-on-write container), `State<T>` resolves the scoped value (`the_state_extractor_resolves_the_scoped_value`), and the same overlay answers over cleartext HTTP/2 (`a_mounted_router_answers_with_its_own_state_over_h2c`). Evidence: 4 unit tests in `aether/src/app.rs`, 14 wire tests in `framework/tests/tests/nested_routers.rs`, 1 in `http2_e2e.rs`, plus a live `Router` doctest; ten injected mutations (overlay ignored, app container consulted first, overlay hiding unregistered types, overlay behind the scope's middleware, child starting a fresh container, scoped chain ahead of the app stack, router actions replayed reversed, `static_dir` dropping the chain, `ws` dropping the chain, a mounted route registered buffered instead of streamed) were each killed by a named test — `a_route_stream_in_a_mounted_router_streams_and_sees_its_state` only joined that list after it was rewritten to assert the streaming shape, because its first version passed with the mutation in place. This row said "no nested state or middleware" while the next row delivered the group middleware — corrected 2026-10-27 (track 88) |
| Per-route middleware/guards | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | App-wide (`AetherApp::use_middleware`), group-scoped (`RouteGroup::use_middleware`) and per-route (`AetherApp::route_sync_with`, `route_async_with`, `ws_with`), which binds the chain to one registration: another method on the same path and every other route keep the app-wide stack only. A guard is a middleware that answers without calling `next` |
| Automatic OPTIONS / CORS preflight | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | A routable path answers `OPTIONS` with `204`, an empty body and an `Allow` listing what the target really serves — `GET` and `HEAD` when a `GET` route exists (RFC 9110 §9.3.2), `OPTIONS` itself, and no `HEAD` on a WebSocket route; a registered `OPTIONS` route wins over the generic answer, and a CORS preflight is answered by the CORS layer first. A server-wide `OPTIONS *` (RFC 9112 §3.2.4) names no path, so it answers `204` with the union of every route's methods, computed once when the server is built; `HEAD` is listed only when a non-WebSocket `GET` route exists. Wire tests: `an_options_request_answers_the_methods_a_path_serves`, `a_405_allow_header_names_the_methods_the_server_answers`, `options_on_a_websocket_route_lists_get_without_head`, `a_registered_options_route_wins_over_the_generic_answer`, `options_asterisk_answers_for_the_whole_server` |

## Request extraction

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Path parameters | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `param`, `param_parse` |
| Query parameters (raw) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `query`, `queries` |
| Query parameters (percent/form-decoded) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `query_decoded`, Arabic `%D9%85` live check |
| Headers, case-insensitive | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `header()` |
| Duplicate / multi-value headers | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Repeated field lines are retained per name in wire order and exposed via `header_values()`; `header()` returns the first line and the public `headers` map resolves a duplicated field to that same first line, keyed by the casing the field first arrived with. E2E tests: duplicate cookie/`X-Dup` lines, mixed-case dedup against `header()`, repeated `Connection` lines, CORS preflight first-line rule |
| Cookies | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | `cookie()` parser for the raw value; `aether_security::CookieSigner` (2026-11-02, track 114) issues and verifies `base64url(payload).base64url(HMAC-SHA256)` with the cookie name bound into the tag, and `RequestContext::signed_cookie` separates `Present` from `Absent`, `Invalid` and `Ambiguous` (a name carried twice is refused, never resolved by taking one). Proven on both frontends (2026-11-03, track 115): eleven wire tests over HTTP/1.1 in `tests/signed_cookies.rs` and five over HTTP/2 in `tests/http2_e2e.rs` — four of those on cleartext prior-knowledge h2c, one on TLS with ALPN `h2`, and the duplicate-name shapes and the four-stream overlap are h2c-only, so nothing here claims them over TLS. What the HTTP/2 half adds is that frontend's own header store: the issued value must arrive byte for byte as the peer sent it (a separator written `%2E` is refused, not percent-decoded) and a name carried twice must stay refused whichever field shape it arrived in, which is what a store that coalesced repeated fields with a comma breaks — it answers 403 where the accessor must answer 409. **Not encrypted:** the payload is base64url and the client reads it, so this row's remaining half is confidentiality, which signed cookies deliberately do not provide |
| JSON body parsing | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `json()` via serde; size bounded by parser; no `Content-Type` enforcement |
| `application/x-www-form-urlencoded` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `form_data()` |
| Raw body bytes | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `RequestContext::body: Bytes` |
| Multipart / file uploads | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | RFC 7578 parser (`http/src/multipart.rs`): borrowed parts, strict boundary/header grammar, quoted parameters, part/header/size bounds; 11 unit tests + E2E |
| Typed extractors (`FromRequest`) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether/src/extract.rs` + `aether/src/handler.rs`: `Path<T>`, `Query<T>`, `Json<T>`, `Form<T>`, `State<T>`, `Extension<T>` on the `*_async` methods (0–4 args); 19 wire tests in `tests/tests/extractors.rs` + 2 in `tests/tests/extensions_e2e.rs` |
| Streaming request bodies | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | **Both frontends stream** since 2026-10-30 (track 100 for HTTP/1.1 on 2026-10-29, track 100b for HTTP/2 on 2026-10-30): `AetherApp::route_stream` marks a route's handler id in `ServiceCore.streaming_routes`, so each frontend dispatches from the head — before any body byte is needed — into a one-slot lane (`aether/src/request_body.rs`) that `RequestContext::body_stream()` pulls from, while `read_body(max)` reads the lane on a streamed route and the buffered copy everywhere else. HTTP/1.1 (`aether/src/body_pump.rs`) decodes both framings through the one `ChunkedBodyDecoder::stream_feed`, enforces `max_stream_body_bytes` on the whole body (a declared `Content-Length` over it is `413` **before** dispatch, while a declared length over the *buffered* ceiling is not refused on the way - the head applies no ceiling and leaves that decision to the route; a chunked body that crosses it mid-stream fails the read with `BodyError::TooLarge`), bounds the body by `receive_timeout` counted from the head (`BodyError::Timeout`), re-prices the connection's registration at every chunk so the lane's held bytes are charged as they arrive, and answers `Expect: 100-continue` before the body. After the answer `settle()` decides the socket: a body that finished its framing keeps the connection, a finished-with-error or unfinished one closes it (response → FIN → bounded discard) so its unread tail cannot be parsed as a next request. HTTP/2 (`serve_request` branches on the same route set into `serve_streamed` + `Http2Body`) pulls DATA through `RecvStream::poll_data` into the same lane, stops when the lane is full so a handler that stops reading stops crediting the peer, returns capacity with `flow_control().release_capacity` at every frame (so a 256 KiB body crosses the 65,535-byte default window whole), enforces `max_stream_body_bytes` on cumulative payload (a declared `content-length` over it is `413` before dispatch, handler never dispatched), bounds the body by `receive_timeout` (the deadline fails the lane: `BodyError::Timeout` reaches the handler's read, which answers), and re-prices the stream's registration at each chunk. An answered request whose body was not finished is abandoned by dropping the `RecvStream`: `h2` ignores further DATA and returns the capacity to the connection, so the connection serves the next stream — `RST_STREAM(NO_ERROR)` is deliberately **not** sent, because `h2` implements a reset by dropping the stream's queued frames and so destroys the response it is meant to follow. Thirteen socket tests in `tests/tests/streaming_request.rs` (eight when this row was written; track 110 added the pair that stops the lane at its declared length and frames the surplus as the next request, tracks 116-117 added the `100-continue` and receive-timeout lanes, and track 142 added the case where a declared `Content-Length` over the buffered ceiling streams to a route whose streaming ceiling allows it) and six in `tests/tests/http2_e2e.rs` (dispatch before the body, chunked decoded, `413` pre-dispatch, mid-stream `TooLarge`, unread body closes/keeps its stream, small early-answered body keeps the connection, receive timeout `408`, `100`-continue; over h2: dispatch before DATA, whole body across receive windows, pre-dispatch `413`, mid-stream `TooLarge`, unread body keeps the connection, timeout reaching the handler). **Remaining:** `Json<T>`/`Form<T>` read the buffered copy and therefore do not work on a streamed route |
| Request-scoped extensions/state | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether/src/extensions.rs`: a `TypeId`-keyed map boxed on first insert (an unused request allocates nothing — `a_fresh_map_holds_nothing_and_has_not_allocated`), `insert`/`get`/`get_mut`/`remove`, values need not be `Clone` and are not shared (one request has one owner, so nothing is locked). Reached as `RequestContext::{extension, insert_extension, extensions, extensions_mut}` or as the `Extension<T>` argument (which clones, the same constraint Axum's `Extension<T>` carries; a missing value is a 500 rejection, not a handler that runs anyway). 17 unit tests + 8 HTTP/1.1 wire (`tests/extensions_e2e.rs`: middleware→handler, two middlewares on one map, group scoping, no leak between keep-alive requests, and since the pricing track one that registers a declared payload against a live request and releases it, plus one that charges an undeclared value its container while the answer still serves) + 4 HTTP/2 wire (`http2_e2e.rs`: middleware→handler over TLS `h2`, **per-stream** isolation proved with 4 concurrent streams that each hold the chain for 100 ms and must not see each other's value, one-port HTTP/1.1-vs-h2c identical answers, and the same declared megabyte priced against the stream's registration). **Since 2026-10-26 the map is priced** (PROB-0016, item 17): `EXTENSION_ENTRY_MEMORY` + `size_of::<T>()` for every entry, plus the declared heap for those inserted through `insert_extension_sized`, charged against the request's own handler registration. **Remaining limit:** a heap payload nobody declared, one grown in place through `get_mut`, and a `Vec`'s reserved tail stay invisible — the budget cannot see inside an arbitrary `Send + Sync` value unless the inserter says how much it owns. A `WebSocket` handler reads the same map: since PROB-0017 the upgrade request runs its middleware chain before the handshake is validated, so a middleware-stamped value is present in the context handed to the session (`a_websocket_upgrade_runs_the_app_middleware_chain`) |

## State and dependency injection

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| App-wide typed state | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `manage`/`state`; TypeId container test |
| Request-scoped state | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Same feature as the "Request-scoped extensions/state" row above (`Extensions` + `RequestContext` accessors + `Extension<T>`). This row was still `NOT_IMPLEMENTED` while that feature was being delivered, and the "No extensions" evidence beside it had been wrong for the same reason the `Host` row below was: the status column is not re-derived from the code when a feature lands |
| Database connection pool | NOT_APPLICABLE | NOT_APPLICABLE | NOT_APPLICABLE | External crates for all three |
| Redis / cache client | NOT_APPLICABLE | NOT_APPLICABLE | NOT_APPLICABLE | External crates for all three |

## Error handling

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Structured error type | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `AetherError` with diagnostics |
| JSON error responses, escaped | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `serde_json` escaping; live quote/newline check |
| Canonical reason phrases | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 418 "I'm a Teapot" live check |
| Panic isolation → 500 | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Spawned handler + `JoinError`; `panic = "unwind"`; live panic check, server survives |
| Typed error → status mapping | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | The blanket bound is `E: IntoResponseError` — a local trait at `aether/src/app.rs:123-166`, answering the handler's `Result` from the impl at `:224-265` — so the error type declares its own status instead of the framework guessing one: `&'static str`/`String` answer 400 with their message (`:168-186`) and `AetherError` answers 400/404/405/413/417/431/500/504 per variant (`:188-222`). The body's `code` is derived from the same class, so a 4xx can no longer carry `E5002` and a 5xx not `E4003`; 5xx bodies redact the internal message. A declared status outside `400..=599` is replaced by `500`/`E5002` with the detail dropped (track 89, `:237-248`), so no error path publishes a success or an unplaceable status. Before 2026-10-24 `E: Display` forced every `Err` to 400. **Breaking:** an unwrapped foreign error type (and `Box<dyn Error + Send + Sync>`) no longer compiles — measured 2026-10-24 on one `Display`-only type in both handler forms: `app.get` with a declared `-> Result<&'static str, E>` yields `E0277` naming `IntoResponseError` and listing its three impls, while `app.get_async` with an inferred `async move` output yields `E0277 … Handler<_> is not satisfied` and does not name the trait. Pinned by `a_handler_error_answers_400_and_says_so_in_the_body` + `typed_handler_errors_answer_their_own_class` + `an_error_status_outside_the_error_range_answers_500` |
| Request diagnostics | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | `Diagnostic` with hint/suggestions; startup banner. Track 118 re-derived the developer-facing half against the C′ error contract: `AetherError::to_diagnostic`'s `Io` arm used to send a runtime 500/E5002 to the port-occupancy advice written for a startup bind failure (the bind still builds its own E1001 report before returning `Io`), and now names the runtime failure, the 5xx redaction and `set_error_reporter`; `HeaderTooLarge`, `BodyLimitExceeded` and `Timeout` name the knobs an app can actually set (`AETHER_MAX_HEADER_BYTES` and `AETHER_MAX_HEADERS_COUNT` for the two head budgets, `AETHER_MAX_BODY_BYTES`, `AETHER_MAX_REQUEST_TIME_SECS`), and `Timeout`'s `Display` no longer calls a 504 a "Connection timeout". The code/status agreement is now a twelve-row cross-product (`every_aether_error_answers_the_status_class_its_code_names`) instead of a claim in a comment, pinned with eight `error.rs` and four `diagnostic.rs` unit tests; the HTTP/1 `417` wire test also now reads the body it asserts empty |
| Access logging | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in `AetherApp::enable_request_logging(show_timing)` installs the wired `LoggerMiddleware`; it is disabled by default so benchmarks and hot paths stay silent, and `show_timing` appends the handler latency. This row said "`AetherApp` exposes no enable method" while the method and its read site (the `logger.enabled` gate in the response post-process) both existed — corrected 2026-10-27 (track 88). Stated limit: no test captures the logger's stdout; the gate and its formatting are read from code, not asserted on the wire. Track 89 wired the previously dead `AetherConfig::x_request_id` into the response post-process, where the framework's id now displaces any handler-written `x-request-id` instead of appending a second one, and `false` drops the header (`the_request_id_header_follows_the_config_flag`; pinned also by `a_handler_cannot_publish_a_second_x_request_id`) |

## Middleware

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| CORS allow-list + preflight | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Origin allow-list, credentials, `Vary: Origin`, preflight validation; live checks |
| Security headers | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in; HSTS, `X-Frame-Options`, `nosniff`, Referrer-Policy, `X-XSS-Protection: 0` |
| Compression (gzip/brotli) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in; q-value negotiation, MIME skip, `Vary: Accept-Encoding`; live checks |
| Request logging | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Same feature as the "Access logging" row: opt-in `AetherApp::enable_request_logging(show_timing)`, off by default. This row's "see above" pointed at a finding that was itself stale — corrected 2026-10-27 (track 88) |
| Deterministic pipeline order | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Fixed order: security → CORS → compression → log |
| User-defined middleware | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `AetherApp::use_middleware` / `RouteGroup::use_middleware`: `Fn(RequestContext, Next) -> Future<Response>`; registration-order execution, short-circuit support; 13 wire tests in `tests/tests/user_middleware.rs` (eight app-wide/group, five per-route since track 94). Chain composes at registration time, so it wraps routes added afterwards only if they are added afterwards. Since PROB-0017 it also wraps a WebSocket route's **upgrade request** (see the WebSocket section) |
| Per-route middleware | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `route_sync_with`, `route_async_with` and `ws_with` (delivered 2026-10-29, track 94); the empty-prefix group stays as the prefix-scoped form and produces the same wire result |
| Rate limiting | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Opt-in `AetherApp::enable_rate_limit(RateLimitConfig)`: per-client token bucket applied before routing, keyed on the caller `RequestContext::client_addr()` resolves — the peer when no proxy list is configured, the forwarded identity otherwise (see *Forwarded client identity*) — with 429, a non-zero `Retry-After` and the connection kept; dead-client sweep, fail-open (counted on `AetherApp::rate_limiter()`) at the tracked-client ceiling. 10 unit + 4 wire tests. Two of the wire tests are the proxy split: one forwarded client cannot spend another's budget, and a claim from an unlisted peer buys no second bucket |
| Forwarded client identity | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Opt-in `AetherApp::enable_trusted_proxies`: the list is a CIDR set (`TrustedProxies`), a hop is believed only when the address that delivered it is on the list, and the chain is walked right to left, so the resolved caller is the nearest untrusted hop. `Forwarded` (RFC 7239) wins over `X-Forwarded-For` when both are present rather than the two being mixed; repeated field lines are joined into one value in wire order before the walk (RFC 9110 §5.2), so a line the client planted cannot outrank the line a proxy appended; a malformed element stops the walk and nothing to its left is believed; 32-hop cap; ports, brackets and IPv4-mapped IPv6 canonicalised. The value reaches handlers as `RequestContext::client_addr()` while `peer_addr()` keeps naming the socket, and the limiter keys on it. Registration refuses an empty list and `0.0.0.0/0`/`::/0`. 10 unit tests in `framework/middleware/src/forwarded.rs` + 3 wire tests |
| Host validation | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Opt-in `AetherApp::enable_host_validation(&[…])`: allow-list where `*.example.com` is sub-at-any-depth and never the apex, ports/trailing root dot normalised, scheme/path/userinfo refused, 400 with a body that never echoes the host, applied before routing. Panics on an unparsable entry so a typo cannot silently widen the list. 7 unit + 1 wire test |
| Authentication middleware | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Opt-in layer `require_bearer(&BearerAuth)`, registered per route, per group or app-wide (delivered 2026-10-29, track 98b): reads every `Authorization` field line and requires exactly one, the `Bearer` scheme case-insensitively (RFC 9110 §11.1) and a `token68` credential (RFC 7235 §2.1); verifies signature and claims with `JwtValidator` and attaches the verified `Principal` (subject plus claims, the claims' heap priced into the request's memory registration) before the handler runs. Zero lines, two lines, another scheme, a malformed credential, a bad signature, an expired token and an `alg` mismatch are one refusal: `401` with `WWW-Authenticate: Bearer error="invalid_token"` and an empty body. 8 unit tests in `aether-security/src/auth.rs`, 1 facade unit test, and 3 wire tests in `tests/tests/bearer_auth.rs` |

## Security

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Request-smuggling defenses | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Dual CL, CL+TE, unsupported TE codings, HTTP/1.0 chunked rejected; tests + live probes. Track 110 closed the last smuggling-shaped assumption on this row by measuring it rather than reasoning from the spec: there is **no** content-length defect on either frontend. HTTP/1 frames a body to its declared length and hands the surplus to the next message on the connection (five raw-socket tests, including the `400`-and-close when that surplus is not a request and the non-ASCII-padded length that ASCII OWS would smuggle past a Unicode-trimming parser); HTTP/2 cannot be desynced at the framing layer because the `h2` receive path enforces the promise before the head is ever handed to this framework, which is what the eight `RST_STREAM(PROTOCOL_ERROR)` tests observe — each of them also checks the connection is not killed (no GOAWAY) and that its next request still gets served |
| Header count/byte budgets | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 431 before parse; stack/heap header slots; tests. Default count budget raised 64 → 100 field lines, hyper's ceiling; `max_header_bytes` (8 KiB) is what actually bounds per-line work |
| Body budget | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Two quantities, both enforced and both now named on the field (PROB-0028, closed 2026-10-26): the parser bounds the **decoded** body by `max_body_bytes` on both frontends, and HTTP/1.1 refuses earlier against a **wire** budget of `max_header_bytes + max_body_bytes`, because a request's framing and the decoder's copy are both resident until it completes. Declared, chunked cumulative, and exact-limit tests; `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes` pins the wire boundary with one payload byte of slack (on the defaults, 350 879 served and 350 880 refused), and moves when the guard moves — ±6 bytes on it flips each half of the pair |
| Slow-client protection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 5 s per-read idle timeout **and** a total receive deadline set by `max_receive_time`, a setting separate from the handler timeout (`max_request_time`); a trickle client receives 408 and is closed (verified live at an 8 s configured deadline, when the receive budget still reused the handler knob). The same deadline covers body upload, so a very slow large upload is refused only when it exceeds the receive budget. Three shapes, measured on this tree on 2026-11-04: a head or body that keeps arriving is answered 408 inside the budget; a streamed body that stops gets its lane deadline and 408; and a buffered request that stops — a head never finished, a body never started, a body stopped part way, a chunk never closed — is answered `408` with `Connection: close` by the shorter of the 5 s per-read idle timeout and what is left of the receive budget. PROB-0034 recorded the same shapes before the fix as EOFs at 5.005 s with no body byte sent, 5.003 s with three of five, and 5.002 s even at the default 30 s budget; track 117 closed it, and the late-byte shape pins the arithmetic (one byte at 1.1 s of a 1.5 s budget answers at the budget, not after a fresh 1.5 s). An idle keep-alive socket still closes silently because it has no request to answer. Pinned by `a_request_that_stops_arriving_is_answered_408_by_the_receive_budget`, `a_stopped_body_waits_no_longer_than_the_budget_it_started_with` and `an_idle_keep_alive_connection_is_closed_without_an_answer` |
| Concurrent-connection cap | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 10 000 default via semaphore |
| Path-traversal protection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `safe_resolve_path`; Windows ADS/device-name/trailing-dot rejection; live traversal checks |
| CSRF protection | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Opt-in app-wide `AetherApp::enable_csrf(CsrfTokens::new(secret, CsrfConfig))`: signed double-submit. A safe response (GET/HEAD/OPTIONS/TRACE, RFC 9110 §9.2.1) that does not already carry a cookie this secret signed issues `aether_csrf=nonce.mac` — 16 random bytes, HMAC-SHA256 over their base64url text, `SameSite=Lax`, `Secure` by default, `Path=/`, no `Domain`, not `HttpOnly` — and puts the same string in the `CsrfToken` extension the handler renders. An unsafe method must echo it in exactly one `x-csrf-token` field; every missing, mismatched, unsigned, duplicated or doubled credential is one `403` with an empty body, refused before the handler runs and without issuing a token. A valid cookie is reused rather than rotated so an already-rendered form keeps working, a planted one is replaced by a freshly signed one, `exempt_prefixes` skips machine endpoints, and no token is issued on a preflight or on a response ≥ 400. 20 unit tests in `aether-security/src/csrf.rs` and 9 wire tests in `tests/tests/csrf.rs` |
| CSP | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Opt-in `Content-Security-Policy` header applied by `SecurityHeadersMiddleware`, emitted byte for byte as configured and never duplicated over a handler's own header. A policy may name `{nonce}`: the serving core draws 16 random bytes for a request that will reach a handler (`aether_security::CspNonce`, base64url), substitutes the same token into the header and into the request extensions the handler reads, and refuses the request with `500` rather than serve a guessable nonce when the OS RNG fails; a response the framework writes without dispatching — a security refusal, a routing `404`/`405`, a preflight, the generic `OPTIONS`, the h2 upgrade refusal — draws nothing and goes out with the source expression removed rather than emitted empty. `csp_hash_sha256` renders a `'sha256-…'` source expression for markup that cannot carry an attribute. Verified by `a_csp_nonce_reaches_the_handler_and_the_header_that_authorises_it` (a dispatched page, then a `404` and a `429` that must name no nonce), `a_policy_without_the_nonce_placeholder_draws_nothing`, the nonce half of `a_websocket_route_reached_over_h2_answers_400`, and the `csp.rs`/`security.rs` unit tests. Not implemented: `Content-Security-Policy-Report-Only` and `report-to`/`report-uri`, and there is no template engine to inject the nonce into markup automatically — the handler renders it |
| HSTS | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Opt-in; default `max-age=31536000` |
| TLS termination | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether/src/tls.rs` on `rustls` + `tokio-rustls`, pinned to the **ring** provider (aws-lc-rs compiles C through cmake and would make Windows support depend on an external toolchain); TLS 1.3 + TLS 1.2, no client auth. `AetherApp::tls` / `tls_from_files` / `tls_from_pem`, optional `require_tls(true)` which closes a plaintext connection instead of answering it. PEM loading rejects an absent key, an encrypted key, and a key that does not match the chain (`CertifiedKey::from_der` SPKI comparison, so `KeyMismatch` is its own error, not a string match). 25 unit tests in `aether/src/tls.rs` (15 of them revocation: six CRL, nine OCSP) + the 12 wire tests in `tests/tls_e2e.rs`, plus live runs against clients that are not rustls: Windows curl/Schannel (built **without** HTTP/2 — `--http2` is refused by that binary) negotiated ALPN `http/1.1` and received `HTTP/1.1 200 OK` through the TLS record on the same port an `h2` client was using, and Python/OpenSSL did the same. Node.js `http2` negotiated `h2` and completed the HTTP/2 exchange over **both TLS 1.3 (`TLS_AES_128_GCM_SHA256`) and TLS 1.2 (`ECDHE-ECDSA-AES128-GCM-SHA256`)** (`examples/h2_interop_server`). Untrusted CA ⇒ curl exit 60; a h2c preface ⇒ immediate EOF until `enable_h2c` opts that listener in |
| SNI / multiple certificates on one port | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | A `ResolvesServerCert` picks the identity per handshake from a name table; names are normalised (lowercase, trailing root dot, length, IP literals refused) and a hello with an unknown or absent name gets the catch-all rather than a failed handshake. Exactly one catch-all is required at build time, so a table that cannot answer every client hello is refused with an error instead of silently aborting. webpki exposes no public SAN accessor, so names are operator-supplied, never parsed out of the DER (`sni_chooses_the_named_identity_and_unknown_names_use_the_default`) |
| Live certificate reload (no restart) | VERIFIED_IMPLEMENTED | NOT_APPLICABLE (ecosystem) | NOT_APPLICABLE (ecosystem) | `TlsAcceptor::replace_certificates` validates a complete new table first and then swaps one `Arc` under a lock: success is visible to the next handshake, failure leaves the currently-served chain byte-identical (asserted by `Arc::ptr_eq` before/after a rejected reload). A cert renewed by a cron job therefore needs no restart, and a bad reload cannot take TLS down. For Axum/Actix this is the TLS library's job, not a framework feature |
| OCSP stapling / revocation | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED (ecosystem) | Offline by design and enforced at load time. `TlsCertSet::with_crl` accepts PEM or DER, verifies the signature with the chain's issuer (the second certificate, or the leaf when self-signed), refuses a stale or not-yet-valid list, and refuses the set when the leaf's serial is listed. `TlsCertSet::with_ocsp` staples a *basic* OCSP response only after proving: the envelope answered `successful`; one `SingleResponse` carries this leaf's serial and this issuer's name and key hashes (SHA-1 or SHA-256, recomputed with ring); that status is `good`; `thisUpdate` is not in the future and a present `nextUpdate` is still ahead; every time field is a GeneralizedTime, never read through asn1-rs' tag-blind accessors; and the signature verifies under the issuer's key or under an issuer-signed responder certificate carrying `id-kp-OCSPSigning`, named by `ResponderID` byKey or byName. Refused material is never stapled. 15 unit tests across both halves (`tls::`: six CRL, nine OCSP); 13 OCSP mutants and five CRL mutants, each rejected by a named test. Not implemented: fetching the response (the operator supplies it) and any refresh task beyond reloading the set |
| `Host` header validation | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | Same feature as the middleware table row: opt-in `AetherApp::enable_host_validation`, applied after the head parses and before routing, static-file work or upgrade; 400 whose body never echoes the refused host. This row was stale (it still said "no host allow-list") while the feature and its wire test existed |
| JWT validation (HS256) | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | Reachable from the facade as `aether::JwtValidator` and `aether::JwtValidationOptions` (the crate is also nameable as `aether::security` since track 91): alg pinning, exp/nbf/iat, iss/aud, canonical base64url, constant-time tag check; a padded tag segment is refused rather than trimmed. 9 unit tests in `aether-security` plus `a_jwt_verifies_through_the_facade`, which imports only `aether::`, verifies a precomputed HS256 token, and rejects the same claims under another key. The middleware that consumes it is the *Authentication middleware* row (track 98b) |
| Constant-time comparison | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `aether::constant_time_eq` (and the module path `aether::security::constant_time_eq`) compares equal-length byte strings without early exit; the JWT path's tag check is `ring::hmac::verify`, also constant-time. Pinned by `constant_time_comparison_is_reachable_through_the_facade` |
| Request desync audit | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `aether::audit_request_desync` takes an `aether::header::HeaderMap` (the header vocabulary is re-exported with it, so a caller needs no second dependency) and refuses a head two HTTP/1 parsers could frame differently: `Content-Length` alongside `Transfer-Encoding`, a repeated `Content-Length` or `Transfer-Encoding` field, a `Content-Length` that is not a decimal number, or a `Transfer-Encoding` that is not exactly one `chunked` coding. These are the parser's own framing rules, so `Ok(())` names a head the server accepts on framing grounds. It remains an audit, not the enforcement point: the parser's rules (the request-smuggling rows above) are what run on every request, and nothing in `aether` calls this one. It sees headers only, so it cannot refuse `Transfer-Encoding` on an HTTP/1.0 request, and the chunked body itself is decoded while parsing. Added to the matrix 2026-10-27 (track 88); reachable and facade-tested 2026-10-28 (track 91, `the_desync_audit_rejects_conflicting_framing_through_the_facade`); its rule set aligned with the parser and the repeated-`Transfer-Encoding` and non-`chunked` gaps closed 2026-11-06 (track 136, `smuggling::tests`) |
| Secret management | NOT_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | **Deliberate non-goal (track 103, 2026-10-30): no wrapper type.** Secrets are constructor arguments — the deployment docs' environment variables — and never fields of `AetherConfig`. What is enforced instead is that the framework never renders one: `JwtValidator`, `BearerAuth` and `CsrfTokens` implement no `Debug` (a `compile_fail,E0277` doc test on each fails if one is added), `CsrfToken`'s `Debug` prints `CsrfToken(<redacted>)`, the request-extension map prints its count and charged bytes but never a value, `BearerError` carries neither a reason nor the credential, and the TLS types' derived `Debug` renders the public chain plus rustls' signing key — whose own `Debug` prints the algorithm only, re-checked from `rustls` 0.23.46's source — pinned by a test that searches the rendering for key-unique windows of the private key in PEM text, DER hex and decimal byte form. Redacting an application's own secret stays the application's job: a wrapper nobody is required to use would be API surface with no guarantee behind it |

## Timeouts, resources, lifecycle

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Read timeout | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 5 s default; live slowloris check |
| Write timeout | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 10 s default around response write |
| Keep-alive timeout | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 60 s default |
| Per-handler timeout | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED (ecosystem) | `tokio::time::timeout` around handler task; 504 on expiry |
| Concurrent-connection limit | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | VERIFIED_IMPLEMENTED | Semaphore, RAII permit |
| Graceful shutdown | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `AetherServer::{bind, local_addr, serve_with_shutdown}`; Ctrl+C plus SIGTERM on Unix; stops accepting immediately, drains HTTP connections with a bounded deadline, then aborts. E2E test proves idle keep-alive sockets close and the listener is released. Since 2026-10-27 an upgraded WebSocket session is given the same shape: the socket holds a clone of the shutdown watch, so a handler in `recv` sends one `1001` close frame and returns on its own terms, and only a handler that never returns to `recv` is aborted — after a 1 s grace, which is what keeps shutdown bounded |
| Client-disconnect cancellation | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | A handler is aborted when its peer is observably gone: `serve.rs::dispatch` races the handler task against a `PeerWatch` in a `biased` select and calls `abort_handle.abort()` when the watch resolves, returning `ServeOutcome::ClientGone` so no bytes are written. HTTP/1 watches the transport for a read **error** — an RST on plaintext, or rustls reporting the stream ended without `close_notify` — and keeps a pipelined request's bytes instead of mistaking them for departure. A clean EOF is deliberately not a departure, because a client may shut its write half and still wait for the answer (RFC 8446's relaxed `close_notify`; the repo's own `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes` is that contract), so a clean close is still detected at write time. HTTP/2 watches `SendResponse::poll_reset`, so a per-stream `RST_STREAM` — or the failure h2 reports for every stream when the connection ends — cancels the handler while other streams keep being served. Pinned by `client_disconnect_cancels_the_running_handler`, `a_pipelined_request_does_not_look_like_a_disconnect`, `a_client_that_stays_connected_is_not_cancelled`, `a_stream_reset_cancels_the_running_handler_and_keeps_the_session`, `a_dropped_h2_connection_cancels_its_running_handlers`, `a_tls_client_that_leaves_cancels_the_running_handler` and `a_tls_client_that_half_closes_still_gets_its_response` |
| Bounded response memory | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | A buffered answer is priced by the block it reserves and released when the write is done; a streamed answer is priced one chunk at a time, so the budget covers what a response actually holds and never its whole length. Since 2026-10-23 the block a serialized answer reserves is charged to the scheduler memory budget before the bytes go to the socket and handed back after the write, so an idle keep-alive socket that once served a 5 MiB answer no longer carries it (three `app.rs` unit tests plus `a_large_answer_is_priced_against_the_memory_budget_without_being_withheld`; the charge is best-effort — a refused budget never withholds an answer the handler already produced). Scope: since 2026-10-24 both emitters price the block behind the answer they write. HTTP/1 charges the buffer the framework owns; HTTP/2 charges the allocation behind the body it hands to the `h2` crate (`price_response_body`, `framework/aether/src/http2.rs:1099`, registered before the response head is queued) and releases it with the stream — pinned by `a_response_body_is_registered_by_its_block_while_the_stream_holds_it`, `a_response_body_over_the_budget_is_sent_anyway` and the wire test `a_reader_that_stops_gets_a_stream_reset_without_killing_the_connection`. The charge is one block per stream and covers only what the stream can be handed back without a copy — measured 2026-10-26 against `price_response_body` on each body shape: a body this stream owns alone is priced by its allocation's `capacity()` and is never copied, a `&'static` body and a block another handle still retains stay at the stream's base (correctly: static bytes are not per-stream heap, and a retained block belongs to its holder's accounting, so pricing it per stream would price one cached allocation once per reader), and a body sliced out of a larger buffer is priced from its first byte to the end of that buffer, so a dropped prefix is the bound that remains. `Bytes::from_owner` is the one real gap in that list: `bytes` reports an owner-backed block as never recoverable, even for its sole handle, so a handler that wraps its own buffer that way keeps heap this budget cannot see and no zero-copy change here can price it (PROB-0027, closed 2026-10-26 on the rule that replaces it — **an HTTP/2 response body is built with `Bytes::from`, never `Bytes::from_owner`**, stated on `IntoResponse` and on `price_response_body`; the framework's own producers — `IntoResponse for String`, `IntoResponse for (u16, String)`, `Response::text_dynamic`, `Response::html_dynamic`, `Response::file`, and the same shape `static_dir`, the embedded-asset route and the compression middleware emit — are pinned priceable by `every_body_this_framework_produces_is_a_block_the_budget_can_name`). `StreamCharge::buffer_set` is set-semantics, so a stream that holds both a request and a response block is priced by the response alone — a deliberate approximation, not a leak. The framework's own error answers no longer keep a dropped request buffer registered: `read_body` re-prices the stream down to its base on every refusal path, where the refused buffer dies. Pinned by `a_body_the_stream_solely_owns_is_priced_by_its_allocation_not_by_a_copy`, `a_body_sliced_from_a_larger_block_is_priced_only_up_to_its_end`, `the_bodies_left_unpriced_are_the_static_shared_and_owner_backed_ones` and `a_refused_upload_stops_registering_the_buffer_it_dropped` (PROB-0023, closed 2026-10-26). A peer that trickles flow-control credit no longer keeps its stream alive for free: a write interval renews the deadline only once it has moved [`MIN_PROGRESS_FOR_RENEWAL`] = 1024 B (`framework/aether/src/http2.rs:981`), so `max_response_time` is also a floor on how slowly a client may consume — ≈102 B/s at the 10 s default — and a trickler is cut with `RST_STREAM(CANCEL)` while the connection survives. Pinned by `a_trickling_reader_is_cut_at_the_deadline_it_cannot_earn`, and by its pair `a_slow_reader_that_earns_every_deadline_finishes_the_whole_body`, which is the half that keeps a legitimately slow long download uncut (PROB-0022, closed 2026-10-25). A stream is priced chunk by chunk and never by its length: HTTP/1 re-prices the connection after every chunk with `reprice_conn_memory` and hands the oversized write buffer back once it is drained (`release_stream_write_buf`), and HTTP/2 sets `StreamCharge` to the block of the chunk in hand and clears it as soon as that chunk is queued (`price_stream_chunk`), so a pause between chunks holds no budget. A static file therefore costs one 64 KiB read, not its size, and the `HEAD`/`304`/`416` answers above cost none. |
| Backpressure / load shedding | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Connection semaphore on the default path; opt-in adaptive scheduling (`AetherApp::with_scheduling`) adds bounded admission queues, explicit 503 + `Retry-After` shedding, per-kind capacity, and queue/rejection metrics. 18 wire tests in `framework/tests/tests/scheduling.rs` (recounted 2026-10-27 from the `test result:` line of `cargo test -p aether --test scheduling`; this row said 9, then 14, then 15, then 16, then 17) |
| Adaptive work scheduling (opt-in) | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | `aether-scheduler` crate (protocol-independent engine; FIFO/RoundRobin/Deficit/AdaptiveTree policies, baseline learned from measurements, quota slicing of divisible work, starvation guard, adaptive resource budgets). 57 tests in the crate — 11 unit (`baseline.rs` 3, `engine.rs` 6, `policy.rs` 2) + 46 in `scheduler/tests/` (`engine.rs` 21, `sim.rs` 9, `pump_head_of_line.rs` 8, `memory_reprice.rs` 4, `policy_extension.rs` 2, `stress.rs` 2), recounted 2026-10-22; this row said "27 crate tests (17 engine, 8 simulator, 2 stress)" — plus the 18 wire tests in `framework/tests/tests/scheduling.rs`; design in `docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md`, measured policy comparison in `docs/SCHEDULING_COMPARISON.md` |

## Static files

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Serve a byte buffer with MIME type | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `static_file`/`static_asset` |
| Serve from a directory at runtime | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `static_dir`/`static_dir_with_cache`, plus `static_dir_with_options` for the opt-in listing and pre-compressed selection; live checks |
| MIME detection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Extension table; tests |
| Strong ETag | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | An in-memory answer hashes its bytes (`"len-hash"`, syntactically strong, no embedded `W/`). A streamed file answer publishes a **weak** metadata validator instead — `W/"<len hex>-<mtime nanos hex>"` — because its bytes are never read to hash them; the weak/strong distinction is what keeps `If-Range` from slicing a range under a tag that cannot confirm the content (see the *Streaming / `sendfile`* row) |
| Conditional GET (304) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Weak, list, and `*` matching per RFC 9110; live checks |
| Cache-Control | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Configurable max-age |
| Path-traversal protection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Tests + live checks |
| Directory listing | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED (ecosystem) | Opt-in through `StaticDirOptions::directory_listing` on `static_dir_with_options`; a bare `static_dir` keeps answering `404` for a directory that holds no page, which is what an application that never asked for a listing must keep doing. A directory that holds an `index.html` is that page in either mode — since track 141 from the moment the path resolves to a directory rather than from inside the listing branch, which is where the lookup sat until then and why a default `static_dir` answered `404` for a site's home page; both the slash-terminated and the slashless form answer it, and the tree's own root is reachable through a prefix route registered when listing is on or the root holds `index.html` at registration (the catch-all needs one character after the prefix). A directory that is about to be listed and was reached without its trailing slash is `301`-ed with a `Location` rebuilt from the prefix and percent-encoded segments, and a listing is `Cache-Control: no-cache`. Entries are filtered to names the resolver will actually serve (no leading dot, no `\`, no `:`, no trailing dot or space, UTF-8) with symlinks skipped and the `.br`/`.gz` siblings folded into their base file; names are HTML-escaped as text and href-escaped so `rock&roll.txt` is both readable and requestable; a listing stops at 10 000 entries and says so. Traversal is unchanged — the listing renders only what `safe_resolve_path` would serve — and the wire test proves `../`, `%2e%2e` and `..%2f` still answer `404`. Pinned by `directory_listing_is_opt_in_and_never_widens_the_resolver`, `a_directory_with_an_index_page_is_still_that_page`, `an_index_page_is_served_when_listings_are_off`, and six `static_dir` unit tests (`a_name_that_cannot_be_served_is_never_listed`, `a_hostile_name_is_rendered_as_text`, `a_segment_escapes_every_byte_that_is_not_unreserved`, `directories_are_listed_with_a_trailing_slash_and_a_parent_link`, `the_root_listing_has_no_parent_link`, `a_redirect_names_the_directory_once`) |
| Range requests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Single and suffix ranges, `206`/`416`, `Content-Range`, `Accept-Ranges`; malformed/multi-range ignored; 6 `static_files.rs` tests + E2E. `If-Range` accepts a strong ETag or the exact `Last-Modified` date the answer published, and never a weak ETag (RFC 9110 §13.1.5); a streamed file answer resumes from its date (`a_weak_etag_does_not_satisfy_if_range_but_the_published_date_does`) |
| Pre-compressed `.gz`/`.br` assets | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Opt-in through `StaticDirOptions::precompressed`: a request for `app.js` is answered from `app.js.br` or `app.js.gz` when the sibling exists and the client's `Accept-Encoding` selects that coding, with `Content-Encoding` naming it, the base file's MIME type, an `ETag` computed over the bytes actually sent and `Vary: Accept-Encoding` on every answer — the plain one and the `304` included, so a cache never replays one encoding as another. Selection reuses the middleware's single `Accept-Encoding` authority (`select_encoding`, q-values and wildcard, tie to Brotli) rather than a second parser, and `accepts_identity` is its companion: `identity;q=0` or a bare `*;q=0` is answered `406` (with `Vary`) instead of a body the client refused, while naming only other codings still permits the plain file. The sibling is canonicalised and re-checked against the root — a symlinked `app.js.br` cannot point out of the tree — and when listing is on the variants are folded into their base entry. Off by default, so nothing changes for a tree whose siblings are not deliberate. Pinned by `precompressed_variants_are_selected_by_accept_encoding`, `test_accepts_identity_only_refuses_explicit_zero` and the existing `select_encoding` suite |
| Streaming / `sendfile` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | A static file is planned from its metadata (`plan_file` in `framework/http/src/static_files.rs`) and its bytes are streamed from disk in 64 KiB reads (`FileBody` in `framework/aether/src/static_dir.rs`), so the answer costs the memory budget one chunk and not the file. `HEAD`, `304` and `416` are decided before the payload is opened, so none of them reads the file at all — the row's old note that HEAD also read it is no longer true. A range seeks straight to its first byte (`FileBody::open` seeks, `remaining` is the range length), and a file that shrinks mid-stream is one error the writer turns into a closed connection, not a short body under a full `Content-Length`. The validator is built from metadata and published with `Last-Modified` (see the *Strong ETag* row for why it is weak). `App::static_file` uses the same path, so a registered file is streamed too. Pinned by `a_static_file_is_streamed_with_its_length_and_its_ranges`, `a_streamed_file_revalidates_weakly_and_resumes_from_its_date`, `an_unsatisfiable_range_is_416_with_the_full_length`, the `static_files_support_conditional_and_range_requests` e2e test, `a_file_body_reads_in_bounded_chunks_and_starts_at_its_offset` and the three `plan_file` tests. Literal `sendfile(2)` is not used: the reads are buffered 64 KiB chunks, which is what keeps the memory bound expressible |
| Async disk I/O | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `tokio::fs` in `static_dir`; in-memory helpers are caller-supplied bytes |

## Testing utilities

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Unit tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 495 in-crate tests, re-measured 2026-11-06 on the track-143 tree (`cargo test --workspace`: 946 passed / 0 failed / 1 ignored, incl. 430 integration across 30 targets + 21 doc over 15 doc runs) |
| Integration tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 430 tests across 30 checked-in targets (counted from the per-target `test result:` lines, 2026-11-06, track-142 tree), largest being `tests/http_e2e.rs` (102 raw-socket wire tests), `tests/http2_e2e.rs` (86, five of them the signed cookie read over this frontend, five added by track 125 for the CORS and identity refusals, and one added by track 126 for the plain `cookie()` accessor reading a jar a client split across fields), `tests/nested_routers.rs` (14), `scheduler/tests/engine.rs` (21), `tests/extractors.rs` (21), the two property suites (`http/tests/parser_properties.rs` 4, `router/tests/router_properties.rs` 5), `security/tests/secret_rendering.rs` (2), `tests/http_conformance.rs` (7 request-target wire tests), `tests/streaming_request.rs` (13, three of them the streamed declared-length lane), `tests/openapi_derivation.rs` (8 tests reading the document an `AetherApp` derives from its own route table) and `tests/signed_cookies.rs` (11 tests that a cookie the client wrote is not a cookie this server issued, mirrored by five HTTP/2 wire tests inside `tests/http2_e2e.rs`) |
| End-to-end wire tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `framework/tests/tests/http_e2e.rs`: routing, pipelining, chunked bodies (including one split across reads and one answered by `Expect: 100-continue`), duplicate headers, multipart, static ranges, the opt-in directory listing and pre-compressed static selection, CORS, limits, HTTP/1.0 close-by-default with the version echoed, Host allow-list refusing before routing, 429 + `Retry-After`, panic isolation, timeout, shutdown, WebSocket upgrade/echo/close, and fifteen tests that register a WebSocket route (by `app.ws` or a group `.ws`) *and* install middleware (recounted mechanically from the file: chain runs, refusal before `next` answers, refusal after `next` answers, a post-`next` stall times out at 504, panic maps to 500, a chain forked into `tokio::spawn` still gets the guard's own answer and no handover, an abandoned chain future never starts a session, a guard can neither grant an upgrade by adding headers nor revoke one by removing them (the request header store is read-only), group middleware wraps a group WS route, middleware registered after the route does not wrap it, a non-`GET` refused before the chain (counter, not just `405`), a hand-built `101` refused with `500`, that refusal's scope, a per-route chain wrapping one WS route's upgrade request, and a per-route guard refusing one upgrade), plus shutdown sending a `1001` going-away close frame and the receive loop returning on it, plus a handler that never returns to `recv` still aborted at the shutdown grace |
| Fuzz target | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `fuzz/fuzz_targets/http_parse_fuzz.rs` drives a deterministic mutation campaign over `RequestView::parse`, `parse_request_head`, `parse_request`, `awaits_100_continue` and the incremental chunked decoder, from 12 committed seeds under `fuzz/corpus/http_parse/`; a panic is saved under `fuzz/artifacts/` and exits non-zero. Run on 2026-10-30 (release, seed 20261030): **16 089 088 executions in 120.006 s** (134 069/s), corpus grown from its 12 committed seeds to 711 inputs, 709 outcome buckets, **0 crashes**. That is the re-run over the track-106 parser, which changed how a request-target is classified; the track-102b run that first closed this row made 15 843 328 executions in 120.011 s, corpus 719 and 717 buckets, also 0 crashes. The oracle was proved to fire: a one-line chunk-boundary mutant (`data.len() - self.pos < 2` dropped) was caught at execution 6 507 with an 8 192-byte artifact. Stated limits: no `cargo-fuzz`, no sanitizer and no coverage instrumentation (buckets hash parse outcomes, not edges), so memory-unsafety still needs ASan on a Linux host |
| Test client / request builder | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether::test_client::{TestClient, TestRequest, TestResponse}`: real HTTP/1.1 over loopback, request builder, JSON/form helpers, graceful shutdown; 12 wire tests in `tests/tests/test_client.rs` |
| Property-based tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `proptest` 1.12 as a dev-dependency of `aether-http` and `aether-router`. Four parser properties (`http/tests/parser_properties.rs`): arbitrary bytes never panic any entry point; a generated well-formed request round-trips method, target, query, headers, body and exact consumed length through both `RequestView::parse` and `parse_request`; every proper prefix is `Ok(None)` rather than prematurely complete; a chunked body decodes identically however its wire is split. Four router properties (`router/tests/router_properties.rs`): a registered static route matches itself; declared `{names}` are the capture keys in declaration order even when a sibling route registered those nodes first; a literal beats a parameter sibling; an unserved method on an existing path is 405 with the exact union of the path's methods, and an absent path is 404. Eleven targeted mutants — seven parser (premature completion, consumed length and query split, with the first two broken in each entry point) and four router (restamped names, precedence, the 405 union and 404 versus 405) — were each killed by a named property; see the register entry for track 102a |

## WebSocket

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Frame parser | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | RSV/opcode/control-frame/minimal-length rules; 8 parser tests |
| Client-frame unmasking | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Mask requirement + unmask tests; unmasked client frame gets close 1002 in E2E |
| HTTP upgrade handshake | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `validate_handshake` (GET, `Upgrade`, `Connection` tokens, version 13, canonical 16-byte key, RFC sample accept vector); `AetherApp::ws` writes 101; 400/426/405 E2E |
| User middleware around the upgrade request | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | The upgrade request is dispatched through the route's middleware chain before the handshake is validated, so an app guard stamps `Extensions` the handler reads and a guard's response wins over the upgrade. Fifteen wire tests in `tests/tests/http_e2e.rs` (recounted from the file): `a_websocket_upgrade_runs_the_app_middleware_chain`, `a_middleware_can_refuse_a_websocket_upgrade` (401 + handler never starts + the socket still serves the next request), `a_middleware_that_answers_after_next_refuses_the_upgrade` (403 returned *after* awaiting `next`), `a_middleware_hanging_after_next_times_the_upgrade_request_out` (504 + close, no `101`), `a_panicking_middleware_answers_the_upgrade_request_with_500` (panic before *and* after `next`, both 500), `a_middleware_that_forks_the_chain_still_answers_the_upgrade_request` (the chain is `tokio::spawn`ed and completes with `101` while the guard returns 202 — the client gets 202, no handover), `a_middleware_that_abandons_the_chain_future_refuses_the_upgrade` (dropped future, no session started), `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` (a guard reads the same store the validator later reads and sees the real handshake fields; a plain `GET` carries none, so it still answers `400`; passing the request through still upgrades), `group_middleware_wraps_the_upgrade_request_of_a_group_websocket_route`, `middleware_registered_after_a_websocket_route_does_not_wrap_it`, `per_route_middleware_wraps_the_upgrade_request_of_its_websocket_route` (a chain bound to one registration, with a sibling route as the control), `a_per_route_guard_refuses_a_websocket_upgrade_before_the_handover` (401 and the handler never starts, then the same route upgrades when the guard is satisfied), `a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain` (`405` + `Allow: GET` with the chain counter still at 0, and a positive control showing an ordinary request does run the chain), `a_hand_built_101_from_a_middleware_is_refused_with_500` (item 18), `a_101_without_a_handoff_is_never_served` (item 18; since track 58 the refusal covers a handler's `101` on an ordinary route too, over both protocols, and track 89 widened it from `101` to the whole informational class at the shared response exit — `every_1xx_from_a_handler_is_refused_not_only_101` pins `103` and `199` answering `500`). This closes PROB-0017, where `app.ws` bypassed the chain and `RouteGroup::ws` dropped the group's own middleware. **Stated limits:** the upgrade proceeds only when the chain hands back the terminal's `101` untouched, so middleware cannot edit that response (item 18), it cannot grant or revoke an upgrade by mutating headers — there is no mutable header store at all (PROB-0005, closed in 0.2: one store behind read-only accessors), so a returned response is the only lever, a guard cannot call `next` twice because `RequestContext` is not `Clone` and `next` consumes it (verified from the type signatures, not from a test — the second call does not compile), and the session after handover is not wrapped by anything |
| Message loop / connection driver | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Bounded `WebSocket`: fragmentation, interleaved control frames, auto-pong, close echo, UTF-8 and close-code validation, 16 MiB default message/frame caps; 19 driver tests in `websocket/src/connection.rs` (recounted from the file 2026-10-27) + E2E. Both directions carry an inactivity policy of the same shape: `WebSocketLimits::read_timeout` (120 s default) bounds a socket wait inside `recv` and fails the session with `WsError::IdleTimeout`, and `WebSocketLimits::write_timeout` (10 s default, added 2026-10-26 for PROB-0030) bounds how long a peer may leave an outgoing frame unconsumed before it fails with `WsError::WriteTimeout`. Either `None` disables that direction, and both bound an interval rather than a whole frame — an interval renews only after 1024 bytes have actually left, so a normally-consuming peer is never cut off mid-message and a trickling one is shed |
| Message-size and buffer bounds | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Declared frame length rejected before payload is buffered; reassembled message cap; tests |
| TLS transport (WSS) | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | VERIFIED_IMPLEMENTED | `WebSocket<S>` is generic over the transport and the server hands it the type-erased `aether::transport::Stream`, so an upgraded session runs over a rustls socket: TLS handshake, `101`, the RFC accept vector, an echoed frame and a close frame are all verified on one live WSS connection (`websocket_upgrade_and_echo_run_over_tls`). **Breaking API consequence:** the handler signature changed from `WebSocket<tokio::net::TcpStream>` to `WebSocket<aether::transport::Stream>` — explicit annotations are rare and nothing in-workspace or in the downstream probe annotates it, but embedders that do must update the type |
| Subprotocol negotiation | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED | `AetherApp::ws_subprotocols` (and `RouteGroup::ws_subprotocols`) declare a route's protocols in its own preference order; `select_subprotocol` applies RFC 6455 §4.2.2 — the first declared name the client also offered is echoed in the `101`, whatever order the client listed it in — and only a declared name is ever echoed, so a client's bytes cannot reach a response header. A client that offers nothing gets the plain upgrade; one that offers protocols and matches none is answered `400` before any session starts, because a client that asked for a subprotocol is required to fail the connection when the `101` names none, so a `101` would only relocate that failure. Routes registered with `ws` declare nothing and ignore any offer, which is the pre-negotiation behaviour. **Stated limits:** the session is not told the selection (the handler knows its route; the raw offer is still in `RequestContext::header`), and a route declares its protocols through `ws_subprotocols` only — there is no separate `ws_with` + protocols combination. Pinned by four unit tests in `websocket/src/handshake.rs` (`selection_prefers_the_routes_declared_order`, `selection_ignores_ows_and_is_case_sensitive`, `no_offer_is_distinct_from_no_match`, `only_a_declared_name_is_ever_echoed`) and five wire tests (`an_offered_subprotocol_is_echoed_from_the_route_declaration`, `a_route_without_a_protocol_declaration_ignores_the_offer`, `an_offer_that_matches_no_declared_protocol_is_refused`, `a_client_that_offers_no_subprotocol_still_gets_the_route`, `a_group_websocket_route_negotiates_its_subprotocol`) |
| Streaming / permessage-deflate | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | No compression or streaming API |
| Graceful close on server shutdown | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Since 2026-10-27 an upgraded session whose server starts shutting down gets one close frame with status `1001` ("going away", RFC 6455 §7.4.1.1) carrying the reason `server shutting down`, then its write half is shut down so the frame cannot be lost to an RST. The socket writes it, not the connection task, because the task no longer owns the transport once the handler holds it: `WebSocket::recv` races the peer read against a `watch` receiver with `biased` select (so a reader that never blocks cannot starve the signal) and ends the session through the same `Ok(None)` the closing handshake returns, which is what makes the handler's loop return. `WS_SHUTDOWN_GRACE` (1 s, `app.rs`) bounds the wait for that return; a handler that never comes back to `recv` is still aborted, exactly as before the frame existed. A receiver cloned after the flag already flipped is handled by checking the current value before waiting on `changed()`, and a dropped sender resolves rather than parking. Pinned by `a_server_shutdown_closes_a_waiting_session_with_going_away`, `a_shutdown_that_started_before_the_upgrade_still_closes_the_session`, `a_session_closed_by_shutdown_refuses_further_sends`, `shutdown_sends_a_going_away_close_frame_and_ends_the_session` and `a_session_that_never_returns_to_recv_is_still_aborted_at_shutdown` |

## Developer experience

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Typed route registration | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Method-specific builders; PATCH supported |
| Handler macros | NOT_IMPLEMENTED | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | Manual closures |
| OpenAPI generation | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Reachable at `aether::openapi` since track 91 (2026-10-28) and **derived from the route table** since track 108 (2026-10-31): `AetherApp::openapi_spec(title, version)` enumerates every registration the application carries through `RadixRouter::route_listing`, which rebuilds each declared template with that route's own capture names, and `OpenApiSpec::from_routes` emits an OpenAPI 3.1.0 document — one path item per path with a lowercase verb key per method, a `required` path parameter for every `{slot}` and every `*catch-all`, and `operationId`s that stay unique when two paths sanitise alike. A `RouteGroup` or an `AetherApp::mount` contributes its prefixed routes, a `static_dir` appears as the catch-all it registers (plus the `{prefix}` route the tree also registers when it lists or its root ships an `index.html`), and a `ws` registration carries the `101` plus the `400`/`426` refusals the framework actually answers instead of a `200` it never sends. The output is a function of the route table, not of the order it was written in: the listing sorts and the generator sorts again. **Stated limits:** the document claims only what a registration proves — a registration records a method, a path template and a handler id, no request or response type, so an operation carries a status and no schema; `HEAD` and the generic `OPTIONS` answer are produced by the framework without a registration, so they are not listed; and two `Method::Custom` routes on one path collapse into one operation, because the verb map keys by method name. Pinned by seven `aether-openapi` unit tests (`the_document_uses_the_specs_field_names`, `a_catch_all_is_derived_as_a_path_parameter`, `the_derived_document_does_not_depend_on_input_order`, `operation_ids_are_unique_when_two_paths_sanitise_alike`, `a_websocket_registration_answers_the_upgrade_and_its_refusals`, `nothing_in_a_derived_document_claims_a_body_schema`, `a_malformed_slot_declares_no_parameter`) and eight app-level tests in `framework/tests/tests/openapi_derivation.rs` (`the_document_covers_every_route_the_app_registered`, `a_shared_dynamic_level_is_named_per_route`, `a_group_and_a_mount_contribute_their_prefixed_routes`, `a_static_dir_is_listed_as_the_catch_all_it_registers`, `a_websocket_route_carries_the_upgrade_instead_of_a_twenty`, `the_document_omits_the_answers_that_have_no_registration`, `the_document_is_the_same_whatever_order_the_routes_were_registered_in`, `the_derived_json_carries_the_specs_keys`) |
| Environment configuration | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Seven `AETHER_*` variables, strict parsing; `workers` sizes the runtime when `AetherApp::run` builds it (track 62), and the banner prints the active count, not the requested one |
| Startup banner / diagnostics | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | Banner now states actual runtime/allocator |
| Compiling examples | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `basic_api`, `full_async_api` compile in the workspace |
| Hot reload | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | — |
| README / docs accuracy | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Root README was an unrelated AI Studio template; corrected during this audit. The live set is now checked mechanically on four axes. `cite_check.py` pins 112 citations over 77 spans in 16 live documents across root `docs/` and the teaching set in `framework/docs/` (track 92 extended it: 27 citations over 24 spans, 10 coordinates corrected and a comma-separated pair of error-code lines in `core/src/error.rs` rewritten as two citations; track 98c added the five CSRF spans its two sections name; track 104 made `PRODUCTION_READINESS_REPORT.md` a live document and pinned the five spans its maintained sections name), and since track 109 (2026-10-31) the same gate also reads the names those documents assert: it indexes the product's own sources and resolves every code span in all 28 live documents — re-counted 2026-11-06 on the track-144 tree at 390 qualified and 1455 bare names, 36 mentions sheltered by 30 per-document reasons, 10 attributed to another crate, 1 design document exempted as a proposal, 0 findings. It found one real phantom the coordinates could not see: the Arabic error-handling guide promised a production answer carrying code and error_id, while the answer this tree builds carries error and code and the workspace declares no error_id anywhere — and that sentence's own citation was green the whole time. Neither README was read by any gate before track 109; both are now, but its loudest claims re-measured true here: 946 / 0 / 1, 16 workspace crates, and the per-suite breakdown the root page prints. `evidence_check.py` fails a document that cites a path outside the repository, and a test asserts the two document gates read an identical corpus (33 → 36 documents). The `counts` gate (tracks 119-120) holds every printed suite, citation and status figure to the run that wrote the log, in six live documents with this matrix among them. The gate cannot shelter itself: only sources under `framework/` are API evidence, because the first version harvested the tool's own comments and turned six genuine review records into false stale-exception findings. Still `PARTIALLY_IMPLEMENTED`, for two stated reasons: the eight dated records keep their wrong numbers behind a `cite-check: snapshot` marker and a superseded banner on purpose, because rewriting a historical report makes it a different report; and a name that exists is not a claim that is true — the pass proves a document does not invent an API, not that its prose describes behaviour. Measured and re-worded 2026-10-27 (track 88), coverage extended 2026-10-28 (track 92), names added 2026-10-31 (track 109), counts added 2026-11-04 (track 119) |

## Performance features

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Zero-copy head parsing | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Parser borrows the connection buffer, but the server copies the body and every header into owned `RequestContext` fields before handlers run |
| Request arena | NOT_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `RequestArena` exists in `aether-core` and is tested, but the server path does not use it |
| SIMD-style validation | PARTIALLY_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | 64-bit word ASCII scan used by `Email`; not in the HTTP hot path |
| Connection buffer reuse | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `BytesMut` retained per connection; `split_to` for pipelining |
| Thread-per-core runtime | NOT_IMPLEMENTED | NOT_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Aether uses Tokio's multi-threaded work-stealing runtime; `AETHER_WORKERS` sets its thread count under `AetherApp::run`, which is not thread-per-core placement |
| Compression | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in gzip/brotli middleware |
| Release profile tuned for size | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "unwind"` |

## Status count (Aether only)

| Status | Rows |
|---|---|
| VERIFIED_IMPLEMENTED | 123 |
| PARTIALLY_IMPLEMENTED | 3 |
| NOT_IMPLEMENTED | 8 |
| BROKEN | 0 |
| UNVERIFIED | 0 |
| UNWIRED_IMPLEMENTATION | 0 |
| NOT_APPLICABLE | 2 |

Total: 136 rows.

The current reading, re-derived from the rows above by the `counts` gate: **123 / 3 / 8 / 0 / 0 / 0 / 2 = 136 rows**.

The totals are the literal row counts in the table above — every five-column `| Feature | Aether | … |`
row, with `cells[1]` taken as the status word before its first parenthesis. Recounted 2026-10-29 by
stripping only the outer pipes and requiring exactly five cells, skipping the header and separator
rows. **The first recount of this ticket read 104 / 9 / 19 / 0 / 0 / 0 / 2 = 134 at `f3b1e02` and
`cba12f1`, and that was a tool artifact, not a drift in the table.** The `awk` used required exactly
seven pipe-delimited fields, and one table row — *HTTP/2 over TLS* — had lost its closing `|`, so it
split into six fields and was silently dropped; 134 was one row short. The same day's second pass,
run in Python against the rule stated above, reads **105 / 9 / 19 / 0 / 0 / 0 / 2 = 135** on
`f3b1e02`, on `cba12f1` and on the commit before this track, and 134 rows (100 / 10 / 19 / 3 / 2) on
`e60eca7` and 135 (102 / 8 / 19 / 4 / 2) on `72459e6`, so the published table was right and no
track-88/91 arithmetic is implicated. The row has been given its closing pipe in this track so both
parsers agree. Track 94's changes are two rows moved from `PARTIALLY_IMPLEMENTED` to
`VERIFIED_IMPLEMENTED` — *Per-route middleware/guards* and *Per-route middleware*, now that
`route_sync_with`, `route_async_with` and `ws_with` exist with wire tests — so 105 / 9 becomes
107 / 7 and the total stays 135. Track 95 moves one more row the same way — *Subprotocol
negotiation*, now that `ws_subprotocols`, `select_subprotocol` and the echoed `101` line exist —
taking it to 108 / 6 / 19. Track 96 moved *Automatic OPTIONS / CORS preflight* and *CSP* the same
way in code and in `.project-governance/notes/2026-10-21-improvement-register.md`, but this count
was left at 108 / 6 / 19, so for one track the section and the table it summarises disagreed — the
same defect class the register's B11 records, found here while recounting (the file itself read
110 / 5 / 18 at track 96's tree). Track 97 moves the two rows it implemented — *Directory listing*
and *Pre-compressed `.gz`/`.br` assets* — from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`, so the
table read **112 / 5 / 16 / 0 / 0 / 0 / 2 = 135**, recounted mechanically from the file with
the rule above on the track-97 tree. Track 98a adds a row rather than moving one — *Forwarded
client identity* — so the total grows to 136 and the first cell to 113: **113 / 5 / 16 / 0 / 0 /
0 / 2 = 136**, recounted the same way on the track-98a tree. Track 98b moves one row —
*Authentication middleware* — from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`. It published
**114 / 4 / 16** here and in the table above, but that arithmetic decrements
`PARTIALLY_IMPLEMENTED` instead of `NOT_IMPLEMENTED`; recounting the same file against the rule
here reads 114 / 5 / 15 — the section-versus-table disagreement this paragraph already records
for track 96, found again and corrected now rather than carried forward. Track 98c moves one row —
*CSRF protection* — the same way, taking it to **115 / 5 / 14 / 0 / 0 / 0 / 2 = 136**, recounted
mechanically from the file on the track-98c tree. Tracks 99 and 100 moved three rows under this
section without it following: *Response body streaming* and *Streaming / `sendfile`* left
`NOT_IMPLEMENTED` for `VERIFIED_IMPLEMENTED` in track 99 (the status table above read 117 / 5 / 12
while this section still said 115 / 5 / 14), and *Streaming request bodies* left
`NOT_IMPLEMENTED` for `PARTIALLY_IMPLEMENTED` in track 100. Track 100b carries the last one to
`VERIFIED_IMPLEMENTED` now that the HTTP/2 frontend streams too, and the section is recounted from
the rows again: **118 / 5 / 11 / 0 / 0 / 0 / 2 = 136** on the track-100b tree. This is the third
appearance of the section-versus-table drift (tracks 96, 98b, and now 99–100), always the same
way: a row is flipped in the table and this summary is not re-run. Track 101 moved *OCSP stapling /
revocation* from `PARTIALLY_IMPLEMENTED` to `VERIFIED_IMPLEMENTED` (the offline CRL and OCSP
enforcement of 101a/101b), and this section was re-run with the flip in the same commit.
Track 102 did not move a status for *Fuzz target* (it was already verified) but replaced
its "compiles" evidence with a sustained campaign, and flipped *Property-based tests*
from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`; this section was re-run in the same
commit: **120 / 4 / 10 / 0 / 0 / 0 / 2 = 136** on the track-102 tree. Track 103 moved no status — the *Secret management* row stays `NOT_IMPLEMENTED` because the track's answer is a documented non-goal — so a re-run of the same rules gives the same **120 / 4 / 10 / 0 / 0 / 0 / 2 = 136** on the track-103 tree; only that row's evidence cell changed. Tracks 104–106 moved no status either (104 made the readiness report a checked live document, 105 and 106 rewrote evidence cells for rows already `VERIFIED_IMPLEMENTED`), and the recount on the track-106 tree still gave **120 / 4 / 10 / 0 / 0 / 0 / 2 = 136**. Track 107 moves *Nested routers / independent sub-apps* from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED` — group-scoped state and a mountable `Router` — so this section was re-run with the flip in the same commit: **121 / 4 / 9 / 0 / 0 / 0 / 2 = 136** on the track-107 tree. Track 108 moves *OpenAPI generation* from `PARTIALLY_IMPLEMENTED` to `VERIFIED_IMPLEMENTED` — the document is derived from the registered routes now, not built by hand — so this section was re-run with the flip in the same commit: **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136** on the track-108 tree. Tracks 109–116 moved no status — 109 added the name-resolution gate, 110 and 116 pinned wire contracts on rows already verified, 111 deleted scaffold, 112 and 113 and 114 and 115 changed documents, tools or evidence cells — so the recount on the track-116 tree is the same **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136**, re-derived here by two independent parsers that agree on the same line set, and the claim is checked rather than asserted by diffing every (feature, status) pair in this table against commit 9329a19 (track 108): the pair set was unchanged through track 116. Track 120 moves *Bounded response memory* from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED` — the row's evidence had described the priced answer and named its tests since tracks 74–75, while its status cell was never moved — so this section was re-run with the flip in the same commit: **123 / 3 / 8 / 0 / 0 / 0 / 2 = 136** on the track-120 tree.

The count is not a quality score: a `NOT_APPLICABLE` row (database pool, Redis) is not a
deficiency, and several `NOT_IMPLEMENTED` rows are ecosystem features in Axum/Actix too. The
error-contract round (PROB-0009, 2026-10-24) flipped *Typed error → status
mapping* from `PARTIALLY_IMPLEMENTED` to `VERIFIED_IMPLEMENTED` without adding or removing a row.

---

## Highest-priority remaining work

### Correctness / security
1. ~~**Error detail is dropped, not logged.**~~ **Closed 2026-10-27.** A 5xx still keeps its
   message off the wire (`aether/src/app.rs:209-214`), but the redacted detail now reaches an
   optional process-wide reporter — `aether::set_error_reporter`, given
   `aether::ErrorReport { request_id, status, code, detail }` — under the response's own
   `X-Request-ID`. The request serial is carried as a task-local through `tokio::spawn`
   (`aether/src/serve.rs:905-912`), so the hot path allocates nothing; the id string is formatted only
   on the failure path. A type that redacts stays silent until it opts in via `redacted_detail`,
   and a 4xx is never reported. Pinned by
   `a_redacted_5xx_detail_reaches_the_reporter_under_the_response_request_id`.
2. ~~**Separate receive deadlines.** The total receive deadline currently reuses
   `max_request_time` (also the handler timeout), so a legitimately slow large upload and a
   slow handler share one budget. A dedicated header/body timeout setting is preferable.~~
   **Closed 2026-10-27 (track 84).** `max_receive_time` (`AETHER_MAX_RECEIVE_TIME_SECS`) now
   bounds the HTTP/1 receive phase and the HTTP/2 body deadline, while `max_request_time` bounds
   handler execution only. Pinned in both directions by
   `slow_trickle_receives_408_within_the_total_receive_deadline` and
   `a_slow_upload_is_not_bounded_by_the_handler_timeout`.
3. ~~**Client-disconnect cancellation.** A handler is not cancelled when its client goes away.~~
   **Closed 2026-10-27 (track 85).** A running handler is now aborted when its peer is observably
   gone: HTTP/1 on an RST (plaintext) or rustls's end-without-`close_notify` (TLS), HTTP/2 on a
   per-stream `RST_STREAM` or a dead connection. A clean EOF/half-close stays answerable by design
   (RFC 8446; `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes` is the contract),
   and is still detected at write time. Pinned in all three protocols by
   `client_disconnect_cancels_the_running_handler`, `a_stream_reset_cancels_the_running_handler_and_keeps_the_session`,
   `a_dropped_h2_connection_cancels_its_running_handlers`, `a_tls_client_that_leaves_cancels_the_running_handler`,
   and in the not-gone direction by `a_pipelined_request_does_not_look_like_a_disconnect`,
   `a_client_that_stays_connected_is_not_cancelled` and `a_tls_client_that_half_closes_still_gets_its_response`.
4. ~~**CSRF middleware.**~~ **Closed 2026-10-29 (track 98c).** Signed double-submit, app-wide: a
   safe response issues a token the handler renders, an unsafe method must echo it in one header,
   and every failure is one empty-bodied `403` refused before the handler. A verified bearer token
   travels in a header and is still not an ambient credential; the cookie-authenticated form is now
   covered too. What remains absent is binding the token to a server-side session — there is no
   session store to bind it to, and the HMAC secret is process-wide by design.

### Feature completeness
5. HTTP/3 (a new connection engine: QUIC + `quinn`, no TLS-in-crate path applies). HTTP/2 is
   delivered over TLS by ALPN (2026-10-20) and over cleartext by opt-in prior knowledge
   (2026-10-21, `AetherApp::enable_h2c`); what remains inside HTTP/2 is server push, which RFC 9113
   §10.8 deprecates, and the `Upgrade: h2c` exchange, which RFC 9113 §3.1 deprecates and this
   project declines for the reasons on its row.
6. ~~HTTP/2 request-body streaming.~~ **Delivered on both frontends** (2026-10-29 HTTP/1.1,
   2026-10-30 HTTP/2): HTTP/1.1 in
   track 100 (`route_stream`, `RequestContext::read_body`, a per-chunk memory registration,
   `max_stream_body_bytes`, a receive deadline and the close-on-unread rule), HTTP/2 in track
   100b (`serve_streamed` + `Http2Body`: dispatch from HEADERS, DATA pulled into the same lane,
   flow-control credit per frame, the same ceiling and deadline, and an unread body abandoned by
   dropping the `RecvStream` so the connection survives).
7. Custom and per-route middleware API (**delivered** 2026-10-18: app-wide and group-scoped
   `use_middleware` with `Next`-continuation composition; the dedicated single-route form —
   `route_sync_with`, `route_async_with`, `ws_with` — delivered 2026-10-29, track 94).
8. Typed extractors (delivered 2026-10-18: `Path`/`Query`/`Json`/`Form`/`State`) and
   request-scoped extensions (delivered 2026-10-21: `Extensions` type-map, `RequestContext`
   accessors, `Extension<T>` extractor, proven per-stream over HTTP/2 and non-leaking across
   keep-alive requests).
9. WebSocket permessage-deflate (compression is owner-deferred) and subprotocol negotiation —
   negotiation delivered 2026-10-29 (track 95: `ws_subprotocols`, RFC 6455 §4.2.2 selection, the
   echoed `Sec-WebSocket-Protocol` line, and `400` when a declared-protocol route cannot meet the
   client's offer). The graceful close frame on server shutdown was
   delivered 2026-10-27: a session gets one `1001` going-away frame and ends on it, and only a
   handler that never returns to `recv` is aborted at a 1 s grace.

### Performance (only after measurement)
10. Remove per-request header `HashMap` construction and body copy where handlers do not need
    them; re-measure before/after at several concurrency levels. Do not assume zero-copy wins
    end-to-end.
11. Arena integration, *if* profiling shows allocator time in the request path.
12. ~~Run the fuzz target for a sustained period.~~ **Closed 2026-10-30 (track 102b).**
    The in-repo runner completed 15 843 328 executions in 120.011 s at seed 20261030
    (12 seeds, corpus 719, 717 outcome buckets, 0 crashes) and caught a deliberately
    injected chunk-boundary panic at execution 6 507. Track 106 re-ran it over its own
    parser change: 16 089 088 executions in 120.006 s, corpus 711, 709 outcome buckets,
    0 crashes. It is outcome-guided, not coverage-guided, and it does not replace ASan on
    a host that has one.

### HTTP/2 follow-ups (found while verifying the protocol stage, 2026-10-21)
13. ~~**Two enforced limits are not operator-configurable.**~~ **Closed 2026-10-22.**
    `max_concurrent_streams` and `max_headers_count` are fields of `AetherConfig` (and env
    variables `AETHER_MAX_CONCURRENT_STREAMS` / `AETHER_MAX_HEADERS_COUNT`), and
    `with_config`/`apply_config` now derive `ServerLimits` from one shared function instead of two
    hand-written field copies. The wire tests pin each bound twice: once as the default it
    produces and once as a number only a configured field can produce
    (`a_concurrency_cap_set_through_the_config_is_advertised_and_enforced` reads the advertised
    `SETTINGS_MAX_CONCURRENT_STREAMS` off the wire, `a_field_line_budget_set_through_the_config_is_the_number_it_counts`
    checks the 431 boundary at the configured line count). `AetherApp::with_limits` was not added:
    the configuration object already is that setter, and a second one would be a second
    derivation to keep in sync.
14. **This server advertises receive windows and does not tune them.** `http2::builder` sets
    `MAX_CONCURRENT_STREAMS` and `MAX_HEADER_LIST_SIZE` only, so `h2`'s defaults apply: a 64 KiB
    stream window and a 64 KiB connection window. Credits are now released as bytes arrive, so
    uploads of any size work; a larger advertised window would trade memory (each stream may hold
    that much unacknowledged data) for fewer round trips. No measurement of that trade exists yet,
    which is why the numbers were not changed.
15. **A connection whose peer goes silent after GOAWAY costs the drain bound.** Graceful shutdown
    sends GOAWAY and `h2` closes the connection once its streams end, but a hand-framed client that
    neither closes nor resets its refused streams keeps the accept future pending until the 30 s
    drain bound fires. Bounded, not a hang, and it is why `streams_over_the_advertised_concurrency_cap_…`
    ends its own connection before stopping the server. A per-peer connection bound does not exist
    either: `max_connections` is global, and no code counts connections per address.
16. **Lints and format are both gated as of 2026-10-26.**
    The two sentences this item used to lead and close with — "the build is not warning-gated" and
    "there is no `-D warnings` gate, so 'clippy clean' is not a claim this matrix makes" — are retired.
    `cargo clippy --workspace --all-targets -- -D warnings` exits **0** on the 2026-10-26 tree
    (track 73 slice 2, three commits, 28 files). Five lints were refused rather than fixed, each as
    `#[expect(lint, reason = …)]` naming its cost: `ServeOutcome`'s and `Stream`'s
    `large_enum_variant` (boxing costs an allocation per upgrade / per connection), the CORS preflight
    `collapsible_if` (a 30-line commented block re-indented for style), the router's 8-argument
    `search`, and `safe_resolve_path`'s `Result<_, ()>` — the last one is the only refusal whose
    alternative is user-visible, so it is on the breaking-changes list, not in a lint commit. Because an
    unfulfilled `#[expect]` is itself a warning, the green run proves all five are still earned; the
    gate is green *with* its exceptions named, not silenced.
    History, kept because the method is the point: the 2026-10-21 lib-scope reading was 39 warnings
    (13 `aether`, 9 `aether-http`, 7 `aether-middleware`, 4 `aether-security`, 3 `aether-router`,
    2 `aether-scheduler`, 1 `aether-validation`), counted from the per-target `generated N warnings`
    summaries — counting `warning:` lines over-counts (46) because each summary is itself a `warning:`
    line, and `cargo clippy -p aether --all-targets` printed 41 for 34 unique warnings because lib and
    lib-test duplicate sites. Those two figures were both real, of differently scoped runs; that was
    the whole of the earlier "41 could not be reproduced" note. Scope cut both ways: until 2026-10-22
    `--all-targets` did not merely warn, it **failed** on
    `clippy::unused_io_amount` at [`framework/tests/tests/tls_e2e.rs`](../tests/tests/tls_e2e.rs) — a
    deny-by-default *correctness* lint, because a single-byte TLS read loop discarded the returned count.
    Pinned as `git show 907dfcd^:framework/tests/tests/tls_e2e.rs`, whose line 452 reads
    `tls.read(&mut byte)` (the bare `git show HEAD:` form stopped reproducing once `907dfcd` landed the
    `read_exact` fix; replaced 2026-10-23), it was test code only and `cargo test` passed regardless
    because rustc does not run the lint. And every per-crate row above was scoped to one crate or one
    target, which is why none of them listed the `security` (8), `validation` (2) or `aether-tests` (23)
    records that the workspace `-D warnings` run found and closed.
    The invariant-not-a-coordinate rule still holds, and here it bit: `cargo clippy -p aether 2>&1 |
    grep "serve.rs"` was the re-runnable check that no lint fell in the WebSocket upgrade branch. On the
    2026-10-26 tree it prints nothing, because the crate has no warnings left — the answer that claim
    kept converging on, reached by removing the warnings rather than by re-checking their coordinates.
    The line numbers cited for it (87/192/445/461, branch spanning 239–346) are dated readings of a tree
    that no longer exists; none of the shifts they tracked ever changed the answer, which is exactly why
    this matrix does not treat a coordinate as evidence.
    Format is gated now too, and not by a sweep. `cargo fmt --all -- --check` exits **0** with zero
    diff blocks on the 2026-10-26 tree, after seven per-crate batches (leaf crates, router +
    websocket, http, middleware, `aether`, scheduler, the e2e test package — `63b6326..e7644b9`)
    instead of the one unreviewable repo-wide commit this item refused. The readings it closed are
    kept as the dated readings they are: 574 hunks across 66 files on 2026-10-23, 567 on 2026-10-22,
    558 before the behaviour-test and `101` slices, 570 / 65 after track 72, 560 / 65 after track 73
    slice 2. Two things the green run does **not** buy. There is no CI in this repository, so the gate
    is a command, not an enforcement: the next slice can re-introduce drift and only a re-run finds it,
    and the reading is rustfmt-version-dependent (`rustfmt 1.9.0-stable`, no `rustfmt.toml`, so every
    default is this toolchain's). And reformatting moves line numbers: every coordinate the batches
    shifted had to be re-derived in the docs that cite them as current code — `ACTIX_FAIR_COMPARISON.md`
    F1 and F4–F7, this matrix's `price_response_body` / `MIN_PROGRESS_FOR_RENEWAL` row, and
    `PRODUCTION_READINESS_REPORT.md`'s bodyless-`1xx` row — while the dated records in
    `.project-governance/` and the perf-baseline snapshots were deliberately left holding the
    coordinates of the trees they measured. That is the invariant-not-a-coordinate rule applied to the
    docs themselves: a coordinate says when it was true, and after a formatting change "when" moved.
17. ~~Extension payloads are not priced by the memory budget.~~ **Closed 2026-10-26 (PROB-0016).**
    The scheduler already accounted for a connection, its streams, socket buffers, request bodies and
    the block a response reserves; a value a middleware inserted was not counted, so a request could
    hold unreported memory. It is now: every entry costs
    `EXTENSION_ENTRY_MEMORY` (160 B) plus `size_of::<T>()` unconditionally, and an inserter that holds
    a heap allocation declares it with `Extensions::insert_sized` /
    `RequestContext::insert_extension_sized(value, owned_heap)` so the budget registers that too. The
    charge rides the request's own handler registration (attached in `dispatch`, released with the
    request), and a refused grow registers nothing while the answer is still served — best-effort, like
    `price_response_body`, and counted in `metrics.memory_shed`. The 160 B is measured, not chosen:
    `framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs` reports the map the framework builds
    (`Entry` = boxed value + two `u64` price fields, 32 B) at 264 B for its first entry (48 B boxed map +
    table + slot) and ~148 B marginal per further entry (16 entries 2 492 B). It reports the pre-pricing
    shape (200 B first, 1 724 B at 16, ~101 B marginal) in the same run because that is where the draft
    constant — 128 B — came from, and the draft under-priced the shape that actually shipped: the entry
    gained its price fields in the same track that measured them, so the constant was re-derived before
    it was committed. The same probe is why the cheaper "charge the map's capacity" option was refused —
    capacity is ~148–264 B/entry while one parsed JSON document retains 6.78–8.00× the bytes it was
    parsed from (2 MiB in, ~14 MB held).
    What stays unpriced, stated: heap nobody declared, growth through `get_mut`, and a `Vec`'s reserved
    tail beyond its length. See the "Request-scoped extensions/state" row and
    `PRODUCTION_READINESS_REPORT.md` §16.

### WebSocket (found while wiring middleware into the upgrade request, 2026-10-21)
18. **A `101` is outside the response pipeline.** The upgrade request now runs its middleware
    chain, and the chain's *status* decides what happens: only the terminal's `101` handed back
    untouched proceeds to the handshake, so a guard that refuses before `next` or replaces the
    answer after awaiting it gets that response served (403, 500, 504 — all wire-tested). The same
    rule is the whole story for exotic chain shapes: a guard that moves `next(req)` into
    `tokio::spawn` (or races it with `select!`) can let the chain complete with `101` internally and
    still answer the client with its own 202, and a guard that builds the chain future and drops it
    is answered with its own 503 — both pinned by
    `a_middleware_that_forks_the_chain_still_answers_the_upgrade_request` and
    `a_middleware_that_abandons_the_chain_future_refuses_the_upgrade`. The mechanism behind both is
    worth stating exactly, because the obvious explanation is wrong: calling `next` records the
    request *synchronously* (the terminal's closure body does it before returning its future), so a
    dropped chain future still fills the handover slot. What authorizes a handover is not reachability
    of the terminal but the **status the chain returns**, and that is why the guard is a status
    identity check rather than a flag. It was measured, not assumed: replacing it with
    `.filter(|_| true)` makes the fork test fail `left: 101, right: 202` and the abandoned test fail
    `left: 101, right: 503`. A guard cannot run `next`
    twice, and that was measured rather than read off the signatures: a scratch middleware that calls
    `next(req)` twice on the same request fails to compile with `error[E0382]: use of moved value:
    req` and the note that `RequestContext` does not implement `Copy`. So the double-handover case is
    unexpressible, not merely untested.
    There are two different `101`s here, and the sentence this section used to carry covered only one
    of them. The handshake the client receives on a **granted** upgrade is synthesized by the protocol
    frontend after the core returns, and CORS/security-header/compression post-processing does not
    touch it. A `101` a middleware **returns itself**, with the terminal never reached, is a different
    thing: `101` means the protocol transition happened, and that guard performed none. As of
    2026-10-22 the core refuses the claim with `500 Internal Server Error` rather than sending it
    (`None if chain_response.status == 101 => Response::status(500, ..)`), because serving it would put
    the client into WebSocket framing on a socket the server still parses as HTTP/1. Measured shape of
    the refusal — the injected guard was given a `101` carrying `Upgrade`, `Connection`,
    `Sec-WebSocket-Accept` and a JSON body, and none of it survives: the client receives
    `HTTP/1.1 500` with `X-Request-ID`, `Date` and `Content-Length: 0` only, no body, no session
    started, and the same socket still answers a following `GET` with `200`
    (`a_hand_built_101_from_a_middleware_is_refused_with_500`, which also proves the granted path is
    untouched by upgrading through the same middleware to the RFC accept vector and reading back the
    echoed frame).
    The refusal is a rule about the informational class, not about the arm it was first written in
    (track 58, widened in track 89): the core's single response exit replaces **any** `1xx` it did
    not synthesize for a handoff — `100 Continue` is written straight to the stream by the read
    loop, so no informational status reaching that exit has a transition behind it — so a guard's
    claim and a handler's on an ordinary route get the same `500`, and `http2_e2e`'s
    `a_101_from_a_middleware_is_refused_over_h2` shows the same answer over HTTP/2, where the
    alternative was a stream reset. What the original test still pins is the boundary of the class:
    a `418` returned on a WS route stays itself, while
    `every_1xx_from_a_handler_is_refused_not_only_101` pins the widened half (`103` and `199`
    answer `500`). Both directions were falsified: removing the core check puts
    `left: 101, right: 500` back, and narrowing the check to `status == 101` makes
    `a_101_without_a_handoff_is_never_served` fail `left: 103, right: 500` (measured on the
    track-89 tree). Over h2c the arm remains unreachable for a WebSocket route (refused
    `400` before the chain). The wider `1xx` question — whether a middleware may generate an
    informational response at all — was registered as A6 in
    `.project-governance/notes/2026-10-21-improvement-register.md`; track 89's answer is that it
    may not, on either protocol or for either author.
    A second, independent limit: middleware cannot grant or revoke an upgrade by mutating headers,
    because there is no mutable header store on `RequestContext` at all — one store, read through
    `header`/`header_values`/`headers`, and the handshake validator is fed from that same store. A
    refusal has to be a returned response, and
    `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` pins both directions. PROB-0005 was
    closed in 0.2 (2026-11-06) by removing the public map, not by making the two stores
    write-through.
    Making a `101` an ordinary `Response` (and the session an ordinary handler that owns it) is
    the shape Axum uses; here it would move socket handover and the memory re-pricing into the
    response path, which is a redesign of `ServeOutcome`, not a patch.
    Related, and still open: the upgraded session is not wrapped by anything after handover (item 9).
    The two pre-chain refusals, on the other hand, are now settled by wire tests rather than by
    reading (2026-10-22): a WS route reached over HTTP/2 answers `400` before middleware and a
    non-`GET` on a WS route answers `405`, both with a
    middleware counter pinned at `0`
    (`a_websocket_route_reached_over_h2_answers_400`, `a_websocket_route_reached_over_h2c_answers_400`,
    `a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain`). Both refusals were
    falsified by disabling their gate; the counter is what fails, not the status. The re-run also
    corrected **which** component answers the `405`: `app.ws` registers `GET` only, so the router's
    `MethodNotAllowed` arm serves it and the WebSocket branch's own method gate is unreachable through
    the public API (it is kept as defence in depth, commented where it sits). `allows_upgrade`
    is a *transport capability* flag, not a per-request header scan, so a well-formed-looking plain
    `GET` on a WS route does run the chain and the handshake failure (400, or 426 for an unsupported
    version) is decided by the validator afterwards.
