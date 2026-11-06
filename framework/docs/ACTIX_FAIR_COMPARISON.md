<!-- cite-check: snapshot 2026-10-18 -->
# Aether vs Actix Web — fair-comparison re-benchmark and robustness audit (2026-10-18)

This document supersedes the earlier snapshot-based comparison. It re-runs everything
under unified conditions after fixing the methodology gaps, and records the serve-path
robustness audit with confirmed/possible classification per finding. Raw repeat numbers
are kept verbatim from the measured runs; nothing was averaged in advance.

## 1. Unified conditions (Release vs Release)

- `threekway` (user site on Aether): `cargo build --release` (workspace release profile,
  fat LTO), run via `AETHER_PORT=3002` on `0.0.0.0:3002`.
- `actix_cmp` (Actix Web 4.17): `cargo build --release` (`lto = "fat"`, `codegen-units = 1`),
  bound `0.0.0.0:3001`, default worker count.
- Load client: `loadgen.py` (Python, one keep-alive `HTTPConnection` per thread), identical
  for both servers; 15 s per cell, 3 repeats; response bytes integrity checked per response
  (`mismatch=0` everywhere).

## 2. Route-equivalence finding

`/learning` in `threekway` contains **no SQLite query** — it is a Tera render of
`learning.html` + `base.html` with a single context variable (`active_page = "learning"`),
Tera built once at startup, autoescape default, no custom filters, no session check,
no compression (disabled on the site). The Actix replica renders the same template
directory with the same Tera version and returns the same header set.

Verification: `GET /learning` bodies from `:3002` and `:3001` are SHA-256 identical
(`e9e2e87b5fddb39f0ad7f747818bc3c9ed389daaa599a50bc9203f87a16de8d4`, 205,267 bytes);
CSS is `include_bytes!` of the same 45,832-byte file, also hash-identical.

## 3. Repeated matrix (median of 3, range in parentheses)

| Path | conns | Aether rps | Actix rps | Aether p50/p95/p99 (µs) | Actix p50/p95/p99 (µs) |
|---|---|---|---|---|---|
| css | 1 | 2282 (2208–3236) | 2658 (2490–2746) | 400/708/1119 | 337/682/1002 |
| css | 8 | 3456 (3129–3561) | 3249 (2872–3496) | 2085/4355/6390 | 2219/4658/6634 |
| css | 16 | 3496 (3425–3507) | 2744 (2674–3221) | 4086/8969/11865 | 4504/9400/13127 |
| /learning | 1 | 1388 (1367–1397) | 1412 (1345–1426) | 637/1314/2219 | 579/1397/2186 |
| /learning | 8 | 1984 (1849–2190) | 1949 (1837–1972) | 3706/6934/8616 | 3708/6986/8763 |
| /learning | 16 | 1667 (1625–1813) | 1817 (1447–1920) | 8235/15105/20216 | 8099/16577/25271 |

Errors: `err=0` and HTTP 200 only, in every one of the 36 cells.
CPU (cores consumed by the loaded server, /learning @8): Aether ≈1.35, Actix ≈1.26.
RAM (working set under /learning @16 load): Aether ≈20.1 MB avg / 21.2 max; Actix ≈16.6–17.1 avg / 17.8 max. Idle: Aether 13.6 MB, Actix 12.9 MB.

**Conclusion:** with identical release builds, identical route work and identical client,
the two servers are statistically equivalent (±5–10 % run-to-run noise dominates the
difference). The earlier "Actix +40 %" and "Aether wins @16" claims were both artifacts
of mixed build profiles and of the snapshot-vs-render shortcut; they are retracted.

## 4. Step-load ladder (/learning, 10 s/step, rps)

