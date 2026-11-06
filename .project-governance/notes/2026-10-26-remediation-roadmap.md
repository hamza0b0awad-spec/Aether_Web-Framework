# Remediation & improvement roadmap — drafted 2026-10-26

Derived from the live ledger (`problems.json`, `requirements.json`, `proposals.json` — 8 open problems,
1 requirement, 1 proposal, 0 criticisms, after track 72 closed PROB-0030), the improvement register's
D/E sections, and the standing gate readings. Each track keeps the house definition of done: reproduce
first, fix, mutate the fix and show a named test rejects it, recount the suite mechanically, record,
commit locally (no push).

## 0. The frame: the gates that are red, and stay red until they are not

At drafting: format, lints and the ledger. After track 73 slice 2 the lints row is green; after slice 3
(seven per-crate `fmt` batches, `63b6326..e7644b9`) the format row is green too. One red gate remains:
the permanent, now-ratcheted ledger line.

| gate | exact command | reading on the track-73 slice-3 tree |
| --- | --- | --- |
| format | `cargo fmt --all -- --check` | 0 hunks / 0 files, exit **0** (was 560 / 65 on slice 2 and 570 / 65 before it — the chain is in `FEATURE_COMPLETENESS_MATRIX.md` item 16) |
| lints | `cargo clippy --workspace --all-targets -- -D warnings` | 0 warnings, exit 0 (track 73 slice 2; replaces the scoped per-crate readings listed below the table) |
| ledger | `python .project-governance/tools/project_governance_bootstrap.py validate` | exit 1: 3 `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001` — permanent, and now ratcheted (track 73 slice 1) |
| ledger tests | `python -B -m unittest discover -s .project-governance/tools/tests` | 42 / 0, exit 0 (track 73 slice 1; was 37) |
| suite | `cargo test --workspace`, per-target `test result:` lines | 513 / 0 / 1 (50 lines) |

All six rows were re-executed on the slice-3 tree, plus `cargo build --workspace --examples` (exit 0).
The `fmt` half of this track also owed the docs a coordinate sweep, because reformatting moves line
numbers: see §2's slice-3 record for what was re-derived and what was left as a dated reading.

The readings the workspace `-D warnings` row replaces, kept so the replacement is checkable: 13 lib + 16
lib-test for `-p aether --all-targets`, 3 + 3 for `-p aether-router`, 9 for `http/src`, 7 for
`middleware/src`, 2 for `scheduler/src`, 0 for `-p aether-websocket --all-targets`. Every one of them was
scoped to one crate or one target, which is why none of them listed the `security` (8), `validation` (2)
or `aether-tests` (23) records that closing the workspace run found and fixed.

No track below may add a hunk or a warning of its own, and the project is not described as green while
these read red. The numbers are gate identities, not targets to be quietly redefined.

## 1. Track 72 (closed, committed with this note) — PROB-0030: a stalled WS peer costs one interval

Reproduced before fixed, at both levels. Driver level: `send_text` on a transport whose peer never
reads stays pending while `read_timeout` is 40 ms (`C:/tmp/t72/probe_a.log`). Wire level: with
`read_timeout` at 300 ms, a session parked in a write still held `active_connections=1` and
`registered_memory_bytes=33,603,584` six seconds later (`C:/tmp/t72/probe_b.log`) — the registration's
"read from the code, not reproduced" clause is now a measurement.

Fix: `WebSocketLimits::write_timeout` (`connection.rs:39`, default 10 s at `:54`, matching
`ServerLimits::write_timeout`), and `write_frame` (`:380`, routed at `:406`) hands the frame over in
pieces under `timeout_at` (`write_bounded` `:481`, `flush_bounded` `:502`) with renewal **earned** by
`MIN_PROGRESS_FOR_RENEWAL = 1024` bytes per interval (`:449`) — the PROB-0022/track-67 rule, applied
to the socket that had no bound at all. `WsError::WriteTimeout` names it.

Three tests, one per shape, and each mutant is caught by a different one:

| mutant | what it is | test that rejects it |
| --- | --- | --- |
| M1 | behaviour reverted (no deadline) | `a_peer_that_stops_consuming_costs_the_session_one_write_interval`, `a_peer_that_trickles_one_byte_per_interval_is_shed`, and the wire test (fails holding 33,603,584 B) |
| M2 | absolute per-frame cap (deadline never renews) | `a_peer_that_reads_slowly_gets_the_whole_frame` |
| M3 | renewal on any progress (no floor) | `a_peer_that_trickles_one_byte_per_interval_is_shed` |

Logs: `C:/tmp/t72/mutants.log`, `mutants_m23.log`, `mutants_m1m3.log`. Two lessons kept in the record:
a detector that **hangs** instead of failing is worthless (M2 hung until the drain loop learned to exit
on EOF), and a trickle test that stops trickling before it asserts lets M3 survive (it did, once).

## 2. Track 73 (closed 2026-10-26, three slices) — make the red gates honest rather than permanent

Order matters: ledger → lints → format.
- **`validate` — done, and not by ledger repair.** The draft of this note said the recycled rows were
  "a few lines of ledger repair"; that was wrong and the ledger says so. The three recycles live in the
  append-only `event_log.jsonl` and can only disappear by rewriting history, which the rules refuse;
  renumbering the one open item (`REQ-0001` → the genuinely free `REQ-0003`, since `REQ-0002` is a real
  closed allocation) would clear 1 of 4 lines and still leave exit 1, at the cost of falsifying 20+
  dated citations in the notes. What was actually broken is that a permanently-red check reports the
  same four lines whether nothing happened or a new ID was recycled. Closed with a ratchet instead:
  `.project-governance/tools/tests/test_recycling_ratchet.py` (5 cases) pins the standing sets by exact
  equality, recomputes them from the ledger rather than trusting the printed line, fires on two forged
  new reuses, and shows that pruning history is the only way to green the command. Track 73's first
  slice; full record in `WORKING_CONTEXT.md`'s Last Update.
- **clippy — done, in three file-locus batches, and the gate is now stricter than the debt list it
  removed.** The inventory this note started from (13 lib + 16 lib-test `aether`, 3+3 `aether-router`,
  9 `http`, 7 `middleware`, 2 `scheduler` — every one of those rows lib-or-target-scoped, so none of them
  was the workspace total) is closed: `cargo clippy --workspace --all-targets
  -- -D warnings` exits 0, which also covers the `security`, `validation` and `aether-tests` warnings
  that no per-crate row had ever listed. Where the lint asks for a change the code refuses, the refusal
  is annotated with the cost rather than silenced: `Stream`'s and `ServeOutcome`'s
  `large_enum_variant` (boxing costs an allocation per connection / per upgrade), `safe_resolve_path`'s
  `Result<_, ()>` (a named error type is a break for every caller — a track-76-shaped decision, not a
  lint one), and the router's 8-argument `search` (a per-call aggregate would not remove any state).
  The rest is fixed: let-chains, `strip_prefix`, struct literals instead of default-then-reassign, one
  `is_ok_and`/two `is_none_or`, `as_chunks::<8>`, `div_ceil`, `impl Default` in place of an inherent
  `default()` (every existing `::default()` call site keeps compiling), and a named
  `parser::ParsedRequest` alias for the `(RequestView, Option<Vec<u8>>, usize)` tuple repeated in two
  public signatures — additive, no caller changed. `fmt` came back by hand: 570 → 560 hunks, still the
  same 65 files, and rustfmt's 2024 shape for a let-chain was derived by checking each collapsed site
  against `rustfmt --edition 2024 --check` rather than by eye — a chain that *starts* with a `let` is
  split one term per line, a chain that starts with a bool expression may stay inline.
- **format — done, in seven per-crate commits, and for the same reason the ledger was ratcheted rather
  than excused.** The two honest shapes this note offered were ~8 per-crate commits or one dated sentence
  accepting the debt. The debt sentence was refused: track 73 slice 1 had just established that a
  permanently-red check carries no marginal information — it prints the same thing whether the tree got
  worse or not — and a `fmt` row deliberately left red is that same failure wearing a decision. So the
  batches went in along crate boundaries, largest-diff-file last: `63b6326` leaf crates (core, dependency,
  security, validation, runtime, benchmarks, examples), `a46f6f4` router + websocket, `dbc3b78` http,
  `aec827b` middleware, `018b5a5` `aether`, `3506038` scheduler, `e7644b9` the e2e test package. What is
  checkable about the split is its shape: `git show --name-only` on each of the seven lists `framework/`
  directories the subject line already names — one crate for batches two to seven, and the seven leaf
  crates (core, dependency, security, validation, runtime, benchmarks, examples) for batch one — so no
  commit is a repo-wide sweep and each is reviewable on its own, and the workspace gates re-executed on
  the finished tree (the §0 table) are what say the last batch did not break something an earlier one
  depended on. `cargo fmt --all -- --check` now exits **0** with zero diff blocks.
  The cost the sweep imposed is the part worth recording: reformatting moves line numbers, so every doc
  coordinate naming current code had to be re-read against the new tree. The re-derived set is
  `ACTIX_FAIR_COMPARISON.md` F1 (`tests/http_e2e.rs:1527`) and F4–F7 (scheduling `:376-383`/`:234-238`/
  `:391-397`/disarms `:240,:253,:257,:264,:269`, `QueueToken` test `:284`, `static_file` `app.rs:1121-1126`,
  `http_e2e.rs:725`, `serve.rs:158-169`, `app.rs:1853-1864`, `connection_has_token` `app.rs:2610-2622`,
  its unit test `app.rs:2965`, `read_limit` `app.rs:2183-2188`, `http_e2e.rs:770`/`:826`, WS `:168-172`,
  `app.rs:2013-2017`/`:1188-1191`, WS tests `:721`/`:734`/`:862`/`:902`/`:959`, `MIN_PROGRESS_FOR_RENEWAL`
  `:452`), `FEATURE_COMPLETENESS_MATRIX.md` (`price_response_body` `http2.rs:662`,
  `MIN_PROGRESS_FOR_RENEWAL` `http2.rs:525`), `AETHER_CORE_CAPABILITIES.md` (`parser.rs:338`),
  `ERROR_HANDLING_GUIDE_AR.md` (`app.rs:101-129`/`131-149`/`151-177`/`171-176`/`179-204`,
  `http_e2e.rs:2830`/`:2866`) and `PRODUCTION_READINESS_REPORT.md` (bodyless-`1xx` guards
  `response.rs:292-294,302-304`, `status_forbids_body` `:338`, its test `:730-748`). Coordinates inside
  dated records — the `.project-governance` track entries, the perf-baseline snapshots, the
  `git show 907dfcd^:` pins, and the rows that already say "no longer describes" — were left holding the
  coordinates of the trees they measured, which is the rule, not an oversight.

## 3. Track 74 (closed 2026-10-26) — the two memory-pricing holes that are still open (PROB-0016, PROB-0027)

Closed as written above, on the measurement it asked for: the priced container is
`EXTENSION_ENTRY_MEMORY = 160` (measured 264 B for a map's first entry and ~148 B marginal against the
shape that ships — the 128 first drafted from the *pre-pricing* shape was re-derived, see
`framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs`), the opt-in declaration shipped as
`insert_sized`/`insert_extension_sized`, the capacity-only option was refused because a parsed payload
retains 6.78–8.00x its wire bytes, and the PROB-0027 rule test covers five producers rather than three.
Landed as `d681956` (code + tests), `725a6d5` (the probe), `e2fddeb` (docs), `d1eb68d` (governance), and
re-measured on that tree: 526 / 0 / 1 over 50 result lines, clippy and `fmt` exit 0, examples build 0,
`validate` still the same four permanent findings at exit 1.

- **PROB-0016** — extension payloads ride `RequestContext` and are never reported to the budget, which
  prices connections, streams, socket buffers and bodies. Shape exists already: an absolute re-price on
  change (`AdmissionGuard::buffer_set`), charged on insert, released with the request. The hard part is
  sizing an arbitrary `TypeId` value, so the two honest options are an opt-in `ExtensionSized` trait
  (additive) or charging the map's own capacity and documenting that the values inside it are unpriced.
  Pick with a measurement of what a real payload costs, not by arguing.
- **PROB-0027** — `bytes` 1.12.1 answers `owned_is_unique` false even for the allocation's sole handle,
  so a `Bytes::from_owner` body's heap cannot be named zero-copy. No client can reach it today
  (compression, `static_dir`, embedded assets all yield uniquely-owned VEC blocks). Every close costs an
  API break, a copy on the hot path, or a rule. Recommendation: the rule — a documented "h2 response
  bodies must not wrap foreign owners" plus a test that the three real producers stay priceable. A
  per-response copy to buy an unreachable case is the wrong trade.

## 4. Track 75 (closed 2026-10-26) — stop advertising one quantity and enforcing another (PROB-0028)

