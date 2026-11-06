# Working Context

## Current Goal
Execute REQ-0001: full Aether re-engineering mission (10 phases per vision doc attached 2026-10-18).

## Active Track
Adaptive Tree Scheduling phase (user-requested 2026-10-18). Delivered and verified
(the per-target counts in the bullets below are that delivery's reading, not the current tree's — the
current per-target table is `framework/docs/BASELINE_MEASUREMENTS.md` (2026-10-31, track-108 tree):
- `aether-scheduler` crate: protocol-independent engine (admission, bounded queues,
  three-class tree, baseline from measurements excluding large jobs, quota slicing,
  starvation guard, adaptive per-kind capacity; FIFO/RR/Deficit/AdaptiveTree policies)
  — 17 engine + 8 simulator-invariant + 2 stress tests
- Opt-in serve-path integration (`AetherApp::with_scheduling`): connection/handler
  admission, 503 + Retry-After shedding, RAII release on panic/abort/disconnect,
  `run_slices` divisible driver — 9 wire tests; default path unchanged
- Windows RST fix on the scheduling reject path (graceful `shutdown()` before close)
- Design doc `framework/docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md`; measured four-policy
  comparison `framework/docs/SCHEDULING_COMPARISON.md` (real simulator output, including
  the large-heavy scenario where RoundRobin beats AdaptiveTree); example
  `framework/examples/scheduling_demo.rs` run live (20 000-unit job sliced to completion
  while HTTP traffic kept flowing)

Workspace at the time of that delivery: 328 tests, 0 failures, 1 ignored (46 suites; was 258).
Release build clean. (Current tree, 2026-10-26, after track 75: **527 tests, 0 failures, 1 ignored**
— 305 unit + 217 integration + 5 doc, 50 result lines; track 60 measured 488 and track 61 added no
tests, track 62's three `app.rs` worker-count tests took it to 491, track 65's one `app.rs`
bind-advice test took it to 492, track 67's one `http2_e2e` wire test (the paced-reader pair to
the trickle test) took it to 493, track 68's four `http2.rs` unit tests — three body shapes for
`price_response_body` and one refused-upload registration case — took it to 497, track 69's six —
three `transport.rs` classifier tests, two `trie.rs` catch-all tests, one `http_e2e` HTTP/1.0-upgrade
wire test — took it to 503, track 70's three `http_e2e`/`scheduling` wire tests for delivering a
buffer-limit 413 took it to 506, track 71's three — two `trie.rs` unit tests and one `extractors`
wire test pinning that a shared radix `Param` level reports each sibling route's own capture name —
took it to 509, and track 72's four — three `aether-websocket` unit tests and one `scheduling` wire
test holding a session whose peer stops consuming, which is what releases its slot and its 33,570,816 B
buffer registration — took it to 513; track 73 added none (its work was lint-shaped rewrites and
markdown, and no markdown in this tree is a build input, so 513 stood across its three slices), and track
74's thirteen — nine `extensions.rs` unit tests for the priced map, one `http2.rs` unit test that calls
every body producer this framework has, two `extensions_e2e` wire tests that park a live request and
read its registration back (a declared payload charged in full and released with the request; an
undeclared one charged container-only and still served), and one `http2_e2e` wire test for the same over
h2c — took it to 526, and track 75's one — a `http_e2e` wire test that uploads a 1-byte-chunk body
to one payload byte below the connection's wire budget and one byte past it, pinning 350,879 served
and 350,880 refused — took it to 527; track 81 added none (it repaired two racing baselines in
`http2_e2e`, so the count is unchanged while the tree changed), and track 80 added none (markdown, one
Python checker and its unittests, none of which `cargo test --workspace` builds), and track 79 added
none either (markdown, four raw logs, a `.txt` header and one Python checker with its unittests, none
of which the Rust suite builds), so 527 stood.
The per-track history that follows is a
reading of each day's tree, not a running sum: the 2026-10-21 tree measured 455, §14's behaviour
tests took it to 458, track 23's two HTTP/2 configuration tests plus one derivation unit test took
it to 462 and track 27's capacity-pricing tests — three `app.rs` unit, one `scheduling` wire — took
it to 466; track 28 changed markdown only, and no markdown in this tree is a build input, so 466
stood; track 29's two `aether-websocket` unit tests for the idle deadline took it to 468; track 30
changed markdown only again, so 468 stood; track 31's six — three `app.rs` write-charge unit tests
plus one wire test each in `scheduling`, `http2_e2e` and `http_e2e` — took it to 474; track 43's
one — the
`http_e2e` wire test for the `IntoResponseError` status table — took it to 475; the track-45 header-read
slice's one `http_e2e` test (compression gating on `Accept-Encoding`) took it to 476; track 46's three
parser slot-estimate tests took it to 479; track 50's four `http2_e2e` response-write tests took it
to 483, and its fifth, an `http2.rs` unit test that drives `serve` with no listener in the path,
took it to 484 on that day's full working tree (the slice-only export measured 482). The counts were
then re-read per tree: 483 with the allocation slice, 486 with the parser slice, 487 with track 52's
trickle test, and 488 with track 58's HTTP/2 test.
See the track-20 entry under Last Update;
the per-stage numbers live in `framework/docs/FEATURE_COMPLETENESS_MATRIX.md`.)

Previous track (Developer Experience, delivered 2026-10-18): `TestClient`, typed
extractors, `use_middleware`.

## Known Breaking Change (documented)
`get_async`/`route_async`/group `*_async` now take `Handler<Args>`: closures taking
`RequestContext` directly must annotate: `|req: RequestContext| async { .. }`. Sync
`get`/`post`/... unchanged.
**Still unfixed downstream (verified 2026-10-20):** `../aether-probe/src/main.rs:98` passes an
unannotated `|_|` to `get_async` and fails `cargo check` with E0283 — pre-existing, not caused
by track 13 (which adds nothing to the public API). `../aether-bench-compare` checks clean.

`WebSocket<tokio::net::TcpStream>` → `WebSocket<aether::transport::Stream>` (track 14): a second
breaking signature, because the transport an upgraded session runs on is no longer always a raw
TCP socket. Only embedders that *annotate* the type are affected; nothing in the workspace
annotates it, `../aether-probe` fails with the same single pre-existing E0283 as before and no
new error, and `../aether-bench-compare` still checks clean against the changed `aether`.
`Stream` is re-exported at the crate root and in `prelude`, so the fix is one type name.

## Active Track 2 — Scheduling performance campaign (2026-10-19 → 2026-10-20)
Goal: find where Aether's measured cost vs Actix/axum comes from, one variable at a time.
Delivered: (1) connection-memory cliff fixed — per-socket reservation is re-priced live
instead of reserving `max_body` for the whole keep-alive lifetime, which had stalled
admission near 127 connections; (2) **rotation-index fix** — `JobTree` held every job kind,
so `pump()` was O(live connections) under the engine lock (9.20× cost growth 32→256 conns,
−55.6 % throughput at 256); now divisible-jobs only: slope 1.11×, `pump only`
10.77 µs → 74 ns. Anchor: `framework/docs/perf-baseline/BASELINE-2026-10-20-SCHED-ROTATION.md`;
(3) **Mutex-as-bottleneck hypothesis REFUTED** by a bench-side WAIT/HOLD split
(`aether-bench-compare/results/lock_split/SUMMARY.md`): WAIT 17–34 ns at the server's
operating rate, HOLD 162→315 ns/acq from 1→8 threads, mutex held ≈2.2 % of wall clock —
therefore do **not** split, shard or replace `Mutex`/`Notify`; the only lever left is the
work `admit()` does inside the section. Instruments: `scheduler/examples/pump_cost.rs`,
`scheduler/examples/active_trace.rs` (main-scene decisions byte-identical before/after).
Not proven: attribution of the residual scheduled-vs-plain gap at 256 connections (8 reps,
sign test p ≈ 0.29) and whether a Tokio worker is descheduled while holding the engine lock.

## Active Track 3 — Header materialisation (2026-10-20)
The L0→L6 ladder found the first layer where allocations rise is **header materialisation**
(22.00 allocs/req, 56 % of all framework-path allocations). `RequestContext` owned two copies
of every field line — the public `headers: HashMap<String, String>` plus the private
`header_multi: HashMap<String, Vec<String>>` — and the public map deduplicated by *wire
casing*, so `X-Dup` and `x-dup` could both appear in it while `header()` reported only the
first line, and its doc comment claimed "last-value-wins", which the code never did.
Delivered (public API and HTTP semantics preserved): vacancy of the public map is now decided
on the lowercased key, so it can never contradict `header()`; `header_multi` values are
`SmallVec<[String; 2]>`, removing the per-field `Vec` allocation; `header()` reads the store
directly instead of building a `Vec<&str>` just to take element 0; lowercase query names skip
the `to_ascii_lowercase()` allocation; the stale docs in `app.rs`, `docs/05`,
`docs/09_api_reference.md`, `FEATURE_COMPLETENESS_MATRIX.md` and `PRODUCTION_READINESS_REPORT.md`
are corrected. Tests: 3 new E2E (mixed-case dedup vs `header()`, repeated `Connection` lines
still signal close, CORS preflight first-line rule) + 1 unit (lines past the inline capacity).
Suite: `cargo test --workspace` 332 passed / 0 failed / 1 ignored; release build clean.
Measured: ladder L2 rung 23.01 → 19.03 allocs/req (−4.00, spread 0.00); production binary A/B
at 8 connections 52.55 → 46.55 allocs/req (−6.00) and −96 B/req; at 128 connections the same
difference reads −24.00/−384 because the counter is per-request but the surplus scales with
connection count. CPU/req and rps unchanged within noise.
Not done (owner decision): collapsing the two stores into one would require replacing the
public `pub headers` field with an accessor — breaking for downstream readers.

## Active Track 5 — Simulation honesty (2026-10-20, commit da2b547)
`SimResult.throughput_work_per_s` was generated workload ÷ makespan while
`SCHEDULING_COMPARISON.md` called it "work units finished", and the invariant test
guarding it could not fail (it recomputed the same product, and skipped its body whenever
any policy rejected — which every 200-job scenario does). Throughput is now derived from
engine `work_executed` (`SimResult.served_work`, also printed as its own column), the test
asserts its no-shed precondition instead of assuming it, a second test pins that shed work
is never counted as served, and `SimResult.scenario` — hardcoded `""` — is set from the
`Scenario` now passed to `run()`. **API impact:** `run()` gained a parameter (bench
harness only: 10 call sites, no serve-path or downstream use); the published tables are
annotated as historical instead of regenerated, because re-sweeping is the campaign the
owner paused.

## Active Track 4 — Scheduler batch E correctness (2026-10-20)
Four defects in `framework/scheduler`, all verified against code first, none on the
bench hot path by default (`AdaptiveTree` is the shipped policy):
1. **Deficit round robin re-billed its own credit.** `on_slice` did `deficit += granted
   − used` while `granted` already contained the banked deficit, so a job that once
   under-used a quantum got `quantum + owed` every round forever and the debt was never
   repaid — the advertised algorithm was not deficit round robin. `on_slice` now sets the
   bank to the unused part of the grant just made and clears it when the credit is spent
   (`policy.rs`, regression `deficit_round_robin_repays_the_banked_credit_instead_of_re_billing_it`).
   `SCHEDULING_COMPARISON.md` §"what the data shows" item 4 and `ADAPTIVE_TREE_SCHEDULING_DESIGN.md` §5
   corrected; the published tables stand unchanged, because that generator always consumes
   its full grant, which is exactly why the bug never showed there.
2. **Per-job policy state leaked on cancel.** The trait had no cancel hook and
   `finish()` only notified on completion, while cancel is the *most common* terminal
   transition (disconnect, timeout, shutdown drain). Added `SchedulingPolicy::on_cancel`
   (default no-op, so no existing implementation breaks), wired it into `finish()`, and
   `DeficitRoundRobin` now drops its entry there. Regression pins both that every terminal
   transition notifies the policy once and that a repeat call on a terminal job does not
   double-release.
3. **Memory shedding never reached the adaptive signal.** `rejected_in_window` is the only
   rejection term of pressure, and only queue-full rejections entered it: over-ceiling
   arrivals bumped `rejected_memory` alone and a refused `adjust_memory` grow bumped
   `memory_shed` alone, so a memory-bound server kept its full handler ceiling — the load
   that consumes the budget. Both now feed the window. `skipped_unfit` (ordinary slot
   backpressure, which fires constantly and is not a rejection) deliberately does not.
4. **`complete()` billed never-run work.** The public `complete()` had no state guard, so
   finishing a job still in `Queued` credited its whole declared hint into
   `work_executed` and into the baseline sample the policy learns from. Implicit remainder
   is now credited only for a job that was actually admitted; the intended
   completion-accounts-the-whole-total path is unchanged (`run_slices`/guards only ever
   complete Active jobs).
Suite: `cargo test -p aether-scheduler` 54 passed / 0 failed (6 new), `cargo test --workspace`
green, release build clean. No benchmark run: the DRR change cannot move the published sim
rows (reason above) and the memory-pressure change only acts when the budget is already
shedding work.

## Active Track 6 — `QueueToken` pin (2026-10-20, PROB-0002 closed)
The claim "wire tests assert no stranded jobs" was stronger than reality: every existing
abort test cancels an already-`Active` handler, so the RAII token that cancels a job still
sitting in the queue had no regression pin. Added
`framework/tests/tests/scheduling.rs::aborting_a_queued_admit_leaves_no_stranded_job`: one
handler guard saturates the single slot, a second `admit` is spawned and confirmed queued via
`queue_diag()`, its task is aborted, and the test asserts the queue drains to 0, `active_handlers`
stays at the one legitimately held, `cancelled == 1` (exactly once, no double-release), and the
freed slot admits a later request. To express that test at the wire layer, `NewJob` was added to
the `pub use aether_scheduler::{...}` list in `framework/aether/src/lib.rs` — additive re-export of
an already-public dependency type, no API removed or changed.
Result: the mechanism held. No production code changed; the defect was the missing pin, and the
Actix-comparison note's claim is now true.
Suite: `cargo test --workspace` 340 passed / 0 failed (1 ignored), release build clean.

## Active Track 7 — Dead-crate doc honesty (2026-10-20, PROB-0001 closed)
Verified before editing, with evidence: `aether-security`, `aether-observability`,
`aether-serialization` and `aether-openapi` have zero dependents (no member `Cargo.toml` lists
them, no `.rs` file anywhere contains `aether_security::` etc.), `sonic-rs` appears only in
`framework/Cargo.toml:39` `[workspace.dependencies]`, is referenced by no member, and does not
even reach `Cargo.lock` (zero matches); `subtle` — credited by
`AETHER_CORE_CAPABILITIES.md` for constant-time comparison — is not a dependency of anything, and
`aether-http` contains no HTTP/2 code (no `h2` dep, no frame layer). Test counts were re-measured:
`cargo test --workspace` 340 passed / 0 failed / 1 ignored, and the per-target numbers now in
`BASELINE_MEASUREMENTS.md` sum to 212 unit + 125 integration + 3 doc tests = the same 340, which is
how the stale "aether-security: 2 tests" contradiction (real number: 7) was resolved.
Doc edits: crate table in `FULL_FRAMEWORK_DOCUMENTATION_AR.md` gained an explicit unwired marker per
dead crate plus a "wiring status" note listing the eight crates `aether` actually depends on; the
§8/§10/§13/§14 examples that `use aether_security::` / `_validation::` / `_openapi::` /
`_observability::` each gained a one-line "not part of `aether`" caveat; the HTTP/2 claim and the
`sonic-rs` claim were removed; `AETHER_CORE_CAPABILITIES.md` JWT row now names the real mechanism
(`ring::hmac` + the hand-written `constant_time_eq`) and states the crate is unreachable from the
facade; `FEATURE_COMPLETENESS_MATRIX.md` gained an `UNWIRED_IMPLEMENTATION` status code and the JWT,
constant-time and OpenAPI rows were reclassified with it; `PRODUCTION_READINESS_REPORT.md` no longer
lists JWT validation as a shipped capability in either place. `FINAL_REPORT.md` keeps its stale
counts because the file is already headed **SUPERSEDED**.
Docs only — no code changed, no test or build re-run needed beyond the counts above. The "or wire
`aether-security`" branch of the fix was not silently dropped: it is recorded as PROP-0001.

## Active Track 8 — Per-method path param names (2026-10-20, PROB-0007 closed)
> The API described below is the 2026-10-20 tree's. Track 71 (2026-10-26) replaced it: that design
> fixed cross-*method* naming but still answered one *method*'s two sibling routes with the first
> name registered, which is PROB-0031. `param_names` is now `Vec<(Method, Vec<String>)>`, held on the
> node that owns the handler, and `param_name_for`/`set_param_name` are gone — see the track-71 entry.

`/users/{id}` GET + `/users/{user}` POST collapsed onto one radix node and extraction only ever
saw the first-registered name, so the POST handler's `ctx.param("user")` was `None` — a silent
wrong-answer, not an error. The kind-based child lookup was deliberate (name-matched siblings made
the second handler unreachable and `reject_duplicate` never fired) and is kept; what was wrong was
carrying one name for the whole node. `RouteNode` now holds
`param_names: Vec<(Method, String)>`, `add_route` binds the declared name to the registering method
when it reuses an existing dynamic node (`framework/router/src/trie.rs`), and `search` resolves each
Param/CatchAll key through `param_name_for(method)` with the node's own name as fallback
(`framework/router/src/node.rs`). Same-method duplicate registration still panics, and no public API
changed: `RouteNode` is not re-exported by `aether` and no code in the workspace constructs it with
an exhaustive struct literal, so the new field is additive.
Tests: the trie test that *pinned* first-name-wins was rewritten (its expectation was the defect) to
assert both directions for Param and CatchAll, plus a three-way GET/POST/DELETE independence case;
and a wire test in `framework/tests/tests/extractors.rs` proves `ctx.param()` at the `RequestContext`
layer returns each method's own name and never the other method's.
Falsifiability note: the direct "run it against pre-fix code" check was blocked by the permission
layer, so it is argued from the old data path instead — pre-fix, `POST /items/shirt` pushed only
`("id","shirt")`, so the new assertions would have produced `post-item:?:leaked` against the expected
`post-item:shirt:clean` and failed.
Suite: `cargo test -p aether-router` 20 passed (was 19), `cargo test --workspace` 342 passed / 0 failed,
release build clean. No benchmark: routing cost is a per-node `Vec` scan over the methods actually
registered on that one dynamic level (normally 1-2), and the change adds nothing to the static path.

## Active Track 9 — Scheduler extension point made implementable (2026-10-20, PROB-0003 closed)
`aether_scheduler` exported the `SchedulingPolicy` trait while `SchedulerState` — the first parameter
of every hook — stayed crate-private, so no third-party policy could be written at all (the failure
was a compile-time `E0425`, not a design limitation). Fixed by adding `SchedulerState` to the
`pub use engine::{...}` list in `framework/scheduler/src/lib.rs:29`. Compatibility: purely additive —
no signature, method or existing export changed, nothing was removed; it makes an already-public
trait's own parameter name reachable, which is the least disruptive option (the alternative was
un-exporting `SchedulingPolicy`, which would break the documented pluggable-policy design instead).
`framework/scheduler/tests/policy_extension.rs` is the proof and lives *outside* the crate on purpose:
one external policy observes `on_arrive`/`on_slice`/`on_complete`/`on_cancel` through shared counters
(2 admits, 1 completion, 1 cancellation → exactly one hook each, also re-pinning batch E's
`on_cancel` from downstream), and a second overrides the defaulted `admission_order` and
`round_budget` to show the defaults are callable on the re-exported state.
Falsifiability is structural here and needs no revert: `git show HEAD:framework/scheduler/src/lib.rs`
still lacks `SchedulerState` in the export list while the new test names it in two signatures, so the
test could only ever compile after this change.
> Corrected 2026-10-23 (register **B16**): "HEAD" was the pre-track-9 tip, and HEAD has moved — today
> `git show HEAD:framework/scheduler/src/lib.rs | grep -c SchedulerState` → **1**, because the change
> was committed as `628d214`. The sentence is reproducible only against that commit's parent:
> `git show 628d214^:framework/scheduler/src/lib.rs | grep -c SchedulerState` → **0**, and
> `framework/scheduler/tests/policy_extension.rs` does not exist there at all. A command that resolves
> through `HEAD` cannot be evidence about a past state.
Suite: `cargo test --workspace` 344 passed / 0 failed, release build clean.

## Active Track 10 — HTTP/1.x protocol gaps closed (2026-10-20, PROB-0006 partially closed)
Re-triaged the four items PROB-0006 recorded before touching anything. Two were already done
and test-pinned (1.0 close-by-default + keep-alive echo, chunked-on-1.0 rejection:
`framework/aether/src/app.rs` version decision and `http_e2e.rs::http10_*`/`http10_chunked_request_is_rejected`),
one was a default-tuning choice (`max_headers_count` 64 vs hyper's 100 — enforced, configurable
and tested already, so it stays a decision), and two were genuine: no `Expect: 100-continue`
anywhere in the tree, and chunked bodies re-decoded from byte zero after every read.

**100-continue.** `parser.rs::awaits_100_continue(buf, limits)` answers from the head alone:
head complete, `version_minor >= 1`, and an `expect` field whose comma-separated token list
contains exactly `100-continue` (a token that merely contains it — `100-continue-x` — is a
different expectation and earns nothing). The connection loop calls it only in the
`Ok(None)` branch, i.e. after a complete head with the body still pending, behind a
per-request `continue_sent` flag, and writes `HTTP/1.1 100 Continue\r\n\r\n` with no headers.
A whole request in one read never reaches that branch, so it never gets an interim response.
Falsifiability by derivation: pre-change a head-only `Expect` request sat in the read timeout
and the wire test's first `response()` would have hit its 5 s timeout; without the per-request
reset the second pipelined `Expect` on the same connection would have got no interim response.

**Chunked decode is now resumable.** The old `decode_chunked(data, limits)` re-scanned the whole
body-wire after every read and threw the partially decoded `Vec` away: with the default 2 MiB
body limit and 8 KiB reads that is ~256 re-decodes ≈ 256 MiB of copying per upload request,
an ~130x CPU amplification a client pays nothing for. Replaced by `ChunkedBodyDecoder`, which
keeps the validated offset, the decoded `Vec`, the current chunk's remaining bytes and the
trailer budgets, so each wire byte is validated once. `parse_request_incremental` is the
connection-loop entry point; `parse_request` keeps its exact signature as a whole-buffer
wrapper over it, so the one implementation stays shared with the fuzz target, the legacy
`RequestView::parse` and every existing test. The body budget is now checked on the chunk
*declaration* rather than after its data lands (same error, less work).
`framework/aether/src/app.rs` holds one decoder per connection. The post-read reprice charges
the decoded copy (`decoder.body_bytes()`) plus the read buffer's **reserved block**
(`conn_buf.capacity()` minus the 8 KiB every connection starts with, which `CONN_BASE_MEMORY`
already covers), and releases an emptied oversized block when the request finishes. Fixed 2026-10-23
under PROB-0008: the charge used to be `conn_buf.len()`, so a body fed in 8 KiB reads sat in a
block up to nearly 2x larger and was under-priced by that margin. The wire copy and the decoded
copy are both live until the request is served, so the decoded prefix must not be invisible to
the memory budget. Peak memory during a chunked upload is unchanged (wire + decoded); that
earlier fix was CPU and allocation count, not the 2x.

**Bug the new tests caught, not the old ones:** the first version treated `chunk_remaining == 0`
as "between chunks", so a read boundary landing on a chunk's trailing CRLF made the CRLF be
parsed as an empty — illegal — size line, and every split body failed with 400. Fixed with an
explicit `in_chunk` state. A second defect: a completed decoder kept `complete == true`, so
re-parsing the same buffer short-circuited and returned an *empty* body; fixed by resetting the
decoder inside `parse_request_incremental` when the body is handed over, which also makes the
stale-offset case (a pipelined request consumed and the buffer rebased) self-correcting.

Tests: 2 parser Expect + 4 parser incremental (resume at slice sizes 1/2/3/7/13/64/4096,
monotonic held progress with no early serve, two chunked requests through one decoder, a
remembered offset past a deliberately shortened buffer) +
3 `http_e2e` wire (interim-before-body with a second keep-alive `Expect`, none for 1.0 or a
whole request, chunked split across nine writes then reused and pipelined). Suite:
`cargo test --workspace` **353 passed / 0 failed** (was 344), release build clean, no benchmark run.

Docs corrected against measurement: the `FEATURE_COMPLETENESS_MATRIX.md` HTTP/1.0 row claimed
"1.0 close-by-default semantics are not applied" (wrong since the version handling landed) and
its test counts were stale; the always-`HTTP/1.1` status line is now recorded as the actual
deviation — permitted, because 1.1 is the greatest version this server supports (RFC 9110 §9.6),
but it does not echo the request version, which stays an owner decision. Baseline per-target
counts re-measured (219 unit / 131 integration / 3 doc). New registry item **PROB-0008** records
a gap found while wiring the charge: memory is priced on buffer *length* while the allocator
holds *capacity*, which doubles as a body grows, so a large-body connection is under-priced by
up to roughly 2x.

## Track 11 — rate limiting + Host validation wired into `AetherApp` (2026-10-20, delivered)
Both started as library code and are now reachable opt-in features. `framework/middleware/src/rate_limit.rs`
is a per-client token bucket (`RateLimiter::check(IpAddr, Instant)`) that refills in whole periods,
advances a slot's `last` by exactly `granted × per` so fractional time is not lost, does not bank
idle time, sweeps dead clients at `max_clients` and fails *open* (counted) rather than refusing all
traffic when its map is still full; 10 tests. `host_validation.rs` normalises a `Host` value
(lowercase, one trailing root dot, port stripped, `@`/`?`/`#`/`://`/slash and bracketless multi-colon
authorities refused) and matches an allow-list where `*.example.com` means sub-at-any-depth and never
the apex; a rejection is a 400 with a fixed body — the offending host is never reflected. 7 tests.
Two real bugs those tests caught before any wiring: a config entry carrying a scheme
(`http://good.com`) was silently accepted, and `*.` was accepted as a pattern no client can present.

**Wiring.** `AetherApp::enable_host_validation(&[…])` (panics on an unparsable entry, so a typo in a
security allow-list cannot degrade into "protects less than intended") and
`AetherApp::enable_rate_limit(RateLimitConfig)`, with `AetherApp::rate_limiter()` handing out an
`Arc<RateLimiter>` *before* `serve` moves the app so a metrics route can read the counters afterwards.
`RequestContext` gained a **private** `peer_addr` and a `pub fn peer_addr()` accessor: the struct
already had private fields, so no external caller could construct it with a literal, which is why
this is non-breaking where a public field would have been breaking. The decision runs after the head
parses and before routing, so a refused request never reaches a handler, a static file, the WebSocket
upgrade path, or the router's 404/405 work; Host is checked first because it is a header comparison
while the limiter takes a lock. A 429 carries a rounded-up, never-zero `Retry-After` and keeps the
connection open. Both default to off, so the recorded benchmark path is untouched.
Known limit, documented on the accessor: the key is the *transport* peer, so behind a proxy every
end client shares one bucket — signing a forwarded identity is not implemented.

Tests: 4 new raw-socket wire tests (off-list host → 400 with the handler counter still at zero and the
host absent from the body, on-list host → 200; burst 2 → third request 429 + `Retry-After ≥ 1` and a
fourth on the same connection still answered; counters == 2 refusals for 3 requests at burst 1;
`peer_addr()` returns the accepted loopback peer). Suite: `cargo test --workspace` **376 passed /
0 failed / 1 ignored** (31 in `http_e2e`), release build clean with 0 warnings, no benchmark run.
The FEATURE matrix rate-limiting and Host-validation rows moved to VERIFIED_IMPLEMENTED and the
PRODUCTION_READINESS status table was recounted mechanically from the matrix column (86 verified /
11 partial / 24 not implemented / 3 unwired / 2 not applicable) — its previous 78/12/30 snapshot had
already drifted, including a `UNWIRED_IMPLEMENTATION` status it did not list at all.

## Active Track 12 — PROB-0006 closed (2026-10-20)
The last two items Track 10 left. **Status-line version echo:** `Response` is an all-public-field
struct, so adding a `version` field would break every construction site; the version is instead a
parameter of serialisation — `write_http1_to_for_request_version(out, minor, include_body)` with
the old `write_http1_to`/`write_http1_to_with_body` kept as thin 1.1 wrappers, and anything other
than minor 0 or 1 clamped to 1.1 rather than emitting a token no client parses. Wiring is at the
two sites where the request actually parsed: the main response (the `ServeOutcome::Response` arm
that calls `response.write_http1_to_for_request_version`, body suppressed for HEAD) and the
WebSocket memory-reprice 503 (the `conn_buffer_charge` rejection inside the upgrade branch, whose
`Response::status(503, …)` goes through the same call). Coordinates are deliberately not recorded
here — per register B9 they rot every round; re-derive with
`grep -n write_http1_to_for_request_version framework/aether/src/app.rs`, which listed exactly
those two sites (`app.rs:1769`, `app.rs:1857`) on 2026-10-23. Sites that fire *before* a head parses
— the scheduler's pre-read 503, 408, the 431/413/400 parse failures, the buffer-limit 413, and the
`HTTP/1.1 100 Continue` interim — still claim 1.1, which is what they know.
Honesty about severity: answering an HTTP/1.0 request with `HTTP/1.1` was **not** illegal (RFC 9110
§9.6 only forbids exceeding the greatest supported version), so this is an interoperability fix —
a 1.0 client keys persistence off the version token and would hold a connection the server already
decided to close. It stays recorded as a deviation that a WebSocket `101` still claims `HTTP/1.1`,
which is unreachable for a 1.0 client in practice.
**Header budget default:** `max_headers_count` 64 → 100, hyper's `httparse` ceiling. The count limit
exists to cap per-line work amplification; `max_header_bytes` (8 KiB) is what actually bounds it, so
the old default only rejected legitimate clients (gRPC metadata, large cookie jars, JWT headers).
Falsifiable without a revert: `test_default_header_budget_is_a_hundred_field_lines` asserts the
default *is* 100 and that 101 returns 431, so pre-change it could not compile-and-pass; the e2e
harness now captures the status-line version as `HttpResponse::version`, so the three `http10_*` /
`http11_*` wire tests assert `HTTP/1.0` / `HTTP/1.1` where they previously asserted only framing,
and `http10_head_echoes_the_version_and_sends_no_body` pins the echo × HEAD interaction (version
echoed, `Content-Length: 4` kept, no payload, connection closed).
Suite: `cargo test --workspace` **372 passed / 0 failed / 1 ignored** (was 353; 87 in `aether-http`,
59 in `aether-middleware` incl. the parked modules, 27 raw-socket tests in `http_e2e`, 132
integration, 3 doc), release build clean with 0 warnings, no benchmark run.
Docs re-measured against this: FEATURE matrix header/evidence lines, its parsing + HTTP/1.0 +
header-budget rows, BASELINE per-target table and the stale "missing tests for middleware/websocket"
claim (both crates have tests), PRODUCTION_READINESS §4.1. PROB-0006 is closed; the register keeps
PROB-0004, PROB-0005, PROB-0008, PROB-0009 and PROP-0001 — all owner decisions.

## Active Track 13 — Shared serving core, pre-HTTP/2 (2026-10-20)
Owner picked the next front: **HTTP/2 + TLS on the same port**, using proven `h2` + `rustls`
(not a bespoke protocol stack). Stage 1 of 4 is the prerequisite refactor, delivered here.
`framework/aether/src/serve.rs` now owns everything protocol-independent that used to be
inlined in the HTTP/1.1 connection task: radix match (+ HEAD fallback to GET), Host-validation
and rate-limit rejections before routing, CORS preflight routability rule, handler dispatch
with scheduler admission / `request_timeout` / panic→500 / timeout→504+abort, and the shared
post-processing (`X-Request-ID`, CORS, security headers, compression, the response-derived
close decision). A `ServiceCore` is built once in `serve_with_shutdown` and cloned per
connection; the frontend builds a `RequestParts` (new `pub(crate)` type, the single shape both
protocols must agree on — same case-insensitive first-line dedup, same lowercase multi store,
same percent-decoded params) and `RequestContext::from_parts` is the only way in, with `state`
supplied by the core instead of smuggled through a thread-local.
Two behaviours moved with intent, not accident:
- **WebSocket upgrade** now surfaces as `ServeOutcome::WebSocketUpgrade { accept, ctx, handler,
  ws_memory }`. The core validates the handshake and hands the transport the handler plus the
  session's *extra* memory headroom; the frontend re-prices and only then writes the `101`,
  because after the switch there is no HTTP status left to refuse with. `reprice_conn_memory`
  gained an `extra` parameter for exactly this (it also makes the old
  `conn_buf.len() + decoder.body_bytes()` charge explicit instead of arithmetic).
- **`Connection: close` semantics are version-scoped.** The core decides close only for
  HTTP/1.x; on HTTP/2 a `Connection` field is invalid (RFC 7540 §8.1) and honouring it would
  let one client drop a multiplexed connection carrying other live streams, so `Version::Http2`
  never sets it and a response cannot close an h2 connection either. Emitting the `Connection`
  header itself stays in the HTTP/1.1 frontend, which is where the wire syntax belongs.
Falsifiability: three wire tests failed on the first run (`http10_requests_close_after_one_response_by_default`,
`http10_explicit_keepalive_persists_and_is_advertised`, `repeated_connection_lines_still_signal_close`)
because the extraction had dropped the HTTP/1 `Connection` header emission — restored, then green.
Suite: `cargo test --workspace` **376 passed / 0 failed**, identical to the pre-refactor count,
`cargo build --release -p aether` clean with 0 warnings. No benchmark: this change moves code
between modules, adds no allocations to the hot path, and the HTTP/2 track is what needs
measuring (it will need a fresh baseline, not a comparison against these numbers).
Also recorded: `framework/docs/adr/ADR-001-algorithm-strategy.md`, answering the owner's
algorithm-strategy advisory against real code — what Aether already does, what stays
evidence-gated (direct-match vs radix, SIMD head scan, mutex sharding, bespoke allocator),
and what is rejected (per-component protocol reinvention; "1–2 cycle dispatch" as a criterion).
Stages 2–4 pending: transport abstraction + same-port sniffing (TLS record `0x16` / h2 preface
/ else HTTP/1.1), TLS termination with ALPN, then the h2→core bridge.

## Active Track 14 — Transport, TLS, one port (2026-10-20)
Stages 2 and 3 of the owner's HTTP/2 + TLS choice, delivered together because neither is
testable alone: TLS without a transport abstraction cannot reach the serving loop, and a
transport abstraction with nothing to terminate is untestable by construction.

**Stage 2 — `framework/aether/src/transport.rs`.** `Stream` is an enum, not a `Box<dyn>`:
`Plain(TcpStream)` or `Tls(tokio_rustls TlsStream<TcpStream>)`, with `AsyncRead`/`AsyncWrite`
delegated per arm, plus `peer_addr()` (always the transport peer — a TLS-terminating proxy is
still the peer rate limiting sees) and `negotiated_alpn()`, which exists because the HTTP/2
frontend must key close semantics off the handshake, not off a header. `classify()` `peek`s up
to 24 bytes without consuming them: `0x16 0x03` → TLS, the full RFC 9113 preface → h2c, anything
else → HTTP/1.1. HTTP/1 is the **fallback**, not a method-name match, deliberately: a method-list
classifier would hang up on clients that deserve a real 400. `establish()` bounds the first byte
by `read_timeout` and the handshake by the new `ServerLimits::tls_handshake_timeout` (10 s),
because a client that completes the TCP handshake and then trickles pins a task, a permit and a
socket buffer — the same slowloris class the header path already defends, moved one layer down.
Refusals close with `shutdown()`, never a bare drop: on Windows an unread receive buffer turns a
drop into an RST that discards bytes the client has not read yet.

**Stage 3 — `framework/aether/src/tls.rs`** (rewritten after a first draft invented API surface;
every call below is against vendored rustls 0.23.46 / tokio-rustls 0.26.6 source). rustls pinned
to the **ring** provider — aws-lc-rs compiles C through cmake and would make Windows support
depend on an external toolchain. SNI resolves through a `ResolvesServerCert` reading a
`Mutex<Arc<SharedCerts>>`; unknown or absent name falls back to the catch-all instead of aborting
the handshake, and a table with no catch-all is refused at build time. `replace_certificates`
validates a complete new table before swapping the one `Arc`, so a failed reload leaves the live
chain untouched (asserted by `Arc::ptr_eq`) while an in-flight handshake keeps its snapshot.
`TlsCertSet::from_pem` refuses an absent key, an encrypted key (checked before rustls-pemfile
silently skips the block) and a key whose SPKI differs from the certificate — `InconsistentKeys::
KeyMismatch` is matched as an enum variant, not by `to_string().contains(...)`. webpki 0.103.16
exposes no public SAN accessor, so SNI names are operator-supplied (`TlsIdentity::for_name`); no
hand-rolled DER walker was added to work around that.

**Two decisions that look like gaps and are not.** ALPN advertises `http/1.1` alone: `advertise_h2`
exists and defaults to off, because offering `h2` before the frontend exists makes a client start
HTTP/2 framing on a server that can only answer HTTP/1.1 — silent corruption, no error anyone can
see. A h2c preface gets a closed connection rather than an HTTP/1 status line, for the same
reason. Both are the honest interim contract, and both are tested.

**Public API consequence (breaking, and now documented):** handlers that name the transport
changed from `WebSocket<tokio::net::TcpStream>` to `WebSocket<aether::transport::Stream>`
(`WsHandlerFn` and both `ws(...)` bounds). Nothing in the workspace or in the downstream probe
annotates the type, and `aether-probe` still fails only with its pre-existing `E0283` at
`../aether-probe/src/main.rs:98` — no new downstream breakage. Root + prelude now re-export
`TlsAcceptor`, `TlsAcceptorBuilder`, `TlsCertSet`, `TlsError`, `TlsIdentity`, `Stream`.

**Caught by the suite, not by reasoning:** the first wiring classified *before* scheduler
admission, so `scheduling::idle_sockets_are_charged_what_they_hold_not_what_they_could_reach`
failed — a silent socket registered 0 bytes instead of `CONN_BASE_MEMORY`. Admission is first
again; the shed path only classifies (peek, no handshake) and writes its plaintext 503 when the
greeting is `Greeting::Http1` and TLS is not required, and the guard is completed if `establish`
later fails. That regression is the argument for running the suite rather than trusting the
design.

Falsifiable fixtures, not committed secrets: `tests/tests/tls_e2e.rs` mints a throwaway CA + leaf
per run with `rcgen` (dev-only), so there is no PEM in the repo, nothing expires, and no openssl
dependency for cross-platform CI.

**Interop with clients that are not rustls.** The wire suite speaks rustls on both ends, which
proves the framework's logic but not the wire format against a foreign stack, so a live server on
a fixed port (throwaway harness, deleted after the run) was driven by two third-party clients:
Windows `curl` (Schannel) negotiated **`ALPN: server accepted http/1.1`** over TLS 1.3 and
returned `HTTP/1.1 200 OK … pong` (with `-k`; without trusting the minted CA it exits 60, which is
the correct refusal), and a Python 3.14 / OpenSSL client offering `h2, http/1.1` reported
`selected_alpn_protocol() == "http/1.1"`, `TLSv1.3`, and the same 200. The same client sending the
24-byte h2c preface got an immediate EOF with no bytes back — the documented refusal, observed from
the outside.

**Also found and fixed while measuring the suite:** `framework/http/src/parser.rs` had a doc
comment and `#[test]` collapse onto one line, so `test_header_byte_budget_enforced` was commented
out of existence — it reported "never used" as dead code while the row it covers stayed green.
Restored (88 crate tests). Swept the whole workspace for the same pattern: no other hit.

**Docs re-measured against this, not pasted forward:** FEATURE matrix gained framing-per-
connection, TLS termination, SNI, live reload, OCSP-as-limit and WSS rows, and its "no host
allow-list" security row was corrected to point at the delivered `enable_host_validation` (it
contradicted the middleware table in the same file). Status counts recounted *mechanically* from
the Aether column: **92 verified / 12 partial / 22 not implemented / 3 unwired / 2 n/a = 131
rows**; the 86/11/24/3/2 snapshot had drifted again. ARCHITECTURE §5.1 records the transport
layer and its three load-bearing consequences. DEPLOYMENT §4 documents TLS for real — including
that ALPN does not advertise `h2`, that revocation is not enforced, and that renewal is an
operator-side fetch. Its `rust:1.80-alpine` line was corrected to 1.98 with a note that edition
2024 makes the old image unbuildable, and flagged as not executed here.
PRODUCTION_READINESS §2 keeps its 2026-10-18 snapshot intact and carries a dated re-measure line
instead of being quietly rewritten. Every TLS snippet in those three guides is in statement form
on purpose: `AetherApp::new().get(..).tls_from_files(..)?` does not compile, because route
registration is `&mut self -> &mut Self` while the builders consume `Self` (registered as
PROB-0011 — found by writing the docs, not by reading them).

**Review method, stated because the mandate requires it:** no independent reviewer agent was used
for this increment. The feasibility/security/correctness pass was done by reading the vendored
rustls, tokio-rustls, rustls-pemfile, webpki and rcgen sources for every API touched, then by
running the suites; performance was deliberately not claimed. The one design error the review
caught was in my own first draft of `tls.rs` (a `resolve()` that would have aborted every
handshake without SNI), and the one behavioural error the suite caught was the admission-order
regression above — which is why neither was shipped on reasoning alone.

Suite: `cargo test --workspace --no-fail-fast` **395 passed / 0 failed / 1 ignored**, `cargo
build --workspace --release` clean with 0 warnings. No benchmark: TLS was not measured against
plaintext throughput — that claim needs a same-session A/B and belongs with the HTTP/2 stage,
where the handshake cost is no longer the only new variable. Platform caveat, because Windows/Linux
parity is a standing requirement and this host has no WSL and no container runtime: every number
above is Windows MSVC. The new layer carries no platform `#[cfg]` at all (the only one in
`tls.rs`/`transport.rs` is `#[cfg(test)]`) and depends only on rustls + `ring` + tokio-rustls, which
is an argument for portability, not a Linux test result — the first Linux run is still owed
(recorded as PROB-0012, alongside the two other open items this stage produced: PROB-0010 for the
version-controlled `framework/target`, and PROB-0011 for the builder-chaining asymmetry that broke
the first draft of the TLS documentation snippets).

## Active Track 15 — HTTP/2 frontend on the shared core, then cleartext by opt-in (2026-10-21)
Stage 4 of the owner-selected path (`h2` + `rustls`, no bespoke protocol stack), plus the h2c
follow-up this track's own "Next Candidate Tracks" note left open. Delivered together because the
bridge without a cleartext decision would have to be re-opened for the same three functions.

**What exists now**
- `framework/aether/src/http2.rs` (new, 638 lines): `h2` 0.4.20 bridged onto the same `ServiceCore`
  HTTP/1.1 uses. One task per accepted stream, because `server::Connection` is neither `Clone` nor
  a `Future` and the accept loop must never await a handler; the request head becomes the same
  `RequestParts` HTTP/1 produces; every live stream re-prices the connection's scheduler
  registration with what it actually holds; a stream the memory budget cannot cover is shed with
  503 while the streams already running on that socket finish untouched; shutdown is
  `graceful_shutdown()` (GOAWAY) plus continued accept inside the listener's drain budget.
- `http::sanitize_field_value` (`framework/http/src/response.rs`) is now one function shared by both
  frontends, so a handler cannot get a CR/LF/NUL through on HTTP/1 and a refused stream on HTTP/2;
  `Cow::Borrowed` keeps the clean-value path allocation-free (2 unit tests, `aether-http` 90).
- `ServerLimits::max_concurrent_streams` (default **100**) is advertised as
  `SETTINGS_MAX_CONCURRENT_STREAMS`, chosen rather than defaulted: RFC 9113 leaves it unbounded and
  an unbounded setting turns one socket into as many concurrent handlers as the client likes.
- `transport::Frontend::{Http1, Http2}` and `establish(tcp, tls, first_byte_timeout,
  handshake_timeout, allow_h2c) -> (Stream, Frontend)`: the framing decision exists **once** —
  ALPN for TLS, preface plus opt-in for cleartext — and the listener dispatches on the value.
  Re-deriving it from the stream is not a fallback but a wrong answer, since a plaintext connection
  has no ALPN to report.
- `AetherApp::enable_h2c` — RFC 9113 §3.3 prior knowledge over the same listener as HTTP/1.1,
  **off by default**, and `require_tls(true)` still refuses every cleartext connection including
  one carrying a preface. Nothing behind it differs: one core, one set of limits, host policy, rate
  limiting, per-stream memory accounting, drain-on-shutdown.
- The handshake is now bounded by `read_timeout` on **both** h2 paths. Before this, a client that
  sent the preface and stopped held a task, a connection permit and a socket buffer indefinitely —
  the slowloris class the header path already defends against, moved one layer earlier. Measured
  live: the stalled connection was closed at 5.03 s and the scheduler's memory registration returned
  to zero.

**Two decisions, recorded because they are refusals rather than gaps**
- `Upgrade: h2c` is **not implemented**. RFC 9113 §3.1 deprecates the upgrade token and
  `HTTP2-Settings` outright, and RFC 7540 §3.2 requires the request that asked for the upgrade to
  be answered as an implicitly half-closed stream 1 — a state `h2` does not synthesise, so the
  exchange means hand-writing the switch over from the HTTP/1 loop. Clients that want cleartext
  HTTP/2 send the preface, which is the path the spec kept.
- Server push is not implemented (RFC 9113 §3.6, §10.8 deprecate it). A WebSocket route reached
  over h2 answers 400, because §8.1 gives HTTP/2 no upgrade mechanism.

**Evidence**
- `cargo test --workspace`: **419 passed / 0 failed / 1 ignored**. `cargo check --workspace
  --all-targets` and `cargo build --workspace --release --examples` clean.
- 17 wire tests in `framework/tests/tests/http2_e2e.rs` (12 over TLS with ALPN `h2`, 5 over
  cleartext h2c, both against a live `AetherServer` with a `rcgen` CA minted per run) and 5 head
  translation unit tests in `http2.rs`. New cleartext coverage includes HTTP/1 and h2c sharing one
  port, h2c still refused when TLS is required, a WebSocket route answered with 400, and the stalled
  preface closed.
- Independent clients, because both ends of the Rust suite share `h2` and `rustls`: **Node.js
  24.18** (`http2`/nghttp2) over TLS as before, and this time **python-h2 3.2.0 + hyperframe +
  hpack** over cleartext against `examples/h2_interop_server 18446 h2c` — 20/20, covering GET/HEAD/
  POST with body+header+query, PING ACK, a fast stream answered in 2 ms while a 300 ms handler was
  still running on the same socket, a 5 MiB flow-controlled download intact, 431 over
  `max_header_list_size` with the connection surviving, WS route → 400, stalled preface closed, no
  PUSH_PROMISE on seven connections. Connection-specific *request* fields needed a fourth script
  that serialises the HEADERS block with hpack and frames it by hand — 8/8 — for the reason below.

**Three claims of mine that measurement killed, kept here because each was plausible**
- The Node client's `SETTINGS headerTableSize=4096 maxFrameSize=16384 initialWindowSize=65535` were
  **client-assumed protocol defaults**, not bytes this server sent. Reading the frame off the wire
  shows it advertises `MAX_CONCURRENT_STREAMS=100` and `MAX_HEADER_LIST_SIZE=8192` and nothing else.
  The matrix now records the correction instead of silently restating the numbers.
- I implemented `has_forbidden_request_field()` (a 400 for `Connection`/`Transfer-Encoding`/
  `Upgrade` on a request) after a python-h2 probe reported those fields being served with a 200.
  The 200 was the *probe's own client*: `validate_outbound_headers=False` silences python-h2's
  errors, not its filter, and its encoder removes `connection`, `keep-alive`, `transfer-encoding`
  and `upgrade` before they reach the socket — proved by decoding python-h2's own output frame.
  Hand-framed HEADERS showed the truth: `h2` answers each of those, and a non-`trailers` `te`, with
  RST_STREAM before any handler runs, while `te: trailers` passes through untouched. The function
  and its two unit tests are deleted — unreachable code is the same defect as `UNWIRED_IMPLEMENTATION`
  in the matrix — and `FORBIDDEN_FIELDS` now documents which direction each side enforces.
- A probe FAIL reading `slow=b'slow' fast=b''` was not a lost stream: the interop example never
  defined `/fast`, so the second stream got a 404 with an empty body. The probe now asks `/ping`.
- One earlier assertion, "a refused connection must write zero bytes", was wrong too: `h2` writes
  its own SETTINGS before it reads the peer's, so 21 bytes are normal on a refused h2c connection.
  The test now walks the frames and asserts no HEADERS/DATA and complete frame consumption, which
  cannot pass vacuously.

**Review method, stated because the mandate requires it:** no independent reviewer agent was used
for this increment. The feasibility/correctness pass was reading the vendored `h2` 0.4.20 sources
for every API touched (builder settings, `serve_request`, `SendResponse`, reset and drain paths),
then running the suites and the external clients above; a first draft that relied on reasoning
alone was corrected twice by the probes, as recorded above. Performance was not measured and is not
claimed — h2c changes nothing about HTTP/1.1's fast path (the flag is off by default, and the only
added work on that path is one `bool` argument to `establish`), and any HTTP/2-vs-HTTP/1 number
would need a same-session A/B with the pre-declared criteria the perf protocol requires.

**Open after this track:** PROB-0012 stands — every number above is Windows MSVC, and Linux has
never run this suite here (no WSL, no container runtime), so the peek semantics `classify` depends
on and the `refuse()`-before-drop reasoning written against Windows RST behaviour are unverified on
Linux; macOS untested at all. PROB-0010 (version-controlled `framework/target`) is **closed**:
commit `01e79ef` untracked the tree and added the root `.gitignore` without rewriting history. The
`problems.json` entry was deleted with the bootstrap tool rather than marked resolved, because the
tool has no resolution note — `event_log.jsonl`'s `done_problem` line and this paragraph are the
record.
Registry now awaiting owner decisions: PROB-0004, PROB-0005, PROB-0008, PROB-0009, PROB-0011,
PROP-0001. Matrix recount: **94 VERIFIED_IMPLEMENTED / 11 PARTIALLY_IMPLEMENTED /
22 NOT_IMPLEMENTED / 3 UNWIRED_IMPLEMENTATION / 0 BROKEN / 0 UNVERIFIED / 2 NOT_APPLICABLE over
133 rows** (HTTP/2-over-TLS promoted from partial, h2c prior knowledge promoted to verified,
`Upgrade: h2c` added as its own declined row). Docs updated: ARCHITECTURE §5.1/§5.2, DEPLOYMENT,
AETHER_CORE_CAPABILITIES, FEATURE_COMPLETENESS_MATRIX, both READMEs, and the Arabic
FULL_FRAMEWORK_DOCUMENTATION (§18 limits, new §19) and GETTING_STARTED_AR (§5د bullet, new §5هـ).

## Open defects register (2026-10-20) — what is left
Fixed since recording: PROB-0002 (`QueueToken` pin, track 6), PROB-0001 (dead-crate docs, track 7 →
PROP-0001 keeps the "or wire `aether-security`" branch), PROB-0007 (per-method param names, track 8),
PROB-0003 (`SchedulerState` re-export + external policy tests, track 9), and all four PROB-0006 items
(`Expect: 100-continue` + resumable chunked decode in track 10, status-line version echo + the
100-field-line header budget in track 12 — closed). PROB-0008 (buffer *capacity* was not what the
memory budget priced) closed in track 27: the connection charge now follows the block the allocator
handed out and an emptied oversized block is released, with idle pricing unchanged by construction —
the first 8 KiB sits inside `CONN_BASE_MEMORY`, so admitted-connection counts at default limits do
not move, which was the concern that kept this item waiting for a decision.
Still in `problems.json`: PROB-0004 (published comparison tables are historical; re-sweeping means
resuming the paused benchmark campaign — owner decision), PROB-0005 (the second owned header store;
true single ownership means a breaking accessor — owner decision), PROB-0009 (every handler `Err`
answers 400 — the body claimed `E5002` until track 31 replaced it with `E4003` on 2026-10-23, so only
the status class is left: a non-breaking specialization is impossible under coherence
rules against the blanket `Result<T, E: Display + Send>` impl, so the status mapping is a breaking API
decision — **and that is how it went: the owner chose option C′ and track 43 executed it on
2026-10-24**, so the bound is `E: IntoResponseError` now, `Display`-only errors no longer compile
unwrapped, and the entry stays open for one residual only (the redacted 5xx detail is dropped, not
logged). The `Display + Send` blanket quoted above is the 2026-10-20 reading, kept because the
sentence records why the decision was waiting), and PROB-0010 (added 2026-10-20: `framework/target` is version-controlled — 10,101 tracked
files, 8.3 GB, no `.gitignore` — and the Windows `os error 1224` / `LNK1104` build failures recur
while it is scanned; **closed 2026-10-21 by commit `01e79ef`** — the tree is untracked and a root
`.gitignore` exists, so this entry no longer lives in `problems.json`),
and PROB-0011 (added 2026-10-20, found while writing the TLS docs: `get`/`post`/`*_async` take
`&mut self -> &mut Self` while `tls*`/`require_tls`/`listen` consume `Self`, so `AetherApp::new().get(..).tls_from_files(..)?`
is E0507 and no configuration can be chained — unifying the builder style is a breaking API
decision, and it is the reason three doc snippets in `DEPLOYMENT.md`, `GETTING_STARTED_AR.md` and
`FULL_FRAMEWORK_DOCUMENTATION_AR.md` had to be rewritten into statement form), and PROB-0012
(added 2026-10-20: cross-platform parity is required by the vision but every result in this
repository is Windows/MSVC — Linux has never run the suite here and macOS never at all; this is an
open verification gap, not a reported failure).
Track 11 (rate limiting + Host validation) is delivered and wired, so it is not an open
defect.
Later additions: **PROB-0014** (`max_concurrent_streams` and `max_headers_count` are
advertised and enforced but not settable through `AetherConfig`, so their tests could only pin the
defaults) was **closed 2026-10-22 in track 23** — both are `AetherConfig` fields and `AETHER_*`
environment variables, `with_config` and `apply_config` now share one derivation, and each bound is
pinned at a configured value as well as at the default. **PROB-0015** (the HTTP/2 response side hands
a whole payload to
`h2` in one `send_data`, so a slow peer buffers it and nothing charges it to the memory budget —
chunked writes are also the prerequisite for streaming responses) and **PROB-0016** (extension
payloads are not priced by the memory budget; an owner decision, since an arbitrary
`Send + Sync` value has no measurable size) — **PROB-0016 closed 2026-10-26 in track 74, and the
"owner decision" framing turned out to be unnecessary: measuring the container priced it at 160 B +
`size_of::<T>()` for every entry, and the part that genuinely cannot be measured from here (a value's
heap) is now declared by the inserter through `insert_extension_sized`.** Recorded 2026-10-21 in the same
pass that delivered request-scoped extensions, so that the feature is documented with its limit rather
than without it.
**PROB-0017** (WebSocket upgrade routes bypassed the user middleware chain, and `RouteGroup::ws`
dropped the group's own middleware) was found while documenting that slice, proven on the wire before
anything was changed, and **closed in track 18 the same day** — five wire tests plus one corrected
scheduling tally. Its two remaining consequences are recorded as matrix item 18 (the `101` is outside
the response pipeline, so middleware and the CORS/security/compression pass cannot edit it) and
matrix item 9 (nothing wraps the session after handover).

**Read that last paragraph with the ID in mind, not the number.** The WebSocket item above really was
`PROB-0017` when it closed on 2026-10-21, and the number was then recycled: `problems.json` carries
`PROB-0017` today as "HTTP/1 response write buffer is not priced by the memory budget" (created
2026-10-23T13:43:59Z, **closed by track 31 the same day**), beside `PROB-0018` (the h2 length-vs-block
price, also closed by track 31) and `PROB-0019`, which is the registered hazard itself — the registry
tool numbers from the items still present and deletes on closure, so a closed ID comes back as a
different problem. Eleven citations in four docs and this file still mean the WebSocket one; they are
left as written, because they were true when written, and **PROB-0019** is where the fix belongs.

## Next Candidate Tracks
Current front (owner decision 2026-10-20, `h2` + `rustls`, no bespoke protocol stack): **delivered**
— Stage 4 bridged `h2` onto `ServiceCore` and `advertise_h2` default-on, and the h2c question was
answered as opt-in prior knowledge, with `Upgrade: h2c` declined on RFC grounds (track 15).
After that, previously listed: streaming request/response bodies, signed forwarded-client
identity (so rate limiting works behind a proxy), CSRF/auth middleware, per-route middleware API,
and the first Linux run (PROB-0012). Request-scoped extensions — also on that list — are
**delivered 2026-10-21** (track 17); their one open consequence, PROB-0016, was **closed 2026-10-26 in track 74**. The per-route
middleware API is **delivered 2026-10-29** (track 94: `route_sync_with`, `route_async_with`,
`ws_with`), and WebSocket subprotocol negotiation is **delivered 2026-10-29** (track 95:
`ws_subprotocols`, RFC 6455 §4.2.2 selection, the echoed line, `400` on an unmatched offer;
permessage-deflate stays owner-deferred). WebSocket routes now
take part in that chain too (track 18, PROB-0017 closed), which removes "auth middleware" from the
gap list for everything *except* the upgrade-response limit in matrix item 18.
Repo hygiene: **resolved 2026-10-21** — `framework/target` is untracked and a root `.gitignore`
exists (commit `01e79ef`, PROB-0010 closed). History still holds the blobs; shrinking them is a
history rewrite and is out of the rules. The same deletion was made independently on GitHub
(`fb6a4e8`, 1841 build-output paths, no source, no insertions); it was reviewed read-only and merged
as `41dbe39` on 2026-10-22 with zero content change (track 22). **Whether to push `main` is still the
owner's open decision** — `main` was 32 ahead / 0 behind when the payload below was measured
(`git rev-list --left-right --count origin/main...main` → `0 32`, 2026-10-22, before track 23's own
commits landed; track 23 adds three more, so re-run the command rather than trust either number),
and everything it would publish is
Windows/MSVC-only until PROB-0012 closes. The cost of that push is now measured rather than guessed
(track 23): ~671 MiB of it is build-output blobs that no tree in `main` still contains, i.e. 99.99 %
of the payload is `framework/target/` files that were added and later deleted, and the source, docs
and governance files that a reader would actually want are under 0.1 MiB packed. History cannot be
rewritten under the project rules, so the payload is a property of the whole unpushed chain, not of
HEAD — and track 23's own commits change that only by adding source, doc and governance blobs, never
build output, so the 670.8 MiB figure below still describes what a push would transfer.

## Last Update
2026-10-27 (track 79 — **the evidence now outlives the scratch directory, and a committed gate keeps
it that way**):
Taken from `notes/2026-10-26-remediation-roadmap.md` §9 row 79, under the decision-independence
policy. The rule is §8's: every probe a claim rests on becomes a committed, re-runnable artifact, and
where a probe cannot be committed the document carries the reading, the exact command and the date.
- **Measured before touching anything.** Every machine-absolute path in the committed tree, by file:
  the two user-facing globs (`docs/**`, `framework/docs/**`) held **5 mentions in 2 files**; the four
  governance note files hold **118 mentions / 71 distinct paths**, and they survive their own subject
  because each line states the reading it was taken from; `AGENTS.md` 1 (the `C:/tmp` isolation rule,
  deliberately outside the repository), and `framework/tests/tests/scheduling.rs` 1 (a doc comment
  pointing at a deleted probe log). Four of the five user-facing mentions were in-repository
  `file:///c/Users/...` links and one was a duplicate hash line; all six of the failures were
  `PRODUCTION_READINESS_REPORT.md` scratch pointers.
- **The archive that `SUMMARY.md` indexed was half missing.** `lock-split-2026-10-20/SUMMARY.md` names
  six raw logs; only `run5_paced_fixed.txt` and `run6_repeats.txt` were in the repository, and the
  four that hold the withdrawn first paced run (`run4_pacer_control.txt`) and the per-site table
  (`run1.txt`, `run2_controls.txt`, `run3_sites.txt`) existed only in the owner's scratch directory.
  They are committed now — the directory held **12** entries while that baseline said 11, so the
  documented count was itself off by one; with the four logs it holds **16**. `cmp` showed
  `run5`/`run6` already in the tree are byte-identical to the copies beside them, so the archive
  holds every log `SUMMARY.md` names and the external path it used to point at is gone.
  `BASELINE-2026-10-20-SCHED-ROTATION.md`'s archive count went from 12 to 16 with it, and the `11`
  it used to say — the number the document claimed, one short of what `git ls-tree 3c838e3` shows —
  is gone.
- **The five report sites now carry what a reader needs.** `:431` and `:441` name the command and the
  date instead of scratch logs; `:449` says the run's log is not preserved and what the number
  therefore stands on; `:765-767` writes the temp directory in portable notation; `:781` says
  `aether-probe` is a sibling directory beside this repository. The four `file:///c/` links stay: they
  resolve inside the repository, which is the whole test.
- **The gate.** `.project-governance/tools/evidence_check.py`: **a machine-absolute path named in a
  user-facing document must resolve inside the repository.** `~`, `$TMPDIR` and `%TEMP%` are exempt by
  construction because they resolve per reader; a drive path, a `file://` URL and a UNC path are
  checked. It fails closed — a document that legitimately wants `C:/Program Files` produces a finding
  whose fix is portable notation or a reasoned exemption in the tool — because a gate that looks only
  for the paths this track happened to find is the gate that stays green on the next one. **15
  unittests** pin the refusals, and the repository test asserts a floor of 30 documents and at least
  one token, since zero documents and zero findings print the same thing.
- **Three false-positive shapes the first version had, each fixed rather than excused.** `http://` and
  `https://` were read as drives (`p:\`, `s:\`) because any letter before `:` qualified — the letter
  must now stand alone and a following slash is refused; a prose `C:` with no separator is not a path;
  and `file:///etc/x` had its remainder read as the *relative* path `etc/x`, so every POSIX-spelled
  link was silently excused as inside the repository — the POSIX spelling now reads as rooted.
- **Falsified against the tree it was written for, not a fixture.** Running the gate on
  `3c838e3:framework/docs/PRODUCTION_READINESS_REPORT.md` returns 10 tokens and **6 findings** at
  `:431`, `:441`, `:449`, `:766` (twice) and `:781` — every scratch pointer, no misses; the four
  in-repo links flag only because that fixture root is not the repository.
- **Two lines outside the globs, found by hand.** `scheduling.rs`'s doc comment dropped
  `C:/tmp/t72/probe_b.log` and keeps the reading it was taken from (`active=1 registered=33603584`
  six seconds into a 300 ms read deadline). `code-hashes-2026-10-19.txt` carried `response.rs`
  **twice** — line 1 relative, line 6 with the author's absolute path — with **different hashes**, so
  the revert anchor for that file was ambiguous and unreadable off this machine. Three of the six
  hashes reproduce from `1116e1a`, the first commit, via `git show 1116e1a:framework/<path> |
  sha256sum` (cors.rs, policy.rs, engine.rs); `app.rs` and both `response.rs` lines match no committed
  blob at either layout, nor a CRLF variant, because they were taken on a working tree that changed
  before that commit. The file now carries a dated header stating that, naming which pair disagrees
  with itself, and recording that `sha256sum -c` parses the comments and reports six FAILED, which is
  what a pre-optimization anchor should do.
- **Linux typing, the cheap half of PROB-0012.** `rustup target add x86_64-unknown-linux-gnu` works;
  `cargo check --workspace --target` fails in exactly one place — `ring v0.17.14`'s build script wants
  `x86_64-linux-gnu-gcc` (`ToolNotFound`) — with no gcc/clang/zig on PATH and no WSL (`wsl -l -v`
  reports no distribution). Two readings were obtained without a cross compiler: `-p aether-http -p
  aether-router -p aether-core` checks exit 0 for the Linux target, and the one `#[cfg(unix)]` server
  block (the `signal::ctrl_c` shutdown arm) compiles as a tokio fragment crate. A suspicion that
  `use tokio::signal::unix::{signal, ...}` shadows the `signal` module and would break `ctrl_c()` was
  **refuted by compiling** and is not recorded as a defect. PROB-0012 stays open: it still needs a
  Linux host or a cross C toolchain for the full workspace, which is the owner's decision.
- **Residual, named rather than implied clean.** The gate reads the two document globs only: the notes
  corpus (118 mentions) stays ungated by design because each line states its reading; a `.rs` doc
  comment and a `.txt` transcript are outside it (two existed, both fixed above); and a POSIX absolute
  path without a `file://` prefix is not matched, because a leading `/` in these documents is almost
  always a route.
- **An independent reviewer checked the two commits, and refuted two of my numbers.** Read-only, one
  verdict (SHOULD-FIX), per-point evidence. It confirmed the gate's rule on temp roots outside the
  repository, replayed the pre-track report (6 findings at the six sites), and re-ran the tests — then
  refuted the hash archaeology above and the archive count. The hash scan had iterated the recorded
  table's short paths (`middleware/src/cors.rs`) instead of the full ones, so every lookup missed and
  the uniform negative was an artifact of an unused variable, not a fact about the tree; a second scan
  tested only CRLF variants. The reviewer also showed the POSIX test passed under the old `as_path`
  because the finding echoes the token the assertion looked for. All three are fixed in the follow-up
  commit whose subject names the review:
  header and baseline corrected, counts corrected (12 → 16, not 11 → 16), and the test now asserts
  `as_path(...) == Path("/etc/run.txt")` plus the folded text of three Windows spellings — both
  mutants are caught (2 failures under the old `as_path`, 7 fixture tests under the old regex).
  Reviewer's unverified list, kept: that `C:/tmp/t79-post-test.log` came from the exact committed
  tree; that the four logs byte-match the scratch originals (those copies are gone — `cmp` was only
  ever possible against `aether-bench-compare`'s copies of `run5`/`run6`); fmt/clippy not re-run by
  the reviewer.
- **Gates on this tree.** `cargo fmt --all -- --check` 0 bytes; `cargo clippy --workspace --all-targets
  -- -D warnings` exit 0 / 0 warnings; `cargo test --workspace` **527 / 0 / 1** over 50
  `test result:` lines (the only Rust edit is a doc comment); governance `python -B -m unittest
  discover -s .project-governance/tools/tests` exit 0 at **77 tests** (61 before, 16 new);
  `cite_check.py` 66 citations / 45 spans / 0 findings; `evidence_check.py` **33 documents / 0
  findings**; `validate` **exit 1** on the same four standing findings. **Nothing pushed.**

2026-10-27 (track 80 — **PROB-0032 closed: every `path:line` in `docs/` is now checked against the
code it names, and 36 of them named something else**):
Taken from `notes/2026-10-26-remediation-roadmap.md` §9 under the standing mandate, at the owner's
«قرر بنفسك». The stated method was content, not existence: resolve the file, require the span inside
it, read the span, and accept the citation only if the span holds what the sentence claims.
- **The corpus, measured before and after.** At `e81efed` the documents carried **61** citations
  (60 of the form `file.rs:NNN[-MMM]` and one bare `:664` on `docs/07:86`) over 40 distinct spans.
  After this track: **66** over 45 spans — three coordinates on `docs/05:129` were prose-only digits
  (`app.rs:1540` was backticked, `1986`/`2036`/`2090`/`2120` were not), so no scanner saw them; that
  sentence is now four citations, and the count it states was wrong too (four refusal sites today,
  not five). The other two new citations are `docs/02`'s rebuilt excerpt (below).
- **What was dead, counted from the diff rather than by hand.** Thirty-six coordinates in 8 of the 14
  documents now name a different span: 29 in `b8d6ffa`, and 7 more once every pin was read (below).
  `app.rs` 1769→1915, 1857→2008, 1540→1680, 1631→1771, 2029-2034→2185-2189, 2034→2190,
  2068-2083→2224-2234, 1946-1950→2102-2106, 2329→2499-2501, 239→250, 285-322→296-333, 535→580;
  `serve.rs` 423-428→487-495; `parser.rs` 262→338; `response.rs` 206-236→225-255, 227-235→246-255,
  238→257, 287→310; `worker.rs` 133→23; `trie.rs` 84→100 (two documents); `jwt.rs` 82→84 (two
  documents). Several old coordinates landed on unrelated code rather than blank space —
  `app.rs:1946-1950` was another branch of the same loop, `response.rs:287` was a date write — which
  is why existence checks pass on them. The `docs/04` and `docs/07` slice of the same class landed
  earlier, in `8603d82` with track 75. The two writer spans were first re-derived as `225-253` and
  `246-253`, then widened by two lines: the third function's body ends at `:255`, so a span that
  stopped at `:253` showed its signature and not its delegation — the prose calls them three
  functions that all delegate, and the cited span has to hold that.
- **The seven the pins found, after the sweep looked finished.** Reading each pin against its span
  instead of trusting the first sweep exposed three more defects, each a span whose *ends* were wrong
  while its content passed an anchor-anywhere check: `docs/02:66` quoted a parse-loop excerpt at
  `app.rs:1624-1636` (the accept arm) and `app.rs:1936-1950` (the write deadline) — two regions of the
  right function that the excerpt does not show, now the composite the code actually has
  (`app.rs:1766-1776` head and call, `:2093` the buffer advance, `:2133`/`:2184` the two arms);
  `docs/09`'s `IntoResponseError` span started at `:99`, inside the doc comment above the trait, true
  at `:110`; and `node.rs:41-43` stopped one line short of `supports_method`'s closing brace, true at
  `42-44`, in three documents.
- **The gate, committed rather than scripted once.** `.project-governance/tools/cite_check.py` reads
  both citation forms, resolves an exact path first then a unique basename, and fails on nine things:
  a bare coordinate naming no file, no such file, ambiguous basename, span out of bounds, a citation
  whose span has no pin, a pin whose shape disagrees with the span, a first line moved or replaced, a
  last line moved or replaced, and a pin no citation points at. **The pin is
  `(file, first line, last line) -> regex | (first regex, last regex)`** — 45 for 45 spans — because
  a pin that proves one line is blind to an edit *inside* the span: the first version anchored
  somewhere in the span, and a blank line above a 3-line citation then moved it with no finding, the
  exact off-by-one that had shipped in `docs/02`. A multi-line pin states both ends and the checker
  refuses a shape mismatch, so no span can be half-checked — the repository's "re-derive both ends"
  rule, mechanised. 19 unittests pin each refusal on a four-file tree, and the repository test refuses
  to pass vacuously (`>= 50` citations, zero findings, every pin used).
- **Falsified before it was trusted.** Four mutants on the real tree: a doc coordinate edited by one
  line → *not pinned*; a cited symbol renamed → *the span does not hold what the prose claims*; one
  blank line inserted above a cited span → *the span's first line moved* (pinned at :2499, found at
  :2500); one blank line inserted at the top of `app.rs` → **25 findings**, every `app.rs` citation
  below it, no survivors. The first version of the checker passed the third mutant — a 3-line span
  with a mid-span anchor tolerates a 1-line shift — which is the redesign above. The redesigned rule
  was then falsified in its own direction on a fixture: a line inserted inside a function leaves the
  first line matching and moves only the closing brace, and the last-line pin alone reports it.
- **Two tool defects found by running it.** `@dataclass` cannot be loaded through
  `importlib.util.spec_from_file_location` without a `sys.modules` entry — the pattern the existing
  governance tests use — so the records are `NamedTuple`; the finding text used `→`, which crashed
  under a cp1252 stdout, so the output is ASCII with `errors="replace"`; and a finding whose pattern
  matched several survivors said "matches only at" and listed two lines, so the message now says the
  pattern is not unique and names the survivors.
- **The notes, measured and named rather than claimed.** PROB-0032 also ordered the
  `.project-governance/notes/` corpus. It holds **268** citations, of which **32** do not resolve to
  a unique in-tree file — they name external crates (`actix-http-3.19.0/src/config.rs:207`), other
  projects (`rust_aether/src/main.rs:71`) or a short form whose file is the sentence
  (`scheduling.rs:489`). That corpus is not mechanically gate-able at a prize worth paying, so the
  seven coordinates the problem named were re-derived by hand: `app.rs:190`→`:201` (with
  `canonical_reason` and `Response::json_status` read to confirm neither clamps — the finding stands),
  `serve.rs:481`→`:508`, `response.rs:357`→`:383` and `asserted at :829`→`:877`,
  `status_forbids_body` `312-314`→`338-339`, and the `Payload Too Large` sites, four in the register
  and **three** today (`app.rs:2187`, `:2573`, `http2.rs:372`). The same block's claim that no doc
  asserts the phrase was false — `docs/00:204` says it in both languages — and was corrected with a
  dated marker rather than deleted. What remains unverified there is said so, not implied.
- **Ledger.** `done PROB-0032` closed it: problems 6 → 5, and the five that remain are 0004, 0005,
  0009, 0011, 0012. The event log gains `done_and_deleted PROB-0032`; the ID stays in the allocation
  history the recycling ratchet reads, so closing it does not free the number.
- **Gates on this tree.** `cargo test --workspace` **527 passed / 0 failed / 1 ignored** over 50
  `test result:` lines; `cargo fmt --all -- --check` 0 bytes; `cargo clippy --workspace --all-targets
  -- -D warnings` exit 0, 0 warnings; governance `python -B -m unittest discover -s
  .project-governance/tools/tests` exit 0 at **61 tests** (42 before, 19 new); `cite_check.py` exit 0
  at 66 citations / 45 spans / 0 findings; `validate` **exit 1** on the same four standing findings.
  **Nothing pushed.**

2026-10-27 (track 81 — **PROB-0033 closed: two memory-release tests measured the harness's own
scheduling, and track 75's final re-verify of its committed tree is what caught them**):
Not on the roadmap. It surfaced because track 75 re-ran the full ladder on the tree it had just
committed instead of on the tree its own readings came from, and that run returned `TEST=101`.
- **The reading.** `a_declared_extension_is_priced_on_the_http_2_path_and_released_with_the_stream`
  failed at `framework/tests/tests/http2_e2e.rs:1487` *on `e81efed`* (the assertion now sits at
  `:1502`, after this track's own edits) with
  `a declared extension outlived its HTTP/2 stream` and `left: 32768, right: 0`. Run alone it passed
  3/3 in the same window, so the failure is load-only. The suite also printed only **28**
  `test result:` lines (394 passed / 1 failed / 1 ignored) because cargo stops after a failing target
  — the one reading that displaced the recorded 527 for that moment, and it is recorded as a failure,
  not smoothed into the green chain.
- **Root cause, located rather than guessed.** The test sampled its baseline with
  `let open = layer.resources().registered_memory_bytes;` immediately after `H2Client::connect_plain`.
  The socket's own charge is `CONN_BASE_MEMORY = 32 * 1024` (`framework/aether/src/scheduling.rs:61`)
  and it is admitted by the server's accept task at `framework/aether/src/app.rs:1658` — on the
  server's schedule, not the client's handshake. Under suite load the sample read **0**, and that 0
  then became the target `wait_for_registration(&layer, open)` polled back toward for 2 s: the test
  asked whether the stream's charge had fallen to a baseline that never included the connection. The
  production behaviour was correct throughout; the baseline was raced.
- **The class, swept rather than patched once.** Three sites in that file take a settle baseline.
  `:888` (the stalled-body test) already waited for `CONN_BASE_MEMORY` and said why in a comment —
  that is the pattern. `:1458` was the failing one above; `:1352`
  (`finished_streams_leave_no_memory_registration_behind`, blame `32df904`, 2026-10-22) had the same
  shape *plus* a comment asserting "the connection itself is already registered at this point" — an
  unverified assumption written as fact, which is why it survived: its 4×16 B trickle happens to give
  the accept task time to land. Both now take `wait_for_registration(&layer, CONN_BASE_MEMORY)` and
  assert the baseline equals it. The `assert_eq!(…, 0, "no baseline")` at `:2176` runs before any
  client connects, and the two `wait_for_registration(&layer, 0)` sites run after `server.stop()`,
  which joins the serving tasks — checked, unchanged.
- **The mutants.** With the handler guard deliberately leaked in `dispatch`
  (`std::mem::forget(g)` in `framework/aether/src/serve.rs`), both fixed tests fail at their settle
  assertion — `65552` vs `32768` and `540856` vs `524472` — so the assertions detect a real leak and
  the baseline is exactly the connection's charge, not a number chosen to make them pass. `serve.rs`
  restored; `git diff --numstat` for it is empty.
- **Why the number is 81.** 76–80 are already reserved in `notes/2026-10-26-remediation-roadmap.md`
  (76 API coherence, 77 platform breadth, 78 performance, 79 evidence durability, 80 PROB-0032).
  Taking 76 would put two meanings on one track number, which is the same defect the ledger's
  recycled-ID ratchet exists to pin. The roadmap gains a §10 and a sequencing row instead.
- **Gates on this tree.** Three consecutive `cargo test --workspace` runs each read **527 passed /
  0 failed / 1 ignored** over 50 `test result:` lines; `cargo fmt --all -- --check` 0 bytes;
  `cargo clippy --workspace --all-targets -- -D warnings` exit 0, 0 warnings; governance
  `python -B -m unittest discover -s .project-governance/tools/tests` exit 0 (`Ran 42 tests`, OK,
  re-run after PROB-0033 was allocated and after it was closed); `validate` **exit 1** on the same four
  standing findings (3 `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`). **Nothing pushed.**
- **Re-verified on the committed tree, which is the reading track 75 taught this project to insist on.**
  After `73a1e58`: `cargo fmt --all -- --check` 0 bytes, `cargo test --workspace` exit 0 at
  **527 / 0 / 1** over 50 `test result:` lines, governance `Ran 42 tests` / OK exit 0, `validate`
  exit 1 printing the same four standing lines. No new target, no count moved.
- **Residual of the same class, named not fixed.** `http2_e2e.rs:1531` waits a fixed
  `Duration::from_millis(150)` "for the admitted stream to be holding its charges inside the handler" —
  a fixed pause is a bet on the scheduler, which is what this track just lost. It was not observed
  failing, so it is recorded as unverified rather than rewritten on suspicion. The file's other two
  `from_millis(150)` occurrences were checked and are not this class: `:1251` is an assertion on
  elapsed time (the connection must not close before its keep-alive timeout), and `:841` is the sleep
  inside a bounded poll loop that already re-checks its condition. Fixing flakiness by removing the
  assumption (as here) rather than by retrying is the shape track 79 wants everywhere.
2026-10-26 (track 75 — **PROB-0028 closed as the naming decision it was: the enforced number was
already right, the label was not**):
Taken from `notes/2026-10-26-remediation-roadmap.md` §4 under the standing mandate
«كمل الشغل لحالك واتخذ قرار بنفسك بما يتوافق مع رؤية المشروع», whose two parts were (a) name the
enforced quantity everywhere it is advertised and state the ceiling it actually permits, (b) then
decide whether a framing cap belongs in the API.
- **The two enforcements, located before anything was written.** `framework/http/src/parser.rs:262`
  bounds a declared `Content-Length`, `:647`/`:664` bound a chunked decode as it runs, and
  `framework/aether/src/http2.rs:432` bounds DATA payloads — all three on **decoded** bytes. HTTP/1.1
  applies an earlier bound on **wire** bytes at `framework/aether/src/app.rs:2247`,
  `conn_buf.len() + n > max_header_bytes + max_body_bytes` = 2,105,344 on the defaults. So the memory
  budget is priced correctly (track 32 and track 70 established why: framing and the decoder's copy
  are both resident until the request completes) and only the name was wrong. HTTP/2 has no mismatch
  because its frames are not buffered as wire bytes.
- **The ceiling, measured rather than computed.** At 1-byte chunks the framing costs six wire bytes
  per payload byte (`1\r\nx\r\n`), this request's head is 62 bytes and the terminator 5, so
  350,879 payload bytes are 2,105,341 on the wire — served, with the handler holding all 350,879
  decoded bytes — while 350,880 are 2,105,347 and are refused 413. That is **16.7 %** of the
  advertised 2 MiB, and it replaces this problem's own earlier "about 18 %", which was one sampled
  upload rather than the boundary. A `Content-Length` upload still reaches the full 2 MiB.
- **The test is the falsification of the label.**
  `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes`
  (`framework/tests/tests/http_e2e.rs:1654`) drives both halves over two live raw sockets and asserts
  the served half's decoded length, so it cannot pass on a truncated body. Three mutants were run
  against it on this tree, each with a pre-declared expected red:
  **replace the guard's wire comparison with `false`** (`framework/aether/src/app.rs:2247`) — the test
  fails at the 413 assertion with `left: 200`, which is the reading that says the refusal is the wire
  guard's and not the parser's, since a 350 880-byte decoded body is well inside the parser's
  2 097 152; **`FITS` := 350 880** — the served half fails with `left: 413`, so nothing beyond the
  boundary gets a 200; **`FITS` := 350 873** (six bytes back) — the refused half fails with `left: 200`,
  so the boundary is the stated byte and not merely something earlier. All three reds are the correct
  outcome; none of them is a test that passes on a different rule. `app.rs` and `http_e2e.rs` were then
  restored from backups and the numstat re-read as 8/3 and 63/0 — that, not the prose, is what says the
  mutations left. Reading the real diff earlier in this track caught a defect of my own: the insertion
  had been anchored on the first line of the neighbouring doc comment and silently deleted it, leaving
  `a_buffer_limit_refusal_finishes_the_exchange_without_waiting_for_the_client`'s comment starting
  mid-sentence. Restored, and the file's diff is now 63 insertions / 0 deletions — the numstat is the
  check that says so, not the prose.
- **Where the two quantities are now named.** `ServerLimits::max_body_bytes` and
  `AetherConfig::max_body_bytes` carry the pair and the arithmetic; the guard's comment says wire and
  points at the field; `framework/README.md`'s `AETHER_MAX_BODY_BYTES` row and its security-limits
  bullet state the boundary; the matrix's Body-budget row describes both enforcements and the ±6
  falsification; `PRODUCTION_READINESS_REPORT` gained §17 and an eleventh dated recount reading. In the
  human set: `docs/07` §4 (it already had the right sum and no distinction), both `docs/09` struct
  blocks, `docs/00`'s RFC-9112 hardening item in Arabic and English, `docs/04`'s CL branch.
  `framework/docs/DEPLOYMENT.md` was looked for and names no body-size number, so it had nothing to
  rename — recorded rather than skipped silently.
- **Three changes refused.** Not moved to decoded bytes (a memory regression dressed as correctness,
  exactly as the roadmap warned). Not renamed to say "wire": one field is read on two frontends and on
  HTTP/2 it *is* the decoded cap. No new framing-overhead cap: the wire bound already prices framing,
  and a second cap would refuse legitimate clients for a cost the budget has already taken. The residue
  is an open *idea*, not a defect, and sits in the improvement register — an operator still cannot name
  the two budgets separately, only as a sum.
- **A gate that had never been run cleanly is now clean.** `cargo doc --workspace --no-deps` warned
  about seven phantom links: two `[size_of]` redundant targets this author added in track 74, a
  `MultipartError` path that named nothing, a link to the private `ChunkedBodyDecoder::feed`, and
  three in crates outside this track (`benchmarks`' `[job_count]` run line, `runtime`'s link to
  `aether::AetherApp` which is not its dependency, `scheduler`'s unqualified `SchedulerEngine::admit`).
  All seven are fixed, and the run now exits 0 with 0 warning lines — verified with the command in this
  bullet, not inherited from the earlier per-crate reading.
- **Seven coordinates in `docs/` did not hold their own quoted code, and all seven sat in files this
  track had to edit, so all seven are re-derived here** — counted off the diff, not off memory: four in
  `docs/07` (read-timeout span → `app.rs:2224-2234`, semaphore span → `:1611-1618`, buffer ceiling →
  `:2247` and shrink → `:2102-2106`) and three in `docs/04` (parser excerpt → `parser.rs:207-265`,
  CL.TE test → `:689`, `hmac::verify` → `jwt.rs:84`). Each was content-checked, not existence-checked:
  the span was read and required to hold the snippet or the named item the prose quotes. The rest of the
  class is registered as **PROB-0032** and scheduled as track 80: `docs/05`'s response-emit coordinates
  are dead wholesale, and the same is true of several citations inside
  `.project-governance/notes/2026-10-21-improvement-register.md`.
- **Counts, re-derived from the 50 `test result:` lines.** `cargo test --workspace` is
  **527 passed / 0 failed / 1 ignored** = 305 unit + 217 integration + 5 doc, with `http_e2e` 52.
  Every live site moved from 526/216/51 and none of the dated readings was rewritten: root `README.md`,
  `framework/README.md` (:55 and §suite paragraph), `BASELINE_MEASUREMENTS.md` (its per-target table,
  its chain extended by this slice's one test), `FEATURE_COMPLETENESS_MATRIX.md` (evidence bullet, its
  "53 tests added since 474", the `http_e2e` family list, §Unit/Integration rows), and the
  `PRODUCTION_READINESS_REPORT` §10 evidence list. The reproducible rule: 526 plus the one
  `#[tokio::test]` attribute this slice added.
- **The gate this track's own closure broke, and what it says.** `python -B -m unittest discover -s
  .project-governance/tools/tests` ran **42 tests, 1 failure, exit 1** on the tree after `done
  PROB-0028`. The failure was not the standing red — `validate` still printed exactly its four
  pre-existing findings — but `test_recycling_ratchet.RealLedgerRatchetTest
  .test_a_new_reuse_is_not_absorbed_by_the_standing_set`, the test that forges two second allocations
  on a copy of the real ledger and requires both detectors to name them. It hard-coded `PROB-0028` as
  "an ID that is open today", and this track closed it: the forged reuse still printed as
  `[RECYCLED ID]`, `[OPEN VS HISTORY]` correctly stayed silent for a closed item, and the test read that
  as a detector failure. Two exits were refused — widening `STANDING_OPEN_ON_RECYCLED` (a deliberate act
  per that file's own header, and here it would have absorbed a *fixture* choice into an integrity
  baseline) and re-opening PROB-0028 to keep a test green. The root cause was the fixture naming a
  repository fact instead of deriving one: `forged_pair()` now picks, from the live ledger, one ID
  allocated once and closed and one allocated once and still open, and raises `AssertionError` when no
  such pair exists — so an emptied open ledger stops the test loudly rather than letting it pass with
  nothing to forge. `STANDING_RECYCLED` and `STANDING_OPEN_ON_RECYCLED` are untouched.
  Sensitivity re-proved on the repaired test by handing `validate` four defects, each of which the test
  must reject: never print `[OPEN VS HISTORY]` (red, correct); print it for *every* recycled ID
  (red — it must not flag the closed forgery); skip a reuse whose ID is not open (red — this is exactly
  the absorption the test exists to catch); drop every `[RECYCLED ID]` line (red). Then
  `Ran 42 tests ... OK`, exit 0. The probe driver is `C:/tmp/t75-ratchet-probes.py`; the same class of
  drift as PROB-0032, one file earlier: a claim frozen as a literal, falsified only by the ledger
  changing underneath it.
- **Split into four commits, with the first one's tree verified on its own.** `cea1cee` (the seven
  rustdoc links), `3f375b3` (the naming + the boundary test), `8603d82` (the markdown docs), this record
  + the ledger + the ratchet fix. `app.rs` carries two unrelated concerns, so it was staged hunk by
  hunk (`git apply --cached` on a single-hunk patch, the remainder re-read as 2/2 then 6/1) rather than
  letting a doc-comment link fix travel inside a naming commit. The order is what makes every
  intermediate tree gate-clean: the link repair comes first, so `cea1cee`'s own tree is the one the
  doc gate is true of. Verified by stashing the rest under a unique tag (`t75-verify-A-hamza`), running
  `cargo fmt --all -- --check` (0 bytes), `cargo doc --workspace --no-deps` (0 warning lines, `Finished`
  + `Generated` on stderr-free success) and `cargo test --workspace --doc` (**5 passed / 0 failed**, the
  only non-zero `test result:` row) on that bare tree, then `git stash apply stash@{0}` — not `pop` —
  and comparing the restored numstat against the 20-file fingerprint taken before the stash: identical,
  so the entry was dropped only after its contents were proven back in place.
- **Gates on this tree, re-run after that fix.** `cargo fmt --all -- --check` 0 bytes; `cargo clippy
  --workspace --all-targets -- -D warnings` 0 warnings; `cargo doc --workspace --no-deps` 0 warnings;
  `cargo test --workspace` exit 0 at 527 / 0 / 1; `cargo build --workspace --examples` exit 0; governance
  `python -B -m unittest discover -s .project-governance/tools/tests` exit 0 (42 / OK); `validate`
  **exit 1** on the same four pre-existing findings (3 `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`),
  recorded as failed and not worked around.
- **Ledger.** `new problem` added **PROB-0032** first, then `done PROB-0028`: open problems 6 → 7 → 6,
  `status` re-read as `Requirements: 1 | Problems: 6 | Proposals: 1 | Criticisms: 0`, and the event log
  holds `new_problem PROB-0032` and `done_and_deleted PROB-0028`. The 6 now open: 0004, 0005, 0009,
  0011, 0012, 0032.
- **No throughput claim.** The boundary is a byte count read off a live socket. Nothing here is a
  request-per-second or latency result, and no benchmark was run; the owner measures those.
- **This entry's own final re-verify is what found track 81.** Every reading above was taken on the
  pre-commit tree. The same ladder re-run on the committed tree (`e81efed`) returned `TEST=101`: one
  `http2_e2e` memory-release test failed on a baseline sampled before the connection's own charge had
  landed. That red reading is kept as a red reading — the 527 above describes the tree those gates ran
  on — and the defect became PROB-0033, closed in track 81.
- **Nothing pushed.**
- **Next without the owner:** track 79 (evidence durability — the probes this track cites are inside the
  tests that pin them, which is the shape 79 wants everywhere), then track 80 (PROB-0032's coordinate
  re-derivation). 76, 77 and 78 stay blocked on the owner's decisions or a machine that is not here;
  task #69's pinned worker-count sweep still needs the owner's run.
2026-10-26 (track 74 — **PROB-0016 and PROB-0027 closed; the first was closed by a measurement that
contradicted its own first answer**):
Taken from `notes/2026-10-26-remediation-roadmap.md` under the standing mandate
«كمل الشغل لحالك واتخذ قرار بنفسك بما يتوافق مع رؤية المشروع», with the roadmap's instruction that these
two close *by measurement, not argument*.
- **The probe.** `framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs` is a standalone crate
  (`cargo run --release`, no dev-dependencies) whose stdout is pasted into its own header; every number
  below is that stdout. A wrapper allocator tags each block with the epoch that allocated it, reports
  that epoch's peak and what it left alive at drop (the leak check), and counts frees of
  foreign-epoch blocks separately — this run excluded 16 777 472 B of them (the probe's own JSON fixtures
  built between cases, plus the runtime's lazy work) so they cannot be charged to whichever case happens
  to trigger them, which is what a before/after counter does. Integrity of the run: 0 full probe chains
  and every case freed everything it allocated.
- **The correction, because it is the interesting part.** `EXTENSION_ENTRY_MEMORY` was drafted at 128
  from the pre-pricing map shape (`HashMap<TypeId, Box<dyn Any + Send + Sync>>`: 200 B for a map's first
  entry, 1 724 B at 16, ~101 B marginal). The same track then gave that entry two `u64` price fields, and
  a container price measured against a shape the code no longer has is a guess wearing a measurement.
  Re-run against the shape that ships (`Entry` = 32 B, its `HashMap` block = 48 B, both `size_of` printed
  by the probe): **264 B for the first entry, 268 / 684 / 1 288 at 2 / 4 / 8, 2 492 B at 16, ~148 B
  marginal.** The constant is therefore **160** — 148 rounded up, because over-pricing a container is
  safe and under-pricing it is what these constants exist to avoid — with a one-time ~104 B residual on
  a map's first entry (264 − 160), stated instead of smoothed away. The probe now measures *both* shapes
  in one run so the 128 → 160 correction is checkable from the artifact rather than from this prose.
- **What the payload rows bought.** One entry holding a 1 MiB payload peaks at **1 048 860 B**; parsed
  JSON kept in the map retains **6.78–8.00×** the wire bytes it was parsed from (989 → 7 915, 15 921 →
  112 821, 259 937 → 1 793 869, 4 237 633 → 28 727 357), so at the default 2 MiB `max_body_bytes` one
  request can keep **~14 MB** alive behind a middleware. That is why the roadmap's cheaper closure —
  charge the map's *capacity* and document the values as unpriced — was refused: 264 + ~148 B/entry
  against a 14 MB payload is a registration two orders of magnitude off, and it would have let the docs
  say the gap was closed. Refused on the numbers, not on taste.
- **PROB-0016 shipped as pricing, not as a warning.** Every entry charges `EXTENSION_ENTRY_MEMORY` +
  `size_of::<T>()` unconditionally; heap a value points at is charged only when its owner declares it,
  through the additive `Extensions::insert_sized(value, owned_heap)` /
  `RequestContext::insert_extension_sized(value, owned_heap)`. The charge rides the request's *own*
  handler registration — `dispatch` calls `ctx.attach_extension_charge()` between admission and the
  handler — so it is released with the request by construction and cannot orphan; an overwrite releases
  the old entry's charge before asking for the new one, and `remove` gives it back. A refused grow
  registers nothing and **the answer is still served** (the same best-effort contract
  `price_response_body` follows) and counts in `metrics.memory_shed`. Three things stay invisible and
  are named, not implied handled: heap nobody declared, growth through `get_mut`, and a `Vec`'s reserved
  tail beyond its length.
- **PROB-0027 closed on a rule, not on a copy.** `bytes` 1.12.1 answers `owned_is_unique == false` for
  every owner-backed block, even the sole handle, so `Bytes::try_into_mut()` can never hand
  `Bytes::from_owner`'s buffer back: pricing it zero-copy is impossible, and the two alternatives each
  cost something — asking the owner for its capacity is a public-API break for whoever implements the
  trait, and copying the body to price it allocates exactly what the charge exists to bound. So the
  rule is documented where a user meets it (`IntoResponse`) and where it is enforced
  (`price_response_body`): an HTTP/2 response body is built with `Bytes::from`, never
  `Bytes::from_owner`. It is backed by `every_body_this_framework_produces_is_a_block_the_budget_can_name`,
  which calls the five real producers (`IntoResponse for String`, `IntoResponse for (u16, String)`,
  `Response::text_dynamic`, `Response::html_dynamic`, `Response::file`), asserts each one's registration
  rises by its block without the payload moving, then releases it to the base.
- **Both closes were falsified, not just asserted.** Mutating a producer to `Bytes::from_owner` makes the
  producers test fail *by name*; deleting the `attach_extension_charge` line in `serve.rs` makes the
  HTTP/2 wire test report `left: 0` — the reading that says it measures the wiring and not the
  arithmetic. The h2 test (`a_declared_extension_is_priced_on_the_http_2_path_and_released_with_the_stream`)
  samples the layer from inside the server on two routes over one h2c connection and asserts the diff is
  `EXTENSION_ENTRY_MEMORY + size_of::<Vec<u8>>() + HEAP` = **524 472 B**, then that the registration is
  still `open` *before* the client disconnects and 0 after `server.stop()` — sampling after the close was
  the flake this slice found and fixed, and the diff is computed from the constants so the 128 → 160
  correction moved the test's expectation with it instead of breaking it.
- **Tests: 513 → 526, and the count is reproducible by a rule.** 13 added `#[test]`/`#[tokio::test…]`
  attributes: 9 in `aether/src/extensions.rs` (17 unit there now), 1 in `aether/src/http2.rs` (13), 2 in
  `tests/extensions_e2e.rs` (8 wire), 1 in `tests/http2_e2e.rs` (38 wire). No test was renamed away and
  none deleted.
- **Gates, all re-executed on this tree.** `cargo fmt --all -- --check` exit **0**, zero output.
  `cargo clippy --workspace --all-targets -- -D warnings` exit **0**, 0 warning lines.
  `cargo test --workspace` exit **0** at **526 passed / 0 failed / 1 ignored** over 50 `test result:`
  lines (305 unit + 216 integration + 5 doc). `cargo build --workspace --examples` exit 0. Governance
  Python suite exit 0 (`Ran 42 tests ... OK`). `validate` exit **1** with the same four standing lines
  (three `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`) — pre-existing, registered as PROB-0019,
  recorded as failed and not worked around. Coordinates re-derived after this slice moved code:
  `ACTIX_FAIR_COMPARISON` F4's five `scheduling.rs` citations (the +18-line constant shifted everything
  below it), and the probe's own `STREAM_BASE_MEMORY` pointer is now by symbol because `:56` had already
  drifted to `:54`.
- **No throughput claim.** These are allocation-accounting numbers from a counting allocator on one
  Windows/MSVC host. Nothing here is a request-per-second or latency result, and no benchmark was run —
  the owner measures. No independent agent was used: the two designs this slice imitates
  (`http::Extensions` pricing nothing at all, Actix's `req.extensions()` likewise) are read directly, and
  the falsifications that matter live in the tests named above.
- **Ledger: PROB-0016 and PROB-0027 closed via `done`**, one after the other from the repository root,
  then `status` re-read: open problems **8 → 6** (`Requirements: 1 | Problems: 6 | Proposals: 1 |
  Criticisms: 0`), and the event log holds both `done_and_deleted` lines. What the 6 are: PROB-0004
  (historical scheduler tables), 0005 (duplicated header lines), 0009 and 0011 (the breaking-shape
  pair), 0012 (Linux), 0028 (chunked-body budget vs advertised limit). Docs updated:
  `PRODUCTION_READINESS_REPORT` §16 (the measurement, the refused option, the rules, the gates),
  `FEATURE_COMPLETENESS_MATRIX` item 17 → Closed and the extensions row, and the `IntoResponse` /
  `price_response_body` doc comments carry the user-facing rule.
- **Committed as four, and re-verified on the tree the last one left.** `d681956` the code and its tests
  (`scheduling.rs`, `extensions.rs`, `app.rs`, `http2.rs`, `serve.rs`, `extensions_e2e`, `http2_e2e`),
  `725a6d5` the probe, `e2fddeb` the docs, `d1eb68d` governance — split by locus, each staged with its
  files named, each `git diff --cached --stat` equal to the working diff it replaced. The intermediate
  trees are the same Rust tree the gates ran on, and that is checkable rather than asserted:
  `git diff --name-only d681956 HEAD` lists 13 files, of which the only `.rs` is the probe in
  `framework/docs/perf-baseline` — a standalone crate, not a workspace member, which is why the `fmt`
  check stays exit 0 with it on disk. On `d1eb68d` itself: `cargo test --workspace` 526 passed / 0 failed
  / 1 ignored across 50 `test result: ok` lines, `cargo clippy --workspace --all-targets -- -D warnings`
  with 0 warning lines, `cargo build --workspace --examples` finished, `cargo fmt --all -- --check` 0
  bytes of output, governance `unittest discover` exit 0, `validate` exit 1 on the same four permanent
  findings (3 `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`). Working tree clean but for
  `.project-governance/tools/tests/__pycache__/`, which is not tracked and was not added.
- **Nothing pushed.**
- **Next without the owner:** track 75 (PROB-0028's label), then 79 (evidence durability — this slice's
  probe is committed *because* the last tracks' detectors were scratch files that got deleted with the
  tree they measured). 76, 77 and 78 stay blocked on the owner or on a machine that is not this one, and
  the pinned worker-count sweep still needs the owner's run.
2026-10-26 (track 73, third slice — **the `fmt` gate is green, and the price was every line number in the
docs**):
Taken from the roadmap's last open step in track 73 (`notes/2026-10-26-remediation-roadmap.md` §2), under
the standing mandate «كمل الشغل لحالك واتخذ قرار بنفسك بما يتوافق مع رؤية المشروع».
- **The decision, and why the softer option was refused.** This track offered itself two honest shapes for
  the 560-hunk debt: ~8 per-crate commits, or one dated sentence in `PRODUCTION_READINESS_REPORT`
  accepting it. The sentence was refused for the reason slice 1 had just established about `validate`: a
  gate deliberately left red carries no marginal information — it reads the same whether the tree got
  worse or not — and every later track compares exit codes, so "accepted debt" would have become exactly
  the noise slice 1 pinned. Markdown and governance notes only in this slice; the Rust changes are the
  seven `fmt` commits already in the history.
- **`cargo fmt --all -- --check` → 0 hunks / 0 files / exit 0**, closed by seven commits along crate
  boundaries: `63b6326` (core, dependency, security, validation, runtime, benchmarks, examples), `a46f6f4`
  (router + websocket), `dbc3b78` (http), `aec827b` (middleware), `018b5a5` (`aether`), `3506038`
  (scheduler), `e7644b9` (the e2e test package). The checkable claim about the split is its shape: `git
  show --name-only` per commit lists exactly the `framework/` directories its subject line names, so no
  commit is the unreviewable repo-wide sweep the standing rule forbids.
- **The cost the batches imposed, measured rather than guessed.** Reformatting preserves content and
  destroys positions, so every doc coordinate naming current code moved. A scratch detector
  (`C:/tmp/t73/drift.py`, and `drift_pre.py` for the before-reading) aligned each touched `.rs` between
  `7bb4356` (last commit before the batches) and `HEAD` with `difflib` over stripped lines and replayed
  every `*.rs:NNN[-MMM]` citation in `framework/docs` + both READMEs through that map: **55 carried
  numbers on the pre-`fmt` docs (56 as printed, before the detector dedups F7's double-cited
  `connection.rs:305–307`), 26 left on the swept tree (27 counted)** — 29 resolved by this sweep. Triage
  was by kind of claim, not by file: "where this lives now" is re-derived from the file; a coordinate
  inside a dated record
  (track entries, the perf-baseline snapshots, the `git show 907dfcd^:` pins, rows that already disclaim
  their own coordinate) keeps the tree it measured, because rewriting that falsifies the record instead of
  the map. **`dc5203a` rewrote 62 coordinate readings** across six shipped docs — 41 attached
  (`ACTIX_FAIR_COMPARISON` 24, `ERROR_HANDLING_GUIDE_AR` 6, `FEATURE_COMPLETENESS_MATRIX` 4,
  `PRODUCTION_READINESS_REPORT` 4, `FULL_FRAMEWORK_DOCUMENTATION_AR` 2, `AETHER_CORE_CAPABILITIES` 1) plus
  21 bare `` `:NNN` `` continuations (17 of them ACTIX) — counted by regex over `git show -U0`, because
  this entry's first draft said "46 (28/6/5/4/2/1)" and that hand count is not reproducible by any rule.
  Every one re-read from the source: the map is a pointer, not an authority — 41 attached rewrites to
  resolve 29 of its flags means 12 coordinates it declared unmoved had in fact moved, and F1's
  `http_e2e.rs:1336` was already dead before the batches landed, invisible to a detector that only reports
  numbers that moved.
- **Two holes in the checker, both found by attacking it, both recorded because the next citation sweep
  will hit them again.** (a) `path/file.rs:NNN` is invisible to the bare form: F6's second wire test is
  cited `` `:756` `` after the file was named earlier on the line, and only a second pass resolving bare
  numbers against the line's own file found it. (b) resolving by basename is a false-positive machine:
  `config.rs:207` inside a row about Actix's `actix-http-3.19.0/src/config.rs` resolves to an in-repo
  `config.rs` and is then reported out-of-bounds on a file nobody meant — the repository has two
  candidates (`framework/aether/src/config.rs`, 191 lines, `framework/scheduler/src/config.rs`, 127), and
  the resolver written twice in this session blamed a different one each time, so even the file the
  checker points at is not a stable reading. Re-measured over `framework/docs` + both READMEs +
  `.project-governance` (28 Markdown files): **474 attached citations, 18 resolving to no file in this
  repository** (Actix registry sources, `aether-probe`, `benchmarks/rust_aether`), **12 reported
  out-of-bounds — 8 the `config.rs:207` collision, four of them this slice's own notes quoting the example
  so the census now counts its own report, and 4 bare `scheduling.rs:NNN` governance citations that land
  on `framework/aether/src/scheduling.rs` (397 lines) when the writer meant the scheduler engine or the
  wire-test file.** The 447 this entry first wrote was the same census before these notes grew;
  `framework/docs` alone carries 72 Markdown citations (73 strings — the extra lives in a saved `.rs`
  artifact), and that is the scope the 55 → 26 drift figures are quoted against.
- **Matrix item 16 changed from a decision to a fact** — "Lints are gated as of 2026-10-26; format is not,
  and that is a decision" → "Lints and format are both gated as of 2026-10-26" — keeping its dated chain
  (574 / 66 → 567 → 558 → 570 / 65 → 560 / 65 → 0) and adding what the green run does **not** buy: there
  is no CI in this repository, so the gate is a command, not an enforcement, and a later slice can
  re-introduce drift silently; and `rustfmt 1.9.0-stable` with **no `rustfmt.toml`** means every choice
  inside the reading is this toolchain's default, not a project decision.
- **Gates, all re-executed on this tree.** `cargo fmt --all -- --check` exit **0** with zero `Diff in`
  blocks. `cargo clippy --workspace --all-targets -- -D warnings` exit **0**, 0 `warning`/`error` lines.
  `cargo test --workspace` exit **0** at **513 passed / 0 failed / 1 ignored** over 50 `test result:`
  lines. `cargo build --workspace --examples` exit 0. `validate` exit **1** with the same four standing
  lines (three `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`), untouched by this slice. Governance Python
  suite `Ran 42 tests` / **OK**, exit 0. **Nothing pushed.**
- **Track 73 is closed: all three of its gates now read what they mean.** The ledger line is the only red
  gate left, and it is red by construction and pinned by slice 1, so the project is still not described as
  green overall. Next without the owner: track 74 (PROB-0016 extension pricing, PROB-0027 as a documented
  rule), 75 (PROB-0028's label) and 79 (evidence durability — this slice's detectors are scratch and will
  be deleted with it, which is the gap 79 exists to close). 76, 77 and 78 stay blocked on the owner or on
  a machine that is not this one.
2026-10-26 (track 73, second slice — **the clippy debt is closed, and the gate it replaces was weaker
than the one now standing: `cargo clippy --workspace --all-targets -- -D warnings` exits 0**):
Continued under the same mandate («كمل الشغل لحالك واتخذ قرار بنفسك بما يتوافق مع رؤية المشروع») and the
same order the roadmap set (ledger → lints → format). Three commits, one per file-locus batch:
`ffa19f5` (`aether` + `aether-router`), `1ca4fd4` (`aether-http` + `aether-middleware` +
`aether-scheduler`), `819fdef` (`aether-security` + `aether-validation` + the `aether-tests` package).
- **The reading before.** The inventory slice 1 measured and recorded: `aether` 13 lib + 16 lib-test,
  `aether-router` 3 + 3, and the dependency crates it had listed but not yet touched — 9 `http/src`,
  7 `middleware/src`, 2 `scheduler/src`. What no per-crate row had ever listed was `security` (8
  records), `validation` (2) and the e2e/test package (23), all of which `--all-targets` over the whole
  workspace does see; they were taken in the same slice rather than left as a second debt.
- **Fixed where the lint describes a real improvement.** Nested `if`/`if let` pairs became let-chains
  (cookie lookup, host parsing, the h2 body charge, the CORS preflight gates, the JWT `exp`/`nbf`/`iat`
  claims, the TLS test's head scan, the load generator's body comparison); `limits_for` builds
  `ServerLimits` as one literal and 20-odd test fixtures build `AetherConfig`/`SchedulingResourceConfig`
  the same way, so a field can no longer be defaulted then forgotten; a catch-all segment is stripped
  with `strip_prefix('*')`; `map_or(false, …)`/`map_or(true, …)` became `is_ok_and`/`is_none_or`; the
  ASCII scan uses `as_chunks::<8>()` instead of `chunks_exact` + `try_into().unwrap()`; the h2 test
  drivers spawn the connection future directly; the scheduling test helper's `stop` lost a
  `map_err(|e| e)` that mapped an error onto itself.
- **Refused where the lint asks for a change the code's own rule rejects — and the refusal is annotated
  with its cost, not silenced.** Five `#[expect(clippy::… , reason = …)]` sites: `Stream`'s TLS variant
  and `ServeOutcome`'s upgrade context (boxing either costs an allocation per connection / per upgrade,
  and `Stream`'s own doc explains why it is an enum), the CORS preflight `if` (a 30-line commented block
  re-indented to satisfy a style lint reads worse in review), the router's 8-argument `search` (a
  per-call aggregate would add a struct without removing state), and `safe_resolve_path`'s
  `Result<PathBuf, ()>`. That last one is the only refusal with a user-visible alternative: giving it a
  named error type is a break for every caller, so it belongs on track 76's breaking list, not in a
  lint-cleanup commit. All five are earned — an `#[expect]` that never fires is itself a warning, which
  is how the `-D` run proves each one still applies.
- **API surface: additive only, checked not assumed.** `parser::ParsedRequest<'req>` is a new `pub` type
  alias for the `(RequestView, Option<Vec<u8>>, usize)` tuple that two public signatures spelled out
  (the alias is what `type_complexity` asked for; naming it beats hiding it). `CompressionMiddleware`
  loses its inherent `default()` and gains the `Default` impl — every call site
  (`app.rs:738` plus 13 in-crate tests) still compiles unchanged because `T::default()` resolves through
  the trait, which is why this is a lint fix and not the rename it could have been. `LoggerMiddleware`
  derives `Default` (both fields were already `false`), with the reason kept as a field comment instead
  of being deleted with the manual impl.
- **`fmt` returned by hand, and the measurement method was itself corrected.** Per file, the HEAD copy
  and the working copy are run through `rustfmt --edition 2024 --check` and the `Diff in` counts
  compared; no touched file ended above its HEAD count except where a fix genuinely removed a hunk, and
  the workspace total moved 570 → **560 hunks over the same 65 files**. Two hazards died on the way:
  an earlier `rustfmt --config-path . … 2>/dev/null` reading reported **0** hunks for `trie.rs` because
  rustfmt was erroring on the argument and its stderr was thrown away — the same abort-early-and-call-it
  clean shape the memory note warns about — and the shape rule for a collapsed chain was derived, not
  eyeballed: rustfmt 2024 splits a chain whose first term is `let` one operand per line, while a chain
  that starts with a bool expression may stay on the line, which is why three hand-collapsed sites had
  to be re-broken and one left inline.
- **One claim of this slice was wrong when it was made, and the gate caught it.** After the last batch
  the JSON-message attribution printed `TOTAL 0` clippy warnings and was about to be recorded as the
  workspace reading. It was filtered to codes starting with `clippy::`, so it silently dropped the
  `E0277` the `as_chunks::<8>()` edit had introduced (`&[u8; 8]` does not `TryInto<[u8; 8]>`): a
  warning inventory is not a compile gate. The `-- -D warnings` run failed on it (exit 101,
  `aether-validation` did not compile), the slice was fixed to `u64::from_ne_bytes(*chunk)`, and the
  only reading now published is the one that fails on any warning *or* error.
- **Gates, all re-executed on this tree.** `cargo clippy --workspace --all-targets -- -D warnings` exit
  0. `cargo fmt --all -- --check` 560 hunks / 65 files / exit 1 — recorded red. `cargo test --workspace`
  **513 passed / 0 failed / 1 ignored** across 50 `test result:` lines, exit 0, and
  `cargo build --workspace --examples` exit 0 (the scheduler `active_trace.rs` edit is in the batch, and
  the suite alone would not have had to compile it). `validate` is untouched by this slice and still
  prints its four standing lines with exit 1; the governance Python suite is still 42 / 0, exit 0.
  **Nothing pushed.**
- **Two live docs were making claims this slice made false, and both are corrected in place with their
  dated figures kept.** `FEATURE_COMPLETENESS_MATRIX.md` item 16 led with "the build is not warning-gated"
  and closed with "there is no `-D warnings` gate"; both are retired there, with the 2026-10-21/22/23
  readings (39 / 46 / 41-for-34, the `unused_io_amount` failure and its `git show 907dfcd^` pin, the
  574→560 format chain) kept as history, and with the item's own invariant-not-a-coordinate rule turned
  back on it: the re-runnable check `cargo clippy -p aether 2>&1 | grep "serve.rs"` now prints *nothing*
  because the crate is warning-free. `PRODUCTION_READINESS_REPORT.md` §5 keeps its 2026-10-21/23 rows and
  gains a dated 2026-10-26 block (the `-D warnings` exit 0, 513 / 0 / 1, the examples build, `fmt` still
  red at 560/65), because its 2026-10-22 conclusion that "lib-scope clippy is the gate that can actually
  be used" is now the weakest of the three. The dated per-track gate blocks in this register and the
  2026-10-22 addenda in report §14/§15 are left exactly as written — they record what those trees
  measured.
- **Next in track 73, and the only owner-shaped question left in it.** The `fmt` half: 560 hunks over 65
  files, unreviewable as one commit and forbidden as a repo-wide sweep, so it is either ~8 per-crate
  commits or one sentence in `PRODUCTION_READINESS_REPORT` accepting the debt. Then tracks 74
  (PROB-0016 extension pricing, PROB-0027 as a documented rule), 75 (PROB-0028's label) and 79 (evidence
  durability), none of which need the owner.
2026-10-26 (track 73, first slice — **the `validate` gate's remaining defect is that it can no longer
detect anything, and it is closed with a ratchet rather than by renumbering or by softening the
detector**):
Taken from the roadmap drafted this session (`notes/2026-10-26-remediation-roadmap.md`, track 73: gate
debt — validate, then clippy batches, then the `fmt` question), under the standing mandate
«كمل الشغل لحالك واتخذ قرار بنفسك بما يتوافق مع رؤية المشروع». Governance tooling only: no Rust file
changed, so the Rust gate identities are untouched by construction (verified, not assumed —
`git diff --stat HEAD -- framework` is empty and the `aether` clippy run on this tree still reports
13 lib + 16 lib-test warnings).
- **The reading before the change, verbatim.** `python .project-governance/tools/project_governance_bootstrap.py validate`
  exits 1 with four lines: `[RECYCLED ID] PROB-0007 was allocated 2 times`, `[RECYCLED ID] PROB-0017
  was allocated 2 times`, `[RECYCLED ID] REQ-0001 was allocated 3 times`, and `[OPEN VS HISTORY]
  REQ-0001 is open now under an ID that earlier items already used`. The three recycles are facts in
  the append-only `event_log.jsonl` (log lines 1/4/9 for REQ-0001) produced by the allocator that
  track 53 fixed; erasing them means rewriting history, which the project rules refuse.
- **The option that looked like the fix, and why it was rejected.** Renumbering the *live* requirement
  is technically open — `REQ-0002` turns out to be a genuine closed allocation (log lines 5 and 7), so
  `REQ-0003` is a truly never-used number, and `next_id()` already proves it. It was rejected on
  evidence: it clears one of the four lines while `validate` still exits 1, and it does so by editing
  the 20+ dated citations of `REQ-0001` across `WORKING_CONTEXT.md`, the improvement register and the
  roadmap — every one of those is a record of what a past track measured, and rewriting them to say a
  number that did not exist then falsifies the record to make a gate line disappear. Tracks 53/54
  reached the same conclusion in-repo ("No shipped doc is renumbered", `WORKING_CONTEXT.md:1976-1977`),
  and this slice honours it. The live ambiguity is already disambiguated at the point of use: the
  `validate` run itself prints all three REQ-0001 allocations with their timestamps and titles.
- **What the gate actually lacks.** A permanently-red check has zero marginal information: a *fourth*
  recycle, or a second open item on a reused ID, hides inside the same exit 1 and the same four lines,
  so nobody could tell "nothing happened" from "a new integrity break happened". That — not the red
  itself — is the remaining defect. Closed by pinning the standing set, leaving `validate`'s code,
  output and exit semantics untouched: no acknowledgement list, no void entry, no severity split, so
  nothing here can excuse a future reuse.
- **The ratchet.** `.project-governance/tools/tests/test_recycling_ratchet.py`, 5 cases. It pins
  `STANDING_RECYCLED = {PROB-0007, PROB-0017, REQ-0001}` and `STANDING_OPEN_ON_RECYCLED = {REQ-0001}`
  against the repository's own ledger by *set equality*, cross-checks the printed line against the
  data by recomputing the recycled set from `ledger_allocations()` independently of the printer, pins
  that no `[UNLOGGED ID]`/`[DUPLICATE ID]`/`[DUPLICATE TITLE]` flag is standing, proves the detector
  fires on new data (a copy of the real ledger with two forged second allocations — a closed ID, which
  only the ledger can see, and an open one, which must join both flags — must report exactly
  `STANDING ∪ {PROB-0030, PROB-0028}` recycled and `STANDING ∪ {PROB-0028}` open), and states the
  refusal in the last case: keep one allocation per ID and the same validator goes `[VALID]` / exit 0,
  i.e. pruning history is the only way to silence the gate, which is precisely why history stays.
- **Seven mutants, each killed by a named test** (`C:/tmp/t73/mutants.py`, scratch; every run restores
  the file byte-identically and asserts the mutation landed before believing the result): M1 drops
  REQ-0001 from the baseline → 3 failures; M2 adds a phantom to the open baseline → 2; M3 asserts the
  real ledger is green → 1; M4 omits the reused-and-open forgery from the recycled union → 1; M5 omits
  it from the open union → 1; M6 adds a phantom recycled member (PROB-0012, open and single-allocated)
  → 2; M7 asserts the pruned copy stays red → 1. M4/M5 are the second version of this slice: the first
  fourth-reuse check asserted membership plus a subset relation and **survived** being weakened, which
  is how the exact-union form on both flags was arrived at rather than assumed.
- **Suite and gates.** The tool's own suite goes 37 → **42** green under the documented command
  `python -B -m unittest discover -s .project-governance/tools/tests` (26 `test_id_allocation` +
  11 `test_ledger_location` + the 5 new), exit 0. `validate` on the repository is unchanged and still
  **red**: exit 1, the same four lines — recorded as red, not described as green. Rust gates hold at
  their standing readings: `cargo fmt --all -- --check` 570 hunks / 65 files, clippy `aether` 13+16,
  `aether-router` 3+3, `aether-websocket --all-targets` 0, suite 513 / 0 / 1 (50 result lines).
  **Nothing pushed.**
- **Next in this track, already measured.** The clippy half of the debt, enumerated from
  `cargo clippy -p aether --lib --message-format=json` and attributed by source path (a `--lib` run
  prints the path-dependency crates' warnings too, so the 34 records it emits are 13 `aether/src/*`
  + 9 `http/src/*` + 7 `middleware/src/*` + 3 `router/src/*` + 2 `scheduler/src/*`; the `aether/src/*`
  set is exactly the gate's 13). `aether` lib: 9× `collapsible_if` (`app.rs:558,1991,2638`,
  `http2.rs:448,493,603`, `serve.rs:222,483,499`), 2× `large_enum_variant` (`serve.rs:87`,
  `transport.rs:23`), 1× `empty_line_after_doc_comments` (`extract.rs:107`), 1×
  `field_reassign_with_default` (`app.rs:1356`). The 16 lib-test figure is those 13 plus 3 more
  `field_reassign_with_default` (`app.rs:2739,2746`, `http2.rs:899`). `aether-router` lib's 3 are
  `collapsible_if` `trie.rs:288`, `manual_strip` `trie.rs:104`, `too_many_arguments` `trie.rs:261`.
  No Rust test count moves from any of it, and a fix that changes a line must return the `fmt` hunk
  delta by hand as track 71 did.
2026-10-26 (track 72 — **PROB-0030 closed: a WebSocket session whose peer stops consuming is now
failed by a write deadline, and the buffers it was priced against are released. The defect track 69
registered as "read from the code, not reproduced" was reproduced at two levels first**):
Continuing the owner's standing mandate («اتخذ قرار بنفسك وحل مشاكل لو في شوف منطق الكود»), this track
took the registered exposure whose consequence is a budget the scheduler can never reclaim. No
performance claim is made anywhere on it.
- **Reproduced before anything was changed.** Deterministic level: a `tokio::io::duplex(8)` peer that
  never reads leaves `send_text` parked forever — the session task, its slot and its registration all
  stay. Wire level: a real TCP client that reads only the `101` and nothing after it, against a server
  with `read_timeout: Some(300ms)` and a handler pushing 1 MiB frames; six seconds in, the scheduler
  still reported `active=1 registered=33603584` (`C:/tmp/t72/probe_b.log`, scratch — the number that
  outlives this note is the assertion in the wire test, not the log). The 33,603,584 B is
  `max_message_bytes + max_frame_bytes + CONN_BASE_MEMORY`; the 33,570,816 B the docs quote is the
  buffer pair alone. Nothing in `recv`'s deadline could ever fire, because a task sitting in a write
  never reaches a read.
- **Shape chosen from in-repo precedent, not invented.** `WebSocketLimits::write_timeout`
  (`connection.rs:35-39`, `Some(10s)` default at `:54` — the same figure `ServerLimits::write_timeout`
  already uses for the HTTP/1 response and for the `101` handshake write, so the asymmetry inside one
  framework closes rather than moves) + `WsError::WriteTimeout` + a `WriteBudget` (`:453`) that
  `write_bounded`/`flush_bounded` (`:481`/`:502`) hold against `timeout_at`. It bounds **an interval,
  not a frame**, and renews only once `MIN_PROGRESS_FOR_RENEWAL` = 1024 B (`:449`) has actually left —
  the earned-renewal rule track 67 settled for HTTP/2 at `http2.rs:521`. An absolute per-frame cap was
  the weaker option and is refused: it would fail a normally-consuming peer mid-message on a slow link.
  `None` disables the direction, matching `read_timeout`.
- **API shape, checked rather than assumed.** `WebSocketLimits` is not `#[non_exhaustive]`, so any new
  field breaks out-of-tree struct-literal construction. A private field with a builder breaks it
  harder (E0451 plus functional-update rules), so the public field is the least-breaking shape. The
  blast radius on this host is empty: no source-level reference to `WebSocketLimits` or `WsError` in
  `my_cool_site`/`threekway` (the path-dep consumer), `aether-probe`, `aether-bench-compare` or
  `benchmarks/rust_aether` — only build artifacts matched. Compiled on the post-fix tree: `my_cool_site` and
  `aether-bench-compare` both `cargo check` → exit 0 (each with warnings of its own); `aether-probe` and
  `benchmarks/rust_aether` fail `cargo check` for the pre-existing unannotated-`get_async` inference reason
  recorded since the Developer-Experience round, not for a symbol this track touched.
- **Four tests, and each of the three mutants is caught by a different one** (logs in
  `C:/tmp/t72/mutants*.log`, scratch): M1 (drop the bound entirely) fails the stall test, the trickle
  test and the wire test; M2 (renew on any wake, i.e. an absolute cap per frame) fails
  `a_peer_that_reads_slowly_gets_the_whole_frame` — the only test that proves a slow-but-consuming peer
  still gets its message; M3 (renew on *any* progress, however small) fails
  `a_peer_that_trickles_one_byte_per_interval_is_shed`. Coverage is non-overlapping, so no test is a
  duplicate of another.
- **Two test-shape defects found by running the mutants, not by writing them.** M2 first **hung** the
  suite: the paced reader's drain loop had no EOF exit, so a writer that gave up left the test spinning
  — a detector that hangs is worth nothing, it now breaks on `Ok(0)` and fails. M3 first **survived**:
  the trickle finished its 20 drains before the assertion looked, so the parked writer was judged after
  it had already been given up on; the test now races the ongoing trickle against the send future with
  `tokio::select!` and M3 dies. A mutant that passes is a hole in the test.
- **Gates, re-measured on the final tree.** `cargo test --workspace` exit 0 at **513 passed / 0 failed
  / 1 ignored**, 50 result lines = 295 unit + 213 integration + 5 doc (from track 71's 509:
  `aether-websocket` lib 25 → 28, `scheduling` 17 → 18); the timing tests were repeated 5/5 green
  before the recount, and the two changed binaries re-ran green after the hand-formatting below. Re-run once
  more on the committed tree (`e9ed605`): 50 `test result:` lines, **513 / 0 / 1** again.
  `cargo fmt --all --check` exit 1 at the standing **570 hunks / 65 files** — the first run read 574,
  all four inside this track's new `scheduling.rs` test (one `let server = …` split, three long
  assertions), hand-corrected to rustfmt's shape rather than reformatting 65 files. `cargo clippy -p
  aether-websocket --all-targets` is **clean (0 warnings)**; `-p aether --all-targets` stays at its
  recorded 13 lib + 16 lib-test and the `scheduling` target's 2 warnings pre-date this track — none in
  a track-72 range. `validate` exit 1 with the same three `[RECYCLED ID]` rows plus
  `[OPEN VS HISTORY] REQ-0001`, no new violation. `status` recount: Problems 9 → **8**.
  `fmt`, `clippy` and `validate` **stay red** for those recorded reasons — the project is not green
  overall.
- **Left as found, stated rather than implied.** The bound prices an interval of non-consumption, so a
  peer that pays 1024 B per interval keeps an arbitrarily long session by design — that is what an
  interval bound must leave open; the total frame time is not capped. Nothing was measured over a real
  slow network or behind TLS (PROB-0012), and a partial frame written before the deadline is still a
  partial frame on the wire (`write_frame`'s doc records it). The registration release is asserted on
  the wire, the *task* abort itself is the existing handover path's, unchanged by this track.
- **Still open after this track**: PROB-0027 (owner-backed heap the budget cannot name), PROB-0012
  (first Linux run), PROB-0016 and PROB-0028 (pricing decisions), PROB-0011/0005/0009's breaking half,
  the pinned same-session worker-count sweep (task #69), and the unattributed p50 gap. The sequencing
  of all of them is in `.project-governance/notes/2026-10-26-remediation-roadmap.md`.

2026-10-26 (track 71 — **PROB-0031 closed: a radix `Param` level shared by sibling routes no longer
answers both handlers with the first-registered capture name. The defect track 69 registered as
"read from the code, no wire test yet" now has a first-hand reproduction and a fix**):
Continuing the owner's standing mandate («اتخذ قرار بنفسك وحل مشاكل لو في شوف منطق الكود» and «Ok تتخذ
قرة»), this track took the largest registered logic defect whose close is a contained router change.
No performance claim is made anywhere on it.
- **Reproduction came first, on the unmodified tree.** A `trie.rs` probe registering
  `GET /users/{user}/edit` and `GET /users/{id}` failed with `left: None, right: Some("42")` — the
  request matched `/users/42` (handler 20) but the capture arrived under `user`, the
  first-registered name, because `RouteNode::param_names` was `Vec<(Method, String)>` keyed by
  method alone and `param_name_for(GET)` answered the same name for both routes. `reject_duplicate`
  never saw it: it compares method+handler at the leaf, and these are different paths.
- **The fix records the name where only the leaf can know it.** `param_names` is now
  `Vec<(Method, Vec<String>)>` and `add_route` threads the route's ordered dynamic names
  (`declared`) onto the node that holds its handler; the two `Search::Found` sites
  (leaf + catch-all) call the new `RadixRouter::resolve_keys`, which restamps each `Param.key` from
  that list before the params are cloned out. The key pushed while *descending* is only a
  placeholder — the shared `Param` node cannot know which sibling route will win — so
  `resolve_keys` runs against the clone made while the leaf node is still in scope, keeping the
  borrow of the trie (`&'a str`) intact and the zero-alloc `SmallVec` shape unchanged.
  `param_name_for`/`set_param_name` are gone; nothing outside `router/src` referenced them.
- **Three tests, each mutation-proven against the pre-fix tree** (`C:/tmp/t71/`):
  `sibling_routes_on_one_param_node_capture_under_their_own_names` and
  `multi_param_siblings_restamp_captures_in_declaration_order` (`trie.rs`) both FAIL (2 failed) when
  node.rs+trie.rs are reverted to HEAD, so they are not carried by the fix — they require it. The
  second one exists because the first is single-parameter: a reversed name↔capture zip (mutant M3)
  passes every one-param test, so `/a/{x}/{y}` vs `/a/{one}/{two}/z` pins positional pairing.
  `sibling_same_method_param_routes_keep_their_own_capture_names` (`extractors.rs`) drives the same
  shape over the real serving path and fails (1 failed) on the reverted tree, proving the restamped
  key survives the `serve.rs:262-266` param copy into `RequestContext::param`.
- **Mutant readings.** M1 (delete the leaf restamp) — killed by 3 tests; M4 (drop `declared`
  registration) — killed by 1; M3 (reverse the zip) — **initially undetected**, which is why the
  two-param test was added; after it M3 fails as required. A mutant that passes is a hole in the
  test, not a free pass for the code.
- **Left as found, stated rather than implied.** Cross-*method* naming (`/items/{id}` GET vs
  `/items/{item}` POST) was already correct and its two tests stayed green untouched. The catch-all
  placeholder path is exercised by the catch-all `Found` site's own `resolve_keys`, not by a new
  test — the existing catch-all tests confirm the value but the cross-name catch-all key collision is
  symmetric with the param case and shares the same restamp.
- **Gates, re-measured on the final tree.** `cargo test --workspace` exit 0 at **509 passed / 0
  failed / 1 ignored**, 50 result lines = 292 unit + 212 integration + 5 doc (from track 70's 506:
  `aether-router` lib 22 → 24, `extractors` 18 → 19). `cargo fmt --all --check` exit 1 at the
  standing **570 hunks / 65 files** — the first run read 573, and the three new hunks were this
  track's own added lines (two long `assert_eq!`s in `extractors.rs`, one stray EOF blank in
  `node.rs`), hand-corrected to rustfmt's shape so the track adds no fmt debt and pre-existing
  drift in those files was not reformatted. `cargo clippy -p aether --all-targets` at **13 lib + 16
  lib-test**, `-p aether-router` at **3 lib + 3 lib-test** (the 8-arg `search` and the two pre-existing
  `trie.rs` lints — none in a track-71 range); `resolve_keys` added no finding. `validate` exit 1 with
  its three pre-existing `[RECYCLED ID]` rows and `[OPEN VS HISTORY] REQ-0001`, no new violation.
  `status` recount: Problems 10 → **9** after PROB-0031 closed. `fmt`, `clippy` and `validate`
  **stay red** for those recorded reasons — the project is not green overall.
- **Still open after this track**: PROB-0027 (owner-backed heap the budget cannot name), PROB-0012
  (first Linux run), PROB-0028 (advertised limit vs wire budget — a pricing decision), PROB-0030
  (WS `write_frame` has no deadline — read from code, not reproduced), the pinned same-session
  worker-count sweep (task #69), and the unattributed p50 gap.

2026-10-26 (track 70 — **PROB-0029 closed: an over-budget request is now answered, not reset. The
diagnosis track 69 registered was wrong in one respect — the FIN *was* being sent — and the missing
piece was discarding the peer's queued bytes before the socket closes**):
Continuing the owner's standing instruction from track 69 («اتخذ قرار بنفسك وحل مشاكل لو في شوف منطق
الكود»), this track took the largest registered logic defect whose close is a code fix rather than a
scope decision. No performance claim is made anywhere on it.
- **The reproduction came first, on the unmodified tree**: a chunked upload of 400 000 payload bytes
  (2,400,000 wire bytes against the 2,105,344-byte default connection budget) failed in 0.15 s with
  `os error 10053` and `upload write: Ok(())` — the client had queued its whole upload and read
  nothing. Same test after the fix: green in 0.14 s (`C:/tmp/t70_probe_before.log`,
  `C:/tmp/t70_probe_after.log`).
- **Why a graceful FIN was not enough, and what is.** `break` already fell through to
  `framework/aether/src/app.rs:2206`, which shuts the stream down — so the response *was* preceded
  by a FIN. Windows still aborts a socket closed with received data left unread, and the abort
  discards the response. The fix is the discard: `refuse_over_budget` (`app.rs:2466`, called from the
  static bound at `:2158` and the re-price refusal at `:2188`, replacing two inline copies) writes the
  413 + `Connection: close`, releases the refused request's blocks, shuts the write half, then reads
  the peer's queued bytes into the existing 8 KiB buffer and throws them away until EOF/error, bounded
  by `read_timeout`.
- **The DoS objection that made track 69 register this instead of fixing it does not apply to this
  shape**: the bound protects *memory*, and a discard that allocates nothing while returning the 2 MiB
  request block before it starts waiting strictly reduces resident bytes. The remaining exposure is
  task/permit occupancy for at most `read_timeout` — already the exposure the same loop accepts for a
  trickling request in `Ok(None)`. Option (b) (close and document that the request may be reset) was
  rejected because the code would keep claiming an HTTP answer it cannot deliver; option (c) describes
  the current behaviour, since the refusal already happens on the first read past the budget.
- **Three tests, three claims, with their mutant readings** (`C:/tmp/t70_mutants.log`,
  `C:/tmp/t70_mutants2.log`): `an_over_budget_chunked_upload_is_still_answered_with_413`
  (`http_e2e.rs:1450`) fails 4/4 without the drain; `a_buffer_limit_refusal_finishes_the_exchange_
  without_waiting_for_the_client` (`http_e2e.rs:1523`) fails 4/4 if the FIN is moved after the drain
  (it reads answer + close in ms, and 2.02 s without it) and 4/4 without the drain;
  `a_memory_budget_refusal_reaches_a_client_that_is_still_sending` (`scheduling.rs:741`) pins the
  re-price site's 413, `Connection: close`, no-handler and `memory_shed/completed/cancelled` tallying,
  and caught the missing drain in 3 of 4 repeats — recorded as the weaker detector, not as the proof.
  **Left unverified on purpose and said so in the code doc comment:** releasing the refused blocks
  before the drain (mutant M3) is not observable by any test — no test samples RSS — so that claim is
  argued from dropping a `BytesMut`.
- **PROB-0028 stays open, and reading the code moved it out of the pricing column.** `consumed` is
  returned only with a completed request (`app.rs:1685-1690`) and applied only at `:1966`/`:2009`, so
  while a body is incomplete every wire byte is resident next to the decoder's copy (already charged
  at `:2185`). The wire bound is therefore correct memory pricing; what is wrong is advertising the
  enforced ceiling as `max_body_bytes`, which reads as decoded bytes. Its close is a naming/doc or
  framing-cap decision, not a pricing change.
- **The count sweep this delivery owed the live docs.** `README.md`, `framework/README.md`,
  `framework/docs/BASELINE_MEASUREMENTS.md` and `framework/docs/FEATURE_COMPLETENESS_MATRIX.md` were
  re-derived from the same 50 `test result:` lines (506 / 211 / `http_e2e` 51 / `scheduling` 17), and
  the matrix's `aether`-lib split line was still self-inconsistent: it said "the 47-test `aether` lib
  total" while listing 10 + 17 + 8 + 12 = 47 and omitting `transport.rs` 3 — recounted from the
  attributes, 50. `PRODUCTION_READINESS_REPORT.md` had no current total at all: its chain ended at
  "the sixth describes the tree in the repository" (474, 2026-10-23), so a seventh reading was added
  from `C:/tmp/t70_suite_final.log` and that sentence marked superseded in place; §10's raw-socket
  count went 49 → 51. Dated historical counts were left as written.
  `SCHEDULING_COMPARISON.md:13-14` described its 2026-10-18 reproduce block in present tense ("8
  wire-level integration tests", "17 engine tests + 8 simulator") — those commands now report 17 and
  57, so each claim got its own date rather than a silent correction; the tables in that doc are dated
  2026-10-18 measurements and were not re-run or altered.

- **Gates, re-measured on the final tree.** `cargo test --workspace` exit 0 at **506 passed / 0 failed
  / 1 ignored**, 50 result lines = 290 unit + 211 integration + 5 doc (`http_e2e` 49 → 51,
  `scheduling` 16 → 17; every in-crate lib target unchanged). `cargo fmt --all --check` exit 1 at the
  standing 570 hunks / 65 files. `cargo clippy -p aether --all-targets --no-deps -- -D warnings` exit
  101 with the identical 13 lib + 16 lib-test histogram, `-p aether-router` exit 101 with the same 3.
  `validate` exit 1 with the same four lines. Ledger after the close: Requirements 1 | Problems **10**
  | Proposals 1 | Criticisms 0. `fmt`, `clippy` and `validate` are still red for these recorded
  reasons; the project is not green overall.
- **Re-verified on the committed tree** (three commits, `git status` clean, no Rust edit between the
  measured run and the code commit): `cargo test --workspace` exit 0 at **506 / 0 / 1**, 50 result lines
  (`C:/tmp/t70_head_suite.log`); `validate` re-run after the markdown edits → exit 1, same four lines,
  Problems still **10** (`C:/tmp/t70_validate3.log`). The drain was deliberately **not** extended to the
  WebSocket reprice 503 (`app.rs:1914`) or the parse-error path: same shape, but their backlog is a
  parsed head rather than a body in flight, and no failure was measured there. `refuse_over_budget` also
  kept the inline code's `write_http1_to`, so a 1.0 client still reads `HTTP/1.1 413` — the deviation
  `PRODUCTION_READINESS_REPORT.md:166-167` already records, inherited here rather than introduced.

2026-10-26 (track 69 — **the code-logic read found four defects that were each silently doing the wrong
thing on a real connection: a split HTTP/2 preface was classified as HTTP/1, an HTTP/1.0 request could
switch protocols, the WebSocket budget refusal neither closed its socket nor settled its guard, and a
mid-path catch-all swallowed its own prefix. All four are fixed with mutation-proven tests; four more
findings were measured and registered instead of patched**):
The owner's message on this track was «اتخذ قرار بنفسك وحل مشاكل لو في شوف منطق الكود» — decide for
yourself and fix the problems if there are any, look at the code logic. No performance claim is made
anywhere on this track.
- **1) The classifier's second `peek` re-reported bytes it had already stored.** `classify`
  (`framework/aether/src/transport.rs:237`) peeked into `&mut probe[filled..]` and did `filled += got`,
  but `MSG_PEEK` always answers from the socket's *first unread byte* — so an h2c preface arriving in two
  segments became `P` + `PRI * HTTP/2.0…`, which is not the preface, and a real HTTP/2 client was handed
  to the HTTP/1 parser, which answers `PRI` with a 400 on a socket the peer believes is HTTP/2. It now
  peeks into a separate `scratch` and copies only the grown tail, and waits for a stalled discriminator
  up to `CLASSIFY_RETRY_LIMIT` (4) × `CLASSIFY_RETRY_SLACK` (25 ms) ≈ 100 ms — bounded because a
  non-consuming peek is always ready, so an unbounded wait is a hot loop on one worker. Three tests
  (`transport.rs:352/:395/:426`); the first draft of the split-preface test **passed against the buggy
  classifier** (it awaited `classify` after both writes) and only became a proof once it asserts no
  answer existed before the second segment — the vacuous-test class again.
- **2) `allows_upgrade` was not a version test.** A `GET /ws HTTP/1.0` with the upgrade headers reached
  the WebSocket arm and got the framework's hardcoded `HTTP/1.1 101 Switching Protocols` — a 1.1 status
  line to a 1.0 client, for an upgrade mechanism HTTP/1.0 does not have (RFC 9110 §6.7). Now
  `allows_upgrade: version == Version::Http11` (`app.rs:1784`); the new `http_e2e` test asserts `HTTP/1.0`
  **400** + zero sessions + closed socket for the 1.0 head and still `101` + one session for the same
  head over 1.1.
- **3) The 503 that refuses a WebSocket for want of memory did not close the socket or settle the
  guard.** No `Connection: close` (a reused connection parses its next request as WS frames), no
  `shutdown()` (on Windows a bare drop RSTs away the answer the peer has not read — the hazard
  `error_responses_survive_unread_pipelined_bytes` exists for), and the guard fell through to
  *cancellation* while the identical 400/413/431 rejections counted as *completed* (`app.rs:2090-2096`).
  `websocket_buffers_are_charged_to_the_memory_budget` now asserts `connection: close`,
  `completed >= 1` and `cancelled == 0`.
- **4) A catch-all declared before the last segment registered silently and matched everything.**
  `/files/*rest/trailer` attached its handler to the `*rest` node, so every `/files/…` request ran it and
  the declared tail was never compared — captured against the pre-fix code, where `/files/a` matched it.
  `add_route` now panics at startup the way a duplicate method+path does (`trie.rs:157`, precedent at
  `:154-161`), with a terminal-catch-all test alongside so the loud failure cannot be bought by breaking
  the working case.
- **Registered, not patched, each because the close is a decision and not a fix**: **PROB-0028** (a
  chunked body is refused by a *wire*-byte budget while the advertised limit counts *decoded* bytes —
  380,000 payload bytes = 2,280,063 wire got a 413, i.e. ~18 % of the advertised 2 MB; decode stayed
  linear at 23/45/120/171 ms for 50k/100k/200k/300k, so it is a pricing mismatch, not the quadratic item;
  left alone because connection memory is priced by capacity, PROB-0008), **PROB-0029** (that 413 is
  *delivered* at 2,280,063 wire bytes and never at ≥2,400,063 — the client reads nothing and gets Windows
  `os error 10053` after ~135-150 ms; the three candidate closes are a DoS-posture choice),
  **PROB-0030** (`write_frame` at `websocket/src/connection.rs:376-378` has no deadline, so a peer that
  pings and stops reading parks a session holding the ~33.6 MB registration — read from the code, **not
  reproduced**), **PROB-0031** (one node binds one param name per method, so `/users/{id}/edit` and
  `/users/{user}` report each other's key; `reject_duplicate` does not catch it because the paths differ
  — read from the code, **no wire test yet**). The minor findings (`error_status` unclamped at
  `app.rs:190`, unconditional `X-Request-ID` push at `serve.rs:481`, `Payload Too Large` vs
  `canonical_reason(413)` = `Content Too Large`, `runtime/src/worker.rs:24-34` discarding `_consumed`)
  are recorded in the improvement register's track-69 section.
- **Gates, re-measured on the final tree.** `cargo test --workspace` exit 0 at **503 passed / 0 failed /
  1 ignored**, 50 result lines = 290 unit + 208 integration + 5 doc (from track 68's 497: `aether` lib
  47 → 50, `aether-router` lib 20 → 22, `http_e2e` 48 → 49; `scheduling` stayed 16 — rewritten, not
  added). `cargo fmt --all --check` exit 1 at the standing **570 hunks / 65 files**, unchanged — the
  track added no fmt debt. `cargo clippy -p aether --all-targets --no-deps -- -D warnings` exit 101 at
  **13 lib + 16 lib-test** with the same finding *identities* as the baseline, none in a track-69 range;
  `-p aether-router` exit 101 at the same 3 lib-test findings. `validate` exit 1 with its three
  pre-existing `[RECYCLED ID]` rows and `[OPEN VS HISTORY] REQ-0001` — no new violation from
  PROB-0028…0031. `fmt`, `clippy` and `validate` **stay red** for those recorded reasons.
- **Still open after this track**: PROB-0027 (owner-backed heap the budget cannot name), PROB-0012 (first
  Linux run), the pinned same-session worker-count sweep (task #69), the unattributed p50 gap, and the
  four PROB-0028…0031 decisions above.

2026-10-26 (track 68 — **PROB-0023's pricing residuals were measured against `bytes` itself instead of
described: three are the right answer, one is a real gap that left the closure with its own ID, and the
stale registration on the refusal path is fixed. Also: the clippy reading three earlier tracks relied on
turns out to have never linted this file**):
The owner's message on this track was «اخذ قرارا» — take a decision. The decision was PROB-0023: it is
the only memory-budget problem left that this host can settle without a measurement the owner has to
take, and its description had been asserting four body shapes since 2026-10-24 with none of them ever
run. Reading the code answered three of them and falsified one expectation of mine.
- **The rule came from the dependency, not from a guess.** `price_response_body`
  (`framework/aether/src/http2.rs:658`) prices a block only through `Bytes::try_into_mut()`, which
  succeeds iff `is_unique()`. In the vendored bytes 1.12.1 (`src/bytes.rs`): `static_is_unique` is
  `false`, `owned_is_unique` (`:1140`) is `false` **unconditionally**, a VEC/promotable kind is always
  unique, an ARC kind only at refcount 1; `Bytes::slice` promotes VEC→ARC (so the sibling must be
  dropped before the pricing can reclaim it); `advance_unchecked` does `self.cap -= count` and
  `BytesMut::capacity()` returns `self.cap` — which is why the *direction* a body was sliced in
  decides what gets priced.
- **Four unit tests now pin that on the function, not on the wire** (`http2.rs`, `mod tests`):
  `a_body_the_stream_solely_owns_is_priced_by_its_allocation_not_by_a_copy` (`:1506`) — 128 KiB body in
  a ≥640 KiB allocation, registration = base + `capacity()`, and `as_ptr` equality proves the payload
  was reused, not copied; `a_body_sliced_from_a_larger_block_is_priced_only_up_to_its_end` (`:1556`) —
  both directions, a tail-dropping slice priced at `SLICE` (192 KiB, the 64 KiB prefix unpriceable) and
  a head-keeping slice priced at the whole `BLOCK` (256 KiB); `the_bodies_left_unpriced_are_the_static_shared_and_owner_backed_ones`
  (`:1636`) — static, retained-clone and `Bytes::from_owner` each stay at base, each with pointer
  identity, and the owner-backed one asserts `!body.is_unique()` for its *sole* handle; and
  `a_refused_upload_stops_registering_the_buffer_it_dropped` (`:1780`) for the fix below. Helper:
  `stream_charge` (`:1488`) builds the same `StreamCharge` `Charge::acquire` produces, so every reading
  is exact rather than polled.
- **Verdict: no client-triggerable under-pricing on this path today.** Every body the framework
  produces is a uniquely-owned VEC block — compression returns `Bytes::from(encoder.finish())`
  (`framework/middleware/src/compression.rs:120,134`), `static_dir` returns `Bytes::from(content)`
  (`framework/aether/src/static_dir.rs:48`), embedded assets are `Bytes::from_static` — so the priced
  branch is the one handlers actually take. Static and shared non-charges are correct by construction,
  not gaps: `.rodata` is not per-stream heap, and a block another handle retains belongs to that
  holder's accounting — charging it here would price one cached allocation once per reader. The one
  real hole is `Bytes::from_owner`, and it is not closeable without either an API break or the copy
  the charge exists to avoid → registered as **PROB-0027** with its own pre-declared, unrun check.
- **My own prediction was wrong, and the run said so.** I asserted a suffix slice would be priced by
  the whole allocation; the test failed with `left: 245760, right: 311296` (245760 − 49152 base =
  192 KiB = the slice), because `advance_unchecked` shrinks `cap`. The older doc prose ("priced from
  its first byte to the end of that buffer") was the accurate one; the test became two-directional and
  the prose now names which half is unpriced. `price_response_body`'s doc was rewritten to match
  bytes' real rules rather than the summary in it.
- **The one production change.** `read_body` was split so the invariant has one exit: the loop moved
  to `read_body_buffer` (`:412`) and `read_body` (`:389`) calls `charge.buffer_set(0)` on every `Err`,
  at the point the refused buffer dies, before the error answer is written and before the next
  admission decision is taken. `enum Refusal` gained `#[derive(Debug)]` for the `.expect()` the new
  test needs. **Severity, stated honestly:** every framework error answer is payload-less
  (`AetherResponse::status` sets `body: Bytes::new()`, `framework/http/src/response.rs:126`), so
  nothing parked the stream task and the stale figure lived for one queued HEADERS — invisible on the
  wire, which is why this was a rule violation and not a measured leak. `send`'s doc
  (`:546-556`) now says which of its `charge = None` answers follows a refused read (413/408: released)
  and which follow a successful one (431/500: the request block is still held and still charged — the
  set-semantics approximation, not an over-registration).
- **The refusal test drives a real `h2` stream over `tokio::io::duplex`** (`duplex_upload`, `:1721`),
  because the wire cannot show this: the release happens before any sample a wire test could take.
  Both directions run in one test — a refusal drops the buffer's registration, a successful read keeps
  its block charged while the bytes live — since `buffer_set(0)` on *every* exit would pass the first
  half alone. Three harness facts learned by hitting them: `h2::server::Connection` is not a Future in
  0.4.20, so the server task drives `while conn.accept().await.is_some() {}`; `server.await`/
  `client.await` hang because the server keeps polling accept, so both are `abort()`ed; and a
  `&[usize]` of chunk sizes cannot escape into the spawned task (E0521), so the helper takes a `Vec`.
- **Mutation evidence** (five mutants, each reverted; `grep -c MUTATION framework/aether/src/http2.rs`
  → 0 after the runs): `capacity()` → `len()` killed three tests, including the pre-existing wire test
  `a_response_body_is_registered_by_its_block_while_the_stream_holds_it` (`left 1097728, right
  1359872`); charging by length on the `Err` path killed the static case (`left 53248, right 49152`);
  `block.freeze()` → `Bytes::copy_from_slice(&block)` killed **only** the new pointer asserts — every
  registration number stayed correct and the existing wire test passed, which is the evidence that
  pointer identity adds coverage the wire tests do not have; no reset on refusal killed phase 1; an
  unconditional reset killed phase 2 (`a buffer this stream still holds must stay registered: 49152
  for a 4096-byte body`).
- **PROB-0023's pre-declared check ran, in an adapted instrument, and is recorded that way.** It asked
  for "a wire test in which a handler returns a `Bytes` whose block is also retained elsewhere … and
  `registered_memory_bytes` stays at CONN_BASE + STREAM_BASE while a counting allocator shows the body
  live". What was run is a direct call under a real `StreamCharge`, with **pointer identity** standing
  in for the counting allocator and the stream-side shapes covered by a duplex `h2` connection instead
  of a listening socket. The substance of Confirm is met for the shared and owner-backed shapes and
  nothing came out Refute-shaped, so the residuals that are correct-by-design were closed rather than
  re-argued, and the one that is not left under a fresh ID. No ID was recycled.
- **A gate reading three earlier tracks published is void** (register **B21**). "clippy exit 101 with
  no finding naming `http2.rs`" came from `cargo clippy --workspace --all-targets`, which on this tree
  stops inside `aether-validation`/`aether-scheduler`/`aether-http` and never prints `Checking aether v`
  (0 occurrences in the 275-line capture) — it cannot have linted the file. The reading that does:
  `cargo clippy -p aether --all-targets --no-deps -- -D warnings` → exit 101, **13 findings on the lib,
  16 on the lib test, 4 inside `http2.rs`**: `collapsible_if` at `:448` (the per-frame reprice), `:493`
  (authority→`host`), `:603` (default `date`) and `field_reassign_with_default` at `:899` (an existing
  limits test). All four constructs are verbatim in `git show HEAD:framework/aether/src/http2.rs`, so
  none came from this track, and none were fixed — that is a lint sweep, not a pricing change. The
  pre-existing count was established by comparing constructs, not by clippy over an exported HEAD tree.
- **Coordinates and counts re-derived by symbol.** This slice inserts +27 at `read_body`, +2 in `send`'s
  doc and +10 in `price_response_body`'s doc, so below those points everything moved: `MIN_PROGRESS_FOR_RENEWAL`
  `:493` → `:521`, `send` `:526` → `:556`, `price_response_body` `:618` → `:658`. Live cites updated in
  `FEATURE_COMPLETENESS_MATRIX.md` (bounded-response-memory row) and `PRODUCTION_READINESS_REPORT.md`
  (§ memory-pricing paragraph); dated records left as the history of their own trees. Suite re-measured:
  **497 passed / 0 failed / 1 ignored** over 50 `test result:` lines = 285 unit + 207 integration +
  5 doc, `aether` lib 43 → 47 recounted from the attributes (`tls.rs` 10 + `app.rs` 17 +
  `extensions.rs` 8 + `http2.rs` 12). Swept: root `README.md`, `framework/README.md`,
  `BASELINE_MEASUREMENTS.md` (prose, `aether` row, total), `FEATURE_COMPLETENESS_MATRIX.md` (evidence
  bullet, ladder, unit split, per-file split, unit row), `PRODUCTION_READINESS_REPORT.md` (file
  inventory, pricing paragraph), `FULL_FRAMEWORK_DOCUMENTATION_AR.md`, and this file's Active-Track
  parenthetical. Two stale claims found en route and fixed while there: the matrix said "the 17 tests
  added since [474]" where the arithmetic is 23, and the Arabic guide said 9 `tls.rs` unit tests where
  the file has 10.
- **Gates.** `cargo test --workspace` exit 0 at 497/0/1, 50 result lines; `cargo test -p aether --lib --
  http2::tests` → 12 passed / 0 failed. `cargo fmt --all --check` exit 1 at the standing **570 hunks /
  65 files** — and that only after a correction: the first run reported 575, all five inside the new
  tests (`:1577, :1592, :1642, :1659, :1775`, long single-line asserts), each hand-expanded into the
  multi-line form rather than running `cargo fmt` across 65 files. Like-for-like per B15/D4:
  `rustfmt --edition 2021 --check` gives **11 hunks over the HEAD blob and 11 over the working file**,
  the same eleven shifted by +28/+30/+40, so zero new fmt hunks. clippy as above. `validate` exit 1 with
  the standing three `[RECYCLED ID]` reuses plus `[OPEN VS HISTORY] REQ-0001`. **These reds are the
  repository's standing state; the project's overall status is not green and is not described as green
  here.** No throughput number appears in this entry or in any doc it touched.
- **Ledger.** `new problem` → `[ADDED] PROB-0027`, then `done PROB-0023` → `[COMPLETED & REMOVED]`;
  event log holds `new_problem PROB-0027` and `done_and_deleted PROB-0023` and open problems stayed at
  7 (`status`: `Requirements: 1 | Problems: 7 | Proposals: 1 | Criticisms: 0`).
- **Cleanliness:** no listener was started on any port, so **the owner's :3000 process was untouched**;
  nothing pushed, no amend, no history rewrite; scratch logs in `C:/tmp` only.
- **Open after this track:** **PROB-0027** (owner-backed response heap the budget cannot name),
  **PROB-0012** / task #5 (a Linux run of this suite has never happened), the unattributed p50 gap,
  deferred Stage B, B19's unswept `text`/`console` fences, B20's un-swept sibling Status tables, B21's
  own sweep (other per-crate "no finding names X" claims read from the same aborting command), the four
  pre-existing `http2.rs` clippy findings, and the fact that only the `max_body_bytes` refusal was
  driven over a stream — the deadline and budget-refusal exits share the same `buffer_set(0)` path but
  were not each exercised on a live stream. The next candidate for the biggest proven gain remains the
  **pinned, same-session worker-count sweep** (task #69).

2026-10-25 (track 67 — **an HTTP/2 stream can no longer be kept alive by paying one byte per
deadline: the write timer is now earned, and the tripwire that proved the hole was inverted**):
The owner's message on this track was «قرر من نفسك» — decide for yourself. **PROB-0022** was the
largest thing left that this host can actually settle: its pre-declared branch, measured on track 52,
said *Refute = the stream is bounded independently of elapsed time, by a byte or rate bound or by an
absolute cap that does not truncate ordinary downloads*. That is what shipped.
- **Root cause, one line.** `write_body` restarted `deadline = Instant::now() + write_timeout` after
  *any* successful `send_data` (then `http2.rs:658`). A peer that grants one byte of flow-control
  credit before each expiry therefore moves one byte, renews the deadline, and keeps its stream, its
  registration and up to `h2`'s 400 KiB send-buffer leftover alive for as long as it likes, at a cost
  of one credit per interval. Track 51 priced the memory; pricing never ended the stream.
- **Fix shape, and the alternative refused.** `MIN_PROGRESS_FOR_RENEWAL = 1024` (`:493`) must be moved
  before the clock restarts (`:679-683`, accumulator `:646`), and bytes accumulate across waits until
  the minimum is paid, so credit earned in one interval is not wasted and a normal consumer never
  notices. What this adds is a floor on consumption — `1024 / write_timeout`, i.e. ≈102 B/s at the
  default `max_response_time` of 10 s — while the bound stays an *interval* bound: a stream that earns
  every interval may run for any total time. The owner's D1 ruling refused an absolute per-response cap
  because it truncates legitimate long downloads, and this is not one; `send`'s doc (`:506-508`) and
  `write_body`'s (`:639-642`) say so at the functions that implement it. **The ≈102 B/s figure is
  arithmetic from the constant and the default, not a measured throughput, and is not published as one.**
- **The tripwire was inverted, as track 52's record required.**
  `a_trickling_reader_keeps_an_h2_stream_alive_past_every_configured_deadline` is deleted;
  `a_trickling_reader_is_cut_at_the_deadline_it_cannot_earn` (`framework/tests/tests/http2_e2e.rs:949`)
  asserts the opposite in the same shape (both timers 1500 ms, one byte of credit every 100 ms for
  5000 ms) and now requires: `RST_STREAM(CANCEL)` as the end, bytes moved after the window drained,
  movement under one renewal minimum (so the run measured a trickle and not a slow download), fewer
  than all 256 KiB received, the cut inside the window, and a HEAD on the same connection still
  answering 200. Its premises are compile-time arithmetic: `GRANT_MS * 10 <= TIMEOUT_MS`,
  `TIMEOUT_MS / GRANT_MS < MIN_PROGRESS` (at most 15 bytes can move in an interval against 1024),
  `OBSERVE_MS > 3 * TIMEOUT_MS`.
- **And its pair, because one test cannot separate a floor from a cap.**
  `a_slow_reader_that_earns_every_deadline_finishes_the_whole_body` (`:1109`) reads a 100 KiB body at
  2 KiB per 40 ms against a 200 ms timer: whole body arrives, ≥4 paced intervals, elapsed time beyond
  three deadlines, connection still serving. Two mutants, each reverted after its run: renewal on any
  progress (the pre-fix line) → the trickle test fails with "a peer granting one byte per 100 ms still
  held the stream open for the whole 5000 ms window after 65571 of 262144 bytes"; a total-cap cut →
  the pair fails with "a reader that earns every deadline was cut after 75775 of 102400 bytes in 8
  paced intervals: Error { kind: Reset(StreamId(1), CANCEL, Remote) }". `grep -c MUTATION` on the tree
  after the runs → 0, and the whole `git diff` was re-read to confirm only the intended change remains.
- **The +12-line insertion moved every coordinate below it in `http2.rs`, and the current-tense
  citations that pointed there were re-derived by symbol, not by arithmetic**: the one non-dated doc
  claim naming `price_response_body` (`FEATURE_COMPLETENESS_MATRIX.md`'s `Bounded response memory` row)
  moved `:604` → `:618`, verified with `grep -n "fn price_response_body"`, and the two new citations
  this track wrote (`MIN_PROGRESS_FOR_RENEWAL` at `:493`) were taken from the same grep. Dated records
  were deliberately left holding the coordinates of the trees they measured — the track-49/50/51/52
  entries here and the PROB-0020/0021/0022 paragraphs in the register still read `:604`, `:658`,
  `:512`, `:556`, `:1264`, `:1347`, and that is what a dated record is for. **What is *not* swept:**
  no line numbers are quoted for those records here, because this track's own insertion shifted them —
  a citation of a governance file's line inside a governance file is the hazard B20 names, and the
  track number is the stable key.
- **Counts re-measured, not carried.** `cargo test --workspace` on the final tree → exit 0, **493
  passed / 0 failed / 1 ignored** over 50 `test result:` lines = **281 unit + 207 integration + 5 doc**
  (20 unit targets, 15 integration targets, 15 doc-test lines, one of them non-zero). The mover is
  exactly this track: `http2_e2e` 36 → **37** (`grep -c 'tokio::test' tests/tests/http2_e2e.rs` → 37,
  and `grep -c '^#\[tokio::test'` → 37), with its construction split re-derived by classifying each
  test body: 19
  `H2Client::connect` over TLS, **15** `connect_plain` over h2c (was 14 — the new pair test), 3
  hand-framed. Updated in `README.md`, `framework/README.md` (the count paragraph and the suite
  inventory), `framework/docs/BASELINE_MEASUREMENTS.md`
  (narrative + the `http2_e2e` row + the total row, whose 15 target numbers sum to 207),
  `framework/docs/FEATURE_COMPLETENESS_MATRIX.md` (evidence block, both Status rows, and the
  `Bounded response memory` row), `framework/docs/PRODUCTION_READINESS_REPORT.md` (§5's HTTP/2 row and
  §10's target list) and the two Arabic docs (`GETTING_STARTED_AR.md`,
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md` twice). The middleware-in-the-chain count (34, `http2_e2e` 7) is
  unchanged: neither new test builds an app with middleware.
- **Gates, as measured.** `cargo test --workspace` exit 0 at 493; `cargo fmt --all --check` exit **1**
  at **570 hunks over 65 files** — the same shape track 66 recorded, because the one hunk rustfmt
  demanded from the new test (`assert_eq!` on the pair test's tail) was fixed by hand rather than by
  running `cargo fmt` over 65 files; no reported hunk line falls inside any range this track edited
  (`http2.rs` hunks at 28/36/443/456/576/586/762/794/811/850/879 vs changes at 483-510 and 638-684;
  `http2_e2e.rs` at 12/102/110/202/236/354/382/601/627/1348… vs changes at 948-1201). `cargo clippy
  --workspace --all-targets -- -D warnings` exit **101** with **no** finding naming `http2.rs` or
  `http2_e2e.rs` (grep over the log), and `validate` exit **1** with the same four lines as before this
  track: three `[RECYCLED ID]` reuses plus `[OPEN VS HISTORY] REQ-0001`. **These three reds are the
  repository's standing state and this track does not change them; the project's overall status is not
  green and is not described as green here.**
- **Ledger.** `done PROB-0022` run from the repository root → `[COMPLETED & REMOVED]`, event log line
  `done_and_deleted PROB-0022`, open problems **8 → 7** (`status`: `Requirements: 1 | Problems: 7 |
  Proposals: 1 | Criticisms: 0`); `validate` printed no `[LEDGER]` notice, so the resolution walked
  nothing — the track-66 fix holding on its own command.
- **Cleanliness:** no listener was started on any port, so **the owner's :3000 process was untouched**;
  nothing pushed, no amend, no history rewrite; scratch logs live in `C:/tmp` only.
- **Open after this track:** **PROB-0023** (the unpriced `&'static`/shared/sliced response blocks,
  `buffer_set`'s one-block-per-stream semantics, and the framework's own error answers keeping the last
  inbound block registered for one error response), **PROB-0012** / task #5 (a Linux run of this suite
  has never happened), the unattributed p50 gap, deferred Stage B, B19's unswept `text`/`console` fences
  in the other docs, B20's un-swept sibling Status tables, and the two things this track deliberately
  did not bound: a peer that consumes *above* the floor may still hold a stream forever, and the ≤400 KiB
  post-`send_data` leftover is still only bounded by the connection idle timer. The next candidate for
  the biggest proven gain remains the **pinned, same-session worker-count sweep** (task #69).

2026-10-25 (track 66 — **one ledger per repository: the governance tool stopped trusting the directory
it was standing in, and stopped overwriting the files that hold the owner's rules**):
The owner's message on this track was «كمل» — continue, decide for yourself. Track 65's own record
named **PROB-0025** as still open, and it is the hazard that every number in this ledger has to be
read knowing about: the tool resolved its root from the shell's cwd, so a governance command run from
`framework/` believed `framework/` was the project.
- **The two defects are one root cause, and the second was found while reproducing the first.**
  PROB-0025 measured: `new problem` from a subdirectory created `framework/.project-governance/`,
  allocated **PROB-0001** from its own empty list — a number the real ledger had used and closed —
  left the root ledger untouched, and `validate` at the root could not see the stray tree, so nothing
  failed. PROB-0026 is the same resolution bug pointed the other way: `init` run *on top of an
  existing ledger* on an exported scratch copy **exited 0 and printed `[SUCCESS]` while replacing
  `AGENTS.md`, `SYSTEM_PROMPT.md` and `PROJECT_MAP.md` with the blank template** — the first two being
  where the owner's rules live. `OWNER_VISION.md` and `WORKING_CONTEXT.md` survived only because those
  two writes were already guarded, which is how the damage was so selective.
- **Fix, in the tool.** `LEDGER_MARKERS` (`:41`), `locate_ledger()` (`:45`) and `resolve_root()`
  (`:59`): the ledger is found by walking up from the target and is identified by its **data files**,
  not by the directory name — so an empty `.project-governance/` left by an interrupted run cannot
  shadow the real one above it — and a walk that reaches the filesystem root is a refusal rather than
  a new project. `main()` (`:538`, resolution at `:563`) applies it to every command except `init` and
  prints `[LEDGER] using <root> (resolved by walking up from <start>)` on stderr *only* when the walk
  moved the target. `init_project()` (`:299`, guard at `:300`) refuses where a ledger exists, and the
  `SYSTEM_PROMPT.md` / `AGENTS.md` writes became `[KEPT]` guards.
- **Wire-confirmed on the real repository, not only in tests.** `status` from `framework/aether/src`
  → `Location: …\aether-web-framework\.project-governance/`, `Problems: 9`, exit 0. `validate` from
  that subdirectory → exit **1**, the same four finding lines as the root run (PROB-0007 ×2,
  PROB-0017 ×2, REQ-0001 ×3, `[OPEN VS HISTORY]`) plus the `[LEDGER]` notice, and the root run prints
  no notice; compared line-by-line by `C:\tmp\t66_cmp.py`. `init` from `framework/aether/src` → exit
  **2** with `[ERROR]`, `framework/.project-governance/` never created. `--root framework/aether init`
  → also exit 2, while `--root C:/tmp/t66fresh/nested init` → exit 0 with a real tree, so the refusal
  is about the ledger and not about the subcommand or the depth. Registered **PROB-0026** through the
  fixed tool *from the repo root*, then `done PROB-0025` and `done PROB-0026`; open problems 9 → 8.
- **Tests: 11 cases, and the first version of one of them was wrong.** `test_ledger_location.py` plus
  `test_id_allocation.py` = **37 green** under `python -B -m unittest discover -s
  .project-governance/tools/tests`. My read-the-real-ledger case asserted `PROB-0001` in `status`
  output and **failed**: `status` prints counts and the location, never problem IDs — the assertion was
  testing a sentence the tool does not write. It now seeds a *duplicate ID in the root ledger*, which
  only reading the root can reveal (`Problems: 2`, `[DUPLICATE ID] PROB-0001`, exit 1), while a
  parallel tree would print `Problems: 0` and `[VALID]`. Sensitivity proven by mutation on a copy of
  the tool and both test files in `C:/tmp/t66mut`: cwd-only `resolve_root` → **6 red**;
  `existing = None` in `init_project` → **3 red** (including the CLI exit-2 case); both `[KEPT]`
  guards forced false → **1 red**; the unmutated copy in the same sandbox → OK, so the reds belong to
  the mutations, not to the copy.
- **Rule text kept in step with the behaviour**, in both places an agent reads it: item 6 of
  `.project-governance/SYSTEM_PROMPT.md` (verified `python3 … status` runs on this box, exit 0) and the
  `init` template inside the tool, so a fresh repository inherits the rule rather than rediscovering
  the hazard. The walk is unconditional, so `init` for a genuinely nested repository inside this tree
  is refused too; that is stated as a limit in the module docstring (`:11-12`) instead of left to be found.
- **Gates, as measured.** `.rs` / `.toml` / `.lock` are **byte-identical to HEAD** (`git diff --stat
  HEAD -- '*.rs' '*.toml' '*.lock'` printed nothing), so track 65's Rust numbers describe this tree
  exactly: `cargo test --workspace` exit 0 at **492** (281 unit + 206 integration + 5 doc), `cargo fmt
  --all --check` exit 1 (570 headers / 65 files), `cargo clippy --workspace --all-targets -- -D
  warnings` exit **101**, `validate` exit **1**. Nothing on this track changed those three reds, and
  **the project's overall status is not green and is not described as green here.**
- **Cleanliness:** the two scratch trees (`C:/tmp/t66mut`, `C:/tmp/t66fresh`) are deleted after the
  measurements were recorded; no listener was started on any port, so **the owner's :3000 process was
  untouched**; nothing pushed, no amend, no history rewrite. **No performance number appears in this
  track** — it changes no runtime code path of the framework.
- **Open after this track:** **PROB-0022/0023/0012**, task #5 (a Linux run of this suite has never
  happened), the unattributed p50 gap, deferred Stage B, B19's unswept pasted-output fences in the
  other docs, and B20's un-swept sibling Status tables. The next candidate for the biggest proven gain
  is still the **pinned, same-session worker-count sweep** (task #69) that track 65 refused to skip;
  nothing in this tree justifies changing the `num_cpus` default yet.

2026-10-25 (track 65 — **the E1001 advice stopped recommending commands this OS does not have, and the
doc table that promised "current coordinates" was measured to be lying**):
The owner's message on this track was «اتخذ قرار انت» — decide for yourself. Two things followed from
that, and the order matters.
- **What was NOT done, and why.** The biggest *proven* gain still on the table is the worker default:
  `AetherConfig::default()` sets `workers = num_cpus::get().max(1)` (`framework/aether/src/config.rs:53`)
  and track 64 measured 8 workers costing **3.0×** the CPU per request of 1 worker on this box. That is
  an argument for changing a default every example inherits, but the evidence behind it is an unpinned,
  same-box sweep — the `Shape` column of `framework/target/pgo-ab*` reads `w1/anone`, `w8/anone`, i.e.
  no affinity mask — and a default is the kind of change that must not rest on a shape that could be
  the cause. Decision: **do not touch the default on this track**; the prerequisite for any default
  change is one pinned, same-session worker-count sweep (server pinned, generator on the complement
  mask), and that sweep is this track's carry-forward candidate for the next one.
- **PROB-0024, closed.** The `Try:` block of an E1001 bind failure told Windows users to run
  `lsof -ti:3000 | xargs kill -9`: two commands that do not exist on the only platform this tree
  builds, naming a port that was not the one that failed. Fixed in the product
  (`framework/aether/src/app.rs`), not in the doc: `port_change_advice` (`:2485`) derives the
  alternative from the port that actually failed, and `port_holder_advice` (`:2504`) returns
  platform-shaped lines — on Windows `netstat -ano | findstr LISTENING | findstr :<port>` then
  `taskkill /PID <pid> /F`, elsewhere the original `lsof` line — wired into the bind-failure arm at
  `:1309-1316`. Closed through the governance tool **from the repo root** (`done PROB-0024`), the
  **PROB-0025** hazard.
- **The advice was wrong once more, and only running it found that.** The first Windows line I wrote
  was `netstat -ano | findstr :<port>`. Executed against a live listener on 3215 it returned **two**
  PID cells for one port: the real holder and a `TIME_WAIT` *client* row whose PID column is `0`. A
  user following that line is told to kill a phantom, and my own proof script caught it by refusing to
  continue when the PID was not unique. Hence `| findstr LISTENING` in the shipped text, the advice
  split into two lines, and a third assertion in the test. This is B19's lesson one level up: a pasted
  *instruction* is a runtime claim and is falsified by executing it, not by reading it.
- **Test, and the three proofs that it can fail.** `bind_advice_names_the_failed_port_and_a_command_this_platform_has`
  (`framework/aether/src/app.rs:2759`) checks the port appears, `3000` does not, the Windows branch
  names `netstat`+`taskkill` and never `lsof`, the non-Windows branch keeps `lsof`, and the alternative
  port is the failed one +1. Sensitivity proven by mutation, three ways: hard-coding the port back to
  3000 fails it, dropping the `LISTENING` filter fails it, and returning the `lsof` line on Windows
  fails it. `cargo test -p aether --lib app::tests::bind_advice… -- --exact` green (the first attempt
  at this used a partial filter with `--exact` and reported `0 passed; 42 filtered out` under a
  `test result: ok.` — a green that meant nothing).
- **The artifact, not the source, is what the docs quote.** Rebuilt `framework\target\release\basic_api.exe`
  (3,246,080 B, md5 `f52b0aa8…`, mtime 19:10) and captured E1001 from it on port 3213 against a real
  second listener → `os error 10048`, exit 1. The first capture printed the *old* `lsof` line: the
  build that was supposed to produce the new binary had failed, invisibly, because the command was run
  through a pipe and `$?` was read after the pipe. Wire-confirm the artifact (mtime, size) before
  quoting it. `ERROR_HANDLING_GUIDE_AR.md` §2b now carries that capture and is verified
  **byte-identical to the file the binary wrote**, after stripping the ANSI escapes `format_terminal(true)`
  emits, by `C:\tmp\t65_check_fence.py` (exit 0). `FULL_FRAMEWORK_DOCUMENTATION_AR.md` §14b's hand-written
  `Diagnostic` illustration no longer teaches the `lsof` line either, and its fragment re-compiles
  (`cargo check --bin probe` in `C:\tmp\t25\depcheck`, exit 0). The documented
  `cargo build --release --bin basic_api` was re-run and is correct (exit 0, 1.07 s, already fresh) —
  my earlier failure was my own `--example basic_api`, and `basic_api` is a `[[bin]]` of the
  `aether-examples` package, not an example target.
- **B20, found while checking whether this track broke a coordinate.** It did not, but the sweep
  measured eight `app.rs` citations in the Status columns of `ACTIX_FAIR_COMPARISON.md` §6 against HEAD
  *before* the edit and six were already dead — the table's own banner promises those columns carry
  "current coordinates". Rows F5/F6/F7 are now anchored on the construct (`match version` in
  `serve.rs`, the `if close_after_response` block, `connection_has_token`, `read_limit`,
  `WebSocket::from_parts`, `with_websocket_limits`) with 2026-10-25 line numbers beside them, both ends
  verified, and the banner says the numbers are a dated measurement. Registered with its rule as
  **B20**; the same sweep across the other Status-column matrices was *not* run and B20 says so.
- **Suite and counts, re-measured on the final tree.** `cargo test --workspace` exit 0:
  **492 passed / 0 failed / 1 ignored** = 281 unit + 206 integration + 5 doc over the same 50
  `test result:` lines; the only mover is this track's one test (`app.rs` 16 → **17**, `aether` lib
  42 → **43**, and 43 was re-derived per brace-matched body as `tls.rs` 10 + `app.rs` 17 +
  `extensions.rs` 8 + `http2.rs` 8, not taken from the doc). Six current-tense claims swept: root
  `README.md`, `framework/README.md`, `BASELINE_MEASUREMENTS.md` (prose, `aether` row + split, total),
  `FEATURE_COMPLETENESS_MATRIX.md` (five places) and this file's header line. Dated per-track records
  left as written (**B11**).
- **Gates, measured after the last edit, not inherited.** Repo-wide `cargo fmt --all --check` → exit 1,
  **570 `Diff in` headers across 65 files** — identical to the pre-track figure, and `app.rs` is at its
  own baseline of **40**: my lines were reshaped into rustfmt's own output (macro args past the 60-char
  `fn_call_width` heuristic expand, string literals never do) so this track added zero hunks; the
  gate stays red as registered and was not "fixed" by reformatting 65 files. `cargo clippy --workspace
  --all-targets -- -D warnings` → exit **101**: 23 `error:` lines of which 5 are `could not compile`,
  i.e. **18 lints**, and every one of their 25 `-->` lines is under `framework/http/src` (21),
  `framework/scheduler/src` (2) or `framework/validation/src` (2) — so **no location in this run
  points at `framework/aether/src/app.rs`** and this track added no lint. The recorded per-crate split (`aether-http` 9, `aether-scheduler` 2,
  `aether-validation` 1) re-appeared from `cargo clippy -p <crate> --lib -- -D warnings` as **10/3/2
  error lines** including each crate's `could not compile` line, but that form is a cached replay, not
  a fresh count (**B18** measured this exact instability), and it does not equal the 21/2/2 of the
  `--all-targets` run — the workspace number is the one to quote, and why the two differ was not
  chased. `validate` → exit **1** on the same three historical ID reuses (PROB-0007,
  PROB-0017, REQ-0001); closing PROB-0024 raised nothing. So the project's overall status is **not
  green** and is not described as green anywhere in this record.
- **Cleanliness:** every listener this track started is stopped (the one stray holder, PID 5040, was
  killed by a name-verifying script after it was discovered through its own defect), no
  `basic_api.exe` is left running, only ports 3213/3215 were used, and **the owner's :3000 process was
  never touched**. Nothing pushed, no amend, no history rewrite. **No performance number on this track
  is claimed as a project result.**
- **Open after this track:** PROB-0024 is the only ledger item closed. Still open — **PROB-0025** (the
  tool still resolves its ledger from cwd, so `new` from `framework/` would mint a parallel ID space),
  **PROB-0022/0023/0012**, task #5 (a Linux run of this suite has never happened), the unattributed
  p50 gap, deferred Stage B, B19's unswept pasted-output fences in the other docs, and B20's un-swept
  sibling Status tables. The next candidate for the biggest proven gain is the pinned worker-count
  sweep described at the top of this entry; nothing in this tree justifies a default change yet.

2026-10-25 (track 64 — **the owner's verification ladder ran in order, and its last step deleted the
PGO speed claim before it could be published**):
Tracks 62 and 63 shipped a real worker setting and a runnable PGO route. The owner separated the two
kinds of proof they had produced — 491 green tests prove the code does what it says, nothing proves it
is *faster* — and set a five-step ladder: (1) confirm `AETHER_WORKERS=1` reaches the runtime, (2) train
PGO under representative load with a natural exit, (3) verify the profile is valid before trusting a
build against it, (4) measure release-vs-release from one source under one shape, (5) isolate the worker
effect from the PGO effect and combine only if worth it. All five ran; **no performance number on this
track is claimed as a project result** and every figure below is agent-run, this session, this 8-core
box, generator co-located.
- **Step 1 found a defect before measuring anything.** `framework/examples/basic_api.rs` built its
  config with `AetherConfig::default()`, so every env var the README documents — `AETHER_WORKERS`,
  `AETHER_PORT` — was parsed by a code path this example never entered. It now uses `from_env()` and
  binds `config.host:config.port`, and the live banner on port 3210 reported `Workers : 1 (active;
  tokio work-stealing)` with no mismatch note. `framework/README.md` gained the two-condition rule a
  reader needed: an env var must be *read* (`from_env`, not `default`) and *acted on*.
- **Step 2 became a tool, because a detached server cannot be trained by hand.** The flush rule was
  measured four ways on one scratch binary: counters are written when the process **returns from
  `main`** — even with threads still running — and `std::process::exit(0)`, `Stop-Process -Force` and
  `taskkill` without `/F` each write **0** `*.profraw`. A server started detached has no console to
  receive Ctrl+C, so "stop it with Ctrl+C" was not a recipe the harness could follow. `pgo.ps1`
  gained `-Action train` and `framework/tests/examples/pgo_train.rs`: a deterministic in-process
  training run that builds its runtime through `AetherApp::build_runtime` (so the worker setting
  applies exactly as in `basic_api`), serves the same routes on an ephemeral port, drives its own
  keep-alive load, shuts down via `serve_with_shutdown` and returns. Measured: 5,000 req / 8 conns →
  536 ms, 9 `*.profraw` (7,561 KiB); 50,000 / 16 → 1 file (2,326 KiB). Its printed rate is the
  in-process client's, labelled in its own output as not a server throughput figure.
- **Step 3 is what made step 4 honest.** `merge` runs `llvm-profdata show` and exits 2 on a profile
  that records nothing, so a successful command can never stand in for a valid profile. The merged
  50,000-request profile is still re-checkable on disk at `framework\target\pgo-profile\merged.profdata`
  (`functions=648`, `max_function_count=7,404,024`, `total_count=131,387,411`). The trap found here is
  that **`Total functions` is not a quality signal**: the 5,000-request pass merged 9 files and
  reported *five times* the functions (3,341) of the 50,000-request pass that merged 1 — host
  artifacts from `RUSTFLAGS` reaching build scripts and dependencies, not deeper training. Cost of the
  route, measured: **68 m 52 s** for the `-Cprofile-use` build vs **2 m 55 s** plain (≈23×), artifact
  3,647,488 B vs 3,242,496 B (**+12.5%**, md5 `82059eb4…` / `e9127f96…`).
- **Step 4: the build delta is +5.0% µs CPU/request at +0.0% rps — and the harness cannot resolve 5%.**
  `framework/tools/pgo_ab.ps1` holds shape fixed and swaps only the binary, rotates arm order every
  rep, reads the *server process'* own CPU seconds before/after plus peak RSS, and treats
  µs CPU/request as the metric with rps as the control (an rps read by a co-located generator
  describes the pair, not the server).
- **Step 5 killed it, and that is the result.** `-Self` — both arms on the *same binary*, same shape
  (`AETHER_WORKERS=1`, pinned to one core, 5,000/s, 64 conns, 3 reps) — printed a noise floor of
  **+0.0% rps and +4.5% µs/request**, i.e. the harness's own zero is nearly as large as the effect
  step 4 attributed to PGO. Within-arm per-rep spread (baseline 89.8–123.0, PGO 91.8–114.5, self
  65.2–85.9) is wider than either figure. So: **PGO has no measured gain in this repository**, only a
  route that runs end to end and a cost that is known. The fix is more reps, not a friendlier reading
  of three. This is the outcome the owner's own instruction predicted, recorded as a null rather than
  dressed as a win.
- **The worker effect, isolated with one binary in one session, reproduced — and it is 3×.** At a rate
  both arms hold (5,000/s): 1 worker 84.4 µs CPU/req, 8 workers 251.2, both at ~4,999 rps. Near
  saturation (20,000/s offered): 1 worker answered 17,699/s and *missed the rate* (exit 2) while 8
  answered 19,939/s (exit 0). Process CPU is the sum over worker threads, so wakeups and work-stealing
  are charged, not hidden: the count is a **trade** — headroom under load, paid per request — not the
  win track 62's wording implied.
- **No rps ceiling is established, and the ladder was not allowed to invent one.** Growing the client
  pool made the same server shape *worse*: 17,699/s at 128 conns → 14,707/s at 256, and the 1-worker
  arm collapsed to 8,522/s at 512 with `latency p50` 39 ms and `lag p95` 6.2 s. The criterion declared
  before the run was refuted by the data, so the number is withheld and only the direction is stated.
- **The combined run was deliberately not run**, with the reason written into §5: PGO's term sits
  inside the noise floor, the worker term is a structural factor of three, and no build flag moves it —
  so combining would spend 69-minute rebuilds to re-measure the term the harness cannot resolve. Next
  useful measurement: more reps at one shape, or the generator on another host.
- **Four defects the ladder exposed, all fixed.** (1) A body-equality check that could not survive its
  own shell: the A/B driver passed `--expect {"status":"healthy",…}` and every rep reported a mismatch,
  because PowerShell 5.1 hands a native exe that 43-byte JSON as **35 bytes with every quote stripped**.
  The generator gained `--expect-file PATH` (same bytes, same slot, last flag wins) so the driver reads
  the body from a file — §4 now says a JSON `--expect` is not safe from PowerShell. (2) `openloop_loadgen.rs`'s exit-2 message asserted
  *"this run does not measure the server"* — an attribution the four diagnostic runs (512/128/64/48
  conns) disproved. It first grew a symmetric rule, which was itself wrong: `pool capacity < offered
  rate` is *not* decisive, because under saturation the measured latency is queue delay, so inflating
  the pool can make things worse. It now fires only the asymmetric, genuinely decisive branch (capacity
  ≥ offered rate rules the pool out → read the shortfall as evidence about the server) and otherwise
  says "inconclusive, publish neither". (3) `ServerBanner::render`'s box was mis-sized on every row —
  border 61 dashes, content rows 61 chars (two short of the frame), title 62, and only the literal
  Allocator row reaching 63: two opposite errors hid each other in the printed output. It is now
  width-driven from one `INNER` constant. (4) `pgo_ab.ps1` could only compare builds; it now takes
  `-WorkersB`/`-AffinityB` so a *runtime-shape* delta is measured in one session without a second
  build, with a three-way verdict (noise floor / shape delta / build delta) that prints the ratio so a
  sign convention cannot be misread.
- **The banner fix found a bug a green test could not see.** The first width-parameterised version
  printed the literal `{:<12}` in place of the `Allocator` label — a 63-character row, correctly boxed,
  that passed the width assertion and the corner assertion. It was caught only by running the binary.
  `test_server_banner` now checks equal row widths, the frame characters per row index, all five labels
  by name, and refuses an unrendered `{:`; both mutations (hand-padded `Routes` row, `.to_string()`
  label) were proved to fail.
- **Docs corrected against measured output** (improvement-register item **B19**). A pasted-output fence
  is a claim about runtime behaviour: nothing compiles it, so B12–B15's extract-and-compile method was
  structurally blind to it, and the only falsifier is running the command. `ERROR_HANDLING_GUIDE_AR.md`
  §2a carried a banner the binary has never printed (`Rust Native Zero-Copy Engine`,
  `Workers : 8 (Thread-per-Core Async Epoll)`,
  `Allocator : RequestArena (O(1) Bump Reset - Zero Heap)`) — three claims refuted by the code (the
  runtime is tokio work-stealing, this is a Windows/MSVC build with no epoll, and
  `aether_core::RequestArena` exists but nothing in the serving path allocates on it), replaced by the
  real `AETHER_WORKERS=1` capture with the refutation named in Arabic beside it. §2b's Linux-shaped
  E1001 sample (`os error 98`) was replaced by the real Windows one (`os error 10048`, exit 1).
  **Both samples were re-produced on the final tree before committing**: the banner re-captured from
  `target\release\basic_api.exe` at `AETHER_PORT=3211 AETHER_WORKERS=1`, so the doc's provenance line
  names the binary and port actually run and each of its nine rows measures 63 characters (64 display
  cells only on the ⚡ row); the E1001 block re-run by starting a second listener on 3212, byte-identical
  to what the doc now prints apart from the port. `BENCHMARKS.md` §4/§5 carry the null result and the
  worker trade with their conditions stated. The product still prints `lsof … | xargs kill -9` inside
  the E1001 hint on a Windows-only build, with the port hard-coded to 3000 even when a different one
  failed → **PROB-0024**.
- **Governance footgun, hit and registered: PROB-0025.** Running
  `project_governance_bootstrap.py new` with the shell's cwd inside `framework/` created a parallel
  `framework/.project-governance/` tree and minted **PROB-0001** from its empty `problems.json` — a
  live ID reused in a second ledger the root `validate` cannot see. The root ledger was untouched; the
  stray tree (files created minutes earlier in this session) was deleted and the registration redone
  from the repo root. The tool should refuse to initialise a second ledger inside the repo.
- **Coordinates re-derived for what this track shifted** (the track-42 precedent): the `Workers` label
  in `framework/core/src/diagnostic.rs` moved `:175`→`:196` and `app.run` in
  `framework/examples/basic_api.rs` `:53`→`:57`; both corrected inside the track-62 record. A sweep for
  `name.rs:<n>` citations of every file this track touched found no other live reference.
- **Gates, measured after the last edit, not inherited.** `cargo test --workspace` three times on this
  track's trees: **491 passed / 0 failed / 1 ignored** across 50 `test result:` lines every time — 280
  unit + 206 integration + 5 doc, identical to the documented claim (`pgo_train.rs` adds an example,
  not a test). `cargo fmt --all --check`: **still red, 570 `Diff in` headers across 65 files** (the
  recorded figure said 66 — formatting the three files this track changed moved one out of the list;
  only own hunks were formatted, and the 3 pre-existing hunks in `tests/src/lib.rs` plus
  `arena.rs`/`error.rs` were
  left alone; one `cargo fmt -p aether-core` over-reach did reformat those two and both were reverted
  with `git checkout` after confirming they were clean minutes earlier). `clippy`: the banner rewrite
  initially added **two lints of its own** in `core/src/diagnostic.rs` — `useless_format` and
  `format_specifiers_have_no_effect_on_format_args` — both worth fixing rather than suppressing: the
  inner `{:<44}` wrapped a `format_args!` value, which clippy reports as a no-op, and the row's padding
  provably comes from `row()`'s `{:<INNER}` anyway, so the code read as if the width came from a place
  it did not. Fixed, and `cargo clippy -p aether-core --all-targets` is now clean for
  that file; `aether-tests` keeps its pre-existing warnings (`http_e2e.rs`, the `collapsible_if` at
  `openloop_loadgen.rs:632` and two `redundant_pattern_matching` at `tests/src/lib.rs:231,235`, all
  outside this track's edits). Repo-wide `clippy -D warnings` stays red exactly as B1 recorded.
  `validate` still exits **1** for the same three historical ID reuses (PROB-0007, PROB-0017,
  REQ-0001) and raised nothing about PROB-0024/0025. The live banner was re-captured from the rebuilt
  release binary after the rewrite, not inferred from the passing test. **Nothing pushed; no amend; the
  owner's :3000 process was never touched.**
2026-10-25 (track 63 — **PGO went from "blocked, the tool is missing" to "runnable, with its four
traps written down"**):
Track 61 left the owner's rps-ceiling path with two blockers: the pinned-one-core server (closed by
track 62) and PGO, which could not even start because `llvm-profdata` was not in the toolchain. This
track proved the whole route on this box and shipped the driver for it.
- **Toolchain change, stated as one.** `rustup component add llvm-tools-preview` was run here on
  2026-10-25; it adds `llvm-profdata.exe` (and `llvm-cov`, `llc`, …) under
  `$(rustc --print sysroot)\lib\rustlib\x86_64-pc-windows-msvc\bin`, and it does not change rustc or
  its codegen, so it does not disturb the comparability of any earlier measurement. It is reversible
  with `rustup component remove`.
- **The deliverable is `framework/tools/pgo.ps1`**, four actions — `generate`, `merge`, `use`,
  `status` — with parameters `-Bin` (default `basic_api`, the pinned ceiling server), `-Package`,
  `-CountersPerSite` and `-TargetBase`. Exit `0` on success, `2` on a guard (no `llvm-profdata`, no
  `*.profraw`, no merged profile), `3` when cargo fails. The three passes use separate
  `--target-dir` trees (`target/pgo-gen`, `pgo-use`, `pgo-base`) so a plain release build can never
  be silently an instrumented one. `framework/docs/BENCHMARKS.md` §5 documents the route.
- **Four properties measured before writing the recipe, each of which would otherwise have cost the
  owner a blind debugging session.** (1) The profile flushes at exit: `Stop-Process -Force` produced
  **0** `*.profraw`, a natural exit produced **1** — so the training run must be stopped with Ctrl+C,
  and `merge` exits 2 with that sentence when the directory is empty. (2) The default value-profile
  counter budget is too small for this code: the instrumented run printed
  `Unable to track new values: Running out of static counters` **12 times**, and with
  `-Cllvm-args=-vp-counters-per-site=64` the same run printed it **0** times, which is why the script
  passes 64. (3) `RUSTFLAGS` instruments build scripts and dependencies as well, so one generate pass
  plus one run left **16** `*.profraw` files; `merge -sparse` folds them and unmatched functions are
  ignored. (4) An unknown flag in `RUSTFLAGS` makes cargo die in its target-information query
  (`unknown codegen option`, measured with `-Cprofile-remapping`, which this rustc lacks) — it looks
  like a cargo failure, not a flag mistake, so only stable `-C` flags are used.
- **End-to-end proof, and how it avoided a server.** The chain was first proved on a scratch crate in
  `C:/tmp/pgo_check` and `C:/tmp/pgo_cargo` (fast, and it showed `strip = true` does not block
  profiling), then at repository scale through the script: `generate -Bin sched_compare
  -Package aether-benchmarks` (2 m 41 s, instrumented), run it (a deterministic simulator that exits
  by itself — chosen precisely because proving PGO needs no listener and no load), `merge` (16 files
  → `merged.profdata`), `use` (PGO binary). The `-p` handling exists because of that run: the
  workspace's `default-members` is `examples`, so `--bin sched_compare` alone is "no bin target
  found". **Behaviour check, the only result here: the PGO binary's output was byte-identical to the
  instrumented run's at the same seed and workload.** No speed claim, no rps number.
- **What the owner still has to do, and why it is his.** For a throughput-relevant profile the
  training run has to be the measured server itself (`basic_api`, `AETHER_WORKERS=1`, pinned per
  track 62) driven by the §4 generator off-box, and the A/B is `target/pgo-base` vs
  `target/pgo-use`: release vs release, one session, repeats, keep PGO only at ≥3% rps or ≥3%
  µs CPU/request. The script prints both commands and reports no number itself.
- **Gates.** No Rust source changed on this track, so the suite is the 491 track 62 measured (50
  result lines, 0 failed, 1 ignored) — nothing here adds or removes a target; the `.ps1` is not a
  build input and the scratch crates stayed outside the tree. Repo-wide `fmt` stays at its known
  570 hunks / 66 files, clippy stays red on the same pre-existing lints, `validate` still exits 1 for
  the three historical ID reuses. **Nothing pushed.**
2026-10-25 (track 62 — **`AETHER_WORKERS` now builds the runtime it used to only print; the banner
reports the workers that exist, not the ones that were asked for**):
Track 61 ended on a measurement the repository could not make: pinning the server to one logical
core, which is the only way to read a ceiling on this box without the harness competing with the
server. That pin was impossible because `AETHER_WORKERS` was inert — `from_env` parsed it into
`AetherConfig::workers`, the banner printed it, and nothing handed it to a runtime builder: every
entry point in the tree was `#[tokio::main]`, which sizes workers from the logical CPU count. The
number in the banner was a wish.
- **The change.** `AetherApp::build_runtime()` (`framework/aether/src/app.rs:1266`) builds a
  multi-thread runtime with `worker_threads(config.workers.max(1))`, and `AetherApp::run(addr)`
  (`:1278`) owns it and blocks in `listen`. `workers` is clamped, not trusted: `AETHER_WORKERS=0`
  parses successfully today and a builder would reject zero, so zero is served by one worker. `run`
  refuses to nest — if a runtime already owns the thread it returns an error that names `listen`,
  because a runtime cannot be built inside another one and a silent fallback would recreate exactly
  the illusion this track removes. `listen`/`bind`/`serve` are unchanged: an embedder that owns the
  runtime owns the thread count, and that is the right division.
- **The banner tells the truth now.** `serve_with_shutdown` reads
  `Handle::try_current().metrics().num_workers()` (`:1424`) — what the running runtime has — and the
  `Workers` field's label in `framework/core/src/diagnostic.rs:196` went from "(configured; tokio
  work-stealing)" to "(active; tokio work-stealing)". When the active count differs from the
  configured one, a note is printed after the banner (`:1444`) saying the count is applied by
  `AetherApp::run`, not on a caller-built runtime.
- **The example shows the supported path.** `framework/examples/basic_api.rs` went from
  `#[tokio::main] async fn main` to `fn main` + `app.run(…)` (`:3`, `:57`), so
  `AETHER_WORKERS=1 … basic_api` is the pinned-single-core server the ceiling recipe needs. (Track 64
  re-derived both coordinates: `app.run` moved `:53`→`:57` when the example switched to
  `AetherConfig::from_env()`, so the address it binds is now `config.host:config.port` rather than a
  literal, and the `Workers` label row below moved `:175`→`:196` when the banner box became
  width-driven.)
- **Three tests, and their sensitivity was proved.** `build_runtime_starts_the_configured_number_of_workers`
  (`:2609`, asks for 3 and reads 3 back from the live handle),
  `zero_workers_is_served_by_one_instead_of_a_builder_error` (`:2616`), and
  `run_refuses_to_nest_inside_an_existing_runtime` (`:2625`, asserts the error names `listen`). The
  helper at `:2601` measures `Handle::current().metrics().num_workers()` inside the built runtime —
  the assertion is on the runtime, not on the config field. Deleting the
  `.worker_threads(self.config.workers.max(1))` call makes the first two fail; with it restored, all
  three pass. The count is measured, never carried forward.
- **Suite: 491, re-derived.** `cargo test --workspace` on this tree → **491 passed / 0 failed /
  1 ignored**, 50 `test result:` lines = 280 unit + 206 integration + 5 doc. The movers are exactly
  this track: `app.rs` 13 → 16, `aether` lib 39 → 42 (10 `tls.rs` + 16 `app.rs` + 8 `extensions.rs`
  + 8 `http2.rs`, counted by attributes per file). Every integration target held its count. Current-tense
  claims swept in root `README.md`, `framework/README.md`, `BASELINE_MEASUREMENTS.md`,
  `FEATURE_COMPLETENESS_MATRIX.md` and this file's Active-Track parenthetical; a dated bullet for the
  same recount was appended to register B11 rather than rewriting its 488 records.
- **Docs that claimed it was not applied.** `framework/README.md:198` (the `AETHER_WORKERS` row said
  the value was read and ignored), `FEATURE_COMPLETENESS_MATRIX.md:352` and `:366`,
  `PRODUCTION_READINESS_REPORT.md:564-565` and `docs/09_api_reference.md` — the last now documents
  `app.run`, its single-nesting refusal, and that `workers` applies only under it, with a snippet
  that was compile-checked against the crate (scratch target deleted after the run, as it is
  auto-discovered). The matrix's status columns were deliberately not moved: they feed the status
  table, and one applied config key is not a stage promotion.
- **Not verified by me — and that is a fact about this record.** The owner stopped the release build
  partway through this track («وققف فورا»), so the live server was never launched and the banner and
  note paths are verified only by the unit tests above and `cargo check`. The owner's commands:
  `cargo build --release --bin basic_api`, then `AETHER_WORKERS=1 framework\target\release\basic_api.exe`
  and confirm `Workers : 1 (active; tokio work-stealing)` with **no** note, and that any
  caller-runtime path (e.g. `full_async_api`, which keeps `#[tokio::main]`) prints the note. The
  pinned ceiling recipe then is `cmd /c start /affinity 1 …` for the server with the complement mask
  for `framework\target\release\openloop_loadgen.exe`. **No throughput number is recorded here.**
- **Gates.** `cargo test --workspace` exit 0 as above. `rustfmt --check --edition 2024` clean inside
  every range this track touched; repo-wide `fmt` stays at its known 570 hunks / 66 files and was
  not re-run into a reformat. `cargo clippy --workspace --all-targets -- -D warnings` is red for the
  same pre-existing toolchain lints as track 61 (`aether-http` 9, `aether-scheduler` 2,
  `aether-validation` 1); the two hits near this track's lines (`limits_for`, `from_env`) predate it,
  and no new lint comes from these files. `validate` still exits 1 for the three historical ID
  reuses. **Nothing pushed.**
2026-10-25 (track 61 — **the rps ceiling can be offered from the repository now, open-loop, and the
generator refuses to publish its own limits as the server's**):
The owner's recorded goal for 2026-10-25 is the server's open-loop rps ceiling, and the only
generator the project had lived outside the tree (a scratch `loadtest.py` in `C:/tmp`). This track
checks one in: `framework/tests/examples/openloop_loadgen.rs`, an HTTP/1.1 fixed-rate generator.
It is an example, not a test — it has no assertions and needs a live listener — so no suite count
moves; `cargo test --workspace` is unchanged at 488.
- **The schedule is absolute.** Request *k* goes out at `t0 + k / rate` whether or not request
  *k−1* has returned; connections only cap capacity. Each connection keeps one request in flight,
  reconnects solely after a transport failure, and drains the body by `Content-Length` before
  reusing the socket. Two clocks are reported separately: `lag` (send − due) and `latency`
  (send → last body byte).
- **Exit codes are the contract.** `0` = the offered rate was delivered and every response was
  2xx with the expected body (`--expect`); `1` = a request failed, answered non-2xx or returned
  the wrong body; `2` = the run describes the client — answered < 99% of the offered rate, or
  median `lag` above one per-connection slot (`connections / rate`), or the connections could not
  be opened. No number from a run that exits 2 is a server measurement.
- **Sizing is stated, not defaulted.** The header and `BENCHMARKS.md` §4 say `connections ≳ rate ×
  expected latency`, and Windows additionally needs `rate / connections ≤ 500/s` (1 ms timer
  floor); the run refuses a pool that cannot pace before it starts.
- **Five defects found by running it, each fixed in the same file.** (1) The first draft opened a
  connection per request — no keep-alive; restructured to split halves with reconnect only on
  transport failure. (2) Windows sleeps in ~15.6 ms quanta: at 2000/s the run reported `lag` p50
  13.7 ms and 1846.9/s; fixed with an RAII `timeBeginPeriod(1)` guard (per-process since Win10
  2004), after which the same offer delivered 1989.9/s. (3) The generator-bound check compared
  `lag` p99 against the *global* slot (500 µs) instead of the per-connection slot, so a 1998.6/s
  run was refused; corrected. (4) `lag` p99 against one slot was then too strict for Windows
  jitter (a run that delivered 1994.5/s — 99.7% — failed at p99 4.7 ms vs a 4 ms slot); the gate
  is now answered-rate ≥ 99% plus `lag` p50 ≤ per-connection slot, with every percentile still
  printed as the run's own error bars. (5) The runtime default took every core, competing with a
  co-located server; `--client-threads` was added (default: half the cores, clamped 2..=4) after
  2 threads showed p99 13 ms in two of six runs at 2000/s while 4 and 8 stayed at 2.4–3.1 ms.
- **Verified live** against `basic_api` (release) on 127.0.0.1:3000, `/health`, body checked:
  2000/s × 3 s × 8 connections → `scheduled=started=completed=6000`, `lag` p50 735 µs / p99
  4249 µs, `latency` p50 339 µs, `wall_s=3.013`, answered 1991.1/s, **exit 0** (and 2000/s × 2 s
  answered 1999.0/s, exit 0, after formatting). Negative cases: wrong `--expect` → 1000/1000
  mismatches, **exit 1**; `--rate 5000 --connections 8` → refused with "625/s per connection …
  raise --connections to at least 10", **exit 2**; dead port 3999 → connect refused, **exit 2**;
  `--client-threads 0` → usage, **exit 2**; offered 20000/s over 120 connections → answered
  10082.8/s with a 5.951 s wall for a 3 s schedule, **exit 2**.
- **Not a result.** On this one 8-core box, offering 20 000/s with the generator co-located
  pushed measured `latency` p50 to 9.0 ms at 120 connections and 17.4 ms at 240, and the answered
  rate fell to ~10–11 k/s — the generator and the server are competing for the same cores, so
  neither number belongs to the server. Recorded here as a hypothesis about the harness, not as a
  throughput claim; the owner runs the measurement, ideally with the generator off-box.
- **Gates.** New file + `BENCHMARKS.md` §4, which states the sizing rule, the exit codes and that
  no number in that document comes from this tool. `rustfmt --check --edition 2024` clean on the
  new file. `cargo clippy -p aether-tests --all-targets -- -D warnings` is **red on this tree for
  pre-existing reasons** — reproduced with `cargo clippy -p aether-http --lib -- -D warnings`
  alone (9 errors: collapsible `if`, `map_or`, `type_complexity`, …), with the workspace run also
  failing in `aether-scheduler` (2) and `aether-validation` (1), i.e. none from this file and
  none from anything this track touched; it is the current toolchain's lints on code no track has
  re-linted since, and it stays red rather than being silenced. `fmt` repo-wide is still 570 hunks
  / 66 files and `validate` still exits 1 for the three historical ID reuses. **Nothing pushed.**
2026-10-25 (track 60 — **the counts the docs called current were re-derived, not carried forward; the
suite is 488, and the per-target split moved again**):
Every doc claim about the suite was a number copied from an earlier recount. The suite kept growing
after those copies, so the "current" splits were stale in seven files — root `README.md` still said 468 /
45 / 29 / 15, `framework/README.md` said 468 / 45 / 29 / 15 with 32 middleware tests, and the matrix's
evidence bullet and unit-test row said 474 / 271 / 198.
- **Measured, not asserted.** `cargo test --workspace` on this tree: **488 passed, 0 failed, 1
  ignored**, 50 `test result:` lines — 20 unit targets (277), 15 integration targets (206) and 15
  doc-test targets (5 non-zero). Integration: `http_e2e` 48, `http2_e2e` 36, `scheduler/tests/engine`
  21, `extractors` 18, `scheduling` 16, `test_client` 12, `tls_e2e` 9, `sim` 9, `user_middleware` 8,
  `pump_head_of_line` 8, `handler_exec_semantics` 7, `extensions_e2e` 6, `memory_reprice` 4, `stress`
  2, `policy_extension` 2 = 206. The `aether` lib is 39 (10 `tls.rs` + 13 `app.rs` + 8
  `extensions.rs` + 8 `http2.rs`), and the one ignored test is
  `response::tests::bench_date_write_paths` in the `aether-http` lib (a perf microbench).
- **The HTTP/2 split has a rule, so it stays.** 36 = 19 `H2Client::connect` over TLS + 14
  `connect_plain` over h2c + 3 hand-framed, classified per test by brace-matching each function body.
  The first script split the file on the literal `#[tokio::test]` and found zero tests: every
  attribute here carries `(flavor = "multi_thread", worker_threads = 4)`, so the marker never
  matched. The numbers now in the docs come from the fixed classifier, not from the earlier recount.
- **The middleware split moved too.** The mechanical rule the README already stated — test functions
  whose app ends up with a middleware, inline or via `ext_app` / `marked_app` / `extractor_app` —
  now yields **34, not 32**: `http_e2e` 13, `user_middleware` 8, `http2_e2e` **7** (was 5), and
  `extensions_e2e` 6. `scheduling.rs` now holds **16** tests, not 15; the scheduler crate is still
  57 (11 unit + 46 integration).
- **Docs swept.** Root and framework READMEs, `PRODUCTION_READINESS_REPORT.md` §7 and §10,
  `FEATURE_COMPLETENESS_MATRIX.md` (evidence bullet + unit-test row + HTTP/2 and h2c rows),
  `BASELINE_MEASUREMENTS.md` (its per-target table — the one table kept current in a doc whose
  performance half is superseded), `GETTING_STARTED_AR.md` and
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md`. Each corrected claim names the
  count and the grep or `test result:` line it came from, so the next reader can re-derive it.
- **Left dated on purpose.** PRR §1/§3/§7's 455–474 ladder, its §4 table and `Suite as of
  2026-10-23` block, `SCHEDULING_COMPARISON.md`'s "today (2026-10-23)" line, and
  `ACTIX_FAIR_COMPARISON.md`'s "474 on the 2026-10-23 tree" are records of a slice, and each carries
  its date; the register's B11 note says to re-derive rather than re-copy.
- **Gates.** Markdown only — no source changed. The suite run above is the measurement, exit 0.
  Clippy and `fmt` readings are unchanged from track 59 (`fmt` still 570 hunks across 66 files,
  `validate` still exit 1 for the three historical ID reuses). **Nothing pushed.**
2026-10-25 (track 59 — **register B8 closed: the external-client rows are re-run from the repository,
not from a directory the owner deletes**):
§11's HTTP/2 interop table had one qualifier left — the two clients were scratch files under
`C:/tmp/t30`, so a reviewer could read the commands and not run them. B8 allowed either "commit the
files" or "record the commands"; the commands were recorded on 2026-10-23, so this track committed
the files: `framework/tests/examples/node_h2_interop.cjs` and
`framework/tests/examples/h2_handframed_cap.py`, beside the `h2_interop_server.rs` they drive. Cargo
reads only `*.rs` in that directory, so the build is untouched.
- **The commit caught what the command record could not.** Those scripts ran as CommonJS only because
  `C:/tmp/t30` has no `package.json`; the repository root has one that declares `"type": "module"`, so
  the committed `.js` died with `ReferenceError: require is not defined` before its first assertion.
  Renamed to `.cjs`, reason recorded in the file and in §11. Recording commands alone would have
  published a command that does not run.
- **Measured on the current tree.** `cargo build --release -p aether-tests --example
  h2_interop_server`: exit 0, `Finished release profile in 3m 16s`, 3,979,776-byte artifact. Node
  **24/24 twice**, exit 0 (50 streams 325 ms; 110 cap-aware streams 759 ms; 1 MiB echoed as
  1,048,621 B; 5 MiB download 5,242,880 B; 413 with 0 echoed; both 431s; strict port 0 bytes 3/3;
  ALPN `h2` / `http/1.1` / none; `Upgrade: h2c` → `HTTP/1.1 200`). Hand-framed cap **PASS twice**:
  212 frames decoded, 100 × `200`, 10 × `RST_STREAM(7)`, 0 GOAWAY, 0 unresolved, post-storm `200 pong`.
- **Docs.** §11 gained a 2026-10-25 column and a reproduction block naming only repository paths; the
  matrix's "scratch script — not a gate a reviewer can re-run" clause is now "reproducible evidence,
  not part of `cargo test`"; the matrix's h2c row states that its python-h2 and HEADERS probes were
  **not** re-run, so the closure is not read wider than it is.
- **Gates.** Markdown, two non-Rust example files and no source change: the track-58 readings stand
  (488 tests / 0 failures / 1 ignored, clippy exit 0, `fmt` still 570 hunks across 66 files, and
  `validate` still exit 1 for the three historical ID reuses). PROB-0012 (Windows-only) is untouched.
  **Nothing pushed.**
2026-10-25 (track 58 — **the first track decided under the decision-independence policy: a `101` the
framework did not earn is refused everywhere, and HTTP/2 now answers instead of resetting the stream**):
Register item A6 had been parked as `FIX-DECISION (open)` since 2026-10-22, waiting for an owner
decision about how far a refusal should reach. Under `AGENTS.md` → *Decision independence* it is an
engineering choice with a defensible answer, so this track decides it instead of queueing it.
- **The decision: option (a), and two additions the evidence forced.** Option (a) was "reject a
  middleware `101` on any route". The rule is keyed to the status rather than the author or the route,
  because (i) a handler can return `101` too and the same claim is equally false, and (ii) over HTTP/2
  the old behaviour was not "served" but **no answer at all** — see the measurement below.
- **Where the rule lives.** The core's single response exit
  (`aether/src/serve.rs:401`, immediately before `post_process`), never in a frontend: one place, both
  protocols, and every author. A real handoff is untouched because the WebSocket branch returns
  `ServeOutcome::WebSocketUpgrade` before reaching it, so the `101` the frontend synthesizes never
  passes the check. The answer is `500 Internal Server Error`, the same one the handoff arm already
  gives a guard's `101` (A4), so the two refusals cannot drift.
- **Failing-first, measured on the pre-change tree.** `a_101_without_a_handoff_is_never_served`
  failed `left: 101, right: 500`: the guard's `HTTP/1.1 101` reached the wire, body-less, on a
  connection the server kept parsing as HTTP/1. `a_101_from_a_middleware_is_refused_over_h2` failed at
  the client's response head with `Reset(StreamId(1), PROTOCOL_ERROR, Library)` — `h2` rejects such a
  head outright, so the peer received nothing.
- **Sensitivity, both directions.** Removing the check restores `left: 101, right: 500` (the
  pre-change run above). Widening the condition to `response.status < 200` fails the same test
  `left: 500, right: 103` — the informational-class guard is still live, which is why the condition is
  `== 101` and not a range. Both were run, not argued.
- **Test bookkeeping.** The HTTP/1 test was **renamed**, not doubled:
  `the_101_refusal_is_scoped_to_websocket_upgrade_handoffs` → `a_101_without_a_handoff_is_never_served`
  (`tests/tests/http_e2e.rs:2308`), now expecting `500` for a guard's claim and for a handler's on an
  ordinary route, while still pinning `103`/`418` on a WS route. The protocol half is a new test,
  `a_101_from_a_middleware_is_refused_over_h2` (`tests/tests/http2_e2e.rs:544`). Middleware registered
  after a route does not wrap it, so that test registers the chain first — the first draft measured a
  `200` from the handler and had to be corrected before it proved anything.
- **Docs, all re-derived rather than searched-and-replaced.** `README.md`, `PRODUCTION_READINESS_REPORT.md`
  §15, `FEATURE_COMPLETENESS_MATRIX.md` (row + item-18 paragraphs), `GETTING_STARTED_AR.md`,
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md`, the `AetherApp::use_middleware` doc comment (`app.rs:997`) and
  the handoff-arm comment (`serve.rs:320-328`) all stated the old boundary as current. The register's
  A4 record keeps its 2026-10-22 text and is annotated as superseded; A6 is marked FIXED with the
  decision, the location and the measurements.
- **A stale number found while correcting that one.** The Arabic full documentation claimed
  `http2_e2e` had 29 tests "in the current tree"; the target actually holds 35 before this track's
  test and 36 after. Corrected there, with the 29 called what it was — already stale before this
  change — rather than silently overwritten.
- **Gates on the tree this produces.** `cargo test --workspace` → **488 passed / 0 failed / 1
  ignored** (50 result lines; 277 unit + 206 integration + 5 doc; the +1 is the new `http2_e2e` test).
  `cargo clippy --workspace --all-targets` exit 0. `fmt` was not re-run and stays red as recorded (570
  hunks / 66 files) — this track does not start the repo-wide reformat. `validate` still exits 1 for
  the three historical ID reuses. **Nothing pushed.**
- **What this track does not decide.** The rest of the informational class: a middleware's `103` still
  travels untouched (pinned by the same test), and whether an application may author informational
  responses at all remains the wider contract A6 named. A6 is not a `problems.json` item, so no ID was
  closed; the open problem list is unchanged at 8.
2026-10-25 (track 57 — **the owner's decision-independence policy is now a permanent project
instruction, not a one-session understanding**):
The owner delivered the policy in Arabic and asked for it to be kept in permanent memory *and* in a
durable in-repo instructions file, "so it remains a reference in future sessions and tasks".
- **Canonical copy: root `AGENTS.md`.** That file is the one loaded as project instructions, so the
  policy sits there verbatim in the owner's Arabic, followed by a faithful English reading and one
  paragraph naming what it does *not* change. `.project-governance/SYSTEM_PROMPT.md` gained a short
  *Decision Authority* section that points to `AGENTS.md` instead of restating the text, so there is
  one copy to edit and no second version to drift out of sync.
- **What the policy says.** The agent is the engineer responsible for Aether rather than a literal
  executor: it takes the engineering decisions the vision needs, does not wait for approval on small
  steps or ask the owner to pick algorithms and implementation details it can judge itself, traces a
  problem to its root cause and picks the best-evidenced option — explaining why when that overrides
  a weaker suggestion — gathers evidence and tests hypotheses when information is incomplete, and
  asks only about a material change of scope, an irreversible risk, or a personal preference it
  cannot infer.
- **What it does not say.** No new permission. The paragraph in `AGENTS.md` lists the constraints
  that survive untouched: no push, no amend, no history rewrite, no merge or out-of-scope change
  without approval; no performance number the owner did not measure; a failed gate recorded as
  failed; the independent reviewer and `C:/tmp` isolation rules of `OWNER_VISION.md` (owner rulings
  2026-10-24). A policy about how far to decide on its own cannot be read as authorising the
  operations the owner already reserved to himself.
- **Persistent memory.** Saved as a `feedback` memory pointing at `AGENTS.md` as the authoritative
  text, so a later session recalls the rule from memory and reads the full wording from the repo.
- **Gates.** Markdown and a memory file only: no Rust source, no tests, no governance-tool change, so
  the track-56 readings stand and were not re-run (487 tests / 0 failures / 1 ignored, clippy exit 0,
  `fmt` still 570 hunks across 66 files, and `validate` still exit 1 for the three historical ID
  reuses). **Nothing pushed.**
2026-10-24 (track 56 — **the last three fixable findings from the track-54 review: a decode failure
is a refusal, a malformed element is refused where the file is read, and closure rolls back when its
record cannot be written**):
Track 55 closed `promote_item` and `log`. Three findings remained, all of the same kind — a command
that cannot finish must leave the files as they were. All three are now fixed and tested; the one
residual that cannot be fixed without a journal is stated at the end.
- **A non-UTF-8 ledger is refused.** `_ledger_records()`
  (`tools/project_governance_bootstrap.py:75`) caught only `OSError`, but `Path.read_text` decodes,
  so invalid UTF-8 raises `UnicodeDecodeError` (a `ValueError`): `validate` printed a traceback and
  exited 1 like a failing gate. It now raises `GovernanceError` too, and the CLI prints `[ERROR] …`
  and exits 2. This closes the caveat track 55 attached to track 54's sentence "a log that cannot be
  read at all is refused".
- **Elements are checked, not only the file.** `load_json_list()` (`:36`) verified that the file
  holds a list, then handed its elements to readers that call `.upper()` and `norm()`: a number, a
  missing `id` or `title`, or a non-string `description` crashed `validate()`/`add_item()` with
  `AttributeError`/`TypeError`/`KeyError`. Each element is now refused with its 1-based position
  unless it is an object with a string `id` and `title` and, when present, a string `description`.
  A refused read never rewrites the file.
- **Closure rolls back.** `done_item()` (`:362`) removed the item and then wrote
  `done_and_deleted`; a refused write left the item gone with no record. It now restores the list it
  read and re-raises, so the closure either completes with its record or does not happen.
- **Tests, measured against `3400db0` (track 55).** `tests/test_id_allocation.py` is 26 cases (was
  21); the five new ones run against the previous tool give **5 red / 21 green** (unittest reports 4
  failures + 5 errors because the element-shape case covers five inputs as subtests). Red: the
  non-UTF-8 ledger, the non-object element, each shape case, the CLI element refusal, and the
  `done_item` rollback. 26/26 pass with `python -B -m unittest test_id_allocation`.
- **Residual, not fixable by code.** A failure on the final `promoted` record still exits 2 with the
  promotion complete: the `new_requirement` allocation is already in the ledger, so the number stays
  protected, but the `promoted` marker is missing. Recording what happened cannot be made atomic
  against a write failure without a journal, and the ledger stays append-only.
- **Gates.** Python only; Rust stays at the track-52 tree (487 / 0 / 1, clippy exit 0, fmt 570 hunks
  across 66 files). The repository's own `validate` still exits 1 for the three historical reuses
  (PROB-0007 ×2, PROB-0017 ×2, REQ-0001 ×3) and the open REQ-0001 — 0 unlogged, 29 allocations,
  0 malformed. **Nothing pushed.** Coordinates in the track-55 entry are that tree's; this track
  moved `load_json_list` to `:36`, `log` to `:64`, `_ledger_records` to `:75`, `add_item` to `:332`,
  `done_item` to `:362`, `promote_item` to `:385` and `validate` to `:440`.
2026-10-24 (track 55 — **two follow-up defects in the governance tool: promotion creates its
replacement before deleting its source, and an unwritable ledger is a reported refusal**):
The limited review of track 54 found four defects beyond PROB-0019 itself. The owner asked for the
two in `promote_item` and `log`; the other two are recorded at the end as still open.
- **Promotion is create-before-delete.** `promote_item()`
  (`tools/project_governance_bootstrap.py:366`) deleted the source, wrote the `promoted` record, and
  only then called `add_item()` — which reads (and can refuse) the lists and the ledger. On a copy of
  the `f65d9d5` tree with a corrupt `requirements.json` that left the source deleted,
  `requirements.json` untouched, and a `promoted` record in the ledger (CLI exit 2): the ledger
  claimed a promotion that never happened. The function now reads all four lists before its first
  write, creates the requirement, and deletes the source only after `add_item()` returned 0; a
  `GovernanceError` from `add_item()` restores `requirements.json` to the IDs that existed before the
  call, so a refusal inside the creation leaves both sides as they were. A failure on the final
  `promoted` record still exits 2 with the requirement created and the source gone — the
  `new_requirement` allocation is already in the ledger, so the number stays protected. The
  duplicate-title check moved with the creation: a taken title now returns 2 with the source still
  open, where the old order deleted first and failed second. `add_item()` (`:320`) gained
  `exclude_id` so a promotion may keep the item's own title without matching itself.
- **`log()` refuses instead of crashing.** `log()` (`:54`) wraps the append in `except OSError` and
  raises `GovernanceError`, so the CLI prints `[ERROR] …` and exits 2 instead of a traceback after a
  caller has already changed a list. It does not make the callers atomic: `done_item()` still
  deletes before it logs, so a failed write there now exits 2 with the item already gone and no
  `done_and_deleted` record. Recorded here as an open gap, deliberately not changed in this track.
  (Closed in track 56: `done_item()` now restores the list on a refused write.)
- **Tests, measured against `f65d9d5`.** `tests/test_id_allocation.py` is 21 cases (was 16); five
  are new, and the current file run against the pre-fix tool gives **4 red / 17 green**. Red:
  corrupt-file promotion (source lost), ledger refusal during promotion (source lost), taken-title
  promotion (source lost), and `log()` against an unwritable path (raised `PermissionError`, not
  `GovernanceError`). Green before, kept as a guard: unchanged-title promotion — it passes on the
  old code only because the duplicate scan runs after the source is deleted, and it is the test that
  fails if `exclude_id` is dropped. 21/21 pass with `python -B -m unittest test_id_allocation`.
- **Still open from the same review.** (1) `_ledger_records()` (`:65`) catches only `OSError`, so a
  ledger that is not valid UTF-8 raises `UnicodeDecodeError` before the wrapper and the CLI prints a
  traceback; track 54's "a log that cannot be read at all is refused" holds for OS-level read
  failures, not for a decode failure. (Fixed in track 56: `UnicodeDecodeError` is refused too.)
  (2) A list entry that is not an object, or whose `id` is not a
  string, still crashes `validate()`/`add_item()` (`AttributeError: 'int' object has no attribute
  'upper'`), so the element shape is unchecked while the file shape is checked. (Fixed in track 56.)
- **Gates.** Python tooling only; the Rust gates are unchanged and remain tied to the track-52 tree
  (487 / 0 / 1, clippy exit 0, fmt 570 hunks across 66 files). The repository's own `validate` still
  exits 1 for the three historical reuses (PROB-0007 ×2, PROB-0017 ×2, REQ-0001 ×3) and the open
  REQ-0001, with 0 unlogged IDs, 29 allocations and 0 malformed lines — red by fact, not by this
  track. Coordinates in the track-54 entry below are that tree's; this track's insertions moved
  `add_item` to `:320`, `promote_item` to `:366` and `validate` to `:421`. **Nothing pushed.**
2026-10-24 (track 54 — **limited review of the governance tool: the reader declares its corruption
behaviour, and `validate` itself now detects ID reuse**):
The owner's review accepted the track-53 fix but asked for four things before treating PROB-0019 as
finally closed. Addressed as follows.
- **ID-allocation integrity (declared, not incidental).** `_ledger_records()`
  (`tools/project_governance_bootstrap.py:60`) is now the single reader of `event_log.jsonl`. A line
  that is not JSON, or JSON that is not an object, is named on stderr with its line number and
  skipped; every other line still counts, and a record whose `id` is not a string contributes
  nothing. A log that cannot be read at all is refused (`GovernanceError`) instead of being read as
  empty, because an empty ledger restarts the numbering at 0001. (Limit found in track 55: only
  `OSError` is wrapped, so a ledger that is not valid UTF-8 raises `UnicodeDecodeError` first.)
  `ledger_ids()`'s docstring states
  this as a limit: an ID recorded only on a corrupt line is invisible to allocation, and the warning
  is what makes that gap visible rather than silent.
- **The same silent reset existed one level up.** `load_json_list()` (`:36`) read an unparsable
  `problems.json` as `[]`, so the next allocation restarted at PROB-0001 and then overwrote the
  damaged file — both a lost high-water mark and lost open items. It now raises `GovernanceError`
  for an unreadable, unparsable, or non-list file (a *missing* file is still `[]` for `init`), and the
  CLI prints `[ERROR] …` on stderr and exits 2.
- **`validate` checks the history, not only the open lists.** New `ledger_allocations()` (`:111`)
  counts only `new_*` records — `done_and_deleted` and `promoted` carry an ID without handing out a
  new one, so counting them would make every closed item look like a reuse — and `validate()`
  (`:399`) reports `[RECYCLED ID]` for any ID allocated more than once, `[OPEN VS HISTORY]` when one
  of those uses is currently open, and `[UNLOGGED ID]` for an open item with no allocation record
  (its number is protected only while it stays open). Each sets exit 1.
- **Tests, failing first.** `tests/test_id_allocation.py` went from 6 cases to 16. Ten are new, and
  the current file run against the `HEAD` tool gives 7 red / 3 green: red were no warning for a
  malformed line, an `AttributeError` on a valid-JSON non-object line (a crash, not a skip), the two
  reuse cases plus the unlogged-ID case returning 0, `validate` losing a recycle that sits around a
  corrupt line, and the corrupt-list case raising `AttributeError: GovernanceError` before the class
  existed. Green before the change and kept as guards/coverage: the `promote_item()` allocation path
  (it already allocated through `add_item`, so it documents the fix rather than proving one), a
  clean ledger still passing `validate`, and the duplicate-title check. After the change, 16/16 pass
  with `python -B -m unittest test_id_allocation` (without `-B` the test module's own `.pyc` lands
  in `tools/tests/__pycache__`; that artifact was removed and is not tracked).
- **The repository's own `validate` is now red, factually.** It exits 1 with three
  `[RECYCLED ID]` lines — PROB-0007 ×2, PROB-0017 ×2, REQ-0001 ×3 — and one `[OPEN VS HISTORY]` for
  the open REQ-0001; 0 `[UNLOGGED ID]`, 29 allocations parsed, 0 malformed lines. This is the
  historical ambiguity the fix cannot remove without rewriting history, which stays refused. It is
  a red state like the repo-wide `fmt` run and must not be recorded as green; it can only change by
  policy, not by a code change.
- **What the ambiguity means for citations.** A bare `PROB-0017` or `REQ-0001` in older text is not
  resolved by anything in this track. The authoritative reading is the timestamped ledger entry;
  the two PROB-0017 uses are the WebSocket middleware bypass and the HTTP/1 write buffer, and the
  three REQ-0001 uses are three different requirement statements.
- **Gates.** Python tooling only, so the Rust gates are unchanged and remain tied to the track-52
  tree (487 / 0 / 1, clippy exit 0, fmt 570 hunks across 66 files). **Nothing pushed.**
2026-10-24 (track 53 — **PROB-0019 closed: governance IDs are allocated from the append-only ledger,
not from the items still open**):
Chosen under the same delegation, because it was the only open problem that could be settled on this
host without a production change, a public API, or the D2 accounting surface. `next_id()` numbered
from the items still in the file and `done_item()` deletes on closure, so closing the newest item
handed its number back out. The ledger shows this happened three times, not once: PROB-0007 (router
param merge, then connection memory), PROB-0017 (WebSocket middleware bypass, then the HTTP/1 write
buffer — the instance already registered), and REQ-0001 (three different requirements). The fix is
one high-water source: `ledger_ids(root, prefix)` (`tools/project_governance_bootstrap.py:89`) reads
`event_log.jsonl`, `next_id()` (`:125`) takes the maximum over the open items and that set,
`add_item()` passes it in (`:334`), and `promote_item()` inherits it because it allocates through
`add_item`. Closed items stay out of the lists, so nothing downstream of `problems.json` changes
shape. (Coordinates re-derived on the track-54 tree, which inserted above all three.)
- **Failing-first test.** `.project-governance/tools/tests/test_id_allocation.py` (6 cases) was
  written and run against the unfixed tool: 4 failed — a closed PROB-0001 came back as PROB-0001,
  the real PROB-0017 shape came back as PROB-0001, an open PROB-0005 plus a closed PROB-0009
  produced PROB-0006 instead of PROB-0010, and a malformed ledger line fell back to PROB-0001. After
  the fix all 6 pass. The two that passed before are guards, not evidence: no ledger still yields
  PROB-0001, and a PROB ID does not advance the REQ counter.
- **The real ledger shows the difference.** On a copy of `.project-governance`, closing PROB-0023
  and adding a problem gave PROB-0023 with the `HEAD` tool and PROB-0024 with the fixed one. On the
  repository itself `status` and `validate` still passed under the tool as it stood then — track 54
  gives `validate` the ledger check, and it now reports these three reuses as a red state — and the
  tool's own `done` removed PROB-0019, leaving 8 problems.
- **What is not fixed.** History is not rewritten: the 14 PROB-0017 citations and the PROB-0007 /
  REQ-0001 ones keep whatever meaning their context gives them, and the ledger is the only place
  both meanings coexist. The fix makes the collision impossible from now on; it does not make an old
  bare reference unambiguous. No shipped doc is renumbered and no closed item is resurrected into
  `problems.json`.
- **Gates.** The changed file is Python tooling, so the Rust gates are unchanged by this track; the
  last full measurement stands as recorded under track 52 (487 / 0 / 1, clippy exit 0, fmt 570 hunks
  across 66 files). The evidence here is the 6 unittest cases (4 red before, 6 green after) and the
  copy-of-ledger before/after above.
- **Nothing pushed.** Local commit only, under the same delegation.
2026-10-24 (track 52 — **PROB-0022's pre-declared Confirm branch is measured; the hazard is pinned
by a test that has to be inverted when a policy lands, and the problem stays open by decision**):
The next slice was chosen under the standing delegation ("ضلك قرر لوحد حسب الانسب والاكثر قوة"):
PROB-0022 was the only open defect whose Confirm criterion could be settled on this host. No
production line changed — the missing bound is a policy choice, not a bug to patch — no public API
or config was added, and no speed claim is made. The slice is one test (+97 lines in
`framework/tests/tests/http2_e2e.rs`) plus the governance records beside this one.
- **The test is the pre-declared check, not a new claim.**
  `a_trickling_reader_keeps_an_h2_stream_alive_past_every_configured_deadline` (`http2_e2e.rs:923`)
  sets both `keep_alive_timeout` and `max_response_time` to 1000 ms, serves 256 KiB, and grants
  exactly one byte of credit every 200 ms for 3500 ms. No 1000 ms interval passes without a grant,
  so the write deadline never fires; the window is three whole deadlines, so survival cannot be an
  accident of scheduling. Both premises are pinned as arithmetic in `const` assertions. The test
  drains the first burst before the window opens, so in-flight bytes cannot stand in for progress:
  any stream end inside the window fails it, the count must then have grown past the drained
  baseline (credit, not a hung-open stream), it must stay below the full body (the bound, not
  throughput), and a HEAD on the same connection must answer 200 (per-stream survival, not a
  connection-wide deadline).
- **It passes on this tree (4.04 s) and two mutants are rejected.** Removing the renewal at
  `http2.rs:658` cuts the stream with `CANCEL` after 65,537 bytes in 1.29 s and fails the test.
  Holding the stream open while sending nothing (`std::future::pending` for the rest of the body)
  fails the movement assertion in 4.27 s — the case the first version of the test would have
  passed. `http2.rs` was restored byte-for-byte after each run (sha256 `1b08881c…`).
- **The independent review found a real hole, and it is closed.** A fresh-context agent (not this
  session) found one SERIOUS defect in the first version: `received > 0` was already satisfied by
  the pre-loop read, so a server that hung the stream open without moving a byte passed every
  assertion. The fix is the drain-then-require-movement structure above. The same review judged
  the wrong-reason routes closed (h2 0.4.20 gives the client a 65,535-byte stream and connection
  window and replenishes only on `release_capacity`, so the server is genuinely parked), corrected
  a doc-comment overclaim (a total cap longer than the 3500 ms window would not trip this test),
  and noted that `keep_alive_timeout` cannot fire while a stream exists — the timer isolated here
  is `write_timeout`, which the comments now say.
- **The hazard is deliberately not fixed here.** The only bound that ends a trickle is a total
  per-stream cap, and D1 rejected it because it truncates a legitimate long download; the
  concurrency cap bounds how many streams exist, not how long one lives. So the honest outcome is
  the confirmed reading plus a tripwire: whoever implements such a policy has to invert this test,
  and cannot call the hazard closed while it still passes. **PROB-0022 remains open.**
- **Gates on the tree this commit produces.** `cargo test --workspace` **487 passed / 0 failed /
  1 ignored** — 277 unit + 205 integration + 5 doc, 50 result lines, exit 0 — with the new test
  named `ok` in the log. `cargo clippy --workspace --all-targets` exit 0 at 97 warning lines, and
  all 16 locations inside the two h2 files (`http2.rs:420, :465, :559, :833, :834`;
  `http2_e2e.rs:166, :185, :456, :546, :547, :596, :597, :617, :618, :1221, :1222`) sit on
  pre-existing lines, none inside the added range 922–1037. `cargo fmt --all --check` is **still
  red: exit 1, 570 hunks across 66 files**, and the added-line intersection — recomputed against
  the lines rustfmt wants to *rewrite*, not the hunk headers — reports 57 rewrite lines across the
  two files (`http2.rs` 19, `http2_e2e.rs` 38) and **0 among the 116 added lines**. The review fix
  first added three such lines itself (a `panic!` arm rustfmt wanted as a block); that was
  corrected before this reading, which is why the hunk total is back at 570 rather than 571. The
  file count is 66, not the 58 an earlier record carried: the hunk total is the same 570, so that
  figure was a parsing artifact of the same log, not evidence of a different tree. The full fmt
  gate is not claimed green.
- **Nothing pushed.** The test and these records were committed locally only, under the same
  delegation that covered the earlier tracks.
2026-10-24 (track 51 — **PROB-0020 closed: an HTTP/2 response body is now priced by the block that
holds it**; PROB-0022 restated and still open, PROB-0023 registered as the residual):
The owner's instruction was to fix the cost and the critical point and to make the design call
here rather than ask again. The critical point was that a stream carrying a 64 MiB body registered
49,152 B — its base and nothing else — while the process held 67,257,004 B. The change is confined
to two files (+387/−20 in `framework/aether/src/http2.rs`, 14 lines in
`framework/tests/tests/http2_e2e.rs`), keeps `StreamCharge`
internals and the accounting path untouched (D2), and adds no public API or config.
`price_response_body` (`http2.rs:604`) recovers the allocation with `Bytes::try_into_mut()` and
registers `block.capacity()`, never `len()`; `send` (`http2.rs:512`) prices **before** queueing the
response head (`http2.rs:568-574`), so a client that can hold the head can also see the charge that
covers the body under it. `try_into_mut` is zero-copy and succeeds only when this stream is the
block's sole owner, so the two shapes that decline it are left uncharged on purpose rather than
copied into fresh memory to create something to charge: `.rodata` is not per-stream heap, and a
block another handle shares is not this stream's to price. The charge is best-effort, the response
is not — a budget that refuses does not withhold an answer the handler already produced, which is
the HTTP/1 policy (`app.rs`) carried onto h2.
- **Tests first, against the old tree.**
  `a_response_body_is_registered_by_its_block_while_the_stream_holds_it` (`http2.rs:1264`) was run
  before the production change and failed reading **49,152 against 1,359,872**. It serves 1 MiB
  from a 1.25 MiB `Vec` on purpose: a length-based charge registers 1 MiB and cannot satisfy the
  exact total `CONN_BASE + STREAM_BASE + BLOCK`. `a_response_body_over_the_budget_is_sent_anyway`
  (`http2.rs:1347`) puts a 576 KiB block into a 128 KiB budget. Both drive `serve` directly through
  helpers new to this slice (`body_router:1125`, `scheduled_core:1144`, `ScheduledConn:1175`,
  `scheduled_connection:1195`, `registered_until:1231`).
- **Four mutants, each rejected by a named test** (`C:\tmp\mutate_price.py`, restored byte-for-byte
  each time): charge `len()` instead of `capacity()` → test 1 fails 1,097,728 vs 1,359,872; treat a
  refusal as a reason to shed → test 2 fails 32,768 vs 49,152; release nothing on `Drop` → both
  fail, test 1 at 1,359,872 vs 32,768 after the drain and test 2 in `registered_until`; delete the
  pricing call entirely → test 2 fails on `memory_shed` (`http2.rs:1391`), test 1 fails 49,152 vs
  1,359,872, **and** the end-to-end `a_reader_that_stops_gets_a_stream_reset_without_killing_the_connection`
  fails at `49,152 vs 32,768`.
- **The independent review ran, and changed the tests.** A fresh-context agent (not this session;
  the first attempt died on a credit limit, the second completed) returned one SERIOUS finding:
  test 2's registration reading was vacuous, because pricing that never ran and pricing the budget
  refused produce the same number. It now also asserts `layer.metrics().memory_shed > 0`
  (`http2.rs:1391`), which only an attempted-and-refused grow moves
  (`scheduler/src/engine.rs:301-311`), and the same agent's premise complaint is pinned as
  arithmetic — `const _: () = assert!(BODY > 65_535)` (`http2.rs:1272`) — because the parked-task
  claim depends on the client's default stream window and on this test never calling
  `release_capacity`. Its fourth finding was documentation rot outside this file:
  `http2_e2e.rs:855-866` still asserted `during > open` and commented that body bytes "stay outside
  the scheduler (PROB-0020)" — that comment is now false, so the assertion is `during >= open +
  256 KiB`, which is exactly what the no-pricing mutant fails. Three further findings were
  judged rather than obeyed: the set-semantics single-block charge, the stale inbound charge on
  the framework error paths, and the `from_owner` gap are real, documented and now written into
  `send`'s and `price_response_body`'s doc comments (`http2.rs:505-511`, `:584-603`) and into
  **PROB-0023**; repricing them would have widened D2 for no measured gain.
- **The probe reverses the measurement (D4).** `aether-bench-compare/examples/h2_stall` rebuilt
  against the fixed tree, 64 MiB body, h2c, client withholding every WINDOW_UPDATE: registered
  holds **67,158,016 B = CONN_BASE + STREAM_BASE + the exact 67,108,864 B block** for the whole
  8 s stall while the counting allocator reads 67,257,761 B live; a second pass at 5 s stall reads
  the same registered figure against 67,258,081 B live. After the client resumes, all
  67,108,864 bytes arrive and the connection ends at `keep_alive_timeout` with live 92,372 B and
  registered 0. Track 50 read 49,152 B registered against 67,257,004 B live on the same harness, so
  PROB-0020's pre-declared Refute branch is what was measured. Throughput, latency and CPU/RAM were
  not measured here — that is the owner's bench, and no speed claim is made.
- **Gates on the tree a commit would actually produce.** The export had drifted: `C:\tmp\track50-tree`
  was copied at 03:04 and the working tree's `serve.rs`, `app.rs`, `http_e2e.rs` and `parser.rs` were
  last written at 04:07-14:51, so the tree that had been gated was not the tree a commit would make.
  Those four were reset to their `HEAD` bytes and the whole tree re-verified against
  `git ls-files` — 194 tracked files compared, none missing, and exactly two differing from `HEAD`:
  `http2.rs` (sha256 `1b08881c…`) and `http2_e2e.rs` (`17eef00f…`). On that tree:
  `cargo test --workspace` **482 passed / 0 failed / 1 ignored** over 50 result lines (15 of them
  doctest targets), exit 0; `cargo test --workspace --all-targets` **477 / 0 / 1** over 38 result
  lines, exit 0, with all five slice tests named in the log; `cargo clippy --workspace --all-targets`
  exit 0 with 97 warning lines, and every one of the 16 locations inside the two slice files
  (`http2.rs:420, :465, :559, :833, :834`; `http2_e2e.rs:166, :185, :456, :546, :547, :596, :597,
  :617, :618, :1105, :1106`) sits on a line this slice did not add. `cargo fmt --all --check` **still
  fails: exit 1, 573 hunks across 58 files** — and the same command on the HEAD-only tree, with these
  two files reset, reports **576 hunks across 58 files**, so the slice removes 3 fmt hunks rather than
  adding any (`http2.rs` 14 → 11, `http2_e2e.rs` 18 → 18). The added-line intersection was recomputed
  against the lines rustfmt wants to *rewrite* instead of the hunk headers (a header names the first
  context line, three above the change, so header-level intersecting silently mis-attributes): **57
  rewrite lines across the two files (`http2.rs` 19, `http2_e2e.rs` 38), 0 of them among the
  804 + 316 lines the slice added.** The first reading of this gate (482 on a drifted tree,
  "three `collapsible_if`", "573 hunks across 66 files", 33/60 dirty lines, header-level
  intersection) was taken before the reset and with that offset uncorrected — and its file count came
  from a `grep "^Diff in "` that missed headers carrying ANSI prefixes; the numbers above supersede it.
- **Commit state at review: nothing staged, nothing committed, no push.** The code half of the scope
  is clean and
  the governance half is not, and that asymmetry is the decision the owner has to make:
  `git status` lists 13 modified paths. Two are this work — `http2.rs` and `http2_e2e.rs` — and they
  are separable on their own, as the tree above proves. The other eleven split three ways:
  four governance paths (`WORKING_CONTEXT.md`, `event_log.jsonl`, `problems.json`, this register) that
  hold **eight uncommitted dated records, tracks 44–51**, mixed in the same files, so no per-track
  governance commit is available without splitting blocks by hand; two doc paths
  (`FEATURE_COMPLETENESS_MATRIX.md` one row, `PRODUCTION_READINESS_REPORT.md` one paragraph) that this
  track edited and that were *not* in the owner-approved scope; and five paths belonging to earlier
  tracks (`parser.rs`, `serve.rs`, `app.rs`, `tests/http_e2e.rs`, `OWNER_VISION.md`) that are
  G15/harness/vision work never committed and must stay out. The exact split (`git diff --numstat`,
  additions/deletions against `HEAD`): this slice is `http2.rs` 804/47 and `http2_e2e.rs` 316/0; the
  governance paths are `WORKING_CONTEXT.md` 476/3, `notes/…-improvement-register.md` 111/2,
  `problems.json` 13/13, `event_log.jsonl` 6/0; the two docs are `FEATURE_COMPLETENESS_MATRIX.md` 1/1
  and `PRODUCTION_READINESS_REPORT.md` 6/3; the five earlier-track paths are `parser.rs` 254/111,
  `serve.rs` 48/19, `app.rs` 16/4, `tests/http_e2e.rs` 54/0, `OWNER_VISION.md` 313/0. Splitting the
  two code tracks into two commits is still possible (`C:\tmp\http2-final-clean.rs`
  is track 50's `http2.rs`, sha256 `a390085d…`, and `C:\tmp\track50-e2e.diff` its test half), but note
  the dependency direction: track 51's change at `http2_e2e.rs:855-865` asserts
  `during >= open + 256 KiB`, which the unpriced track-50 tree cannot satisfy — so that block belongs
  in the second commit, not the first.
- **Landed the same day, on the owner's delegation ("قرر لوحدك"):** three local commits — `1497cdc`
  (the two h2 files), `a31b4c0` (the two doc paths), `83bf7b4` (these four governance files) — and no
  push. After them, every build input (`.rs`, `Cargo.toml`, `Cargo.lock`) in the gated tree compares
  byte-equal, line endings aside, to the new `HEAD`, so the tree the gates measured is the tree that
  was committed. The five earlier-track paths stayed out and remain modified in the working copy.
- **The five earlier-track paths followed, on the same delegation.** `c097db0` takes the allocation
  slice (`app.rs` 16/4, `serve.rs` 48/19, `tests/http_e2e.rs` 54/0) and `23999a7` the parser's
  request-sized slot array (`parser.rs` 254/111); this commit takes `OWNER_VISION.md` (313/0), which
  also makes the citations already in this record (`OWNER_VISION.md:148-150`, the per-cycle reviewer
  rule) resolve in history. Gates, each on its own exact tree: HEAD + the allocation slice →
  `cargo test --workspace` **483 / 0 / 1** over 50 result lines and clippy `--workspace --all-targets`
  exit 0; that tree plus `parser.rs` → **486 / 0 / 1** and clippy exit 0 at the same 97 warning lines.
  Repo-wide `cargo fmt --all --check`: old HEAD 576 hunks, with the h2 slice 573, with these two
  **570**, all across 58 files — still red, as recorded above. No added line in any of the four files
  has a fmt hunk. One clippy location does land on an added line: the `collapsible_if` the parser
  split relocated verbatim (`parser.rs:180` at HEAD → `:258` now), left alone rather than editing
  another track's code for a cosmetic lint. Nothing pushed.
2026-10-24 (track 50 — **the HTTP/2 write path is paced and the idle timer no longer cuts a live
response**; PROB-0015 and PROB-0021 closed, PROB-0020 still open):
The owner's D1–D4 were applied as written: `write_timeout` is the longest *no-progress* interval for
an h2 write (never a total response cap), no public API or config changed, and the slice is confined
to `http2.rs` (no touch to `StreamCharge::buffer_set` or the accounting path). The connection loop now
keeps `conn.accept()` as the driver whenever streams exist and gives shutdown its own `draining`
state with `drain_budget` (`framework/aether/src/http2.rs:130-213`, the same
`keep_alive_timeout.min(30s)` expression the listener uses at `app.rs:2149`); `write_body`
(`http2.rs:556`) hands `h2` at most 64 KiB per `send_data`, reserves capacity per wait, and renews
its deadline only after a `send_data` that consumed real capacity, so a stalled peer gets
`RST_STREAM(CANCEL)` on that stream and the connection survives. The HTTP/1 *total* cap versus
HTTP/2 *no-progress interval* divergence is stated at the function as intentional
(`http2.rs:474-483`).
- **Tests first, against the old tree.** The four tests were added and run before the production
  change: `an_active_response_outlives_the_idle_keep_alive_timeout` failed at ~0.96 s with
  `ConnectionAborted` while the body was still being read;
  `a_reader_that_stops_gets_a_stream_reset_without_killing_the_connection` failed with "server never
  reset a reader that stopped consuming" at 2.54 s; the two guards
  `an_idle_h2_connection_still_closes_after_keep_alive_timeout` (0.31 s) and
  `a_draining_h2_connection_abandons_a_stream_at_the_drain_budget` (0.43 s) passed on the old tree
  and had to keep passing. After the fix all four pass (`http2_e2e.rs:736, 814, 921, 957`): T1
  multiplexes a HEAD while the slow stream waits for credit, and runs with an 800 ms
  `write_timeout` against a ~2.4 s transfer so a total per-response cap would fail it; T2 asserts
  the reset reason is CANCEL, the release of the stream's registration and a HEAD on the same
  connection; T3 keeps the idle close between 150 ms and 1 s; and T6 proves the combined shutdown
  path honours the 300 ms budget and cuts the stalled stream with an error, not a clean end. T6
  cannot isolate the connection-level deadline: the listener shares the same expression and
  force-aborts the connection at it.
- **The owner-requested isolation of that deadline** is now its own test:
  `the_connection_cuts_a_stalled_stream_at_its_own_drain_deadline` (`http2.rs:794-1051`) builds a
  minimal `ServiceCore` inside the crate's unit tests and calls `serve` directly over a loopback
  socket, with no listener in the path. It opens two streams: a 1 MiB body whose client never
  releases capacity (so its task parks on `poll_capacity`) and a 128 KiB body that is read only
  after 75% of the budget, on a client that advertises a 4 MiB *connection* window so the stalled
  stream cannot starve the second one. The case runs at three budgets — 200, 300 and 600 ms — with
  pairwise-disjoint acceptance windows `[200, 300)`, `[300, 450)` and `[600, 750)`, so no constant
  cut time can satisfy all three (a hardcoded 400 ms cut measured 405.3 ms, a
  `max(budget, 400 ms)` floor 423.1 ms, and a 300 ms floor 318.2 ms, outside the lowest window).
  The connection is still alive after `2 × budget + 150 ms` against a `budget`
  `keep_alive_timeout`; shutdown then ends it inside one budget (baseline over three passes:
  203-232 / 325-330 / 606-627 ms) while the progressing stream delivers all 128 KiB, the client
  driver is still alive at the cut (so the end came from the server's deadline, not a client
  close), and the stalled stream ends with an error after delivering more than 0 and less than
  1 MiB. The delayed read is what separates the two deadline shapes: without a mid-drain wake a
  sliding deadline is indistinguishable from an absolute one, and with it the re-armed shape cuts
  at ~1.75× (381.4 ms at a 200 ms budget) and fails, while the absolute one stays inside the
  window. The test also pins `conn.accept()` as the driver — a loop that exits at shutdown
  reproduces the cut time through the post-loop budget but stops accepting, so the progressing
  stream cannot finish. Eleven mutation shapes were run against a scratch copy and every one
  measured fails: constant 400 ms (405.3), `max(budget, 400 ms)` (423.1), 300 ms floor (318.2),
  sliding deadline (381.4), fresh post-loop budget (422.1), appended 100 ms post-loop grace
  (312.6), the old loop shape, `drain_deadline = None`, a disarmed loop timer (2 s outer timeout),
  a client abort, and break-at-shutdown. Two wrong shapes pass and are recorded, not claimed: an
  appended 50 ms post-loop grace (267.2 / 379.7 / 676.1 ms) and routing a draining connection with
  live streams into the idle branch (223.4 / 320.0 / 631.2 ms) — the latter is observationally
  identical because `drain_budget` is `keep_alive_timeout.min(30 s)`, so the idle timer and the
  drain timer expire at the same instant at every budget below the cap; that routing is pinned by
  the source until a test drives a draining connection that accepted no request. Every mutation
  was reverted and the baseline re-run.
- **T7 on real defaults** (`keep_alive_timeout` 60 s, `write_timeout` 10 s, h2c; a scratch h2 client
  outside the repo reading 64 KiB every 300 ms): 24 MiB (25,165,824 B) arrived whole in **95,906 ms**
  — 35.9 s past the old 60 s cut — and a HEAD at 95,925 ms on the same connection answered 200. The
  earlier 16 MiB run: 16,777,216 B in 63,948 ms, HEAD 200 at 63,967 ms.
- **The stall probe re-run twice, not inferred.** Same harness as track 49, 64 MiB body, client
  consumes 0 B for 15 s: HANDLER at t=835 ms; during the stall **live = 67,257,004 B** while
  **registered = 49,152 B = `CONN_BASE_MEMORY` + `STREAM_BASE_MEMORY`** — the stream charge is now
  held for the duration of the write (track 49 read 32,768 B with `active_handlers = 0`), but the
  body bytes are still unpriced. At **~10.2 s** (the default `write_timeout`) live fell to
  144,789 B, registered to 32,768 B, and the connection stayed open (conns = 1) until the client
  exited; the client's resumed read returns `Reset(StreamId(1), CANCEL, Remote)`. OS memory from
  `Get-Process` during the stall: WorkingSet ≈73.56 MB / Private ≈68.75 MB, falling to ≈6.49 MB /
  ≈1.54 MB after the reset (second run 73,068,544→5,963,776 and 68,702,208→1,458,176). The two
  claims separate cleanly: for a body larger than h2's send buffer the stream is reset at
  `write_timeout` once that buffer fills (a body that fits the buffer never arms the timer and
  stays bounded by the idle timer), and the *bytes* remain unpriced. T7's steady reader is
  evidence about the idle-timer cut only; it says nothing about PROB-0022, where a peer granting
  one byte per interval keeps renewing the deadline.
- **Gates on the exported commit tree** (HEAD + the final `http2.rs` and `http2_e2e.rs`; this
  slice's four governance files are not build inputs, and the five remaining modified paths are
  outside the slice and held at HEAD): `cargo test --workspace` **480 / 0 / 1** — 272 unit + 203
  integration + 5 doc, 50 result lines, exit 0 — `cargo clippy --all-targets` exit 0 with the only
  `http2.rs` warnings the pre-existing `collapsible_if` at 406/451/530 (none in changed code), and
  `cargo fmt --all --check` still red repo-wide — **572 hunks across 66 files**, exit 1 — with an
  added-line intersection probe (`git diff -U0` ranges against rustfmt's affected lines) reporting
  **zero** of this slice's added lines inside any hunk, in either `http2.rs` (10 hunks, 34
  fmt-dirty lines file-wide) or `http2_e2e.rs` (18 hunks, 60). The full fmt gate is not claimed
  green. The 11-file working tree measures 484 / 0 / 1 (275 + 204 + 5) and 569 fmt hunks; T7 and
  the stall probe above were measured on that tree, not on the exported one.
- **Independent review.** A fresh-context, read-only reviewer agent with no access to this
  session's reasoning read the diff and the pinned `h2` 0.4.20 source and returned **no blocker**:
  it confirmed the loop cannot spin, starve shutdown, or extend the drain deadline, and that
  `poll_capacity` never resolves `Ready(0)`. Its five findings were applied — T2 now waits for the
  connection's own registration and asserts a mid-write registration, its follow-up is HEAD (no
  DATA credit needed), T3 gained an upper bound, T6 observes the actual reset, and the
  `WRITE_CHUNK`/`write_timeout` comments now name what really bounds buffered bytes (h2's 400 KiB
  send buffer; the idle timer after the final `send_data`). Every test was re-run after those edits.
- **Second independent review of the final diff** (owner-requested, fresh context, read-only):
  verdict **SHOULD-FIX** with no blocker. It confirmed from the pinned `h2` 0.4.20 source that
  `send_reset` is stream-scoped and cannot GOAWAY, and that the stream charge is acquired once and
  released once by `Drop` on every exit path (normal, error, reset, panic, `JoinSet::shutdown`).
  Its three substantive findings were corrected in the tests: T2 no longer claims the stream's
  charge pays for the body (it held only the 16 KiB stream charge then — track 51 priced the body and
  the assertion at `http2_e2e.rs:855` now requires it, so this sentence describes the old tree), T6 is
  documented as the combined listener+connection shutdown path, and T1 now pins interval-vs-total
  semantics with an 800 ms `write_timeout` against a ~2.4 s transfer; T2 also asserts `CANCEL` and
  its observation window widened to 1 s. After the corrections the target passed 34/34 in the
  workspace run and in three further consecutive runs (5.02 s each), and the full workspace still
  reported 483 / 0 / 1 at that point (484 after the drain-isolation test above).
- **Independent reviews of the isolation test** (owner-requested, fresh context, read-only; four
  rounds). Round 1 ran the baseline 10/10 (cuts 323.9-331.0 ms) and confirmed that a client abort
  at 280 ms fails, so the test cannot pass via a client-side close; it also confirmed the old loop
  shape, a fresh post-loop budget (624.0 ms), `drain_at = now`, `drain_deadline = None`, and a
  disarmed loop timer all fail. Its blocking finding was that a *sliding* deadline — re-armed at
  the top of every drain iteration — passed at 329.0 ms because nothing woke the loop after the
  first iteration, so absolute and sliding deadlines cut at the same instant. The delayed read
  above closes that hole: the same mutation now cuts at 381.4 ms at a 200 ms budget and fails the
  300 ms window. Round 2 found that a hardcoded 400 ms budget passed the then-window and that the
  idle-branch routing was not discussed; round 3 found that a `max(budget, 400 ms)` floor and a
  +50/+100 ms post-loop grace passed, that the claim "no constant can pass" was overbroad, and
  that the 75% read delay left fragile margins; round 4 found that the `[250, 350)` and
  `[300, 450)` windows overlap (the doc wrongly said "disjoint"), that `max(budget, 300 ms)` still
  passed, and that the stated slack was overstated. Each finding was fixed by changing the test,
  not by weakening the claim: the lowest budget moved to 200 ms, giving three pairwise-disjoint
  windows; the 300 ms floor now falls outside the lowest window; and the doc records both wrong
  shapes that still pass plus the limits that are not claimed (a 50 ms post-loop grace, a floor
  below ~270 ms, the idle-branch routing). No fifth review was run after that final change; it is
  covered by the mutation matrix and the baseline runs above instead.
- **Status.** PROB-0015 and PROB-0021 are closed by the evidence above. PROB-0020 stays **open**
  (keeping the stream's 16 KiB charge is not pricing the body), and PROB-0022 stays **open** (a peer
  that grants one byte per interval renews the no-progress deadline indefinitely; D1 rejected a
  total cap). PROB-0012 stays open; Windows/MSVC only. No commit, no push.

2026-10-24 (track 49 — **PROB-0020 measured: Confirm**, and a second h2 defect found while testing it):
The owner approved `h2 = { version = "0.4.20", default-features = false }` as a **dev-dependency of
the bench crate** for this probe; nothing in the framework's dependency graph changed. The harness is
`C:/Users/hamza/Desktop/nwfm/aether-bench-compare/examples/h2_stall.rs`: an h2c server with scheduling
enabled and a counting allocator that reports **live heap bytes** next to the scheduler's own
`registered_memory_bytes` every 250 ms, plus an h2c client that takes the response headers and then
stops consuming for a chosen window.
- **The measurement.** `/big` returns 64 MiB (67,108,864 B) of `'x'`. Client withholds every
  WINDOW_UPDATE for 10 s. Server-side readings in that window: **live = 67,257,145 B**,
  **registered = 32,768 B = `CONN_BASE_MEMORY`** (`framework/aether/src/scheduling.rs:61`), with
  `active_handlers = 0` — so the stream's own charge had already been released and not one byte of
  the body was priced. When the client resumed, all **67,108,864** bytes arrived intact and live fell
  to **148,281 B** (h2 connection buffers) within one sample. Reproduced twice (client wall 15,202 ms
  and 15,072 ms); the two runs' readings are identical to the byte.
- **Why it is not a client artefact.** The first attempt hung and produced a `BrokenPipe` that looked
  like a server fault. It was the harness: `h2` credits the peer only on an explicit
  `RecvStream::flow_control().release_capacity(len)`, exactly the rule the repo's own h2 tests state
  (`framework/tests/tests/http2_e2e.rs:300-306`); a drain loop that consumes chunks without releasing
  stalls its own server after one window. Fixed in the harness before any number was recorded.
- **The second defect, registered as PROB-0021.** With the client withholding credit for 65 s, the
  server drops the connection between **t=60,162 ms and t=60,412 ms** (handler returned at t=207 ms):
  `http2.rs:124-145` races `conn.accept()` against `keep_alive_timeout` even while a response is in
  flight, so a slow but healthy client is cut mid-body (BrokenPipe, 0 of 67,108,864 bytes consumed).
  This is the *time* bound on PROB-0020's retention, and a data-truncation bug in its own right.
  Not fixed: it needs a design (idle timer must not apply to in-flight streams; a non-progressing
  response needs its own deadline), and it must not be mixed with the pricing decision.
- **Status.** PROB-0020 and PROB-0021 both stay **open**; no production code changed in this track;
  no commit, no push. (Superseded by the two records above: this was the state at the probe session.
  PROB-0021 was closed by track 50's paced write path and `draining` state; PROB-0020 was closed by
  track 51's response-body pricing.) The numbers here are instrument readings from the harness and the
  framework's own counter, not OS RSS — the owner's RSS command is in the report (`Get-Process`
  WorkingSet64 /
  PrivateMemorySize64 sampled during the stall), per ruling 1. Windows/MSVC only.

2026-10-24 (track 48 — **G15 is retained**: the independent review ran and reproduced every number):
Under `OWNER_VISION.md:148-150` (an independent reviewer per cycle, and the record states whether it
ran), a fresh-context reviewer agent with no access to this session's reasoning redid the cycle from
zero: it read the parser diff, ran all four gates, killed stale servers, rebuilt the bench binary
itself, and re-ran the allocation probe. Verdict: **RETAIN**.
- **Its own artifacts, not mine.** Release build `2b34eb40e9322c3a8d9680ba0f7d3f381b075dd235c05d248d5d27105c0bc6dd`
  (recompiled `aether-http`, `aether` and the bench crate in 2m31s — a different hash from track 46's
  `4e0fb32b…` is expected: a rebuild is not a bit-reproducible artifact here), process **21056**
  owning `127.0.0.1:3101`, **283** STATS lines all carrying `counting=1`, and
  `INSTRUMENT counting=armed`. It confirmed no stale server existed before its build.
- **The measurement reproduced exactly.** Two independent probe invocations, each solving a 20k/60k
  slope at 8 connections: both print **31.00 allocs/req and 2595 B/req**; slope spread 0.0002
  allocs/req and 0.001 B/req; per-connection intercepts 9.4 and 10.8 allocs/conn. The raw 20k points
  read 31.0057/31.0059 allocs and 2607.37 B, which is the intercept amortised over fewer requests —
  the estimator, not a drift.
- **The gates reproduced exactly**: **479 / 0 / 1** (50 result lines, exit 0), `check --all-targets`
  exit 0 with **0** warning lines, `clippy --all-targets` exit 0 with **97** warning lines,
  `fmt --all --check` **573** hunks with **zero inside the new parser ranges** (84–280, 826–898).
- **Wire behaviour re-verified on the running server**, not only in tests: 64/65/100 total field lines
  → 200, 101 → 431, an LF-only head → 200, two pipelined requests → 200/200, CL+TE and duplicate CL
  → 400. Its code review found no blocking correctness or security defect and confirmed the CL/TE,
  OWS, byte-budget and Partial-ordering code is moved rather than rewritten.
- **Residual risk it named, kept open rather than closed**: the retry's *accept* branch cannot be
  reached end-to-end by a legal head (the estimate over-counts by the request and terminator lines),
  so it is a safety net with no test that exercises acceptance after a retry; the byte-budget retry
  path is exercised by the existing 431 test. Its stated limitations: it did not rebuild the pre-G15
  baseline (uncommitted), and it observed no OS RSS.
- **Consequences.** G15 moves from *measured+gated* to *retained*; the register entry and the vision's
  §9 status line now say so. Still **no commit, no push, no history rewrite** — retention is a
  governance status, not a merge. Next: **PROB-0020** (the owner's approved h2 dev-dependency in the
  bench crate), then **G14 / PROB-0005** as the owner's decision.

2026-10-24 (track 47 — the owner's compression/decompression specification is stored in the vision,
**not scheduled**):
The owner supplied a 218-line Arabic specification for an optional HTTP + WebSocket
compression/decompression system and asked that it be kept in the project vision **with the deferral
condition above it**. It is stored verbatim at `OWNER_VISION.md:174-405`, under the heading
`## نظام ضغط وفك ضغط HTTP وWebSocket — مؤجَّل بقرار المالك (2026-10-24)`, which opens by quoting the owner
verbatim: «ما يتم تطبيقو الا لما نحل كل مشاكل الفريمورك ويصير اداء الفريمورك اقوى من actix باضعاف».
- **Both conditions are joint and both are unmet as of this entry**: (a) every framework problem
  closed — no open item in `problems.json` or the improvement register; (b) performance stronger than
  Actix **by multiples**, shown by an owner-run measurement (ruling 1). Until both hold, no design,
  code, compression dependency, measurement slice or register item is opened for it, and nothing in
  the roadmap is reordered around it.
- **Fidelity check before recording it.** The pasted text was compared byte-for-byte against the
  attachment (`86f0de01…txt`): 218/218 lines identical after correcting two combining-mark order
  differences the paste had normalised (`كلٌّ` and `كلًّا`). The vision diff against HEAD is purely
  additive (311 lines, one hunk at `@@ -98,0 +99,311 @@`); no owner-written text or ruling was
  modified.
- No code, no dependency, no benchmark, no commit. The suite was not re-run for this entry: every
  file it touches is markdown, and no markdown in this tree is a build input.

2026-10-24 (track 46 — G15 closed: the parser's per-request 3.2 KB header-slot array is gone):
**Aether now serves 31.00 allocations/request and 2595 B/request on `/plaintext`, down from 32.00 and
5795 that the pre-G15 tree read with the same counted-vs-counted estimator (8 connections, 20k/60k
slopes, four trials, allocation spread 0.00).** The comparison point is track 45's Actix arm (10.04
allocs/req, ≈700 B); its code did not change, but it was **not** re-run in this cycle. Allocation counts
only; no throughput, latency, CPU or RSS claim is made here — the owner runs those.
- **The defect.** `parse_head` sized the httparse slot array from the configured ceiling, not the
  request: with the default `max_headers_count = 100` (`framework/http/src/limits.rs:36`) above the 64
  stack slots, every request built a zeroed `Vec<Header>` of 100 × 32 B = 3200 B, whatever its real
  header count. Ordinary requests paid it in full.
- **The fix** (`framework/http/src/parser.rs`, all crate-private): `parse_head` now takes a line
  estimate from `header_slot_estimate` — the LF terminators already in the buffer, capped at the
  ceiling, floored at one slot, scanned only up to `max_header_bytes + 1` and stopped at the blank line
  that ends the head, so a pipelined buffer is sized by its first head and a slowloris buffer cannot
  make the estimate a second pass over everything sent. That size is tried first: the 64-slot stack
  array when it fits (no allocation for the head at all), otherwise a `Vec` of exactly that size. Only
  a head that outgrows its own estimate reaches a second attempt at `header_budget`.
- **431 is as enforceable as before, by construction rather than by argument.** The parse tail was
  factored into `parse_head_into`, which returns `Attempt::Head(..)` or `Attempt::SlotsRanOut` — the
  array running out is a separate signal from the limit, and only an attempt made at the ceiling is
  allowed to turn it into `HeaderTooLarge`. The post-parse `headers.len() > limits.max_headers_count`
  check is retained for the `max_headers_count < 1` case that the floored budget cannot express. The
  CL/TE rules, the duplicate-CL refusal, the OWS handling, the `max_header_bytes` checks and the
  Partial ordering are moved code, not rewritten code.
- Three tests added and green: `test_undersized_slot_array_reports_the_array_not_the_limit` (a short
  array reports the array, never the limit — the retry signal itself),
  `test_head_just_above_the_stack_slots_parses` (65 field lines still legal under the 100 ceiling),
  `test_header_slot_estimate_scans_one_head` (one head per estimate, LF-only heads, no-terminator
  bound, ceiling and floor). The pre-existing count-limit tests (4/5 lines, 100/101 lines, default
  budget) run through the new path unchanged, as does `test_header_byte_budget_enforced`.
- **The measured delta corrects the ladder's price.** Removing the array is worth exactly **1.00
  alloc/req and 3200 B/req** (5795 − 2595 = 3200), so the L1 rung's "+2.00 allocs" over-attributed one
  allocation to this array; the register's G15 entry carries the correction.
- **How the number is produced, and on which binary.** `loadtest.py` is only the traffic generator; the
  measurement is the server's own counting allocator (`aether-bench-compare/src/counting.rs`), whose
  `STATS allocs= bytes= counting=1` lines the probe (`C:/tmp/t44_one.py`) differences around each load
  window and solves as a slope over 20k and 60k requests. Four runs on an attested instance: tree HEAD
  `b70ce7c` with this work uncommitted (code-diff sha256 `8fc611c3…`, `parser.rs` sha256 `3f8b1b06…`),
  binary `target/release/aether_server.exe` sha256 `4e0fb32b…` rebuilt with nothing left to compile,
  process 26608 owning `127.0.0.1:3101` (path and start time read from the process, not assumed), log
  showing `INSTRUMENT counting=armed` and `counting=1`. **31.00 allocs/req and 2595 B/req in all four
  runs** (`C:/tmp/t46_g15_runs.txt`); per-connection intercept 9.2–11.3 allocs/conn — idle keep-alive
  allocations, G16's territory, not per-request cost. No stale server was running before the build
  (`tasklist` and `netstat` both empty for 3101), so the numbers cannot be an older process's.
- **Gates on the final tree**: `cargo test --workspace` **479 / 0 / 1** (50 result lines, exit 0 — the
  pre-slice 476 plus these three), `cargo check --workspace --all-targets` exit 0 with **0** warning
  lines, `cargo clippy --workspace --all-targets` exit 0 with **97** warning lines (the pre-slice
  baseline; one `#[allow(clippy::large_enum_variant)]` was needed on `Attempt`, and it was **measured,
  not only argued**: `Attempt` is 624 B, exactly what `Option<(RequestHead, usize)>` — the value
  `parse_head` returned by value before the split — already was, so the discriminant fits existing
  padding and the split adds no bytes; boxing it would add the very allocation this change removes),
  `cargo fmt --all --check` with `inside-my-additions=[]` on all four changed `.rs` files and **573**
  pre-existing hunks against the pre-G15 tree's 574 (the dedented parse tail became fmt-clean; no
  legacy line was reformatted). The size probe was a temporary test, run and then removed: the tree
  passed the gates at `parser.rs` sha256 `3f8b1b06…` and returned to that hash afterwards.
  Diff is 7 files, all intended; **no commit, no push, no history rewrite**.
- No public item changed (`parse_head`, `parse_head_into`, `Attempt` and `header_slot_estimate` are
  module-private), so the downstream `my_cool_site` re-check was not re-run — same limitation as
  track 45, stated rather than papered over. Windows/MSVC only; **PROB-0012 stays open**.
- **Process, per `OWNER_VISION.md:139-154`**: the experiment was designed here and the allocation
  measurement was run here through the existing counting harness in the sibling bench repo
  (`aether-bench-compare`, whose dependency is a path to this working tree), with probes in `C:/tmp`;
  no new exported tree was created for this slice. **No independent reviewer agent ran for this
  cycle**, so under §9 it is measured and gated but **not yet retained**. The next valuable action is
  unchanged and is the owner's: **G14 / PROB-0005**, the double header store, which is the remaining
  ~18 allocs/req and the only route below ~14 allocations for a 3-line request.

2026-10-24 (track 45 — "صلح المشاكل وحسن الاداء": the two benchmark-harness defects are fixed, and the
first root-caused allocation win is measured):
**Aether served 45.00 allocations/request where Actix served 10.04, on byte-identical responses; the
tree now serves 32.00.** Both numbers are counted-vs-counted instrument readings (never throughput
claims — the owner measures), taken at 8 connections, two request counts (20k/60k) per arm to separate
allocations-per-request from allocations-per-connection, reproducible to ±4 allocations across four
trials per arm (allocation spread 0.00).
- The instrument had to be fixed before it could be trusted, and one of the two defects had already
  produced a wrong claim in track 44: the comparison read a *counted* Aether arm against an
  *uncounted* Actix arm, which `aether-bench-compare/src/counting.rs:8-10` forbids. `actix_server.rs`
  now calls the same `counting::stats_task(1500, "Actix")` the Aether arms call — that also settled
  the open question from track 44: the lib-level `#[global_allocator]` at `src/lib.rs:6-7` does reach
  the sibling crate's binaries, proven by `counting=1` and by allocation counts that move with load.
  `arm()` now prints one `INSTRUMENT counting=armed|off` line per process so a counted run cannot be
  mistaken for a control, and reads `CP_COUNT` once (`OnceLock`) because `cost_probe` calls `arm()`
  per accepted socket and `println!` allocates.
- `loadtest.py` returned **0** while its own dump showed 200/200 responses failing the body check. It
  now exits 1 on any `failures` and names the reasons on stderr. Verified in both directions on a live
  server: correct body → exit 0, wrong body → exit 1 with `body_mismatch(len=13): 200`. `mp_loadtest.py`
  already collects child exits into `child_errors`, so the change surfaces rather than crashes drivers.
- The fix itself (this tree, uncommitted): the serving core deep-cloned the whole header store on
  every request (`framework/aether/src/serve.rs:124-132` — the clone existed only so the header reads
  could outlive the move of `parts` into the dispatch, but every value that must outlive it is already
  an owned String). Reading `&parts.header_multi` instead: 45 → 36.00 allocs/req. Then `origin` is read
  only when a CORS layer is configured, `accept-encoding` only when a compression layer is configured,
  `host` only inside the host-validation arm, `header_value_multi` no longer allocates a lowercase
  temporary for an already-lowercase name and returns `Cow` for the single-line case, and `X-Request-ID`
  is moved into the response instead of re-formatted: 36 → **32.00 allocs/req**, 6672 → 5795 B/req.
  No public item changed; the `Cow` helper and `PostFix` are crate-internal.
- `accept_encoding_reaches_compression_only_when_a_layer_is_configured`
  (`framework/tests/tests/http_e2e.rs:2557-2610`) is new because the suite never drove compression
  through the core — every `compression::tests::*` case calls `apply()` directly, so a lost
  `Accept-Encoding` read would have passed the whole suite. It asserts gzip actually arrives when a
  layer is configured, `Vary` stays, `X-Request-ID` is still emitted, and an app with no layer answers
  identity at full length. CORS and host-validation have real e2e coverage already
  (`cors_preflight_reads_the_first_request_method_line`, `host_validation_refuses_an_off_list_host_before_routing`).
- Gates on the final tree: `cargo test --workspace` **476 / 0 / 1** (50 result lines, exit 0 — the
  pre-slice 475 plus this test), `cargo check --workspace --all-targets` exit 0 with 0 warning lines,
  force-cold `cargo clippy --workspace --all-targets` exit 0 with 18 `Checking` and **97** warning
  lines (identical to the pre-slice baseline — no new lint), `cargo fmt --all --check` back to HEAD's
  **574** hunks with `inside-my-additions=[]` on all three changed `.rs`. The downstream `my_cool_site`
  re-check was **not** re-run: no public signature changed.
- **The 32 − 10 gap is now attributed, and the rest is a decision, not a task.** The existing stage
  ladder (`aether-bench-compare/src/bin/cost_probe.rs`, run through `layer_ladder.py --conns 8`) prices
  header materialisation at **+18.00 allocs/req** (spread 0.00) and is the single largest remaining
  site; it comes from `RequestContext` carrying **two** owned header stores — the public
  `headers: HashMap<String, String>` and the private `header_multi: HashMap<String, HeaderLines>` —
  which cost two key Strings and two value Strings per field line plus two table allocations. With the
  double store kept, a 3-line request cannot go below roughly 14 allocations, so closing the gap needs
  **PROB-0005** (merge the stores / remove a public field), which is an API break the owner has
  deliberately kept open. Second site, measured not guessed: the parser sizes its httparse slot array
  to the *configured ceiling*, not to the request — `http/src/parser.rs:82-96` resizes to
  `max_headers_count` (default 100, `limits.rs:36`) whenever that exceeds the 64 stack slots, so every
  request writes a 3.2 KB array (L1 = +2.00 allocs, **+3264 B/req**). The fix is a bounded line count
  with a retry to the ceiling so `TooManyHeaders → 431` stays exact; it needs the ~80-line parse tail
  factored out first, which is security-critical code and was not attempted in this track.
- Also measured and registered, not fixed: an idle connection with live keep-alive sockets allocates
  **21.6 times/second** with nothing in flight (`C:/tmp/t44_idle.py`) — small next to the per-request
  number, but it is a per-connection cost that scales with connection count, which is PROB-0020's
  territory. **No independent reviewer agent ran for this cycle** (vision §9 treats the cycle as not
  yet finished). No throughput/latency claim: every rps figure in this session's runs was contaminated
  by other processes on this box and is recorded nowhere. **No commit made.** Windows/MSVC only —
  PROB-0012 stays open.

2026-10-24 (track 44 — the owner handed over a research methodology and three rulings that make it
operable here):
**`OWNER_VISION.md` now carries an ordered 11-step improvement-research protocol, appended verbatim
as delivered (`:101-137`), followed by the owner's answers to the three clauses that collided with
standing constraints (`:139-154`).** Nothing above it was edited or softened. The rulings, in the
owner's own selection:
- **The owner measures.** §6's "run the candidate and baseline" does not move measurement to the
  agent: the agent designs the experiment and hands over the exact command and tool. No
  throughput/latency/CPU/RAM number enters a record or a document unless an owner run produced it.
  This restates the pre-existing rule rather than replacing it — the vision hardens the *design* of
  the experiment, not ownership of the result.
- **An independent reviewer per cycle.** §9's "you should not be the sole authority approving your
  own proposal" is now mandatory for every research cycle before it is retained, rejected or called
  inconclusive, and the cycle's record says whether the review actually ran. This changes practice
  from track 43 downward: track 43 and everything before it recorded "no independent agent was used"
  as a limitation, which §9 now treats as not yet a finished cycle.
- **The isolated workspace is `C:/tmp`.** §5's "experimental branch or workspace" is executed as an
  exported tree under `C:/tmp/t*/` — the shape already used by the compile probes and the doc-fence
  harness — not as a git branch. No commit, push, history rewrite or merge into `main` without the
  owner's explicit approval, and that approval still follows the report of results.
Experiment records follow §8 into `.project-governance/notes/` beside the improvement register, and
§11's "keep measured results separate from estimates" is the same line the register's E-table already
enforces (E1-E9 stay unmeasured, so they stay unauthorized). No code, test or document changed in this
track; only `OWNER_VISION.md` and this entry.

2026-10-24 (track 43 — PROB-0009's decision half, executed as the owner's option C′):
**the blanket `Result` response no longer guesses the status of a handler error.** `E: Display + Send`
is gone from `framework/aether/src/app.rs:177-202`; the bound is now a local trait,
`IntoResponseError` (`:99-127`, doc `:83-98`), which states `error_status()` and gets its `code`
derived from that status's class and its body detail redacted when the status is 5xx. Full record in
this register's **D9**, including why `E: IntoResponse` was rejected (the measured 200 trap: `&'static
str`/`String` already answer 200 through `IntoResponse`, so bounding the general trait would have made
`Err("database is unreachable")` a success) and why no additive door existed (orphan rule + overlap),
which is exactly what makes a *local* trait the only shape that works.
- Three impls ship: `&'static str` `:129-137` and `String` `:139-147` (400 + their message), and
  `AetherError` `:149-175` — 404, 405, 413, 431, 504, 500 (`DependencyCycleDetected`, `Io`) and 400
  (`MalformedRequest`, `ValidationFailed`, `Serialization`, `Custom`), with the code delegated to
  `AetherError::error_code` (`core/src/error.rs:23-43`) and that function's two wrong arms corrected
  (`Io` `E1001`→`E5002` at `:37`, `Custom` `E5002`→`E4003` at `:42`) so the cross-crate invariant holds.
  `E5002`'s `title()` at `core/src/diagnostic.rs:71` reads "Internal server error"; no test asserts
  `title()` and its only consumer is `format_terminal` (`:118`).
  Re-exported at `framework/aether/src/lib.rs:42-45` and in the prelude at `:60-63`.
- **Breaking by design, and the break is compile-time.** Measured on four handler shapes in a scratch
  crate against this tree: `&'static str` and `AetherError` errors compile (exit 0), an unwrapped
  foreign `Display`-only type fails in both handler forms — `app.get` names `IntoResponseError`,
  `app.get_async` reports only `Handler<_>` and never names the trait. Both diagnostics are in the docs.
  `my_cool_site` (`threekway`) re-checked with `app.rs`/`diagnostic.rs` touched first: 9 `Checking`
  lines, exit 0, 0 warnings.
- **Gates, all re-measured on the final tree** (`cargo test --workspace` **475 / 0 / 1**, 50 result
  lines, exit 0 — 271 unit + 199 integration + 5 doc, `http_e2e` 46 → 47; `cargo check --workspace
  --all-targets` exit 0 with 0 warning lines; force-cold `cargo clippy --workspace --all-targets` 18
  `Checking`, **97** warning lines, 35 instances / 22 distinct `clippy::` lints, 0 errors — identical
  aggregates to the pre-slice baseline `C:/tmp/clippy31-alltargets.txt`, so no new lint; `cargo fmt
  --all --check` **576 hunks / 66 files** against HEAD's **574 / 66**, the whole +2 in `lib.rs` 5 → 7
  from re-wrapping pre-existing dirty imports, `inside-my-additions=[]` on all five changed `.rs`; doc
  fences 66 → 45 compiled / 34 passed / 11 failed, all 11 in the four classes that pass has always had
  and none a fence this track wrote).
- **PROB-0009 stays open.** The redacted 5xx detail is *dropped*, not logged — `into_response()` runs
  without the request, so there is no logger handle and no `X-Request-ID`; making it a log is a
  request-context change, not an error-contract one. Six documents carry the new contract (register D9
  lists them). No performance claim: nothing was benchmarked (the owner measures). **No independent
  agent was used for this track**; every number came from a command run in this session. Windows/MSVC
  only — **PROB-0012 stays open**. No commit made, per the owner's rule that the commit decision
  follows the report.

2026-10-23 (track 31 — the problems the owner pointed at and said "solve them the best way"):
**two of the nine open registry entries were defect-repair items and are now closed, a third
(PROB-0009) was closed as far as the published contract allows, and six need a
decision, a benchmark run by the owner, or a host this machine does not have.** Triage of
`.project-governance/problems.json` as delivered: PROB-0017 and PROB-0018 fixed with tests;
PROB-0009 fixed as far as it can be without breaking the published contract; PROB-0004 (owner runs
the sweep), PROB-0005 (removes a public field), PROB-0011 (builder unification), PROB-0015 (chunked
h2 writes = the streaming API), PROB-0016 (needs an operator-supplied size) and PROB-0012 (Linux and
macOS have still never run this suite) left open with the reason. Two new entries registered on the way:
**PROB-0019**, recycled problem IDs, and **PROB-0020**, the h2 response side the PROB-0017 closure
does not reach — found by the path verification written below, so the registry now holds nine.

- **PROB-0017, the response side of the read-buffer price.** `framework/aether/src/app.rs` now
  carries `write_buffer_charge`, `conn_buffered_charge(read, write)` and
  `release_oversized_write_buf`, with `WRITE_BUF_INITIAL_CAPACITY = 4096` exported from
  `framework/aether/src/scheduling.rs` beside the read constant so the two cannot drift. The block a
  serialized answer reserves is charged *before* `write_all` (a client that stops reading holds those
  bytes for the whole write) and released *after* it, and all three existing reprice sites switched
  from the read-only charge to the combined one. Two deliberate choices, both against the
  cheaper-looking alternative: the charge is **best-effort** — a `Reprice::OverBudget` on this path
  does not withhold a response the handler already produced, because refusing to deliver an answer to
  save RAM is exactly the "weaken a required feature" move the standing protocol forbids — and the
  release replaces the buffer only above `2 × 4096`, so a connection serving many small answers does
  not reallocate per response. Three unit tests pin the arithmetic
  (`fresh_write_buffer_adds_nothing_to_the_base_charge`,
  `a_large_answer_is_priced_by_the_block_it_reserved` — 1,572,864 B written lands in a block of
  exactly 2 MiB, so the charge is 2 MiB − 4 KiB, larger than the payload it held — and
  `oversized_write_buffer_is_handed_back_and_small_ones_are_kept`).
- **The wire test for it is a `memory_shed` test, and that is an honest limit, not a shortcut.**
  `a_large_answer_is_priced_against_the_memory_budget_without_being_withheld` (`scheduling.rs:737`)
  asserts under a 4 MiB budget that a 16 MiB answer still arrives complete **and** that the scheduler
  shed at least once, with `/small` in the same test as the negative control (zero sheds). The first
  two attempts sampled `registered_memory_bytes` during the write and both failed with
  "peak 32768 B, base 32768 B": Windows loopback takes a 16 MiB `write_all` with nobody reading, so
  the charge exists for sub-millisecond and a 2 ms poll cannot see it. Sampling there would be a test
  that passes for the wrong reason on this host; the shed counter is the signal that actually
  reproduces. Both ends of the behaviour are still observed on the wire — the shed moves during the
  write, and `registered_memory_bytes` is asserted back at `CONN_BASE_MEMORY` after it, which is the
  retention half that matters for an idle keep-alive socket. Only the *peak value* is not wire-visible
  here; its arithmetic lives in the unit tests.
- **PROB-0018, the h2 body price.** `StreamCharge::buffer_grow(delta)` in
  `framework/aether/src/http2.rs` is gone, replaced by `buffer_set(buf.capacity())`, an absolute
  re-price that can report a shrink as well as a grow, called from `read_body` after each DATA frame.
  The falsification is measured, not asserted: with the old length price on the same 1,441,792 B
  upload the registration peaked at **1,449,982 B** and the new assertion failed; with the block price
  it crosses **2,000,000 B** and
  `an_h2_body_is_priced_by_the_block_the_allocator_handed_out` (`http2_e2e.rs:1623`) passes — and the
  stream is served, not shed (`200`, body length exact). One harness lesson from getting there: the
  test originally left the stream half-open, so `server.stop()` waited out the ~60 s drain; ending the
  last DATA frame with END_STREAM and reading the response took it from 61.7 s to 0.09 s.
- **Path verification asked for before the commit (2026-10-23).** The review accepted the split only
  after two questions were answered on the code, not by the green test: *can the read and write
  charges price the same memory twice*, and *does `E4003` break a consumer*. Findings, all in
  register **D5c**: `reprice_conn_memory` (`app.rs:2371-2396`) is target-based (`BASE + buffered +
  extra`, delta against the local `charged`), so nothing accumulates, and the three charged
  allocations are distinct objects — `conn_buf`, `write_buf`, and the chunked decoder's own `Vec`
  (`parser.rs:452`), plus `ws_memory` from `serve.rs:323`; four other sites reuse `write_buf` outside
  the success emitter (`app.rs:1986/2036/2090/2120`) and all write a `Response::status`, which is
  head-only by construction (`http/src/response.rs:126-133`), so neither a charge nor a release is
  missing there — now stated as an invariant comment at `app.rs:1982-1984` instead of left to the
  small body; the write-failure `return` (`app.rs:1805`) cannot strand a charge because
  `AdmissionGuard::drop` (`scheduling.rs:350-357`) releases the registration and `conn_charged` is
  task-local, and the release at `:1716` runs before `conn_guard.take()` at `:1730`, the order a
  downward reprice needs. `E4003`: no exhaustive `match` on `ErrorCode` exists in the workspace and
  the enum carries no `Serialize`/`#[repr]`, so the wire form is `code_str()` alone; the honest limit
  is that `ErrorCode` is re-exported (`aether/src/lib.rs:33`, prelude `:67`) without
  `#[non_exhaustive]`, so this is verified against *the consumers on this host* — `my_cool_site` /
  `threekway` `cargo check` → exit 0 in 11.30 s on the modified tree, and its 43 `E5002` strings are
  its own hand-written bodies, never read from the framework.
  **What this verification changed:** it found the real scope limit of PROB-0017's closure —
  `http2.rs:463` answers with `send_data(out.body, true)`, moving the response into the `h2` crate's
  buffers, so no framework-side block exists to price or release there and a stalled h2 reader is
  unpriced. Registered as **PROB-0020** rather than folded into a closed claim; `runtime/src/worker.rs:126`
  has the same shape inside the crate register **D6** already proved dead, so it is not a live path.
  The 148 lines this slice added to `app.rs` (10 removed) moved every coordinate quoted above its
  insertion points — in this file and in `docs/00_{ar,en}`, `docs/02`, `docs/05`, `docs/07` and
  `ACTIX_FAIR_COMPARISON.md`; all are re-derived from the code and listed in register **D5c**. The
  5-line `E4003` comment was *deleted* rather than paid for: as the only insertion above line 1679 it
  would have falsified **55 coordinates across 11 files**, and its rationale already lives on the
  enum variant and in the test that pins the behaviour.
  **Self-reviewed again — no independent agent**, same as the rest of this track.
- **PROB-0009, only the half that breaks nothing.** A handler `Err` still answers **400**, but the
  JSON body no longer claims `E5002` "unhandled internal server error" while doing it: the blanket
  emits the new `ErrorCode::E4003HandlerReturnedError` (`framework/core/src/diagnostic.rs`), pinned by
  `a_handler_error_answers_400_and_says_so_in_the_body` (`http_e2e.rs:2462`). The status class was
  *not* flipped. Re-measured 2026-10-24 (register **D5a** carries the derivation): the review surface
  is **14 test functions / 15 assert sites** — extractors 4, `http_e2e` 7, `http2_e2e` 3, and *zero* in
  `scheduling.rs` / `tls_e2e.rs`, whose 400s are `sleep(400 ms)`; the earlier "19 (… scheduling 1,
  tls_e2e 1)" counted lines holding the digits, not tests asserting the status. **One** of those tests
  pins the blanket; the other 13 come from the five other 400 producers (extractor `reject()`,
  request-head parse, upgrade refusal, host validation, unnamed method token over h2) and stay 4xx
  either way. The published contract is **six lines in five docs**, `docs/09_api_reference.md:116`
  included. And `E: IntoResponse` — the fix this file proposed — is not safe as written: `String` and
  `&'static str` already implement it as a **200** text response, and the pinning test's own error type
  is `&'static str`, so that handler would start answering 200. That decision stays the owner's;
  `PROB-0009` remains open with this written into it.
- **Gates, re-measured on this tree.** `cargo test --workspace` → **474 passed / 0 failed / 1
  ignored**, exit 0, 50 `test result:` lines, reproduced on a second consecutive run with identical
  per-target tallies (unit 271: `aether` 33 → 36; integration 198: `http_e2e` 45 → 46, `http2_e2e`
  29 → 30, `scheduling` 15 → 16; doc 5). `cargo check --workspace --all-targets` → exit 0, **0**
  warning lines. `cargo clippy --workspace` → exit 0, **0 errors**, 46 `^warning` lines / 16 distinct
  lints / 39 warnings in 7 summaries; `--all-targets` → 97 lines / 114 in 22 summaries / 0 errors /
  0 `correctness`. Nothing new was added to either lint budget by this slice. **Final reading, taken
  after the `app.rs:98` comment was deleted** (register **D5c** — the tree the three commits ship):
  `cargo test --workspace` → **474 / 0 / 1**, exit 0, 50 result lines, same 271 / 198 / 5 split;
  `cargo check --workspace --all-targets` → exit 0, 0 warning lines; force-cold `cargo clippy
  --workspace` → 46 `^warning` / 16 kinds / 39 in 7 summaries / 0 errors, `--all-targets` → 97 / 114
  in 22 summaries / 0 errors / 0 `correctness`; `cargo fmt --all --check` → 574 hunks / 66 files with
  `inside-my-additions=[]` for all seven `.rs`; the six new tests re-run individually, `1 passed`
  each. Every figure reproduced, so the gate is one reading and not two. The *first* attempt at that
  reading was not a reading: `cargo test --workspace 2>&1 | tail -60` retained only the last ten
  `test result:` lines — totals unrecoverable — and `$?` reported `tail`, not cargo. Re-run with the
  output to a file and the exit code taken from cargo. The individual re-runs nearly repeated it:
  `--exact` needs the full module path (`app::tests::…`), and with the wrong prefix it printed
  `0 passed; 36 filtered out` under `test result: ok.` — the known "a filter matching no test still
  says ok" trap, caught here only because the filtered-out count disagreed with the suite.
- **Register B18, found while re-measuring those numbers:** `cargo clippy` is cache-sensitive — the
  same command on a warm cache prints 9 `^warning` lines and 2 lints where a forced recompile prints
  46 and 16, so "no new lints" measured on a warm tree is a silent false green. Now written into the
  PRR gate table and into the register.
- **`cargo fmt`:** not run, per D4's non-gate ruling. The new code was still *proven* fmt-clean
  rather than assumed: intersecting `git diff -U0` added-line ranges with the lines rustfmt would move
  found exactly one dirty region in 411 added lines (a three-line `use` rustfmt collapses), now
  collapsed; `cargo fmt --all --check` still reports **574 hunks across 66 files**, unchanged by this
  slice. Re-run on the final tree — 409 lines added / 20 removed across the seven `.rs`, after the
  `app.rs:98` comment was deleted — the same intersection is empty for all seven files (`inside-my-additions=[]`)
  and `--check` still reports 574 hunks across 66 files.
- Documentation corrected wherever the old state was written: `BASELINE_MEASUREMENTS.md` per-target
  table, `FEATURE_COMPLETENESS_MATRIX.md` (evidence block, the typed-error row, the bounded-response
  row, the two scheduling counts, the extensions limit, item 17), `PRODUCTION_READINESS_REPORT.md`
  (§3/§5 readings, the gate table plus the cache caveat, §12 item 5, §11), `SCHEDULING_COMPARISON.md`
  and `ACTIX_FAIR_COMPARISON.md` suite figures, `ERROR_HANDLING_GUIDE_AR.md` (new `E4003` row, the
  `E5002` row no longer claims an HTTP 500 that nothing emits), `FULL_FRAMEWORK_DOCUMENTATION_AR.md`
  (`E4003`, the corrected `app.rs:83-108` span — re-shortened to 108 when the comment above the
  emission was deleted — and a duplicated `فلا يمكن` removed).
- No performance claim: nothing was benchmarked (the owner measures — no handover command is needed
  because no speed question was asked). Windows/MSVC only — **PROB-0012 stays open**, and
  cross-platform behaviour of `release_oversized_write_buf` and the h2 re-price is unverified on
  Linux/macOS like everything else here. **No independent agent was used for this track**; every
  number above came from a command run in this session. No commit made, per the owner's rule that the
  commit decision follows the report.

2026-10-23 (track 30 — register B17 closed: every documented verification command re-run):
**the owner's finding that a handover command of mine could print `test result: ok.` while running
zero tests made every documented command evidence-suspect, so the claim-bearing read-only commands in
tracked markdown were extracted with their prose and run or priced.** Full account in
`.project-governance/notes/2026-10-21-improvement-register.md` **B17**; the durable findings that
changed documents:
- `grep -rn "cfg(unix)" framework` (PROB-0012's note) is *correct* but walks `framework/target/` —
  44,378 files / 15.3 GiB — and produced nothing in 120 s. The executable forms
  (`--include=*.rs`, or `git grep`) are now what the note tells a reviewer to run.
- BRE-vs-`-E` alternation was re-checked in both forms, because a `|` in a basic regex is literal and
  "returns nothing" then proves nothing.
- Count claims re-measured rather than copied: `cargo fmt --all --check` **574 hunks / 66 files**,
  the `serve.rs` clippy invariant sites **87, 192, 445, 461** with the upgrade branch at **239-346**,
  and the lib-scope clippy breakdown (7 summaries → 39, 46 `^warning` lines, 16 lints).
  Track 31's gate re-measurement then reproduced all of those and found the cache caveat (**B18**).
- Also corrected in this track: the `FEATURE_COMPLETENESS_MATRIX` / `PRODUCTION_READINESS` /
  `SCHEDULING_COMPARISON` / `BASELINE_MEASUREMENTS` counts that had drifted, the improvement
  register's B1 pin and B8 re-run, new B16 and B17 entries and §F item 4, and the `HEAD`-relative
  falsifiability sentence in this file's track-9 entry (the **B16** block quote above it).
- Markdown-only track: no `.rs` changed, so 468 stood. Windows/MSVC only — **PROB-0012 stays open**;
  no independent agent; no benchmark (the owner measures); no commit.

2026-10-23 (track 29 — register B15 closed: citation integrity across tracked markdown, plus the two
WebSocket idle-deadline tests that the re-checked F7 row demanded):
**B14 verified what the fences compile to; nobody had checked what the line numbers still point at.**

- Method, two checks. `C:/tmp/t27/citecheck2.py` extracts every `file.rs:line[-line]` citation from
  tracked markdown, resolves it over `git ls-files *.rs` (suffix first, basename second) and flags a
  citation only when **every** candidate fails (dead file, out of range, or no identifier in the
  span): **181 citations → 11 suspects** at the start of the slice, **259 → 26 suspect lines = 9
  distinct targets** on the tree it leaves behind, every one either out-of-tree (4 crate-qualified
  actix registry coordinates + the `../aether-probe` pointer) or a coordinate kept on purpose as the
  2026-10-18/pre-fix reading and labelled as such in the sentence that quotes it (4). Zero unresolved.
  The count is not a stable property of the tree — it passed 181 → 239 → 248 → 259 because the prose
  that *classifies* a suspect re-quotes it; 19 of the final 26 lines are in the two governance files
  and 7 in the five docs files.
  The first citecheck version flagged four because it picked whichever `lib.rs`/`config.rs`/`engine.rs`
  the directory walk reached first; that version was replaced, not tuned.
- `C:/tmp/t27/symcheck.py` was added during this pass, because a citation makes two claims and the
  checker only tests one of them: `dead-citation` checks cannot see `app.rs:1041` naming
  `with_websocket_limits`, since 1041 is a live line that belongs to a different function — the setter
  is at `:1056`. That row was in **this register's** §C table, found by hand while checking the F7
  wiring, and is now a **B9** symbol anchor. The tool demands that a `file.rs:LINE` written within two
  characters of an item name have that name within ±8 lines: **0 repo findings** on this tree, proven
  non-vacuous on a fixture holding both the dead and the live coordinate; its wider 34-char tier
  raises 17 more pairings, all read, all prose noise or preserved 2026-10-18 Evidence cells.
- Six suspects were artifacts and are recorded as such so the next pass does not re-litigate them:
  four actix **dependency-source** citations in `BASELINE-2026-10-19.md` (verified against the
  registry copies `actix-http-3.19.0` and `actix-web-4.17.0` — all four still say what the baseline
  claims, so its "Nagle is symmetric" ruling stands; now written crate-qualified), one deliberate
  pointer into the sibling `../aether-probe`, and the basename-collision lines.
- Five were real drift in the docs, fixed (a sixth coordinate of the same class, this register's own
  §C row, is in the symcheck bullet above): the Track-12 `app.rs:1507`/`:1295` wiring note
  (re-anchored to
  symbols + a re-derive grep, per **B9**, instead of re-numbered), `ADAPTIVE_TREE_SCHEDULING_DESIGN.md`
  §1's "Current serve path" (relabelled as the 2026-10-18 pre-scheduler snapshot), the
  `promote_active(id)` coordinate in `BASELINE-2026-10-20-SCHED-ROTATION.md` (that helper was renamed
  by the very fix the baseline documents), `ACTIX_FAIR_COMPARISON.md` F7's `connection.rs:305-307`,
  and the five remaining places that still drew the live parsing loop as `RequestView::parse` when the
  serving loop calls `parse_request_incremental` (`parser.rs:262`, `app.rs:1631`).
- The finding under the citations: **four of the seven rows** in `ACTIX_FAIR_COMPARISON.md` §6
  ("findings and status") described defects current code fixes — F4 (`QueueToken`
  `framework/aether/src/scheduling.rs:359-366` + the already-checked-in
  `aborting_a_queued_admit_leaves_no_stranded_job`),
  F5 (`static_file` now `tokio::fs` at `app.rs:1083-1088`), F6 (three of four sub-claims,
  `serve.rs:134-145`, `app.rs:1754-1757`, `2411-2423`, `2068-2072`) and F7
  (`WebSocketLimits::read_timeout`, `connection.rs:30-42`/`146-149`, wired at `app.rs:1896-1900`).
  Finding/Evidence columns stay verbatim as the 2026-10-18 record; Classification/Status moved, and
  §6 now says so above the table. F6's fourth sub-claim is registered as **D7**, not fixed, and
  while checking what the docs promise about tooling a second gap surfaced: **D8** — no manifest in
  the workspace declares `rust-version`, so `framework/README.md:185`'s "verified toolchain is
  rustc 1.98.1" is a record of what was used, not a floor anyone is held to.
- Code slice: two tests in `framework/websocket/src/connection.rs` `#[cfg(test)] mod tests` (+37
  lines, nothing outside that module). `a_ping_resets_the_idle_deadline` is the load-bearing one — it
  fails any future change that turns the per-read deadline into a session-wide one. Non-vacuity by
  construction rather than a mutation run: `WsError::IdleTimeout` has exactly one producer
  (`connection.rs:149`).
- Verification as run: `cargo test -p aether-websocket` ×3 → 25 passed each with the two new tests
  appearing in both orders; `cargo test --workspace` → **468 passed / 0 failed / 1 ignored** =
  268 unit + 195 integration (15 targets) + 5 doc, and the same parser reproduces
  **466 = 266 + 195 + 5** on `C:/tmp/t25/test5.log`, so the split is not a new method;
  `cargo check --workspace --all-targets` → exit 0, 0 warning lines; `cargo clippy --workspace` →
  exit 0, 0 errors, 46 `^warning` lines over 16 lints, none in the `correctness` group;
  `--all-targets` → exit 0, 0 errors, 97 (114 by per-target summaries), `aether-websocket` none.
  `cargo fmt` not run (D4: not a gate). Fence sweep over the tree B15 leaves behind,
  `C:/tmp/t27/run14.log`: **95** blocks, **14/14, 48/62, 17/19**, and the failure *name list* is
  set-identical to `run11.out`/`run12.out`/`run13.log` — the same 16, none added here.
- Counts 466 → 468 swept through root `README.md`, `framework/README.md`, `BASELINE_MEASUREMENTS.md`,
  `FEATURE_COMPLETENESS_MATRIX.md`, `PRODUCTION_READINESS_REPORT.md` (×4, incl. the §5 clippy rows
  whose "39 lint mentions" was an older count on an older tree) and the status line above; the older
  455/458/459/462/466 figures stay, each labelled as its own reading.
- Limits: no runtime verification of documentation; the new tests use a `DuplexStream`, not a socket
  or the app-level upgrade path, so "every session gets the deadline" rests on reading
  `app.rs:1896-1900`; F4/F6 cite tests that already existed rather than new ones. Windows/MSVC only —
  **PROB-0012 stays open**. No independent agent was used for this slice.
- **A command in my own report was wrong, and the owner caught it.** The handover line
  `cargo test -p aether-websocket -- --nocapture <name1> <name2>` is not two filters, and the
  narrower `--exact <bare name>` is worse: it matches nothing, because `--exact` compares against
  the **full path**, and libtest still prints `test result: ok.` when zero tests ran. Measured:
  `--exact silent_peer_is_failed_by_the_idle_deadline` → `0 passed; 24 filtered out`, exit 0. A
  silent false green, in the one place this slice was supposed to be about not trusting an unproved
  pass. Re-run properly, each alone then the crate: `--exact
  connection::tests::silent_peer_is_failed_by_the_idle_deadline` → 1 passed; `--exact
  connection::tests::a_ping_resets_the_idle_deadline` → 1 passed; whole crate → 25 passed, and the
  two timing-sensitive tests were repeated five times with the execution order alternating, green
  every time. The rule for this log: a filter is stated with the full `module::tests::name` path, or
  with `--list` output, never with a bare function name.
- `cargo fmt --all --check` was run anyway, as a measurement rather than a gate: **574 hunks across
  66 files** (D4 had 567/66 from 2026-10-22 — the file set is unchanged, so the +7 is code added by
  tracks 26-28, not by this slice, which touches one `.rs` file). Splitting that: rustfmt over the
  HEAD blob of `framework/websocket/src/connection.rs` and over the working file both report **8
  hunks at the same lines** (8, 67, 169, 203, 263, 286, 720, 727), all outside the 604-641 region
  added here. The new tests are therefore fmt-clean and the crate's pre-existing deviations are
  untouched — D4's "not a gate" now has a per-file attribution it did not have before.
- Committed in three local commits after the owner approved the split and the re-verification above:
  `16d3bc3` (the two websocket tests, +38, one file), `8d732d4` (14 docs files, +68/−39, zero `.rs`
  files), and this governance record as the third. Each stage was staged by name and `git diff
  --cached --name-only` checked before committing — commit 1 = 1 file, commit 2 = 14 files with 0
  `.rs`, leaving exactly the 2 governance files for commit 3. All three trees are code-identical, so
  the single measured 468-test run describes each of them; that is a deduction from a verified
  premise (`include_str!`/`include!` over any `.md`: 0 hits; any `.md` string literal in
  `framework/*/src`, tests and examples: 0 hits), not three runs. **Nothing has been pushed.**

2026-10-23 (track 28 — register B14 closed: the twelve docs B13 left unchecked are now
compile-verified or line-cited, docs-only):
**B12/B13 stopped at the files they had already touched; the architecture pages kept teaching APIs
the crate does not expose.**

- Scope: `docs/00_framework_specification_{ar,en}.md`, `docs/02`, `docs/03`, `docs/04`, `docs/05`,
  `docs/07`, `docs/10_competitive_engineering_report_{ar,en}.md`,
  `framework/docs/DEPLOYMENT.md`, `framework/docs/AETHER_CORE_CAPABILITIES.md`, and the two stale
  `http2_e2e` counts in `framework/docs/GETTING_STARTED_AR.md` /
  `framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md`. 13 markdown files, +255/−82; no code, test,
  config or `Cargo.toml` file touched.
- Register: **B14 closed** with the full method and the residual classification; new **D6** row for
  the unused `aether-runtime` dependency in `framework/aether/Cargo.toml:12` (see below).
- Harness (three passes over every tracked fence, `C:/tmp/t27/run11.out`): the final tree has
  **95 rust blocks** — 14 whole programs, 62 statement fragments, 19 declaration listings — and
  **14/14, 48/62, 17/19** compile. The 16 that do not are named and split into two classes in B14:
  10 crate-internal excerpts (private names, source-owned locals, an `impl` lifted out of its type,
  a `break` whose loop was elided) and 6 that reference the *reader's* own types or a missing asset.
- Second method, priced by reading rather than compiling, because `cargo check` cannot see a quoted
  excerpt: all 10 crate-internal fences were re-read against their cited lines. That pass corrected
  two of my own citations (`app.rs:1934-1947` → `1936-1951`, `response.rs:227-233` → `227-235`),
  split one merged `if` in `docs/05` §3 back into the two guards `response.rs:246-254` actually has,
  and confirmed every ServerLimits default against `limits.rs:28-50`.
- Phantom APIs replaced with the real ones (each verified against the file now cited in the doc):
  `pub const METHOD_GET: u64` and `allowed_methods_mask & (1 << method_id)` → `Method::bit()` /
  `bitmask()` (`method.rs:25-43`) and `allowed_methods & method.bitmask()` (`node.rs:41-43`), in
  three files; four `AetherError` variants that do not exist (`MultipleContentLength`,
  `InvalidHeader`, `InvalidContentLength`, `ConflictingLengthHeaders`) → the real
  `MalformedRequest`/`HeaderTooLarge`/`BodyLimitExceeded` branches of `parser.rs:129-184`;
  `subtle::ConstantTimeEq` → `ring::hmac::verify` (`jwt.rs:82`) + `constant_time_eq`
  (`constant_time.rs:6`) — `subtle` is not a dependency of any crate here; the 10-field
  `RequestContext` listing → all 12 fields (`app.rs:285-322`); the hand-written response serializer →
  `write_http1_to` → `write_http1_response` (`response.rs:206-236`) with its CL/TE skip, Date
  injection, 1xx/204/304 body suppression and CR/LF/NUL sanitisation; `read_timeout` on "every read"
  → the real `read_limit` choice between `read_timeout` and `keep_alive_timeout` (`app.rs:2068-2072`);
  `acquire().await.unwrap()` → the shutdown-arm `select!` (`app.rs:1471-1478`);
  `RequestView::parse` → `parse_request_incremental` as the parser the serving loop calls
  (`app.rs:1631`); and `app.get("/", |_| async { "hello" })` in DEPLOYMENT §4 → `get_async`, because
  `get` requires `R: IntoResponse` on the returned value, not the future (`app.rs:797-817`).
- Counts re-derived from the test file, not inherited: `http2_e2e` is **29** `#[tokio::test]` —
  17 ALPN-`h2` over TLS, 9 cleartext prior-knowledge h2 through the `h2` client, 3 raw-frame-only
  (one stops at the preface); 4 hand-arrange bytes. A first classifier said 0 because it matched only
  the bare `#[tokio::test]` attribute while the file uses `#[tokio::test(flavor = …)]`.
- Gate: **no test run was needed and none was claimed as new** — `grep -rn "include_str!\|include!("`
  across the repo's `*.rs`/`*.toml` (excluding `target/`) returns nothing, so no markdown file in this
  tree is a build or doc-test input; the **466 passed / 0 failed / 1 ignored** from track 27's run
  still describes this tree. Fragments were type-checked, never executed, and the harness carries
  `#[allow(dead_code, unused_variables, unused_mut)]`, so nothing here is a runtime verification.
- Found and registered instead of silently fixed: `framework/aether/Cargo.toml:12` declares
  `aether-runtime`, which no `.rs` file in the workspace references (`grep -rn "aether_runtime"
  --include=*.rs framework` → zero hits) and which only `aether` depends on per `cargo tree -p aether
  -i aether-runtime` — while `runtime/src/lib.rs:3-9` states the crate is a legacy prototype whose
  `handle_connection` drops split requests, never enforces `Host` and applies no I/O timeouts.
  Removing a manifest line changes the publish payload, so it is **D6**, an owner decision.
- **Linux and macOS remain untested; PROB-0012 stays open.** No independent agent was used.

2026-10-23 (track 27 — PROB-0008 closed: the connection charge now follows the block the allocator
handed out, code + tests + the counts that track's run re-derived):
**memory was priced by what a buffer has written; the process pays for what it reserved.**

- Register: **PROB-0008 closed** (`done PROB-0008`, logged as `done_and_deleted` at 2026-10-23T13:43:48Z).
  Landed as four commits, no push: `7b96068` (the 4 code/test files), `6685090` (track 26's six
  markdown files), `ff1b9d5` (the five documents whose counts this track re-derived) and this
  governance commit. Only the first changes a test outcome: `grep` for `include_str!`/`include!`
  across `framework/*/src` and the test trees returns nothing, so markdown is not a build or
  doc-test input here, which is why the per-commit verification is the one 466-test run plus that
  grep rather than four recompiles.
  The concern that had kept it as an owner decision — that pricing capacity also re-prices idle
  sockets and moves the admitted-connection count — did not materialise, because the initial 8 KiB
  block is already inside `CONN_BASE_MEMORY`: the buffered charge of a fresh buffer is 0 by
  construction, pinned by `aether/src/app.rs::fresh_buffer_adds_nothing_to_the_base_charge`.
- Code (4 files): `aether/src/scheduling.rs` adds `pub const CONN_BUF_INITIAL_CAPACITY: usize = 8192`
  beside `CONN_BASE_MEMORY`; `aether/src/app.rs` adds `conn_buffer_charge(&BytesMut) -> u64`
  (`capacity() − CONN_BUF_INITIAL_CAPACITY`, saturating), switches the three reprice sites to it,
  widens `reprice_conn_memory`'s private `buffered` parameter from `usize` to `u64`, and releases an
  emptied oversized read buffer after `split_to` (`app.rs:1946-1950`) so an idle keep-alive socket
  does not keep the block one request needed; `http/src/parser.rs` adds
  `ChunkedBodyDecoder::body_capacity_bytes()` so the decoder's `Vec` block is priced the same way.
  Both public additions are additive; nothing existing changed signature or behaviour.
- Tests: 3 unit tests in `aether/src/app.rs` (`cargo test -p aether --lib buffer` → 3 passed) and 1
  wire test in `framework/tests/tests/scheduling.rs`,
  `buffered_body_is_priced_by_the_block_the_allocator_handed_out` (8 MiB budget, `Content-Length:
  2000000`, 176 × 8 KiB writes, a sampler task running *beside* the upload because `write_all`
  returns when the kernel takes the bytes and a post-upload poll can arrive after the 5 s
  `read_timeout` has closed the socket; then `drop(client)` and the registration must return to 0).
  `cargo test -p aether-tests --test scheduling` → **15 passed / 0 failed**.
- Falsification, both halves measured rather than argued:
  1. the charge — with the read site temporarily back on `conn_buf.len() as u64`, the new wire test
     failed and a `--nocapture` poll trace showed `registered_memory_bytes` steady at **1,474,617 B**
     for **1,441,792 B** uploaded (`active_connections: 1`, never reaching the 2,000,000 B the
     2 MiB block costs); the capacity charge is `32 KiB + 2 MiB − 8 KiB = 2,121,728 B` by arithmetic
     and the test asserts the registration crosses 2,000,000 B, which is what it measures.
     `grep -n "conn_buf.len() as u64" aether/src/app.rs` now returns nothing and all three sites use
     `conn_buffer_charge`.
  2. the release policy — disabling only that `if` (with `if false`) made the *existing*
     `idle_sockets_are_charged_what_they_hold_not_what_they_could_reach` fail at
     `registered_memory_bytes: 1073031` with "the body bytes were not given back after the request
     completed"; restored, the scheduling target is 15/15 again. So pricing honestly without the
     release would tax idle sockets, and the two halves depend on each other.
- Gate (final text, `C:/tmp/t25/test5.log` and `C:/tmp/t25/clippy5.log`): `cargo test --workspace`
  exit 0, 50 `test result:` lines, **466 passed / 0 failed / 1 ignored** (266 unit + 195 integration
  + 5 doc); `C:/tmp/t25/test4.log` measured the same 466/0/1 one edit earlier, before the last
  doc-comment rewrite of the new test. `cargo check --workspace --all-targets` exit 0 and
  `cargo clippy --workspace --all-targets` exit 0 with **0 `error` lines**; no clippy warning lands
  on any line this track added (checked by filtering the log for the new line numbers).
- Counts re-derived from that log, not copied (the B11 class): `README.md`, `framework/README.md`,
  `framework/docs/BASELINE_MEASUREMENTS.md` (per-target table, `aether` lib 30 → 33 with `app.rs`
  7 → 10 recounted from the file), `framework/docs/FEATURE_COMPLETENESS_MATRIX.md` (evidence block,
  the two testing-utilities rows, `scheduling` 14 → 15 in both), `framework/docs/PRODUCTION_READINESS_REPORT.md`
  (the re-measurement chain, §5's command table, a new paragraph explaining why §5's 2026-10-22
  split and the header disagree, and the §-level suite line). `http2_e2e` settled at **29** from the
  run, which is what the register's B11 note left open.
- Two gaps this track found and registered instead of silently fixing, because each changes shedding
  behaviour and neither has a test today: **PROB-0017** (HTTP/1 `write_buf` serialises the whole
  response into a per-connection buffer that is `clear()`ed — capacity never shrinks — and
  contributes nothing to the registration: the response-side mirror of what was just fixed on the
  read side) and **PROB-0018** (`http2.rs::read_body` re-prices with a per-frame
  `chunk.len()` delta, so the h2 path still prices length while HTTP/1 prices the block).
- Not claimed: any performance or peak-RSS improvement. The release policy should reduce retained
  memory for a keep-alive socket after a large upload, but that is not measured here — benchmarking
  stays the owner's run.
- Known window, recorded at the release site: a `Bytes` still frozen out of the old block keeps that
  allocation alive while the request's copies live, and the charge has already returned to base by
  then. Bounded by the request, not by the connection.
- **Linux and macOS remain untested; PROB-0012 stays open.** No independent agent was used.

2026-10-22 (track 26 — B12's scope limit became the next phantom-API batch, docs-only):
**every remaining example in the four example-heavy docs is now compile-verified, and the commit for
track 25 landed as `c2b5a1d` first.**

- Committed track 25 as `c2b5a1d` — "docs: replace the APIs our examples taught with the ones the
  crate exposes" (6 markdown files, +202/−72). No push, no history rewrite; `backup/pre-merge-main`
  untouched.
- Register: **B13 recorded and closed** — the sweep extended to `framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md`,
  `framework/docs/GETTING_STARTED_AR.md`, `framework/README.md`, `docs/09_api_reference.md`.
- New verification method for fragments (B12 could only compile whole `fn main` programs): a statement
  block is embedded in `async fn frag(app: &mut AetherApp, req: RequestContext, body: Bytes) ->
  Result<(), Box<dyn Error + Send + Sync>>` in a scratch crate with path dependencies on
  `aether`, `aether-{core,security,validation,serialization,dependency,websocket,openapi,observability}`
  plus `bytes`/`serde`/`serde_json`/`tokio`, `CARGO_TARGET_DIR` on the gitignored
  `framework/target`. Path dependencies to crates that use `version.workspace = true` do resolve from
  outside the workspace, so no repo file was needed to check them.
- Results: 9/9 `fn main` programs compile verbatim from the doc text (7 of them failed on the first
  probe); 52/60 other blocks compile; the rest are 4 source-copied declaration listings and 4 blocks
  that fail only for stated external reasons (a user type defined elsewhere in the doc, the reader's
  own asset file for `include_bytes!`, and `tera`, which no `Cargo.toml` in this workspace declares).
- Phantom/incorrect surface corrected: `ServerLimits { max_headers: 64 }` as an app-level config
  (not re-exported from `aether`, wrong field, seven missing, never applied) → `AetherConfig` +
  `with_config`; `aether_dependency::DependencyContainer::register/add_dependency/
  resolve_build_order` and `DependencyScope::Scoped` → `DependencyGraph::add_node` +
  `compile_execution_plan` and `RequestScoped`; `Json<T>` listed as a returnable `IntoResponse`;
  `:id` as a route parameter; `arena.reset()` on a non-`mut` binding (E0596); partial moves of
  `req.request_id`/`req.body` inside a handler that keeps using `req` (E0382);
  `Result<&str, (u16, &str)>` as a handler return (E0277 — `Err` needs `Display`, and always maps
  to 400 + `E5002`); `&[u8] = req.body` (E0308); four unannotated async handlers (E0282); four
  partial `CorsConfig` literals; `app.get_async` documented as logging "automatically" when
  `enable_request_logging(bool)` is opt-in.
- Claims removed for lack of measurement, not re-numbered: "أقل من 20 نانوثانية", "أقل من 1
  نانوثانية", and the example page's "استجابة بأقل من 1 ميكروثانية". `docs/06`'s 6–25 µs figure
  stayed because it is a dated record of one earlier run.
- What held up: `framework/README.md` needed no correction at all once the scratch crate declared the
  same dependencies the README tells the reader to declare — my first E0433 on `bytes::Bytes::from`
  was a probe artifact and is recorded as such. docs/09's `RequestContext` field/method table, both
  `ServerLimits` (10 fields) and `AetherConfig` listings, `Response::{redirect,file,header,set_cookie}`,
  `group`, `static_asset`, `static_file`, `tls_from_files(..)?` + `.require_tls(bool)` and the logger
  line format all matched the code.
- Prose carry-over the block harness cannot see (same phantom, found by grepping `:id` over every doc
  after the compile pass): `docs/03_routing_and_radix_trie.md` precedence list and trie diagram, and
  `docs/00_framework_specification_{ar,en}.md` §4 plus the `param` table row all taught `/users/:id`
  as a parameter. Five sites now state that braces are the only parameter form and cite
  `router/src/trie.rs:84`, which was re-read for this edit rather than inherited. The two sites that
  already use `:id` as a counter-example were left as they were. Files: 3 more markdown files in this
  slice — 8 total, still markdown only.
- Re-extracted and re-checked after every edit, serially: `C:/tmp/t25/run5c.out` prints 9/9 programs
  and 52/60 fragments with the same 4 declaration listings and the same 4 external-reason failures.
  A first attempt at this run raced a background copy of itself over one scratch `src/bin/probe.rs`
  and printed a spurious program failure; the numbers above come from the clean serial run.
- Gate: `cargo test --workspace` exit 0, 50 `test result:` lines, **462 passed / 0 failed / 1 ignored**
  — re-run from the final text of all eight files (`C:/tmp/t25/test2.log`; the earlier
  `C:/tmp/t25/test.log` was the same command against the first five). Only markdown changed. **Linux
  and macOS remain untested; PROB-0012 stays open.** No independent agent was used.

2026-10-22 (track 25 — the doc-honesty sweep extended from counts to code examples, docs-only):
**four user-facing docs taught an API that does not exist; every example block that forms a
`fn main` program is now extracted from the markdown and compile-verified against the crate it
documents.**

- Register: **B12 recorded and closed**, following B11. B11 fixed stale *numbers*; B12 is the same
  drift class one level down, in the *code*.
- Phantom APIs removed (`docs/00_framework_specification_en.md`, `docs/00_framework_specification_ar.md`,
  `docs/06_middleware_and_performance.md`, `docs/08_getting_started_and_examples.md`):
  `aether::context` / `aether_http::response::Response` imports (the prelude is the only public path),
  `Response::json(&serializable)` (it takes `Bytes` and serializes nothing), `Response::ok(body)`,
  `.with_json`, `app.state_container`, `AetherApp::with_state`, `ctx.state.counter` (the handle is
  `Option<Arc<T>>`), `with_logger(LoggerMiddleware::new().enabled(true))`, async closures on the sync
  `app.get`, `/users/:id` (the router only recognises `{name}`) and `Box<dyn Error>` without
  `Send + Sync`. Replacements use the real surface: `app.manage::<T>()` + `ctx.state::<T>()`,
  `app.enable_request_logging(bool)`, `get_async`/`post_async`, `Response::json(Bytes)` /
  `json_status` / `text` / `text_dynamic` / `status`.
- Verification method, which is the point of this track: each block was extracted **from the doc file
  itself**, compiled with `cargo check` in a scratch crate depending on `aether` by path
  (`C:\tmp\t24\excheck`, `CARGO_TARGET_DIR` on the gitignored `framework/target`), the doc corrected
  to the compiling text, then **re-extracted and re-checked** — exit 0, no warnings. Three defects
  came from the compiler rather than from reading: handler arity (E0593), `.into()` on `Vec<u8>`
  being ambiguous across `bytes`/`rustls`/`rustls_pki_types` (E0283, solved by `String` from
  `serde_json::to_string`), and `bytes::Bytes::from` needing a dependency the example must not add
  (E0433). An `unused_mut` warning from an example added in the C1 commit is also gone.
- Honest scope limits: only whole-`fn main` blocks were compiled. Non-`main` fragments and
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md` / `GETTING_STARTED_AR.md` were name-scanned only (they use
  `Response::ok()` and `{id}` correctly) — not extract-and-compiled. A repo-wide phantom-name grep now
  returns only std/`bytes` helper names plus the sentences that say those APIs do not exist.
- Gate re-run after the edits: `cargo test --workspace` exit 0, 50 `test result:` lines,
  **462 passed / 0 failed / 1 ignored** (log: `C:/tmp/t24/test2.log`). The first aggregation of this
  run printed `ignored 0`; that was my regex, not the suite — re-derived from the same log with
  `test result: [a-z]+\. N passed; M failed; K ignored`.
- Files changed: 4 docs + 2 governance files, all markdown. No code, test or config touched.
  **Linux and macOS remain untested; PROB-0012 stays open.** No independent agent was used.

2026-10-22 (track 24 — the register's remaining `DOCUMENT` items closed, docs-only): **every
current-tense test count in the docs now comes from the file it names, and one of them turned out to
be un-reproducible rather than merely stale.**

- Register: **B2, B3, B4 CLOSED** (verified already corrected in the tree, re-measured rather than
  trusted), **B9 CLOSED** by replacing coordinates with anchors, **B11 recorded and closed** — the
  stale-count class B7 described is repo-wide, not one matrix row.
- Counts corrected against the tree at `6aaf36f`: `http_e2e.rs` 45 (docs said 31 and 41),
  `http2_e2e.rs` 29 = 17 TLS-ALPN + 9 cleartext `h2` client + 2 raw-framed-only + 1 preface-only
  (docs said 27, "8 cleartext", "4 hand-framed", "6 HTTP/1.1 peers"; measured 3 hand-framed
  HEADERS/DATA/SETTINGS tests and 4 tests that also send a plaintext HTTP/1.1 request),
  `scheduling.rs` 14 (matrix said 9), `aether-scheduler` 57 = 11 unit + 46 integration (matrix row
  said "27 crate tests (17 engine, 8 simulator, 2 stress)"), ADR-001's flood-defence evidence cell
  45/21 (said 41/18), root README's "verified in the current tree" line 462/45/29 (said 455/41/27).
- The one claim that could not be settled by measurement: framework/README's "27 tests that register
  middleware (13 + 8 + 3 + 3)". Neither the 3 nor the 3 reproduces. Under the rule now written next
  to the number — a test counts when its app ends up with a middleware, whether the call is inline or
  in that file's own `ext_app` / `marked_app` / `extractor_app` builder — the total is **32**
  (13 + 8 + 5 + 6). Two traps found while measuring: `extractor_app` also exists in `extractors.rs`
  with no middleware, so a repo-wide grep for the helper name yields 15 false positives; and slicing a
  test body to the *next* test attribute instead of the column-0 `}` swallows the file-local helpers
  and mislabels the preface test.
- B9's invariant restated without line numbers: no `serve.rs` clippy location falls between the
  `if let Some(ws_handler_ref) = self.ws_handlers.get(&handler_id)` arm and the
  `else if let Some(handler_ref) = self.handlers.get(&handler_id)` arm. Measured 2026-10-22:
  `cargo clippy -p aether` → lib scope 13 warnings, `serve.rs` locations **87, 192, 445, 461** — the
  fourth different set of coordinates for the same invariant, which is the point.
- Gate re-run on this slice's tree, because docs are not build inputs but the suite line is still a
  claim: `cargo test --workspace` → **462 passed / 0 failed / 1 ignored**, exit 0
  (`C:/tmp/t24/test.log`, 50 `test result:` lines). No source, test or config file changed; 6 files,
  all markdown.
- Not done here, still open: **B8** (external-client rows cannot be re-run from the repository —
  owner decision), **A6**, **PROB-0012** (Linux/macOS, environmentally blocked), PROB-0004/0005/
  0008/0009/0011/0015/0016, PROP-0001, and E1–E9 (explicitly not authorized). No independent agent
  was used for any measurement above; all six files are doc-only, so nothing here moves behaviour.
2026-10-22 (track 23 — pre-publish review turned into measurement, the last red CI-shaped gate
closed, PROB-0014 closed): **the publish risk is the repository's own history, not Linux, and the
two bounds the protocol stage advertised are now operator-settable.**

- Public-repo proof, because a content review only matters if anyone can read it: anonymous
  `git -c credential.helper= ls-remote` returns tip `fb6a4e8` (so no credential is used to read it)
  and the repository page is fetchable without a session. The remote tip equals local `origin/main`.
- Payload measured, not estimated. `git rev-list --objects main --not origin/main` → 7679 lines
  (7647 carry a path); joined to `git verify-pack -v` on the single local pack, taking field 4
  (size-in-pack), 7312 of those objects resolve to **670.8 MiB**, of which **670.8 MiB — 99.9886 %,
  7275 paths — sit under `framework/target/`**. The remaining source, docs and governance content
  totals well under 0.1 MiB. 335 path-carrying objects were loose at that moment and are not in the
  figure; `git count-objects -v` bounds them at 442 loose / 4.2 MiB. Why build output at all:
  `git ls-tree -r --name-only main | grep -c '^framework/target/'` → **0**, and the same on
  `origin/main` → **0**, so no tree contains those files — they were committed in the early history
  and deleted later, and a push transfers them because they are reachable from the unpushed chain.
  Under the no-history-rewrite rule the payload is a property of that chain, so this is a fact about
  *how* to publish, not a defect to fix in code.
- The internal notes decision, tested against its own gate rather than assumed. Owner ruling:
  `notes/2026-10-20-chat-paste.md` (434 lines, 32.6 KB) stays local, unpublished, undeleted. Its
  blob `84838e6` is inside `origin/main..main`, so "keep it local and never publish" and "push the
  current chain" are mutually exclusive without a rewrite — that is the reason a preview snapshot is
  on the table at all. Building one proved the second half of the owner's condition: an orphan
  `git commit-tree HEAD^{tree}` in a scratch `--shared` clone is 194 paths / 3.6 MiB uncompressed,
  but `git grep -l -e 'C:\\Users\\hamza' -e '.qoder' preview` still matches
  `.project-governance/notes/2026-10-20-chat-paste.md`. So a compliant snapshot must exclude
  `.project-governance/notes/` (193 paths), which makes it content-different from `main` and is
  exactly why it can only live on its own never-merged branch. **Not created: no commit-tree object
  was written into this repository, the measurement ran in a throwaway clone, and the branch
  question is the owner's.**
- Gate closed. `cargo clippy --workspace --all-targets` was red for one deny-level
  `clippy::unused_io_amount` at `tests/tests/tls_e2e.rs:452`, where a byte-wise keep-alive upgrade
  head read ignored the return value — an EOF there looped forever rather than failing. It now uses
  `read_exact`, the idiom the same file already used at 402–414. Measured after:
  `cargo clippy --workspace --all-targets` exits **0** with **0** `^error` lines, and the lib-scope
  warning total is unchanged at **39** (summed from the per-crate `generated N warnings` lines, so
  the number counts the same thing it did last track — and the sum has to include the singular
  `generated 1 warning` line, which a `warnings`-only regex drops and turns a true 39 into 38). The gate that had been declared "stays red"
  in three consecutive track entries is now green, with no source change and no allow attribute.
- **PROB-0014 closed** — `max_concurrent_streams` and `max_headers_count` are now `AetherConfig`
  fields (`framework/aether/src/config.rs`), read from `ServerLimits::default()` rather than
  restated as literals so the two cannot drift, and read from `AETHER_MAX_CONCURRENT_STREAMS` /
  `AETHER_MAX_HEADERS_COUNT` in `from_env` (both reject `0`: a zero line budget fails every request
  that carries a `Host` line, a zero stream cap advertises a connection that refuses everything
  offered to it). `with_config` and `apply_config` stopped hand-copying a subset of fields into
  `ServerLimits`; both now call one `AetherApp::limits_for`, which is the bug class that made the
  two sites disagree with the documented config. Proof: `a_concurrency_cap_set_through_the_config_is_advertised_and_enforced`
  opens 9 streams against a configured cap of 4, reads `SETTINGS_MAX_CONCURRENT_STREAMS` **off the
  wire** and asserts it equals 4, then asserts every stream was answered or reset with
  `REFUSED_STREAM` and no GOAWAY; `a_field_line_budget_set_through_the_config_is_the_number_it_counts`
  configures 8 field lines and checks the boundary — 9 lines answer 431, exactly 8 answer 200 — so a
  wired field is told apart from a coincidental agreement between two constants. Both default-pinned
  tests were refactored to share that wire code and now assert the advertisement too, which the
  default-only versions did not.
- Suite after this track: **462 passed / 0 failed / 1 ignored** (263 unit + 194 integration across
  15 targets + 5 doc), `cargo check --workspace --all-targets` and `cargo build --workspace` clean.
  `http2_e2e` 27 → 29, `aether` lib 29 → 30. Downstream consumers re-checked against the widened
  struct: `cargo check` in `../aether-bench-compare` finishes (no `AetherConfig` use), and
  `../aether-probe` fails with only its documented pre-existing `E0283` — its one `AetherConfig`
  construction uses `..AetherConfig::default()`, so the new fields are source-compatible.
- Declared limits, so the next round does not over-read this one: the two new `from_env` branches
  have **no test** — setting `AETHER_*` means `std::env::set_var`, which is `unsafe` in Rust 2024
  and process-global under a parallel harness, and this repository has no precedent for testing an
  env var at all. The `0` rejection therefore protects only the environment path: an app built with
  `AetherConfig { max_headers_count: 0, ..Default::default() }` reaches both frontends as 0, and
  neither rescues it — HTTP/1 sizes its array with `.max(1)` but compares `headers.len() > 0`
  (`parser.rs:82` and `:171`), and the HTTP/2 counter has no clamp at all, so every request with a
  header line is refused. That is precisely why `from_env` rejects `0` rather than clamping it, and a
  constructor-level guard would be a separate decision, not a silent override of an operator value.
  The HTTP/1 half of `max_headers_count` is exercised at the `ServerLimits` level
  (`http/src/parser.rs` unit tests), not through `AetherConfig`; the h2 tests are what prove the
  derivation reaches a frontend. Windows/MSVC only — **PROB-0012 (Linux/macOS) stays open**, and the
  one platform-conditional block in the workspace, `#[cfg(unix)]` in `aether/src/app.rs:2474` (the
  SIGTERM arm of `wait_for_shutdown_signal`; `grep -rn "cfg(unix)" framework` returns exactly this
  line), is a compile surface this host has never type-checked: `cfg`-ed-out code is parsed but not
  checked, so a green `cargo check --workspace` says nothing about it. No performance claim: nothing
  was benchmarked this track, the release binary was not re-built, and no independent agent was used
  for any measurement above.
- How this track landed, because the split is part of the record. The owner ruled that the work be
  pinned as three commits and that the real diff be reviewed first rather than a summary: (1) the
  configurable bounds, their tests and `PROB-0014`'s closure; (2) the `read_exact` gate fix with the
  clippy-gate documentation; (3) this record plus the remaining count reconciliation. Each group was
  measured on **its own exported tree** (`git checkout-index -a` of the staged index, then the
  command run there), not only on the cumulative tree: commit 1 measures `cargo test --workspace`
  462 / 0 / 1 with all four bound tests present, and its exported tree still shows the un-fixed
  `tls.read(&mut byte)` at `tls_e2e.rs:452` — the gate was read as still red there, it was not
  re-run, because clippy was measured only on commit 2's tree, where it exits 0 and the suite is
  still 462 / 0 / 1. Markdown is not a build or doc-test input in this workspace (no `include_str!`
  of any `.md`), so the doc-only differences between a staged tree and the next one cannot move
  those numbers. The dates in this entry are the measurement day, 2026-10-22 UTC; the three commits
  carry local timestamps of 2026-10-23 because the work crossed local midnight.
2026-10-22 (track 22 — read-only divergence review, then the owner-authorized merge of `origin/main`):
**`main` and `origin/main` diverged by 29/1 over one merge base, the remote commit was pure generated
build output, and the merge that adopts it changes no file.**

- Ground truth first, because the report's numbers were the agent's own: `git merge-base --all main
  origin/main` → one base, `1116e1a` ("first commit", 2026-10-19 18:40 +0300); `git rev-list
  --left-right --count main...origin/main` → `29 1`; `git rev-list --parents -n1 fb6a4e8` → its only
  parent is that base. The remote commit was authored by the owner and **committed by GitHub**, i.e.
  made in the web UI at 2026-10-19 21:59, three and a half hours after the base.
- What `fb6a4e8` actually did: **1841 files, 8596 deletions, 0 insertions, and all 1841 statuses are
  `D`**; `git diff --name-only 1116e1a origin/main` has **0 paths outside `framework/target/`**. The
  only four `.rs` entries in it are `framework/target/{debug,release}/build/serde*/out/private.rs` —
  build-script output, not project source. So the remote change is the hygiene fix of PROB-0010, done
  a second time and by hand on GitHub, with no content in it.
- Conflict analysis done by path sets, not by eye: the local `01e79ef` deleted **the same 1841 paths**
  (`comm -23` and `comm -13` on the two sorted deletion lists both → 0), the 84 locally added or
  modified paths have **0** intersection with the remote deletion set, **0** paths exist on the remote
  that `main` lacks, and `.gitignore` (`/framework/target/`) is a local-only add on a branch that has
  no `.gitignore` at all. Delete/delete with identical sets and no modify/modify overlap has only one
  possible outcome, and that outcome was then measured instead of asserted.
- The measurement: a `git clone --shared` into scratch (so the owner's object database stayed put —
  `git count-objects -v` before and after, and the clone was deleted afterwards) with
  `git merge-tree --write-tree main remote-main` → **exit 0, result tree `c3ff6719`, which is
  byte-for-byte the tree `main` already had** (`git rev-parse main^{tree}` = the same OID).
- Corrected in the follow-up commit, because the sentence above claimed more than the first run
  measured: that run printed `count-objects -v` only **after** the probe. The strict before/after
  comparison is a second, identical probe (`clone --shared` → `merge-tree --write-tree main rm` →
  delete the clone): source `438 loose / 4222 KiB / 9126 in-pack / 1 pack` before **and** after, probe
  exit 0 again. So the claim "the probe wrote nothing into this object database" is now measured, not
  inferred, and the merge result reproduced outside the original run.
- Execution, after the owner read that analysis and said to merge: `git fetch --dry-run origin` (no
  output → `origin/main` had not moved since the last fetch, so the merge target was the commit that
  had been analysed, not an unknown one), a `backup/pre-merge-main` ref at `a7be6cb` as a rollback
  path, then `git merge origin/main` → "Merge made by the 'ort' strategy", no `files changed` line,
  **`41dbe39` with parents `a7be6cb` + `fb6a4e8`**, `HEAD^{tree}` = `c3ff6719` = the pre-merge tree,
  `git diff --stat a7be6cb HEAD` empty, `git status --short` 0 lines, and
  `git rev-list --left-right --count` now `30 0`. No history was rewritten, no commit was amended,
  nothing was pushed, and all 29 local commits are still in `main`'s ancestry with their original OIDs.
- **Tests were deliberately not re-run.** The reason is methodological, not convenience: a claim about
  "does the suite pass after the merge" needs the same bytes to be present, and tree OID equality with
  `a7be6cb` *is* that proof, so a re-run would measure the same sources again and add a log, not
  evidence. The post-split **459 / 0 / 1** measurement is therefore still the current one. If a
  post-merge test record is wanted for its own sake, that is a separate request.
- Verification limits, unchanged and still declared: every result in this repository is Windows/MSVC
  (**PROB-0012 open** — Linux and macOS have never run this suite); clippy `--workspace --all-targets`
  is still red on the pre-existing `clippy::unused_io_amount` at `tests/tests/tls_e2e.rs:452` (lib
  scope 39 diagnostics, none correctness); no performance claim was made or measured in this track;
  the release build was not re-run; and no independent agent was used — the review, the merge and both
  verifications were done by this agent with the commands quoted above.
- Two decisions stay with the owner and were **not** taken here: whether to `git push origin main`
  (`main` is now 30 ahead / 0 behind, so a push would be a fast-forward, but it would publish content
  that has never been reviewed on Linux/macOS), and when to delete `backup/pre-merge-main`. Deleting
  `framework/target` blobs from history remains out of the rules — it needs a history rewrite, and the
  merge neither added nor removed any.
2026-10-22 (track 21, step 5 — the commit split executed with per-stage verification): **the
accumulated tree is now five commits on `main`, and each stage was built and tested on its actual
staged tree before its commit was created.** The owner approved execution after the steps-2-4
report ("كمل").

- The split, in the owner's order, each commit's tree exported with `git checkout-index` from the
  staged index and gated with a full `cargo test --workspace` before `git commit`:
  - `1d0502e` aether: add request-scoped extensions (type-map) and Extension extractor —
    **411 passed / 0 failed / 1 ignored** (255 unit + 151 integration + 5 doc; `extensions_e2e` 6,
    `http_e2e` still at HEAD's 31).
  - `32df904` aether: serve HTTP/2 over TLS via ALPN and opt-in cleartext h2c — **445 / 0 / 1**
    (262 + 178 + 5; `http2_e2e` 27 green on a tree whose `serve.rs` is still HEAD's, which is the
    proof the h2 slice stands alone; `tls_e2e` 9 with the new ALPN assertion).
  - `75e07cc` aether: run user middleware around WebSocket upgrade requests — **457 / 0 / 1**
    (262 + 190 + 5; `http_e2e` 43 = 31 + 12 chain tests; `scheduling` green with the new
    completed==1 semantics, which is why that test file belongs to this stage and not the h2 one).
  - `96ba84b` aether: refuse a middleware-fabricated 101 as a 500 — **459 / 0 / 1** (262 + 192 +
    5; `http_e2e` 45). Kept as its own reviewable change per the owner's Decision-1 ruling; the
    Decision-2 two-layer comment above the WS method gate travelled with the middleware commit
    that owns the guard it documents.
  - the docs/governance commit (this one): no code difference from `96ba84b` — the working tree
    was verified byte-identical to HEAD for every code path before staging (`git diff HEAD --
    framework/aether framework/http framework/tests …` empty), so the 459/0/1 measurement of the
    final tree covers this commit's tree too.
- Method, so the next round can audit it: the four files whose diffs mixed stages (`app.rs`,
  `lib.rs`, `serve.rs`, `http_e2e.rs`) were built as intermediate versions — `app.rs` in four,
  `lib.rs` in two, `serve.rs` and `http_e2e.rs` in two — and the forward chain HEAD→s1→s2→s3→s3b
  was verified **byte-exact** against the working tree before anything was staged (diff lines: 0;
  two hand-wrapping drifts in doc comments were caught by exactly that check and sourced from the
  final file instead). Whole-stage files were staged with `git add`, mixed intermediates with
  `git hash-object -w` + `git update-index --cacheinfo`. Working-tree files were never modified,
  no history was rewritten, no `git stash` was used, and each commit contains precisely what its
  exported-and-tested tree contained.
- The 5-commit split adds one commit to the owner's four-stage list: the `101` refusal is its own
  change between the WebSocket middleware commit and docs, because the owner required the fix as
  "تغيير مستقل قابل للمراجعة". Extensions → HTTP/2 → WebSocket middleware → 101 fix → docs/governance.
- Limitations unchanged and still declared: clippy `--workspace --all-targets` stays red on the
  pre-existing `clippy::unused_io_amount` at `tests/tests/tls_e2e.rs:452` (lib scope 39, none
  correctness); `PROB-0012` (Linux/macOS) stays open; no performance claim; release build not
  re-run. No independent agent was used for any measurement in this track.
2026-10-22 (track 20, steps 2-3 of the owner's approved order — test independence, then the `101`
protocol-correctness fix as its own change): **the four tests are independent and the fabricated
`101` is now refused with `500`.**

- Independence (step 2): each of the five new/extended tests passes alone via
  `cargo test -p aether-tests --test <target> -- --exact <name>`, both files pass with
  `--test-threads=1` (deterministic order) and with the default parallel harness, and the coupling
  candidates were checked rather than assumed — ports come from `127.0.0.1:0`, every test builds its
  own `App`, every counter is a test-local `Arc<AtomicUsize>`, and the only process-global mutable on
  the serving path (`REQUEST_COUNTER`, `aether/src/app.rs`, which numbers `X-Request-ID`) is asserted
  as `.is_some()`, never as a value.
- The fix (step 3), one match arm in `framework/aether/src/serve.rs`:
  `None if chain_response.status == 101 => Response::status(500, "Internal Server Error")`. All four
  conditions the owner attached were measured, not argued — the refusal fires only when the terminal
  never ran (a guard that calls `next` still upgrades: `101` + the RFC accept vector + session started
  + frame echoed); HTTP/2 and h2c are untouched (that arm is unreachable there, `http2_e2e` held at
  27); the `500` carries no body and none of the guard's claims (`X-Request-ID`, `Date`,
  `Content-Length: 0` only, even with `Upgrade`/`Connection`/`Sec-WebSocket-Accept`/JSON body injected
  into the `101`); and deleting the arm puts `left: 101, right: 500` back. Scope is tested too —
  `103`/`418` on a WS route and a hand-built `101` on a **non**-WS route all still travel the ordinary
  path, and widening the arm to `status < 200` makes that test fail `left: 500, right: 103`.
- Deviation recorded, not hidden: no extra log line. `LoggerMiddleware` already reports the response
  status per request in the same post-pass, so the refusal is visible as its `500`; a per-request
  `eprintln!` there would be an unbounded stderr write on a client-influenced path. The non-WS-route
  `1xx` question is registered as **A6** rather than folded in without a decision.
- Decision 2 landed as documentation, not a refactor: the WS method gate stays, and `aether/src/serve.rs`
  now states the two responsibilities separately (router = which methods a route accepts, `405` +
  `Allow`; serving core = nothing that slipped past reaches the upgrade path).
- Suite: **459 passed / 0 failed / 1 ignored** (262 unit + 192 integration across 15 targets + 5 doc;
  `http_e2e` 44 → 45), recounted from the per-target `test result:` lines. `cargo check --workspace
  --all-targets` clean, `cargo build --workspace` clean, `cargo clippy --workspace` still exactly 39
  diagnostics with none in the correctness group (so this change added none), `cargo clippy --workspace
  --all-targets` still **red** on the same pre-existing `clippy::unused_io_amount` at
  `tests/tests/tls_e2e.rs:452` — untouched this round, still a separate decision. Downstream re-checked:
  `../aether-bench-compare` finishes clean, `../aether-probe` still fails with only its documented
  pre-existing `E0283` at `src/main.rs:98` and no new error.
- Counts elsewhere were re-derived while I was here, and two more doc errors surfaced: the `aether`
  lib breakdown said **9** TLS unit tests where `aether/src/tls.rs` has **10** (6 app + 8 extensions +
  5 http2 + 10 tls = the 29 that run), and README's "20 tests that register middleware (8 + 12)" had
  simply never counted `http2_e2e` (3) or `extensions_e2e` (3) — the measured figure is 27. `cargo fmt
  --all --check` went 558 → 567 diff blocks across the same 66 files (still not a gate, still not run).
- Step 4, run on the final tree: the last code change was comment-only (the Decision-2 two-layer note
  replaced the shorter one above the WS method gate), and because a comment in a linted file moves
  coordinates the gates were run again rather than quoted — `cargo test --workspace` 459 / 0 failed /
  1 ignored (262 + 192 + 5; `http_e2e` 45, `http2_e2e` 27), `cargo check --workspace --all-targets`
  clean, `cargo build --workspace` clean, lib-scope clippy 39 diagnostics and 0 errors, `--all-targets` still one error at the same
  pre-existing `tls_e2e.rs:452`. The `serve.rs` locations shifted 442/458 → 445/461 (full set
  87/192/445/461), and none falls inside the upgrade branch (239–346) — the invariant the matrix
  now states, with the coordinate as its snapshot only. No unintended code change was found: the only
  files touched after the fix are docs and governance.
- Windows only: `PROB-0012` stays open, no performance claim, release build not re-run. **No commit was
  created.** No independent agent was used.
2026-10-22 (track 19, the owner's three approved tracks — docs corrected, four tests written, split
planned but **not executed**): the five CONTRADICTED doc claims were re-measured and rewritten to
describe the actual tree, including the wording that refuses to call any clippy scope "clean"
(`cargo clippy --workspace --all-targets` still fails on `clippy::unused_io_amount`,
`tests/tests/tls_e2e.rs:452`, pre-existing at HEAD; lib-scope is 39 lint mentions with none in the
correctness group). All four behaviour tests landed and **each was falsified by injecting its target
fault and reading the failure**, which is what produced two corrections the documentation had wrong:

- the `405` for a non-`GET` on a WS route is the **router's** answer (`app.ws` registers `GET` only,
  `router::RouteNode::get_handler` has no HEAD→GET fallback, so `MatchResult::MethodNotAllowed` is
  served without dispatching). The WebSocket branch's own method gate is a second line of defence that
  normal registration cannot reach — made reachable in a scratch build it answered `405` with the
  chain at `0`, and removing it there handed a `POST` a `101`.
- a hand-built `101` from a middleware is **not** a framed lie: the HTTP layer already sends every 1xx
  body-less, so the client gets `HTTP/1.1 101` with the ordinary post-pass headers, no
  `Sec-WebSocket-Accept`, no body, and a connection that still serves HTTP/1.1. Injecting the
  candidate fix (`None if status == 101 → 500`) moved **only** the reproducing test and left the 12
  WebSocket-chain neighbours green — cost measured, decision still the owner's (task #12).
- both HTTP/2 refusals (TLS and h2c) now assert a middleware counter, and disabling `!allows_upgrade`
  makes **both** fail `left: 1, right: 0` while the status stays `400`, demonstrating the blindness the
  old status-only tests had.
- shutdown-abort of a live session is now tested (echo first, then `stop()`, then EOF with zero bytes
  and the handler's post-loop counter at `0`); detaching instead of aborting makes it hang.

Suite: **458 passed / 0 failed / 1 ignored** (262 unit + 191 integration across 15 targets + 5 doc),
`cargo check --workspace --all-targets` clean. Windows only — `PROB-0012` (Linux/macOS) stays open and
no performance claim is made; the release build was not re-run for these tests. Code changes this
round were test-only plus doc comments and one comment in `aether/src/serve.rs`; the four falsification
patches were all reverted and verified reverted. **No commit was created.** The Gate-6 split (Extensions
→ HTTP/2 → WebSocket middleware, then docs/governance) remains a plan awaiting the owner's go-ahead.
No independent agent was used for any of the measurements above.
2026-10-21 (pre-commit audit, Gates 5 and 6): **the accumulated tree was audited claim by claim and
no code was changed.** The durable record is
[`.project-governance/notes/2026-10-21-improvement-register.md`](notes/2026-10-21-improvement-register.md)
— Gate-4 findings written up as documentation-only, every Gate-5 claim classified against a
re-runnable measurement, the Gate-6 split constraints, and the performance/structure backlog the
owner asked to have written down so it is not lost. Five doc claims were CONTRADICTED by
re-measurement (`clippy --all-targets` fails on a deny-by-default correctness lint in
`tests/tests/tls_e2e.rs:452`, pre-existing at HEAD; §2/§5 still say HTTP/2 is not implemented; §3's
matrix status counts are 94/11/22 over 133 rows while the matrix and a mechanical recount give
97/12/20 over 134; §5 says doc tests contain no runnable examples while `Doc-tests aether` passes 5;
matrix `:164` cites 16 HTTP/2 wire tests against 27 in the file). Two behaviours are stated in docs
with no test behind them (non-`GET` on a WS route → 405 before the chain; the HTTP/2 `400` is
asserted as a status only, not as a chain skip), and one edge found here is untested and
under-described (a middleware that returns a hand-built `101` without the terminal is served as an
ordinary response — post-pass included — with no protocol switch). What held up on the final tree:
455/0/1 with the 262+188+5 split, `cargo check --workspace --all-targets` exit 0 clean, release build
with zero warnings, lib-scope clippy 39 with no correctness lint, and the WS/TLS/limits/JWT/fuzz/
extension-pricing rows. **Gate 6 found the split order the owner proposed is blocked by real code
dependencies:** `http2.rs:264` and the h2 extension tests reference the extensions map, so HTTP/2
cannot be committed before Extensions; `tests/tests/scheduling.rs`'s changed tally must travel with
the WS commit; `app.rs @@ -1117` is the single hunk that mixes HTTP/2 and WS lines and needs a
line-level split verified by building each intermediate tree. Docs are not stage-separable (every
doc file carries all three stages), so the plan is 3 code commits + 1 docs/governance commit. No
commit was created and no code changed; Windows only, **PROB-0012 stays open**, no performance claim.
No independent agent was used for this round — every verdict comes from a command recorded in the
register so the owner can re-run it.
2026-10-21 (track 18, handoff-edge round): **the four WebSocket-middleware cases the owner left open
were settled, and one earlier doc claim was corrected by a test.** Scope was explicitly not expanded:
no router redesign, no performance work. (1) *Double `next`*: not expressible, and that was measured
rather than read off the signatures — a scratch middleware that calls `next(req)` twice fails to
compile with `error[E0382]: use of moved value: req` (`RequestContext` does not implement `Copy`); the
scratch file was deleted after capturing the error. Stated as a type-level guarantee instead of faked
with a runtime test, and the handover slot is `Option<RequestContext>` so a repeated terminal visit
could not start two sessions either. (2) *Chain in a separate task / chain dropped*: two new wire tests,
`a_middleware_that_forks_the_chain_still_answers_the_upgrade_request` (the chain is `tokio::spawn`ed,
completes with the terminal's `101`, the guard answers 202 → the client gets 202 and the handler never
starts) and `a_middleware_that_abandons_the_chain_future_refuses_the_upgrade` (chain future built and
dropped → the guard's 503 is served and no session starts). That second test **corrected the
explanation I had written for it**: the handover slot is filled synchronously when `next` is called,
because the terminal's closure body records the request before it returns its future, so an abandoned
chain *does* reach the terminal. What refuses the upgrade is the status identity of the response the
chain returns, not the terminal being unreachable — which is exactly why the guard is a status
comparison rather than a "route reached" flag. Supported behaviour is now stated in the `ws`
docs, `framework/README.md`, matrix item 18 and both Arabic docs: **what the client receives is the
response the guard returns, never the chain's.** Falsified, not assumed: with `.filter(|_| true)` in
place of the status guard, the fork test fails `left: 101, right: 202` and the abandoned test fails
`left: 101, right: 503`; restored to
`.filter(|_| chain_response.status == 101)` and green. (3) *Two header stores*: settled as a contract
instead of an accident — public-map writes are handler-visible, never framework-visible — documented
on `RequestContext::headers`, and pinned in both directions by
`editing_the_public_header_map_does_not_grant_or_revoke_a_websocket_upgrade` (strip `Upgrade` /
`Connection` / `sec-websocket-version` from the map of a real handshake → still `101` +
`handler-ran`; write the four handshake keys into the map of a plain `GET` on the same route → still
`400`, and the same connection then serves `GET /ping` normally). Whether the two stores should merge
into one write-through surface is the open owner decision **PROB-0005**, deliberately not pre-empted
here. Writing that test also corrected a belief the docs carried: there is **no per-request
upgrade-header gate** before the chain — the only pre-chain refusals on a WS route are non-`GET`
(405) and a transport that cannot hand a stream over (`!allows_upgrade`, HTTP/2 → 400) — so a plain
`GET` on a WS route does run the chain and the 400/426 comes from `validate_handshake` afterwards.
Matrix item 18, both Arabic docs and the test comment now say exactly that. Suite re-run on the final
tree: **455 passed / 0 failed / 1 ignored** (262 unit + 188 integration across 15 targets, `http_e2e`
41 with the 10 WS-middleware tests + 5 doc); `cargo build --workspace --release` clean in 1m 18s;
`cargo clippy --workspace` still 39 warnings (13/9/7/4/3/2/1), with the `aether` `serve.rs` locations
re-measured at 87, 192, 432, 448 — none inside the upgrade branch. Docs refreshed to 455 everywhere
(matrix evidence + Unit/Integration/E2E/WebSocket rows + items 16/18, `PRODUCTION_READINESS` §2/§10/
§11/§13 including a new handoff-edge paragraph, `BASELINE_MEASUREMENTS` table, both READMEs,
`GETTING_STARTED_AR`, `FULL_FRAMEWORK_DOCUMENTATION_AR`, `ADR-001`). `cargo fmt` was not run: it
reports 558 diff blocks across 66 pre-existing files, which would touch far outside this slice, and
that non-action is now stated in matrix item 16. Windows only — **PROB-0012 stays open**, Linux and
macOS have still never run this suite, and no performance claim is made. No independent agent was
used for this round; the falsification is the `.filter(|_| true)` swap described above. No commit was
made, per the owner's rule that the commit decision follows this report.

2026-10-21 (track 18, review remediation): **the WebSocket-middleware handoff was defective and an
independent adversarial review found it.** The first cut flagged "the route was reached" with an
`AtomicBool` set by the terminal, so a guard that awaited `next` and *then* answered had its answer
discarded while the socket was upgraded anyway — the exact opposite of the refusal power the slice
claimed, and it contradicted the sentence written in its own test doc. Sequence, per the protocol:
the review ran over the diff, its two findings were re-verified in the code before anything changed
(`RequestContext::header` does read the private parsed store, so finding 4 — middleware cannot revoke
by editing headers — is real and stronger than reported; `Response.status` is a public `u16`, so the
terminal's sentinel can be identified without a user-visible marker), then the flag was replaced by a
status guard: the upgrade proceeds only when the chain returns the terminal's `101` untouched.
Two new wire tests plus a widened third:
`a_middleware_that_answers_after_next_refuses_the_upgrade` (403 after `next`, handler counter stays
0, connection still serves), `a_middleware_hanging_after_next_times_the_upgrade_request_out`
(`max_request_time` 150 ms → 504 + `Connection: close`, no `101`, no orphaned socket), and
`a_panicking_middleware_answers_the_upgrade_request_with_500` now panics both before **and** after
`next`. **The fix was falsified, not assumed:** replacing the status guard with the old
always-hand-off behaviour made all three fail with `left: 101`, `right: 403 / 504 / 500`, and they
pass with it in place. Suite **452 passed / 0 failed / 1 ignored** (262 unit + 185 integration across
15 targets + 5 doc; `http_e2e.rs` is 38). `cargo build -p aether` and `cargo doc --no-deps -p aether`
clean apart from the pre-existing `MultipartError::UnsupportedContentType` link; clippy's unique
warning count is unchanged and none of its `serve.rs` locations (87, 192, 429, 445) fall inside the
upgrade branch. Documentation corrected wherever the old claim was written: `AetherApp::ws` (the
"exactly as it does for an HTTP route" phrase removed, the header-store limit and the embedder-facing
behavioural break added), matrix item 18 and the two WebSocket/test rows, `PRODUCTION_READINESS §13`,
`ARCHITECTURE §5`, `AETHER_CORE_CAPABILITIES §2`, both READMEs and both Arabic docs. Matrix item 16
was also repaired honestly rather than re-guessing: the "41 could not be reproduced" note was wrong —
39 is the sum of `cargo clippy --workspace` per-target summaries, 41 is that command's raw `warning:`
line count under `-p aether --all-targets` including lib/lib-test duplicates, and the doc now names
which number comes from which count. Independent agent: **used** for this review (the track-18 entry
below, written before it, says otherwise — that statement is superseded). No commit; no benchmark (the
owner measures); Windows only, `PROB-0012` remains open.

2026-10-21 (track 18): **PROB-0017 closed — WebSocket upgrade requests now run the route's middleware
chain.** Found while documenting track 17, by asking which routes `wrap_with_middleware` actually
touches: `app.ws` inserted into `ws_handlers`, which that function never sees, and
`ServiceCore::serve` answered an upgrade with `ServeOutcome::WebSocketUpgrade` without dispatching a
handler — so an `use_middleware` auth guard covered every HTTP route and left every `app.ws` route
open, and a middleware-inserted extension could never reach a session. `RouteGroup::ws` compounded it
by calling `AetherApp::ws` and dropping the group's own middleware. Method, per the protocol: a
characterization wire test ran against the unmodified tree first and measured the defect (HTTP control
route `stamped`, upgraded handler reported `absent`); only then was the code changed. Delivered: each
`ws` registration captures its middleware list (app then group, same composition as HTTP routes) in
`ServiceCore::ws_middleware`; on an upgrade request the core folds it around a terminal that records
the request and flags that the route was reached (a `101` is synthesized by the frontend, so the
terminal cannot answer), dispatches it through the same `dispatch` path — admission, request timeout,
panic mapping — and either serves the middleware's refusal or validates the handshake against the
post-middleware context and hands that context to the session. Suite **450 passed / 0 failed /
1 ignored** (262 unit + 183 integration across 15 targets + 5 doc): 5 new wire tests in `http_e2e.rs`
(chain runs, refusal answers 401 and the handler counter stays 0 and the socket still serves the next
request, panic maps to 500, group middleware composes, late-registered middleware does not wrap).
`cargo build --workspace --release` clean; clippy unchanged, with no reported location inside the new
code. **One existing expectation was corrected, not weakened:**
`scheduling.rs::panicking_websocket_session_is_tallied_as_cancelled` asserted `completed == 0` and
failed on the first run after the change, because an upgrade request is now itself a dispatched unit of
work; the measured tally is `admitted 2, completed 1, cancelled 1`, so it asserts `completed == 1`
(the chain) with the panicking session still the cancellation — a session that tallied itself would
make it 2 and fail. Deliberate non-actions: the Axum shape where the upgrade *is* a `Response` was not
adopted (it moves socket handover and the WS memory re-pricing into the response path and breaks every
`app.ws` handler; recorded as matrix item 18), no benchmark was run (the owner measures), and no
commit was made. Limits stated in the matrix, both READMEs, `ARCHITECTURE §5`,
`AETHER_CORE_CAPABILITIES §2`, `PRODUCTION_READINESS §13` and the two Arabic docs: middleware cannot
edit the `101`, nothing wraps the session after handover, a WS route over HTTP/2 still answers `400`
before middleware, and an upgrade now consumes a handler admission slot (no dedicated test for the
503-under-load case). Windows only — PROB-0012 remains open. No independent agent was used for this
slice; the falsification attempts are in the tests named above. *(This last sentence was true when
written and is now superseded: an independent adversarial review run afterwards found the handoff
defect described in the entry above, which is what the 452-test recount reflects.)*

2026-10-21 (track 17): request-scoped extensions landed as a complete slice. Ground truth first:
`PRODUCTION_READINESS §9` and the matrix still listed "typed extractors and request extensions" and
"HTTP/2" as remaining work while the code had extractors (`extract.rs`, 18 wire tests) and middleware
(`user_middleware.rs`, 8 wire tests) — so the remaining-work lists were re-derived from the tree, not
trusted, and `extensions` was the one genuinely missing parity mechanism. Delivered:
`aether/src/extensions.rs` (a `TypeId` map, boxed only on first insert, `insert`/`get`/`get_mut`/
`remove`, values need not be `Clone`, `Debug` prints a count never the values), plumbed through
`RequestParts` → `RequestContext::from_parts` so both frontends populate it identically, exposed as
`RequestContext::{extensions, extensions_mut, extension, insert_extension}` and as the `Extension<T>`
extractor (Clone-gated exactly like Axum's). Suite **445 passed / 0 failed / 1 ignored** — 262 unit +
178 integration across 15 targets + 5 doc; 19 of those are new here (8 unit, 6 HTTP/1.1 wire,
3 HTTP/2 wire, 2 doc examples). `http2_e2e.rs` is now **27** tests, and its transport split was
recounted from the file (17 TLS-with-ALPN, 8 cleartext via the `h2` client, 2 raw-frame-only) because
the three different splits previously recorded in three different documents were all wrong or
out of date. The falsification attempts that matter are in the tests, not in prose: a non-`Clone`
value defeats a cloning context, four concurrent streams that must each finish inside 280 ms defeat a
sequential "concurrency" test, and an unmarked fourth stream defeats a connection-scoped map.
Newly registered **PROB-0016** (extension payloads are not priced by the memory budget — same
accounting class as PROB-0008, needs an owner decision because an arbitrary `Send + Sync` type has no
measurable size). No independent agent was used for this slice; it was self-reviewed against
`http::Extensions` and Actix `req.extensions()`, and §6 of the vision requires saying so.
Clippy is unchanged at 41 style warnings and none of them come from this slice (the single
`extract.rs` warning is a pre-existing empty-line-after-doc-comment above `ParamsDeserializer`,
already in `HEAD` before these edits — left alone rather than folded into this diff). Windows only —
**PROB-0012 (Linux/macOS) is still open and now environmentally blocked: `wsl.exe` reports no
installed distribution and there is no docker/podman on this box**, so no Linux claim is made anywhere
in this tree. No benchmark was run and no performance claim is made: the only cost statement here is
that an unused map allocates nothing, which comes from a unit test, not a measurement. Nothing is
committed.
2026-10-21 (track 16): the HTTP/2 stage was re-reviewed against the owner's six points and one real
defect was found and closed. `http2.rs::read_body` consumed request bodies without calling
`RecvStream::flow_control().release_capacity(n)`; `h2` credits the peer only on that call, so every
upload past one receive window (64 KiB default) stalled to `request_timeout` and answered 408, while
downloads were unaffected and the suite stayed green. Fixed, with two regression tests
(`a_body_larger_than_the_receive_window_arrives_whole`,
`a_body_over_the_limit_is_413_when_the_limit_is_crossed_mid_stream`); the pre-existing 413 test could
not have caught it (16-byte limit, 64-byte body: the limit is crossed inside the first window). The
same blind spot was then found in this suite's own harness — `read_body` in `http2_e2e.rs` released
nothing for response bodies either, which is what the "failing" 256 KiB test actually was
(`UnexpectedEof` at 60 s). Rule recorded: a body test confined to one receive window is not a flow
control test. Five wire tests added for the cases no well-behaved client can produce, hand-framed:
over-cap streams reset with REFUSED_STREAM (no GOAWAY, connection survives), a header list over the
per-line budget (431), a panicking handler (500 on one stream, 200 on the next), a peer that vanishes
mid-body (registration settles at 0), an unusable `:method` token (400). Scratch example
`h2_canonical_server.rs` deleted. Suite **426 passed / 0 failed / 1 ignored** (24 h2 wire tests:
16 over TLS with ALPN `h2`, 10 over cleartext h2c, two covering both transports). Release binary re-probed with independent clients on three listeners: Node.js
`http2` **16/16** on both TLS ports (ALPN h2, ALPN http/1.1, and served with no ALPN offered),
cleartext **3/3 refused** on the `require_tls` + `enable_h2c` port, 1 MiB upload → 200, 3 MiB → 413
at 2,146,304 bytes mid-stream. Two probe corrections kept in the record: python-h2's "blocked by flow
control, window 16383" measured the probe's own accounting, and Node's `remoteSettings` mixes
client-assumed defaults. No performance claim: nothing was benchmarked this round. PROB-0013 (the
defect) recorded and closed; PROB-0014 (`max_concurrent_streams` / `max_headers_count` not reachable
from `AetherConfig`) and PROB-0015 (response side writes the whole payload in one `send_data`, so a
slow client holds it in `h2`'s send buffer, uncharged) opened as owner-decision items. `enable_h2c`
docs now state prior-knowledge-only and that it cannot open a cleartext door on a strict port; the
`enable_cleartext_http2` rename is left as an owner decision, not applied. Windows only — PROB-0012
(Linux/macOS) stays open.
A third class of claim died in this round: **my own counts**. `framework/README.md` still described
17 HTTP/2 wire tests split 12 TLS + 5 cleartext, and the matrix said 16 + 8, when `http2_e2e.rs`
holds 24 in a 16-TLS / 10-cleartext split with two tests covering both transports. Both were written
from memory of the diff rather than from the suite output; the numbers now come from pairing each
`Running …` line with its `test result` line in the log. External evidence was re-taken against a
fresh `--release` build of the final tree — 16/16 Node checks on each TLS port, 3/3 cleartext
refusals plus the permissive-port contrast on the strict port, 1 MiB upload → 200 / 3 MiB → 413 at
2,146,304 bytes, and 110 hand-framed streams → 100 answered + 10 `REFUSED_STREAM` with no GOAWAY.
`cargo clippy -p aether --all-targets` = 41 style warnings, 5 in `http2.rs`, none correctness-class;
left unfixed to keep this diff protocol-only and recorded as matrix item 16.
Scratch rule, restated because the owner deletes these on purpose: every probe/client script this
round lives **outside** the repository (`C:\tmp\` for the current round's 38 items — `strict_probe.py`,
`h2_node_client.js`, `flow_control_client.js`, `handframed_overflow.py`, `h2c_*.py`, `*_ca.pem`, the
`*.log` captures — plus `C:\Users\hamza\AppData\Local\Temp\aether_*.{py,log}` from earlier rounds), and
nothing in the record depends on them surviving: the numbers are transcribed into
`PRODUCTION_READINESS_REPORT` §10/§11 and the matrix, and §10 now carries the commands that remake a
run. `C:\Users\hamza\Desktop\nwfm\aether-probe` is the exception — it is the downstream compile check,
not scratch. Also added this round, outside the repo: `C:\tmp\bench.py`, a fixed-count keep-alive HTTP
load client (`-n`, `-c`, `--path`, `--pid`) handed to the owner to run; its ceiling is the client's, so
its rps is not server evidence — the caveat is written into ADR-001.
Review method for this track, stated because the mandate requires it: one independent agent **was**
used — an `Explore` subagent audited connection-permit and stream-registration ownership
(`app.rs` owns the permit through `AdmissionGuard`, `http2.rs` re-prices per stream) and named the
paths its read could not close, which is what put "permit release on the unscheduled semaphore path"
and "the drain-timeout abort branch" on the unverified list in `PRODUCTION_READINESS_REPORT` §11; the
review is
therefore independent for the lifecycle question and self-conducted everywhere else. Everything else
was: reading the vendored `h2` 0.4.20 source for each API touched, then running the suites, then
running the non-Rust clients. The two probe corrections above are the check on my own reasoning —
both were my conclusions that only the wire disproved.
2026-10-21 (track 15): stage 4 and its cleartext follow-up landed together — `h2` 0.4.20 bridged
onto the shared `ServiceCore` (`http2.rs`, one task per stream, per-stream memory re-pricing, GOAWAY
drain), the framing decision consolidated into `transport::Frontend` returned by `establish`,
`AetherApp::enable_h2c` for RFC 9113 §3.3 prior knowledge (off by default, refused while
`require_tls` is set), and an unbounded HTTP/2 handshake made a bounded one. `Upgrade: h2c` and
server push are declined on RFC grounds, not overlooked; a WebSocket route over h2 answers 400.
Suite **419 passed / 0 failed / 1 ignored** (17 h2 wire tests: 12 TLS + 5 cleartext), release build
clean. Independent clients: Node.js `http2` over TLS, python-h2 20/20 over cleartext, and 8/8 from a
hand-framed hpack HEADERS probe — the same probe that exposed two of my own false conclusions
(a SETTINGS list that was really the client's defaults, and a request-field check `h2` performs
before any handler, so mine was unreachable and is deleted). PROB-0010 closed by `01e79ef`. Matrix:
94/11/22/3/0/0/2 over 133 rows. Windows only — PROB-0012 (Linux/macOS) is still open.
2026-10-20 (track 14): stages 2 and 3 of the HTTP/2 + TLS front landed together — a peek-based
per-connection classifier and type-erased `Stream` (`transport.rs`), rustls termination with SNI,
live certificate reload and a bounded handshake on one port (`tls.rs`, `tls_handshake_timeout`),
ALPN deliberately advertising `http/1.1` only until the h2 bridge exists. Public API break:
`WebSocket<tokio::net::TcpStream>` → `WebSocket<aether::transport::Stream>`, documented in
ARCHITECTURE §5.1 and the feature matrix. Workspace **395 passed / 0 failed / 1 ignored** (after
restoring a `#[test]` that had been swallowed by a doc comment in `http/src/parser.rs`), release
build clean with 0 warnings, no benchmark. Matrix recount is mechanical again:
92/12/22/3/2 over 131 rows.
2026-10-20 (track 11): the token-bucket limiter and the Host allow-list became reachable features —
`AetherApp::enable_rate_limit` / `enable_host_validation`, both applied after the head parses and
before any routing, upgrade or static-file work, with 429 + non-zero `Retry-After` on a connection
that stays open and a 400 whose body never echoes the refused host. `RequestContext::peer_addr()` is
the non-breaking shape (private field + accessor). Defaults are unchanged: neither runs unless asked.
Workspace **376 passed / 0 failed / 1 ignored** (31 wire tests in `http_e2e`), release build clean
with 0 warnings, no benchmark run. The PRODUCTION_READINESS status table was recounted mechanically
from the matrix (86/11/24/3/2) because its 78/12/30 snapshot had drifted.
2026-10-20 (track 12): PROB-0006 closed. The response status line now echoes the request version at
the two sites where the head parsed (main response + the WebSocket reprice 503) — interoperability,
not legality, since RFC 9110 §9.6 permitted the old always-`HTTP/1.1` — and `max_headers_count`
defaults to 100 field lines (hyper's ceiling) instead of 64. Interim/error responses emitted before
a head parses still claim 1.1, as does a WebSocket `101`. Newly recorded: PROB-0009 (handler errors
collapse to 400 with a contradictory `E5002` body). Registry: PROB-0004, PROB-0005, PROB-0008,
PROB-0009, PROP-0001 — all owner decisions. Perf campaign stays paused — no new benchmark campaigns,
resume only for a specific unresolved technical cause.)

## Index (2026-10-27)

This file is the historical log. Tracks 16–81 are the append-only `## Last Update` section above.
Start at `.project-governance/TRACK_INDEX.md` instead: it maps every track to the file and section
that records it, names the open work, and states the rule that new tracks are recorded once, in the
register.