| conns | 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32 |
|---|---|---|---|---|---|---|---|---|
| Aether | 1604 | 2295 | 2147 | 2064 | 1992 | 1989 | 1863 | 1901 |
| Actix | 1310 | 2160 | 1987 | 1847 | 1937 | 1816 | 1843 | 1893 |
| Aether p50 (µs) | 468 | 717 | 1635 | 3620 | 5658 | 7694 | 12372 | 16062 |
| Aether CPU (cores) | 0.54 | 0.85 | 1.22 | 1.28 | 1.30 | 1.45 | 1.41 | 1.28 |

Both saturate at ~1.2–1.45 cores at ~1.9–2.0 k rps for a 205 KB Tera-rendered page
(~0.65 ms server CPU per request); beyond saturation only latency grows, throughput is
flat and failures stay zero up to 32 keep-alive connections. The cap is the workload +
the shared laptop (load generator runs on the same 4 cores), not the Aether connection
engine: static 45 KB files reach 3.5 k rps at ~0.57 cores.

## 5. Soak (180 s @12 connections, /learning)

| | requests | rps | errors | CPU cores | RAM avg/max | RAM after idle |
|---|---|---|---|---|---|---|
| Aether | 317,319 | 1763 | 0 | 1.37 | 19.3/20.4 MB | 13.6 MB |
| Actix | 334,983 | 1861 | 0 | 1.28 | 16.9/17.8 MB | 12.9 MB |

No throughput decay, no growth trend, working set returns to baseline after load →
no leak detected in 3 minutes of sustained load (limitation: 3 min is not hours).

## 6. Serve-path robustness audit (findings and status)

The Finding and Evidence columns are the 2026-10-18 audit record and are kept verbatim — including
the `app.rs`/`response.rs`/`scheduling.rs` coordinates inside them, which are as-of-then and several
of which no longer point at the construct named. The
Classification and Status columns were re-checked against the code on 2026-10-23, and every
row that has since been closed says so with the construct that carries the behaviour and the test that
now covers it. F4, F5, F6 and F7 were open on 2026-10-18 and are closed now.
The Status columns are re-checked again on 2026-10-25, and this is what that pass found: six of the
`app.rs` coordinates they carried (`1083-1088` excepted) had stopped pointing at what they name — the
rot is the track-24→62 insertions, not a wrong re-check on the 23rd — so each is now anchored on the
function or `match` that implements the claim, with the 2026-10-25 line numbers beside it. A bare
coordinate in this table is a claim that breaks every time `app.rs` grows; a symbol plus its
measurement date is a claim that says when it was true (**register B9/B16 rule**).