Measured: chunked bodies cost ~6 wire bytes per payload byte at 1-byte chunks, so the reachable decoded
body is far under the advertised `max_body_bytes`. Track 70 settled that the *wire* bound is correct
memory pricing — the enforced number is right, the label is wrong. Close in two parts: (a) name the
enforced quantity everywhere it is advertised (`ServerLimits`, `DEPLOYMENT.md`, the `AETHER_*` key), and
state the decoded ceiling the wire bound actually permits; (b) then decide whether a framing-overhead
cap belongs in the API at all. Do not move the bound from wire to decoded bytes — that is a memory
regression dressed as a correctness fix.

Closed as (a) + (b), with the ceiling measured rather than computed on paper. The boundary is
`a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes` (`http_e2e.rs:1654`): a 62-byte
head plus one-byte chunks at six wire bytes each, so 350,879 payload bytes are 2,105,341 on the wire
and served with the whole decoded body in the handler's hands, while 350,880 are 2,105,347 and are
refused 413 — 16.7 % of the advertised 2 MiB. Falsified in both directions on the guard itself: +6
bytes there fails the served half by name, −6 fails the refused half; the file was restored and the
tree re-tested. Both quantities are now stated together at every site that advertises one:
`ServerLimits::max_body_bytes` and `AetherConfig::max_body_bytes`, the guard's own comment, the
`AETHER_MAX_BODY_BYTES` row and security bullet in `framework/README.md`, the matrix's Body-budget row,
and `docs/07` §4, both `docs/09` struct blocks, `docs/00`'s RFC-9112 items in each language, and
`docs/04`'s CL branch. `framework/docs/DEPLOYMENT.md` was checked and names no body-size number, so it
had no site to rename. (b) is a refusal, not an omission: a separate framing-overhead cap would refuse
legitimate clients for a cost the wire bound has already priced, and moving the bound to decoded bytes
would break the memory registration track 32 and track 70 built. What an operator still cannot do —
name the two budgets separately rather than only as their sum — is written into the improvement
register as an open idea, not a defect, because nothing measured shows the sum being wrong.
Re-deriving the citations around these sites found six pointing at code that no longer holds the
quoted snippet; they are fixed with this track and the rest of the class is PROB-0032 (track 80).
The `cargo doc --workspace --no-deps` run went from seven phantom links to none: four in the crates
this track touched, three in crates it did not (`benchmarks`, `runtime`, `scheduler`).

## 5. Track 76 — API coherence: two items whose cost is user-visible churn

Grouped deliberately, because both are breaking and should ship as one documented pass, not two
surprises:
- **PROB-0011** — `&mut self` route registration versus by-value builders means `AetherApp::new().get(..).tls_from_files(..)`
  cannot compile (E0507). Unifying is breaking.
- **PROB-0005** — `RequestContext` owns header lines twice (public `HashMap` + private multi store).
  True single ownership means replacing the public field with an accessor: breaking for readers. Note
  this also settles register item E2, whose performance argument is weaker than the ownership one.
- ~~**PROB-0009** (remaining half) — a handler `Err` answers 400 even for internal failures; actix/hyper
  answer 500. Status-semantics change, user-visible.~~
  **Closed 2026-10-27, and this bullet was stale before it.** D9 (2026-10-24) made the error type
  state its own status — `AetherError` answers 500 for `Io`/`DependencyCycleDetected` and 504 for
  `Timeout` — and the ledger entry was deleted on 2026-10-27 (`event_log.jsonl`), so it is not part
  of any future breaking pass. What remains 400 is a `&'static str`/`String`, by design: those are
  the handler's own words, not an internal failure.
- ~~**PROP-0001** — `aether-security` (JWT, constant-time compare, desync audit) is not reachable
  from the facade.~~ **Delivered 2026-10-28 (track 91).** `aether` depends on `aether-security`
  and re-exports `JwtValidator`, `JwtValidationOptions`, `VerifiedJwt`, `constant_time_eq` and
  `audit_request_desync`, with `aether::security` and `aether::header` module paths; the ledger
  entry was closed on 2026-10-28. The proposal's 2026-10-20 caveat that `-openapi` "stays out" is
  superseded by F-02 in this roadmap: `aether-openapi` is re-exported too, and the additive wire
  is what the owner's 2026-10-26 plan authorized.
Decision the owner owns: the release shape (one 0.2 API pass with a migration note, or leave the API
frozen). Everything after that is mine to implement.

## 6. Track 77 — verification breadth (PROB-0012) without waiting for a Linux box

Every green number here is Windows 10.0.26200 x64 MSVC; there is no WSL or container runtime on this
host. Achievable without a second machine: `cargo check --target x86_64-unknown-linux-gnu` and
`--target aarch64-apple-darwin` (type-level parity, catches `cfg` and dependency breakage), plus a
`cfg(unix)`/`cfg(windows)` audit of `transport.rs`/`tls.rs` where the platform assumptions would live.
Real runtime parity needs either a hosted CI runner or the owner's Linux machine — that is the ask.
Until one of those exists, every doc claim stays worded as "verified on Windows".

2026-10-27 reading: eleven of the thirteen library crates type-check for `x86_64-unknown-linux-gnu`;
`security` and `websocket` stop inside `ring`'s build script for want of `x86_64-linux-gnu-gcc`, the
same with `--no-default-features`, and this host still has no docker, podman or WSL (PROB-0012 carries
the command and the crate list). Nothing executed on Linux, so the wording above does not change.

## 7. Track 78 — performance: blocked on measurement, not on ideas

Still open: the p50 gap vs Actix remains unattributed (the topology claim was withdrawn after it died
against a compiled client), no rps ceiling has been measured, and the `num_cpus` default has never been
swept with the workers pinned. Register items E1–E9 stay registered-but-unauthorized until a profile
names the stage; E5 (sharding the scheduler mutex) is already refuted and must not be re-proposed.
The protocol does not change: the owner measures, release-vs-release, same session, repeated, rotated
order, with a null arm that prices the harness's own zero. Track 61's in-repo load generator and track
62's worker knob exist for exactly this. No improvement is published before that.

## 8. Track 79 — make the evidence outlive the scratch directory

Docs and governance notes cite measurements by path (`C:/tmp/t70_suite_final.log`,
`C:/tmp/t72/probe_b.log`). That directory gets deleted — by the owner, routinely, and it is not
malice. Fix is structural, not clerical: every probe that a claim rests on becomes a committed,
re-runnable artifact (a `framework/tests/probes/` binary, or the test itself with the number written
into its doc comment the way track 70 wrote a 3-of-4 flake rate into one). Where a probe cannot be
committed, the doc must carry the reading, the exact command, and the date it was taken.

## 9. Sequencing

| track | items | owner decision needed? | first evidence step |
| --- | --- | --- | --- |
| 72 | PROB-0030 | no | done — reproduced, fixed, three mutants caught |
| 73 | fmt / clippy / validate | one: repair vs accepted-debt (the `fmt` half only) | validate half ratcheted (slice 1); clippy half closed — `--workspace --all-targets -- -D warnings` exits 0 (slice 2); only the `fmt` decision is left |
| 74 | PROB-0016, PROB-0027 | no (PROB-0027 close is a rule) | done — priced on a live server, both on HTTP/1.1 and h2c; 513 -> 526 |
| 75 | PROB-0028 | no — the label was the defect, and the cap was refused on evidence | done — ceiling re-measured (350,879 served / 350,880 refused, 16.7 %), both quantities named at every advertised site, one wire test added; 526 -> 527 |
| 76 | PROB-0005/0009/0011, PROP-0001 | **yes** (three breaking) | nothing until the release shape is chosen; PROP-0001 can go now |
| 77 | PROB-0012 | **yes** (CI runner or a Linux host) | first step taken 2026-10-27 — the target installs and `cargo check --workspace --target x86_64-unknown-linux-gnu` fails only in `ring`'s build script (`x86_64-linux-gnu-gcc` missing; no gcc/clang/zig and no WSL here); `aether-http`, `aether-router`, `aether-core` and the `#[cfg(unix)]` shutdown block check clean for Linux; the whole workspace still needs the owner's host or cross compiler |
| 78 | perf, E1–E9 | **yes** (owner measures) | pinned worker sweep, task #69 |
| 79 | evidence durability | no | done 2026-10-27 — six scratch pointers in the readiness report now state method instead of a directory, the four raw logs `SUMMARY.md` already indexed are committed (12 -> 16 files), and `evidence_check.py` fails on the next absolute path that leaves the repository; see §12 |
| 80 | PROB-0032 | no | done 2026-10-27 — 36 of the 61 citations in the `docs/` human set named a span that did not hold the prose; all re-derived, and `cite_check.py` now fails on the next one; see §11 |
| 81 | PROB-0033 | no | done 2026-10-27 — baselines taken with `wait_for_registration`; three full-suite runs 527 / 0 / 1; a leaked handler guard caught by both fixed tests; see §10 |

I will take 73, 74, 75 and 79 as far as the evidence allows without asking again; 80 is done (§11);
79 is done (§12); 76, 77 and 78 are stopped on the owner's decisions and machine, not on analysis.

## 10. Track 81 (executed 2026-10-27, unplanned) — a test baseline that measured the harness's own scheduling