| # | Finding | Classification | Evidence | Status / fix |
|---|---|---|---|---|
| F1 | Error-path closes (408/431/400/503) `return` without graceful FIN; Windows RST discards the just-written response when request bytes are still unread in the receive buffer | **Confirmed (live)** | `rst_probe.py` on release :3002: `WinError 10054` for both 431 and 400 cases; Actix delivered its errors. After fix: probe returns `HTTP/1.1 431…` / `HTTP/1.1 400…` | Fixed in `aether/src/app.rs`: `stream.shutdown().await` after 408, parse-error 400/413/431, `Connection: close` keep-alive exit, and WS-handshake write failure. Regression test `error_responses_survive_unread_pipelined_bytes` in `tests/http_e2e.rs` (`:1527`, re-read on the post-`fmt` tree 2026-10-26). Full suite green at the time (259 tests on 2026-10-18; 474 on the 2026-10-23 tree) |
| F2 | Error-response writes were un-bounded awaits (no write timeout) — a never-reading client pins the task+permit | **Confirmed at code level** (pre-fix) | old `app.rs:1336/1350`: bare `write_all().await` | Fixed together with F1: writes wrapped in `tokio::time::timeout(write_timeout, …)`. Runtime torture test (stalled client) not executed — preventive fix |
| F3 | Mid-response IO error treated as success: `timeout(...).is_err()` ignores `Ok(Err(io))`, keep-alive loop continues on a dead socket | **Confirmed at code level** | `app.rs:1311` pattern vs correct `map_or(true, …)` at `app.rs:1170` | Fixed to `map_or(true, \|r\| r.is_err())`. Wire-level repro test not executed |
| F4 | `SchedulingLayer::admit` not cancellation-safe: if the awaiting future is dropped at the internal `timeout(...).await` (task abort), the queued job is never cancelled → permanent `queued` inflation | was **Possible** (code path traced; not reproduced live); **closed and tested in current code (re-checked 2026-10-23)** | `aether/src/scheduling.rs:240–291` (`SchedulingLayer::admit`, re-read on the track-74 tree 2026-10-26); serve path does not wrap `admit` in a select/timeout, so exposure is limited to shutdown-abort scenarios | The proposed guard exists: `QueueToken` (`framework/aether/src/scheduling.rs:397-401`, re-read on the track-74 tree 2026-10-26, which is the tree that adds `EXTENSION_ENTRY_MEMORY` (+18 lines above this point, so every earlier citation of this file moves)) is armed immediately after engine admission (:252-255), covers exactly the window before an `AdmissionGuard` is created, and its `Drop` releases the queued job once (`:409-415`); every normal exit disarms it (`:258`, `:271`, `:275`, `:282`, `:287`). Required test is now present: `aborting_a_queued_admit_leaves_no_stranded_job` (`framework/tests/tests/scheduling.rs:284`) saturates the single active-handler slot, parks a second arrival in the queue, aborts the waiting task, and asserts `queue_diag().queued` returns to 0 |
| F5 | `static_file` route serves via `std::fs::read` on the async worker (blocking syscall) | was **Confirmed at code level**, **no impact on the user site** (it used `static_asset`/`from_static`; `static_dir` used `tokio::fs`); **closed in current code (re-checked 2026-10-23)** | `framework/http/src/response.rs:76`, reached from `app.rs:678` | `static_file` is now an async route that reads through `tokio::fs::read` on every request, so disk latency never occupies a runtime worker (`AetherApp::static_file` → `get_async` → `tokio::fs::read`, at `framework/aether/src/app.rs:1162-1167` as measured 2026-10-26 on the post-`fmt` tree); `static_dir` already used `tokio::fs` (`framework/aether/src/static_dir.rs:30-38`). Neither `spawn_blocking` nor a separate `file_async` constructor was needed. Behaviour test: `static_file_serves_disk_content_over_async_route` (`framework/tests/tests/http_e2e.rs:725`); the slow/NFS/AV-locked-disk latency impact was never measured and remains **possible** |
| F6 | Keep-alive conformance: no max-requests-per-connection; HTTP/1.0 requests kept alive without `Connection: keep-alive`; multi-value `Connection: keep-alive, close` missed; `keep_alive_timeout` (60 s) used only for shutdown drain (doc at `app.rs:855`), idle close is actually `read_timeout` 5 s | was **Confirmed (live for HTTP/1.0)** on this 2026-10-18 tree; **three of the four sub-claims are closed in current code (re-checked 2026-10-23)**, only the `max_requests_per_connection` cap is still absent | raw-socket probe: HTTP/1.0 request → socket still open 3 s later (RFC 9112 requires close); grep shows `keep_alive_timeout` referenced only at `app.rs:1425` | Current code: `close_after_response` is derived per `Version` from the `close`/`keep-alive` tokens (the `match version` in `serve.rs`, at `framework/aether/src/serve.rs:158-169` as measured 2026-10-26 on the post-`fmt` tree — the row's earlier `serve.rs:134-145` has been dead for a while and names a different construct now); the frontend then states the socket's fate — `Connection: close` when closing, `Connection: keep-alive` for a persisted 1.0 exchange (`app.rs:1894-1905`, the `if close_after_response` block that runs only when the handler wrote no `Connection` header); `Connection` is tokenised on commas, so a multi-value line cannot hide a token (`connection_has_token`, `app.rs:2651-2663`, unit-tested as `connection_tokens_are_read_per_token_not_per_line`, `app.rs:3006`); and `keep_alive_timeout` is now the between-request idle deadline while `read_timeout` still bounds a half-received head (`app.rs:2224-2229`, the `read_limit` selection). Wire tests: `http10_requests_close_after_one_response_by_default` (`framework/tests/tests/http_e2e.rs:770`), `http10_explicit_keepalive_persists_and_is_advertised` (`:826`), both green in the 2026-10-23 `cargo test --workspace` run. Still open by design choice: no max-requests-per-connection cap — a missing operator knob, not a conformance break |
| F7 | WebSocket session has no recv timeout; stalled peer pins the connection task until shutdown | was **Possible** (code-level, `connection.rs:305–307`); the recv half **closed 2026-10-23**, the send half **closed 2026-10-26 (PROB-0030)** | not exercised live; PROB-0030's wire probe did (`a_websocket_peer_that_stops_reading_loses_its_buffer_registration`) | `WebSocketLimits::read_timeout` (`framework/websocket/src/connection.rs:31-34`, 120 s default at `:49`) bounds every socket wait inside `recv` and returns `WsError::IdleTimeout` (`:168-172`); the app hands its limits to each upgraded session (`WebSocket::from_parts(stream, initial, core.websocket_limits)`, `framework/aether/src/app.rs:2054-2058` as measured 2026-10-26 on the post-`fmt` tree) and operators retune them via `AetherApp::with_websocket_limits` (`app.rs:1229-1232`). Tests added 2026-10-23: `silent_peer_is_failed_by_the_idle_deadline` (`connection.rs:721`) and `a_ping_resets_the_idle_deadline` (`:734`, proves the deadline is per read, so a heartbeat keeps a session alive). **That was only half the stall.** A peer that reads fine but stops being read from never enters `recv`, so no read deadline could ever fire: `write_frame` awaited `write_all` + `flush` with no bound and parked the session, its slot and its 33,570,816 B of re-priced buffers. `WebSocketLimits::write_timeout` (`connection.rs:35-39`, 10 s default at `:54`, `None` disables) now bounds each interval of an unsent write and returns `WsError::WriteTimeout`; like `read_timeout` it bounds an interval, not a frame, and renews only after `MIN_PROGRESS_FOR_RENEWAL` (1024 B, `:452`) has actually left, so a slow-but-consuming peer finishes its frame and a trickle does not buy a permanent session (`a_peer_that_reads_slowly_gets_the_whole_frame`, `:902`). Tests: `a_peer_that_stops_consuming_costs_the_session_one_write_interval` (`:862`), `a_peer_that_trickles_one_byte_per_interval_is_shed` (`:959`), and the wire-level release of the registration. The 2026-10-18 coordinate `connection.rs:305-307` no longer describes a recv path — it is the tail of `accept_data_frame` and the `complete_message` header |

Checked and found correct (no action): request-parser smuggling defenses (TE.CL/CL.TE,
duplicate TE, chunk bounds), memory bounds on `conn_buf`/chunked bodies, slow-loris
deadlines (per-read + whole-request), `AdmissionGuard` RAII release on all exits,
fail-open on mutex poisoning, no network-reachable `unwrap/expect`, shutdown drain
task tracking, HEAD semantics, pipelined-buffer accounting.

## 7. Retained previous results (for the record — now invalidated for comparison use)

Debug Aether (:3000) vs Debug Actix snapshot (:3001): css 923/1055, 1564/2191, 2006/1907 rps;
learning 356/620, 964/928, 1082/1060 rps (1/8/16 conns). Release Actix snapshot vs Debug
Aether: css 1597/1692/2059, learning 1073/1288/1447. Flask vs debug Aether: Aether ≈7–10×
Flask throughput (Flask GIL-capped ≈200 rps), 23 MB vs 45 MB RAM. All of these mixed build
profiles and/or compared snapshot-serving against live rendering; §3 replaces them.