Not in this roadmap when it was drafted: track 75's final re-verify, run on the tree it had just
committed rather than on the tree its own readings came from, returned `TEST=101`. Two `http2_e2e`
tests sampled `registered_memory_bytes` immediately after the client handshake, but the connection's
own `CONN_BASE_MEMORY` (32 KiB) is admitted by the server's accept task — so under suite load the
sample read 0 and the settle assertion then asked whether a stream's charge had fallen to a baseline
that never included the connection. Production was correct; the test measured the harness. Both sites
now take the baseline with `wait_for_registration(&layer, CONN_BASE_MEMORY)` and assert it — the
pattern the same file already carried at `:888` — and a deliberately leaked handler guard
(`std::mem::forget`) makes both tests fail at their settle assertion (65552 vs 32768, 540856 vs
524472), so the assertions are shown to have teeth. Called 81, not 76, only because 76–80 are
reserved above: one number, one meaning. One fixed-pause wait of the same class remains in that file
(`:1531`, a `150 ms` sleep before reading the stream's charges); it is recorded as unverified rather
than rewritten on suspicion, and the file's other two `150 ms` occurrences are not this class
(`:1251` asserts on elapsed time, `:841` sleeps inside a bounded poll loop).

## 11. Track 80 (executed 2026-10-27) — the citations checked against the code they name

The plan was one sentence: read each cited span and require it to hold what the prose claims. The
measurement made it worth writing a gate rather than fixing a list. At `e81efed` the `docs/` human set
carried 61 citations over 40 distinct spans, and **36** coordinates — in 8 of the 14 documents —
named a span that did not hold the prose. Every one resolved to an existing file and an in-bounds
span, so an existence check had passed them all; `app.rs:1946-1950` pointed at another branch of the
same `conn_buf` loop, and `response.rs:287` at a date write beside the header emitter. The last seven
were found only by reading every pin: `docs/02` quoted a parse-loop excerpt at the accept arm and the
write deadline rather than at the loop it pastes, `docs/09` started an `IntoResponseError` span inside
the doc comment above it, and `node.rs:41-43` stopped one line short of the function's closing brace.

`.project-governance/tools/cite_check.py` now reads both citation forms, resolves exact path then
unique basename, and fails on nine conditions including a missing file, an ambiguous basename, an
out-of-bounds span, an unpinned span, a pin whose shape disagrees with the span, and a first or last
line that moved or was replaced. Pins are `(file, first line, last line) -> regex |
(first regex, last regex)`, 45 for 45 spans; both ends are in the key because a pin that proves one
anchor line is blind to an insertion *inside* the span — the same off-by-one that had shipped in
`docs/02` survived the first version of the gate because a 3-line span tolerates a one-line shift.
Four mutants on the real tree, a fixture that moves the span's end without touching its start, 19
unittests, and a repository test that asserts `>= 50` citations and that every pin is used, so an
emptied corpus fails instead of passing.

The notes corpus was measured too: 268 citations, 32 unresolvable (external crates, other projects,
sentence-local short forms), so it is not mechanically gate-able at a prize worth paying. The seven
coordinates PROB-0032 named were re-derived by hand and are recorded in the register; the residual is
said to be unverified rather than implied to be clean.

## 12. Track 79 (executed 2026-10-27) — evidence that survives the directory it was taken in

§8 asked for one thing: every probe a claim rests on becomes a committed, re-runnable artifact, and
where a probe cannot be committed the document carries the reading, the command and the date. Measured
first, across the whole committed tree: the two user-facing globs held 5 machine-absolute paths in 2
files; the four governance notes hold 118 mentions over 71 distinct paths and survive their own
subject because each line states its reading; `AGENTS.md` holds the `C:/tmp` isolation rule (outside
the repository by design) and `framework/tests/tests/scheduling.rs` held one doc comment pointing at a
deleted probe log. Every failing user-facing path was a `PRODUCTION_READINESS_REPORT.md` scratch
pointer — and the archive those sections rest on was itself half missing: `SUMMARY.md` indexes six raw
lock-split logs and four of them existed only in the owner's scratch directory, including
`run4_pacer_control.txt`, the record of the withdrawn first paced run. `cmp` showed the tree's `run5`
and `run6` are byte-identical to the copies beside them, so the four logs were committed (12 -> 16
files; the baseline had said 11), the index became whole, and the external path left
`BASELINE-2026-10-20-SCHED-ROTATION.md` with its count corrected to 16.

The report now states method where it used to state a directory: `cargo test --workspace` and a date
at `:431`/`:441`, an explicit "the log is not preserved, here is what the number stands on" at `:449`,
portable notation for the temp directory at `:765-767`, and `aether-probe` named as a sibling
directory at `:781`. The four `file:///c/Users/...` links stay because they resolve inside the
repository, which is the only test that matters.

`.project-governance/tools/evidence_check.py` enforces that test: a machine-absolute path in a
user-facing document must resolve inside the repository, over `docs/**/*.md` and
`framework/docs/**/*.md`; `~`, `$TMPDIR` and `%TEMP%` are exempt because they resolve per reader, and
the rule fails closed — a legitimately external path is a finding whose fix is portable notation or a
reasoned exemption here. 16 unittests pin the refusals; the first version read every `http://` as
drive `p:`, missed that `file:///etc/x` was being read as the *relative* path `etc/x` (so every
POSIX-spelled link passed as inside), and is falsified against the tree it was written for: on
`3c838e3`'s readiness report it returns 6 findings at `:431`, `:441`, `:449`, `:766` (twice) and
`:781`, none missed. Two lines outside the globs were fixed by hand — the `scheduling.rs` doc comment
that cited a deleted probe log, and `code-hashes-2026-10-19.txt`, which listed `response.rs` twice
with two hashes and one absolute spelling; three of its six hashes reproduce from `1116e1a`, the
first commit, while `app.rs` and both response.rs lines were taken on a working tree that changed
before that commit, and the file carries a dated header saying which pair disagrees with itself.

An independent reviewer read the two commits (read-only, one verdict: SHOULD-FIX) and refuted two
numbers in the first draft of this section: the hash archaeology above — my scan had iterated the
table's short paths instead of the full ones, so every lookup missed, and a second scan tested only
CRLF variants — and the archive count, since `git ls-tree 3c838e3` shows 12 entries, not 11. It also
showed the POSIX test passed under the old `as_path`, because the finding echoes the token the
assertion searched for. All three are corrected, and both tests now fail under the old code (2
failures under the old `as_path`, 7 fixture tests under the old regex) instead of merely counting.

The same pass took the cheap half of PROB-0012: the Linux target installs and
`cargo check --workspace --target x86_64-unknown-linux-gnu` fails only in `ring`'s build script
(`x86_64-linux-gnu-gcc` absent; no gcc/clang/zig and no WSL on this box), while `aether-http`,
`aether-router`, `aether-core` and the one `#[cfg(unix)]` shutdown block check clean for Linux. A
suspected shadowing defect in `use tokio::signal::unix::signal` was refuted by compiling, so it is not
in the record as a defect. The full workspace still waits on the owner's Linux host or cross C
toolchain.

## 13. Track 82 (executed 2026-10-27, governance) — one command, and a red line that cannot grow

§0's table states the gates; from track 82 one command runs them: `bash gates.sh` at the repository
root. It prints one line per gate and exits non-zero if any failed. `validate` is the exception that
needed a decision, not a workaround: its exit 1 is pinned to exactly the four standing findings, the
same set the recycling ratchet test guards, so a fifth finding fails the run and a missing one fails
it too. Register: `2026-10-21-improvement-register.md`, `## Track 82`. The four earlier hygiene
commits of the same pass (map, PROB-0009 retitle, track index) are recorded there as well. Nothing in
this track changes what any gate checks; it changes who has to remember to run them.

## 14. Track 83 (executed 2026-10-27, framework) — the dropped 5xx detail now has somewhere to go

PROB-0009's residual is closed: a 5xx still redacts its message from the body, but the message is
handed to an optional process-wide reporter (`aether::set_error_reporter`, `ErrorReport { request_id,
status, code, detail }`) under the same `X-Request-ID` the client receives. The request serial is a
task-local inside the spawned handler task, so the success path allocates no id string. A user error
type opts in through `redacted_detail`; a 4xx is never reported, and a 5xx that publishes its detail
is not reported twice. One new integration target (`tests/tests/error_reporting.rs`) and one
doctest take the suite to 529 / 0 / 1 over 51 `test result:` lines. Register:
`2026-10-21-improvement-register.md`, `## Track 83`. Commits `a0c1b94` (framework) and `1f01275`
(docs, which also re-derived 30 citations after the code moved lines). Nothing in this track changes
a protocol or adds a wire field; it changes where an operator can look when a 500 has no body.

## 15. Track 84 (executed 2026-10-27, framework) — the receive deadline stands on its own

Every request used to be received under `max_request_time`, the same number that bounds handler
execution, so a slow-but-legitimate upload could only be admitted by giving every handler the same
extension. The receive phase (HTTP/1 head and body, HTTP/2 body) is now bounded by
`max_receive_time` / `AETHER_MAX_RECEIVE_TIME_SECS`, and `max_request_time` bounds only the handler.
Defaults are unchanged (30 s each), so no deployed behaviour moves unless an operator sets the new
knob. Two wire tests pin the independence in both directions, and the HTTP/2 stalled-body test moved
to the new setting. Register: `2026-10-21-improvement-register.md`, `## Track 84`. The suite reads
530 / 0 / 1 over 51 `test result:` lines. Nothing pushed.

## 16. Track 85 (executed 2026-10-27, framework) — a handler stops when its client is gone

The matrix's remaining-work item 3. Dispatch parks the connection task inside
`timeout(request_timeout, join_handle)`, so a client that walked away mid-handler was learned only
at write time and the handler ran to completion for a socket nobody was reading. The fix is a
`PeerWatch` arm raced against the handler in a `biased` select: when it resolves, the handler is
aborted exactly as the deadline aborts it and the core returns `ServeOutcome::ClientGone`, which
the frontend turns into a close with no bytes. HTTP/1 reads ahead through the transport (the only
observation that works for TLS) into the connection's own buffer, and treats only a read **error**
as departure — an RST on plaintext, rustls's end-without-`close_notify` on TLS; a clean EOF is a
half-close that the framework must still answer, so it settles to `Pending` and the write path
remains the detector. HTTP/2 watches `SendResponse::poll_reset`, which covers a per-stream
`RST_STREAM` and the failure h2 reports for every stream when the connection dies. Six wire tests
pin the cancelling direction in all three protocols and the not-gone direction (pipelined request,
connected keep-alive client, `close_notify` half-close); three mutations (watch never fires, EOF
treated as gone, `ResetWatch` never fires) each fail the intended tests and were reverted.
Register: `2026-10-21-improvement-register.md`, `## Track 85`. Nothing pushed.
The suite reads 536 / 0 / 1 over 51 `test result:` lines.

## 17. Track 86 (executed 2026-10-27, framework) — a server that is going away says so

The matrix's remaining-work item 9. `AetherApp::shutdown` dropped every WebSocket session on the
floor: the connection task sat inside `WebSocket::recv`, so a client saw the TCP close with no
closing handshake, which RFC 6455 §7.4.1.1 has a status code for — 1001, "going away". The
transport now owns the signal rather than the runtime: `WebSocket::from_parts_with_shutdown` takes
the `watch::Receiver<bool>` the app already uses for HTTP/1 and HTTP/2 drains, and `recv` races it
(`biased`) against the pending read. The receiver is pre-checked with `borrow_and_update` because a
clone made after the flag flipped inherits the seen version and `changed()` alone would go quiet
forever — a mutation that removed the pre-check hung the upgraded-during-shutdown test, which is
how that hazard was found and pinned. Dispatch gives the session a bounded grace
(`WS_SHUTDOWN_GRACE`, 1 s) inside `tokio::time::timeout`: a session that returns within it has
completed its closing handshake and the connection guard is marked complete, while one that is
stuck in a handler is aborted exactly as before, so the HTTP drain's "grace then cancel" shape now
covers WebSockets too. Four alternatives were rejected on evidence (writing the frame from the
connection task, which no longer owns the transport; keeping the abort and documenting it; waiting
for the handler without a bound, which one stuck handler would turn into a hung shutdown; reusing
`write_timeout` as the grace, three orders of magnitude too long and measuring something else), and
four mutations were run — three decisive, one (abort immediately, no grace) honestly recorded as
not decisive because the wire test cannot distinguish it. The code insertion above the connection loop
moved 22 pinned `app.rs` coordinates; all 28 inline citations in six root `docs/` files were
re-derived, so `cite_check.py` reads 66 citations over 45 spans and 0 findings. Register:
`2026-10-21-improvement-register.md`, `## Track 86`. The suite reads 540 / 0 / 1 over 51
`test result:` lines (308 unit + 226 integration + 6 doc). Nothing pushed.

## 18. Track 87 (executed 2026-10-27, docs) — the error contract exists; the papers said it did not

The readiness report's §9 item 1 and register row E9 both still presented "typed error mapping"
(`IntoResponse for AetherError`, 500 for internal errors) as open, six days after D9 delivered the
contract as a local `IntoResponseError` trait with a blanket `Result` impl and two wire tests. Both
now say delivered and name the shape the code actually has: the trait at `app.rs:121-159`, the
message impls at `:161-179`, `AetherError`'s statuses at `:181-214`, the response at `:216-249`,
and the reason the bound is a local trait rather than `IntoResponse` — the latter was measured to
answer an `Err` with 200, because the same blanket serves `&'static str`.

Verifying the claim against the source also found 15 dead coordinates in the documents that teach
the error contract: 8 in `ERROR_HANDLING_GUIDE_AR.md` (six `app.rs` spans and the two `http_e2e`
test lines, which had been dead since before track 86), 2 in `FULL_FRAMEWORK_DOCUMENTATION_AR.md`,
4 in the matrix's two error rows, and 1 in the readiness report's wire-budget link. All are
re-derived from the code; historical documents that declare themselves snapshots are untouched, and
the measured coverage boundary of `cite_check.py` (root `docs/` only: 66 citations / 45 spans
against 67 citations in `framework/docs/`, 21 of which sit in unpinned teaching documents) is
recorded with the follow-up to extend the glob to that teaching subset. Register:
`2026-10-21-improvement-register.md`, `## Track 87`. No code or test changed, so the suite still
reads 540 / 0 / 1. Nothing pushed.

## 19. The open-problem inventory and the sequence after track 87 (plan, 2026-10-27)

The owner asked for every open problem in one place and a long plan (`اتخذ قررار وصلح كل مشاكل
اعمل خطة طويلة`, 2026-10-27). This section is that inventory. It is not itself a track: each
group below becomes its own numbered track when executed, and the plan is rewritten rather than
annotated when a reading here turns out wrong.

### 19.1 What was read, and what each source says on the 540 / 0 / 1 tree

| source | reading |
| --- | --- |
| `.project-governance/problems.json` | **4 open**: PROB-0004 (scheduler tables are historical), PROB-0005 (two header stores), PROB-0011 (builders cannot chain), PROB-0012 (Windows-only verification). |
| `proposals.json` | **1 open**: PROP-0001 (wire `aether-security` into the facade). Additive, so the 2026-10-25 decision policy authorizes it. |
| `criticisms.json`, `requirements.json` | 0 criticisms; REQ-0001 stays open by design — it is the `[OPEN VS HISTORY]` line the `validate` gate pins. |
| `FEATURE_COMPLETENESS_MATRIX.md` | **136 rows** (recounted mechanically 2026-10-30 after track 107): 121 `VERIFIED_IMPLEMENTED`, 4 `PARTIALLY_IMPLEMENTED`, 9 `NOT_IMPLEMENTED`, 0 `BROKEN`, 0 `UNVERIFIED`, 0 `UNWIRED_IMPLEMENTATION`, 2 `NOT_APPLICABLE`. The 2026-10-26 snapshot this row first carried (134 rows / 100 / 10 / 19 / 3 unwired) was stale twice over: F-01/F-02 wiring closed the three unwired rows, and tracks 94/95 flipped three partial rows. The 6/19 split this row carried after tracks 94/95 was itself off by one — the same rule run on that tree gives 108 / 7 / 18 / 2 — and track 96 flipped two of the seven (the generic `OPTIONS` answer and `CSP`) to 110 / 5 / 18, which is also what the file itself read then: the *Status count* section of the matrix lagged at 108 / 6 / 19 until track 97 corrected it. Track 97 moved *Directory listing* and *Pre-compressed `.gz`/`.br` assets* out of `NOT_IMPLEMENTED`, so `NOT_IMPLEMENTED` 18 → 16 and `VERIFIED_IMPLEMENTED` 110 → 112. Tracks 98–100 flipped six more (`CSRF`, `Authentication middleware`, `Trusted-proxy client identity`, `Response body streaming`, `Static-file sendfile`, `Streaming request bodies` at `PARTIALLY_IMPLEMENTED`), and track 100b flipped the streaming-request row, so the count above is one run of the PRR §3 command. Track 101 then flipped *OCSP stapling / revocation* from `PARTIALLY_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`, and the four partials that remain are *OpenAPI generation*, *README / docs accuracy*, *Zero-copy head parsing* and *SIMD-style validation*. Track 102 flipped *Property-based tests* (`NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`) and replaced *Fuzz target*'s "compiles" evidence with a sustained campaign, so the count above is one run of the PRR §3 command on the track-102 tree: **120 / 4 / 10 / 0 / 0 / 0 / 2 = 136**. Track 103 left every status where it was — the answer to *Secret management* is a documented non-goal, so that row keeps `NOT_IMPLEMENTED` and only its evidence cell changed — and a re-run of the same rule on the track-103 tree gives the same **120 / 4 / 10 / 0 / 0 / 0 / 2 = 136**. Tracks 104–106 moved no status row — the readiness report's own checking, the HTTP/1 request-target set and its HTTP/2 half are evidence and conformance work, not matrix features — and track 107 flipped *Nested routers / independent sub-apps* from `NOT_IMPLEMENTED` to `VERIFIED_IMPLEMENTED` (121 / 4 / 9). Track 108 flipped *OpenAPI generation* from `PARTIALLY_IMPLEMENTED` — the row F-02 left reachable-but-manual — to `VERIFIED_IMPLEMENTED`, so the figures above are one run of the PRR §3 command on the track-108 tree: **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136**, and the three partials that remain are *README / docs accuracy*, *Zero-copy head parsing* and *SIMD-style validation*. |
| readiness report §9 | Items 1–6 delivered (3 on both frontends in track 100b; 6 over TLS/ALPN and cleartext). Item 8 (OCSP/CRL) is delivered in track 101 — a CRL and an OCSP staple are enforced at load, nothing is fetched. Item 10 (fuzz) is delivered in track 102 — a seeded runner over a committed corpus, 15 843 328 executions in 120 s with 0 crashes. Open: 7 (permessage-deflate — subprotocol negotiation delivered in track 95) and 9 (performance). |
| register | A1–A6 and B1–B21 carry no unresolved *defect*; D1–D4 are the open, non-defect gaps; E1–E9 are registered and unauthorized; the five minor findings after track 69 are still open edges. |
| code markers | `grep -rn "TODO\|FIXME\|XXX\|HACK" framework --include=*.rs` → **0**. There is no ignored code-level debt marker hidden from the ledger. |

### 19.2 The classification rule

- **Defect** — a claim the repository makes that is false, or behaviour the framework itself can
  produce that is wrong on the wire. Fixed immediately; never queued behind a decision.
- **Parity gap** — a capability Axum/Actix have and Aether does not. Sequenced: each is a new
  surface with its own limits, tests and documentation, so it gets its own track.
- **Decision** — belongs to the owner: material scope, irreversible risk, or a personal
  preference the code cannot settle.
- **Environment** — cannot be closed on this host (no Linux, no container runtime, no owner-run
  measurement).
- **Non-goal** — a stated, reasoned refusal. It stays a non-goal until the reason is falsified.

### 19.3 Defects — every one is scheduled, none is left as "known"

| id | defect | evidence / where | fix | track |
| --- | --- | --- | --- | --- |
| D-01 | Matrix says `AetherApp` "exposes no enable method" for access logging | the *Access logging* and *Request logging* rows; `AetherApp::enable_request_logging` exists and the response post-process reads `logger.enabled` | rewrite both rows to name the method and its default-off choice | 88 |
| D-02 | Matrix status table counts are stale | table read 99 / 10 / 17 / 0 / 0 / 3 / 2 = 131; the same file counted by the stated method was 100 / 10 / 19 / 0 / 0 / 3 / 2 = 134 before track 88's row changes | re-derive the table and the prose beside it | 88 |
| D-03 | Roadmap §5 still lists PROB-0009's "remaining half" as open and breaking | `event_log.jsonl` closes PROB-0009 on 2026-10-27; D9 delivered the status-class decision | mark the bullet closed and point at D9 | 88 |
| D-04 | Register D2b says the WS graceful-close question is "still open" | track 86 delivered a `1001` close on shutdown with wire tests | date the sentence closed by track 86 | 88 |
| D-05 | Register D3 lists PROB-0009 among "owner decisions still unmade" | D5a records D9 superseding it; D3 also still named PROB-0016 and PROB-0019, both closed | rewrite D3 against the ledger | 88 |
| D-06 | Register B21's hazard is stated as standing | the workspace clippy gate is green *and* workspace-wide since track 73 slice 2; a green `--workspace --all-targets` cannot be vacuously clean the way a command that aborted in another crate was | close B21, and record that a cached `Finished` line is freshness, not a missing crate | 88 |
| D-07 | A user `IntoResponseError` can put a non-error status on the wire | the blanket `Result` impl calls `err.error_status()` and `canonical_reason(status)` without a range check, so `42` writes a malformed status line and `200` serves an `Err` as success | one boundary: a status outside `400..=599` answers `500`, documented on the trait | 89 |
| D-08 | `post_process` pushes `X-Request-ID` unconditionally | `serve.rs` pushes the field even when the response already carries one, so the header appears twice | the framework's id wins: remove any existing entry, then push one | 89 |
| D-09 | A lone `1xx` other than `101` is served as a bare interim line | the `101` refusal is scoped to `101`; a handler's `103` is written with no final response, so the client waits until its own timeout | refuse any 1xx at the response exit with `500`, since the frontend synthesizes none other than the handoff `101` | 89 |
| D-10 | Two reason phrases for 413 in one framework | three hand-written `Payload Too Large` sites against `canonical_reason(413) = "Content Too Large"` (RFC 9110's name), and both languages of `docs/00` name the old phrase | use the canonical phrase everywhere and update the two doc lines | 89 |
| D-11 | `aether-runtime::handle_connection` cannot serve a split or pipelined request | `worker.rs` parses each read standalone and discards `_consumed`; `aether` depends on the crate but nothing calls it | loop on `consumed`, keep the buffer across reads, add the crate's first tests | 90 |
| D-12 | `cite_check.py` covers root `docs/` only | 21 citations in 5 teaching documents under `framework/docs/` are unpinned, and a document has no way to declare itself a dated snapshot | extend the glob, add a snapshot marker, pin or declare every new citation | 92 — done 2026-10-28: `framework/docs/**/*.md` is inside the gate (93 citations / 66 spans / 15 live documents), 10 coordinates corrected, and the 9 dated records carry a reviewed `cite-check: snapshot` marker (61 citations reported but not validated) |
| D-13 | The `scheduling` test binary has no serial guard | two recorded full-binary failures at 48.65 s vs 2.04–2.27 s for the same binary under suite load; re-runs green | a process-wide serial guard inside that test binary, not larger timeouts | 93 — done 2026-10-29: `SERVER_TURN` serialises the 17 server lifetimes (all bounds unchanged); a 24× host slowdown against the stress test's 16–17× queue-wait margin is the mechanism the 503s came from, and it did not reproduce this sitting in five load shapes (2 copies, 8 CPU hogs, a sibling `http_e2e`, all 18 tests at once, 3 copies) — 18 / 18 green each time |
| D-14 | Register minor-finding coordinates shifted with tracks 84–86 | `serve.rs:508` is now `:634`; `app.rs:201` is now `:229` | replace them with symbol anchors and state the direction of the shift | 88 |
| D-15 | The *Nested routers* row contradicts the *Per-route middleware* row | it said "no nested state or middleware" while group-scoped middleware is delivered | re-word to "prefix grouping and group middleware; no nested state or mountable sub-app" and keep the status | 88 |
| D-16 | The *README / docs accuracy* row is a standing "partial" with no measurement | "legacy reports still contain stale claims" | measure it: name the remaining stale documents and why they stay, or flip the row | 88 |
| D-17 | `aether-security`'s desync audit is absent from the matrix | the crate exports `smuggling::audit_request_desync`, and only JWT + constant-time had rows | add the row with `UNWIRED_IMPLEMENTATION` until track 91 | 88 |
| D-18 | `AetherConfig::x_request_id` is declared, documented and never read | `grep` found the field only in `framework/aether/src/config.rs`: setting it `false` changed nothing, while its doc claimed the header was under the app's control | carry the flag into `ServiceCore` and gate the header in `post_process`; correct the doc — disabling drops the field, not the id | 89 |
| D-19 | The OpenAPI generator emits keys no consumer reads | `Operation::operation_id` serialised as `"operation_id"`, but the OpenAPI 3.1 field is `operationId`; nothing had ever inspected the output because no shipped crate depended on the builder | rename the serde key, pin the document's field names in the crate, and let the facade test read the generated JSON | 91 |

**Status 2026-10-27:** track 89 closed D-07, D-08, D-09, D-10 and D-18, each behind a
failing-first wire test (the 413 phrase also has an `http2` unit test). D-18 was not in the
inventory; it surfaced while fixing D-08 and is recorded rather than quietly fixed, because a
documented setting that does nothing is the same class of defect as the rows above. Every other
D-row is still open.

**Track 90, same day:** D-11 is closed. Two tests failed on the old loop first — a pipelined pair
produced one answer, a split request produced none — and the loop now accumulates, serves every
complete request in the buffer, advances by `consumed`, and answers a framing error instead of
dropping the socket. The crate is repaired but still uncalled: whether `aether` should depend on
it is D6, which stays an owner decision about the publish payload.

**Track 91, 2026-10-28:** F-01 and F-02 are delivered and D-19 is closed. A `facade_wiring` test
that imports only `aether::` did not compile against the old facade — six unresolved paths — and
compiling it against the new one read the generator's own JSON, where `operationId` came out as
`operation_id`. D-19 was not in the inventory; the wiring is what made the output observable, and
the defect is recorded rather than quietly fixed. The four `UNWIRED_IMPLEMENTATION` matrix rows are
now three `VERIFIED_IMPLEMENTED` and one `PARTIALLY_IMPLEMENTED` (OpenAPI is reachable, still a
manual builder). PROP-0001 is closed in the ledger.

### 19.4 Parity gaps — sequenced, with the dependency that forces the order

| id | gap | matrix / PRR | depends on | track |
| --- | --- | --- | --- | --- |
| F-01 | `aether-security` reachable through the facade (`JwtValidator`, `constant_time_eq`, desync audit) | PROP-0001; the *JWT validation*, *Constant-time comparison* and *Request desync audit* rows | — | 91 |
| F-02 | `aether-openapi` reachable, or explicitly retired from the facade | the *OpenAPI generation* row | — | 91 |
| F-03 | Per-route middleware/guard API | the *Per-route middleware/guards* and *Per-route middleware* rows; PRR 4 | — | 94 — done 2026-10-29: `route_sync_with`, `route_async_with`, `ws_with` bind a chain to one registration; five `user_middleware` and two `http_e2e` wire tests plus a doctest, and both rows are `VERIFIED_IMPLEMENTED` |
| F-04 | WebSocket subprotocol negotiation | the *Subprotocol negotiation* row; PRR 7 | — | 95 — done 2026-10-29: `ws_subprotocols` on the app and the group, `select_subprotocol` (RFC 6455 §4.2.2), the echoed `Sec-WebSocket-Protocol` line, `400` on an unmatched offer; four handshake unit tests and five `http_e2e` wire tests, and the row is `VERIFIED_IMPLEMENTED` |
| F-05 | Generic OPTIONS responder | the *Automatic OPTIONS / CORS preflight* row | — | 96 — done 2026-10-29: a routable path answers `OPTIONS` with `204` + `Allow` computed from what the target really serves (`allow_header_for_target`), a registered route still wins, and the `405` arm shares the same computation; four `http_e2e` wire tests and one `trie.rs` unit test, and the row is `VERIFIED_IMPLEMENTED` |
| F-06 | CSP nonce/hash generation | the *CSP* row | — | 96 — done 2026-10-29: `aether_security::CspNonce` (16 bytes from the OS RNG) and `csp_hash_sha256`, a `{nonce}` placeholder the middleware substitutes per response, the same token in the request extensions, and removal-not-empty when no nonce is available; seven `csp.rs`, six `security.rs` and two `http_e2e` tests plus one doctest, and the row is `VERIFIED_IMPLEMENTED` |
| F-07 | Opt-in directory listing | the *Directory listing* row | — | 97 — done 2026-10-29: `StaticDirOptions::directory_listing` on `static_dir_with_options` (default off, a directory still answers `404`), an `index.html` still winning over the listing, the tree's own root reachable through a prefix route because the catch-all needs one character, a `301` to the slash-terminated form, names filtered to what the resolver would serve and HTML/href-escaped, dotfiles/symlinks/non-UTF-8 skipped and the `.br`/`.gz` siblings folded into their base, 10 000-entry truncation, `Cache-Control: no-cache`; two `http_e2e` wire tests with the traversal controls the track's first step demanded, plus six `static_dir` unit tests, and the row is `VERIFIED_IMPLEMENTED` |
| F-08 | Pre-compressed `.gz`/`.br` variant selection | the *Pre-compressed `.gz`/`.br` assets* row | — | 97 — done 2026-10-29: `StaticDirOptions::precompressed` selects `name.br`/`name.gz` through the middleware's existing `select_encoding` authority (q-values, wildcard, tie to Brotli) with a new `accepts_identity` companion, names the coding in `Content-Encoding`, keeps the base file's MIME type, computes the `ETag` over the bytes actually sent, adds `Vary: Accept-Encoding` to every answer including a `304` and the `406` refusal, and re-checks the sibling's containment; one `http_e2e` wire test and one `accepts_identity` unit test, and the row is `VERIFIED_IMPLEMENTED` |
| F-09 | Streaming response bodies and chunked responses | the *Response body streaming* and *Bounded response memory* rows; PRR 3; E8 | — | 99 — done 2026-10-29: `Response.stream` with the pull-based `BodySource`/`BodyStream` (`Response::stream`, `stream_with_length`, `BodyStream::from_chunks`), `BodyFraming` decided once from the head (chunked on 1.1, close-delimited on 1.0, none for 1xx/204/304), DATA frames on h2 with `END_STREAM`, a failed body ending the connection or resetting the stream without a terminator, `MAX_EMPTY_CHUNKS = 1024`, per-chunk pricing on both emitters and compression declining a stream; eleven `streaming` wire tests, five `response` unit tests, three `http2_e2e` tests, and both rows are `VERIFIED_IMPLEMENTED` |
| F-10 | Static-file `sendfile`/streamed file answer | the *Streaming / `sendfile`* row | F-09 | 99 — done 2026-10-29: `plan_file` decides status, length, range and validator from metadata (a HEAD, `304` or `416` never opens the payload), `open_file` reads 64 KiB `FileBody` chunks at the range offset, the validator is the weak `W/"<len>-<mtime>"` metadata tag with weak `If-None-Match` and an `If-Range` that takes the published date but never the weak tag; three `static_files` and one `static_dir` unit tests plus three `streaming` wire tests, and the row is `VERIFIED_IMPLEMENTED` (a read loop, not a literal `sendfile(2)`) |
| F-11 | Streaming request bodies | the *Streaming request bodies* row; PRR 3 | — | 100 + 100b — **done on both frontends 2026-10-29/30**: `route_stream` dispatches from the head on either protocol, `read_body`/`body_stream` pull from a one-slot lane (`body_pump` over HTTP/1.1, `Http2Body` over `h2`), `max_stream_body_bytes` refuses a declared oversize with `413` before dispatch and fails a mid-stream one on the read, `receive_timeout` bounds the body, an unfinished HTTP/1.1 body closes the connection after the answer while an unfinished h2 body is abandoned by dropping the `RecvStream` and costs one stream, and the registration is re-priced per chunk on both; eight HTTP/1.1 wire tests + six h2 wire tests + five lane unit tests, the row is `VERIFIED_IMPLEMENTED` (track 100b) |
| F-12 | CSRF middleware | the *CSRF protection* row; PRR 8 | F-01 | 98 |
| F-13 | Authentication middleware over the existing JWT validator | the *Authentication middleware* row; PRR 8 | F-01 | 98 |
| F-14 | Forwarded-client identity for rate limiting behind a proxy | PRR 8 | a documented trusted-proxy allow-list (mine, security-sensitive) | 98 |
| F-15 | OCSP/CRL enforcement | the *OCSP stapling / revocation* row; PRR 8 | an outbound fetch + refresh task; offline builds must still work | 101 — done 2026-10-30: `TlsCertSet::with_crl` refuses a serial a signed, current CRL lists and `with_ocsp` refuses a staple that did not answer successfully, does not cover this chain, is outside its window, or is not signed by the issuer or by a responder it authorised; every field is tag-checked and the time fields must be GeneralizedTime; 15 revocation unit tests (six CRL, nine OCSP), 13 sensitivity mutants each rejected by a named test, the matrix row is `VERIFIED_IMPLEMENTED`; nothing is fetched — a refresh is a new load/reload call |
| F-16 | Property-based tests for the parser and router | the *Property-based tests* row | — | 102 — **done 2026-10-30**: `proptest` 1.12 as a dev-dependency of `aether-http` and `aether-router`; four parser properties (arbitrary bytes never panic any entry point; a generated request round-trips method, target, query, headers, body and exact consumed length through both `RequestView::parse` and `parse_request`; every proper prefix is `Ok(None)`; a chunked body decodes identically however its wire is split) and four router properties (a static route matches itself; declared `{names}` survive a shared parameter level; a literal beats a parameter sibling; an unserved method is 405 with the exact union and an absent path is 404); eleven targeted mutants, each killed by a named property after the first run showed that only one parser entry point was being driven — the row is `VERIFIED_IMPLEMENTED` |
| F-17 | A sustained fuzz run with the result recorded in-repo | PRR 10 | a corpus and a runner that exist on Windows | 102 — **done 2026-10-30**: a deterministic seeded runner in `framework/fuzz` over 12 committed seeds (no `cargo-fuzz`, no nightly, no sanitizer — none exists on this Windows host), with outcome-bucket guidance, crash artifacts under `fuzz/artifacts/` and a non-zero exit on a crash; a release run at seed 20261030 made **15 843 328 executions in 120.011 s** (corpus 12 → 719, 717 outcome buckets) with 0 crashes, and a deliberately injected chunk-boundary panic was caught at execution 6 507 — the record lives here, in the matrix row and in the crate's module docs |
| F-18 | Secret handling beyond environment variables | the *Secret management* row | decision inside the track: a redacting wrapper vs a documented non-goal | 103 — **done 2026-10-30**: a documented non-goal, not a wrapper. Secrets stay constructor arguments and never fields of `AetherConfig`; what the track adds is an enforced no-render guarantee — no `Debug` on `JwtValidator`/`BearerAuth`/`CsrfTokens` (three `compile_fail,E0277` doc tests), a redacted `Debug` on `CsrfToken`, a refusal that carries no credential, and a TLS `Debug` pinned by a test that searches the rendering for key-unique windows in PEM text, DER hex and decimal byte form. The row stays `NOT_IMPLEMENTED` by decision; its evidence cell states the guarantee. |
| F-19 | Request arena / zero-copy head / SIMD validation in the hot path | the *Zero-copy head parsing*, *Request arena* and *SIMD-style validation* rows; E1–E3, E6 | a profile naming the stage (owner measures) | blocked |
| F-20 | Thread-per-core placement | the *Thread-per-core runtime* row | the worker sweep, task #69 (owner measures) | blocked |
| F-21 | Signed cookie values: the documents read a cookie as a session source while nothing in the tree could say the server wrote it | the *Cookies* row; PRR remaining-work 7 | F-01 (the signer lives in the crate track 91 wired into the facade) | 114 - **done 2026-11-02**: `aether_security::CookieSigner` issues `base64url(payload).base64url(HMAC-SHA256)` with the cookie name bound into the MAC input, `RequestContext::signed_cookie` separates `Present` from `Absent`, `Invalid` and `Ambiguous` (a name carried twice is refused, never resolved by taking one), and 15 unit + 11 wire + 2 doctests + 1 `compile_fail` pin it. The row's other half - confidentiality - stays open by decision: the payload is base64url and the browser reads it |

### 19.5 Blocked, and exactly what unblocks it

| id | blocked on | unblocked by |
| --- | --- | --- |
| PROB-0005 | an owner decision: replacing the public `headers: HashMap` with an accessor is breaking | the release-shape decision below |
| PROB-0011 | the same: unifying `&mut self` registration with by-value builders is breaking | the release-shape decision below |
| release shape | owner: one 0.2 API pass with a migration note, or the API stays frozen | an owner ruling |
| PROB-0004 | owner paused the scheduler comparison sweep | the owner resuming it with pre-declared confirm/refute criteria |
| PROB-0012 | no Linux/macOS host, no WSL/container runtime here | an owner CI runner or a Linux machine; until then every claim stays worded "verified on Windows" |
| PRR 9 / E1–E9 | no profile names the stage, and the p50 gap vs Actix is unattributed | the owner's measurement protocol (release-vs-release, same session, repeats, rotated order, null arm) |
| permessage-deflate | owner-deferred (PRR 7) | an owner ruling; the going-away close half of PRR 7 is delivered (track 86) |
| the *HTTP/3* row | no QUIC engine exists in this workspace | a scope decision, not a patch |

### 19.6 Non-goals — kept as refusals, with the reason that keeps them

| matrix row | refusal | reason |
| --- | --- | --- |
| *HTTP/2 upgrade over cleartext (`Upgrade: h2c`)* | not implemented, not planned | RFC 9113 §3.1 deprecates the token; RFC 7540 §3.2 needs an implicitly half-closed stream 1 the `h2` server does not synthesize; prior-knowledge h2c serves those clients |
| *HTTP/3* | not implemented | no QUIC connection engine here, and Axum only has one through ecosystem crates |
| *Handler macros* | not implemented | closures with typed extractors are the design; a macro layer would be new public surface with no measured benefit |
| *Hot reload* | not implemented | a dev-server concern; nothing in the runtime model needs it |
| *Secret management* (a `Secret<T>` wrapper) | refused | the framework's own secret-holding types are already opaque and its render paths are pinned (track 103); a wrapper would only be adopted voluntarily, so it adds public API without adding a guarantee, and redacting an application's own secret is the application's job |
| per-request `eprintln!` for a refused `101` (register A6) | refused | it is an unbounded, client-influenced stderr write; `LoggerMiddleware` already records the status |

### 19.7 The order

Tracks are numbered from 88 because 76–78 are reserved and unexecuted; 88–103 were the free numbers when this table was written, and every one of them plus 104–109 below has since been executed.

| track | scope | owner decision? | first step |
| --- | --- | --- | --- |
| 88 | D-01…D-06, D-14…D-17 (docs honesty) | no | count, then correct each row against the source |
| 89 | D-07…D-10, D-18 (response-exit hygiene) | no | write the four failing cases first |
| 90 | D-11 (`aether-runtime` loop) | no | split a request across two reads and watch it fail |
| 91 | F-01, F-02 (facade wiring), D-19 | no | re-export, compile a downstream-style test |
| 92 | D-12 (`cite_check` teaching subset + snapshot marker) | no | enumerate the 27 citations — done 2026-10-28, see the improvement register's track-92 record |
| 93 | D-13 (`scheduling` serial guard) | no | run the binary under a concurrent load twice — done 2026-10-29, no reproduction in five load shapes; `SERVER_TURN` added, see the improvement register's track-93 record |
| 94 | F-03 (per-route middleware) | no | empty-prefix group is the behaviour to preserve — done 2026-10-29: `route_sync_with`/`route_async_with`/`ws_with`, seven wire tests and a doctest, both matrix rows flipped, see the improvement register's track-94 record |
| 95 | F-04 (WS subprotocol) | no | RFC 6455 §4.2.2 selection rule — done 2026-10-29: the route's declaration order is the preference, five wire tests plus four unit tests, see the improvement register's track-95 record |
| 96 | F-05, F-06 (OPTIONS, CSP nonce) | no | done 2026-10-29: the generic `OPTIONS` answer with a truthful `Allow`, and per-response CSP nonces that reach the handler and the header as one value; twenty-one tests, two matrix rows flipped |
| 97 | F-07, F-08 (directory listing, pre-compressed) | no | traversal tests first — done 2026-10-29: `StaticDirOptions` on `static_dir_with_options`, nine sensitivity mutants each rejected by a named assertion, twelve tests, two matrix rows flipped, and the matrix's own status section corrected from 108 / 6 / 19 to 112 / 5 / 16 |
| 98 | F-12, F-13, F-14 (CSRF, auth, forwarded identity) | no, but the proxy allow-list is security-sensitive | written threat note before code — split 98a/98b/98c; **98a done 2026-10-29**: `enable_trusted_proxies` (CIDR list, right-to-left walk, `Forwarded` precedence, repeated lines joined per RFC 9110 §5.2, 32-hop cap), `client_addr()`, limiter keyed on the resolved caller, ten unit + three wire tests, nine sensitivity mutants; **98b done 2026-10-29**: `BearerAuth`/`Principal` in `aether-security` plus the facade`s `require_bearer` chain entry, exactly one `Authorization` credential and one `401` shape, twelve tests and nine sensitivity mutants; **98c done 2026-10-29**: app-wide signed double-submit (`enable_csrf`, `nonce.mac` cookie, one `x-csrf-token` echo, one empty-bodied `403`, reuse-not-rotate, priced `CsrfToken` extension), twenty unit + nine wire tests, twelve sensitivity mutants, five new pinned spans — row 98 closed |
| 99 | F-09, F-10 (streaming responses, sendfile) | no | the memory budget is the design constraint — done 2026-10-29: `Response.stream` + `BodySource`/`BodyStream`, framing from the head (chunked 1.1, close-delimited 1.0, DATA + END_STREAM on h2), failed bodies end the message's connection or reset the stream, per-chunk pricing on both emitters; static files planned from metadata and streamed in 64 KiB reads with the weak metadata validator and an `If-Range` that takes the published date only; eleven `streaming` wire tests, nine unit tests, three `http2_e2e` tests, two matrix rows flipped and one pair rewritten — row 99 closed |
| 100 | F-11 (streaming requests) | no | the wire budget is the design constraint — done for HTTP/1.1 on 2026-10-29: `route_stream` + a one-slot pull lane + `body_pump`, both framings through the one decoder, `413` before dispatch for a declared oversize and `BodyError::TooLarge` mid-stream, the receive deadline failing the lane rather than aborting the handler, `settle()` closing after an unfinished or failed body, per-chunk repricing, one shared response writer; eight wire tests and five lane unit tests, the matrix row at `PARTIALLY_IMPLEMENTED` |
| 100b | F-11's HTTP/2 half (stream DATA to the handler) | no | done 2026-10-30: `serve_request` branches on the route set before buffering into `serve_streamed` + `Http2Body`, which pulls DATA into the same one-slot lane, stops on a full lane, credits the peer per frame (`release_capacity`), enforces `max_stream_body_bytes` on cumulative payload (a declared `content-length` over it is `413` before dispatch), takes `receive_timeout` onto the lane as `BodyError::Timeout`, and re-prices the stream's registration per chunk; an unfinished body is abandoned by dropping the `RecvStream` (`h2` ignores further DATA and returns the capacity) — `RST_STREAM(NO_ERROR)` was implemented, measured to destroy the queued response, and removed; six h2 wire tests, five mutation runs (disable the branch, drop `release_capacity`, ignore the deadline, remove the pre-dispatch `413`, re-add the reset) each rejected by named tests, the matrix row is `VERIFIED_IMPLEMENTED` |
| 101 | F-15 (OCSP/CRL) | no | offline behaviour first — **done 2026-10-30**, split 101a/101b: **101a** loads and enforces a signed, current CRL against the served serial, and **101b** parses and enforces an OCSP staple (response state, response type, coverage by name+key hash+serial, thisUpdate/nextUpdate, signature by the issuer or a delegated responder whose own certificate is checked by the issuer, and a GeneralizedTime-only reading of every time field); 15 revocation unit tests (six CRL, nine OCSP), 13 sensitivity mutants each killed by a named assertion, `CertificateRevoked`/`RevocationUnusable` surfaced at load or reload rather than mid-handshake, docs and the matrix row re-derived — nothing is fetched from the network |
| 102 | F-16, F-17 (property tests, fuzz) | no | two properties that must fail on a mutant — **done 2026-10-30**, split 102a/102b. **102a**: eight proptest properties in two new integration targets (four parser, four router) and eleven targeted mutants, each killed by a named property after the first run exposed that the properties drove only one of the two parser entry points. **102b**: a seeded Windows fuzz runner over a committed corpus; 15 843 328 executions in 120.011 s at seed 20261030, 0 crashes, and the oracle proved by an injected panic caught at execution 6 507 with an artifact. Suite 718 → 730 (408 unit + 310 integration + 12 doc over 62 `test result:` lines, 25 integration targets), `Property-based tests` flipped to `VERIFIED_IMPLEMENTED` — row 102 closed |
| 103 | F-18 (secrets) | the shape, if it turns into public API | decide between wrapper and documented non-goal — **done 2026-10-30**: the evidence says the framework's own render paths are the whole surface (`JwtValidator`, `BearerAuth` and `CsrfTokens` hold key material and have no `Debug`; `AetherConfig` holds no secret; the extension map prints no values; rustls' ring signing-key `Debug` prints the algorithm only), so the shape is a documented non-goal with those paths pinned rather than a `Secret<T>` wrapper nobody is required to use. One real leak was found and closed (`CsrfToken`'s derived `Debug` printed the live token), and the TLS `Debug` is now tested against key bytes. Suite 730 → 736, the matrix row's evidence rewritten, no status moved — row 103 closed |
| 104 | make `PRODUCTION_READINESS_REPORT.md` a checked live document | no | **done 2026-10-30**: the report's own census and coordinates are now re-derived by a gate instead of copied, so a stale number fails rather than sits in prose; register B22 closed — row 104 closed |
| 105 | RFC 9112 §3.2 request-target conformance on the HTTP/1 request line | no | **done 2026-10-30**: `classify_target` answers the four legal forms, serves `OPTIONS *` server-wide and refuses a target carrying a fragment; seven raw-socket tests in the new `http_conformance` target — no F-item and no register row named this defect, so the row is recorded against the code marker it moved |
| 106 | the same request-target set over HTTP/2, and F-17's fuzz figure re-run over `classify_target` | no | **done 2026-10-30**: closes what track 105 named unverified — `:path` `*` and a `*` on any other method, a non-origin `:path`, `CONNECT` answered from the head, the `:authority`-over-`Host` fold on the wire; six `http2_e2e` wire tests and a 16 089 088-execution re-run of the committed corpus with 0 crashes — F-17 re-measured, row 106 closed |
| 107 | nested state and a mountable sub-app (D-15's follow-through, the *Nested routers / independent sub-apps* row) | no | **done 2026-10-30**: `RequestContext` carries a per-scope state overlay answered before the app's, `RouteGroup::manage` copies the container on write, and `Router` + `AetherApp::mount` defer and replay registrations under a prefix; 20 tests (4 unit, 15 wire, 1 doctest), ten sensitivity mutants, the row flipped `NOT_IMPLEMENTED` → `VERIFIED_IMPLEMENTED` — row 107 closed |
| 108 | the *OpenAPI generation* row track 91 left partial — derive the document from the route table | no | **done 2026-10-31**: `RadixRouter::route_listing()` reports every registration as the (method, canonical template, handler) it answers under, each dynamic level stamped with that route's own names; `OpenApiSpec::from_routes` turns that into a document — `PathItem` became a verb-keyed `BTreeMap` because two fields could not hold nine methods — and `AetherApp::openapi_spec` is the app-side call that needs no hand-written description. 19 tests (4 router unit, 6 openapi unit, 1 router property, 8 wire in the new `openapi_derivation` target), 8 injected mutants all killed, `aether-openapi` 1 → 7 tests, the matrix row `VERIFIED_IMPLEMENTED`, see the improvement register's track-108 record |
| 109 | a green citation can still teach an API that does not exist: check the *names* the live documents assert, not only their coordinates | no | **done 2026-10-31**: `cite_check.py` gained the second pass inside the same gate — an index built from the product's own sources (declarations with their impl members, enum variants and struct fields, Python definitions, string literals, Cargo targets, test stems, the Node interop scripts) read against every code span in the 27 live documents; `Head::leaf` judged only when `Head` names something the tree declares, a bare `snake_name` judged in every other span, an unresolved name needing a per-document reason in `NAME_EXCEPTIONS` (an unused key is itself a finding), a design document needing both a `PROPOSALS` listing and its own marker. One real phantom found and fixed (the Arabic error-handling guide promised a field the tree does not build); the gate's own comment channel closed after it had turned six genuine review records into false stale findings; both READMEs brought inside both gates and their claims re-measured; nine injected mutants all rejected with byte-identical restores; cite tests 42 → 43 and govtests 111 → 112 with the ladder green through all seven — see the improvement register's track-109 record |
| 110 | the request `content-length` promise, audited on both frontends (the body rule tracks 105 and 106 left alone) | no | **done 2026-10-31**: no defect, and the negative result is the evidence — over HTTP/2 the `h2` 0.4.20 receive path already refuses every shape RFC 9113 §8.1.1/§8.2.1 makes malformed (non-digits, padding, an unparseably large value, a repeated field whose lines disagree, `END_STREAM` on the head while bytes are owed, a body longer or shorter than the promise, `content-length` beside `Transfer-Encoding`) each with `RST_STREAM(PROTOCOL_ERROR)` and no answer, and this frontend owns only the pre-dispatch `413` against `max_stream_body_bytes`; over HTTP/1 the body stops at the declared length, so the surplus *is* the next message, and any second `Content-Length` line is a `400` even when it agrees. 19 tests (11 hand-framed `http2_e2e`, 5 `http_e2e`, 2 streamed-lane, 1 `parser.rs` unit), ten sensitivity mutants killed plus two null arms recorded as null — which located two seams the tests do not hold: `ows_trim` is load-bearing only for the Unicode refusal, and `declared_length` cannot be probed for first-vs-last value because a disagreeing pair never reaches it. The real defects this track moved were in the documents: a phantom call-graph claim about `RequestView::parse`, a 49 that was 67, a 13 that was 15 — see the improvement register's track-110 record |
| 111 | the artifacts an app-generator left inside the repository | no | **done 2026-11-02**: three deletions, no feature and no behaviour changed — the Vite/React presentation site at the repo root (10 files, 1820 lines, owner-approved), the two workspace crates nothing depended on (observability, serialization: 5 files, 66 lines, zero tests, zero dependents), and twelve dependency declarations no source line named plus the two orphaned `[workspace.dependencies]` keys; the two mechanical rules (a workspace key needs a declarer, a declared name needs a whole-word hit in some `.rs`) were read by hand before every cut and the compiler was the acceptance test, so the facade now installs the eight crates its sources import, while runtime and dependency stay workspace members and keep running their tests; `members` 18 → 16, the lock 202 → 199 packages, the suite re-measured at 814 / 0 / 1 over 62 `test result:` lines, and the five live documents whose workspace claims the deletions made false re-derived — see the improvement register's track-111 record |
| 112 | the measurement path the owner had to walk by hand | no | **done 2026-11-02**: no throughput number — a tool, and two documented commands that were false. The arm-identity rules that made `pgo_ab.ps1`'s rows readable were lifted into `framework/tools/load_arm.ps1` so the second harness would not need a copy of them; `framework/tools/workers_sweep.ps1` measures one binary across N `AETHER_WORKERS` shapes in one session (rotating rep order, per-rep spread beside every median, exit-0-only medians, a `-NullArm` arm that prints what this harness calls zero, and a `banner_workers` column so the inherited default is recorded rather than guessed); `BENCHMARKS.md` §4 now names the generator's real path instead of a bare command that is not on `PATH`, and every `.ps1` recipe carries `-ExecutionPolicy Bypass` because this box refuses script files as written. Two binder defects — `1,0` arriving as the single shape 10 under `-File`, and a local variable colliding case-insensitively with the `-NullArm` switch — were caught by running the harness, not by reading it — see the improvement register's track-112 record |
| 113 | live documents naming a mechanism the tree does not have | no | **done 2026-11-02**: no behaviour changed — seven documents corrected, one doc comment reworded. The readiness report called the streamed static-file answer “static-file `sendfile`” in two places while no `.rs` file names `sendfile` or `TransmitFile` and the answer is a 64 KiB userspace read loop (`STATIC_CHUNK` / `FileBody` in `framework/aether/src/static_dir.rs`); sweeping the mechanism names over the live documents then moved four more claims onto what the code does — the validation module’s “SIMD-Optimized” title, its Arabic references’ “processor instructions”, ADR-001’s “SIMD where it pays” row (whose evidence cell “validation tests” is false in a second way: the crate has no `#[test]`), and `ARCHITECTURE.md`’s io_uring/epoll diagram box, which its banner disclosed but never named; `BENCHMARKS.md`’s notice disclaimed the throughput table only, so §1’s `SO_REUSEPORT` / multi-queue-RSS / `wrk` environment is now named as part of that same unverified run. The name pass refused `io_uring` in backticks and the citation pass refused two unpinned `:line` spans; both refusals were taken as correct, so the text moved instead of the exception table — see the improvement register’s track-113 record |
| 114 | the *Cookies* row's missing half (F-21) - a value the client wrote being read as if this server issued it | no | **done 2026-11-02**: integrity, not confidentiality, and the record says so. `framework/security/src/cookie.rs` adds `CookieSigner` (HMAC-SHA256 over name + NUL + encoded payload, verified by `ring`'s constant-time `hmac::verify`, four outcomes, no default secret, no `Debug`) and `framework/aether/src/cookie.rs` adds `RequestContext::signed_cookie` in its own module so no live `app.rs` coordinate moves. 814 -> **843** (15 + 11 + 2 + 1 = 29). Eight injected mutants: six killed, two survived because the 43-character rule implies them, and that is recorded rather than smoothed. The claim then had to reach every live sentence that pointed a reader at `cookie()` - the Arabic and English reference, both specification tables, the teaching doc's extractor sample, the readiness report's item 7, the matrix row - and five new citation pins had to be added before the gate accepted them |
| 115 | the gap track 114 recorded in its own "What is not claimed": no HTTP/2 wire test covered the signed-cookie accessor | no | **done 2026-11-03**: five wire tests appended at `framework/tests/tests/http2_e2e.rs:3716-4163` read `RequestContext::signed_cookie` through this frontend's own `header_maps` - twelve mutants, eleven killed, and the survivor recorded as the equivalence it proves; the *Cookies* row's confidentiality half stays the owner's to scope |
| 116 | the `Expect` contract, read once and answered the same way on both frontends | no | **done 2026-11-04**: `417 (Expectation Failed)` from the head for a token this server cannot meet, a real RFC 9113 §8.2.3 informational `100` over HTTP/2 (the frontend's first `send_informational`), one comma-separated token-list reading shared by both frontends, the field-line ceiling outranking the expectation over h2, and the streamed HTTP/1 path no longer answering an interim to a request that arrived whole; 23 tests, 11 sensitivity mutants with two recorded outcomes. The track's own probe found a defect it refused to bundle: a buffered body that stops arriving is closed by the 5 s per-read timeout with no `408` at all, registered as PROB-0034 with four timings and narrowed out of four documents — see the improvement register's track-116 record |
| 117 | PROB-0034: a buffered request whose body stops arriving is closed with no answer instead of being answered `408` by the receive budget | no | **done 2026-11-04**: the pending read waits the shorter of `read_timeout` and what is left of `receive_timeout`, and a request already arriving is answered `408` + `Connection: close` from one helper on both expiry sites; an idle keep-alive socket still closes silently. Four wire tests, six mutants each killed, two independent reviews (SHOULD-FIX then PASS), PROB-0034 closed; suite 879 passed — see the improvement register's track-117 record |
| 118 | `AetherError::to_diagnostic` still described a runtime `Io` as a bind failure, and nothing called or tested it | no | **done 2026-11-04**: the `Io` arm describes the runtime 500/E5002 it now is (5xx redaction, `set_error_reporter`, the file/directory/socket named); `HeaderTooLarge` names both head budgets (`AETHER_MAX_HEADER_BYTES`, `AETHER_MAX_HEADERS_COUNT`), `BodyLimitExceeded` names `AETHER_MAX_BODY_BYTES` instead of a `ServerLimits` field the facade cannot set, and `Timeout` names the handler deadline (`AETHER_MAX_REQUEST_TIME_SECS`); `Display` stops calling a 504 a connection timeout. Twelve variants are pinned as a status/code/detail/redaction cross-product, and the HTTP/1 `417` wire test now reads the body it asserted empty. Eleven mutants killed, two independent reviews (both SHOULD-FIX, all items applied); suite 892 passed — see the improvement register's track-118 record |
| 119 | the live documents print suite and citation counts that no gate compares with the run | no | **done 2026-11-04**: `count_check.py` pins 31 sentences across the five live documents (README, framework/README, the completeness matrix, the baseline, the readiness report) to the census of this run's `tests.log` and the citation summary of this run's `cite.log`, stores no number, and fails closed on a missing or stale log, an unparsed summary, a reworded or duplicated sentence, a runner-less summary line, and a crate or target the run measured that the documents do not name. The five documents were re-derived (892 passed / 0 failed / 1 ignored over 63 result lines; 112 citations over 77 spans), one stale per-crate row corrected (`aether` 96 to 97, `app.rs` 22), eighteen mutants all caught, one independent review (SHOULD-FIX, all seven findings applied); the ladder is eight gates - see the improvement register's track-119 record
| 120 | the completeness matrix marked *Bounded response memory* `NOT_IMPLEMENTED` while its own evidence cell had described the priced answer since tracks 74-75 | no | **done 2026-11-04**: the row is `VERIFIED_IMPLEMENTED`, every printed copy of the count reads 123 / 3 / 8 / 0 / 0 / 0 / 2 = 136, and `count_check.py` now recounts the matrix's own status rows and pins that summary in the matrix, the readiness report and `OWNER_VISION.md` - 52 pins over six documents, 48 tests, 18 mutants (17 caught and one null arm), three code mutants killed, one SHOULD-FIX review applied; the ladder is still eight gates - see the improvement register's track-120 record
| 121 | the live figures around the figures: the workspace crate count, the evidence corpus and the `running N tests` cross-check | no | **done 2026-11-05**: `count_check.py` gains the `workspace_members` (framework/Cargo.toml) and `corpus` (evidence.log) pin kinds and compares the running-lines sum with the passed count in the readiness report and the vision; five pins, 52 -> 57 over six documents, 60 tests, twelve document/source mutants and two code mutants killed; no printed figure moved because all three sets were already true - see the improvement register's track-121 record
| 122 | three process-wide locks on the response path (admission twice, Date cache once), and `gates.sh` shares fixed log names between runs | no | **done 2026-11-05, measurement pending (owner)**: the admission fast path and wait loop read the post-pump state under the admitting lock (one acquisition saved per admission and per wake-up), the Date cache is thread-local instead of one process-wide `Mutex`, and `gates.sh` refuses a concurrent run after an interrupted run's log splice made `counts` report 22 false findings; four release arms built for the owner's eight-worker `pgo_ab.ps1` A/B, no performance number claimed; suite unchanged at 892 passed / 0 failed / 1 ignored - see the improvement register's track-122 record
| 123 | two request-dependent answers written without `Vary` (a CORS denial and a refused preflight), and an identity body served to a client that excluded identity | no | **done 2026-11-05**: `CompressionMiddleware::apply` negotiates identity once (RFC 9110 §12.5.1), answers a bodyless `406` when identity is refused and no coding is acceptable, stops applying the size floor and the precompressed-MIME skip to a client that refused identity, sends a selected coding even when it is larger, leaves a handler-chosen `Content-Encoding` alone, keeps empty/bodyless answers, and strips the withheld representation's metadata; `CorsMiddleware` writes `Vary` before every denial (origin and the three preflight fields), and `serve.rs` runs the preflight middleware even for an unroutable method and keeps its `Vary` on a rebuilt `403`; 15 middleware unit tests + 4 HTTP/1.1 wire pins, seven injected mutants killed, one SHOULD-FIX review applied; suite 911 passed / 0 failed / 1 ignored - see the improvement register's track-123 record
| 124 | the stage ladder had not been rebuilt since 2026-10-20, and its runner divided production arms by a count that omits the client's own warmup | no | **done 2026-11-05**: no framework change - the measurement named no stage as a bottleneck. Every rung re-priced on 989f488 at 8 connections x 3000 requests, 3 reps, rotated order: parser +0.00 allocs (was +1.00 before G15), header materialization +18.00 / +1268 B (still the largest site and still PROB-0005), identity +4.00 / +56 B, response writer +3.00 / +267 B, route lookup +0.00, scheduling +2.00 / +64 B, spread 0.00; the scheduling rung matches the production scheduled-vs-plain mirror (+2.00 allocs / +64.1 B) in the same session. The runner was dividing production arms by the client's `completed` while the counted window also covered its 200-per-connection warmup, so every production allocation and CPU/req figure it printed was 6.67 % high; the G15 tree read 33.07 under the old denominator and 31.01 under the corrected one, reproducing the attested 31.00. Matched series after the fix: 44.01 (`b70ce7c`) -> 31.01 (`23999a7`) -> 30.01 (989f488 plain) -> 32.01 (989f488 scheduled); +5.00 allocs/req and +1066.3 B/req remain outside the ladder and are named, not explained. The probe's comments now cite symbols rather than rotted line numbers - see the improvement register's track-124 record
| 125 | track 123 pinned two request-dependent answers on HTTP/1.1 only and said the h2 path was not claimed; nothing checked that the multiplexed frontend answers them the same way | no | **done 2026-11-05**: five h2 wire pins over TLS with ALPN - a denied origin and a refused preflight vary on the fields that decided them, an unroutable approval is rebuilt without CORS headers but keeps `Vary`, a routable allowed preflight answers `204` with its allow fields beside a refused origin, and a refused identity answers bodyless `406` beside a complete gzip member; `H2Response.raw` added so the gzip pin reads bytes; six injected mutants killed, one SHOULD-FIX review applied; suite 916 passed / 0 failed / 1 ignored, `http2_e2e` 85 - see the improvement register's track-125 record |
| 126 | `RequestContext::cookie` answered from the first `Cookie` field line while `signed_cookie` and the CSRF check read every one, so a jar a client split across fields reported a name as absent | no | **done 2026-11-05**: `cookie()` now loops `lines_for("cookie")` with the first occurrence winning across lines exactly as within one, no allocation added; two wire pins failed before the fix (`http_e2e.rs`, `http2_e2e.rs`), three injected mutants killed with the case-sensitivity mutant surviving the h2 pin by design (that property is pinned on HTTP/1.1); seven documents' coordinates and `cite_check.py`'s pin table remapped from the files' actual diffs; suite 918 passed / 0 failed / 1 ignored, `http_e2e` 98, `http2_e2e` 86 - see the improvement register's track-126 record |
| 127 | `RequestContext::query_decoded` decoded the value with the form rule and the key with percent-decoding alone, so a browser-encoded parameter name (`first+name`) was reported absent while `queries_decoded` and the `Query` extractor found it in the same string | no | **done 2026-11-05**: `query_decoded` compares `form_decode_cow(k)` - a new borrowing helper in `aether_http::url`, with `%2B` still a literal plus because the `+` substitution stays before percent-decoding; one wire pin and one unit pin failed before the fix, three injected mutants killed; 36 `cite_check.py` pins and 31 document references remapped from the files' diffs; suite 920 passed / 0 failed / 1 ignored, `http_e2e` 99, `aether-http` lib 111 - see the improvement register's track-127 record |
| 128 | the static root's name-based rules do not decide a link: nothing in the suite sent a request through a reparse point under the root, so the containment check in `safe_resolve_path` had no wire pin | no | **done 2026-11-05**: no framework change - the suspicion that a junction could escape the static root was refuted by a red-first wire pin that passed before any edit; `safe_resolve_path` already canonicalises root and target, refuses a target outside the canonical root and returns the canonical path to open; the pin now answers a junction out of the root `404` with an empty body, the junction directory `404`, and a junction that stays inside `200`; two mutants (dropping the rejection, comparing the lexical join) each turn it red; suite 921 passed / 0 failed / 1 ignored, `http_e2e` 100 - see the improvement register's track-128 record |
| 129 | track 117 left the delivery of its `408` over TLS unverified: the four pins ran on cleartext sockets, and no TLS wire test asserted the receive-budget answer | no | **done 2026-11-05**: no framework change - one wire pin, `a_partial_request_over_tls_is_answered_408_and_closed` (`tests/tls_e2e.rs:524`), answers two stopped shapes with `408` + `Connection: close` at the 600 ms budget and then a clean `close_notify`; four injected mutants (no answer, `400`, no close header, no TLS shutdown) each turn it red, the last one only on TLS |
| 130 | the multipart parser sliced a part as `body[cursor..delimiter_start]`, which runs backwards when two MIME delimiter lines are adjacent, so a 10-byte body through `ctx.multipart()` panicked the handler | no | **done 2026-11-05**: clamp the part end at the cursor so the empty part reaches `parse_part`'s existing header-block refusal; a red-first unit pin over four adjacent/blank shapes and a wire pin expecting 400 + "header block" both die under the pre-fix slice (panic in the unit pin, 500 on the wire), and a second mutant that skips the empty part fails both; limited independent review PASS; suite 923 passed / 0 failed / 1 ignored, `aether-http` lib 112 - see the improvement register's track-130 record |
| 131 | `CompressionMiddleware::apply` replaced a response body with coded bytes but kept the identity representation's `ETag`, its integrity digests and `Accept-Ranges: bytes`, so a client could cache a gzip body under the identity validator and a later `Range`/`If-Range` answer was served from the stored identity bytes | no | **done 2026-11-05**: `strip_stale_representation_metadata` removes `ETag`, `Content-MD5`, `Digest`, `Content-Digest`, `Repr-Digest` and `Accept-Ranges` on the transform path and on the bodyless `406`, while `Last-Modified` stays; one helper, two call sites, three unit pins and one wire pin (the gzip answer of a `serve_static` asset drops the validator and range promise the identity answer keeps fails before the fix), five injected mutants all killed, limited independent review PASS; the vacuous incompressible-body fixture it exposed is repaired; suite 926 passed / 0 failed / 1 ignored, `aether-middleware` lib 93, `http_e2e` 101 - see the improvement register's track-131 record |
| 132 | `JwtValidator::new` handed any secret, including an empty slice, to `ring`'s `hmac::Key::new`, which accepts a zero-length key: an application built as `BearerAuth::new(b"", ...)` verified every token signed with that empty key, and anyone can compute one | no | **done 2026-11-05**: the constructor asserts a non-empty secret and panics naming the value, matching `CookieSigner::new` and `CsrfTokens::new`, and `BearerAuth::new` inherits the refusal; three unit pins (the refusal, a short non-empty control, the layer reaching the validator's check), three injected mutants all killed, limited independent review PASS; suite 929 passed / 0 failed / 1 ignored, `aether-security` lib 60 - see the improvement register's track-132 record |
| 133 | `Json<T>::from_request` compared `application/json` case-insensitively but the `+json` structured suffix with `ends_with("+json")`, so a legal `application/vnd.api+JSON` was answered `415` although RFC 9110 §8.3.1 makes a media type case-insensitive and the type's own doc promises `+json` types are accepted | no | **done 2026-11-06**: the suffix is read through `checked_sub` + `str::get` and compared with `eq_ignore_ascii_case`, allocation-free and boundary-safe; two integration pins (seven accepted spellings including mixed-case suffixes; missing `Content-Type` still lenient), the 415 pin widened from one row to five (`json-seq`, a parameter, `+jsonx`, `x-json`), seven injected mutants all killed after the review named four survivors, limited independent review SHOULD-FIX with every point closed; suite 931 passed / 0 failed / 1 ignored, `extractors` 21 - see the improvement register's track-133 record |
| 134 | `http2.rs` advertised the header byte budget as `limits.max_header_bytes as u32`, the HTTP/2 `SETTINGS_MAX_HEADER_LIST_SIZE` field, while `AETHER_MAX_HEADER_BYTES` and the public `AetherConfig::max_header_bytes` accept any `usize`: a budget at or above 2^32 truncated modulo 2^32, and 4 GiB became a cap of 0, under which `h2` answers 431 to every request while HTTP/1 on the same listener honours the configured budget | no | **done 2026-11-06**: `header_list_cap` saturates at `u32::MAX` instead of wrapping, called from `builder`; two unit pins (the pass-through and the ceiling, plus a 64-bit-only row just past it), two injected mutants killed including the off-by-one clamp that proves the pin holds the exact ceiling; no independent review, so the claim rests on the maintenance pass and the vendored `h2` source read; suite 933 passed / 0 failed / 1 ignored, `aether` lib 99 - see the improvement register's track-134 record |
| 135 | `graceful_shutdown_drains_streams_in_flight` slept 80 ms between beginning an h2 request and shutting the server down, which does not prove the server has read that stream's `HEADERS`: under load the final `GOAWAY` can name a last stream id below it and the client's response future fails with `GOAWAY`/`NO_ERROR`, which is the protocol's retryable outcome, not a drain failure | no | **done 2026-11-06**: the test now registers `/slow` itself and waits for the handler to set an `AtomicBool` before dropping the shutdown sender, a happens-before with `h2` having decoded the request's `HEADERS`; 48 isolated and 12 full-suite runs before the fix did not reproduce the flake and 60 isolated runs after it passed, so the causal chain is the failure text plus the vendored `h2` source rather than a repeatable red run; no production line touched, no count moved (933 passed / 0 failed / 1 ignored) - see the improvement register's track-135 record |
| 136 | `audit_request_desync` refused a repeated `Content-Length` and a `Content-Length` beside a `Transfer-Encoding`, but accepted a repeated `Transfer-Encoding` field, any coding that is not exactly one `chunked` (`gzip`, `chunked, gzip`), and a `Content-Length` that is not a decimal number, while the parser refuses all of them and the module doc claimed the chunked half | no | **done 2026-11-06**: the audit now applies the parser's framing rules - one `Transfer-Encoding` field whose OWS-trimmed value is exactly `chunked`, and a `Content-Length` that is all decimal digits and fits a `usize`; six unit pins in `smuggling.rs` (three red before the fix) and two more shapes through the facade, five injected mutants each killed, 939 passed / 0 failed / 1 ignored - see the improvement register's track-136 record |
| 137 | `JwtValidator::verify_signature` decoded the signature segment with a helper that trims trailing `=`, and nothing constrained its length, so every padded spelling of a valid HS256 token verified as the same credential although RFC 7515 §2 spells each segment as unpadded base64url | no | **done 2026-11-06**: a 43-character canonical tag length (`SIGNATURE_CHARS`) is enforced before decoding, accepting every genuine HMAC-SHA256 tag and rejecting only aliases; one unit pin and one facade assertion, five injected variants (three killed, two surviving-by-equivalence and argued), the `jwt.rs` citation coordinate remapped from 106 to 123 in three documents and the pin table, the matrix's stale "7 unit tests" corrected to 9, and the derived "455 tests added since" corrected to 466; suite 940 passed / 0 failed / 1 ignored, `aether-security` lib 67 - see the improvement register's track-137 record |
| 138 | `an_h2_body_past_the_ceiling_mid_stream_fails_the_read` sent four 50-byte DATA frames against a 64-byte ceiling and expected every client write to be accepted, but once the handler's read fails the server may reset the request stream, so a later write legitimately returns `User(InactiveStreamId)`; the test asserted the ordering of client queueing against that reset | no | **done 2026-11-06**: the loop tracks the bytes the client accepted, stops at the first refused write and asserts more than 64 bytes were enqueued before any reset, so crossing the ceiling stays the precondition; deterministic red under a forced reset, green under the same reset, 10/10 clean runs, and the guard fires when the ceiling is injected to 0 with a forced early reset; the 413 and the usable connection stay asserted; suite unchanged at 940 / 0 / 1 - see the improvement register's track-138 record |
| 139 | `serve.rs` ran the CSRF gate above the host allow-list and the rate limiter even though CSRF is the only one of the three that does cryptographic work, so a request the cheaper checks were about to refuse still paid an HMAC verification, while the block's comment claimed a host-first order and "no router work" for a refusal - false since the serving-core extraction, where the route match runs above it | no | **done 2026-11-06**: host validation and the limiter fold into one `rejection` that the CSRF block is skipped on, and the 403 is chained after the cheaper answers; the comment now names the shipped order, the one routing work a refusal still pays for (the reused route match) and the CSP nonce draw it cannot defer; one wire pin (`tests/csrf.rs` 9 -> 10) whose combined mutant proves chain precedence, a temporary marker measuring 1 CSRF consultation for three requests against 3 under the old order, and no wire-visible change; suite 941 passed / 0 failed / 1 ignored, integration 428 - see the improvement register's track-139 record |
| 140 | `serve.rs` drew the CSP nonce in the request prologue, above the route match and every security refusal, so a `404`, a `405`, a limiter `429`, a host `400`, a CSRF `403`, a preflight or the generic `OPTIONS` paid an OS-entropy draw for markup it never renders - and an entropy failure answered `500` before the refusal was decided, masking the real status | no | **done 2026-11-06**: the draw moved into `MatchResult::Found` behind a reachability predicate (any non-WebSocket route, or a WebSocket `GET` over HTTP/1.1), so the entropy-failure `500` can only affect a request that reaches a handler; framework answers keep the policy with the nonce expression removed (the middleware's fail-closed rendering); two existing tests extended (`http_e2e` with a `404` and a `429`, `http2_e2e` with the h2 upgrade `400`), two injected mutants killed, five coordinates and their pins re-derived, census unchanged at 941 passed / 0 failed / 1 ignored - see the improvement register's track-140 record |
| 141 | With `static_dir`'s default options, `serve_from_dir` answered `404` for any directory before it reached the `index.html` lookup, which sat inside the listing branch - so a plain static site's home page and every sub-directory page were unreachable, and the tree root had no route at all (the catch-all needs one character after the prefix and the prefix route was registered only for a listing) | no | **done 2026-11-06**: the contained-index check now runs before the listing branch, `serve_directory` is only the listing, and the prefix route is registered when the listing is on or the root ships `index.html` at registration; one wire pin (`http_e2e` 101 -> 102) red first at `/assets/`, two mutants killed on `/assets/app/` and `/assets/`, 21 coordinates and their pins re-derived, suite 942 passed / 0 failed / 1 ignored with integration 429 - see the improvement register's track-141 record |
| 142 | The shared head parser refused any declared `Content-Length` over the buffered `max_body_bytes` before the route was known, so an HTTP/1 streaming route answered `413` to every declared upload larger than 2 MiB although `max_stream_body_bytes` exists to carry it and the h2 frontend streams the same request | no | **done 2026-11-06**: `parse_head`/`parse_head_into` take the declared-length ceiling from their caller - the buffered entry points pass `max_body_bytes`, the streaming head and the `Expect` probe pass none - and `app.rs`'s pre-dispatch `max_stream_body_bytes` refusal remains the streaming ceiling; one unit and one wire pin red first, three killed mutants, nine parser/fuzz coordinates and their pins re-derived, suite 944 passed / 0 failed / 1 ignored with unit 495 and integration 430 - see the improvement register's track-142 record |
| 143 | The 0.2 release boundary: PROB-0011's builders could not be chained and PROB-0005's public header map let a middleware rewrite a field the framework had already decided on, and the workspace still said 0.1 | no | **done 2026-11-06**: 38 `AetherApp` builders take `self` and return `Self`, `RequestContext` keeps one private `header_multi` store with `header`/`header_values`/`headers` and no write surface, both frontends build it, the workspace is 0.2.0 with all 16 members inheriting, and `docs/11_migration_0_1_to_0_2.md` is the migration note; one `compile_fail,E0609` doctest and one `no_run` doctest, the count gate now attributes cargo's second `compile_fail` doctest binary to the doc lane, 35 citations re-derived, suite 946 passed / 0 failed / 1 ignored - see the improvement register's track-143 record |
| 144 | The 0.2 header-store pass corrected the matrix and the migration note but left four live documents teaching the 0.1 API: the request-context page still printed `pub headers: HashMap<String, String>` and described a public map, the readiness report said that map is keyed by wire casing, and the full documentation told readers that a middleware could revoke an upgrade by editing `RequestContext::headers` while PROB-0005 was still open | no | **done 2026-11-06**: documentation only - the struct excerpt now matches `app.rs:394-428` field for field, the three passages name the one private store and the `header`/`header_values`/`headers` reads, and the `compile_fail,E0609` block plus the wire test are named as the pins; the roadmap's merged rows 140 and 141 were split onto their own lines; no code or test changed, so the suite held at 946 passed / 0 failed / 1 ignored while the name pass re-derived to 390 qualified / 1455 bare names - see the improvement register's track-144 record |
Two rules bind every track above: a failed gate is recorded as failed rather than smoothed with a
looser bound, and no performance claim is published that the owner did not measure. Dated and
historical documents keep their coordinates; only live documents are re-derived.

### 19.8 What this plan does not claim

- It does not claim every `NOT_IMPLEMENTED` row becomes `VERIFIED_IMPLEMENTED`. The set is 8 rows on
  the track-120 tree (19 when this plan was written, 10 after tracks 88–103, track 107 moved one of
  those ten, track 108 moved none — it flipped a *partial* — and track 120 moved *Bounded response
  memory*, whose evidence cell had described the priced answer since tracks 74-75): six are refusals
  or an owner-deferred item — `Upgrade: h2c`, HTTP/3, *Secret
  management*, permessage-deflate, *Handler macros*, *Hot reload* — and the remaining two are the
  F-19/F-20 shaped work this plan cannot price without a profile the owner measures: *Request arena*
  and *Thread-per-core runtime* are blocked on it.
- It does not claim Axum/Actix parity as a finish line. The parity rows are the ones this
  repository chose to measure, not an exhaustive list of what those frameworks do.
- It does not claim any Linux or macOS behaviour. Every green number stays Windows 10.0.26200 x64
  MSVC until PROB-0012 is unblocked.
- It does not price any performance work. Tracks 99–100 have memory-budget constraints, not speed
  claims; speed remains track 78's, measured by the owner.
