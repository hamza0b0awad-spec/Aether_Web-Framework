# Improvement register — 2026-10-21 (pre-commit audit, Gates 4–6)

Scope rule in force while this register is written: **no performance work and no code changes**
until Gates 5 and 6 are finished. Every entry below is a *record*, not an authorized change. Each
entry names the action type so the next round can decide separately:
`DOCUMENT` (wording only), `FIX-DECISION` (code change needing the owner's go-ahead),
`MEASURE` (must be measured before any claim), `DECISION` (owner's open choice).

All measurements below are **Windows 10.0.26200 x64 (MSVC) only**. Nothing here has been built or
run on Linux or macOS (`PROB-0012` stays open), and nothing here is a performance result.

Commands used for the measurements are recorded inline so each number can be re-derived rather
than trusted.

---

## A. Gate-4 findings — WebSocket handoff edges (accepted as DOCUMENT-only)

### A1. `handed_off.lock().unwrap()` is the only panic path that escapes handler isolation
`framework/aether/src/serve.rs:278-282` (and the terminal write at `:257`) unwraps a `Mutex`
guard. The lock is per-request (`Arc::new(Mutex::new(None::<RequestContext>))`, `:249`), held only
inside this branch, and every holder returns without panicking while holding it — the terminal's
body writes then returns a future; the take-and-filter is a single statement. So a poison can only
be produced by a panic *inside* the guard body, which today cannot happen.
- Worst case if it ever fired: the connection task panics and the socket is dropped — the client
  loses the connection, not the process. There is no `catch_unwind` anywhere around `serve`, so
  this panic is not mapped to a 500 the way a handler panic is.
- Impact limit: per-connection, not per-listener; no shared state is corrupted, because the mutex
  is not shared between requests.
- Action: **DOCUMENT**. If it is ever changed, the replacement is `expect("uncontended per-request
  handoff slot")` or `unwrap_or_else(PoisonError::into_inner)` — a *decision*, not a default.

### A2. A chain run in a separate task is not cancelled by a refusal
`tokio::spawn(next(req))` detaches work. If the guard answers 202 while the forked chain is still
running, the refusal is served, but the spawned chain keeps executing until it finishes on its own;
dropping the `JoinHandle` does not abort it. The same applies to a `select!` where the guard wins.
- Safety property that *does* hold: a detached chain cannot start a session, because the session is
  only started by the frontend on a handoff the status guard authorises. Verified by
  `a_middleware_that_forks_the_chain_still_answers_the_upgrade_request`
  (`framework/tests/tests/http_e2e.rs:1767`) — the handler never starts.
- No readiness doc currently describes a refusal as full cancellation. Checked by grep over
  `framework/README.md`, `framework/docs/*.md` and the `ws` doc block in
  `framework/aether/src/app.rs:876-911`; the only "cancel" hits are about the scheduler tallying a
  panicking session (`PRODUCTION_READINESS_REPORT.md:717-719`), which is a different thing.
- Action: **DOCUMENT** — add the explicit sentence "a refusal does not cancel work the guard already
  spawned" to `AetherApp::ws`, so nobody reads status-authorisation as task cancellation.

### A3. `101` is the handover *authorisation*, not proof the chain completed
`serve.rs:282` filters the recorded request by `chain_response.status == 101`. The status identity
check was chosen over a flag because a flag would upgrade the socket and throw away the guard's own
answer. This was falsified, not assumed: replacing the guard with `.filter(|_| true)` makes the fork
test fail `left: 101, right: 202` and the abandoned test fail `left: 101, right: 503`.
- Measured causal detail that the obvious explanation gets wrong: the terminal records the
  `RequestContext` **synchronously when `next` is called**, so a dropped chain future still fills the
  slot. What refuses the upgrade is the returned status, never unreachability.
- Remaining sharp edge: see A4.
- Action: **DOCUMENT** (done in `app.rs:885-911`, `README.md`, matrix item 18). Changing the shape —
  a `101` as an ordinary `Response` that owns the socket, Axum's design — is a `ServeOutcome`
  redesign and a separate decision, recorded as such in
  `framework/docs/FEATURE_COMPLETENESS_MATRIX.md:484-486`.

### A4. A middleware could return a hand-built `101` and get it served with no protocol switch — FIXED 2026-10-22
Discovered while classifying claims in Gate 5, so it is listed here too.
When the slot is empty (the terminal never ran) but the chain still returned status `101` —
`Response::status(101, ..)` is a public constructor — `serve.rs` returned `chain_response` on the (`None => chain_response`)
**ordinary** response path. That path applies the CORS / security-header / compression post-pass and
wrote a generic status line, so the client received `HTTP/1.1 101 …` while the server kept parsing
HTTP/1 framing on the same socket. The client believed the protocol had switched; the server had not.
- Evidence (then): the `None => chain_response` arm of `aether/src/serve.rs` plus the ordinary serializer in
  `framework/http/src/response.rs:196` (formats the status line from the response's own status).
- Coverage (reproduction, before the fix): `a_hand_built_101_from_a_middleware_is_not_an_upgrade`
  (`framework/tests/tests/http_e2e.rs`). Measured wire shape — and one prediction here was wrong:
  the response arrived as `HTTP/1.1 101` with `Date` and `X-Request-ID`, **no**
  `Sec-WebSocket-Accept`, **no body and no `Content-Length`** (the HTTP layer already sends every 1xx
  body-less, `http/src/response.rs:269,279`, unit-tested at `:697`), no session started, and the same
  connection answered a following `GET` with `200`. So the draft's "post-pass and body and all"
  over-stated the damage: a client that checks the accept token (RFC 6455 §4.1) rejects this answer,
  and a client that reads the status alone is the one that switches framing into an HTTP parser.
- Doc accuracy: `FEATURE_COMPLETENESS_MATRIX.md` said the post-processing "does not touch it
  either". That was true of the *synthesized* handshake and **false** of this path. Narrowed in the
  same round, then superseded by the fix.
- **Decision (owner, 2026-10-22): candidate (a) — refuse the claim.** `101` means the protocol
  transition happened, and a middleware that never let the terminal run did not perform it, so it
  must not be allowed to announce it. Implemented as one arm in the serving core:
  `None if chain_response.status == 101 => Response::status(500, "Internal Server Error")`, placed
  after the handoff is taken and before the fall-through `None => chain_response`, so the only
  response it can touch is a `101` that has no recorded handoff behind it.
- The four conditions the owner attached, each measured rather than argued:
  1. *Scoped to a middleware `101` with no upgrade path.* The arm sits inside the WS-route branch and
     keys on the exact status. A guard that **does** call `next` records the context, so its handoff
     still takes the `Some(ctx)` arm and upgrades — proven by the positive control in
     `a_hand_built_101_from_a_middleware_is_refused_with_500` (`101` + the RFC 6455 accept token
     `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=` + session started + echo frame received).
     `the_101_refusal_is_scoped_to_websocket_upgrade_handoffs` pinned the two boundaries as they stood
     on 2026-10-22: a hand-built `101` on a route that is **not** a WS route was still served as `101`
     (body-less, no `Content-Length`, connection stays HTTP/1), and a `103` or a `418` returned on a WS
     route was still served as itself; widening the arm to `chain_response.status < 200` made it fail
     `left: 500, right: 103`. **Superseded for the first boundary by track 58 (A6, 2026-10-25):** the
     test is now named `a_101_without_a_handoff_is_never_served`, a `101` outside a handoff is refused
     on every route and both protocols, and the `1xx`/non-`101` half is unchanged and still pinned.
  2. *HTTP/2 and h2c unaffected.* Over h2 a WS route is refused `400` before the chain
     (`allows_upgrade` is false), so this arm is unreachable there; measured by the whole
     `http2_e2e` target staying at **27 passed** across the change, including
     `a_websocket_route_reached_over_h2_answers_400` and `..._over_h2c_answers_400`.
  3. *No body, no contradictory headers on the `500`.* The refusal is a freshly built response, so the
     claim is dropped with the guard's own answer. Measured shape:
     `HTTP/1.1 500 Internal Server Error` with exactly `X-Request-ID`, `Date`, `Content-Length: 0`
     and an empty body — no `Upgrade`, no `Connection`, no `Sec-WebSocket-Accept`, no `Content-Type`,
     even though the injected guard put all four plus a JSON body on the `101` it returned.
  4. *The test fails without the fix.* Restoring `None => chain_response` for the `101` case makes
     `a_hand_built_101_from_a_middleware_is_refused_with_500` fail `left: 101, right: 500`; with the
     arm in place it passes. Both were run, not inferred.
- Deliberate deviation from the draft: **no extra log line.** Option (a) as first written said "downgrade
  and log it". The refusal does not need its own diagnostic: the response post-pass in
  `aether/src/serve.rs` already records `response.status` per request through `LoggerMiddleware`
  (`if self.logger.enabled { self.logger.log(.., response.status, ..) }`), so the fabricated upgrade
  arrives in the access log as the `500` the client sees. Adding a separate `eprintln!` — the only
  other diagnostics surface in this crate — would write to stderr once per request on a path a client
  can influence through an application-owned guard, which is an unbounded-write risk rather than a
  diagnostic improvement. If a distinct signal is wanted (once-per-route, or a metric), that is a
  separate decision with its own cost measurement.
- Still open, deliberately out of this scope: a hand-built `101` returned by a middleware on a
  **non-WebSocket** route is still served as `101`. Fixing that would mean deciding how *all*
  server-generated `1xx` (e.g. `103 Early Hints`) may be produced, which is a wider contract than
  the WebSocket-handoff decision the owner approved. Registered as A6.

### A5. The `405` for a non-`GET` on a WS route comes from the router, not from `serve.rs` — NEW, from falsification (2026-10-22)
`app.ws` registers the path for `GET` only (`aether/src/app.rs`, the `add_route(Method::Get, ..)`
call in `ws`), the router has no HEAD→GET fallback (`router/src/node.rs::RouteNode::get_handler`),
and a method mismatch on a matched path is `MatchResult::MethodNotAllowed`
(`router/src/trie.rs::RadixRouter::leaf_result`), answered in the `MethodNotAllowed` arm of
`aether/src/serve.rs` without dispatching. Consequence: **the WebSocket branch's own method gate is
unreachable through the public API** — the earlier
draft attributed the pre-chain `405` to it, which was wrong.
- Measured, three ways: (1) disabling the branch's method gate alone left the new test **green** (the router
  still answered `405`, `chain_runs == 0`) — a fault that a status-only reading would have missed
  entirely in the other direction; (2) with the route also registered for `POST`, the guard answered
  `405` and the chain stayed at `0`; (3) with that registration *and* the guard removed, the framework
  handed a `POST` a `101`. So the branch is genuine defence in depth, not dead weight, and it is
  commented as such in `aether/src/serve.rs`.
- Action: **DOCUMENT** (done: README, both Arabic docs, `serve.rs` comment, PRR §14). **DECISION**
  (resolved 2026-10-22 by the owner): **keep** the duplicate guard, and document each layer's own
  responsibility — the router decides which methods a registered route accepts and answers the rest
  with `405` + `Allow`; the service core guarantees that nothing which slipped past an earlier check
  can reach the upgrade path. Unifying them was explicitly rejected unless a flow analysis showed the
  guard adds no independent protection; measurement (3) above shows it does.

### A6. A hand-built `101` on a **non-WebSocket** route is still served as `101` — NEW, found while scoping A4 — **FIXED 2026-10-25 (track 58)**
The A4 refusal lives in the WS-handoff arm, so a middleware on an ordinary route could still return
`Response::status(101, ..)` and get `HTTP/1.1 101` on the wire (body-less, no `Content-Length`, the
connection stays HTTP/1). As measured on 2026-10-22 by
`the_101_refusal_is_scoped_to_websocket_upgrade_handoffs`, which asserted this as the boundary of the
approved fix rather than as desired behaviour; that test is now
`a_101_without_a_handoff_is_never_served` and asserts the opposite (see the action below).
- Why it was not folded into A4: refusing every middleware-authored `1xx` would decide the contract
  for informational responses in general (`103 Early Hints` is the legitimate case), which is wider
  than the WebSocket-handoff decision the owner approved. Scope was not to be extended without a
  decision.
- Action: **FIX-DECISION** — **DECIDED AND IMPLEMENTED 2026-10-25 (track 58), option (a).** The agent
  took the decision under the owner's *Decision independence* policy (`AGENTS.md`) rather than leaving
  it queued for an approval turn; nothing about the informational class beyond `101` was decided.
  - *Decision.* Any `101` the frontend did not synthesize for a handoff is refused, wherever it was
    authored (guard or handler) and on whichever protocol. Option (a) said "reject a middleware `101`
    on any route"; two things were added because the evidence showed the status, not the author or the
    route, is what makes the claim false: a handler can return `101` too, and over HTTP/2 the old
    behaviour was worse than mis-serving — `h2` reset the stream with `PROTOCOL_ERROR` and the client
    got **no answer at all**.
  - *Where.* The core's single response exit (`aether/src/serve.rs`, immediately before
    `post_process`), not the frontends: one rule, and the WebSocket handoff arm returns
    `ServeOutcome::WebSocketUpgrade` before reaching it, so a real handoff is untouched and the
    synthesized `101` is never routed through it. Answer is `500 Internal Server Error`, the same one
    the handoff arm already gives a guard's `101` (A4).
  - *Failing-first evidence, measured.* With the rule absent: `a_101_without_a_handoff_is_never_served`
    failed `left: 101, right: 500` (the `HTTP/1.1 101` reached the wire, body-less), and
    `a_101_from_a_middleware_is_refused_over_h2` failed with
    `Reset(StreamId(1), PROTOCOL_ERROR, Library)` at the client's "response head". With the rule in
    place both pass. Widening it to `response.status < 200` fails the same test
    `left: 500, right: 103` — the `1xx` guard is still live, which is why the condition is `== 101`.
  - *Suite.* `cargo test --workspace` → 488 passed / 0 failed / 1 ignored (50 result lines; 277 unit +
    206 integration + 5 doc), up one from 487 because `http2_e2e` gained the protocol-half test;
    `http_e2e` keeps its count (the scope test was renamed, not doubled). `cargo clippy --workspace
    --all-targets` exit 0.
  - *Still undecided, and deliberately so:* the rest of the class. A middleware's `103` still travels
    untouched (it is pinned by the same test), and whether an application may author informational
    responses at all is a wider contract than this finding — no code in this track answers it.

---

## B. Gate-5 claim audit — findings that are CONTRADICTED or UNVERIFIED

Each row states the measurement that produced the classification, so a repeat can confirm or refute.

### B1. `cargo clippy --workspace --all-targets` does **not** pass — CLOSED 2026-10-22 (track 23)
```
$ cargo clippy --workspace --all-targets          # exit 101
tests\tests\tls_e2e.rs:452:9: error: read amount is not handled
    = note: `#[deny(clippy::unused_io_amount)]` on by default
error: could not compile `aether-tests` (test "tls_e2e") due to 1 previous error
```
`clippy::unused_io_amount` is a **correctness** lint, deny-by-default. It was **pre-existing at the
`HEAD` of 2026-10-21**, pinned now as `git show 907dfcd^:framework/tests/tests/tls_e2e.rs` (that
commit is `c847261`; line 452 prints `tls.read(&mut byte).await.expect("read upgrade head")`), so this
session did not introduce it — but the readiness docs describe clippy output as
style-only warnings, and at `--all-targets` scope that is false. **The bare `git show HEAD:…` form of
this citation stopped reproducing on 2026-10-22**: `907dfcd` fixed the line, so HEAD now shows
`read_exact` and a reviewer running the command as written gets evidence that contradicts the
sentence. Per **B9**, a citation that depends on where HEAD happens to point is a coordinate, not an
anchor.
- Library scope is clean of it: `cargo clippy --workspace` → exit 0, 39 tagged warnings,
  families `19 collapsible_if, 3 unnecessary_map_or, 2 type_complexity, 2 needless_borrow,
  2 large_enum_variant, 1 each of while_let_loop, too_many_arguments, sliced_string_as_bytes,
  should_implement_trait, result_unit_err, manual_strip, implicit_saturating_sub,
  field_reassign_with_default, empty_line_after_doc_comments, derivable_impls,
  chunks_exact_to_as_chunks`. **None** in the correctness group → the existing "39 warnings, none
  correctness-class" claim is VERIFIED *for lib scope only*.
- Whole-tree scope: 55 tagged diagnostics (`--all-targets`), adding `7 field_reassign_with_default,
  5 needless_borrows_for_generic_args, 2 needless_borrow, 2 large_enum_variant, 1 unused_io_amount,
  1 needless_range_loop, 1 manual_div_ceil, 1 chunks_exact_to_as_chunks` … across test targets.
- Action: **DOCUMENT** (state the two scopes separately) + **FIX-DECISION** for `tls_e2e.rs:452`
  (`read_exact(&mut byte)`, a one-line test change). Nothing else in the tree blocks pinning.
- **Closure, 2026-10-22:** the action was taken. `tls_e2e.rs:452` now reads
  `tls.read_exact(&mut byte)`, matching the four uses already in the same file (402, 406, 410, 414),
  and the EOF case that used to spin the `while !head.ends_with(b"\r\n\r\n")` loop forever now fails.
  Re-measured: `cargo clippy --workspace --all-targets` exits **0** with **0** `^error` lines, and
  `cargo clippy --workspace` (lib scope) is unchanged at **39** warnings. `--all-targets` is now a
  usable gate.

### B2. "HTTP/2 and HTTP/3 are not implemented and are not claimed" — CONTRADICTED as written — **CLOSED 2026-10-22**
`PRODUCTION_READINESS_REPORT.md:78-79` (§2) and `:294-296` (§5 "Not verified") still say HTTP/2 is
not implemented. HTTP/2 shipped in the same tree: `framework/aether/src/http2.rs` (untracked, ~660
lines), 27 wire tests in `framework/tests/tests/http2_e2e.rs`, `h2 = "0.4.20"` in
`framework/aether/Cargo.toml`, and `FEATURE_COMPLETENESS_MATRIX.md:134` states "Aether speaks
HTTP/1.1 and HTTP/2". §11 is the correct record; the body sections were never amended, and the
"report body is the 2026-10-18 record and is not edited" policy does not hold for the *executive
summary*, which is what a reader reads first.
- Action: **DOCUMENT** — either amend §2/§5 or put an explicit pointer to §11 in them.
- Action taken: §2's protocol bullet and §5's "Not verified" list both amended in place; §5 now
  points at §11 and at §10 for the composition.
- Re-measured this round, the same sentence carried a *stale count*: it said "27 wire tests on the
  final tree" while the file holds 29 (`grep -c tokio::test tests/tests/http2_e2e.rs` → 29,
  2026-10-22). Corrected to 29 on the current tree with the command attached. That is B7 again —
  closing the claim is not closing the number.

### B3. Status counts in §3 are stale — CONTRADICTED — **CLOSED 2026-10-22**
```
$ awk -F'|' '/^\|/ && NF>3 {gsub(/^ +| +$/,"",$3); print "["$3"]"}' docs/FEATURE_COMPLETENESS_MATRIX.md | sort | uniq -c
97 VERIFIED_IMPLEMENTED / 20 NOT_IMPLEMENTED / 12 PARTIALLY_IMPLEMENTED / 3 UNWIRED_IMPLEMENTATION / 2 NOT_APPLICABLE  (= 134)
```
`PRODUCTION_READINESS_REPORT.md:87-98` claims 94 / 22 / 11 / 3 / 2 = 133 rows. The matrix's own
tally (`:349`, `:357` "Total: 134 rows") matches the measurement; §3 does not.
This is the recurring doc-drift failure mode — re-derive from the file, never from another doc.
- Action: **DOCUMENT** (§3 recount) and keep the mechanical command in the doc so the next round
  re-measures instead of re-copying.
- Action taken: §3 was rewritten to 97 / 12 / 20 / 3 / 2 over 134 with the `awk` line kept beside it.
  Re-run on 2026-10-22, after this round's row edits, the tally is unchanged
  (97/20/12/3/2 = 134), so the doc and the file still agree.

### B4. "Doc tests contain no runnable examples" — CONTRADICTED — **CLOSED 2026-10-22**
`PRODUCTION_READINESS_REPORT.md:285`. Measured on the final tree: `Doc-tests aether … 5 passed`.
The `Extensions` examples are the runnable ones (`§12` claims 2; the crate total is 5).
- Action: **DOCUMENT** (correct the line and state the per-crate doc count).
- Action taken: §5 now states doc **5** and says in terms that the earlier line was stale; the
  per-target split in the same paragraph carries the 263/194/5 chain that §2 and §10 repeat.

### B5. "Only two refusals skip the chain" — the 405 half is UNVERIFIED by tests — **CLOSED 2026-10-22**
The WebSocket branch in `aether/src/serve.rs` answers `405 + Allow: GET` for a non-`GET` on a WS
route and `400` when `!allows_upgrade`, both before the chain. Framework/README and matrix item 18
state this as behaviour.
```
$ grep -rn "POST /ws" tests/tests/     → (no matches)
$ grep -rn "405" tests/tests/*.rs | grep -iE "ws|websocket|upgrade"   → (no matches)
```
No test sent any non-`GET` to a WS route. The 405-on-a-WS-route claim rested on reading code only.
- Action taken: `a_non_get_on_a_websocket_route_is_refused_before_the_middleware_chain` now sends a
  `POST` **with a complete handshake** and asserts `405` + `Allow: GET` + a middleware counter still
  at `0` + no session started + the connection still serving `GET /ping`, then flips that counter to
  `1` on the ordinary request as a positive control.
- The falsification reclassified the claim: see **A5**. The refusal is real and pre-chain, but the
  component answering it is the router, and the branch's own gate is the normally-unreachable second
  line.

### B6. HTTP/2 → 400 on a WS route is asserted as a status, not as a chain skip — **CLOSED 2026-10-22**
`framework/tests/tests/http2_e2e.rs` asserted `status == 400` and that the connection keeps serving.
Neither test carried a middleware counter, so neither pinned the stronger claim in
`framework/README.md` ("only two refusals skip the chain") — they would still pass if the chain ran
and every middleware answered 400.
- Action taken: both the TLS (`a_websocket_route_reached_over_h2_answers_400`) and cleartext
  (`a_websocket_route_reached_over_h2c_answers_400`) tests now register a counting middleware and
  assert `chain_runs == 0`.
- Falsified: with `!allows_upgrade` replaced by `false`, **both** fail `left: 1, right: 0` while the
  status stays `400` — which is the exact blindness this row complained about, demonstrated rather
  than asserted.

### B7. The HTTP/2 matrix row cites "16 wire tests" — stale — **CLOSED 2026-10-21**
`FEATURE_COMPLETENESS_MATRIX.md:164` said "Verified by 16 wire tests (`tests/http2_e2e.rs`)" while
the file had 27. Rewritten to cite the recount command and to label the row as evidence rather than a
re-runnable gate.

### B8. §11's external-client table is not reproducible from the repository — **CLOSED 2026-10-25 (track 59)**
The Node.js/python-h2/hand-framed rows were run from scratch scripts. `framework/tests/examples/`
holds only `h2_interop_server.rs` (tracked-to-be, 151 lines); the client scripts live in `C:\tmp` and
`%LOCALAPPDATA%\Temp`, which the owner deletes. The checked-in suite now covers the same behaviours
with our own `h2` client, but "independent implementation" evidence cannot be re-run by a reviewer.
- Action: **DECISION** — either commit the client scripts under `framework/tests/examples/` or
  record the exact reproduction commands in §11. Do not re-claim these rows as current until re-run
  against a tree that also contains the WS-middleware change.
- **Re-run done 2026-10-23; commands recorded; the commit decision is still the owner's.** Against
  `cargo build --release -p aether-tests --example h2_interop_server` on the current tree (WS
  middleware, `101` refusal, PROB-0008 pricing and the two idle-deadline tests all present):
  a freshly written Node client — `node C:/tmp/t30/node_h2_interop.js <exe>` — passed **24/24 twice**
  (22/22 on the first pass, before the cleartext-WS and `Upgrade: h2c` probes were added), and the
  hand-framed cap probe — `python C:/tmp/t30/h2_handframed_cap.py <exe> 18976` — passed three times:
  110 `HEADERS` frames at once → 100 × `200`, 10 × `RST_STREAM(7 REFUSED_STREAM)`, 0 `GOAWAY`, next
  request `200 pong`. §11's table now carries a per-row re-run column and the reproduction commands.
- Two probe findings, both of the [[feedback-verify-probe-stack]] shape, are recorded in §11 rather
  than smoothed over: the first SETTINGS assertion read id `0x5` and printed `undefined` — `0x5` is
  `MAX_FRAME_SIZE`, `MAX_HEADER_LIST_SIZE` is `0x6`, and only decoding the frame proves either; and
  110 streams through Node come back 110/110 **answered with no refusals**, because a cap-aware client
  queues against the advertised 100 and never violates it. That row is only reproducible by a client
  that ignores `SETTINGS`, which is why the hand-framed script exists and why "the cap works" must
  never be claimed from a library client's status codes.
- Newly measured while re-running, consistent with the code comment on `AetherApp::enable_h2c`: Node
  24.18 opens a cleartext `http2.connect()` with the `PRI * HTTP/2.0` preface (prior knowledge), and an
  explicit `Upgrade: h2c` HTTP/1.1 request to that listener is answered `HTTP/1.1 200 OK` — no `101`,
  no switch. The `Upgrade: h2c` non-goal is now observed on the wire, not only read from `app.rs`.
- **Decision made 2026-10-25 (track 59) under the owner's decision-independence policy: the files are
  committed, not just the commands.** The choice had been left open as "the owner's"; it is an
  engineering choice with a clear better answer, so it was taken. `framework/tests/examples/`
  `node_h2_interop.cjs` (282 lines) and `h2_handframed_cap.py` (151 lines) now sit beside
  `h2_interop_server.rs`; cargo reads only `*.rs` there, so the build is untouched. §11's
  reproduction block was rewritten to those paths.
- **What the commit itself caught — and the reason it was the right half of the choice.** The scratch
  copy ran as CommonJS because `C:\tmp\t30` has no `package.json`; the repository root has one that
  declares `"type": "module"`, so the committed `.js` was parsed as ESM and died with
  `ReferenceError: require is not defined` before its first assertion. Renamed to `.cjs` and the
  reason recorded in the file and in §11. A record of commands alone would have shipped a command
  that does not run.
- **Re-run on the current tree, 2026-10-25, from the committed paths.** Built with
  `cargo build --release -p aether-tests --example h2_interop_server` (`Finished release profile in
  3m 16s`, 3,979,776-byte artifact); the tree carries track 58's core-level `101` refusal. Node
  **24/24 twice, exit 0**. Hand-framed **PASS twice**: 212 frames decoded, 100 × `200`, 10 ×
  `RST_STREAM(7)`, 0 GOAWAY, 0 unresolved, post-storm `200 pong`. Per-row values: 50 streams
  325 ms; cap-aware 110 streams 759 ms; 1 MiB upload echoed 1,048,621 B; 5 MiB download 5,242,880 B;
  413 with 0 bytes echoed, next request 200; both header cases 431; strict port 0 bytes on 3/3 ALPN
  cases; `Upgrade: h2c` answered `HTTP/1.1 200`.
- **Not closed by this, stated so the closure is not read wider than it is.** The `python-h2` 20/20
  and the 8/8 hand-framed HEADERS probe cited in the h2c matrix row were different scratch scripts
  (`C:\tmp\h2c_*.py`), are not part of §11's table, and were **not** re-run today; the matrix row now
  says so instead of implying current evidence. Evidence that matters for the same behaviours —
  prior-knowledge cleartext `/ping` 200, the WS route 400 over cleartext, flow control and the 413 —
  is covered by today's committed-client rows over TLS and by the checked-in `http2_e2e` suite.
- Windows/MSVC only — **PROB-0012 stays open**.

### B9. Per-round clippy line numbers in the governance log drift — keep only the stable form — **CLOSED 2026-10-22**
`WORKING_CONTEXT.md` records `serve.rs` clippy locations as (87, 192, 429, 445) in one entry and
(87, 192, 432, 448) in the next; today the same command reports **87, 192, 428, 444**. Each entry was
true when written — the file moved between rounds — so neither is an error, but a reader comparing
two snapshots will find a mismatch. The only statement worth carrying forward is the invariant:
**no `serve.rs` lint location falls inside the upgrade branch (`245`-`330`)**, which is true in all
three snapshots and true now.
- Action: **DOCUMENT** — record invariants, not coordinates.
- Action taken: the invariant is now stated against source anchors instead of numbers. The branch is
  the arm that opens with `if let Some(ws_handler_ref) = self.ws_handlers.get(&handler_id)` and ends
  where `else if let Some(handler_ref) = self.handlers.get(&handler_id)` begins — on the current tree
  those two lines are `serve.rs:239` and `serve.rs:347`, and that span is what the sentence means,
  not those numbers.
- Re-measured 2026-10-22 on the post-track-23 tree:
  `cargo clippy -p aether 2>&1 | grep -oE 'serve\.rs:[0-9]+' | sort -u` → **87, 192, 445, 461**
  (`aether` (lib) generated 13 warnings). The coordinates moved again — the `101` fix inserted code
  above them — and the invariant still holds: none of the four falls between the `ws_handlers.get`
  arm and the `handlers.get` arm. A fourth snapshot would have been meaningless; the anchor is not.
- Reusable form of the check, so a future round measures instead of re-copying:
  `grep -n 'ws_handlers.get(&handler_id)\|self.handlers.get(&handler_id)' framework/aether/src/serve.rs`
  bounds the branch; the clippy line above filters the lint locations.

### B11. The stale-count class B7 described is repo-wide, not one row — **CLOSED 2026-10-22**
Sweeping current-tense test counts after PROB-0014 added three tests, five documents carried a
number the tree no longer supported:
- `framework/README.md:53` — "31 checked-in raw-socket end-to-end tests" (`http_e2e.rs` holds 45) and
  "27 HTTP/2 wire tests … 8 over cleartext … 4 hand-frame"; measured: 29 = 17 TLS + 9 cleartext
  through the `h2` client + 2 hand-framed-only + 1 preface-only, 3 hand-frame HEADERS/DATA/SETTINGS,
  4 also send a plaintext HTTP/1.1 request.
- `framework/docs/FEATURE_COMPLETENESS_MATRIX.md:169` — "Verified by 27 wire tests" → 29 with the
  `grep -c tokio::test` command attached (this is B7's row drifting a second time).
- `framework/docs/FEATURE_COMPLETENESS_MATRIX.md:281-282` — "9 wire tests in `scheduling.rs`" (14) and
  "27 crate tests (17 engine, 8 simulator, 2 stress)" → 57 = 11 unit (`baseline` 3, `engine` 6,
  `policy` 2) + 46 in `scheduler/tests/` (`engine` 21, `sim` 9, `pump_head_of_line` 8,
  `memory_reprice` 4, `policy_extension` 2, `stress` 2).
- `framework/docs/adr/ADR-001-algorithm-strategy.md:22` — "41 `http_e2e` wire tests, 18 middleware
  wire tests (8 + 10 around a WebSocket upgrade)" → 45 and 21 (8 + 13).
- `framework/docs/PRODUCTION_READINESS_REPORT.md` §10 — "27 HTTP/2 wire tests … 6 also assert that a
  plaintext HTTP/1.1 peer … is still served" → 29 and 4, with the composition written out and the
  superseded numbers named in place.
- Root `README.md:20` — "Verified in the current tree (2026-10-21): 455 … 41 … 27" → the tree it
  claims to describe: 462 / 45 / 29, dated to the run.
- One claim could not be settled by measurement and was rewritten rather than re-numbered:
  framework/README said "27 tests that register middleware — 13 + 8 + 3 + 3". Neither the 3 in
  `http2_e2e.rs` nor the 3 in `extensions_e2e.rs` is reproducible: counting the tests whose app ends
  up with a middleware in its chain, whether the `use_middleware` call is inline or in that file's
  own builder (`ext_app`, `marked_app`, `extractor_app`), gives 13 + 8 + 5 + 6 = **32**. The rule is
  now written next to the number. Note the rule is name-sensitive: `extractor_app` also exists in
  `extractors.rs`, where it installs no middleware, so a repo-wide grep for that helper name reports
  15 false positives.
- Method: per-file `grep -c` of the test attributes, then a body slice at column-0 `}` to attribute
  each `use_middleware` call to its test. A naive slice that ends at the *next* attribute silently
  swallowed the file-local helpers and mislabelled the preface test; the corrected slice is what the
  numbers above come from.
- Re-derived again 2026-10-23 after PROB-0008, same method (per-target `test result:` lines of
  `C:/tmp/t25/test5.log`, cross-checked against `grep -c` on the two changed files): **466 = 266 unit
  + 195 integration + 5 doc**, `scheduling.rs` 14 → **15**, `aether` lib 30 → **33** with `app.rs`
  7 → **10**, `http2_e2e` **held at 29** (the 27-vs-29 question this note raised is settled: 29 is
  what the run reports and what the file's attributes count). Five documents updated in that pass:
  root `README.md`, `framework/README.md`, `BASELINE_MEASUREMENTS.md`, `FEATURE_COMPLETENESS_MATRIX.md`,
  `PRODUCTION_READINESS_REPORT.md` — where §5's 2026-10-22 per-target split (262/192/5, `http2_e2e`
  27) is a *different run on an earlier tree* from §2's 462, which is why that section now carries
  both readings instead of one number copied twice.
- Re-derived again 2026-10-25 (track 60), this time from `cargo test --workspace` on the current tree
  rather than from a scratch log (50 `test result:` lines): **488 = 277 unit + 206 integration + 5
  doc**, `http2_e2e` 29 → **36** (19 `H2Client::connect` / 14 `connect_plain` / 3 hand-framed,
  classified per brace-matched test body), `http_e2e` 45 → **48**, `scheduling.rs` 15 → **16**,
  `aether` lib 33 → **39** with `http2.rs` 5 → **8**, and the middleware count 32 → **34**
  (13 + 8 + **7** + 6 — the rule is unchanged; `http2_e2e` grew by two). Seven files updated: root
  `README.md`, `framework/README.md`, `FEATURE_COMPLETENESS_MATRIX.md` (evidence bullet, unit row,
  HTTP/2 and h2c rows), `PRODUCTION_READINESS_REPORT.md` §7/§10, `BASELINE_MEASUREMENTS.md` (its
  per-target table, the one table in that otherwise superseded doc that is kept current),
  `GETTING_STARTED_AR.md`, `FULL_FRAMEWORK_DOCUMENTATION_AR.md`. The drift is structural, not careless: each slice added
  tests and edited its own prose, and no slice owned the aggregate paragraphs. The mitigation this
  item already states now has a companion rule — a slice that adds tests re-runs the count, and a
  corrected count is never published without the command that re-derives it.
- Re-derived again 2026-10-25 (track 62), same method (parse the 50 `test result:` lines of
  `cargo test --workspace` on the current tree): **491 = 280 unit + 206 integration + 5 doc**,
  0 failed, 1 ignored. The only movers are track 62's three new `app.rs` unit tests: `app.rs`
  13 → **16** and the `aether` lib 39 → **42**; every integration target held its count. Five
  current-tense claims swept: root `README.md`, `framework/README.md`, `BASELINE_MEASUREMENTS.md`
  (table header, `aether` row, total row), `FEATURE_COMPLETENESS_MATRIX.md` (evidence bullet,
  unit-test row, per-file split) and `WORKING_CONTEXT.md`'s Active-Track parenthetical. The dated
  per-track records — PRR's 455–474 ladder, `WORKING_CONTEXT.md`'s track-58/60/61 entries and the
  two 488 bullets in this note above — were left as written: each says what its own tree measured.
- Re-derived again 2026-10-25 (track 65), same method (parse every `test result:` line of
  `cargo test --workspace` on the final tree): **492 = 281 unit + 206 integration + 5 doc**,
  0 failed, 1 ignored, across the same 50 result lines. The only mover is track 65's one new `app.rs`
  unit test: `app.rs` **16 → 17** and the `aether` lib **42 → 43** (43 = `tls.rs` 10 + `app.rs` 17 +
  `extensions.rs` 8 + `http2.rs` 8, re-counted per brace-matched body rather than taken from the doc);
  every integration target held its count. Six current-tense claims swept — root `README.md`,
  `framework/README.md`, `BASELINE_MEASUREMENTS.md` (prose, `aether` row with its per-file split,
  total row), `FEATURE_COMPLETENESS_MATRIX.md` (evidence bullet, ladder line, unit split, per-file
  split, unit row) and `WORKING_CONTEXT.md`'s Active-Track parenthetical — plus B19's own
  "still open" line below. Dated per-track records left as written, same rule as above.

### B12. The user-facing docs taught an API that does not exist — **CLOSED 2026-10-22**
B11 fixed *counts*; the same class existed one level down in the *code examples*. Four docs
(`docs/00_framework_specification_en.md`, `docs/00_framework_specification_ar.md`,
`docs/06_middleware_and_performance.md`, `docs/08_getting_started_and_examples.md`) showed calls that
cannot compile against the crate they document:
- `use aether::context::RequestContext` / `use aether_http::response::Response` — no `aether::context`
  module exists; both types come from `aether::prelude::*` (`framework/aether/src/lib.rs:56-75`).
- `Response::json(&serializable)` and `Response::ok(body)` — `Response::json` takes `Bytes` and
  serializes nothing; `Response::ok()` takes no body (`framework/http/src/response.rs:49-136`).
  There is no `.with_json`.
- `app.state_container.insert(..)` and `AetherApp::with_state(..)` — neither is public; state is
  `app.manage::<T>(state)` plus `ctx.state::<T>() -> Option<Arc<T>>` (`framework/aether/src/app.rs:720`,
  `:484`), and `ctx.state.counter` is wrong because the handle is an `Option<Arc<_>>`.
- `app.with_logger(LoggerMiddleware::new().enabled(true))` — no `with_logger`; the switch is
  `app.enable_request_logging(bool)` (`framework/aether/src/app.rs:788-794`).
- `app.get("/x", |ctx| async move { .. })` — the sync `get` requires a non-async handler taking one
  argument (E0593); async closures belong to `get_async`/`post_async` (`:703`, `:735`).
- `/users/:id` — the router only treats `{name}` as a parameter (`framework/router/src/trie.rs:84`).
- `fn main() -> Result<(), Box<dyn std::error::Error>>` — `listen` returns a
  `Box<dyn Error + Send + Sync>` (E0277).
- Method (this is why the item is closed and not merely edited): each example block was extracted
  *from the markdown file itself*, not retyped, into a scratch crate that depends on `aether` by
  path, then `cargo check`-ed (`CARGO_TARGET_DIR` pointed at the gitignored `framework/target`).
  Failures were fixed in the doc, and the block was re-extracted and re-checked, so the committed
  text is the text that compiled — exit 0, no warnings. Three fixes came from the compiler, not from
  reading: `|_ctx|` arity, `String`-based `serde_json::to_string(..)` instead of `.into()` on
  `Vec<u8>` (E0283 — `bytes`, `rustls` and `rustls_pki_types` all impl `From<Vec<u8>>`), and avoiding
  `bytes::Bytes::from` so the example needs no extra dependency (E0433). One pre-existing example
  added in the C1 commit emitted `unused_mut`; its `let mut app` became `let app`.
- Scope limits, stated so this is not read as more than it is: only blocks that form a whole
  `fn main` program were compiled. Non-`fn main` fragments and the two large Arabic docs
  (`FULL_FRAMEWORK_DOCUMENTATION_AR.md`, `GETTING_STARTED_AR.md`) were only scanned for the phantom
  names above; they use `Response::ok()` and `{id}` correctly but were not extract-and-compiled.
  A repo-wide grep for those phantom names now returns only standard-library and `bytes` helper
  names (`acquire_owned`, `from_utf8`, `split_to`, `writer`, …) plus my own "this does not exist"
  sentences. `cargo test --workspace` after the edits: 462 passed / 0 failed / 1 ignored across
  50 `test result:` lines, exit 0. No independent agent was used; no code, test, or configuration
  file was touched — the four changed files are all markdown.

### B13. B12's declared scope limit was the next batch of phantom APIs — **CLOSED 2026-10-22**
B12 stopped at whole-`fn main` blocks in `docs/00/06/08`. The remaining four example-heavy docs
(`framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md`, `framework/docs/GETTING_STARTED_AR.md`,
`framework/README.md`, `docs/09_api_reference.md`) held 9 `fn main` programs and 60 other blocks.
Seven of the nine programs did not compile; the method was extended to fragments: a statement block
is embedded in `async fn frag(app: &mut AetherApp, req: RequestContext, body: Bytes) -> Result<…>`
so `app`/`req` are the real types, and `cargo check` runs against path dependencies.
- `framework/README.md`: **all 7 blocks compile** (verified in a scratch crate whose deps match the
  `Cargo.toml` the README itself shows, including `bytes = "1"`). Correction to the record: my first
  probe reported `bytes::Bytes::from` as E0433 — that was my scratch crate missing a dependency the
  README does declare, not a doc defect. Verified true in that file: the env-var table, the
  `from_env`-is-the-only-zero-guard paragraph, the single-derivation claim, `static_asset`,
  `static_dir_with_cache`, `TestClient`, `use_middleware`, `insert_extension`/`Extension<T>`, `ws`.
- `docs/09_api_reference.md` (the API reference — the file a reader copies from): fixed `Json<T>`
  being listed as a returnable `IntoResponse` (the six impls are `Response`, `&'static str`,
  `String`, `(u16, &'static str)`, `(u16, String)`, `Result<T, E: Display + Send>`; `Json` is an
  extractor), the "`{id}` أو `:id`" claim, four unannotated `get_async`/`post_async` closures
  (E0282 — `Handler<Args>` cannot infer the argument list from the call site), the partial
  `CorsConfig` literal, the missing prelude in the §8 program, and `with_config`'s description, which
  promised workers and "runtime paths" while §6 of the same file says `workers` is parsed and
  displayed but not applied. Verified accurate: every `RequestContext` field and method signature,
  `Response::{ok,text,html,json,json_status,bytes,html_dynamic,text_dynamic,status,redirect,file,header,set_cookie}`,
  `group`, `static_asset`, both `ServerLimits` (10 fields) and `AetherConfig` field lists, and the
  documented `Err` → 400 + `{"error":…,"code":"E5002"}` behaviour (accurate as of that check; track 31
  changed the code on 2026-10-23 to `E4003` while keeping 400, so the docs this bullet cleared were
  re-edited and no longer say `E5002`; **track 43 changed the bound on 2026-10-24** — the six
  implementors are the same set, the sixth is now `Result<T, E: IntoResponseError>` (D9), and 400 is no
  longer what it guarantees: the status is the error's own).
- `FULL_FRAMEWORK_DOCUMENTATION_AR.md`: `ServerLimits { max_headers: 64, …}` (the type is not
  re-exported from `aether` at all, the field is `max_headers_count`, the literal lacked seven
  fields, and the value was never passed to anything) → `AetherConfig` + `with_config`;
  `aether_dependency::DependencyContainer::register/add_dependency/resolve_build_order` and
  `DependencyScope::Scoped` are all fictional — the crate exports `DependencyGraph::add_node` +
  `compile_execution_plan` and `Singleton`/`RequestScoped`/`Transient`; `let arena = …` then
  `arena.reset()` (E0596, `reset` takes `&mut self`); `let trace_id = req.request_id;` and
  `let raw_bytes = req.body;` inside a handler that keeps using `req` (E0382 partial move);
  `-> Result<&'static str, (u16, &'static str)>` (E0277: the tuple is not `Display`, and the impl
  calls `err.to_string()`); a `Response` catalogue block that used `username`/`image_bytes` never
  defined anywhere in the doc; `include_bytes!("../static/css/learning.css")` — no such file exists
  in this repo, so the block only compiles in a project that owns it; and `tera`, which is in no
  `Cargo.toml` in the workspace, now says so explicitly.
- `GETTING_STARTED_AR.md`: the logger was documented as printing timings "تلقائياً" — it is opt-in
  (`enable_request_logging(bool)`, `app.rs:788`), and the shown line format
  `⚡ [Aether] {METHOD} {path} -> {status} ({n}µs)` was verified against `middleware/src/logger.rs:44`;
  `let raw_bytes: &[u8] = req.body;` (E0308 — `body` is `Bytes`; the borrow is `&req.body`); the same
  partial `CorsConfig` literal and the same missing prelude.
- Unmeasured timing magnitudes were removed rather than re-numbered: "تعمل في أقل من 20 نانوثانية"
  (sync handlers), "تصفير فوري في أقل من 1 نانوثانية" (arena reset) and the example page's
  "استجابة بأقل من 1 ميكروثانية". Each site now states the structural guarantee instead, and the one
  real measurement command is named where it applies:
  `cargo bench -p aether-benchmarks --bench router_bench` — which measures the trie, not handler
  execution. The `6 إلى 25 ميكروثانية` figure in `docs/06` stayed: it is a dated past-tense record of
  a specific earlier run, not a current claim.
- Final state: 9/9 `fn main` blocks compile verbatim from the doc text (exit 0, no warnings);
  52/60 other blocks compile in the harness; the remaining 8 are 4 declaration listings copied from
  source (not statements) and 4 that fail only for the documented external reasons above.
- Method limits, stated as run: the harness carries `#[allow(dead_code, unused_variables, unused_mut)]`
  so it proves *type correctness of the documented calls*, not warning-freedom; fragments were
  embedded rather than executed, so nothing here is a runtime verification; and `Cargo.toml`/`toml`
  fences and shell snippets were not compiled at all.
- Prose carry-over the harness cannot see (same class, found by grepping `:id` across all docs after
  the block check): `docs/03_routing_and_radix_trie.md` taught both forms in its precedence list and
  drew `:id (Param)` in its trie diagram, and `docs/00_framework_specification_{ar,en}.md` did the
  same in §4 and in the `param` table row. All five sites now say braces are the only parameter form
  and cite `router/src/trie.rs:84` (`if seg.starts_with('{') && seg.ends_with('}')`), which was
  re-read for this edit, not inherited from the earlier note. The two places that already used `:id`
  as a *counter-example* (`docs/00` ar line ~269, en line ~262) were left alone.
- Re-extraction after every edit: the 69 blocks were re-pulled from the committed markdown and
  re-checked serially (`C:/tmp/t25/run5c.out`) — 9/9 programs and 52/60 fragments, same 4 declaration
  listings and same 4 external-reason failures. A first attempt at this run raced with a background
  copy of itself over one scratch `src/bin/probe.rs` and printed a spurious program failure; the
  numbers above come from the clean serial run, not from the racing one.
- Gate: `cargo test --workspace` exit 0, 50 `test result:` lines, 462 passed / 0 failed / 1 ignored,
  re-run from the final text of all eight files (`C:/tmp/t25/test2.log`; `C:/tmp/t25/test.log` is the
  same command against the first five). Markdown only; no code, test or config file touched; no
  independent agent used. Linux and macOS remain untested — **PROB-0012 stays open**.

### B14. B13 compiled the docs it had already touched and left the architecture pages unchecked — **CLOSED 2026-10-23**
B12/B13 covered the four example-heavy docs plus `docs/00/06/08`. Thirteen markdown files were edited
here: the architecture/specification pages `docs/00_framework_specification_{ar,en}.md`, `docs/02`,
`docs/03`, `docs/04`, `docs/05`, `docs/07`, `docs/10_competitive_engineering_report_{ar,en}.md`, plus
`framework/docs/DEPLOYMENT.md`, `framework/docs/AETHER_CORE_CAPABILITIES.md`, and the two stale
`http2_e2e` wire counts in `framework/docs/GETTING_STARTED_AR.md` /
`framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md`. Five files in the same sweep needed no
correction — `docs/06_middleware_and_performance.md` and `docs/08_getting_started_and_examples.md`
compile as written, and `framework/README.md`, `framework/docs/ERROR_HANDLING_GUIDE_AR.md` and
`framework/docs/09_api_reference.md` fail only on the reader-owned fences listed below.

- Harness (same shape as B13, `C:/tmp/t26` then `C:/tmp/t27`): a scratch crate with path deps on
  `aether` + the eight `aether_*` crates + `bytes`/`serde`/`serde_json`/`tokio`/`smallvec`/`tera`.
  Three passes — whole `fn main` programs verbatim; statement blocks embedded in
  `async fn frag(app: &mut AetherApp, req: RequestContext, body: Bytes) -> Result<…>`; declaration
  listings embedded in `#[allow(…)] mod doc { … }` with a `fn main(){}`, retried through the fragment
  harness when the body turned out to contain statements.
- Final tree, serial run `C:/tmp/t27/run11.out`: **95 rust blocks** across the tracked markdown
  (14 programs, 62 fragments, 19 declaration listings) → **14/14, 48/62, 17/19 compile**.
  Markdown is not a build or doc-test input here (`grep` for `include_str!`/`include!` across
  `framework/*/src` and the test trees returns nothing), so none of this can move a test outcome;
  the 466-test suite from track 27 described the tree as it stood at that commit. Track 29 added
  two `aether-websocket` unit tests, taking the current tree to **468 = 268 unit + 195 integration
  across 15 targets + 5 doc**, 0 failed, 1 ignored (2026-10-23, `C:/tmp/t27/test_ws_467.log`, same
  per-target `test result:` parser that reproduces **466 = 266 + 195 + 5** on `C:/tmp/t25/test5.log`).
  The fence sweep re-run over the tree as B14 left it, `C:/tmp/t27/run12.out`, is byte-identical to
  `run11.out`: **14/14, 48/62, 17/19**. B15 then changed markdown again, so that tally was taken a
  third and fourth time — see B15's sweep bullet, `C:/tmp/t27/run14.log`.
- The 16 blocks that do not compile are two named classes, not an unknown remainder:
  1. **crate-internal excerpt (10)** — `00_ar_1`, `00_en_1`, `03_1`, `03_2` (the one-line body of
     `RouteNode::supports_method`, lifted out of its `impl`; `03_1` additionally is an inherent
     `impl` on a foreign type ⇒ E0116), `02_1`, `04_1`, `05_1`, `05_2`, `07_2`, `07_3`. They fail on
     private names (`ows_trim`, `HeaderLines`, `lines_for`, `status_forbids_body`,
     `write_http_date_now`, `write_sanitized_field_value`), on locals owned by the surrounding source
     (`conn_buf`, `limits_clone`, `chunked_body`, `recv_started`, `conn_semaphore`, `shutdown_rx`), or
     on a `break` whose loop was elided (E0267). Each now carries the `file:line` it was copied from
     and says it is read, not translated.
  2. **reader-owned types or asset (6)** — `00_ar_2`, `00_en_2`, `09_3`, `09_5`,
     `ERROR_HANDLING_GUIDE_AR_2` (`DatabasePool`, `RedisCache`, `CreateItemDto`, `CheckoutDto` are the
     reader's application, not framework API) and `FULL_FRAMEWORK_DOCUMENTATION_AR_24`
     (`include_bytes!("../static/css/learning.css")`; no such file exists in this repo).
- Second method, for the class the compiler cannot price: every one of the 10 crate-internal excerpts
  was re-read against its cited source lines rather than inherited — `parser.rs:129-184` and
  `:594`, `node.rs:41-43`, `method.rs:25-43`, `trie.rs:24-26` and `:38`, `response.rs:206-236` and
  `:238` and `:287`, `app.rs:1377-1384`, `1530-1542`, `1811-1825`, `1936-1951`, `1954`, `1969`,
  `2196`, `app.rs:191-228`, `limits.rs:28-50`, `jwt.rs:82`, `constant_time.rs:6`,
  `runtime/src/worker.rs:133`, `runtime/src/lib.rs:3-9`. Two citations were corrected during this
  check (`app.rs:1934-1947`→`1936-1951`, `response.rs:227-233`→`227-235`) and one merged `if` in the
  `docs/05` §3 excerpt was split back into the two guards the source actually has.
- Phantom APIs found by compiling, all replaced with the code's real shape:
  - `docs/03` §3 taught `pub const METHOD_GET: u64 = 1 << 0; …` and
    `node.allowed_methods_mask & (1 << method_id)`. Neither exists: bits come from
    `Method::bit()`/`bitmask()` and the test is `self.allowed_methods & method.bitmask()`
    (`AETHER_CORE_CAPABILITIES.md` carried the same wrong expression and got the same fix).
  - `docs/04` §2 taught four error variants that are not in `AetherError`
    (`MultipleContentLength`, `InvalidHeader`, `InvalidContentLength`,
    `ConflictingLengthHeaders`) and a boolean `has_transfer_encoding` where the code counts
    (`te_count`). There are no smuggling-specific variants at all (`core/src/error.rs:5-16`).
  - `docs/04` §3 and `docs/10_{ar,en}` credited `subtle::ConstantTimeEq`; `subtle` is not a
    dependency of any crate here (`grep` over `framework/**/Cargo.toml` + `*.rs` returns nothing).
    The real constant-time path is `ring::hmac::verify` plus the project's own `constant_time_eq`.
  - `docs/05` §2 listed 10 of the 12 `RequestContext` fields (`peer_addr`, `extensions` were missing)
    and §3 showed a `format!`-free serializer that is not the shipped one — the public entry delegates
    to `write_http1_response`, which skips caller `Content-Length`/`Transfer-Encoding`, injects `Date`,
    suppresses both header and payload for 1xx/204/304, and sanitises every field value.
  - `docs/07` §2 claimed "كل عملية قراءة" is wrapped in `read_timeout`; the served path picks
    `read_timeout` vs `keep_alive_timeout` first, and §3's `acquire().await.unwrap()` is not what runs
    (a `select!` with a shutdown arm and an explicit `Err(_) => break`, because a closed semaphore is
    a reason to stop accepting, not a panic).
  - `docs/02` §3 attributed the live serving loop to `RequestView::parse`; the serving loop calls
    `parse_request_incremental`, and `RequestView::parse` belongs to the legacy `runtime/worker.rs`.
  - `framework/docs/DEPLOYMENT.md` §4 showed `app.get("/", |_| async { "hello" })`, which does not
    compile — `get` requires `R: IntoResponse` on the *return value*, and an `async` block is a future,
    so an async handler goes through `get_async`. Verified: that fence
    (`DEPLOYMENT_1`) compiles in the fragment harness after the fix.
- Counts re-derived, not copied: `http2_e2e` is **29** `#[tokio::test]` functions — 17 negotiate ALPN
  `h2` over TLS, 9 drive cleartext prior-knowledge h2 through the `h2` client, 3 are raw-frame-only
  (one of which stops at the preface); 4 of the 29 hand-arrange bytes on the wire. Both files that
  said 27 were corrected, and the "لم يغيّر ذلك الإصلاح العدد" sentence in
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md` replaced the stale count with the current one. A first
  classifier returned 0 because it matched only the bare `#[tokio::test]` attribute; the numbers above
  come from matching `^#\[tokio::test` and slicing to the column-0 `}`.
- Not done here, and not claimed: `toml`/shell fences were never executed, fragments were type-checked
  rather than run, and the harness carries `#[allow(dead_code, unused_variables, unused_mut)]` so it
  proves type correctness of the documented calls, not warning-freedom.
- Gate: markdown only — 13 files, +255/−82; no code, test or config file touched; `git status`
  confirms the file list. No independent agent was used. Linux and macOS remain untested —
  **PROB-0012 stays open**.


The same WS sentence appears in `framework/README.md`, root `README.md`, `ARCHITECTURE.md` §5,
`AETHER_CORE_CAPABILITIES.md`, `GETTING_STARTED_AR.md`, `FULL_FRAMEWORK_DOCUMENTATION_AR.md`,
`ADR-001` and the matrix. Counting them as eight verifications is exactly the anti-pattern the
owner named; each was classified from the test/code evidence, once, and the doc copies inherit that
one verdict. B3 is what happens when a copy is trusted instead.

### B15. B14 left two doc classes unverified — non-`rust` fences and line citations — and the citations were hiding four stale findings plus a fifth in this register's own table — **CLOSED 2026-10-23**

- Scope: the classes B14 named as unchecked. (1) non-`rust`-tagged fences, (2) every `file.rs:line`
  citation in tracked markdown, (3) the status tables those citations live in.
- **Citation method.** `C:/tmp/t27/citecheck2.py` walks `git ls-files *.md`, extracts
  `path.rs[:a][-b]`, resolves repo-relative suffix first and basename second over `git ls-files *.rs`,
  and calls a citation suspect only when **every** candidate fails (no such file, out of range, or the
  span holds no identifier). Result: **181 citations across 22 files → 11 suspects**. The first
  version, `citecheck.py`, resolved an ambiguous basename to whichever copy `os.walk` reached first
  and produced four false alarms that way — replaced, not tuned.
- **Re-run over the tree B15 leaves behind: 259 citations across 22 files → 26 suspect lines, which
  are 9 distinct targets** (`C:/tmp/t27/citecheck2.out`). The count is not stable and that is worth
  saying: it passed through 181 → 239 → 248 → 259 across the slice, because the bullet that
  *classifies* a suspect re-quotes the coordinate and so re-raises it. 19 of the 26 lines are in the
  two governance files (15 here, 4 in `WORKING_CONTEXT.md`) and 7 in the five docs files; the 9
  distinct targets fall in two classes and none is an unresolved defect:
  - **out-of-tree, 5 targets** — the four actix dependency-source coordinates, now written
    crate-qualified (`actix-http-3.19.0/src/config.rs:207`, `actix-web-4.17.0/src/server.rs:126`,
    `:665-667`, `actix-http-3.19.0/src/service.rs:28-30`) in `BASELINE-2026-10-19.md` and the register:
    9 suspect lines between the qualified and the older bare form. The checker walks
    `git ls-files` and cannot see the registry. The 5th is `../aether-probe/src/main.rs:98` (4 lines,
    one of them bare `src/main.rs:98` at `WORKING_CONTEXT.md:1213`, whose sentence names
    `aether-probe` immediately before it). Re-read on 2026-10-23: line 98 still is
    `app.get_async("/slow", |_| async {`, exactly the unannotated closure the note claims, so that
    dated claim holds.
  - **kept on purpose as the historical reading, 4 targets** — `app.rs:1507` and `engine.rs:403`,
    each quoted twice inside the sentences that record them as drift, plus `connection.rs:305-307`
    in the F7 Evidence cell and the four `app.rs` coordinates in the design doc's §1, all of which
    the row text and the §1 header now state are the 2026-10-18/pre-fix view. That the §1 disclaimer
    is *true* was checked, not assumed: `app.rs:847` is `&mut self,`, `:848` `method: Method,`,
    `:873` a doc-comment line and `:1157` empty — none is the semaphore, the spawn or the watch.
- **The class citecheck cannot see, and the one it hid.** A citation is two claims — the line is
  alive, and it holds what is named. `citecheck2.py` verifies only the first. The §C table row that
  named the WebSocket setter `with_websocket_limits`, and this register cited it at
  `framework/aether/src/app.rs:1041`, passed every dead-citation test the checker has, while 1041 is
  an argument in the *static-asset* closure several functions earlier and the setter is at `:1056`.
  Found by hand, not by tooling, while checking the F7 wiring claim.
  `C:/tmp/t27/symcheck.py` now closes that gap: where a `file.rs:LINE` sits within two characters of
  a snake_case/camelCase item name, that name must occur within ±8 lines of LINE. Run over this tree
  it reports **0** repo findings; the run that led to the fix reported **1**, and it was this row.
  Non-vacuity proven on a four-row fixture (`C:/tmp/t27/fixture.md`) carrying both the dead
  coordinate and the live one: the dead rows report, the live rows stay silent. The tool also needs a
  second tier — a 34-char window produces **17** further pairings, 4 of them raised by these two
  governance bullets quoting the row they describe — and all 17 were read. None is a defect: the name
  belongs to a *different* citation
  in the same sentence (`lines_for` beside
  `HeaderLines` at `app.rs:239`, `static_dir` beside the `static_file` span at `app.rs:1083-1088`,
  `keep_alive_timeout` beside the `connection_tokens_…` test at `app.rs:2645`) or the preserved
  2026-10-18 Evidence cells (`app.rs:1336/1311/1170/678`). Every one of those was re-opened at its
  cited line and says what the sentence claims — **as re-numbered on 2026-10-23**: the track-31
  insertion moved three of the numbers in this very bullet list (the test `2484`→`2551`,
  `connection_has_token` `2250-2262`→`2317-2329`, the idle deadline `1936-1940`→`1974-1978`), and the
  track-43 insertion moved those same three again (`2551`→`2645`, `2317-2329`→`2411-2423`,
  `1974-1978`→`2068-2072`), which is
  what the sentence below claims and why a coordinate is a *dated* claim: `HeaderLines` is the
  `pub(crate) type` at `app.rs:239`,
  `write_http1_response` is at `response.rs:238`, `allowed_methods & method.bitmask()` at
  `node.rs:42`, `hmac::verify` at `jwt.rs:82`, `tokio::fs::metadata`/`tokio::fs::read` at
  `static_dir.rs:30`/`:38`. One file is deliberately not cleaned: `notes/2026-10-20-chat-paste.md`
  is the owner's pasted transcript, kept locally and never published, so its two stale coordinates
  stay as received.
- **Six of the eleven were checker artifacts, and naming them is the point.** Four
  (`config.rs:207`, `server.rs:126`, `server.rs:665`, `service.rs:28-30` in
  `framework/docs/perf-baseline/BASELINE-2026-10-19.md`) are **actix dependency sources**, not repo
  paths: read from the registry copies (`actix-http-3.19.0`, `actix-web-4.17.0`) all four still say
  exactly what the baseline claims — `tcp_nodelay: None` defaults, the `if let Some(enabled)` gate,
  the `desired_nodelay` passthrough — so the ruling "symmetric, cannot explain the p50 gap" stands.
  They are now written as `actix-http-3.19.0/src/config.rs:207` etc. so the next reader does not have
  to re-derive that they are outside this repo. `../aether-probe/src/main.rs:98` points at the
  sibling downstream crate by design. The `lib.rs`/`engine.rs`/`config.rs`/`scheduling.rs` AMBIGUOUS
  lines are basename collisions in this workspace, not doc defects.
- **Five real drifts, fixed:**
  - `.project-governance/WORKING_CONTEXT.md` Track-12 wiring note cited `app.rs:1507`/`app.rs:1295`;
    both now sit in unrelated code. Per **B9** the numbers were *replaced by symbol anchors* (the
    `ServeOutcome::Response` arm and the `conn_buffer_charge` rejection, both calling
    `write_http1_to_for_request_version`) plus the one grep that re-derives them, rather than
    re-numbered into the next round of rot.
  - `framework/docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md` §1 is headed "Current serve path" and cites
    four `app.rs` coordinates that no longer point at those constructs — and its "no bounded queue,
    no rejection" bullet describes the *pre-scheduler* tree this design removed. Relabelled as the
    2026-10-18 audit snapshot with a pointer to `framework/aether/src/scheduling.rs`.
  - `framework/docs/perf-baseline/BASELINE-2026-10-20-SCHED-ROTATION.md` §1 cited
    `engine.rs:403 promote_active(id)`; `promote_active` no longer exists (the fix this baseline
    documents renamed it) and 403 is a brace. Now anchored to `Engine::pump` + `promote_divisible`.
  - `framework/docs/ACTIX_FAIR_COMPARISON.md` F7 cited `connection.rs:305-307` for a recv path; that
    span is now the tail of `accept_data_frame` and the `complete_message` header.
  - `docs/02`, `docs/00_{ar,en}`, `docs/01`, `AETHER_CORE_CAPABILITIES.md`: the live serving loop was
    still drawn as `RequestView::parse`. `RequestView::parse` (`framework/http/src/parser.rs:44`) is
    the `Content-Length`-only variant and is called only by the unused `framework/runtime/src/worker.rs:34`;
    the loop calls `parse_request_incremental` (`parser.rs:262`, `app.rs:1631`). Diagram boxes and
    bullets corrected (the Arabic boxes needed re-padding twice — the content rows are 42 columns).
- **What the citations were hiding — the real finding.** §6 of `ACTIX_FAIR_COMPARISON.md` is a
  "findings and status" table, and re-checking each row against the code showed **four of its seven
  rows describing defects that current code fixes**:
  - **F4** "admit not cancellation-safe" — the exact mitigation the row asks for exists:
    `QueueToken` (`framework/aether/src/scheduling.rs:359-366`), armed at `:232` to cover the
    pre-guard window, disarmed on every normal exit, and the row's own "Required test" is checked in
    as `aborting_a_queued_admit_leaves_no_stranded_job` (`framework/tests/tests/scheduling.rs:228`).
  - **F5** "`static_file` reads with `std::fs::read` on the worker" — it is now an async route using
    `tokio::fs::read` (`framework/aether/src/app.rs:1083-1088`), tested at
    `static_file_serves_disk_content_over_async_route` (`framework/tests/tests/http_e2e.rs:674`).
  - **F6** keep-alive conformance — three of four sub-claims closed: per-`Version`
    `close_after_response` (`framework/aether/src/serve.rs:134-145`), the frontend stating the
    socket's fate (`app.rs:1754-1757`), comma-tokenised `Connection`
    (`app.rs:2411-2423`, unit test `app.rs:2645`), `keep_alive_timeout` as the between-request idle
    deadline (`app.rs:2068-2072`). Wire tests `http10_requests_close_after_one_response_by_default`
    (`:703`) and `http10_explicit_keepalive_persists_and_is_advertised` (`:756`). The fourth is
    registered as **D7**.
  - **F7** "WebSocket session has no recv timeout" — `WebSocketLimits::read_timeout`
    (`framework/websocket/src/connection.rs:30-42`, 120 s) bounds each socket wait in `recv`
    (`:146-149`), is handed to every upgraded session (`app.rs:1896-1900`) and is operator-tunable
    (`app.rs:1147-1150`). **Nothing proved it**, so the row was not just rewritten: two tests were
    added (see the code slice), and F1's "(259 tests)" was dated as the 2026-10-18 reading.
  - The Finding and Evidence columns stay verbatim as the 2026-10-18 record; only Classification and
    Status moved, and §6 now says so above the table.
- Non-`rust` fences: `C:/tmp/t27/otherlang.py` listed every tag in tracked markdown; 4 `text` fences
  hold rust-like bodies and all four are explicitly labelled excerpts or reader-owned sketches, so no
  change. The 9 toml/bash fences were read line by line against the tree for the first time:
  `docs/DEPLOYMENT.md`'s `[profile.release]` matches `framework/Cargo.toml:47-55` (incl. `panic =
  "unwind"`, whose cited test `handler_panics_are_isolated_to_one_response` exists at
  `framework/tests/tests/http_e2e.rs:943`); `basic_api` and `scheduling_demo` are real `[[bin]]`
  targets (`framework/examples/Cargo.toml:14-24`); the `h2_interop_server` command was actually run
  here (`cargo build --release -p aether-tests --example h2_interop_server`, 2m36s, artifact
  3,958,784 B) and that build is what surfaced D6's live cost — `Compiling aether-runtime` on every
  build; `BASELINE-2026-10-20-SCHED-ROTATION.md`'s `git revert f909c78` names a real commit.
- **What the docs promise about tooling — registered as D8, not fixed.** Sweeping the toolchain
  sentences turned up no MSRV anywhere: `git ls-files "*Cargo.toml" | xargs grep -c rust-version`
  reports 0 for all 19 tracked manifests, while `framework/README.md:185` says the verified toolchain
  is rustc 1.98.1 (matching `rustc --version` here). The sentence is accurate as a record; it is not
  a floor, and declaring one across 19 crates plus downstream `my_cool_site` is an owner call.
- **Code slice**: `framework/websocket/src/connection.rs` +37 lines inside `#[cfg(test)] mod tests`
  only — `silent_peer_is_failed_by_the_idle_deadline` and `a_ping_resets_the_idle_deadline`. The
  second one is the load-bearing half: it asserts a ping resets the deadline, so a future change to a
  session-wide deadline fails it instead of silently killing heartbeats. Non-vacuity argument rather
  than a mutation run: `WsError::IdleTimeout` is constructed at exactly one place
  (`connection.rs:149`) and `grep` shows no other producer.
- Verification, as run: `cargo test -p aether-websocket` ×3 → 25 passed each, and the two new tests
  appeared in both orders across the repeats; then each test **alone**, which is where the handover
  command I first wrote turned out to be wrong. `--exact <bare function name>` matches nothing —
  `--exact` compares against the full `connection::tests::…` path — and libtest prints
  `test result: ok. 0 passed; 24 filtered out` with exit 0 anyway, so the command I handed the owner
  would have looked like a pass while running zero tests. Fixed at the source, not in the claim: the
  full paths were re-listed with `--list` and each test re-ran alone (1 passed each), then five
  repeats of the crate with alternating order for the two timing assertions. `cargo test --workspace` → **468 passed / 0 failed /
  1 ignored** = 268 unit + 195 integration across 15 targets + 5 doc; the parser
  (`C:/tmp/t27/counts.py`) reproduces **466 = 266 + 195 + 5** on `C:/tmp/t25/test5.log`, so the split
  is the same method, not a new one. `cargo check --workspace --all-targets` → exit 0, 0 warning
  lines. `cargo clippy --workspace` → exit 0, 46 `^warning` lines, 16 distinct lints, none in the
  `correctness` group, 0 errors; `--all-targets` → exit 0, 0 errors, 97 top-level warning lines
  (114 by the per-target summaries), `aether-websocket` contributing none. `cargo fmt` not run — D4
  records it as a non-gate and the websocket crate alone already had pre-existing diffs. The doc
  fence harness was re-run over this tree too (`C:/tmp/t27/run14.log`, after `run12`/`run13`): still
  **95** rust blocks, **14/14** programs, **48/62** fragments, **17/19** declaration listings, and the
  failure *name list* is set-identical to `run11.out` — the same 16, none added by B15.
- Count sweep: 466 → 468 updated in root `README.md`, `framework/README.md`, `BASELINE_MEASUREMENTS.md`
  (its `aether-websocket` row 23 → 25), `FEATURE_COMPLETENESS_MATRIX.md` (×2), `PRODUCTION_READINESS_REPORT.md`
  (×4) and the WORKING_CONTEXT status line. Every older figure in those same sentences is kept and
  labelled as its own reading — 455/458/459/462/466 — because they are history, not errors.
- Limits: no runtime verification of docs (B14's same limit); the new tests exercise
  `WebSocket::recv` over a `DuplexStream`, not a real socket and not the app-level upgrade path, so
  "wired into every session" rests on reading `app.rs:1896-1900`, not on a wire test; the F4/F6 rows
  inherit tests that already existed rather than new ones. Windows/MSVC only — **PROB-0012 stays
  open**, and no independent agent was used for this slice.

### B16. A citation that resolves through a moving ref stops reproducing the moment that ref moves — **CLOSED 2026-10-23**
B9 retired *line numbers inside a file*; the same failure exists one level up, in citations that name
a **ref** instead of a commit. Three documents said the `unused_io_amount` defect was "pre-existing at
HEAD (verified with `git show HEAD:framework/tests/tests/tls_e2e.rs`)", and each sentence was true on
the day it was written. `907dfcd` then landed the fix, HEAD moved past it, and the command stopped
printing `tls.read(&mut byte)` — measured 2026-10-23: `git show HEAD:…| grep -c 'read(&mut byte)'` →
**0**, while `git show 907dfcd^:…` (that parent is `c847261`) prints it at **line 452**. A reviewer
following the instruction got evidence that *contradicts* the claim it was meant to support, which is
worse than a dead line: a dead line is visibly broken, this one looks confirmed.
- Action taken: all three sentences now cite `git show 907dfcd^:framework/tests/tests/tls_e2e.rs` and
  say why the bare `HEAD` form no longer reproduces — the register (B1), `FEATURE_COMPLETENESS_MATRIX.md`
  item 16 and `PRODUCTION_READINESS_REPORT.md` §5.
- Rule for the next round, stated as an invariant: **a documented command may not resolve through
  `HEAD`, `main`, `origin/main` or a branch name when the fact it proves is about a past state.** Pin
  the SHA, or pin the symbol and let the reader grep. The two `rev-list` rows in `WORKING_CONTEXT.md`
  that already tell the reader "re-run this rather than trust the number" are the acceptable shape —
  they cite a moving ref for a *current* fact and say so.
- Sweep as run: `git grep -n "git show HEAD:" -- '*.md'` → **5** sentences that used a past state as if
  HEAD still pointed at it (the three `tls_e2e.rs` ones plus `WORKING_CONTEXT.md`'s scheduler
  falsifiability line and this register's own §F item 4). All five now carry their pinned SHA and the
  measurement that shows the bare form no longer reproduces; after the edits the same `git grep`
  returns **9** lines, of which **4** are these corrections talking about the retired form and **2**
  are the two stale originals sitting directly above their own correction block. A count of a *search
  for text I had just written* is not a property of the tree — the stable statement is the five
  sentences and their SHAs, not the hit count.
  `git grep -nE "git (checkout |reset --hard|rebase |stash)" -- '*.md'` → 2 lines, both governance
  records of what was *not* done, not instructions. The two `rev-list` rows that tell the reader to
  re-run rather than trust the number are the acceptable shape: a moving ref cited for a current fact.

### B17. Several documented verification commands are true but not reproducible as written — **CLOSED 2026-10-23 (for the ones this round could price)**
The owner caught a handover command of mine (`cargo test -p aether-websocket -- --exact <bare name>`)
that ran **zero** tests while printing `test result: ok.` and exit 0. That made every other documented
command suspect, so the ~200 claim-bearing read-only commands in tracked markdown were extracted with
their surrounding prose and run or priced. Findings:
- `grep -rn "cfg(unix)" framework` (in `WORKING_CONTEXT.md`'s PROB-0012 note) **is** correct — exactly
  one match, `aether/src/app.rs`, re-verified 2026-10-23 at **2380** (it sat at 2313 before this
  slice's insertion) — but it walks `framework/target/`, which
  on this tree is **44,378 files / 15.3 GiB**, and produced no output in 120 s. The forms that finish
  are `grep -rn --include=*.rs …`, or `git grep -n "cfg(unix)" -- framework` (1 line, instant). Same
  class as B16: the sentence is fine, the *instruction* is not executable by a reviewer on a built tree.
- Commands whose pattern relies on BRE alternation were re-checked with and without `-E`, because a
  `|` in a basic regex is literal and "returns nothing" then proves nothing. `grep -rn
  "max_requests|requests_per_connection" --include=*.rs framework` (ACTIX comparison F6) returns
  nothing in **both** forms, and `grep -rnE "max_requests|requests_per_connection" …` also returns
  nothing, so the absence claim stands and the vacuous-form risk is closed rather than assumed.
- Reproduced exactly as documented on the current tree: `grep -c tokio::test tests/tests/http2_e2e.rs`
  → **29**; `… tests/tests/scheduling.rs` → **15** (both superseded later the same day by this
  slice's wire tests: `http2_e2e` is **30** and `scheduling` **16** by the identical command);
  `git ls-files "*Cargo.toml" | xargs grep -c
  rust-version` → **0 for all 19** manifests (D8; note the pipeline exits **123**, which is xargs
  reporting grep's misses, not a failed check); `git ls-tree -r --name-only main | grep -c
  '^framework/target/'` → **0**, and the same on `origin/main` → **0**;
  `grep -rn "aether_runtime" --include=*.rs framework` → **nothing** (D6 — the only `aether-runtime`
  edges are the `aether/Cargo.toml:12` declaration and the crate's own `runtime/Cargo.toml:2` name);
  `grep -rn "extension" scheduler/src/*.rs` → **nothing** (PROB-0016);
  `grep -n "session.abort()" aether/src/app.rs` → one site; `grep -n "fn with_websocket_limits"
  aether/src/app.rs` → **1056**, the anchor B9's §C row now carries; `grep -n "conn_buf.len() as u64"
  aether/src/app.rs` → nothing, all three sites use `conn_buffer_charge`;
  `grep -n write_http1_to_for_request_version aether/src/app.rs` → **1675, 1763** (re-run 2026-10-23;
  the second site was 1732 before this slice moved it), the two sites
  `WORKING_CONTEXT.md` names.
- Two count claims re-measured and found stale against their own current-tense wording, now corrected
  in place: `cargo fmt --all --check` is **574 hunks across 66 files** today (the matrix still said
  567, which was the 2026-10-22 reading — governance already carried 574), and the `serve.rs` clippy
  invariant re-read from the 2026-10-23 log gives **87, 192, 445, 461** with the upgrade branch at
  **239-346**, i.e. unchanged from the sentence that cited 2026-10-22 — verified, not copied.
  The lib-scope breakdown also reproduces: 7 per-crate summaries summing **39**, **46** `^warning`
  lines, **16** distinct `clippy::` lints, `grep -c "can be collapsed"` → **19** `collapsible_if`
  warnings on top, 0 errors — all re-run 2026-10-23 on the response-block tree and identical. It
  reproduces **only from a cold cache**: see **B18**.
- Limits of this sweep: it *runs* commands, so it proves a command emits what the prose says it emits —
  it does not verify the underlying behaviour, which is B14/B15's method and §11's external clients.
  Commands that need a Linux/macOS host (the `cfg(unix)` arm itself) and every mutating instruction
  (`git revert`, `cargo clean && cargo build --release`, `git stash`) were read for scope, not executed.
  Windows/MSVC only — **PROB-0012 stays open**; no independent agent was used.

### B18. `cargo clippy --workspace` reports a different lint count depending on the build cache — **CLOSED 2026-10-23**
Found while re-measuring B17's clippy row on the response-block tree. The command ran twice with
**9** `grep -cE '^warning'` lines and **2** distinct `clippy::` lints; after forcing a recompile it
printed **46** and **16**, which is what every doc row carries. Cause: clippy caches per crate and a
warm replay re-emits only the `generated N warnings` summary, not the individual warnings — so **39**
(7 lib-scope summaries) is the only figure stable across both states, and the other two silently
improve toward zero on a fully warm tree. The failure mode is the dangerous one for a gate: a
reviewer who adds a lint and runs the documented command twice can be shown *fewer* warnings than the
baseline.

Measured method on the 2026-10-23 tree: `cargo clippy --workspace` warm → 9 lines / 2 lints / 39
warnings / 0 errors; then
`find . -path ./target -prune -o -name "*.rs" -print | xargs touch && cargo clippy --workspace` →
**46** / **16** / 39 / 0 errors / 0 `correctness`; `cargo clippy --workspace --all-targets` cold the
same way → **97** lines, 22 summaries summing **114**, 0 errors, 0 `correctness`.
`cargo check --workspace --all-targets` is unaffected in practice (0 warning lines both ways, exit 0)
because rustc replays its diagnostics, and `cargo test --workspace` prints its tallies from the test
binaries, so B17's reproduction of the two identical 474-test runs stands.

Now documented where it can mislead: the gate table in
`framework/docs/PRODUCTION_READINESS_REPORT.md` carries the caveat under the two clippy rows. Any
future "no new lints" claim must state that the cache was forced cold, and a handover command must
include the `touch` step or it is not the measurement the doc describes. Same class as **B16/B17**:
the sentence is true, the instruction is not executable as written. Windows/MSVC only — **PROB-0012
stays open**; no independent agent was used.

### B19. The phantom-API method cannot see a phantom *output sample* — FIXED 2026-10-25 (track 64)
B12–B15 killed the phantom-API class by extracting every `rust` fence from the docs and compiling it.
That method has a blind spot it never named: a fence tagged `text`/`console` is **pasted output**,
nothing compiles it, and a reader trusts it more than any prose because it looks like the program
actually ran. `framework/docs/ERROR_HANDLING_GUIDE_AR.md` §2a carried exactly such a block — a startup
banner reading `Rust Native Zero-Copy Engine`, `Workers : 8 (Thread-per-Core Async Epoll)` and
`Allocator : RequestArena (O(1) Bump Reset - Zero Heap)`. Three claims, all three false on this tree:
the runtime is tokio work-stealing (track 62), there is no epoll in a Windows/MSVC-only build
(**PROB-0012** still open), and no request arena is in the serving path — `aether_core::RequestArena`
exists as a type (`core/src/arena.rs:24`) and is used by nothing outside a doc sentence saying it is
"available" (`aether/src/lib.rs:17`), which is the opposite of the banner's claim that every request
is bump-allocated on it. §2b's E1001
sample was the same class of error: `os error 98` and a different field order — a Linux errno printed
from a doc that documents a Windows-only product.
- **Method, since the fence cannot be compiled:** re-run the command that would have produced it and
  paste what comes out. Both samples were replaced with captures taken during the track-64 ladder from
  the checked-in binary (`AETHER_PORT=3211 AETHER_WORKERS=1 framework\target\release\basic_api.exe` for
  the banner; the same binary given a second listener on 3212 for E1001 → `os error 10048`, exit 1), and
  the three refuted
  claims are named in Arabic next to the replacement so the correction is legible in the doc's own
  language rather than only in this register. (Those two captures are track-64's; §2b has since been
  re-captured again on 2026-10-25 after the PROB-0024 fix changed the message itself — see the last
  bullet.)
- **Rule for the rest of this class:** every pasted-output fence is a claim about runtime behaviour and
  is falsified by running, not by type-checking the code around it. `ERROR_HANDLING_GUIDE_AR.md` §2a/§2b
  are now the only two converted; the other docs' samples were not swept, and a sweep should start with
  `grep -rn "```text\|```console" framework/docs`.
- **Same class, now closed: PROB-0024** — the E1001 hint told the user to run `lsof -t:i:$AETHER_PORT |
  xargs kill -9`, commands that do not exist on the only platform this tree builds, with the port
  hard-coded to 3000 even when a different one failed. Fixed 2026-10-25 (track 65) in the product, not
  in the doc: the `Try:` lines are now built per platform and per failed port by `port_change_advice`
  and `port_holder_advice` in `framework/aether/src/app.rs`, and the Windows pair is
  `netstat -ano | findstr LISTENING | findstr :<port>` then `taskkill /PID <pid> /F`. The `LISTENING`
  filter is not decoration — measured on this box, the looser `netstat -ano | findstr :3215` returned
  two PID cells for one port (the real listener plus a `TIME_WAIT` client row whose PID column is `0`),
  so the advice as first written could send a user to kill a phantom. Caught only because the advice
  was executed for real. This is the same lesson as the banner two bullets above: a pasted-output claim
  is falsified by running it, and so is a pasted *instruction*.

### B20. A banner that promises "current coordinates" is itself a coordinate claim, and it went stale silently — FIXED 2026-10-25 (track 65)
`framework/docs/ACTIX_FAIR_COMPARISON.md` §6 opens by declaring its Finding/Evidence columns
verbatim-2026-10-18 and therefore allowed to be dead, while promising that the Status column of every
closed row "says so with the **current** coordinates". On 2026-10-25 that promise was false for 6 of
the 8 `app.rs`/`serve.rs` coordinates in those Status cells. Measured, both ends, on HEAD *before*
this track's edit (so none of it is caused by track 65 — it is the track-24→62 insertions moving the
lines under a table nobody re-read): `serve.rs:134-145` now sits inside the logger's `start_time`,
`app.rs:1754-1757` in a `RequestParts` literal, `app.rs:2411-2423` in `reprice_conn_memory`,
`app.rs:2645` in a `max_body_bytes` assertion, `app.rs:1896-1900` in the 503-shedding write and
`app.rs:1147-1150` one line above the `with_websocket_limits` doc comment. Only `app.rs:1083-1088`
and the six non-`app.rs` anchors (`connection.rs:30-42/146-149/604/617`, `static_dir.rs:30-38`,
`http_e2e.rs:674/703/756`, `scheduling.rs:232-234/359-366`, `tests/scheduling.rs:228`) still landed on
what they name — checking all of them is what made the difference provable rather than assumed.
- **Fix shape, per B9/B16:** the dead ones are now anchored on the *construct* — `match version` in
  `serve.rs`, the `if close_after_response` block, `connection_has_token`, `read_limit`,
  `WebSocket::from_parts`, `with_websocket_limits` — with the 2026-10-25 numbers beside them and the
  §6 banner rewritten to say the numbers are a dated measurement, not a standing promise. So a reader
  who greps the symbol gets the construct even after the next insertion, and knows exactly how old the
  line number is.
- **The generalisation this track should not quietly drop:** a doc that separates "historical, allowed
  dead" from "current" columns has created a claim about its own freshness, and that claim needs a
  re-check the same way a count does. B11's rule (a slice that changes a number re-runs the count)
  extends to: **a track that inserts into a file cited by a table re-derives that table, or the table's
  freshness banner must stop asserting currency.** Tracks 24 through 64 all inserted into `app.rs`.
- Scope of this pass: `ACTIX_FAIR_COMPARISON.md` §6 only. The same class was *not* swept across the
  other Status-column tables in `FEATURE_COMPLETENESS_MATRIX.md`/`PRODUCTION_READINESS_REPORT.md`, and
  `PROB-0012` (Linux/macOS never run here) plus `PROB-0023` remain open; `PROB-0022` closed on
  2026-10-25 (track 67, below).

### B21. A gate reading taken from a command that aborted early is not a reading — `cargo clippy --workspace --all-targets` never reaches `aether` — CLOSED 2026-10-27 (the condition ended with track 73 slice 2; recorded by track 88)
Track 67's record — and the three before it — state "clippy at exit 101 with **no finding naming
`http2.rs`**" as a gate for a slice that only edited `http2.rs`. Re-running the same command on this
tree shows why that sentence carries no information: `cargo clippy --workspace --all-targets` with
`-- -D warnings` appended exits 101 (re-run on the final track-68 tree: 275 lines of output, 24
`^error` lines, the last one `could not compile aether-http (lib test) due to 15 previous errors`),
failing inside `aether-validation`, `aether-scheduler` and `aether-http` while
cargo never prints `Checking aether v` (0 occurrences). A crate that was not linted cannot have produced a finding, so "no finding names
`http2.rs`" was vacuously true and would have stayed true no matter what was written there. It is the
same probe-stack class as D4's zero-diff fmt reading and B18's cache-dependent count: **the exit status
proves the command ran, not that it reached the file under review.**
- The reading that does reach it: `cargo clippy -p aether --all-targets --no-deps -- -D warnings` →
  exit 101, 13 findings on the lib target and 16 on the lib-test target, **4 of them inside
  `http2.rs`** — `collapsible_if` at `:448` (the per-frame reprice in `read_body_buffer`), `:493` (the
  authority→`host` fill), `:603` (the default `date` header) and `field_reassign_with_default` at
  `:899` (an existing limits test). All four constructs exist verbatim at HEAD (`git show
  HEAD:framework/aether/src/http2.rs` matches each one), so none came from track 68, and the four were
  left alone: fixing them is a lint sweep, not a pricing change. What track 68 *added* is zero — its
  one new `if let` is a single-level binding and its limits test uses struct-update syntax, neither of
  which those lints fire on.
- **Not verified, stated:** the pre-existing count of 4 was established by comparing the constructs at
  HEAD, not by running clippy over an exported HEAD tree. The rule this item registers is the one worth
  keeping: **a per-file "no finding names X" claim must come from a command whose own output shows X's
  crate being checked**, and a workspace command that stops early should be reported as "did not reach
  the crate", never as "clean there".

  **Closed 2026-10-27 (track 88).** The condition, not the rule, is what closed: track 73 slice 2
  cleared the debt that made the workspace command abort, so `cargo clippy --workspace --all-targets
  -- -D warnings` now exits 0 over every crate and is the gate `gates.sh` runs. A cached green run
  prints only `Finished … in 0.48s` with no `Checking aether` line; that is cargo's freshness
  fingerprint over the same clippy driver, not a crate that was skipped — the earlier hazard was an
  *abort*, which leaves a crate un-linted while the log looks like a verdict. The rule survives for
  any narrower command: if the output does not name the crate under review, the reading says nothing
  about it.

### B22. A dated-record marker exempted a document that later tracks kept editing — CLOSED 2026-10-30 (track 104)
`framework/docs/PRODUCTION_READINESS_REPORT.md` was named in `SNAPSHOTS` and given
`<!-- cite-check: snapshot 2026-10-18 -->` by track 92, as a dated record whose coordinates cannot be
re-checked against a later tree. Four later tracks then edited the same file (tracks 94, 95, 97 and
98b: `6fc3375`, `5b1cdce`, `ca86b83`, `4e005f1`, `2cb2fc7`), and its body now mixes both kinds of
claim: current-tense statements about today's matrix ("The 2026-10-29 recount reads …", "the matrix
row at `FEATURE_COMPLETENESS_MATRIX.md:134` is the current statement") next to statements that were
true of a named past tree and cannot be made true of this one ("on `e81efed` the assertion sits at
`http2_e2e.rs:1502`", "the pre-existing `clippy::unused_io_amount` at `tls_e2e.rs:452`").
- The consequence is that the document is neither honest as a record — it is edited as if live — nor
  checked as a live document: its 22 citations are exempt, and an edit that silently broke one would
  pass the gate. The same round found the marker's arithmetic had already drifted: §3 printed
  135 rows / 112 / 5 / 16 and the export it describes had moved on.
- **What track 98c did, and did not do.** It annotated the CSRF statements with the document's own
  "Superseded …" pattern rather than rewriting history, corrected the two status counts to 136 rows /
  115 / 5 / 14 as recounted from the matrix's own rows, and left the marker in place. Removing it now
  would turn 22 citations red, most of them historical ones whose "correct" span no longer exists.
- **The resolution is a dedicated track:** either split the file into a frozen 2026-10-18 report
  (marker, no further edits) plus a live readiness page, or convert its historical citations to
  commit-anchored prose and pin the live ones. Until then the marker is a claim to distrust, and the
  file may not be edited silently again — an edit either carries a dated supersession note, as this
  track's did, or rewrites the whole document and the marker together.

- **Closed 2026-10-30 (track 104), by the second of the two options.** The marker and the
  `SNAPSHOTS` entry are gone; every historical coordinate became "line N of `<file>`" prose, anchored
  to a commit where one names the tree; and the five live ones were re-derived and pinned —
  `framework/aether/src/http2.rs:896` and the four `framework/aether/src/app.rs` spans. All five had
  drifted while the marker exempted them. The split option was refused because two files carrying the
  same live status is this repository's documented drift hazard. The change is recorded in Track 104.

---

## C. Gate-5 claims that held up (with the measurement, so they can be re-checked)

| Claim | Verdict | Evidence |
|---|---|---|
| 455 passed / 0 failed / 1 ignored on the final tree | VERIFIED | `cargo test --workspace` after the `touch`-forced recompile; per-target sum 262 unit + 188 integration + 5 doc = 455, matching §2's split |
| the 1 ignored is the `http/src/response.rs:584` perf microbench | VERIFIED | `#[ignore = "perf microbench; …"]` |
| `cargo build --workspace --release` with no warnings | VERIFIED | 2026-10-21 run, `Finished release … in 2m 38s`, zero `warning` lines in the log |
| `cargo check --workspace --all-targets` clean, exit 0 | VERIFIED | 2026-10-21 run on the final tree: exit 0, zero `warning`/`error` lines. Note this is *not* the clippy scope of B1: rustc does not run the `deny`-by-default clippy correctness lint, so `tls_e2e.rs:452` passes `check` and fails `clippy --all-targets` |
| clippy, lib scope: 39 warnings, none correctness-class | VERIFIED | `cargo clippy --workspace` → exit 0, family list in B1 |
| WS defaults 16 MiB message / 16 MiB frame | VERIFIED | `websocket/src/connection.rs:38-39`; re-priced into the scheduler at `serve.rs:306-308` |
| `max_concurrent_streams` (100) and `max_headers_count` (100) enforced but not operator-configurable | CLOSED 2026-10-22 | was `http/src/limits.rs:15,42` vs `app.rs:567-572` (only 5 fields mapped from `AetherConfig`); now both are `AetherConfig` fields + `AETHER_*` env vars, and `with_config`/`apply_config` share one `AetherApp::limits_for` derivation. `max_connections`, `read_timeout` and `tls_handshake_timeout` still have no public setter |
| `WebSocketLimits` **is** operator-configurable | VERIFIED | `AetherApp::with_websocket_limits` — locate with `grep -n "fn with_websocket_limits" aether/src/app.rs` (this row cited `app.rs:1041` until 2026-10-23, when a hand check showed 1041 is an argument in the static-asset closure several functions earlier; the real signature is at `:1056`, doc comment from `:1053`. Per **B9** the row now carries a symbol anchor instead of a fresh coordinate) |
| subprotocol negotiation / permessage-deflate not implemented | **HALF-CLOSED 2026-10-29 (track 95)** | negotiation delivered: `ws_subprotocols` on `AetherApp`/`RouteGroup`, `websocket/src/handshake.rs::select_subprotocol` (RFC 6455 §4.2.2, the route's declaration order is the preference), the echoed line written by the `101` in `aether/src/app.rs`, and `400` on an unmatched offer. Permessage-deflate stays open (owner-deferred), so `websocket/src/lib.rs` now says only that |
| TLS: OCSP attach-only, no revocation enforced | VERIFIED then, **CLOSED 2026-10-30 (track 101)** | the module doc of `aether/src/tls.rs` described the limit, and `DEPLOYMENT.md` plus the matrix row stated it. Track 101a added `TlsCertSet::with_crl` (issuer-signed PEM/DER list, its own window, leaf serial) and track 101b made `with_ocsp` validate the staple (successful state, CertId serial and both issuer hashes, `good` status, `thisUpdate`/`nextUpdate` window, GeneralizedTime fields, issuer or authorised-responder signature); 15 revocation unit tests (six CRL, nine OCSP), 13 OCSP mutants and five CRL mutants. The remaining limit is the honest one: nothing fetches from a CA, so refresh is the operator reloading the set |
| JWT not reachable from `aether` | VERIFIED | `aether/Cargo.toml` dependencies list has no `aether-security` |
| fuzz target compiles, was never fuzzed | **CLOSED 2026-10-30 (track 102b)** | was `tests fuzz_targets\http_parse_fuzz.rs … 0 passed` and no corpus anywhere; now 12 committed seeds under `framework/fuzz/corpus/http_parse/`, a seeded runner with outcome buckets and crash artifacts (`framework/fuzz/src/lib.rs`), 15 843 328 executions in 120.011 s at seed 20261030 with 0 crashes, and the oracle proved to fire by a deliberately injected chunk-boundary panic caught at execution 6 507 |
| extension payloads are not priced by the scheduler memory budget (PROB-0016) | VERIFIED | `grep -rn "extension" scheduler/src/*.rs` → no matches |
| the read buffer's reserved block, not its length, is the memory a large upload really holds (PROB-0008) | VERIFIED by falsification, 2026-10-23 | with the read site temporarily on `conn_buf.len() as u64`, `buffered_body_is_priced_by_the_block_the_allocator_handed_out` failed and its `--nocapture` poll trace held `registered_memory_bytes` at **1,474,617 B** for **1,441,792 B** uploaded; on the capacity charge the same test passes (`scheduling` target 15/15). The companion release policy is equally measured: replacing only its `if` condition with `if false` makes the pre-existing `idle_sockets_are_charged_what_they_hold_not_what_they_could_reach` fail at `registered_memory_bytes: 1073031`. Both temporary edits were reverted and the revert confirmed by `grep` before the gate run |
| extensions slice test counts (8 unit / 6 wire / 3 over h2 / fresh-map non-alloc) | VERIFIED | `grep -c` on `aether/src/extensions.rs` → 8; `tests/tests/extensions_e2e.rs` → 6; `http2_e2e.rs:1399,1423,1460`; test at `extensions.rs:148` |
| live WS session at shutdown is aborted, no close frame | VERIFIED as code, **tested on the wire since 2026-10-22** | `tokio::spawn(handler(...))` in the WS handoff arm of `aether/src/app.rs`, and `session.abort()` (= `JoinHandle::abort`) in its `conn_shutdown` branch — locate with `grep -n "session.abort()" aether/src/app.rs` rather than a line number, which drifts (B9). Pinned by `shutdown_aborts_a_live_websocket_session_without_a_close_frame` (echo round-trip proves the session is live, then EOF with zero bytes and the handler's post-loop counter still `0`). Falsified by detaching instead of aborting: the client's post-shutdown read timed out. §3 still counts "graceful close on shutdown" as missing, and that is unchanged |
| Linux/macOS | UNVERIFIED, environmentally blocked | `PROB-0012` open; no WSL, no container runtime on this host |

---

## D. Open gaps that are not defects (keep visible, don't re-litigate)

D1. `PROB-0012` — no Linux/macOS run of any kind. Every number in this file is Windows-only.
D2. ~~No wire test for: 405-before-chain on a WS route (B5), chain-skip assertion on the h2 `400`
    (B6), a hand-built `101` with no handoff (A4), shutdown-abort of a live session (C, last row).~~
    **All four closed 2026-10-22**, each falsified against its own target fault — see A4/A5, B5/B6
    and `PRODUCTION_READINESS_REPORT.md` §14 for the transcripts. The suite moved 455 → 458.
D2b. New decisions opened by that work: whether to keep the WebSocket branch's duplicate
    method guard (A5) — **decided 2026-10-22: keep it, each layer documented**; the behaviour choice
    for a hand-built `101` (A4, candidate (a) measured surgical) — **decided 2026-10-22: refuse with
    `500`, implemented and tested, suite 458 → 459**; and whether graceful shutdown should send a WS
    close frame instead of aborting the task — **closed 2026-10-27 (track 86)**: shutdown now asks
    the session to write `1001 (going away)` and end on its own terms, with a one-second grace
    before the abort. The test this line was written beside pinned the old abort as *current*
    behaviour, which is exactly why "current" was not the same as "right".
D2c. Opened while implementing A4: a middleware-authored `101` on a **non-WebSocket** route is still
    served (A6), and the A4 refusal adds no extra log line — `LoggerMiddleware` already records the
    `500` as the response status, and a per-request `eprintln!` there would be an unbounded
    client-influenced stderr write.
D3. Owner decisions still unmade (re-read against the ledger on 2026-10-27, track 88): PROB-0004
    (the scheduler comparison sweep is paused), PROB-0005 (two header stores), PROB-0011 (builders
    cannot chain), the release shape those two need, and permessage-deflate (owner-deferred, PRR 7).
    Three names left this list by closing rather than by a ruling — PROB-0009 (D9), PROB-0016
    (track 74) and PROB-0019 (track 53 stopped the tool recycling IDs; its three historical rows are
    the permanent `[RECYCLED ID]` readings the `validate` gate pins). PROP-0001 is not a decision:
    it is additive, so the 2026-10-25 policy authorizes it (track 91). `enable_h2c`'s name is
    settled by its definition — the matrix row and the method's doc both scope it to RFC 9113 §3.3
    prior knowledge and state that it never answers `Upgrade: h2c` — so the name describes the
    feature rather than claiming a wider one. (PROB-0015 was not a decision after all: the track-50
    paced-write slice implemented and closed it on 2026-10-24 together with PROB-0021; see
    WORKING_CONTEXT track 50. PROB-0014 was decided and closed 2026-10-22: the configuration object was
    chosen over a new `AetherApp::with_limits` setter. PROB-0008 was decided and closed 2026-10-23:
    the connection charge follows the allocator's block, and the idle-socket price is untouched
    because the initial 8 KiB sits inside `CONN_BASE_MEMORY`, so no admitted-connection count moves.
    PROB-0017 and PROB-0018 were closed the same day, without an owner decision, because the
    admission-shedding risk D5 registered them for was designed out instead of accepted: the HTTP/1
    write charge is best-effort (a refused reprice never withholds an answer the handler already
    produced) and the block is handed back after the write, so the peak is transient; on the h2 path
    the block price *does* shed slightly earlier than the length price did, and that is the behaviour
    `an_h2_body_is_priced_by_the_block_the_allocator_handed_out` now pins rather than a side effect
    nobody read.)
D4. `cargo fmt` is not a gate: `cargo fmt --all --check` reports 567 diff blocks across 66 files
    (recounted 2026-10-22; 558 before the behaviour-test and `101` slices), all in the
    hand-formatted style the tree already uses; deliberately not run as part of any round.
    Re-run as a measurement on 2026-10-23: **574 hunks, still 66 files** — the file set has not
    grown, so the +7 sits in code tracks 26-28 added to already-deviating files. B15's one `.rs`
    change was attributed rather than assumed: rustfmt over the HEAD blob and over the working
    `framework/websocket/src/connection.rs` both give 8 hunks at identical lines (8, 67, 169, 203,
    263, 286, 720, 727), none inside the 604-641 region added there, so the new tests are fmt-clean
    and the pre-existing deviations are untouched.
    Third re-run, 2026-10-23 after the response-block slice: **still 574 hunks across 66 files**,
    even though that slice added 411 lines (and removed 20) across seven `.rs` files — the added code
    contributed none.
    Same method as B15's, applied per file: intersect `git diff -U0` added-line ranges with the lines
    `rustfmt --edition 2024 --emit stdout` would move. It reported exactly one dirty region in the
    whole slice, a three-line `use crate::scheduling::{…}` that rustfmt collapses onto one line, and
    that line is now collapsed; the re-run reports zero added lines inside any hunk. Note this
    method is the only honest way to use the probe here: `rustfmt --check <file>` fails on seven of
    the touched files *and* on untouched ones (`framework/http/src/response.rs` among them), so a
    per-file exit code proves nothing about the new lines.
    Fourth re-run, same day, after the path verification added a three-line comment to `app.rs`:
    **still 574 hunks across 66 files**, and the intersection probe's added ranges for `app.rs` now
    include `(1893, 1895)` with `inside-my-additions=[]` — the comment is fmt-clean. That re-run
    first came back as **zero diffs**, which was a probe-stack defect and not a result: the shell's
    cwd had been reset to the repo root, `cargo metadata` failed for want of a `Cargo.toml`, and
    `cargo fmt --all --check` printed its usage block to the same file I was grepping for `Diff in`.
    A fmt check that never reached rustfmt reports exactly the same zero as a formatted tree, so the
    count is only trustworthy when the command actually reached rustfmt — check the exit status and
    that the output holds hunks rather than a `cargo metadata` error or a usage block.
    Fifth re-run, same day, on the tree the commits ship — 409 lines added / 20 removed across the
    seven `.rs`, after the `E4003` comment at `app.rs:98` was deleted: **still 574 hunks across 66
    files**, and the per-file intersection now reports `inside-my-additions=[]` for all seven. The
    probe's added ranges for `app.rs` were 13 regions (`98`, `1516`, `1680-1696`, `1713-1726`,
    `1757`, `1857-1862`, `1866`, `1888-1890`, `2018`, `2217-2218`, `2220`, `2238-2266`, `2644-2714`)
    against 96 fmt-dirty lines in that file — none of the slice's own lines among them.
    Sixth re-run, 2026-10-24 after the h2 write-path slice: the 11-file working tree reports
    **569 hunks across 66 files** and the exported six-file tree (the track-45/46 edits held at
    HEAD) reports **572 hunks across 66 files** (`grep -c '^Diff in'`); the added-line intersection
    probe reports `inside-additions=[]` on both trees for both touched Rust files (`http2.rs`,
    `http2_e2e.rs`). The working-tree count is five below the last recorded 574; the difference was
    not investigated and is left as measured — the load-bearing result is the intersection, which
    is empty, and the repo-wide fmt gate is red on both trees.
    Seventh re-run, 2026-10-26 after the track-68 `http2.rs` slice (+412/−12 lines): **570 hunks across
    65 files** (`grep -c '^Diff in'` and distinct paths over `cargo fmt --all --check`), i.e. exactly
    the recorded baseline — but only after a correction. The first run of this track's own tests
    reported **575**, and all five new hunks were in the added lines (`http2.rs:1577, 1592, 1642, 1659,
    1775`, each a long single-line `assert!`). Each was hand-expanded into the multi-line form the tree
    uses rather than running `cargo fmt` over the 65 deviating files, which would have rewritten code
    this track has no business touching. Then B15/D4's per-file method, applied like-for-like:
    `rustfmt --edition 2021 --check` over the HEAD blob gives **11 hunks** at lines 34, 443, 456, 576,
    586, 762, 772, 794, 811, 850, 879 and over the working file gives **11 hunks** at 34, 471, 484, 606,
    616, 802, 812, 834, 851, 890, 919 — the same eleven, the first unmoved and the rest shifted by the
    +28, +30 and +40 their own lines show. Zero new fmt hunks, and the repo-wide gate stays red at
    570/65 as recorded.
    Note the edition: `framework/aether/Cargo.toml` sets `edition.workspace = true`, so a standalone
    `rustfmt` run has to name the edition or it measures a different language level than `cargo fmt`.
    **CLOSED 2026-11-05.** Re-run on this tree from `framework/`: `cargo fmt --all --check`
    exits 0 and prints no `Diff in` line (0 hunks over 0 files), and `gates.sh:80` runs that
    command as the ladder's first gate. The readings above stay as the history of the debt;
    they are no longer a live finding, and this line is corrected rather than deleted so the
    next audit does not re-derive the same answer. No code or formatting changed.
D5. Two gaps found while closing PROB-0008 and registered instead of fixed, because each changes
    when work is shed and neither had a test that reads the value. **Both closed 2026-10-23.**
    **PROB-0017** — the HTTP/1 `write_buf` is the response-side mirror of the read buffer: the whole
    response is serialised into a per-connection `BytesMut`, `clear()` between requests never shrinks
    the block, and the registration did not include it at all, so a large response left ~its size
    in reserved memory on an idle socket for the life of the connection. Fixed as
    `write_buffer_charge` + `conn_buffered_charge` + `release_oversized_write_buf` in
    `framework/aether/src/app.rs`, with `WRITE_BUF_INITIAL_CAPACITY = 4096` exported beside the read
    constant so the two cannot drift. The charge is taken *before* `write_all` (a client that stops
    reading holds the serialized answer) and released *after* it, and it is deliberately best-effort:
    a budget that refuses does not withhold a response the handler already produced — that choice is
    what keeps this a memory-accounting fix rather than a feature regression, and
    `a_large_answer_is_priced_against_the_memory_budget_without_being_withheld` asserts both halves
    (the 16 MiB answer arrives in full under a 4 MiB budget **and** `memory_shed` is non-zero).
    **Stated limit:** that test cannot assert the peak by reading `registered_memory_bytes` on this
    host. Windows loopback accepts a 16 MiB `write_all` with no peer reading, so the charge exists
    for less than a millisecond and a 2 ms poll cannot see it — the earlier version of this test
    failed with "peak 32768 B, base 32768 B" for exactly that reason, which is a probe defect, not a
    code defect. `memory_shed` plus an in-test negative control (`/small` must shed zero) is the
    substitute that actually reproduces.
    **PROB-0018** — `http2.rs::read_body` re-priced with a per-DATA-frame `chunk.len()` delta
    (`StreamCharge::buffer_grow`), so the HTTP/2 path priced length where HTTP/1 priced the block;
    `Vec::reserve` grows to `max(2*capacity, needed)`, so the first frame lands exactly and later ones
    double it. Fixed by replacing that delta with `StreamCharge::buffer_set(buf.capacity())` — an
    absolute set rather than a delta, so shrink is reported too. Measured before/after on the same
    1,441,792 B upload: the length price peaked at **1,449,982 B** and failed the new assertion; the
    block price crosses **2,000,000 B** and passes, in
    `an_h2_body_is_priced_by_the_block_the_allocator_handed_out`.
    **D5a — PROB-0009's half that was safe to fix additively.** The blanket `Result` response kept
    answering 400 but stopped claiming `E5002` "unhandled internal server error"; it now emits the new
    `ErrorCode::E4003HandlerReturnedError`, pinned by `a_handler_error_answers_400_and_says_so_in_the_body`.
    The status class itself is untouched: flipping 400 → 500 would rewrite the published contract for
    every `Display`-only error. That half is still the owner's.
    **Superseded the next day for the status-class half.** On 2026-10-24 the owner chose option C′ and
    **D9** below records it executed: `E: Display + Send` is gone, the blanket now bounds a local
    `IntoResponseError`, and each error type states its own status. What D5a actually closed — the
    `E5002`-on-a-400 body — is unchanged by that; what was "still the owner's" was the decision, and it
    has been made.
    **Re-measured on 2026-10-24, and the first record of this surface was wrong.** The review surface
    is **14 test functions / 15 assert sites** in `framework/tests/tests` — extractors 4,
    `http_e2e` 7, `http2_e2e` 3 — not "19 (extractors 4, http_e2e 9, http2_e2e 4, scheduling 1,
    tls_e2e 1)"; `scheduling.rs` and `tls_e2e.rs` contain no 400 assertion at all (their 400s are
    `sleep(Duration::from_millis(400))`), and the extra five were the count of lines matching the
    digits, not of tests asserting the status. Deriving it properly meant brace-matching each test
    body and testing the assert argument, which also separates the two classes the earlier number
    blurred: **exactly one** of those tests pins the blanket
    (`a_handler_error_answers_400_and_says_so_in_the_body`, `http_e2e.rs:2462`); the other 13 come from
    five *other* 400 producers — extractor rejections build their own `Response`
    (`extract.rs:474, 502, 543, 560, 577`), request-head parse failures
    (`app.rs:2032`, `http2.rs:215` for an unnamed method token), upgrade refusals (`serve.rs:252, 342`)
    and host validation (`middleware/src/host_validation.rs:101`) — and all of those are correct 4xx
    whatever happens to the blanket. The published contract is **six lines in five documents**, not
    five in four: `FEATURE_COMPLETENESS_MATRIX.md:235`, `FULL_FRAMEWORK_DOCUMENTATION_AR.md:351-352`,
    `ERROR_HANDLING_GUIDE_AR.md:37`, `PRODUCTION_READINESS_REPORT.md:523-525`, and
    `docs/09_api_reference.md:116` — the last is the one the earlier sweep missed, and it is the doc
    readers copy from.
    **And that count was wrong too, corrected on 2026-10-24 while sweeping for `Display` to rewrite
    the contract.** The real surface is **eight statement sites in six documents**, measured on the
    tree this slice branched from (`c34e313`, i.e. `git show c34e313:<path>`):
    `FEATURE_COMPLETENESS_MATRIX.md:235`, `FULL_FRAMEWORK_DOCUMENTATION_AR.md:336` and `:349-350`,
    `ERROR_HANDLING_GUIDE_AR.md:37`, `PRODUCTION_READINESS_REPORT.md:522-525`,
    `GETTING_STARTED_AR.md:362`, and `docs/09_api_reference.md:62` and `:116`. Two more documents
    joined the list — `GETTING_STARTED_AR.md`'s handler-return table and `docs/09_api_reference.md`'s
    item 6 — and neither names `Display`, which is why a word-based sweep misses them: they say
    "`Err` gives 400" in prose. `FULL_FRAMEWORK_DOCUMENTATION_AR.md:351-352` is also not the blockquote
    span; `349-350` is, and `336` is a third statement in the code comment above it. The failure mode
    is the one this register has now hit four times: a count taken from one search term, recorded as
    though it were a count of the thing.
    **Four more facts the 2026-10-24 review measured, because each one changes the shape of the
    decision, not just its size.**
    (i) **The framework already distinguishes the two failure kinds — on the panic side.** A handler
    task that panics or is cancelled answers **500** (`serve.rs:422`), a request that outlives
    `request_timeout` answers **504** (`serve.rs:425`), a `State<T>`/`Extension<T>` the app never
    registered answers **500** (`extract.rs:510, 521`), a route pointing at a missing handler id
    answers **500** (`serve.rs:351`), and a `101` that never handed a socket over is replaced by an
    honest **500** (`serve.rs:299`). Only `Err` from a handler is 400. So "the server's own failure is
    invisible as 5xx" is too strong: what is invisible is a *reported* server failure, and the
    contract today reads `panic → 500`, `Err → 400`.
    (ii) **`E: IntoResponse`, the fix this register itself proposed, has a measured trap.** `&'static
    str` and `String` already implement `IntoResponse` as `Response::text` / `Response::bytes`
    (`app.rs:54-65`), i.e. status **200**; the test pinning the current behaviour returns
    `Err::<&'static str, &'static str>` (`http_e2e.rs:2463`), so re-bounding the blanket to
    `E: IntoResponse` would answer that handler **200 "database is unreachable"**. A bound change has
    to come with a status-carrying trait of its own, not with the general `IntoResponse`.
    (iii) **There is no additive door.** `impl IntoResponse for AetherError` would change nothing,
    because the blanket is on `Result<T, E>` with `E: Display + Send` and that impl is selected for
    every `E` regardless of what `E` itself implements; and a second `impl IntoResponse for
    Result<T, AetherError>` overlaps it. `AetherError::error_code()` (`core/src/error.rs:20-35`) is
    therefore dead weight on the wire — it has no consumer — and two of its arms read wrong next to
    the statuses the framework actually sends (`Io → E1001ServerStartupFailed`, `HeaderTooLarge →
    E3002PayloadLimitExceeded` against a 431). With it, `diagnostic.rs:36` still calls `E5002`
    "Unhandled handler error" and `:71` titles it "Internal handler error" although `E4003` took that
    role on 2026-10-23; nothing asserts either string, and nothing sends `E5002`.
    **D9 made every clause of that paragraph false, which is the point of the fix.** `error_code()`
    has a consumer now (`IntoResponseError for AetherError` delegates to it, `app.rs:166`), `Io` reads
    `E5002` and `Custom` reads `E4003`, the `E5002` doc says what sends it, and the `:71` title reads
    "Internal server error" — three lines above its own doc comment, which said "Internal server
    failure". `E5002` is sent: `typed_handler_errors_answer_their_own_class` asserts it on a 500 and a
    504. `HeaderTooLarge → E3002` against a 431 was checked and left: `E3xxx` is the request/limit
    class, the code agrees with the *kind*, and the register's own class rule reads the digit prefix
    against the status class (4xx/5xx), not against the exact number.
    (iv) **The blast radius outside this repository measured zero.** `my_cool_site` (package
    `threekway`, the live consumer) registers **43** `_async` handlers and **none** returns `Result` —
    it maps its own errors to `Response::status(400, …)` or returns the inner error's `Response`
    (`src/main.rs:450, 476, 646, 707, 1005, 1023, 1282, 1424`), so no handler there depends on the
    blanket. The workspace version is `0.1.0` (`framework/Cargo.toml:26`), so a breaking bound change
    is permitted by semver convention; the cost is documentation and downstream good faith, not
    version-rule violation.
    **D5b — a governance defect found while closing these two.** `next_id()` in
    `.project-governance/tools/project_governance_bootstrap.py` numbers from the items *still in
    the file*, and `done_item()` deletes on closure, so closed IDs are recycled — measured:
    PROB-0017 "WebSocket upgrade routes bypassed the user middleware chain" (created
    2026-10-21T12:45:14Z, deleted 12:57:10Z) and PROB-0017 "HTTP/1 response write buffer…"
    (created 2026-10-23T13:43:59Z). Eleven citations in four docs and `WORKING_CONTEXT.md` still mean
    the first. Registered as **PROB-0019** rather than silently renumbered, because the fix changes
    how every future record is read (keep closed items with a status, make `next_id` scan
    `event_log.jsonl` too, or stop citing IDs in shipped docs) and that is the owner's call. Note the
    ordering trap this creates: closing 0017/0018 *before* registering 0019 would have handed the
    number straight back to a third problem.
    **Closed 2026-10-24 (track 53), by the second option.** `next_id()` (`:125`) now takes the
    maximum over the open items *and* `ledger_ids()` (`:89`) — every `id` the append-only
    `event_log.jsonl` has ever recorded for that prefix — so a closed number is never handed out
    again; `add_item()` passes it in (`:334`) and `promote_item()` inherits the fix because it
    allocates through `add_item()`. A scan of the ledger found the reuse is wider than the one
    instance recorded here: PROB-0007 was used for the router param merge and then for connection
    memory, and REQ-0001 for three different requirements. The regression test is
    `.project-governance/tools/tests/test_id_allocation.py` — 6 cases, 4 of which failed against the
    unfixed tool (a closed PROB-0001 returned again; the real PROB-0017 shape returned PROB-0001; an
    open PROB-0005 plus a closed PROB-0009 returned PROB-0006; a malformed ledger line fell back to
    PROB-0001). On a copy of the real governance directory, closing PROB-0023 and adding a problem
    returns PROB-0023 with the `HEAD` tool and PROB-0024 with the fixed one. History is not
    rewritten: the citations listed above keep their contextual meaning, and the ledger remains the
    only place both meanings coexist.
    *Coordinates above are the track-54 tree, which inserted above all three functions.*
    **Limited review before treating the closure as final (2026-10-24, track 54).** Four conditions
    were set: the reader's corruption behaviour had to be declared rather than incidental, the
    historical ambiguity had to be stated and not papered over, `validate` itself had to look for
    reuse (with `promote_item()` covered), and the change had to stay a local commit on a clean tree.
    Each is addressed below; the last one is the commit record, and nothing was pushed.
    *Declared corruption behaviour.* `_ledger_records()` (`:60`) is the single reader of
    `event_log.jsonl`. A line that is not JSON, or JSON that is not an object, is named on stderr
    with its line number and skipped; every other line still counts; a record whose `id` is not a
    string contributes nothing; a log that cannot be read at all raises `GovernanceError` instead of
    being treated as empty (an empty ledger restarts at 0001). The one limit is stated in
    `ledger_ids()`'s docstring: an ID recorded *only* on a corrupt line is invisible to allocation,
    and the warning is what makes that gap visible rather than silent. (Track 55 adds a second
    limit: only `OSError` is wrapped, so a non-UTF-8 ledger raises `UnicodeDecodeError` first.)
    *The same silent reset one level up.* `load_json_list()` (`:36`) read an unparsable
    `problems.json` as `[]`, so the next allocation restarted at PROB-0001 and then overwrote the
    damaged file. It now refuses an unreadable, unparsable, or non-list file (a missing file is still
    `[]` for `init`); the CLI prints `[ERROR] …` on stderr and exits 2.
    *`validate` checks the ledger itself.* `ledger_allocations()` (`:111`) counts only `new_*`
    records (`done_and_deleted` and `promoted` carry an ID without allocating one, so counting them
    would make every closure look like a reuse), and `validate()` (`:399`) reports `[RECYCLED ID]`
    for any ID allocated more than once, `[OPEN VS HISTORY]` when one use is currently open, and
    `[UNLOGGED ID]` for an open item with no allocation record. Each sets exit 1. On this repository
    `validate` therefore **exits 1**: PROB-0007 ×2, PROB-0017 ×2, REQ-0001 ×3, with the open
    REQ-0001 flagged against history; 0 unlogged IDs, 29 allocations, 0 malformed lines. That red is
    a factual state — like the repo-wide `fmt` run — and can only change by rewriting history, which
    stays refused. A bare `PROB-0017` or `REQ-0001` in older text is still not resolved by this fix;
    the timestamped ledger entry is the authoritative reading.
    *Tests.* 16 cases (was 6), 10 of them new; run against the `HEAD` tool the current file is 7 red
    / 3 green. Red: no warning for a malformed line, an `AttributeError` on a valid-JSON non-object
    line, the two reuse cases and the unlogged-ID case returning 0, `validate` losing a recycle that
    sits around a corrupt line, and the corrupt-list case raising `AttributeError: GovernanceError`.
    Green before and kept as guards/coverage: `promote_item()` (it already allocated through
    `add_item`), a clean ledger, and the duplicate-title check. 16/16 pass with
    `python -B -m unittest test_id_allocation` (`-B` because the test module's own `.pyc` would
    otherwise land in `tools/tests/__pycache__`).
    **The two defects the owner asked to fix, and the two left open (2026-10-24, track 55).** The
    limited review of track 54 found four defects past PROB-0019 itself; `promote_item` and `log`
    were the two requested, the other two are recorded below as still open.
    *Promotion is create-before-delete.* `promote_item()` (`:366`) deleted the source, wrote the
    `promoted` record, and only then called `add_item()` — which reads (and can refuse) the lists and
    the ledger. On a copy of the `f65d9d5` tree with a corrupt `requirements.json`, that left the
    source deleted, `requirements.json` untouched, a `promoted` record in the ledger and CLI exit 2:
    the ledger claimed a promotion that never happened. It now reads all four lists before its first
    write, creates the requirement, and deletes the source only after `add_item()` returned 0; a
    `GovernanceError` from `add_item()` restores `requirements.json` to the IDs that existed before
    the call, so a refusal inside the creation leaves both sides as they were. A failure on the
    final `promoted` record still exits 2 with the requirement created and the source gone; the
    `new_requirement` allocation is already in the ledger, so the number stays protected. The
    duplicate-title check moved with the creation, so a taken title returns 2 with the
    source still open (the old order deleted first and failed second). `add_item()` (`:320`) gained
    `exclude_id`, so a promotion may keep the item's own title without matching itself.
    *`log()` refuses instead of crashing.* `:54` wraps the append in `except OSError` and raises
    `GovernanceError`, so the CLI prints `[ERROR] …` and exits 2 rather than a traceback after a
    caller has changed a list. Callers are still not atomic: `done_item()` deletes before it logs,
    so a failed write there exits 2 with the item already gone and no `done_and_deleted` record — an
    open gap, not changed in this track. (Closed in track 56.)
    *Tests.* 21 cases (was 16), five new; the current file against `f65d9d5` is **4 red / 17 green**.
    Red: corrupt-file promotion, ledger refusal during promotion, and taken-title promotion (all
    three lost the source), plus `log()` on an unwritable path (`PermissionError`, not
    `GovernanceError`). Green before and kept as a guard: unchanged-title promotion — it passes on
    the old code only because the duplicate scan runs after the source is deleted, and it is the
    test that fails when `exclude_id` is dropped. 21/21 pass with
    `python -B -m unittest test_id_allocation`.
    *Still open.* (1) `_ledger_records()` (`:65`) catches only `OSError`, so a ledger that is not
    valid UTF-8 raises `UnicodeDecodeError` before the wrapper and the CLI prints a traceback. The
    track-54 sentence "a log that cannot be read at all is refused" therefore holds for OS-level
    read failures, not for a decode failure. (2) A list entry that is not an object, or whose `id`
    is not a string, still crashes `validate()`/`add_item()` (`AttributeError: 'int' object has no
    attribute 'upper'`), so the element shape is unchecked while the file shape is checked.
    (Both fixed in track 56.)
    **The remaining three fixable findings closed (2026-10-24, track 56).** D2, F4 and the
    `done_item` gap were all the same contract — a command that cannot finish must leave the files
    as they were.
    *D2 — a decode failure is a refusal.* `_ledger_records()` (`:75`) caught only `OSError`, but
    `Path.read_text` decodes, so invalid UTF-8 raised `UnicodeDecodeError` (a `ValueError`) and
    `validate` printed a traceback; it now raises `GovernanceError`, and the CLI exits 2 with
    `[ERROR] …`. *F4 — elements are checked.* `load_json_list()` (`:36`) now refuses an element that
    is not an object, lacks a string `id`/`title`, or carries a non-string `description`, naming its
    1-based position; readers no longer meet `.upper()`/`norm()` with a number or a missing key, and
    a refused read never rewrites the file. *Closure rolls back.* `done_item()` (`:362`) restores the
    list it read when the `done_and_deleted` write is refused, instead of leaving the item gone with
    no record.
    *Tests.* 26 cases (was 21); the five new ones against `3400db0` give **5 red / 21 green**
    (unittest prints 4 failures + 5 errors because the shape case is a five-input subtest loop).
    26/26 pass with `python -B -m unittest test_id_allocation`. The only residual left is the final
    `promoted` record: a failure there exits 2 with the promotion complete and the number protected
    by its `new_requirement` record, and cannot be made atomic without a journal.
    **The parallel-ledger hazard closed from both ends (2026-10-25, track 66).** PROB-0025 and
    PROB-0026 are one root cause: `main()` resolved the project root from the shell's current
    directory, so whatever tree the command stood in *was* the project as far as the tool knew.
    *Walk-up resolution.* `LEDGER_MARKERS` (`:41`), `locate_ledger()` (`:45`) and `resolve_root()`
    (`:59`) replace that: the nearest directory at or above the target whose `.project-governance/`
    holds one of the ledger's data files wins — the files, not the directory name, so a half-created
    tree left by an interrupted run cannot shadow the real ledger above it — and a walk that reaches
    the filesystem root without finding one is a refusal, not a new project. `main()` (`:538`,
    resolution at `:563`) applies it to every command except `init` and prints
    `[LEDGER] using <root> (resolved by walking up from <start>)` to stderr only when the walk
    changed the target.
    *Measured before and after, on this repository.* Before (PROB-0025): `new problem` from
    `framework/aether/src` created `framework/.project-governance/`, allocated **PROB-0001** from its
    own empty list — a number the real ledger had used and closed — and nothing failed, because the
    root `validate` never reads that tree. After: `status` from `framework/aether/src` prints
    `Location: …\aether-web-framework\.project-governance/` and `Problems: 9`; `validate` from the
    same directory exits 1 with the *same four finding lines* the root run prints (PROB-0007 ×2,
    PROB-0017 ×2, REQ-0001 ×3, `[OPEN VS HISTORY] REQ-0001`) plus the `[LEDGER]` notice, and the root
    run prints no such notice; `framework/.project-governance/` does not exist.
    *PROB-0026 — `init` is for a repository that has no ledger yet.* On an exported scratch copy
    `init` exited 0 and printed `[SUCCESS]` while replacing `AGENTS.md`, `SYSTEM_PROMPT.md` and
    `PROJECT_MAP.md` with the blank template; `OWNER_VISION.md` and `WORKING_CONTEXT.md` survived only
    because those two writes were already `if not exists`. `init_project()` (`:299`, guard at `:300`)
    now refuses when `locate_ledger` finds a ledger at or above the target, and the `SYSTEM_PROMPT.md`
    and `AGENTS.md` writes print `[KEPT]` instead of overwriting. Live: `init` from
    `framework/aether/src` and `--root framework/aether init` both exit 2 with `[ERROR]` and create
    nothing, while `--root C:/tmp/t66fresh/nested init` exits 0 — so the refusal tracks the ledger,
    not the subcommand or the depth.
    *Declared limit.* The walk is unconditional, so `init` for a genuinely nested repository inside
    this tree is refused as well; the module docstring (`:11-12`) says such a project must be initialised
    outside this tree, which is also what the refusal message names as the found ledger.
    *Tests.* `.project-governance/tools/tests/test_ledger_location.py`, **11 cases**, green together
    with the 26 of `test_id_allocation.py` (37 total) under
    `python -B -m unittest discover -s .project-governance/tools/tests`. The first version of the
    read-the-real-ledger case asserted `PROB-0001` in `status` output and failed for the right reason:
    `status` prints counts, not IDs, so the case now seeds a *duplicate ID in the root ledger* — a
    parallel tree would print `Problems: 0` and `[VALID]`, and only reading the root can produce
    `Problems: 2` with `[DUPLICATE ID] PROB-0001` and exit 1. Mutation-proved on a copy of the tool
    and both test files in `C:/tmp/t66mut`: `resolve_root` returning the cwd-only path gives **6 red**;
    `existing = None` in `init_project` gives **3 red** (including the CLI exit-2 case); forcing both
    `[KEPT]` guards false gives **1 red**; the unmutated copy in the same tree gives OK, so the
    failures belong to the mutations and not to the sandbox.
D5c. **The verification asked for before committing (2026-10-23).** Two questions were put to this
    slice — can the read and write charges price the same memory twice, and does `E4003` break a
    consumer? Both answered by reading every path, not by the passing test.
    *Double counting:* `reprice_conn_memory` (`app.rs:2371-2396`) is target-based — it recomputes
    `CONN_BASE_MEMORY + buffered + extra` and applies a delta against the locally tracked `charged`,
    so nothing accumulates and the only way to double-price is to name the same allocation twice. The
    buffers are three distinct objects: `conn_buf`, `write_buf`, and the chunked decoder's own `Vec`
    (`parser.rs:452`, `body_capacity_bytes() == self.body.capacity()`), charged beside the wire copy
    because both are live RSS; `ws_memory` comes from `serve.rs:323` (`max_message_bytes`-derived),
    not from either buffer.
    *The other write paths:* four sites reuse `write_buf` outside the success emitter —
    `app.rs:1986` (408), `:2036` (400/413/431 parse refusals), `:2090` (buffer-limit 413), `:2120`
    (budget 413). Each writes a `Response::status`, which is head-only by construction
    (`http/src/response.rs:126-133`, `body: Bytes::new()`), so `write_buffer_charge` is 0 there and
    cannot grow the block, and each closes the socket immediately after (`return`/`break`, buffers
    drop with the task). The 101 handshake writes `handshake.as_bytes()` (`app.rs:1877`) and the shed
    path writes `rej_buf` (`:1452`, `:1770`) — neither touches `write_buf`. So **no release is
    missing on any of them**, but that is now a stated invariant (comment `app.rs:1982-1984`) rather
    than a coincidence of small bodies.
    *Write failure and close:* `return` at `app.rs:1805` sits after the pre-write charge and before
    the release, and `AdmissionGuard::drop` (`scheduling.rs:350-357`) releases the registration while
    `conn_charged` is task-local — a failed write cannot strand a charge. The release at `:1716` runs
    while the guard is still alive, i.e. before `conn_guard.take()` at `:1730`, which is the order a
    downward reprice needs.
    *Real scope limit, registered:* the closure is HTTP/1-side only. `http2.rs:463` answers with
    `sender.send_data(out.body, true)` — the whole body moves into the `h2` crate's own buffers, so
    there is no framework-side response block to charge or release, and nothing today prices what a
    stalled h2 reader can pin there. That is **PROB-0020**, with its confirm/refute test spelled out,
    not folded into 0017's closure. `runtime/src/worker.rs:126` carries the same pattern inside the
    crate D6 already proved dead, so it is not a live path.
    *PROB-0020 is now measured (track 49, Confirm):* an h2c client that withholds WINDOW_UPDATE for
    10 s leaves 67,257,145 B live in the server while `registered_memory_bytes` reads 32,768 B —
    exactly `CONN_BASE_MEMORY`, with no stream charge — and all 67,108,864 bytes arrive intact once
    the client resumes. The same harness found **PROB-0021**: the connection is torn down at
    `keep_alive_timeout` (60 s) even with the response still unflushed, so a slow client is
    truncated. **Both addressed 2026-10-24 (track 50), differently:** the write path is now paced
    (`write_body`, at most 64 KiB per `send_data`, `RST_STREAM(CANCEL)` when `write_timeout` expires
    with no progress), which closed PROB-0015 and PROB-0021 — T7 read 24 MiB over 95.9 s on the
    60 s/10 s defaults, and a stalled reader is reset at ~10.2 s while the connection survives. The
    re-run shows the stream charge held during the write (registered 49,152 B) with the body still
    unpriced (live 67,257,004 B): **PROB-0020 remains open**, and **PROB-0022** (a trickling peer
    renews the no-progress deadline) is registered separately. T7's steady reader is evidence
    about the idle-timer cut only and is not evidence about that trickle path.
    *PROB-0020 is now closed (track 51, 2026-10-24):* the response body is priced from the block
    that holds it, not from its length — `price_response_body` (`http2.rs:604`) recovers the
    allocation with `Bytes::try_into_mut()`, which is zero-copy and succeeds only when this stream
    is the block's sole owner, and `StreamCharge::buffer_set` registers `block.capacity()` before
    the response head is queued. The same harness re-run against the fixed tree reverses the
    reading: during an 8 s stall with a 64 MiB body, `registered_memory_bytes` holds
    **67,158,016 B = CONN_BASE + STREAM_BASE + the exact 67,108,864 B block** while the counting
    allocator reads 67,257,761 B live (second pass, 5 s stall: registered identical, live
    67,258,081 B); once the client resumed the full 67,108,864 B arrived, and after the connection
    closed at `keep_alive_timeout` live fell to 92,372 B and registered to 0. Track 50's reading of that harness was
    registered 49,152 B against the same live 67,257,004 B, so the pre-declared Refute criterion —
    "the retained bytes are priced by the scheduler" — is the branch measured.
    **PROB-0022 stays open and is restated**, because pricing does not end a trickling stream, it
    only makes it visible to the budget; and the residual pricing gaps are registered as
    **PROB-0023** rather than folded into the closure: `Bytes::from_static` and shared-block bodies
    (a handler that keeps a clone, `Bytes::from_owner`) stay unpriced, a body sliced from a larger
    unique buffer is priced from its first byte only, `buffer_set`'s set-semantics prices a stream
    that holds two blocks by one of them, and the framework's own error answers keep the last
    inbound block registered for the length of one error response. The independent review of this
    slice (fresh-context agent, not this session) found one SERIOUS defect that is now fixed:
    `a_response_body_over_the_budget_is_sent_anyway` proved "not registered" but not "refused",
    because a pricing step that never ran produces the same registration as one the budget refused
    — it now asserts `metrics().memory_shed > 0`, which only an attempted-and-refused grow bumps.
    That review also pinned the parked-task premise as arithmetic (`const _: () = assert!(BODY >
    65_535)`), and caught the then-false comment in `http2_e2e.rs` ("body bytes stay outside the
    scheduler (PROB-0020)"), whose `during > open` assertion is now `during >= open + 256 KiB`.
    *PROB-0022's pre-declared check ran and confirmed it (track 52, 2026-10-24):*
    `a_trickling_reader_keeps_an_h2_stream_alive_past_every_configured_deadline`
    (`framework/tests/tests/http2_e2e.rs:923`) sets both `keep_alive_timeout` and
    `max_response_time` to 1000 ms and grants exactly one byte of credit every 200 ms — so no
    1000 ms interval passes without a grant — for 3500 ms, three whole deadlines. The stream is
    still open at the end (any end inside the window fails the test), bytes moved *after* the
    client's window was drained (progress, not merely an open stream), fewer than all 256 KiB
    arrived, and a HEAD on the same connection still answers 200, which separates this from a
    connection-wide deadline. Two mutants are rejected: removing the renewal at `http2.rs:658`
    cuts the stream with `CANCEL` after 65,537 bytes in 1.29 s, and holding the stream open with
    no further progress fails the movement assertion in 4.27 s. The independent review (fresh
    context, not this session) found one SERIOUS defect in the first version — `received > 0` was
    already satisfied by the pre-loop read, so a stream that hung open without moving a byte would
    have passed — and the fixed version drains the first burst before the window and asserts
    movement after it. The bound itself is still not implemented, deliberately: a total per-stream
    cap is what would stop the trickle and D1 rejected it for truncating legitimate long downloads.
    **PROB-0022 stays open as a policy decision**; the test remains so that an implementation has
    to invert it rather than let the hazard return unnoticed, and a total cap longer than the
    3500 ms window would need a longer window rather than a passing test.
    *PROB-0022 is closed (track 67, 2026-10-25), and the paragraph above is the decision it was
    closed against.* The root cause was one line: `write_body` restarted `deadline` on **any** send
    progress (then `http2.rs:658`), so a peer granting one byte of credit before each expiry bought
    an indefinitely live stream for one credit per interval — priced response memory (track 51) and
    up to `h2`'s 400 KiB send-buffer leftover included. The fix makes the renewal cost bytes instead
    of waits: `MIN_PROGRESS_FOR_RENEWAL = 1024` (`http2.rs:493`) has to be moved before the clock
    restarts (`:679-683`, accumulator `:646`), and credit earned across several waits counts toward
    it, so a consumer that moves real bytes pays nothing extra. What that adds is a floor on
    consumption — `1024 / write_timeout`, ≈102 B/s at the default `max_response_time` of 10 s, which
    is arithmetic here and not a throughput measurement — while staying an *interval* bound: a
    stream that earns every interval may run for any total time. That distinction is exactly D1's
    objection to a cap, so the cap shape was not implemented; the pair of tests below is what
    separates a floor from a cap, and neither passes without the other.
    *The tripwire was inverted, as this entry required.*
    `a_trickling_reader_keeps_an_h2_stream_alive_past_every_configured_deadline` is gone;
    `a_trickling_reader_is_cut_at_the_deadline_it_cannot_earn`
    (`framework/tests/tests/http2_e2e.rs:949`) asserts the opposite in the same shape — `write_timeout`
    and `keep_alive_timeout` both 1500 ms, exactly one byte of credit every 100 ms for 5000 ms — and
    now requires `RST_STREAM(CANCEL)` as the end, that the post-window movement stayed under one
    renewal minimum (`received - before_window < 1024`, so the run really measured a trickle rather
    than a slow download), and that a HEAD on the same connection still answers 200 (so the cut is
    per-stream, not per-connection). Its premises are arithmetic, not comments: ten grant periods
    inside one deadline, at most `TIMEOUT_MS / GRANT_MS` = 15 bytes movable in an interval against the
    1024 minimum, and an observation window over three deadlines.
    `a_slow_reader_that_earns_every_deadline_finishes_the_whole_body` (`:1109`) is the pair: 100 KiB
    body, `write_timeout` 200 ms, 2 KiB of credit per 40 ms, asserting the whole body arrives, that at
    least four paced intervals were needed, that elapsed time exceeded three deadlines, and that the
    connection still serves afterwards — a transfer that outlives the timer and must not be cut.
    *Measured on this box, this session.* `cargo test -p aether-tests --test http2_e2e` → 37 passed /
    0 failed; `cargo test -p aether --lib` → 43 passed / 0 failed. Two mutants, each reverted after
    its run: renew on any progress (the pre-fix line) fails the trickle test with "a peer granting one
    byte per 100 ms still held the stream open for the whole 5000 ms window after 65571 of 262144
    bytes", and cut every stream once it passes some total (the cap shape D1 refused) fails the pair
    with "a reader that earns every deadline was cut after 75775 of 102400 bytes in 8 paced
    intervals: Error { kind: Reset(StreamId(1), CANCEL, Remote) }". So the first mutant proves the
    tripwire is sensitive to the defect it names, and the second proves the shipped bound is not a
    total cap wearing interval wording.
    *What this closure does not claim.* A peer that consumes *above* the floor still keeps a stream
    open for as long as it likes and now pays ≥102 B/s of real consumption plus priced memory for
    that — the bound is on what an interval must buy, not on total lifetime, and a per-stream byte
    cap remains refused by D1. The edge after the final `send_data` is unchanged: up to 400 KiB of
    leftover in `h2`'s own send buffer is still bounded only by the connection idle timer
    (`send`'s doc, `http2.rs:508-511`). Nothing was measured over a real slow network or behind TLS
    (PROB-0012), and the residual pricing gaps are PROB-0023, untouched by this slice.
    *`E4003` consumers:* no exhaustive `match` on `ErrorCode` exists in the workspace — `error.rs`
    matches on `AetherError`, and the two other hits (`app.rs:1284`, `tests/src/lib.rs:19`) construct
    single variants. The enum derives only `Debug/Clone/Copy/PartialEq/Eq`: no `Serialize`, no
    `#[repr]`, so the wire form is `code_str()` alone and no numbered discriminant shifts. It **is**
    re-exported publicly (`aether/src/lib.rs:33`, prelude `:67`) without `#[non_exhaustive]`, so any
    new variant breaks a hypothetical downstream exhaustive match — which is why the claim here is
    "verified against the consumers on this host", not "cannot break anyone". Verified by compiling,
    not by arguing: `my_cool_site`/`threekway` (path-dep on `framework/aether`) `cargo check` → exit 0
    in 11.30 s on the modified tree, and its 43 `E5002` occurrences are its own hand-written literal
    bodies (e.g. `src/main.rs:152`) plus a private `E4041` — it never reads the framework's code.
    *The other two consumers, reported as they are:* `nwfm/aether-probe` and
    `Desktop/benchmarks/rust_aether` both fail `cargo check` today (E0283 at
    `aether-probe/src/main.rs:98`, E0282 at `rust_aether/src/main.rs:71`), and the attribution is by
    the failing construct, not by assumption: both are unannotated `get_async(|req| async …)` closures,
    i.e. the `Handler<Args>` breaking change `WORKING_CONTEXT.md` has documented since the
    Developer-Experience round, and neither message mentions a symbol this slice touched. `nwfm/aether-bench-compare`
    checks clean (exit 0, 1 warning of its own). So the honest statement is: one live consumer verified
    compiling, one verified compiling-with-a-warning, two already stale from an earlier round and out of
    this slice's scope.
    *Gates, re-measured on the shipped tree* — after the `app.rs:98` comment deletion described
    below, which is the last code delta. `cargo test --workspace` → **474 / 0 / 1**, exit 0, 50
    `test result:` lines, unit 271 (`aether` 36) / integration 198 (`http_e2e` 46, `http2_e2e` 30,
    `scheduling` 16) / doc 5; `cargo check --workspace --all-targets` → exit 0, 0 `^warning` lines;
    force-cold `cargo clippy --workspace` → 46 `^warning` / 16 kinds / 39 in 7 summaries / 0 errors
    and `--all-targets` → 97 / 114 in 22 summaries / 0 errors / 0 `correctness`; `cargo fmt --all
    --check` → 574 hunks / 66 files, and the intersection probe returns `inside-my-additions=[]` for
    all seven `.rs`; the six new tests re-run individually, `1 passed` each. Every number reproduced
    the reading taken before the deletion, so the tree the commits carry has one gate reading, not a
    sequence of them.
    **Both attempts to take it first produced a fake reading, in two different ways.** (a)
    `cargo test --workspace 2>&1 | tail -60 > log` kept the last ten `test result:` lines, so the
    totals were unrecoverable, and `$?` reported `tail`'s exit status rather than cargo's — the log
    "passed" whatever the suite did. Re-run with the whole output in a file and the exit code read
    from cargo. (b) `cargo test --lib -- <name> --exact` with the wrong module prefix printed
    `test result: ok. 0 passed; 0 failed; …; 36 filtered out` — the filter matched nothing because
    these tests live at `app::tests::<name>`, not `<name>`. The recurring lesson: `ok` is not a match,
    and the filtered-out count is what makes the miss visible.
    *Coordinate damage this slice caused.* The first reading of this was the B15 symbol checker
    (`C:/tmp/t27/symcheck.py`, scratch — its numbers are transcribed here so deleting it loses
    nothing): **108 pairs, 19 reports, 2 STRICT**, both this slice's fault — `WORKING_CONTEXT.md`
    citing the `read_limit` choice and the oversized-read-buffer release. That run is kept as the
    record of what was then true; both of the numbers it produced were themselves superseded below,
    which is the whole point.
    **The 5-line `E4003` comment that had sat above `app.rs:98` was the bigger hazard and it is
    gone.** `git diff -U0`
    shows it as the only insertion above line 1679, and everything between 99 and 1679 is where the
    prose puts its coordinates: **55 of them across 11 files** (counted by `C:/tmp/t31/bands.py`,
    scratch). Deleting the comment makes that hunk net-zero and all 55 live again — confirmed by
    re-running the greps they rest on: `fn with_websocket_limits` → **1056**,
    `parse_request_incremental` → **1537**, `static_file` → **989-994**, `pub struct RequestContext` →
    **191**, `pub fn get<F, R>` → **703**, the shutdown `select!` → **1377-1384**, all exactly as
    cited. The rationale is not lost: it is the doc comment on `ErrorCode::E4003HandlerReturnedError`
    (`framework/core/src/diagnostic.rs`), where anyone reading the type finds it, and it is pinned
    behaviourally by `a_handler_error_answers_400_and_says_so_in_the_body`. Lesson worth keeping:
    **a code comment costs line numbers** — rationale belongs on the item, not at the call site that
    already reads clearly.
    What remains is everything above 1679: +17/+31/+35/+38/+67 by hunk in `app.rs`, +6 in
    `scheduling.rs` (the `WRITE_BUF_INITIAL_CAPACITY` const). A new checker, `C:/tmp/t31/shiftcheck.py`
    (scratch), builds the old→new delta from `git diff -U0` and pairs the nearest identifier with each
    coordinate, then reports whether the symbol sits at the cited line, at the cited line *plus the
    computed shift*, or neither. First run with the comment still present: **137 citations to changed
    files, 25 AS_CITED, 12 SHIFTED**; after deleting it and fixing basename resolution (path-suffix
    match instead of first-hit): **16 SHIFTED**; after the sweep below: **1 SHIFTED** — and that one is
    the tool re-shifting a number it had already corrected, because it assumes every cited line is
    pre-slice. It is a one-pass sweep aid, not a steady-state auditor. The **77 MOVED** rows are the
    prose-noise tier B15 already classified (`block`, `done`, `guard`, `aether` are not item names)
    plus the two coordinates in `notes/2026-10-20-chat-paste.md`, left as received by the owner's rule.
    **Two blind spots the tool cannot cover, found only by listing numbers per line.** (i) Bare
    continuation anchors — `:1904`, `:1854`, `:226`, `:368-374` — carry no filename, so any
    filename-keyed checker is structurally deaf to them; `docs/05` §3's five-site list and the ACTIX
    F4 row's six anchors came from printing every 4-digit number on every line that names `app.rs`.
    (ii) A numeric scan also eats git blob hashes: `5d428256e2bfad97` in
    `BASELINE-2026-10-20-SCHED-ROTATION.md` §provenance is a hash, not a coordinate — checked, no edit.
    (iii) **A span's *end* drifts while its start stays put, and every checker here keys on the
    start.** `impl<T, E> IntoResponse for Result<T, E>` is `app.rs:83-108`; the sweep had just
    corrected it *to* `83-113` because the `E4003` rationale comment was then five lines into the body
    — deleting that comment moved the closing brace to 108 and left `113` pointing into the
    `AppStateContainer` doc comment two items away. `shiftcheck` reported the citation as MOVED only
    because its ±8 window happened to exclude 98; a start-anchored checker calls `83-113` clean while
    its second half is false. So: for any span whose region you touched, re-derive **both** ends.
    *The re-derived list.* `app.rs`: the `Result<T, E>` blanket `83-113`→**83-108**, WS limits
    handoff `1771-1775`→**1802-1806**, `read_limit`
    `1936-1940`→**1974-1978** and `1936-1951`→**1974-1989**, read-buffer rebuild `1821-1825`→**1852-1856**
    (re-derived from the code, not shifted: the earlier **1848-1861** included the comment block),
    excerpt span `1811-1825`→**1842-1856**, parse-error match `1897-1902`→**1935-1940**, its
    `Connection: close` `1902`→**1940**, buffer-limit check `1953`→**1991** and `1954`→**1992**,
    `extend_from_slice` `1969`→**2007**, `conn_buffer_charge` `2196`→**2234**, `connection_has_token`
    `2250-2262`→**2317-2329**, its unit test `2484`→**2551**, second version-aware write site
    `1732`→**1763**, the four `write_http1_to` error exits `1854/1904/1958/1988`→**1892/1942/1996/2026**,
    `#[cfg(unix)]` `2313`→**2380** — where `WORKING_CONTEXT.md`'s `2288` for the same line was **already
    25 lines off before this slice**, which is the class of error a dead-line scan cannot see. This
    slice's own numbers after the comment removal: pre-write charge `1685-1701`→**1680-1696**,
    release+reprice `1718-1731`→**1713-1726**, `reprice_conn_memory` `2282-2307`→**2277-2302**, invariant
    comment `1893-1895`→**1888-1890**, handshake write `1788`→**1783**, `rej_buf` writes
    `1457/1775`→**1452/1770**, write-failure `return` `1716`→**1711**, `:1721`→**:1716**,
    `conn_guard.take()` `:1735`→**:1730**. `scheduling.rs`: `QueueToken` `352-366`→**359-366**, armed
    `:226`→**:232**, disarms `:228/:241/:245/:252`→**:234/:247/:251/:258**, its `Drop`
    `:368-374`→**:374-380** (`AdmissionGuard::drop` at `350-357` was measured after the insertion and
    needed no change). Files edited: `docs/00_framework_specification_{ar,en}.md`,
    `docs/02_tcp_framing_and_buffering.md`, `docs/05_request_context_and_memory.md`,
    `docs/07_limits_and_dos_mitigation.md`, `framework/docs/ACTIX_FAIR_COMPARISON.md`,
    `.project-governance/WORKING_CONTEXT.md` and this register — four of them docs files outside the
    seven the commit plan named, because leaving a false coordinate in a shipped doc is not a scope
    reduction the checker can excuse.
    The dead-citation tier with it: `C:/tmp/t27/citecheck2.py` re-run after the sweep reads **297
    citations, 32 suspect lines** (35 before it — the three that left were coordinates this sweep
    re-derived; 26 on the track-29 tree). No suspect line names a file this slice inserted into, which
    is what `shiftcheck` proves directly: it walks every citation to the seven changed `.rs` files and
    reported **0 DEAD**. Every suspect line this slice added is one of the two *external*
    consumer coordinates — `aether-probe/src/main.rs:98` and `rust_aether/src/main.rs:71` — which have
    no in-repo file by design, the same class as the four actix crate-qualified coordinates track 29
    classified. Zero in-repo dead citations were introduced here.
    **Third run, after the gate paragraphs were rewritten on the shipped tree:** `citecheck2` → **303
    citations, still 32 suspect** (the +6 are the coordinates this record itself now quotes);
    `symcheck` → **108 pairs, 17 reports, 0 STRICT**; `shiftcheck` → **139 citations to changed
    files**, `{AS_CITED 53, MOVED 79, NOSYM 2, AMBIGFILE 4, SHIFTED 1, DEAD 0}`. The one SHIFTED is
    the tool re-shifting a number it had already corrected (`docs/00_framework_specification_ar.md:73`
    → `app.rs:2068-2083`, verified at both ends: `let read_limit` at 2068, inside the read `match` at
    1989); the two NOSYM are `docs/07`'s Arabic-prose anchors to **1992** and **2007**, both verified
    by printing the lines — 1992 is the `conn_buf.len() + n > max_header_bytes + max_body_bytes`
    check and 2007 is `conn_buf.extend_from_slice(&temp_buf[..n])`. The 79 MOVED are the prose-noise
    tier B15 already classified, plus, this time, the `app.rs:83` start of the span described above.
    *Dating boundary, stated rather than smoothed over:* the four gate readings above were taken on
    2026-10-23 local, and the `app.rs:83-108` correction plus these last checker runs landed just
    after local midnight — 2026-10-24 in `+0300`, still 2026-10-23 in UTC. Every `2026-10-23` in this
    track therefore means the UTC day; the event log's `...Z` timestamps are the same clock.
D6. `framework/aether/Cargo.toml:12` declares `aether-runtime = { path = "../runtime" }`, and nothing
    in the shipped crate references it: `grep -rn "aether_runtime" --include=*.rs framework` returns
    nothing at all (not even inside `framework/runtime/` itself), and `cargo tree -p aether
    -i aether-runtime` shows exactly one edge (`aether` → `aether-runtime`). That crate's own
    `lib.rs:3-9` says it is a legacy prototype that nothing uses and that its `handle_connection`
    drops split requests, never enforces `Host` and applies no I/O timeouts. So every downstream
    install of `aether` today pulls a crate whose lib doc says "Do not treat it as supported API".
    Registered, not removed: deleting a manifest line is a publish-payload change and the owner's
    `2026-10-22` ruling keeps that decision separate from doc work. Found while verifying the
    `docs/02` §2 claim about `RequestArena` (its only live-looking use is in this dead crate).

D7. No `max_requests_per_connection` cap anywhere in the workspace — surfaced 2026-10-23 while
    re-checking `ACTIX_FAIR_COMPARISON.md` F6, whose four sub-claims turned out to be three fixes
    plus this one absence (`grep -rn "max_requests|requests_per_connection" --include=*.rs framework`
    returns nothing). This is a missing operator knob, not a conformance break — RFC 9112 does not
    require a per-connection request cap. Registered as an owner decision alongside `enable_h2c`
    naming, not fixed here: adding a cap changes the keep-alive contract every benchmark measures.

D8. No MSRV is declared anywhere, while `framework/README.md:185` states "the verified toolchain is
    rustc 1.98.1". Checked 2026-10-23 over all 19 tracked manifests
    (`git ls-files "*Cargo.toml" | xargs grep -c rust-version`): every count is 0. The README sentence
    is true as a record of what this workspace was built with — it is not a floor, and nothing makes
    it one, so a consumer on an older 1.85-plus toolchain gets a resolution or compile failure rather
    than a Cargo error. Registered, not fixed: adding `rust-version` is a public compatibility
    promise across 19 crates and downstream `my_cool_site`, and choosing the floor is an owner call.

D9. **PROB-0009's decision half executed 2026-10-24 — option C′, a dedicated `IntoResponseError`
    trait.** The owner chose C′ over the two alternatives this register put forward: flipping the
    blanket's status to 500, which would rewrite the published contract for every error that is a
    genuine client mistake, and re-bounding `E: IntoResponse`, which the (ii) measurement above shows
    answers `Err` with **200**. C′ is a third thing: a *local* trait that carries a status and nothing
    else, so the 200 path cannot open and the orphan rule still lets the framework implement it for
    `aether_core`'s `AetherError` — the door D5a's (iii) said had no additive opening.
    **What shipped, with the coordinates as they read on this tree.** `framework/aether/src/app.rs`:
    doc comment **83-98** (states both deliberate omissions), trait **99-127** — required
    `error_status()` at 101, `error_code()` default at **105-116** deriving the code from the status
    class (`>= 500` → `E5002`; 404/405/413|431 → `E2002`/`E2003`/`E3002`; anything else 4xx → `E4003`),
    `error_detail()` default at **124-126** returning `None`. Impls: `&'static str` **129-137** and
    `String` **139-147** (400 + their own message), `AetherError` **149-175** (the per-variant status
    match at 151-162: 404, 405, 413, 431, 504, 500×2, 400×4; the code delegated to
    `AetherError::error_code` at **165-167**; the 5xx redaction at **169-174**). The blanket
    `impl<T, E> IntoResponse for Result<T, E>` **177-202** now bounds `E: IntoResponseError` and builds
    `Response::json_status(status, canonical_reason(status), {"error": …, "code": …})`. Re-exported at
    `framework/aether/src/lib.rs:42-45` and in the prelude at `:60-63`.
    **The rule that makes the wire self-consistent:** the `code` in the body comes from the *class of
    the status*, never from a separate guess, so a 4xx cannot carry `E5002` and a 5xx cannot carry
    `E4003`; and a 5xx body reads `"Internal Server Error"` instead of the internal message, because
    an `Io` detail can name a table, a path or a port. `to_diagnostic()`'s redaction note says the
    same thing from the other side.
    **Three supporting changes the decision forced, each measured before it was made.**
    (a) `framework/core/src/error.rs`: `error_code()` **23-43** gained a doc comment at **20-22**
    recording the cross-crate invariant (the arms must agree with the statuses `app.rs:151-162` picks),
    `Io` moved `E1001ServerStartupFailed` → `E5002` at **:37** (`E1001` means *startup failed* and is
    still built where a bind fails) and `Custom` moved `E5002` → `E4003` at **:42** (its `Display` is
    the caller's message and answers 400).
    (b) `framework/core/src/diagnostic.rs`: `E4003`'s doc rewritten at **29-31**, `E5002`'s at **35-36**
    to name what sends it, and the `title()` arm at **:71** changed from "Internal handler error" to
    "Internal server error" — it sat three lines under a doc comment that says "Internal server
    failure", and `E4003` at :69 already owns the handler wording. `grep` for the old string across
    `framework`, `docs` and `.project-governance` found only its own definition and D5a's citation of
    it, and **no test asserts `title()`** — `title()` has exactly one consumer, `format_terminal()` at
    `diagnostic.rs:118`, and the wire body text comes from `canonical_reason`, which is a different
    function (`http/src/response.rs:320`, asserted at `:831`).
    (c) `framework/tests/tests/http_e2e.rs`: `a_handler_error_answers_400_and_says_so_in_the_body`
    **:2460** rewritten to name the trait it now pins, and one new wire test
    `typed_handler_errors_answer_their_own_class` **:2496** registers four routes on a real TCP server
    and asserts status *and* `code` *and* body text for each — `ValidationFailed` 400/`E4002`/message
    shown, `BodyLimitExceeded` 413/`E3002`, `Io` 500/`E5002`/`"Internal Server Error"` (detail
    redacted), `Timeout` 504/`E5002`.
    **The breaking edge, measured rather than asserted.** Four handler shapes compiled against this
    tree in a scratch crate (`C:/tmp/t43/probe`, one `Display`-only type `DbFailure`):
    `-> Result<&'static str, &'static str>` **compiles** (exit 0 — proof the 200 trap stayed shut: the
    `&'static str` `IntoResponse` impl is still there and the blanket still answers it 400),
    `-> Result<&'static str, AetherError>` **compiles** (exit 0), `app.get` with a declared
    `-> Result<&'static str, DbFailure>` **fails** `E0277: DbFailure: IntoResponseError is not
    satisfied`, and `app.get_async` with `Err::<&'static str, DbFailure>` inside an inferred
    `async move` **fails** `E0277: {closure}: Handler<_> is not satisfied` — that diagnostic does not
    name the trait (the string `IntoResponseError` appears 6× in the first log and **0×** in the
    second), so `get_async` users find the trait in `aether::IntoResponseError`, not in the error text.
    Both shapes are written into `ERROR_HANDLING_GUIDE_AR.md` §1أ and `FEATURE_COMPLETENESS_MATRIX.md:235`.
    **The live consumer still builds.** `my_cool_site` (package `threekway`) `cargo check` with
    `framework/aether/src/app.rs` and `framework/core/src/diagnostic.rs` touched first so nothing could
    be served from cache: **9 `Checking aether…`/`Checking threekway` lines, exit 0, 0 warning lines**.
    Its 43 `_async` handlers return no `Result`, which is why D5a's (iv) predicted zero blast radius —
    and the prediction was tested, not trusted.
    **Gates, all re-run on the final tree after the `title()` edit, so every number here belongs to the
    tree this record describes.** `cargo test --workspace`: **475 passed / 0 failed / 1 ignored**, 50
    `test result:` lines, exit 0 — unit 271, integration 199, doc 5; `http_e2e` moved 46 → 47 with the
    new test, both named tests `ok` in the log. `cargo check --workspace --all-targets`: exit 0, **0**
    warning lines. Force-cold `cargo clippy --workspace --all-targets` (every `.rs` touched first): 18
    `Checking` lines, **97** warning lines, **35** `clippy::` instances across **22** distinct lints,
    **0** errors, exit 0 — byte-for-byte the same aggregates as the pre-slice baseline log
    `C:/tmp/clippy31-alltargets.txt`, so nothing in this slice added a lint. `cargo fmt --all --check`:
    **576 hunks across 66 files** vs the HEAD tree's **574 / 66** (HEAD measured from a `git archive`
    export at `C:/tmp/t43/headtree`); the whole +2 is `framework/aether/src/lib.rs` **5 → 7**, because
    the re-export I added made rustfmt re-wrap two import blocks that were already dirty — 66 files on
    both sides, and `app.rs` 40, `http_e2e.rs` 49, `error.rs` 2, `diagnostic.rs` 1 unchanged.
    `C:/tmp/fmt_probe.py` on the final tree reports `inside-my-additions=[]` for all five changed `.rs`
    files: **fmt is still not a gate (D4), but no line this slice wrote is a hunk.** The doc fences were
    re-compiled too (`C:/tmp/t43/doccheck.py`, one scratch crate over the workspace path deps): **66
    fences → 45 compiled, 34 passed, 21 skipped as declarations, 11 failed**, and all 11 sit in the four
    classes that run has always had — 3 harness preamble re-imports (`E0252` on `Duration`/`Serialize`/
    `Deserialize`), 4 `?` conversions the harness declares against `AetherError` (`E0277`, fences in
    `FULL_FRAMEWORK_DOCUMENTATION_AR.md` §TLS and `GETTING_STARTED_AR.md`), 3 snippets that reference
    the reader's own undeclared types (`CheckoutDto`, `DatabasePool`, `CreateItemDto`) and 1 `include_str!`
    resolved from the scratch crate instead of the site root. **None of them is a fence this slice
    wrote**: the new `ERROR_HANDLING_GUIDE_AR.md` example (§1أ, the `ValidationFailed` → 400/`E4002`
    handler) compiled and passed.
    **Not done, and registered as such.** The redacted 5xx detail is *dropped*, not logged:
    `into_response()` has no request in scope, so there is no logger handle and no `X-Request-ID` to
    attach. `PROB-0009` therefore stays **open** with that residual, and `LoggerMiddleware` remains
    println-based in the connection loop (`framework/middleware`) — turning the drop into a log is a
    request-context change, not an error-contract one. Six documents carry the new contract:
    `ERROR_HANDLING_GUIDE_AR.md` (rows :37/:38/:39 + the §1أ table and example),
    `FEATURE_COMPLETENESS_MATRIX.md:235`, `FULL_FRAMEWORK_DOCUMENTATION_AR.md` (the blockquote and code
    comment), `GETTING_STARTED_AR.md:362`, `PRODUCTION_READINESS_REPORT.md` §12 item 5, and
    `docs/09_api_reference.md` (:62 and item 6).

---

## E. Improvement backlog — structure and efficiency (registered, NOT authorized)

The owner's standing rule applies to every line: feature parity first, efficiency second; never
weaken a required feature to save RAM/CPU; **the owner runs the benchmarks** — the agent hands over
the command, it does not report a speed claim. Nothing below has a measurement behind it, so nothing
below may be described as an improvement.

| # | Item | Why it is plausible | What must be measured first | Gate |
|---|---|---|---|---|
| E1 | Route/matcher lookup shape: radix trie vs alternatives for the hot prefix set | The trie already exists; a different structure only pays if lookups show up in a profile | allocations/req **at 8 connections** (deltas scale with connection count) + CPU/req | parity exists → needs a profile, not a refactor |
| E2 | Header storage: the public `HashMap` + private multi store (two stores) | Merging them removes one copy per request **and** settles PROB-0005 — the correctness argument is stronger than the memory one | cost of a write-through single store on the request path; must not break `header_values()` ordering | blocked on PROB-0005 (owner decision) |
| E3 | Per-request allocations: `Bytes`/buffer reuse for the read buffer and response write | `BytesMut::with_capacity(8192)` per connection exists; response formatting pre-formats into one buffer | allocations/req at 8 conns; keep-alive vs one-shot shape | no claim without a same-session A/B |
| E4 | Mutex on the handoff slot (A1) | The slot is per-request and uncontended; an `UnsafeCell`/`Cell` with a `Send` wrapper would remove one lock pair per upgrade | the lock-HOLD cost measured at the instrument floor, on the upgrade path only — not on HTTP | parity is fine today; only worth it if upgrades become hot |
| E5 | `Arc<Mutex<…>>` scheduler admission (admit + pump) | Project memory says the HOLD cost is ~0.9 µs/req, half of it in admit+pump; sharding was previously *refuted* as a fix | do not re-propose sharding; any change needs a new measurement, same-session A/B, rotated order | refuted once — reopen only with evidence |
| E6 | WS frame path: copy-free reassembly, bounded pooling/arena for message buffers | 16 MiB + 16 MiB re-pricing per session is a large nominal charge | live CPU/req on an echo workload; peak RSS with N sessions; bounded pool vs unbounded | no WS benchmark exists at all |
| E7 | Sort/index by search key vs memory layout (AoS vs SoA) | Ordering data by the lookup key is not the same as ordering it in memory; relevant to the route table and header lines only if a profile shows cache misses | cache-level measurement this laptop cannot produce reliably | not actionable on this host |
| E8 | Response-body streaming — **closed**; `sendfile` — still open | This row said "listed as missing parity, not as an optimization". The parity half is delivered: `Response::stream` / `stream_with_length` on both frontends (track 99, 2026-10-29) and streamed request bodies (100, 100b, 2026-10-29/30). The `sendfile` half is not: a streamed file is a 64 KiB read-then-write loop (`STATIC_CHUNK`, `framework/aether/src/static_dir.rs:25`), so the bytes still cross into userspace and back | a same-session A/B of zero-copy vs the chunked loop on a large static file — CPU/req and allocations/req, owner-measured | was "parity gap, not perf work"; now perf work only, and no number exists for it |
| E9 | Separate receive vs upload timeout; client-disconnect cancellation; typed error mapping | Named repeatedly as next parity candidates | error mapping is coherence-blocked in Rust; cancellation now has wire tests in all three protocols | all three delivered — typed error mapping on 2026-10-24 (D9, the local `IntoResponseError` trait), the receive deadline (track 84) and disconnect cancellation (track 85) on 2026-10-27. The "remains open" this row carried after D9 was stale; corrected 2026-10-27 (track 87) |

Standing guardrails to respect when any E item is picked up: closed-loop p50 against concurrency is
Little's law, not a server property; client shape flips results on this 8-core box (a withdrawn
topology claim proved that); release-vs-release, identical work, repeats, CPU/RAM, confirmed vs
possible only.

---

## F. Gate-6 split constraints (measured, plan only — nothing executed)

### F1. File-level stage attribution (29 modified + 5 untracked)
- **HTTP/2 + transport stage**: `aether/src/transport.rs`, `aether/src/tls.rs` (ALPN offer),
  `aether/src/lib.rs` (doc), `http/src/limits.rs` (`max_concurrent_streams`),
  `http/src/response.rs` (`sanitize_field_value` shared with h2), `aether/Cargo.toml`,
  `Cargo.lock`, `tests/Cargo.toml`, untracked `aether/src/http2.rs`, untracked
  `tests/tests/http2_e2e.rs`, untracked `tests/examples/h2_interop_server.rs`,
  `tests/tests/tls_e2e.rs` (ALPN order), app.rs hunks `-28, -130, -468, -509, -914, -964, -1261,
  -1282, -1389` and one line of `-1117`.
- **Extensions stage**: untracked `aether/src/extensions.rs`, untracked
  `tests/tests/extensions_e2e.rs`, `aether/src/extract.rs` (`Extension<T>`), app.rs hunks
  `-153, -170, -195, -215, -1404`.
- **WS middleware stage**: `aether/src/serve.rs` (all 4 hunks), `tests/tests/http_e2e.rs` (all 10
  added tests are WS), `tests/tests/scheduling.rs` (the completion tally changed *because* the
  upgrade request now dispatches), app.rs hunks `-182, -435, -454, -495, -794, -803, -1804, -1926`
  plus one line of `-1117`.
- **TLS polish**: app.rs hunk `-1100` (`(tls required)` banner) belongs to no stage above; it is a
  TLS follow-up and can ride the HTTP/2 commit or stand alone.
- **Docs**: every doc file touched carries edits from all three stages.

### F2. Dependency edges found in the code (these force the order)
1. `aether/src/http2.rs:264` constructs `extensions: crate::extensions::Extensions::new()` →
   **HTTP/2 cannot be committed before Extensions** (compile error, not a style preference).
2. `tests/tests/http2_e2e.rs` contains 3 extension-over-h2 wire tests → same edge, also at test
   level.
3. `tests/tests/http_e2e.rs::a_websocket_upgrade_runs_the_app_middleware_chain` uses
   `insert_extension`/`extension::<WsMarker>()` → **WS tests depend on Extensions**.
4. WS does **not** depend on HTTP/2: `allows_upgrade` and the `WebSocketUpgrade` outcome already
   exist at HEAD (`git show HEAD:framework/aether/src/serve.rs` → 3 hits), so the WS patch applies
   to the HEAD core.
   > Re-measured 2026-10-23 (register **B16/B17**): that command returns **5** hits, not 3, and it
   > returns 5 at `a7be6cb`, `41dbe39`, `01e79ef` (the pre-split tip) and today's `HEAD` — 3 lines
   > carry `allows_upgrade` and 2 carry `WebSocketUpgrade`. The "3" was a count of one of the two
   > symbols; the structural claim it supports is unaffected and is what should be cited: both
   > symbols are present at `01e79ef`, before any commit of the split, which is exactly why the WS
   > patch did not depend on the HTTP/2 stage. "HEAD" here meant the 2026-10-21 tip, and per **B16**
   > a plan record should name the SHA — read this line as `git show 01e79ef:framework/aether/src/serve.rs`.
5. Extensions depends on neither: `extensions.rs` is self-contained and the app.rs EXT hunks touch
   no h2c/WS lines.
6. `tests/tests/scheduling.rs` must move **with** the WS commit: at HEAD it asserts
   `completed == 0`, and that assertion is only correct while the upgrade request still skips the
   chain. Shipping it in the HTTP/2 commit would make that commit's own suite fail.

Net: **Extensions → HTTP/2 → WS middleware** is the only order that compiles and stays green at
each step. The order the owner suggested (TLS/HTTP-2 first) is blocked by edges 1–3.

### F3. The one genuinely mixed hunk
`app.rs @@ -1117,16 +1290,21` adds `let h2c = app.h2c;` (HTTP/2) and
`ws_middleware: app.ws_middleware,` (WS) inside the same `ServiceCore` construction. They are
separated by more than three context lines, so `git add -p` → `s` can split them, and the doc
comment lines in between must be assigned to whichever commit takes them. **This has to be verified
by building each intermediate tree, not assumed.** Same caution for the small comment-only edits in
`-1100`/`-1261`.

### F4. Test command per commit (to run before each `git commit`, all Windows-only)
Expected counts are deliberately **not** predicted here. HEAD's own suite total was not measured in
this round — measuring it means a clean tree or a second worktree, and the working tree must stay
untouched until the split is executed. Each commit must therefore record the number it actually
prints, and the delta against the previous commit is the stage's own test count.
- Extensions commit: `cargo test --workspace`, then targeted
  `cargo test -p aether --lib extensions` and `cargo test -p aether-tests --test extensions_e2e`.
- HTTP/2 commit: `cargo test -p aether-tests --test http2_e2e --test tls_e2e` plus the workspace
  run; `cargo check --workspace --all-targets` must stay exit 0.
- WS commit: `cargo test -p aether-tests --test http_e2e --test scheduling` plus the workspace run;
  re-run the two falsification checks if the status guard is touched (`left: 101, right: 202` /
  `left: 101, right: 503`).
- Any commit: `cargo build --workspace --release` clean, and `cargo clippy --workspace` (lib scope)
  clean of correctness lints. `--all-targets` is usable as a gate since B1 closed (2026-10-22).
- Docs/governance: one final commit (`WORKING_CONTEXT.md`, `problems.json`, `event_log.jsonl`,
  `PROJECT_MAP.md`, all `docs/*`) rather than hand-splitting doc paragraphs, because F1 shows every
  doc file mixes stages; splitting them line-by-line buys review granularity nobody can verify.

### F5. Honest verdict on separability
The **code** splits into three reviewable commits along stage lines with one hand-split hunk (F3)
and a forced order (F2). The **docs do not** split cleanly by stage, and the governance files
(`problems.json`, `event_log.jsonl`, `WORKING_CONTEXT.md`) are append-only histories of all three
rounds. So the plan is 3 code commits + 1 docs/governance commit, and the claim "each commit is
independently reviewable and testable" is true for the code and **not** true for the documentation.

## Allocations-per-request: what is closed and what is a decision (2026-10-24, track 45)

Measured counted-vs-counted at 8 connections with two request counts per arm (20k/60k) to separate
per-request from per-connection cost; allocation spread 0.00 across four trials per arm. Actix served
10.04 allocs/req (~700 B) on a byte-identical `/plaintext` and `/json` response; Aether served 45.00
(6672 B), then **32.00 (5795 B)** after this tree removed the per-request deep clone of the header
store and gated three header reads behind the features that consume them, and **31.00 (2595 B)** after
G15 below. Remaining work, priced:

- **G14 — the double header store is the largest measured site (+18.00 allocs/req, the stage ladder at
  `aether-bench-compare/src/bin/cost_probe.rs`).** `RequestContext` owns both the public
  `headers: HashMap<String, String>` and the private `header_multi: HashMap<String, HeaderLines>`, which
  costs two key Strings plus two value Strings per field line and two table allocations per request; a
  3-line request cannot fall below ~14 allocations while both exist. Closing it is **PROB-0005** (remove
  or merge a public field), an API break deliberately left open for the owner. Not a task: a decision.
- **G15 — CLOSED and RETAINED 2026-10-24 (tracks 46, 48): the parser no longer sizes its httparse
  slot array to the configured ceiling.** `parse_head` estimates the head's field lines from the line terminators already
  in the buffer (`parser.rs:header_slot_estimate` — bounded by `max_header_bytes`, stops at the blank
  line that ends the head), tries that size first, and reaches the ceiling only when httparse reports
  `TooManyHeaders` below it. An ordinary request now fits the 64-slot stack array and allocates nothing
  for its head; a request that outgrows its estimate — the oversized request itself — pays one retry.
  **`TooManyHeaders → 431` keeps exactly its old force**: an exhausted array is reported as
  `Attempt::SlotsRanOut` (never as the limit), only the ceiling-sized attempt can answer 431, and the
  post-parse `headers.len() > limits.max_headers_count` check stays for the `max_headers_count < 1`
  case the floored budget cannot represent. The byte-budget checks, the Partial/Complete ordering, the
  CL/TE rules and the host rule are the same code moved into `parse_head_into`, not rewritten.
  Measured on the final tree, counted-vs-counted as above: **32.00 → 31.00 allocs/req and 5795 →
  2595 B/req**, 31.00 in **four runs against one attested binary** (exe sha256 `4e0fb32b…`, process
  owning `127.0.0.1:3101`, `counting=1`; evidence in `C:/tmp/t46_g15_runs.txt`), plus the two earlier
  runs on the pre-format binary at the same numbers.
  The 3200 B is exactly the 100-header array, so the array was worth **one** allocation, not the
  ladder's `+2.00` — that rung over-attributed one allocation this change did not remove; the
  remaining 2595 B/req and 31 allocs/req stand as measured. Three parser tests lock it:
  `test_undersized_slot_array_reports_the_array_not_the_limit` (the retry signal is the array, not the
  limit), `test_head_just_above_the_stack_slots_parses` (65 lines still legal at the 100 ceiling),
  `test_header_slot_estimate_scans_one_head` (one head, LF-only heads, no-terminator bound, ceiling
  and floor), and the pre-existing `test_header_count_limit_is_enforced` /
  `test_header_count_limit_above_stack_slots` / `test_default_header_budget_is_a_hundred_field_lines`
  now exercise the new path unchanged.
  **Independently reviewed and retained 2026-10-24 (track 48).** A fresh-context reviewer agent
  re-derived the cycle from zero: its own release build (sha256 `2b34eb40…`), its own process (21056
  owning `127.0.0.1:3101`, 283 `STATS` lines all `counting=1`), two probe invocations that both printed
  **31.00 allocs/req and 2595 B/req** (slope spread 0.0002 allocs/req, 0.001 B/req; intercepts 9.4 and
  10.8), the same gate counts (**479/0/1**, check 0 warnings, clippy 97, fmt 573 hunks with none in
  the new ranges), and the same wire answers (101 field lines → 431; 64/65/100 → 200; LF-only → 200;
  pipelining → 200/200; CL+TE and duplicate CL → 400). It found no blocking correctness or security
  defect and named one residual risk: the retry's *accept* branch is unreachable end-to-end for a legal
  head (the estimate over-counts by the request and terminator lines), so it is an untested safety net.
- **G16 — a keep-alive connection with nothing in flight allocates 21.6 times/second**
  (`C:/tmp/t44_idle.py`, 8 live idle sockets). Small against the per-request number at 5k rps, but it is
  per-connection and scales with connection count, so it belongs with **PROB-0022**'s live-stream
  pressure work rather than the CPU hunt. (Written when this pointed at **PROB-0020**; that problem was
  closed on 2026-10-24 by the response-body pricing in track 51, and the residuals it left are
  **PROB-0023** — an idle connection's allocations are still unpriced by either. The pointer to
  **PROB-0022** is historical as well: it closed on 2026-10-25 (track 67) by raising the cost of
  keeping a stream alive, which bounds cost per interval and does nothing about an idle
  connection's allocations. **And PROB-0023's own closure (2026-10-26, track 68) does not reach this
  line either:** that problem was about which *response/request blocks* the h2 write and read paths
  register, and closing it meant measuring those shapes and fixing the refusal path. An idle
  keep-alive connection's 21.6 allocations/second remain unpriced and unauthorized — this G-item is
  their only record. What track 68 did leave open is one hop away and unrelated: heap a handler wraps
  in `Bytes::from_owner`, which the budget cannot name at all (**PROB-0027**).)
- **G17 — the harness was the first defect, and it had already produced one wrong claim.** A counted arm
  compared against an uncounted one is forbidden by `src/counting.rs:8-10` and was exactly what track 44
  measured; `actix_server.rs` now carries the same instrument, `arm()` announces `counting=armed|off`
  once per process, and `loadtest.py` exits non-zero when responses fail their own body check (it
  returned 0 while 200/200 responses mismatched). Do not read any pre-track-45 cross-framework CPU or
  allocation number as a comparison of the same kind of run.

**Denominator note (2026-11-05, track 124).** The 31.00 allocs/req above is still the truth of
that tree, but the product runner that reported production arms was not. `rlbench` sends
`warmup = 200` requests per connection inside the invocation the harness counts, and the counted
window opens before that invocation; `cost_probe` prints a `served` counter and divided by the
truth, while `aether_server`/`aether_sched_server` print no such field and `layer_ladder.py` fell
back to the client's `completed` — 24 000 where the server had served 25 600 at 8 connections x
3000 requests. Every **production-arm** allocation and `us_cpu_per_req` figure that runner printed
before 2026-11-05 is therefore 1 + 200/3000 = **6.67 % high** at those settings (the inflation is
`1 + warmup/measured`, so it falls as the request count rises, and the ladder rungs themselves were
never affected). The discrepancy is exactly how it was found: the G15 tree at `23999a7` prints
**33.07** under the old denominator and **31.01** under `denom = served if served > 0 else reqs +
conns * WARMUP_PER_CONN`, reproducing the attested 31.00 to its per-connection intercept rather
than contradicting it. The same-session series after the fix, one client and one harness, is
**44.01** (`b70ce7c`, pre-G15) -> **31.01** (`23999a7`) -> **30.01** (989f488 plain) -> **32.01**
(989f488 scheduled), with spread 0.00. Read the older production figures 6.67 % down before
comparing them with a post-2026-11-05 run.

## Track 69 — read the logic, fixed what the logic actually gets wrong (2026-10-26)

Owner's instruction for this track: «اتخذ قرار بنفسك وحل مشاكل لو في شوف منطق الكود» — decide
yourself and fix problems if there are any, look at the code logic. Scope: correctness of the
serving path (transport classification, upgrade, refusal accounting, route registration), **no
performance claim of any kind** and nothing here is a speed result. All seven mutants below were
re-run against the final tree (`C:/tmp/t69_mutants_final.log`, `t69_mutants_final2.log`,
`t69_mutants_final3.log`), so each fix has a first-hand reading of its own test failing on the fault
it exists to catch, on the tree being committed.

### Fixed

**T69-1 — the classifier's second `peek` re-reported the bytes it had already stored.**
`transport::classify` (`framework/aether/src/transport.rs:237`) peeks 24 bytes to choose HTTP/1,
HTTP/2 or TLS on one port. `MSG_PEEK` reports from the socket's *first unread byte* and never
advances it, so a second peek returns the same head again. The old loop peeked into
`&mut probe[filled..]` and then did `filled += got`, i.e. it *extended* the probe with a copy of the
prefix it already had. Measured consequence: an h2c preface arriving in two segments produced a
24-byte probe of `P` + `PRI * HTTP/2.0…`, which is not the preface, so `decide_final` answered
`Http1` for a genuine HTTP/2 client and the HTTP/1 parser replied 400 to `PRI` on a socket the peer
believes speaks HTTP/2. The loop now peeks into a separate `scratch` and copies only the tail that
grew (`probe[filled..got]`), and it *waits* for a stalled discriminator — bounded by
`CLASSIFY_RETRY_LIMIT` (4) × `CLASSIFY_RETRY_SLACK` (25 ms), ≈100 ms worst case, well inside every
`first_byte_timeout` the framework ships — because an unbounded wait on a non-consuming peek is a
hot loop on one worker. Three tests: `a_preface_arriving_in_two_segments_is_still_recognised_as_http2`
(`:352`), `a_client_that_pauses_mid_discriminator_is_answered_from_what_it_sent` (`:395`),
`classifying_leaves_the_bytes_for_the_protocol_that_uses_them` (`:426`).
Mutants re-run on the final tree:
- duplicated fill (`probe[filled..filled + got] = scratch[..got]`, `filled += got`) → the two-segment
  test FAILED (`test result: FAILED. 2 passed; 1 failed`), and the mutant's own copy ran off the end
  of the probe (`panicked at aether\src\transport.rs:256:18`) — the old code did not just mis-answer,
  it could panic whenever a later peek reported more bytes than the probe had room left at the
  growing offset.
- bound removed (`waits > usize::MAX`) → the stalled test FAILED at its `expect("a stalled
  discriminator must fall back, not spin")` after 2.02 s (the 2 s timeout, not the ~100 ms budget).
- consuming read (`stream.try_read(&mut scratch)` in place of `peek`) → **all three** transport tests
  FAILED (`0 passed; 3 failed`, 5.02 s: the third test's `read_exact` hit its 5 s timeout because the
  classifier had eaten the preface).
- Process note, because it is the vacuous-test class again: the first draft of the two-segment test
  **passed against the buggy classifier**, because `classify` was awaited only after both writes had
  landed. It now spawns the classifier and asserts no answer existed before the second write
  (`rx.try_recv().is_err()`), which is what makes the mutant above fail for the right reason.

**T69-2 — an HTTP/1.0 request could switch protocols, and the answer claimed 1.1.**
The parsed head set `allows_upgrade` unconditionally, so a `GET /ws HTTP/1.0` carrying the upgrade
headers reached the WebSocket arm and the framework wrote its hardcoded `HTTP/1.1 101 Switching
Protocols` line to a client that never spoke 1.1. HTTP/1.0 has no `Upgrade` mechanism at all
(RFC 9110 §6.7). It is now `allows_upgrade: version == crate::serve::Version::Http11`
(`framework/aether/src/app.rs:1784`), and the gate that reads it (`serve.rs:280`) answers 400.
Test `an_http10_upgrade_request_is_refused_instead_of_switching_protocols`
(`framework/tests/tests/http_e2e.rs:1342`) frames the request by hand and asserts the answer is
`HTTP/1.0` **400**, that zero sessions started, and that the socket closes; the same head over 1.1 in
the same test must still produce `101` and one session. Mutant `allows_upgrade: true` → FAILED at
`http_e2e.rs:1363`: `assertion left == right failed: the rejection must be framed for the version
the client used, left: "HTTP/1.1", right: "HTTP/1.0"`.

**T69-3 — the WebSocket budget refusal neither closed its socket nor settled its guard.**
Refusing an upgrade whose session buffers do not fit the memory budget is track 51's; this is what
followed the refusal. Two defects in that branch: it sent `503 + Retry-After` with no
`Connection: close`, so a client that reused the connection would parse its next request as
WebSocket frames; and it neither called `shutdown()` nor completed the connection guard, so (a) on
Windows the bare drop RSTs away the response the peer has not read — the same hazard
`error_responses_survive_unread_pipelined_bytes` exists for — and (b) the scheduler tallied this
refusal as a *cancellation* while the identical-shaped 400/413/431 rejections were tallied as
*completed* (the rule is stated at `app.rs:2090-2096`). It now writes, `shutdown()`s, and
`conn_guard.take()` → `complete()` before returning. Test
`websocket_buffers_are_charged_to_the_memory_budget` (`framework/tests/tests/scheduling.rs`) asserts
the 503 carries `connection: close`, that `completed >= 1` and that `cancelled == 0`. Mutants:
deleting the header push → FAILED at `scheduling.rs:489`; deleting the guard close-out → FAILED at
`scheduling.rs:505` with `completed: 0, cancelled: 1` and the message `one refusal, completed`.

**T69-4 — a catch-all declared mid-path registered silently as a route that swallows the prefix.**
`RadixRouter::add_route` walked `*rest` as an ordinary segment, but a catch-all consumes *the rest of
the path*, so `/files/*rest/trailer` attached its handler to the `*rest` node: every `/files/…`
request ran it and the declared tail was never compared. The pre-fix behaviour was captured against
the old code before the change (that build matched `/files/a` to the `/files/*rest/trailer`
handler). Registration is a programming error, not a runtime input, so it now fails at startup
exactly like a duplicate method + path does (`reject_duplicate`,
`framework/router/src/trie.rs:154-161`), with `panics` on the message
`declares catch-all '*rest' before its last segment` (`trie.rs:157`).
Tests `a_non_terminal_catchall_panics_at_registration` (`:462`) and
`a_terminal_catchall_registers_and_matches_its_remainder` (`:470`, asserts the captured remainder
and that a terminal catch-all still registers and matches). Mutant `if false` in place of the
position check → the panic test `- should panic ... FAILED` (`0 passed; 1 failed`).
Why panic and not "accept the declaration and ignore the tail": that keeps a route meaning something
different from what it says, which is the defect class this whole register keeps re-measuring.
This does **not** reach **PROB-0031** below — making one mis-registration loud is not the same as
making param names per-registration.

### Registered instead of patched, because each close changes something the ledger settled

- **PROB-0028 — chunked bodies are refused by a *wire*-byte budget while the advertised limit counts
  *decoded* bytes.** Measured on a live server with the default limits (`max_header_bytes` 8192,
  `max_body_bytes` 2,097,152): the connection bound at `app.rs:2148` compares
  `conn_buf.len() + n` against header+body in wire bytes, while `framework/http/src/parser.rs:623`
  and `:635-643` enforce the *same* `max_body_bytes` against the decoded body. A 1-byte-chunk body
  pays 6 wire bytes per payload byte, so a 380,000-byte payload (2,280,063 wire) is answered 413
  although the parser would have accepted it — the effective ceiling for such a client is about
  **18 %** of the 2 MB the configuration advertises. Decode cost stayed linear (50,000 → 23 ms,
  100,000 → 45 ms, 200,000 → 120 ms, 300,000 → 171 ms), so this is a pricing mismatch and **not**
  the quadratic-decode item already on this register. Not patched because connection memory is
  priced by header+body *capacity* (PROB-0008, track 32); letting wire exceed that budget makes the
  registered number false.
  **Closed 2026-10-26 (track 75) as the naming decision this entry said it was.** The quantities are now
  named where a user reads them (`ServerLimits::max_body_bytes`, `AetherConfig::max_body_bytes`, the
  guard's own comment, `framework/README.md`'s `AETHER_MAX_BODY_BYTES` row, the matrix's Body-budget
  row, and the human-facing set — `docs/07` §4, both `docs/09` struct blocks, `docs/00`'s RFC-9112
  items in both languages, `docs/04`'s CL branch), the boundary was
  re-measured rather than carried: with the same defaults and the same 6-wire-bytes-per-payload-byte
  framing, 350 879 payload bytes are served and 350 880 are refused — **16.7 %** of the advertised
  2 MiB, which is the exact ceiling where this entry's "about 18 %" was one sampled upload. No
  framing cap was added: the wire bound already prices framing, and a second cap would refuse
  legitimate clients for a cost the memory budget has already taken. Naming the limit in `docs/`
  meant reading the citations beside it, and seven of those pointed at code that no longer holds the
  snippet the prose claimed; the seven are re-derived with this track and the rest of the set —
  `docs/05`'s response-emit coordinates and the same class in these notes — was registered as
  PROB-0032 and closed by track 80 on 2026-10-27: 29 of the 61 citations in the human set named a
  span that did not hold the prose, all 29 were re-derived, and
  `.project-governance/tools/cite_check.py` now fails on the next one. What an operator still cannot
  do is name the two budgets separately — the wire budget is only ever the sum — so a per-request
  framing allowance beside `max_body_bytes` is an open *idea*, not a defect: nothing measured here
  shows the sum being the wrong number, which is why it is written here rather than in the ledger.
- **PROB-0029 — the buffer-limit 413 is sometimes never delivered.** Same live measurement: at
  2,280,063 wire bytes the client reads `HTTP/1.1 413 Payload Too Large`; at 2,400,063 and every
  larger size tried (410,000 / 420,000 / 430,000 / 500,000 payload bytes) the client reads **nothing**
  — the read fails with Windows `os error 10053` (WSAECONNABORTED) after ~135-150 ms. The buffer-limit
  path (`app.rs:2148-2160`) writes and `break`s while the parse-error path (`:2080-2097`) writes,
  `shutdown()`s and only then leaves. Not patched because the three candidate closes (drain-and-discard
  under a bounded timeout / close immediately and document the reset / refuse at the first read past
  budget) are a **denial-of-service posture** choice, not a local fix, and draining lets a client keep
  a connection task busy sending bytes the server will throw away.
- **PROB-0030 — WebSocket frames are written with no deadline.** `WebSocketLimits`
  (`framework/websocket/src/connection.rs:22-32`) carries only `read_timeout` (:32, default 120 s),
  the read is bounded (:146) and `write_frame` (:376-378) awaits `write_all`/`flush` with no deadline
  from paths the peer controls (ping reply :172, close echo :180, `fail()` :392). A peer that pings
  and then stops reading can park a session in TCP zero-window indefinitely, holding the ~33,570,816 B
  registration the upgrade was priced at (the number
  `live_websocket_session_holds_its_buffer_registration` asserts). The HTTP/1 frontend bounds its own
  writes with `limits.write_timeout`, so the asymmetry is inside one framework. **Evidence status:
  read from the code, not reproduced** — no test in this tree has a session blocked in a write.
- **PROB-0031 — one node binds one param name per method.** `RouteNode::param_names` is
  `Vec<(Method, String)>` keyed by method alone and `set_param_name` keeps the first entry
  (`framework/router/src/node.rs:57-72`), while `trie.rs:265/:286` build the captured key from
  `param_name_for(method)` and `serve.rs:262-267` hands those pairs to the handler. So
  `/users/{id}/edit` and `/users/{user}` — different paths, same final node for GET, and *not*
  caught by `reject_duplicate`, which checks method + handler — make whichever route matched report
  the other route's key. This is the subject the ledger recorded as
  “Router merges cross-named path params silently”; the comment claiming a re-registration is
  impossible is false for that reason. **Evidence status: read from the code and reasoned about the
  collision; **closed on 2026-10-26 by track 71**, which produced the missing test and the fix (see
  the track-71 section at the end of this register).** Not patched at registration time because the
  correct shape threads the name through the match (a router-internal API change) and the quick
  patch — last-registration-wins — is the same bug in the other direction.

### Minor findings from the same read (DOCUMENT-class; none is authorized work)

- A handler-authored informational response other than `101` is written as a bare status line, body
  forbidden by `status_forbids_body` (in `framework/http/src/response.rs`, which covers
  `100..200`), so the client waits for a response that is legal to end there. Track 58 (**A6**)
  refused `101` deliberately and *stated* that nothing else in the informational class was decided —
  this line is that remaining edge, not a contradiction of A6. **Closed 2026-10-27 (track 89):**
  the shared response exit in `framework/aether/src/serve.rs` replaces any handler status in
  `100..200` with `500` before the head is written, so no bare informational status line can reach a
  client. `100 Continue` is written straight to the stream by the read loop and never passes that
  exit, so the legitimate case is untouched. Pinned by
  `every_1xx_from_a_handler_is_refused_not_only_101` (`103` and `199` both answer `500`); the rule
  was falsified in both directions by narrowing the condition to `status == 101`, which made
  `a_101_without_a_handoff_is_never_served` fail `left: 103, right: 500`.
- `err.error_status()` reaches the status line unclamped — the call in the blanket
  `impl IntoResponse for Result<T, E>` in `framework/aether/src/app.rs` (locate with
  `grep -n "err.error_status()"`): the value comes from a user-implemented `IntoResponseError`, and
  no range check sits between it and the wire — `canonical_reason` and `Response::json_status` both
  take the `u16` verbatim — so `fn error_status(&self) -> u16 { 42 }` or `999` is served as written.
  **Closed 2026-10-27 (track 89):** the blanket impl now replaces a declared status outside
  `400..=599` with `500` — code `E5002`, canonical phrase, detail dropped — while `418` still
  answers `418` with its own phrase and detail. Pinned by
  `an_error_status_outside_the_error_range_answers_500` (`/declared-success` 200 and
  `/declared-invalid` 700 both answer `500`; `/declared-teapot` 418 keeps itself).
- `post_process` in `framework/aether/src/serve.rs` pushes `X-Request-ID` unconditionally (locate
  with `grep -n 'X-Request-ID'`), so a response that already carries the field ends up with two
  entries of the same name. **Closed 2026-10-27 (track 89):** it now retains every field whose name
  is not `x-request-id` and pushes exactly one framework id, so a handler's copy is replaced rather
  than appended. Pinned by `a_handler_cannot_publish_a_second_x_request_id` (one non-forged id, and
  an unrelated `X-Kept` survives).
- Two reason phrases for one status inside one framework: the hand-written refusals say
  `Payload Too Large` (the `AetherError::BodyLimitExceeded` arm and the buffer-limit path in
  `framework/aether/src/app.rs`, and `Refusal::TooLarge` in `http2.rs`) while
  `canonical_reason(413)` says `Content Too Large` (asserted by `test_canonical_reason_phrases` in
  `framework/http/src/response.rs`) — RFC 9110's current name. `grep` over
  `framework/tests/tests/` finds **no** test asserting `Payload Too Large`, but `docs/00` does name
  the phrase in both languages (line 204 of each), so aligning them costs no test churn and two doc
  lines; it does change bytes on the wire, which is why it is registered and not done.
  **Closed 2026-10-27 (track 89):** all three code sites now call `canonical_reason(413)`, and both
  `docs/00` lines say `413 Content Too Large`. Pinned by
  `a_body_limit_refusal_names_the_canonical_413_phrase` (a handler's `BodyLimitExceeded` and a
  3,000,000-byte declared head refused at the 2 MiB default) plus the `http2` unit test of the same
  name for `Refusal::TooLarge`.
- `AetherConfig::x_request_id` was declared, documented and **never read** — `grep` found the field
  only in `framework/aether/src/config.rs`, so setting it `false` changed nothing while its doc
  claimed the header was under the app's control. **Closed 2026-10-27 (track 89):** the flag is
  carried into `ServiceCore` and gates the header in `post_process`; the doc was corrected too —
  disabling drops the field, not the id, which is still generated for the request context and the
  error reporter. Pinned by `the_request_id_header_follows_the_config_flag`.

  *(Coordinates in these four bullets are symbol anchors since 2026-10-27 (track 88). The block
  carried line numbers until now, and tracks 84–86 moved two of them without touching the findings:
  the `X-Request-ID` push went from `serve.rs:508` to `:634`, and the unclamped call from
  `app.rs:201` to `:229`. The `Payload Too Large` sites this block listed as four are three on
  today's tree. The "no doc asserts it" sentence was measured on 2026-10-21 and is no longer true.)*

  *(All five bullets above are closed by track 89 on 2026-10-27. The fifth — the dead
  `x_request_id` knob — was found while fixing the `X-Request-ID` bullet and is recorded here rather
  than quietly fixed, because it was a documented setting that did nothing.)*
- `handle_connection`'s read loop in `framework/runtime/src/worker.rs` parses each `&read_buf[..n]`
  standalone and discards the `_consumed` length that `RequestView::parse` returns, so that loop
  cannot serve a request split across two reads nor two pipelined requests in one read. It is **not**
  on the `AetherApp` path — nothing in the workspace calls `handle_connection` (only its own
  re-export in `runtime/src/lib.rs`, and the crate has 0 tests) — yet the `aether-runtime` dependency
  in `framework/aether/Cargo.toml` is still declared. Either finish it or drop the dependency; today
  an embedder can wire it without seeing that it cannot answer a split request.
  **Finished 2026-10-27 (track 90):** the loop accumulates into `conn_buf`, serves every complete
  request the buffer holds before reading again, advances by the parser's `consumed`, and answers
  a framing error with `400`/`413`/`431` plus `Connection: close` instead of letting `?` drop the
  socket with no answer. The crate has its first tests (`framework/runtime/tests/connection.rs`,
  3): one for the split read, one for two pipelined requests, one for the framing error. The
  dependency itself is untouched — that is register D6, still an owner decision about the publish
  payload — so the crate is repaired, not adopted.

### Gates on the final tree (all re-measured after the last edit, Windows 10.0.26200 MSVC)

- `cargo test --workspace` exit **0** — **503 passed / 0 failed / 1 ignored** over 50 `test result:`
  lines = 290 unit + 208 integration + 5 doc. From track 68's 497: `aether` lib 47 → **50**
  (three in `transport.rs`; per file `tls.rs` 10, `app.rs` 17, `extensions.rs` 8, `http2.rs` 12,
  `transport.rs` 3), `aether-router` lib 20 → **22** (both in `trie.rs`, which now holds all 22 of the
  crate's tests), `http_e2e` 48 → **49**; `scheduling` stayed at 16 — that test was rewritten, not
  added.
- `cargo fmt --all --check` exit **1** at the standing **570 hunks over 65 files** (re-counted from
  `^Diff in ` lines, `C:/tmp/t69_fmt_final2.log`) — the track added no fmt debt, because every line
  it added was formatted before it was written.
- `cargo clippy -p aether --all-targets --no-deps -- -D warnings` exit **101**: **13 lib + 16
  lib-test**, and the *identities* are the HEAD baseline's (lint name × count compared line-by-line
  against `C:/tmp/t69_clippy_aether.log`; the `transport.rs:23` `large size difference between
  variants` finding is on the `Stream` enum, which this track did not touch, and no finding site lies
  in a track-69 range). `cargo clippy -p aether-router --all-targets --no-deps -- -D warnings` exit
  **101**: 3 lib-test findings, same three names as the baseline (`stripping a prefix manually`,
  `this if statement can be collapsed`, `this function has too many arguments (8/7)`).
- `.project-governance/tools/project_governance_bootstrap.py validate` exit **1** — unchanged
  reading: the three pre-existing `[RECYCLED ID]` rows (PROB-0007, PROB-0017, REQ-0001) plus
  `[OPEN VS HISTORY] REQ-0001`. PROB-0028…0031 added no new violation.
  `fmt`, `clippy` and `validate` are **still red** for these recorded reasons; the project is not
  green overall.

## Track 70 — PROB-0029 closed: a refusal has to reach the client (2026-10-26)

### The defect, re-measured before it was touched

Track 69 registered this as "a buffer-limit 413 is never delivered". The first thing done here was
to reproduce it as a test on the unmodified tree:
`an_over_budget_chunked_upload_is_still_answered_with_413` (400 000 payload bytes in one-byte
chunks = 2,400,000 wire bytes against the 2,105,344-byte default connection budget) failed in 0.15 s
with **`An established connection was aborted by the software in your host machine. (os error
10053)`** and `upload write: Ok(())` — the client had finished queueing its upload and read nothing
for a request the server had already decided about (`C:/tmp/t70_probe_before.log`).

That reproduction corrected PROB-0029's own diagnosis in one respect. The registered text said the
buffer-limit path "writes the response and breaks, leaving the socket to Drop", as if no FIN were
sent. It is sent: `break` falls to the loop tail, and `framework/aether/src/app.rs:2206` shuts the
stream down before the guard completes. So a graceful FIN alone was never the missing piece — what
is missing is the **discard of the bytes the peer already queued**. Windows aborts a socket that is
closed with received data still unread, however politely it was shut down first, and that abort
takes the just-written response with it. Track 69's own hedge ("a graceful FIN is not obviously
sufficient here either") was the right instinct, and it is why `error_responses_survive_unread_pipelined_bytes`
(still green) survives at 20 KB of unread header and this fails at 2.4 MB of unread body.

### Fixed

One helper, called from the two sites that refuse a request on bytes:

- `refuse_over_budget` — `framework/aether/src/app.rs:2466`. Serializes the 413 + `Connection: close`,
  releases the refused request's blocks, writes the answer under `write_timeout`, shuts the write half
  down (FIN), then reads the peer's queued bytes into the caller's 8 KiB buffer and throws them away
  until EOF or error, bounded by `read_timeout`.
- the static connection bound — `framework/aether/src/app.rs:2158-2166` (was an inline 413 + `break`);
- the scheduler re-price refusal — `framework/aether/src/app.rs:2188-2199` (same inline shape).

The candidate fixes the problem text weighed were (a) drain under a bounded timeout, (b) close
immediately and document that an over-budget request may be reset, (c) refuse at the first read past
the budget. (a) was chosen, and the exhaustion objection that made track 69 hesitate does not bite,
because the objection is about *memory* and this drain grows none:

- the bound exists to stop `conn_buf` growing; every discard pass reads into the fixed 8 KiB
  `temp_buf` and drops it, so resident bytes only go down;
- the refused request's own 2 MiB block and the chunked decoder's copy are released **before** the
  wait, so the connection stops holding what it decided never to use rather than holding it through
  the drain;
- the wait is bounded by `read_timeout` (5 s default), which is already the bound the same loop
  applies to a request that has not finished arriving, while holding the same permit and the same
  admission slot. A client that keeps sending junk therefore cannot pin a task for longer than a
  client that trickles a legitimate request — which is the exposure the tree already accepts.

(b) was rejected because the code would keep claiming an HTTP answer for a request it cannot reply
to; (c) is not a fix at all, since the refusal point is already the first read past the budget —
that is exactly why 287 KB were still in flight.

### Tests, and what each one actually pins

| test | site | pins | mutant readings (4 repeats each) |
|---|---|---|---|
| `an_over_budget_chunked_upload_is_still_answered_with_413` (`framework/tests/tests/http_e2e.rs:1450`) | static bound | the status line is read, `Connection: close` is there, the handler never runs | **M1** (FIN, no drain) → 4/4 FAILED, `os error 10053`; M2, M3 → green |
| `a_buffer_limit_refusal_finishes_the_exchange_without_waiting_for_the_client` (`…/http_e2e.rs:1523`) | static bound | the FIN goes out *before* the drain: a client that keeps its half open reads answer + close inside 2 s | **M2** (drain, no early FIN) → 4/4 FAILED at the 2 s deadline (2.02 s); M1 → 4/4 FAILED, `expected a 413 status line, got ""`; M3 → green |
| `a_memory_budget_refusal_reaches_a_client_that_is_still_sending` (`framework/tests/tests/scheduling.rs:741`) | re-price | the re-price refusal reaches the client with 413 + close, no handler, and `memory_shed ≥ 1`, `completed ≥ 1`, `cancelled == 0` | **M1** → 3/4 FAILED (the weaker detector, stated as such in its own doc); M2, M3 → green |

Readings from `C:/tmp/t70_mutants.log`, `C:/tmp/t70_mutants2.log` and the repeat run; every mutant
was applied to `app.rs` on the final tree and restored byte-identically (`restored identical: True`,
147 839 bytes, 0 CR — `app.rs` is LF, unlike `transport.rs`).

**Stated as unverified, not silently claimed:** M3 — removing the release of the refused blocks
before the drain — left all three tests green. No test samples RSS, so "those megabytes stop being
resident immediately" is argued from dropping a `BytesMut`, not measured. And the re-price site
failed M1 only in 3 of 4 repeats: the delivery claim rests on the static-site test.

### Still open, with the reason sharpened

- **PROB-0028 stays open, and is now a naming/doc decision rather than a pricing bug.** Reading
  where `consumed` is applied: `parse_request_incremental` returns it only with a *completed* request
  (`framework/aether/src/app.rs:1685-1690`), and the only `split_to(consumed)` calls are at `:1966`
  and `:2009`. So while a body is incomplete, every wire byte really is resident in `conn_buf`
  (and the chunked decoder's copy alongside it, which `:2185` already charges). The static bound
  therefore prices memory **correctly**; what is wrong is that `max_body_bytes` is advertised as a
  body limit while the enforced ceiling for a chunked client is a wire budget — 18 % of the
  advertised 2 MiB at one-byte chunks. Fix options: cap the framing overhead (chunk count / max
  chunk size), or say in the docs and the config field that the read path bounds wire bytes.
- The parse-error 400/413/431 path (`framework/aether/src/app.rs:2100-2126`) deliberately was **not**
  given the drain. Its backlog is one oversized header line, `error_responses_survive_unread_pipelined_bytes`
  pins it green today, and there is no measured failure there to fix.

### Gates on the final tree (all re-measured after the last edit, Windows 10.0.26200 MSVC)

- `cargo test --workspace` exit **0** — **506 passed / 0 failed / 1 ignored** over 50 `test result:`
  lines = 290 unit + 211 integration + 5 doc (`C:/tmp/t70_suite_final.log`). From track 69's 503:
  `http_e2e` 49 → **51**, `scheduling` 16 → **17**; every in-crate lib target is unchanged
  (`aether` 50, `aether-http` 92, `aether-middleware` 59, `aether-router` 22, `aether-websocket` 25,
  `aether-core` 8, `aether-security` 7, `aether-scheduler` 11, `aether-dependency` 4, `aether-tests` 12).
- `cargo fmt --all --check` exit **1** at the standing **570 hunks over 65 files**
  (`C:/tmp/t70_fmt3.log`) — the track added no fmt debt; two hunks appeared mid-track (a wrapped
  `timeout` call, a one-line `assert!`) and were hand-formatted back to the baseline.
- `cargo clippy -p aether --all-targets --no-deps -- -D warnings` exit **101**: **13 lib + 16
  lib-test**, 18 `error:` lines, and the name × count histogram is identical to track 69's
  `C:/tmp/t69_clippy_aether_final.log` (9 × collapse-if, 4 × default-field-assign, 2 × large-variant-
  size, 1 × empty-line-after-doc, 2 × could-not-compile). `cargo clippy -p aether-router …` exit
  **101**: the same 3 lib-test findings (`diff` against `C:/tmp/t69_clippy_router_final.log` empty).
- `.project-governance/tools/project_governance_bootstrap.py validate` exit **1** — the same four
  lines (`C:/tmp/t70_validate.log`): three pre-existing `[RECYCLED ID]` rows plus
  `[OPEN VS HISTORY] REQ-0001`. `status` after closing PROB-0029: Requirements 1 | Problems **10** |
  Proposals 1 | Criticisms 0.
  `fmt`, `clippy` and `validate` are **still red** for these recorded reasons; the project is not
  green overall.

### The count sweep this track owed the live docs

Every current-tense count claim re-derived from the same 50 `test result:` lines, not copied from an
older doc: `README.md` (503 → **506**, raw-socket 49 → **51**, scheduling wire 16 → **17**),
`framework/README.md` (503 → **506**, 208 → **211** integration, `http_e2e` 49 → **51**, `scheduling`
16 → **17**), `framework/docs/BASELINE_MEASUREMENTS.md` (provenance chain extended with track 70, the
per-target integration row and the total row), and `framework/docs/FEATURE_COMPLETENESS_MATRIX.md`
(total, the 474-since-delta line 29 → **32**, `http_e2e`, `scheduling`, the shedding row's "16 wire
tests" → **17** and its "recounted 2026-10-23 by grep" method replaced with the `test result:` line it
should have used, and the `aether` lib split — that line still said "the 47-test `aether` lib total"
while the same paragraph counted `tls.rs` 10 + `app.rs` 17 + `extensions.rs` 8 + `http2.rs` 12 = 47 and
omitted `transport.rs` 3; recounted from the attributes it is 10 + 17 + 8 + 3 + 12 = **50**).
`framework/docs/PRODUCTION_READINESS_REPORT.md` had no current total at all: its re-measurement chain
ended at "the sixth describes the tree in the repository" (**474**, 2026-10-23) and §10 claimed 49
raw-socket tests. A seventh reading was added from this track's log (unit **290** = 92 + 59 + 50 + 25 +
22 + 12 + 11 + 8 + 7 + 4, integration **211** over the same 15 targets, doc **5**, 50 result lines,
**506 / 0 / 1**) and the stale "sixth describes the repository" sentence was marked as superseded
in-place rather than deleted; §10's `http_e2e` line went 49 → **51**. Dated historical counts (the PRR
banner, track 69's own gates paragraph, the register's earlier track entries) were left exactly as
written — they describe the trees they measured.

`framework/docs/SCHEDULING_COMPARISON.md:13-14` called `cargo test -p aether --test scheduling`
"8 wire-level integration tests" and `cargo test -p aether-scheduler` "17 engine tests + 8 simulator
invariant tests". Both sit in a block whose measurements are dated 2026-10-18 and must stay dated — but
the commands are written as what a reader runs *now*, and a reader running them now gets 17 and 57. Each
was given its own date instead of being silently corrected: "8 wire-level integration tests (real HTTP)
on 2026-10-18; the same target reports 17 on the 2026-10-26 tree", and the scheduler split is now the
`test result:` list it actually prints (11 unit + `engine` 21 + `sim` 9 + `pump_head_of_line` 8 +
`memory_reprice` 4 + `policy_extension` 2 + `stress` 2 = 57, matching the framework README's 11 + 46).
The same treatment for §"Limitations"'s "(8 wire tests, all passing)". What was *not* re-derived is that
doc's percentile tables — they are 2026-10-18 measurements and the doc says so; re-running the simulator
is a benchmark task, not a count sweep, and no number in it was changed here.

### Re-verified on the committed tree

`git status` clean after the three commits, and no Rust file was edited between the measured 506-run
and the code commit, so the committed tree is the measured one: `cargo test --workspace` re-run on it →
exit **0**, 50 `test result:` lines, **506 passed / 0 failed / 1 ignored**
(`C:/tmp/t70_head_suite.log`). `validate` re-run after the markdown edits: exit **1**, the same four
lines, `status` still Requirements 1 | Problems **10** | Proposals 1 | Criticisms 0
(`C:/tmp/t70_validate3.log`). The fmt (**570** hunks / **65** files) and clippy (**13** lib + **16**
lib-test for `aether`, **3** for `aether-router`) readings above were taken on this same Rust source
and are quoted from `C:/tmp/t70_fmt3.log` / `C:/tmp/t70_clippy_aether.log` rather than re-run, because
the only later edits were markdown.

### Two things left as found, stated rather than implied

- **The drain went only where a failure was measured.** The WebSocket memory-reprice 503
  (`framework/aether/src/app.rs:1914`) and the parse-error 400/413/431 path still write, shut the write
  half and close without discarding. Both have the same *shape* as the site this track fixed, and both
  have a backlog bounded by a parsed request head rather than a body the client is still pushing, so
  `error_responses_survive_unread_pipelined_bytes` covers them at 20 KB. Nothing was measured failing at
  either, so nothing was added — the same rule B21 registers, applied in the other direction: a site is
  not fixed because its neighbour needed fixing.
- **The status line still deviates, unchanged.** `refuse_over_budget` kept the inline code's
  `Response::write_http1_to`, so an HTTP/1.0 client that trips a byte budget reads `HTTP/1.1 413`.
  `PRODUCTION_READINESS_REPORT.md:166-167` already records that deviation for the interim/error
  responses; this track inherited it rather than introducing it. Making the refusal version-aware is a
  separate change with its own wire test, and folding it into a delivery fix would have put two claims
  in one commit.

## Track 71 — PROB-0031 closed: a shared `Param` level must report each route's own name (2026-10-26)

The owner's mandate for this track was «Ok تتخذ قرة» / the standing decision-independence policy: pick
the next defect and close it. PROB-0031 was chosen because it was the largest registered logic defect
whose close was a code change rather than a scope or pricing decision, and because it was the one
item on the list whose evidence status was still only "read from the code".

- **The reproduction was run before the fix, and it failed for the documented reason.** Registering
  `GET /users/{user}/edit` (10) and `GET /users/{id}` (20) and matching `/users/42` returned
  `params = [Param { key: "user", value: "42" }]` — handler 20's own path says `id`, but the shared
  `Param` node answered `param_name_for(GET)` with the first name ever registered for that method.
  Probe log: `C:/tmp/t71/`.
- **Why `reject_duplicate` cannot catch it.** It compares the (method, path) pair at the leaf. These
  are two different paths whose third segment competes for the same radix level, so no duplicate is
  ever seen; the collapse happens in the *name*, which the old model stored once per (node, method).
- **The shape chosen, and the one rejected.** Chosen: record the matched route's ordered dynamic
  names on the node that holds its handler (`param_names: Vec<(Method, Vec<String>)>`, written once
  by `add_route` from the `declared` list) and restamp `Param.key` at the `Search::Found` sites via
  `RadixRouter::resolve_keys`. Rejected: *last-registration-wins* — the same defect pointed the other
  way, and it would still answer one route's question with the other route's word. Rejected:
  storing names on the `Param` node itself — that node is shared, which is the whole problem.
- **Lifetimes and allocation stayed as they were.** `Param<'a>` borrows its key from the trie, so
  restamping from the leaf's `&'a [String]` needs no new allocation, and `resolve_keys` runs on the
  clone taken while the leaf node is in scope, preserving the 4-parameter `SmallVec` stack fast path
  the `params.rs` doc promises.
- **Test sensitivity was proven, not assumed** (`C:/tmp/t71/router_sensitivity.py`,
  `C:/tmp/t71/revert_probe.py`, `C:/tmp/t71/m3.py`, each restoring the tree byte-identically):
  injecting the two new `trie.rs` tests into the committed (pre-fix) node.rs+trie.rs makes both fail
  (2 failed, exit 101); the `extractors.rs` wire test fails (1 failed) on the same reverted tree, so
  the restamped key is proven to survive `serve.rs`'s param copy; and mutant M3
  (`params.iter_mut().rev().zip(names)`) fails the two-param test it initially passed. M1 (drop the
  leaf restamp) is killed by three tests, M4 (never record `declared`) by one.
- **fmt debt was returned, not added.** The first `cargo fmt --all --check` after the change read 573
  hunks / 65 files against the standing 570 / 65. All three new hunks were this track's own lines —
  two over-long `assert_eq!` in `extractors.rs` and one stray blank line at `node.rs` EOF — corrected
  by hand to rustfmt's own output. The pre-existing drift *in those same files* was left untouched,
  because reformatting a file one edits is a repo-wide reformat in disguise and moves the gate
  reading without making the project greener. Re-run: 570 / 65, exit 1, unchanged.
- **Count sweep this track owed the live docs** (all re-derived from the 50 `test result:` lines, not
  copied from a doc): total **509** = 292 unit + 212 integration + 5 doc; `aether-router` lib 22 →
  **24**; `extractors` 18 → **19**. Updated: `README.md`, `framework/README.md`,
  `framework/docs/BASELINE_MEASUREMENTS.md` (provenance chain + router row + integration row + total
  row + the 292 note at the Unit Tests snapshot), `framework/docs/FEATURE_COMPLETENESS_MATRIX.md`
  (evidence block, per-target list, Typed-extractors row, Unit + Integration rows, "35 tests added
  since"), `framework/docs/PRODUCTION_READINESS_REPORT.md` (new eighth reading, the seventh marked
  superseded in place, §4.3 router entry + its stale "16 tests"),
  `.project-governance/WORKING_CONTEXT.md` (counts paragraph + this track's entry).
- **Left as found, stated rather than implied.** Cross-method naming (`/items/{id}` GET vs
  `/items/{item}` POST) was already handled by the per-method map and its two tests were green before
  and after — this track fixed the *sibling-same-method* case only. The `Path<T>`/`Query<T>`
  extractor docs needed no change: the defect was in which key a capture arrived under, not in how a
  key is read.

## Track 72 — PROB-0030 closed: the write side of a WebSocket session gets a deadline (2026-10-26)

The owner's mandate for this track was «اتخذ قرار» under the standing decision-independence policy:
take the next registered problem and close it on the evidence. PROB-0030 was chosen because it was the
only open problem whose close was a contained code change — PROB-0016/0027/0028 are pricing decisions,
PROB-0011/0005/0009's remaining halves are API breaks that need the owner's release decision, and
PROB-0012 needs a machine this project does not have.

- **It was reproduced at two levels before the tree was touched.** Deterministic: `WebSocket` over
  `tokio::io::duplex(8)` with a peer that never reads — `send_text` parks, and the read deadline cannot
  fire because a task sitting in a write never reaches `recv`. Wire: a live TCP client that reads the
  `101` and nothing else, against `read_timeout: Some(300ms)`, handler sending 1 MiB frames; six seconds
  in the scheduler still reported `active=1 registered=33603584` (`C:/tmp/t72/probe_b.log`, scratch).
  That number is the consequence stated in bytes: the 33,570,816 B buffer pair plus the 32,768 B
  connection base, held for as long as the peer liked, by a session that could be one line of handler.
- **The shape came from inside this repo.** `WebSocketLimits::write_timeout` (default 10 s — the value
  `ServerLimits::write_timeout` already uses for an HTTP/1 response and for the `101` write, so this
  closes an asymmetry rather than inventing a policy), `WsError::WriteTimeout`, and a `WriteBudget`
  renewed only after `MIN_PROGRESS_FOR_RENEWAL` = 1024 B has left, which is the earned-renewal rule
  track 67 settled for HTTP/2 (`http2.rs:521`). **Rejected: a total cap per frame.** It is the weaker
  option and it fails a peer that consumes normally but slowly — exactly the peer a bounded *interval*
  is supposed to keep alive; D1 refused the same shape for HTTP/2 responses, and `http2.rs:529-531`
  records why HTTP/1 differs (one socket, one response) and why a session, which is neither, follows
  the interval rule.
- **The API break was checked on this host, not argued.** `WebSocketLimits` is not
  `#[non_exhaustive]`, so a new public field breaks out-of-tree struct-literal construction; a private
  field or builder breaks it *more* (E0451 plus functional-update-expression rules), so the public
  field is the least-breaking shape available. Host-wide search found no source-level consumer of
  `WebSocketLimits` or `WsError` in `my_cool_site`/`threekway` (the path-dep consumer), `aether-probe`,
  `aether-bench-compare` or `benchmarks/rust_aether` — only build artifacts. Compiled on the post-fix tree:
  `my_cool_site`/`threekway` and `aether-bench-compare` both `cargo check` → exit 0 (each with warnings of its
  own); `aether-probe` and `benchmarks/rust_aether` fail `cargo check` today for the pre-existing
  unannotated-`get_async` inference reason recorded in the track-45 entry above, not for anything this track
  touched. So the honest statement is
  "verified against the consumers on this host", not "cannot break anyone".
- **Four tests, three mutants, no overlap** (`C:/tmp/t72/mutants_m1m3.log`, scratch): M1 (remove the
  bound) is killed by the stall test, the trickle test and the wire test; M2 (absolute per-frame cap)
  only by `a_peer_that_reads_slowly_gets_the_whole_frame`; M3 (renew on any progress) only by
  `a_peer_that_trickles_one_byte_per_interval_is_shed`. Each test therefore owns a distinct failure
  mode and none is a restatement of another.
- **Both of the test-shape lessons this track learned came from running the mutants.** M2 *hung* the
  suite first: the paced reader's drain loop had no EOF exit, so a writer that gave up left the test
  spinning — a detector that hangs reports nothing, so the loop now breaks on `Ok(0)` and the test
  fails instead. M3 *survived* first: the trickle completed its 20 drains before the assertion ran, so
  a parked writer was being judged after it had already been abandoned; the test now races the live
  trickle against the send future with `tokio::select!`, and M3 dies. A mutant that passes is a hole in
  the test, not a free pass for the code.
- **Gates, as measured on the final tree.** `cargo test --workspace` exit 0 at **513 / 0 / 1**, 50
  `test result:` lines = 295 unit + 213 integration + 5 doc (`aether-websocket` 25 → 28, `scheduling`
  17 → 18). `cargo fmt --all --check` exit 1 at the standing **570 hunks / 65 files** — it first read
  574, the four extra hunks all inside this track's new test, corrected by hand; the pre-existing drift
  in `scheduling.rs` (43 → 39 hunks in that file alone) was deliberately *not* reformatted. `cargo
  clippy -p aether-websocket --all-targets` is clean, 0 warnings, and neither of the `scheduling`
  target's 2 warnings falls in a track-72 range (they are at `scheduling.rs:52` and `:197-198`).
  `validate` exit 1 with the same three `[RECYCLED ID]` rows and `[OPEN VS HISTORY] REQ-0001`;
  `status` recount: Problems 9 → **8**. `fmt`, `clippy` and `validate` stay red for the recorded
  reasons, so the project is not green overall.
- **Count and coordinate sweep this track owed the live docs** (every figure re-derived from the run
  above or from the file it describes): `README.md`, `framework/README.md` (total, split, `scheduling`
  18), `framework/docs/BASELINE_MEASUREMENTS.md` (provenance chain, `aether-websocket` row,
  integration row, total row, and the Unit-Tests snapshot note, which read **268** while the table
  beside it read 292 — a self-contradiction in one sentence, now both 295),
  `framework/docs/FEATURE_COMPLETENESS_MATRIX.md` (evidence header, its dated chain and its "35 tests
  added since" figure, which is 39, per-target list, `http2_e2e` 36 → 37 re-confirmed by `grep -c
  tokio::test`, the WebSocket driver row, Unit + Integration rows),
  `framework/docs/PRODUCTION_READINESS_REPORT.md` (a ninth dated reading, the eighth superseded in
  place), `framework/docs/ACTIX_FAIR_COMPARISON.md` (F7 rewritten: its coordinates were dead —
  `connection.rs:30-42`/`146-149`/`604`/`617` and `app.rs:1148-1152`/`1940-1944` — and the row claimed
  F7 was closed when only the *read* half ever was), and `.project-governance/WORKING_CONTEXT.md`
  (counts paragraph + this track's entry).
- **Left as found, stated rather than implied.** A peer that pays 1024 B per interval keeps a session
  for an unbounded total time — that is the definition of an interval bound, not an oversight, and the
  test that proves it also proves the frame arrives whole. A write that times out can leave a partial
  frame on the wire; `write_frame`'s doc says so, and no close frame is promised afterwards. Nothing
  was measured over a real slow link or behind TLS (PROB-0012 still open). The scheduling-layer release
  is asserted on the wire; the task abort itself is the existing handover path, unchanged here.
- **Also delivered this track, at the owner's request** («اعمل خطط لاصللح مشاكل وتحسين اطر العمل»):
  `.project-governance/notes/2026-10-26-remediation-roadmap.md` — tracks 73–79 sequenced by what is
  blocked on evidence versus what is blocked on an owner decision or a machine, with the current gate
  table. Tracks 73 (the standing `fmt`/`clippy`/`validate` debt), 74 (PROB-0016/0027 pricing), 75
  (PROB-0028) and 79 (evidence durability — this track's own probe logs are in `C:/tmp` and will be
  deleted, which is why the durable figures are in the table and the tests) proceed without asking;
  76 (the breaking halves), 77 (Linux/macOS parity) and 78 (perf, which is the owner's machine and the
  owner's measurements) do not.

## Track 73, slice 1 — the `validate` gate cannot detect anything any more, and the ratchet that fixes it (2026-10-26)

Started under «كمل الشغل لحالك واتخذ قرار بنفسك بما يتوافق مع رؤية المشروع», taking the roadmap's own
first step (ledger → lints → format).

- **The roadmap's own premise was wrong and is corrected in place.** §2 said the recycled rows were "a
  few lines of ledger repair". They are not repairable: the recycles are records in the append-only
  `event_log.jsonl` (`REQ-0001` on lines 1, 4 and 9 with three different titles), so the only repair is
  rewriting history, which the project rules refuse. The plan text was written from the flag's wording
  rather than from the file behind it — the same mistake class the register has named before.
- **Renumbering the live requirement was evaluated and refused on evidence.** `REQ-0002` is a real
  closed allocation (log lines 5 and 7), so `REQ-0003` is genuinely never-used and `next_id()` proves
  it; one `renumber` action plus 20+ citation edits across `WORKING_CONTEXT.md`, this register, the
  roadmap and `ADAPTIVE_TREE_SCHEDULING_DESIGN.md` would clear `[OPEN VS HISTORY]` — and `validate`
  would still exit 1 on the three historical rows. The edit would also rewrite dated records of what
  earlier tracks measured, so those records would name a number that did not exist when they were
  written. Tracks 53/54 already reached the same conclusion in-repo ("No shipped doc is renumbered").
- **The defect that is real: a permanently-red check has no marginal information.** Before: four lines,
  exit 1. After any *new* violation: still four-plus lines that read identically to anyone comparing
  exit codes, which is what every other track does. Closed by pinning, not by softening:
  `.project-governance/tools/tests/test_recycling_ratchet.py`, 5 cases, `validate`'s code, output and
  exit semantics untouched — no acknowledgement list, no void entry, no severity tier, so nothing here
  can excuse a future reuse. Exact-equality baselines for `[RECYCLED ID]` and `[OPEN VS HISTORY]`, the
  recycled set recomputed independently from `ledger_allocations()` so the baseline is checked against
  the data and not against the report of the data, a "no other flag class is standing" case, a
  sensitivity case that copies the real ledger and forges two second allocations (a closed ID, visible
  only to the ledger, and an open one that must join both flags), and a refusal case showing that
  keeping one allocation per ID makes the same validator print `[VALID]` / exit 0.
- **Seven mutants, every one killed by a named test** (`C:/tmp/t73/mutants.py`, scratch; byte-identical
  restore asserted, mutation-landed asserted before each result): omit a real member from the baseline
  (3 failures), add a phantom to the open baseline (2), assert the real ledger green (1), drop the
  reused-and-open forgery from the expected recycled union (1), from the expected open union (1), add a
  phantom recycled member (2), assert the pruned copy stays red (1). The first version of the
  sensitivity case asserted membership plus a subset relation and **survived** being weakened — that
  survival is why the exact-union form on both flags is what ships, and it is also this register's
  standing rule applied to governance tooling rather than only to Rust.
- **Counts.** Governance suite 37 → **42** green (`python -B -m unittest discover -s
  .project-governance/tools/tests`; 26 `test_id_allocation` + 11 `test_ledger_location` + 5 new).
  `validate` on the repository: unchanged and still **red**, exit 1, the same four lines. Rust gates
  unchanged by construction and verified as such — `git diff --stat HEAD -- framework` is empty, and
  the `aether` clippy run re-executed on this tree still reports 13 lib + 16 lib-test. Suite stays
  513 / 0 / 1, `fmt` stays 570 hunks / 65 files.
- **Partial track 79, by shape rather than by plan.** This slice's evidence is a committed re-runnable
  test, not a `C:/tmp` log: the log holds only the mutant transcripts, and the durable claim is the
  ratchet file itself — which is exactly the fix track 79 describes.
- **Next in this track.** clippy batches, in the inventory order recorded in roadmap §2; `trie.rs:261`
  (`too_many_arguments`, the 8-parameter `search`) stays out of lint noise because changing it is a
  signature decision, not a lint.

## Track 73, slice 2 — the clippy debt closed, by a gate stricter than the debt it removed (2026-10-26)

Continued under the same mandate, in the roadmap's order (ledger → lints → format). Three commits, one
per file locus: `ffa19f5` (`aether/src/{app,extract,http2,serve,transport}.rs`, `router/src/trie.rs`),
`1ca4fd4` (`http/src/{parser,static_files,response,multipart,url}.rs`, `middleware/src/{cors,compression,logger,host_validation}.rs`,
`scheduler/src/{engine,quota}.rs`, `scheduler/tests/pump_head_of_line.rs`, `scheduler/examples/active_trace.rs`),
`819fdef` (`security/src/jwt.rs`, `validation/src/simd.rs`, `tests/src/lib.rs`, six `tests/tests/*` files,
`tests/examples/openloop_loadgen.rs`). 28 files, 292 insertions / 223 deletions, no new test and no
removed one.

- **The gate that replaces the debt is stronger than the reading the debt was measured with.** Slice 1
  and every earlier track cited per-crate rows (`aether` 13 lib + 16 lib-test, `aether-router` 3 + 3).
  The closing reading is `cargo clippy --workspace --all-targets -- -D warnings` → **exit 0**, 0 warning
  or error lines in the log. Running the strictest form is what *found* the `security` (8), `validation`
  (2) and `aether-tests` (23) records no per-crate row had ever listed, plus two
  `field_reassign_with_default` sites at `tests/tests/http_e2e.rs:1417-1418` and `:1931-1932` that the
  truncated loci list (`… | head -60`) had hidden. Those were taken in this slice rather than left as a
  second debt; a debt only counts as closed when the gate standing in its place would catch it reopening.
- **Five warnings refused, each annotated with its cost** as `#[expect(lint, reason = …)]`: `router/src/trie.rs:264`
  `too_many_arguments` (recursive trie walk — a context struct adds a per-call aggregate and removes no
  state), `aether/src/serve.rs:87` `large_enum_variant` on `ServeOutcome` (boxing the upgrade context
  costs an allocation per upgrade), `aether/src/transport.rs:23` the same on `pub enum Stream` (boxing
  TLS costs an allocation per connection, which that type's own doc rule refuses), `aether/src/serve.rs:229`
  `collapsible_if` on the CORS preflight (a 30-line commented block re-indented for style reads worse in
  review), and `http/src/static_files.rs:63` `result_unit_err` on the public `safe_resolve_path`. An
  `#[expect]` that stops firing is itself a warning, so the exit-0 `-D warnings` run proves all five are
  still earned — which is the difference between `expect` and `allow` and the reason none of these is a
  silencing.
- **One refusal has a user-visible alternative, so it is routed rather than parked.** `safe_resolve_path`
  returning `Result<PathBuf, ()>` is a public-API wart; a named error type breaks every caller, so it
  belongs on track 76's breaking list with PROB-0011/0005/0009, not in a lint-cleanup commit.
- **API surface: additive or trait-shape, checked by compiling rather than by arguing.**
  `http::parser::ParsedRequest<'req>` is a new documented `pub` type alias for the
  `(RequestView, Option<Vec<u8>>, usize)` tuple two public signatures used to spell out (the alias is
  what `type_complexity` asked for; naming it beats hiding it). `CompressionMiddleware` lost its
  inherent `default()` and gained `impl Default`; all 14 `::default()` call sites — `aether/src/app.rs:738`
  plus the 13 in `middleware/src/compression.rs`'s own tests — still compile, because `T::default()`
  resolves through the trait. `LoggerMiddleware`
  replaced a manual `impl Default` with `#[derive(Debug, Clone, Default)]`, its rationale moved into the
  `enabled` field doc rather than deleted with the impl. No signature lost a parameter or changed a type.
- **Behaviour asserted, not assumed.** The six test-file edits change shape only: `ServerLimits`/`AetherConfig`
  fixtures became one literal with `..Default::default()` so a field can no longer be defaulted then
  forgotten, `enable_host_validation` takes an array, `tests/src/lib.rs` asserts `.is_none()` directly,
  `scheduling.rs`'s helper lost a `map_err(|e| e)` that mapped an error onto itself. In
  `scheduler/tests/pump_head_of_line.rs`, `65usize.div_ceil(WINDOW)` at :221 keeps all four assertions at
  :225-:228 and the binary still runs 8 tests. The suite stays **513 / 0 / 1**, so no assertion was
  traded for a green run.
- **`fmt` debt returned by hand, never by a sweep**: 570 → 561 (batch 1) → **560 hunks over the same 65
  files**; each touched file's HEAD copy and working copy were run through `rustfmt --edition 2024 --check`
  and the `Diff in` counts compared (`C:/tmp/t73/hunkcmp.py`), so no file is worse than it was and the
  drift in the untouched files stays untouched. Shape rule derived empirically, not eyeballed: rustfmt
  2024 splits a let-chain whose *first* term is `let` one operand per line, while a chain starting with a
  bool expression may stay inline — three hand-collapsed sites had to be re-broken for that reason.
- **Two measurement hazards, both killed by cross-checking against a fail-on-anything gate.**
  (a) `rustfmt --config-path . … 2>/dev/null` printed nothing because rustfmt was erroring on the
  argument and stderr was discarded, and `grep -c` reported **0** hunks — an aborted gate is vacuously
  clean; the real reading was 14 → 13. (b) the JSON attribution filter kept only codes starting with
  `clippy::`, so it dropped the `E0277` that `bytes.as_chunks::<8>()` introduced in `validation/src/simd.rs`
  (`&[u8; 8]` does not `TryInto<[u8; 8]>`) and printed `TOTAL 0` on a tree that did not compile;
  `-D warnings` exit 101 caught it, the fix is `u64::from_ne_bytes(*chunk)`, and the only figure
  published from this slice is the one that fails on any warning *or* error.
- **Gates, all re-executed on the final tree** (what follows is outside `framework/`, so it cannot move a
  Rust reading): clippy `-D warnings` exit **0**; `cargo test --workspace` exit 0 at **513 / 0 / 1** over
  50 `test result:` lines; `cargo build --workspace --examples` exit 0 with 0 warning/error lines
  (the suite alone never has to compile `active_trace.rs`); `cargo fmt --all -- --check` exit 1 at **560
  hunks / 65 files**; `validate` exit 1 with the same four standing lines — three `[RECYCLED ID]` and
  `[OPEN VS HISTORY] REQ-0001` — which is now a pinned baseline rather than noise, because of slice 1;
  governance Python suite `Ran 42 tests` / **OK**, exit 0. **Nothing pushed.** `fmt` and `validate` stay
  red for the recorded reasons, so the project is not green overall; `clippy` no longer is.
- **Next in this track.** Slice 3, the `fmt` decision: 560 hunks over 65 files, unreviewable as one
  commit and forbidden as a repo-wide sweep, so it is either ~8 per-crate commits or one dated sentence
  in `PRODUCTION_READINESS_REPORT` accepting the debt. Tracks 74 (PROB-0016 pricing, PROB-0027 as a
  documented rule), 75 (PROB-0028's label) and 79 (evidence durability) proceed without asking; 76, 77
  and 78 stay blocked on the owner.

## Track 73, slice 3 — the `fmt` gate goes green, and the documentation pays for it (2026-10-26)

- **The two options this track offered itself were paid-in-batches or a debt sentence, and the debt
  sentence was refused for the reason slice 1 had just established.** A gate left red *on purpose* is the
  same failure as a gate that can no longer detect anything: it prints the same reading whether the tree
  got better or worse, and every later track compares exit codes. So `cargo fmt --all -- --check` went to
  **0 hunks / 0 files, exit 0** through seven commits along crate boundaries — `63b6326` (the seven leaf
  crates), `a46f6f4` (router + websocket), `dbc3b78` (http), `aec827b` (middleware), `018b5a5` (`aether`),
  `3506038` (scheduler), `e7644b9` (the e2e test package). `git show --name-only` on each is the checkable
  claim: the files are exactly the ones the subject line names, so no commit is the unreviewable repo-wide
  sweep the standing rule forbids, and the workspace gates re-run on the finished tree are what say the
  last batch did not break what an earlier one depended on.
- **What the sweep actually cost is the part worth recording.** Reformatting is content-preserving and
  position-destroying: every line number in the docs that names current code moved. A drift detector
  (scratch, `C:/tmp/t73/drift.py`) aligned each changed `.rs` file between `7bb4356` (the last commit
  before the batches) and `HEAD` with `difflib` on stripped lines and replayed every `foo.rs:NNN[-MMM]`
  citation in `framework/docs` + the two READMEs through that map: **56 carried numbers on the pre-`fmt`
  docs, 55 after dedup** (`C:/tmp/t73/drift_pre.py` re-derives it from the docs at `7bb4356`; the same
  script re-run over the swept tree leaves 27 / 26). Triage split them by *kind of claim*, not by file: a
  coordinate presented as "where this lives now" is re-derived, and a coordinate inside a dated record —
  a track entry, a perf-baseline snapshot, a `git show 907dfcd^:` pin, or a row that already says its own
  coordinate is dead — is left holding the tree it measured, because rewriting one of those falsifies the
  record instead of the map. The rule is the durable part: a coordinate inside a record that already names
  its own tree is evidence about *that* tree, and no later reformatting is supposed to move it.
- **The 27 are a re-runnable reading, not a remainder.** Re-running the detector over the swept tree
  (`python C:/tmp/t73/drift.py framework/docs README.md framework/README.md`) counts **27** carried
  numbers and prints **26** lines: the extra is F7's `connection.rs:305–307`, cited twice on the same
  line (its classification column and its closing disclaimer), and the printer dedups by
  `(doc, line, token, start)`. By file: `ACTIX_FAIR_COMPARISON` 10, `ADAPTIVE_TREE_SCHEDULING_DESIGN` 3,
  `PRODUCTION_READINESS_REPORT` 4 (all four are the `git show 907dfcd^` pin of `tls_e2e.rs:452` or a row
  quoting a past gate reading of it), perf-baseline snapshots 9 (`BASELINE-2026-10-19` 7,
  `BASELINE-2026-10-20-SCHED-ROTATION` 1, `lock-split-2026-10-20/SUMMARY.md` 1). Each of the 26 sits in a
  row that either names the tree it measured or says outright that the coordinate is dead — the design
  doc is the clearest: "Serve path **as audited on the 2026-10-18 tree** … the four line coordinates no
  longer point at those constructs … Re-derive the current shape from `scheduling.rs` and the serve loop".
- **The map's own failure, measured.** The matrix cited `price_response_body` at `http2.rs:658` on the
  pre-`fmt` tree; the drift map proposes **664** for it and the function is at **662**. The sweep first
  took the map's number and had to be corrected against the file — the concrete shape of "the map is a
  pointer, not an authority": it can name a drifted number without naming the right one. The same `658`
  appears twice more in this register, in the two PROB-0022 entries, where it names the *deadline
  renewal*, not the pricing function; those are dated readings of the tree that measurement ran on and
  stay as they are, which is also why no line number of this file is quoted for them — a note that grows
  by sixty lines a day cannot carry a live coordinate.
- **What the sweep rewrote, counted by a rule instead of by hand.** `dc5203a` changes **41 attached
  `foo.rs:NNN[-MMM]` citations** (`ACTIX_FAIR_COMPARISON` 24, `ERROR_HANDLING_GUIDE_AR` 6,
  `FEATURE_COMPLETENESS_MATRIX` 4, `PRODUCTION_READINESS_REPORT` 4, `FULL_FRAMEWORK_DOCUMENTATION_AR` 2,
  `AETHER_CORE_CAPABILITIES` 1) and **21 more bare `` `:NNN` `` continuations** of a file already named on
  the line (17 in `ACTIX_FAIR_COMPARISON`, 2 in `FEATURE_COMPLETENESS_MATRIX`, 1 each in
  `ERROR_HANDLING_GUIDE_AR` and `PRODUCTION_READINESS_REPORT`) — **62 coordinate readings**, counted by
  regex over `git show -U0 dc5203a`, which is the checkable statement. Two honest notes on the arithmetic.
  The first draft of this record and the `docs:` commit message both said "46 (28/6/5/4/2/1)", a hand
  count no rule reproduces; it is superseded by the figures above, and neither the commit message nor this
  line is being rewritten to hide that. And the map flagged 55 carried numbers (deduped), of which this
  sweep resolved 29 (ACTIX 13, ERROR 5, PRR 4, MATRIX 4, FULL 2, CORE 1) and left 26 — every one of them
  in the dated records the rule protects (ACTIX 10, the design doc 3, PRR's four `tls_e2e.rs:452` pins,
  the three perf-baseline snapshots 9). That the sweep rewrote 41 attached readings to resolve 29 flags is
  the map's own admission: **12 coordinates it declared unmoved had in fact moved** — 11 of them in
  `ACTIX` (24 rewrites against 13 flags) and 1 in `ERROR_HANDLING_GUIDE_AR` — and one, F1's
  `tests/http_e2e.rs:1336`, was
  **already dead before the batches landed**: it names a `client.expect_closed()` line in a different test
  even at `7bb4356`, while the test itself was at 1412 there and 1527 here. The drift detector cannot see
  either class (it only reports numbers that moved); only re-reading the source did.
- **A citation checker is only as good as its escape hatch, and this one had two.** The first pass matched
  `path/file.rs:NNN`; a bare `` `:756` `` after a file named earlier in the same sentence is invisible to
  it, and that is exactly how F6's second wire test was cited — a second pass (`C:/tmp/t73/bare.py`) that
  resolves the bare form against the file already named on the line found it. The other hole is
  resolution by basename: `config.rs:207` in a row about *Actix's* `actix-http-3.19.0/src/config.rs`
  resolves into a same-named in-repo file and is then reported out-of-bounds on a file nobody meant. This
  repository has two `config.rs` candidates (`framework/aether/src/config.rs`, 191 lines, and
  `framework/scheduler/src/config.rs`, 127), and the resolver written twice in this session blamed a
  different one each time — so even the file a checker points at is not a stable reading. Re-measured with
  that walk over `framework/docs` + both READMEs + `.project-governance` (28 Markdown files): **474 attached
  citations, 18 resolving to no file in the repository** (Actix registry sources, `aether-probe`,
  `benchmarks/rust_aether`) — the 447 this record first wrote was the same census before these notes grew —
  **and 12 out-of-bounds: 8 the `config.rs:207` collision (four of them this slice's own notes quoting the
  example, so the census now counts its own report), 4 bare `scheduling.rs:NNN` citations in governance
  notes that resolve by basename into `framework/aether/src/scheduling.rs` (397 lines) when the writer
  meant the scheduler engine or the wire-test file.** Inside `framework/docs` alone the Markdown census is
  72 (73 strings — the extra lives in a saved `.rs` artifact), and that is the scope the 55 → 26 drift
  figures above are quoted against.
- **Item 16 of the matrix changed from a decision to a fact** ("Lints are gated as of 2026-10-26; format
  is not" → "Lints and format are both gated"), keeping its dated chain (574 / 66 → 567 → 558 → 570 / 65 →
  560 / 65 → 0) and adding the two things the green run does *not* buy: there is **no CI in this
  repository**, so the gate is a command and not an enforcement and the next slice can re-introduce drift
  silently, and the reading is `rustfmt 1.9.0-stable` with **no `rustfmt.toml`**, so every choice inside
  it is this toolchain's default rather than a project decision.
- **Gates, all re-executed on the final tree.** `cargo fmt --all -- --check` exit **0**, zero `Diff in`
  blocks. `cargo clippy --workspace --all-targets -- -D warnings` exit **0** with 0 `warning`/`error`
  lines. `cargo test --workspace` exit **0** at **513 passed / 0 failed / 1 ignored** across 50
  `test result:` lines. `cargo build --workspace --examples` exit 0. `validate` exit **1** with the same
  four standing lines (three `[RECYCLED ID]` + `[OPEN VS HISTORY] REQ-0001`) — unchanged by this slice and
  permanent by construction. Governance Python suite `Ran 42 tests` / **OK**, exit 0. **Nothing pushed.**
  The ledger line is now the only red gate, and the project is still not described as green while it reads
  red.
- **Track 73 is closed.** Next, without needing the owner: 74 (PROB-0016 extension pricing, PROB-0027 as
  a documented rule), 75 (PROB-0028's label), 79 (evidence durability — this slice's detectors are scratch
  and will be deleted; the durable output is the re-derived doc text, which is the shape 79 asks for).
  76, 77 and 78 stay blocked on the owner or on a machine that is not this one.

## Track 81 — a memory-release test sampled the harness's own scheduling (PROB-0033, 2026-10-27)

- **What broke, and where it was found.** Track 75 re-ran its ladder on the tree it had just committed
  and got `TEST=101`: `a_declared_extension_is_priced_on_the_http_2_path_and_released_with_the_stream`
  (`framework/tests/tests/http2_e2e.rs:1487` on `e81efed`; `:1502` after this track's edits) failed with
  `left: 32768, right: 0`, then passed 3/3 when
  run alone. Load-only, and production was never at fault.
- **The defect class, stated so it can be recognised again.** Both memory-release tests in that file
  took their baseline as a raw `registered_memory_bytes` read right after the client handshake, but the
  connection's own `CONN_BASE_MEMORY` (32 KiB, `framework/aether/src/scheduling.rs:61`) is admitted by
  the server's accept task (`framework/aether/src/app.rs:1658`), not by the handshake. The baseline is
  therefore a sample of when the *test* ran, not of what the *server* held — and the settle assertion
  built on it can only fail when the sample wins the race. A test whose expected value is read before
  the state it measures exists is not measuring the framework; it is measuring the scheduler.
- **How it was confirmed rather than argued.** A deliberately leaked handler guard
  (`std::mem::forget(g)` in `serve.rs`'s `dispatch`) makes both fixed tests fail at the settle
  assertion (65552 vs 32768; 540856 vs 524472). The new baseline is exactly the connection's charge and
  a leaked stream registration is still caught, so the fix did not make the assertion vacuous.
- **The rule the class deserves.** Where a test compares against "what the server held before", the
  baseline is either `0` (sampled before any connection can exist) or a named constant reached through
  a bounded poll — never a raw read of a value the server writes asynchronously. The same file already
  carried the correct pattern at `:888`; the sweep that closed this did not add a helper, it pointed
  the two stragglers at the existing one.
- **Residual, named not fixed.** `http2_e2e.rs:1531` sleeps a fixed `150 ms` "for the admitted stream
  to be holding its charges inside the handler". None of it was observed failing, so it was not
  rewritten on suspicion; a fixed pause is the same bet this track lost, and a bounded poll is the
  replacement once one is shown to flake. The file's other two `150 ms` occurrences are not this
  class and were checked: `:1251` asserts on elapsed time (the idle connection must not close before
  its keep-alive timeout) and `:841` sleeps inside a bounded loop that re-checks its condition.
- **Gates on the final tree.** Three consecutive `cargo test --workspace` runs at **527 / 0 / 1** over
  50 `test result:` lines; `cargo fmt --all -- --check` 0 bytes; `cargo clippy --workspace
  --all-targets -- -D warnings` exit 0 / 0 warnings; governance `Ran 42 tests` / **OK**; `validate`
  **exit 1** on the same four standing lines. **Nothing pushed.**

## Track 80 — every `path:line` in `docs/` checked against the code it names (PROB-0032, 2026-10-27)

- **The defect, stated as a measurement.** Track 75 re-derived seven citations because it had to edit
  them, and said so in this register: the rest of the class was left registered rather than swept.
  This track swept it, and the guess was low. At `e81efed` the human set carried **61** citations over
  **40** distinct spans; **36** coordinates, in **8 of the 14** documents, named a span that does not
  hold what the sentence claims (29 replaced in `b8d6ffa`, 7 more once every pin was read - below).
  Resolution and bounds were already clean — every file existed, every span was inside its file —
  which is precisely why the defect survived: an existence check passes on a coordinate that lands on
  unrelated code. `app.rs:1946-1950` was another branch of the same `conn_buf` loop; `response.rs:287`
  was a date write beside the header emitter.
- **The classes, because they recur.** `app.rs` moved by whole-region insertions (1769→1915,
  1857→2008, 1540→1680, 1631→1771, 239→250, 535→580, and four multi-line spans into
  2185-2189 / 2190 / 2224-2234 / 2102-2106, plus 2329→2499-2501); `response.rs` moved its writer out
  from under the prose (206-236→225-255, 227-235→246-255, 238→257, and `write_sanitized_field_value`
  287→310); `serve.rs` 423-428→487-495; `parser.rs` 262→338 (the incremental parser replaced the
  whole-request one); `worker.rs` 133→23; `trie.rs` 84→100 in two documents; `jwt.rs` 82→84 in two.
  A citation into a file that grows above the target is wrong the day it is written or silently right
  for a while, which is the same reason this project stopped trusting frozen literals.
- **The seven the pins found, after the sweep looked finished.** Reading each pin against its span
  rather than trusting the first sweep exposed three more defects: `docs/02:66` quoted a parse-loop
  excerpt at `app.rs:1624-1636` (the accept arm) and `app.rs:1936-1950` (the write deadline), two
  regions of the right function that the excerpt does not show - replaced by the composite the code
  actually has (`app.rs:1766-1776` head and call, `:2093` the buffer advance, `:2133`/`:2184` the two
  arms); `docs/09`'s `IntoResponseError` span started at `:99`, inside the preceding doc comment, true
  at `:110`; and `node.rs:41-43` stopped one line before `supports_method`'s closing brace, true at
  `42-44`, in three documents. Seven coordinates, and each was a span whose *ends* were wrong while
  its start or its content was close enough to pass an anchor-anywhere check.
- **The three that no scanner could see.** `docs/05:129` listed five refusal coordinates, but only
  `app.rs:1540` was backticked; `1986` / `2036` / `2090` / `2120` were plain digits. The sentence is
  now four backticked citations (`app.rs:1680`, `:2142`, `:2192`, `:2576`) and the count it states —
  five — was wrong too. A prose count of sites is a claim like any other, and this one had drifted in
  both directions: the list lost one site and gained four.
- **The gate is committed, not a script that ran once.** `.project-governance/tools/cite_check.py`
  reads both citation forms (`file.rs:NNN[-MMM]` and a bare `:NNN` inherited from the file named
  earlier on the same line), resolves an exact repo-relative path first and then a unique basename
  (ambiguity is a failure), and fails on nine conditions: a bare coordinate naming no file, no such
  file, an ambiguous basename, a span outside the file, a citation whose span has no pin, a pin whose
  shape disagrees with the span, a first line that moved or was replaced, a last line that moved or
  was replaced, and a pin no citation points at. **The pin is
  `(file, first line, last line) -> regex | (first regex, last regex)`**, 45 of them for 45 spans.
  Both ends are in the key because the first version pinned one anchor line, and a pin that proves
  only the first line is blind to an insertion *inside* the span: `docs/02`'s off-by-one survived a
  mutant precisely because a 3-line span tolerates a one-line shift. A multi-line pin therefore states
  what its first and last lines must say and the checker refuses a pin whose shape disagrees with the
  span, so no span can be half-checked - this repository's "re-derive both ends" rule, mechanised.
- **19 unittests, and the repository test refuses to pass vacuously.** Each refusal is pinned on a
  four-file tree, including the two-directional hints ("the pattern now matches only at
  `src/deep/app.rs:8`"; "now matches at ...:5, 10, and it is not unique"), a shifted-source case that
  keeps the span alive one line off, and an edit *inside* the span that moves its end while its start
  stays put. The test that runs against this repository asserts `>= 50` citations, zero findings, and
  that every pin is used, so deleting the corpus turns the gate red instead of green.
- **Falsified by four mutants on the real tree**, after the first version of the anchor rule survived
  one of them: a doc coordinate edited by one line → *not pinned*; a cited symbol renamed → *the span
  does not hold what the prose claims*; one blank line above a cited span → *the span's first line
  moved* (pinned at `:2499`, found at `:2500`); one blank line at the top of `app.rs` → **25
  findings**, every citation below it, no survivors. The both-ends redesign was then falsified in its
  own direction on a fixture: a line inserted inside `fn third` leaves the first line matching and
  moves the closing brace, and only the last-line pin reports it. The second and third mutant runs are
  also what found two tool defects: `@dataclass` cannot be loaded through
  `importlib.util.spec_from_file_location` without a `sys.modules` entry (the records are
  `NamedTuple`), and the finding text used `→`, which crashed on a cp1252 stdout — a finding that
  cannot be printed is a crash where a report was owed.
- **The notes corpus was measured and left un-gated, with the residue named.** PROB-0032 also ordered
  `.project-governance/notes/`. It holds **268** citations, **32** of which do not resolve to a unique
  in-tree file — external crates (`actix-http-3.19.0/src/config.rs:207`), other projects
  (`rust_aether/src/main.rs:71`), short forms whose file is the sentence (`scheduling.rs:489`). A gate
  there would need a hand-authored exclusion list larger than the finding set, so only the seven
  coordinates the problem named were re-derived by hand: `app.rs:190`→`:201` (with
  `canonical_reason` and `Response::json_status` read to confirm neither clamps — the finding stands),
  `serve.rs:481`→`:508`, `response.rs:357`→`:383`, its assertion `:829`→`:877`,
  `status_forbids_body` `312-314`→`338-339`, and the `Payload Too Large` sites, four in the register
  and **three** in code today (`app.rs:2187`, `:2573`, `http2.rs:372`). The same block's claim that
  no doc asserts the phrase was false — `docs/00:204` says it in both languages — corrected in place
  with a dated marker rather than deleted. What stayed unverified there is said so, not implied.
- **Ledger.** `done PROB-0032`: problems 6 → 5 (0004, 0005, 0009, 0011, 0012 remain), and the ID
  stays in the allocation history the recycling ratchet reads.
- **Gates on the final tree.** `cargo test --workspace` **527 / 0 / 1** over 50 `test result:` lines
  (no markdown in this tree is a build input and the Python files are not built, so the count could
  not move); `cargo fmt --all -- --check` 0 bytes; `cargo clippy --workspace --all-targets
  -- -D warnings` exit 0 / 0 warnings; governance `Ran 61 tests` / **OK** (42 before this track, 19
  new); `cite_check.py` **66 citations / 45 spans / 0 findings**, exit 0; `validate` **exit 1** on the
  same four standing lines. **Nothing pushed.**

## Track 79 — the numbers now point at something a reader can open (2026-10-27)

Roadmap §8's rule, executed: every probe a claim rests on becomes a committed artifact, and where a
probe cannot be committed the document carries the reading, the exact command and the date.

- **The measurement.** Every machine-absolute path in the committed tree, by owner: the two
  user-facing globs 5 mentions in 2 files; the four governance notes 118 mentions / 71 distinct paths
  (each line states its reading, which is why they survive); `AGENTS.md` 1 (the `C:/tmp` isolation
  rule, outside by design); `framework/tests/tests/scheduling.rs` 1. Every failure was a
  `PRODUCTION_READINESS_REPORT.md` scratch pointer; the four `file:///c/Users/...` links resolve
  inside the repository and stay.
- **A committed archive that was only half indexed.** `perf-baseline/lock-split-2026-10-20/SUMMARY.md`
  names six raw logs and four of them existed only in the owner's scratch directory — including
  `run4_pacer_control.txt`, the record of the withdrawn first paced run. `cmp` proved the tree's
  `run5`/`run6` are byte-identical to the copies beside them, so the four missing logs were copied in:
  the directory held 12 entries while the baseline said 11, and with the four it holds 16, so
  `SUMMARY.md`'s index is whole and the external path is gone from
  `BASELINE-2026-10-20-SCHED-ROTATION.md` along with its count, corrected to 16.
- **The report states method, not location.** `:431`/`:441` now give `cargo test --workspace` and the
  date instead of scratch logs; `:449` says the log is not preserved and what the number stands on;
  `:765-767` uses portable notation for the temp directory; `:781` says `aether-probe` is a sibling
  directory. A reader without this machine can repeat the first two and cannot be misled by the rest.
- **The gate is committed.** `.project-governance/tools/evidence_check.py` enforces one rule — a
  machine-absolute path in a user-facing document must resolve inside the repository — over
  `docs/**/*.md` and `framework/docs/**/*.md`. Portable notation (`~`, `$TMPDIR`, `%TEMP%`) is exempt
  because it resolves per reader. It fails closed: an external path that is legitimate is a finding
  whose fix is portable notation or a reasoned exemption, because a gate that lists only today's
  defects is green for tomorrow's.
- **16 unittests, and three false-positive shapes fixed rather than excused.** `http://` read as drive
  `p:` (the letter must now stand alone, and a slash may not follow the separator); a prose `C:` with
  no separator; and `file:///etc/x` read as the relative path `etc/x`, which excused every
  POSIX-spelled link as inside the repository. The repository test asserts zero findings with floors
  of 30 documents and 1 token, because an emptied corpus and a clean one print the same line. Two of
  these tests were strengthened after review: the POSIX one now asserts the parsed path rather than a
  finding that echoed the token, and a spellings test compares the folded text (`as_posix()`), because
  Windows path equality is case-insensitive and let a lower-case drive through. Both mutants are
  caught — 2 failures under the old `as_path`, 7 fixture tests under the old regex.
- **Falsified on the tree it was written for.** The gate run against `3c838e3`'s
  `PRODUCTION_READINESS_REPORT.md` returns 6 findings at `:431`, `:441`, `:449`, `:766` (twice) and
  `:781` — every scratch pointer, none missed.
- **Two outside the globs, found by hand.** `scheduling.rs`'s doc comment cited
  `C:/tmp/t72/probe_b.log`; the path is gone and the reading stays (`active=1 registered=33603584`,
  six seconds into a 300 ms deadline). `code-hashes-2026-10-19.txt` listed `response.rs` twice with
  two different hashes and one absolute spelling; three of the six hashes reproduce from `1116e1a`,
  the first commit (`git show 1116e1a:framework/<path> | sha256sum`), while `app.rs` and both
  response.rs lines match no committed blob at either layout, so those were taken on a working tree
  that changed before that commit. The file gained a dated header that says which pair disagrees and
  that `sha256sum -c` reports six FAILED by design.
- **An independent review refuted two numbers in the first draft of this section.** Read-only, one
  verdict (SHOULD-FIX), per-point evidence. It confirmed the gate's rule on temp roots outside the
  repository and replayed the pre-track report (6 findings at the six sites), then refuted (a) the
  claim that none of the six hashes matches a committed blob — my scan iterated the table's short
  paths instead of the full ones, so every lookup missed, and a second scan tested only CRLF variants;
  three hashes do reproduce from `1116e1a` — and (b) "11 -> 16 files", since `git ls-tree 3c838e3`
  shows 12. It also showed the POSIX test passed under the old `as_path` because the finding echoes
  the token the assertion searched for. All three are corrected here and in the follow-up commit; the
  reviewer's unverified list is kept as unverified: that the suite log came from the exact committed
  tree, that the four logs byte-match the scratch originals (those copies are gone), and fmt/clippy,
  which it did not re-run.
- **Linux typing, the cheap half of PROB-0012.** The target installs; `cargo check --workspace
  --target x86_64-unknown-linux-gnu` fails only in `ring v0.17.14`'s build script
  (`ToolNotFound: x86_64-linux-gnu-gcc`), and there is no gcc/clang/zig and no WSL on this box. Two
  readings obtained anyway: `-p aether-http -p aether-router -p aether-core` exit 0 for the Linux
  target, and the `#[cfg(unix)]` `signal::ctrl_c` shutdown block compiles as a tokio fragment crate.
  A shadowing suspicion about `use tokio::signal::unix::signal` was refuted by compiling and was not
  reported as a defect.
- **Left open, named.** The notes are not gated (118 mentions, each with its reading); a `.rs` doc
  comment and a `.txt` transcript are outside the globs; a POSIX absolute path without a `file://`
  prefix is not matched because a leading `/` in these documents is nearly always a route. PROB-0012
  still needs a Linux host or a cross C toolchain for the whole workspace.
- **Gates.** `cargo test --workspace` **527 / 0 / 1** over 50 `test result:` lines; fmt 0 bytes;
  clippy exit 0 / 0 warnings; governance **77 tests** (61 + 16) exit 0; `cite_check.py` 66 / 45 / 0;
  `evidence_check.py` 33 documents / 0 findings; `validate` exit 1 on the same four standing lines.
  **Nothing pushed.**

## Track 82 — one command runs every gate, and the red ledger line is pinned, not hidden (2026-10-27)

Governance hygiene first, in four commits, then the gate itself. The map was regenerated by its own
tool (`271a4ad`, 118 → 122 files, the four additions and no removals, and the header now states what
it counts); PROB-0009 was retitled to the residual it still tracks rather than the contract it was
named after (`3a7b0ec`, status and history unchanged); `TRACK_INDEX.md` maps every track to the file
and section that records it (`22312c9`), with the log getting a seven-line pointer at its end instead
of a top index, because a top index would shift every line number the register cites.

`gates.sh` (`17f1e80`, repo root, executable) runs format, lints, suite, governance tests,
`cite_check.py`, `evidence_check.py` and `validate`, prints one PASS/FAIL line each, logs to
`target/gates/*.log`, and exits non-zero if any line failed. The hard part is validate: it exits 1 by
design on four findings that are permanent history (PROB-0007 ×2, PROB-0017 ×2, REQ-0001 ×3, and
REQ-0001 open under the reused ID). The script pins exactly those four prefixes, requires each once
and no other finding line, and then calls the gate passed. A fifth finding fails the run; a missing
one fails it too, because a permanent-history line can only disappear through a ledger rewrite. That
pin is the same standing set `test_recycling_ratchet.py` guards, so the test and the script cannot
drift apart silently.

Sensitivity was proved in both directions against a stubbed copy of the real validate block, so the
comparison under test is the shipped one, not a paraphrase: the baseline fixture printed PASS and
exited 0; the same fixture plus a synthetic fifth finding printed `FAIL … 1 unexpected finding(s),
pinned set intact` and exited 1; the fixture minus one pinned line printed `FAIL … pinned set
changed` and exited 1. The first live run also failed for real: fmt, clippy and tests reported
`could not find Cargo.toml`, because they ran from the repository root while the workspace lives in
`framework/`; the script now runs them there, and the failure path was observed rather than assumed.

- **Readings on the committing tree:** `bash gates.sh` exit 0 — fmt PASS, clippy PASS, tests PASS
  (527 / 0 / 1 over 50 `test result:` lines), govtests PASS (77), cite 66 / 45 / 0, evidence 33 / 0,
  validate PASS with the four findings printed. `target/` at the root is now ignored for the logs.
  **Nothing pushed.**

## Track 83 — the redacted 5xx detail reaches a reporter under the response's own request id (2026-10-27)

PROB-0009's last residual: a 5xx body redacted an internal message and the message then went
nowhere, because `into_response` ran with no request in scope. Closed by an opt-in diagnostics
channel, not by a wire change and not by a log the framework writes on its own.

- **The opt-in.** `IntoResponseError` gains `redacted_detail() -> Option<String>`, default `None`;
  `AetherError` returns its message for 5xx only. The blanket asks for it only when the status is
  5xx **and** `error_detail()` returned `None`, so a 4xx (detail already in the body) and a 5xx
  that publishes its detail are never reported twice. A user type that redacts stays silent until
  it implements the method — the framework does not decide to log a user's internal text.
- **The hook.** `aether/src/diagnostics.rs` holds `ErrorReport<'a> { request_id, status, code,
  detail }` and `set_error_reporter`, which stores one `Fn(&ErrorReport<'_>) + Send + Sync` in a
  `OnceLock` and returns `Err(ReporterAlreadySet)` on the second install. No reporter installed is
  the old drop. The doctest in the module is the sixth doc test.
- **The id half costs nothing on the success path.** `next_request_id()` returns
  `(u64, String)`; the serial rides in `RequestParts` and `RequestContext`, and `serve::dispatch`
  scopes it with `within_request` **inside** the spawned task, because a task-local does not cross
  `tokio::spawn`. The `req_{serial:012x}` string is formatted only on the failure path and equals
  the response's `X-Request-ID` (HTTP/1 and HTTP/2 both feed the same counter).
- **Pins.** `framework/tests/tests/error_reporting.rs` (new target, one test) drives four handlers
  over a live socket: `AetherError::Io` is reported with the client's id and absent from the body;
  a 4xx is not reported; an opt-in user type is reported with its own id; a 5xx that publishes its
  detail is not reported. Sensitivity measured in both directions: removing the report call fails
  the test, and reporting `serial + 1` fails the id-equality assertion; restored, green.
- **Docs.** docs/09 (trait span 110-148 and the reporter API), `ERROR_HANDLING_GUIDE_AR.md` (E5002
  row, three optional methods, the reporter example), the matrix (item 1 closed; counts), the PRR
  (item 5 closed and a twelfth reading instead of a rewrite), `BASELINE_MEASUREMENTS.md` (counts and
  ladder), `FULL_FRAMEWORK_DOCUMENTATION_AR.md` (blanket span 205-238), both READMEs (counts).
- **What the docs sweep exposed.** The code insertion moved every `docs/*.md` coordinate below it;
  30 citations over 22 pins were remapped mechanically from a HEAD-to-tree line map, and
  `cite_check.py` is the record. Removing the pins one at a time also exposed a generic pin the tool
  was designed to fear: two identical `let _ = conn_buf.split_to(consumed);` lines exist, and the
  pin without indentation silently followed the nested one into the main loop's slot. It now pins
  the 32-space form, so a shift cannot swap them again. Measured at the code-only commit
  (`a0c1b94`): 31 findings, exit 1 — the documents there name the pre-change lines by construction.
- **Readings on the committing tree:** `bash gates.sh` exit 0 — fmt PASS, clippy PASS, tests PASS
  (529 / 0 / 1 over 51 `test result:` lines; 305 unit + 218 integration + 6 doc), govtests PASS
  (77), cite 66 / 45 / 0, evidence 33 / 0, validate PASS with the same four pinned findings.
  PROB-0009 removed with `done` (`event_log.jsonl`, 2026-10-27T13:52:46Z); the four remaining open
  items are PROB-0004, PROB-0005, PROB-0011 and PROB-0012. **Nothing pushed.**

## Track 84 — the receive deadline is its own budget, not the handler's (2026-10-27)

The matrix's item 2: one `request_timeout` served three roles — the HTTP/1 total receive deadline
(`app.rs`, the 408 arm), the HTTP/2 body deadline (`http2.rs`), and the handler execution timeout
(`serve.rs:dispatch`). A legitimately slow upload therefore shared its budget with handler
execution, and the only way to admit it was to grant every handler the same extension.

- **The split.** `ServerLimits` gains `receive_timeout` (default 30 s, the previous shared value),
  and `AetherConfig` gains `max_receive_time` / `AETHER_MAX_RECEIVE_TIME_SECS` (default 30 s;
  `from_env` refuses `0`, like the other two impossible-zero bounds). `limits_for` maps it, the
  HTTP/1 408 check and the HTTP/2 body deadline read it, and `serve.rs:dispatch` keeps
  `request_timeout`. The handler clock starts only after the request has arrived, so the two
  numbers cannot leak into each other by construction.
- **Pins, both directions.** `slow_trickle_receives_408_within_the_total_receive_deadline`
  (`http_e2e`) sets only `max_receive_time` and leaves the handler budget at its 30 s default, so
  a trickle is refused by the receive number alone. `a_slow_upload_is_not_bounded_by_the_handler_timeout`
  sets a 100 ms handler timeout and a 5 s receive budget, then trickles a 4-byte body over ~0.6 s
  and requires 200 `got:4`; the HTTP/2 stalled-body test now configures `max_receive_time`. The
  derivation unit test asserts both fields land where they belong. Sensitivity measured by
  restoring the coupling in the 408 check: both wire tests fail (the trickle panics on its 3 s
  poll deadline, the upload gets cut at 100 ms), and restored, green.
- **Docs.** docs/07 (struct + a paragraph that names the split and both tests), docs/09 (both
  structs), docs/08 and `FULL_FRAMEWORK_DOCUMENTATION_AR.md` samples, both READMEs (env table; the
  receive-deadline gap bullet is now the resolved statement), the matrix (slow-client row, HTTP/2
  row, item 2 closed, counts), the PRR (§4.2 marked superseded, the trickle-probe row annotated,
  a thirteenth reading). The insert moved every `docs/*.md` app.rs coordinate below it by one:
  four abbreviated `:N` citations and 18 pins were re-derived, and `cite_check.py` now reports
  66 citations / 45 spans / 0 findings. The sweep also caught a link that was already dead at
  HEAD — the PRR pointed the wire-budget check at `app.rs:2247`, which is a bare `return;`; it now
  points at 2285, where the check is.
- **Readings on the committing tree:** `bash gates.sh` exit 0 — fmt PASS, clippy PASS (one initial
  `byte_char_slices` finding in the new test, fixed rather than allowed), tests PASS (530 / 0 / 1
  over 51 `test result:` lines; 305 unit + 219 integration + 6 doc), govtests PASS (77), cite
  66 / 45 / 0, evidence 33 / 0, validate PASS with the same four pinned findings. **Nothing pushed.**

## Track 85 — a handler stops when its client is gone (2026-10-27)

The matrix's remaining-work item 3: "A handler is not cancelled when its client goes away."
`dispatch` parked the connection task inside `timeout(request_timeout, join_handle)`, so the only
place a departed peer surfaced was the response write — after the handler had already spent its
whole budget producing bytes for a socket nobody was reading.

- **The watch arm.** `serve.rs` gains `PeerWatch` (one `poll_gone`), `ServeOutcome::ClientGone`,
  and a `biased` `select!` in `dispatch` that races the watch against the handler deadline. When
  the watch fires, the handler task is aborted through the same `abort_handle` the timeout uses,
  `dispatch` returns `None` (turned into `ClientGone` at the two call sites), and both frontends
  close without writing a byte. Panic and 503 admission paths are unchanged.
- **HTTP/1: read through the transport, and only an error is departure.** `ReadAheadWatch` polls
  `transport::Stream` into a 1024-byte scratch and appends whatever arrives to `conn_buf`. Peeking
  the socket cannot answer for TLS — an abrupt end surfaces inside rustls as EOF where a record is
  expected, and `close_notify` is an encrypted record — so the read goes through the transport.
  Only `Poll::Ready(Err(_))` is `Ready(())`: an RST on plaintext, or rustls finding the stream
  ended without `close_notify`. `Ok(0)` is **not** departure: a client may shut its write half and
  still wait for the answer (RFC 8446's relaxed `close_notify`), and the repo's own
  `a_chunked_upload_crosses_the_connection_budget_on_its_wire_bytes` is that contract. EOF settles
  the watch to `Pending`, keeping any bytes it carried for the next parse, and a clean close is
  still detected at write time as before. `app.rs` arms the watch only when no request bytes
  already follow the parsed head (`ReadAheadWatch::disarmed()` otherwise), so a pipelined request
  is never mistaken for a departure.
- **HTTP/2: `RST_STREAM` is the departure.** `ResetWatch` wraps `SendResponse::poll_reset`, which
  is valid exactly in the window a handler runs (before the head is sent). A reset, or the error
  h2 reports for every stream once the connection dies, aborts the handler while other streams
  keep being served.
- **Rejected on evidence, not preference.** Aborting the connection's stream tasks is not
  cancellation: `JoinSet::shutdown()` aborts the *stream* task, while the handler lives in an
  inner `tokio::spawn` whose `JoinHandle` simply detaches on drop — measured by removing the
  per-stream watch and re-running the dropped-connection test, which stayed green only because
  h2 fails every stream when the connection ends and `ResetWatch` then fires. A test-earned RST
  was also needed on the plaintext side: `set_linger(0)` is deprecated (`SO_LINGER` blocks the
  thread) and fails clippy `-D warnings`, so the test pipelines `/unread` before `/slow` and
  closes with unread response data, which the kernel turns into an RST portably.
- **Pins.** `client_disconnect_cancels_the_running_handler` (rewritten from
  `client_disconnect_does_not_cancel_running_handler`), `a_pipelined_request_does_not_look_like_a_disconnect`
  and `a_client_that_stays_connected_is_not_cancelled` (`handler_exec_semantics`);
  `a_stream_reset_cancels_the_running_handler_and_keeps_the_session` and
  `a_dropped_h2_connection_cancels_its_running_handlers` (`http2_e2e`);
  `a_tls_client_that_leaves_cancels_the_running_handler` and
  `a_tls_client_that_half_closes_still_gets_its_response` (`tls_e2e`). Six new tests; the
  rewritten test replaces its opposite.
- **Sensitivity, measured in both directions.** Mutation A — make `ReadAheadWatch::poll_gone`
  never resolve — fails both HTTP/1 cancellation tests (plaintext and TLS). Mutation B — treat
  EOF as `Ready(())` — fails the TLS half-close test and the pre-existing chunked
  half-close upload. Mutation D — make `ResetWatch` never resolve — fails both HTTP/2 tests.
  Every mutation was reverted and the tree re-verified green.
- **Docs.** The matrix (the client-disconnect row now `VERIFIED_IMPLEMENTED` with the half-close
  contract named, counts, item 3 closed), the PRR (§9 item 2 closed, a fourteenth reading, per-file
  bullets for `http_e2e`/`tls_e2e`/`http2_e2e`), `BASELINE_MEASUREMENTS.md` (counts), both READMEs
  (counts).
- **Readings on the committing tree:** `bash gates.sh` exit 0 — fmt PASS, clippy PASS, tests PASS
  (536 / 0 / 1 over 51 `test result:` lines; 305 unit + 225 integration + 6 doc), govtests PASS
  (77), cite 66 / 45 / 0, evidence 33 / 0, validate PASS with the same four pinned findings.
  **Nothing pushed.**

## Track 86 — a server that is going away says so (2026-10-27)

The matrix's remaining-work item 9 and its "Graceful close on server shutdown" row: upgraded
WebSocket sessions were aborted when `serve_with_shutdown` began, so a peer read a bare TCP FIN
and could not tell a restart from a crash. RFC 6455 §7.4.1.1 exists for this case: status `1001`
"going away". Compression and subprotocol negotiation stay open (compression is owner-deferred) —
as of this track; the negotiation half was closed by track 95 (2026-10-29, below).

- **The transport's owner writes the frame.** The connection task cannot: once the handler holds
  the `WebSocket`, the task owns only a `JoinHandle`, and writing through it would mean sharing the
  socket. So the socket holds a clone of the same `watch` the serve loop watches.
  `WebSocket::from_parts_with_shutdown(io, initial, limits, shutdown)` stores it (a plain
  `from_parts` stores `None`, so direct and unit use is unchanged), and `recv` races the peer read
  against it in a `biased` `select!` — signal first, so a socket that never blocks on a read cannot
  starve it. On the signal the session ends through a private `going_away`: one `close(1001,
  "server shutting down")` frame, then `io.shutdown()`, then the same `Ok(None)` a closing
  handshake returns — which is what makes the handler's own loop return instead of being killed
  mid-await. The transport shutdown is not decoration: an RST earned by closing a socket with
  unread bytes in its receive buffer discards the just-written frame, which is the same reason
  every HTTP error path in this server shuts the transport down before returning.
- **`watch` bookkeeping, both halves.** A receiver cloned after the flag flipped inherits the
  "seen" version, so `changed()` alone would wait for a transition that already happened — exactly
  the shutdown that races an upgrade. `shutdown_requested` checks `borrow_and_update()` first and
  only then awaits `changed()`. A dropped sender resolves too: no one can ask again, so parking the
  session forever would be a leak, not an error.
- **Shutdown stays a request, not a wait.** `dispatch`'s shutdown arm waits
  `WS_SHUTDOWN_GRACE` (1 s, `app.rs`) for the session to return on its own terms, then aborts — so
  a handler that never comes back to `recv` cannot pin the drain, which is exactly the bound that
  existed before the frame did. A session that returns counts as a completion (`conn_guard.complete()`),
  mirroring the HTTP request that finishes during a drain; only the abort leaves the guard to record
  a cancellation, as before. The grace is deliberately not `write_timeout`: a peer that cannot take
  a close frame is gone, and shutdown should not spend a whole write budget learning that.
- **Rejected alternatives.** (a) Writing the frame from the connection task — it no longer owns
  the transport, and a shared writer would need a lock on every frame write to buy one shutdown
  frame. (b) Keeping the abort and documenting it — that is what the matrix recorded, and it costs
  the peer the only signal RFC 6455 provides for this event. (c) Waiting for the handler
  without a bound — one handler that never returns to `recv` would hold the whole shutdown, the
  property track 84's bounded drain exists to prevent. (d) Reusing `write_timeout` (10 s) as the
  grace — three orders of magnitude too long for a shutdown and unrelated to what it measures.
- **Pins.** Unit (`websocket/src/connection.rs`):
  `a_server_shutdown_closes_a_waiting_session_with_going_away` (asserts the `0x8` frame, code
  `1001`, the reason bytes, and that the peer's next read returns 0 — bounded, so a missing write-half
  shutdown fails instead of hanging), `a_shutdown_that_started_before_the_upgrade_still_closes_the_session`
  (`watch::channel(true)`, 1 s outer bound) and `a_session_closed_by_shutdown_refuses_further_sends`
  (`WsError::Closed`). Wire (`http_e2e`):
  `shutdown_sends_a_going_away_close_frame_and_ends_the_session` (rewritten from
  `shutdown_aborts_a_live_websocket_session_without_a_close_frame`, which pinned the opposite) and
  `a_session_that_never_returns_to_recv_is_still_aborted_at_shutdown` (a handler that sleeps 30 s;
  `server.stop()` returns in under 5 s, the handler never finishes, and the peer reads EOF or an
  error — never bytes).
- **Sensitivity, measured in both directions.** Mutation 1 — build the socket with `from_parts`,
  giving it no signal — fails the wire test (`connection closed before the frame was complete`).
  Mutation 3 — `shutdown_requested` awaits `changed()` without the value pre-check — fails
  `a_shutdown_that_started_before_the_upgrade_still_closes_the_session` with `Elapsed(())`.
  Mutation 4 — drop `io.shutdown()` after the close frame — fails
  `a_server_shutdown_closes_a_waiting_session_with_going_away`. Mutation 2 — abort immediately
  instead of waiting out the grace — **did not fail** the wire test: the session won the race and
  wrote the frame before the abort landed (`1 passed`). Recorded as not decisive rather than
  quietly dropped: the wire test pins the delivered frame, the grace is what makes delivery
  deterministic, and the second wire test is what pins the bound the grace enforces. (Mutation 4
  only became decisive after the unit test's tail read grew a 1 s `tokio::time::timeout`; without
  it the mutated build hung rather than failed.) Every mutation was reverted and the tree
  re-verified green.
- **Coordinates.** The two `app.rs` insertions (an 11-line const block at the top, six lines at the
  socket construction plus thirteen in the shutdown arm) moved 22 pinned spans, so the six
  `docs/` files that cite `app.rs` were re-derived (+11 above the const, +12 after the construction,
  +25 after the dispatch arm) and `cite_check.py`'s pin table moved with them. 28 inline citations
  were rewritten; the checker reads 66 citations / 45 spans / 0 findings again.
- **Docs.** The matrix (row 381 `VERIFIED_IMPLEMENTED` with the mechanism and its five pins, row 334
  and the e2e description, the driver-test count 16 → 19, item 9, counts), the PRR (§4.6 bullet, a
  fifteenth reading, known-gap 6 closed, remaining-work 7), `BASELINE_MEASUREMENTS.md` (counts),
  both READMEs (counts and the "sessions are aborted" sentence, which §Status of
  `framework/README.md` stated as current). The matrix's own *status count* table did not
  reproduce: it said 98 / 11 / 20 / 3 / 2 = 134, and the same literal count over the commit that
  first wrote it (`aca3df7`) gives 97 / 11 / 18 / 3 / 2 = 131 — it was wrong when it was written.
  The table now reads 99 / 10 / 17 / 3 / 2 = 131 (this track's flip is the only change on top of
  the corrected base) and the definition is stated beside it. A stale `tls_e2e` count (9 for 11)
  in row 316 was corrected in the same pass.
- **Readings on the committing tree:** `bash gates.sh` exit 0 — fmt PASS, clippy PASS, tests PASS
  (540 / 0 / 1 over 51 `test result:` lines; 308 unit + 226 integration + 6 doc), govtests PASS
  (77), cite 66 / 45 / 0, evidence 33 / 0, validate PASS with the same four pinned findings.
  **Two ladders of the day failed**, both on the `scheduling` binary and both reverted by an
  immediate re-run. The first: `stress_120_concurrent_clients_over_scheduled_server`, 9 of 120
  clients answered `503` instead of `200`. The second: that test *and*
  `idle_sockets_are_charged_what_they_hold_not_what_they_could_reach`, each hitting its 30 s bound
  (`Elapsed(())`), with the binary taking **48.65 s** where the same binary takes **2.04–2.27 s**
  alone and on both green ladders. That ratio is the evidence: the tests did not slow down by a
  margin, the host did. The whole suite re-ran green (540 / 0 / 1, exit 0) immediately after the
  second failure, before any edit, and the third ladder is green end-to-end. Recorded as
  host-contention sensitivity of the `scheduling` binary, not repaired inside this track: nothing on
  its path changed, and both failing tests assume a handler slot and the queue keep their shape
  while eighteen four-worker runtimes share eight cores — a premise a workspace-wide run with an
  ambient load on the box can starve. Making the bounds looser would hide the next real regression,
  so the follow-up is a serial guard for that binary, not a bigger timeout — delivered 2026-10-29
  by track 93. **Nothing pushed.**

## Track 87 — the error contract is delivered, and the documents that teach it said otherwise (2026-10-27)

`PRODUCTION_READINESS_REPORT.md` §9 item 1 still opened with "Typed error mapping (`IntoResponse
for AetherError`, 500 for internal errors)" as remaining work, and concern-register row **E9** ended
with "typed error mapping remains open". Both were false on 2026-10-24: D9 shipped the contract as
the local `IntoResponseError` trait, with a blanket `impl<T, E> IntoResponse for Result<T, E>` and
two wire tests. A reader following either sentence would rebuild something that exists.

- **What exists, measured on this tree.** The trait `pub trait IntoResponseError: Send` at
  `framework/aether/src/app.rs:121-159` (`error_status` required; `error_code`, `error_detail`,
  `redacted_detail` defaulted); the two message-carrying impls at `:161-179` (`&'static str` and
  `String`, both 400); `AetherError`'s impl at `:181-214` (404/405/413/431/504, `Io` and
  `DependencyCycleDetected` 500, the four 400 variants); the blanket response at `:216-249`, whose
  body takes its `code` from the status class and redacts a 5xx message. Pinned by
  `a_handler_error_answers_400_and_says_so_in_the_body` (`http_e2e.rs:3012`) and
  `typed_handler_errors_answer_their_own_class` (`:3048`).
- **Why it is not `IntoResponse for AetherError`.** Re-bounding the blanket to `E: IntoResponse`
  was measured on 2026-10-24 to answer an `Err` with **200**, because the same blanket serves
  `&'static str`, whose `IntoResponse` is a successful plain-text response. The local trait keeps
  the 200 door shut and still lets the framework implement it for `aether_core`'s `AetherError`
  under the orphan rule. The report now says that, so the item cannot be re-opened as written.
- **Both claims corrected where they stood.** §9 item 1 is struck through with the delivery, the
  shape and the pins, following the convention item 2 already used; E9 now reads "all three
  delivered" and names the date each one landed. §12's sentence about "the coherence constraint
  that decides the framework's version of typed error mapping" was already right and is unchanged.
- **The sweep the correction forced.** Checking the error contract against the source exposed 15
  coordinates in the documents that teach it, all now re-derived from the code:
  `ERROR_HANDLING_GUIDE_AR.md` 8 (six `app.rs` spans — the trait, the impl block, the
  `AetherError` status match, the two `IntoResponse` impls — and the two `http_e2e` test lines),
  `FULL_FRAMEWORK_DOCUMENTATION_AR.md` 2, the matrix's two error rows 4 (`:121-159`, `:161-179`,
  `:181-214`, `:233-238`), and the readiness report's wire-budget link 1 (`:2285` → `:2330`).
  Three of the fifteen were already dead before track 86: the guide's test citations pointed at a
  hint list and at the WebSocket shutdown doc comment rather than at the two tests (which sat at
  `:2936`/`:2972` on the pre-track-86 tree and at `:3012`/`:3048` now), and the report's link
  sat 20 lines above the budget check it quotes. Two matrix spans (`:140-158`, `:160-186`) named
  the wrong construct even after the +11 shift — one started inside the trait, the other stopped
  inside the `String` impl while the sentence was about `AetherError` — which is the same
  "endpoints are part of the citation" defect track 80 measured, one document further out.
- **What was deliberately not re-derived.** `WORKING_CONTEXT.md` is frozen history by the track
  index's own rule; `ACTIX_FAIR_COMPARISON.md` (27 citations) says its coordinates were "re-read on
  the track-74 tree"; `ADAPTIVE_TREE_SCHEDULING_DESIGN.md` (3) opens by saying its four coordinates
  are the 2026-10-18 problem statement and "no longer point at those constructs"; the register's and
  roadmap's own track sections are dated records. Rewriting those would trade a dated reading for a
  false claim of currency.
- **The coverage boundary, measured rather than assumed.** `cite_check.py` reads `docs/*.md` at the
  repository root — 10 of its 14 files carry citations, 66 citations over 45 pinned spans.
  `framework/docs/` holds 15 files, 8 of which carry 67 citations; two of those eight (ACTIX 27,
  ADAPTIVE 3) declare themselves snapshots, the readiness report's 16 are mixed, and the remaining
  21 in five documents teach the current tree without a pin behind them. That is how this track's
  fifteen dead coordinates survived a gate that was green throughout track 86.
- **Follow-up registered, not done here.** Extend the checker's glob to the teaching subset —
  `ERROR_HANDLING_GUIDE_AR.md` (8), `FULL_FRAMEWORK_DOCUMENTATION_AR.md` (3),
  `FEATURE_COMPLETENESS_MATRIX.md` (5), `AETHER_CORE_CAPABILITIES.md` (4), `GETTING_STARTED_AR.md`
  (1) — and give a document a way to declare itself a snapshot instead of pinning history. That is
  a pin-table and tooling change of its own; doing it inside a docs correction would have hidden a
  large mechanical diff behind a small one.
- **Readings on the committing tree:** `bash gates.sh` exit 0 — fmt PASS, clippy PASS, tests PASS
  (540 / 0 / 1 over 51 `test result:` lines; unchanged because no code or test moved), govtests PASS
  (77), cite 66 / 45 / 0, evidence 33 / 0, validate PASS with the same four pinned findings.
  **Nothing pushed.**

## Track 88 — the open-problem inventory, and the first batch of claims it falsified (2026-10-27)

Owner request: «اتخذ قررار وصلح كل مشاكل اعمل خطة طويلة». The plan is roadmap §19 — every open
item classified as a defect, a parity gap, an owner decision, an environment gap or a non-goal,
with a track number and a first step for each. This track executes the plan's first row
(D-01…D-06, D-14…D-17): the defects that are claims rather than code.

Found and corrected:

- **The matrix said `AetherApp` has no way to enable access logging.** `enable_request_logging(show_timing)`
  exists and the response post-process reads `logger.enabled`. The *Access logging* and *Request
  logging* rows said otherwise; both now read `VERIFIED_IMPLEMENTED` with the method named and the
  default-off choice stated, plus the honest limit that no test captures the logger's stdout.
- **The matrix's status table was three rows low, again.** It read 99 / 10 / 17 = 131 under a
  sentence claiming it had been "recounted mechanically 2026-10-27 from the file"; the same method
  reads 100 / 10 / 19 = 129 on that commit. The two flips and one new row above make the live count
  102 / 8 / 19 / 0 / 0 / 4 / 2 = 135. The method is now stated exactly (strip the outer pipes,
  require five cells, take the status word before its first parenthesis).
- **The *Nested routers* row contradicted the *Per-route middleware* row** — "no nested state or
  middleware" against a delivered group middleware. Re-worded; the status stays
  `NOT_IMPLEMENTED`, because a mountable sub-app with its own state is what is missing.
- **The *README / docs accuracy* row named no document.** It now names the two pre-audit reports
  behind their `SUPERSEDED — 2026-10-18` banners and why they stay, the live-set gates
  (`cite_check.py`: 66 citations / 45 spans; `evidence_check.py`: 33 documents) and the unpinned
  teaching set track 92 will cover.
- **`aether-security`'s desync audit was not in the matrix at all.** New
  `UNWIRED_IMPLEMENTATION` row for `smuggling::audit_request_desync`; track 91 wires it.
- **PROB-0009 is closed, and two documents still called it open.** Roadmap §5 listed its "remaining
  half" among the breaking 0.2 items and register D3 among unmade decisions — both now point at D9
  and the 2026-10-27 ledger deletion. D3 also still named PROB-0016 and PROB-0019, both closed, and
  listed PROP-0001 as an owner decision although it is additive.
- **Register D2b still called the WebSocket graceful-close question open.** Track 86 delivered it;
  the line now records that, and why "current behaviour" was not the answer.
- **Register B21 is closed.** Its condition ended when track 73 slice 2 made the workspace clippy
  gate reach every crate and pass. The rule it registered survives; the note now distinguishes a
  cached `Finished` line (freshness) from an aborted command (a crate that was never linted).
- **The register's minor-finding block carried two dead line numbers** (`serve.rs:508` → `:634`,
  `app.rs:201` → `:229`, moved by tracks 84–86). All five bullets now use symbol anchors, per B9,
  so the next insertion cannot make them wrong.

Not done here, scheduled in the plan: D-07…D-10 (response-exit hygiene, track 89), D-11 (the
runtime loop, track 90), D-12 (citation-gate coverage, track 92), D-13 (the scheduling serial
guard, track 93), then the parity sequence F-01…F-20.

Gates on the final tree (Windows 10.0.26200 x64 MSVC): fmt 0 hunks / 0 files, exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0 with 0 warning/error lines (a cached `Finished`
line — see B21); tests 540 / 0 / 1 over 51 `test result:` lines; govtests 77 / 0; cite 66
citations in 10 documents over 45 spans, 0 findings; evidence 33 documents, 0 findings; validate
exit 1 with the four pinned findings. No code changed. Nothing pushed.

## Track 89 — the response exit stops publishing what a client cannot place (2026-10-27)

Continues roadmap §19.7 row 89 (D-07…D-10), with one row — D-18 — that was not in the inventory
and surfaced while fixing D-08. The method was the plan's: write the failing case first, watch it
fail for the reason the defect says, then fix the one boundary the test names.

The five wire cases live in a new target, `framework/tests/tests/response_exit.rs`, because the
contract belongs to the shared response exit rather than to one protocol or one feature. What each
measured **before** the fix, and after:

- **D-07 — an unclamped `error_status()`.** A type declaring `42`, `200` or `700` had that value
  written as the status line. Now the blanket `impl IntoResponse for Result<T, E>` replaces any
  declared status outside `400..=599` with `500`, code `E5002`, the canonical phrase and no detail;
  a `418` still answers `418`, keeps `E4003` and keeps its message, so the boundary is a range test
  and not a whitelist. Pinned by `an_error_status_outside_the_error_range_answers_500`.
- **D-08 — a second `X-Request-ID`.** A handler that set the field produced two entries on the wire,
  the handler's first: the test observed `["forged-by-handler", "req_000000000001"]`. `post_process`
  now retains every field that is not `x-request-id` and pushes exactly one framework id, so the
  framework's value wins and nothing else is disturbed (`X-Kept` survives). Pinned by
  `a_handler_cannot_publish_a_second_x_request_id`.
- **D-09 — a lone informational status.** Track 58 refused a `101` the framework did not produce and
  said the rest of the class was undecided; a handler's `103` or `199` was written as a bare interim
  head, leaving the client to wait for a final response. The exit now answers `500` for any status
  in `100..200`. `100 Continue` is not affected: the read loop writes it straight to the stream, so
  no legitimate informational status reaches the exit. Pinned by
  `every_1xx_from_a_handler_is_refused_not_only_101`. The rule was falsified in both directions,
  not just asserted: narrowing the condition back to `status == 101` made
  `a_101_without_a_handoff_is_never_served` fail with `left: 103, right: 500`, which is the
  failure the comment now cites. That existing test had itself pinned the narrow scope, and was
  rewritten to expect `500` for a `103` while keeping the `418` boundary intact.
- **D-10 — two names for 413.** Three hand-written refusals said `Payload Too Large` while
  `canonical_reason(413)` — RFC 9110's `Content Too Large` — said otherwise: the `AetherError`
  arm, the buffer-limit refusal in `app.rs`, and `Refusal::TooLarge` in `http2.rs`. All three now
  call the canonical function, and both languages of `docs/00` name the current phrase. Pinned on
  both paths by `a_body_limit_refusal_names_the_canonical_413_phrase` (a handler's
  `BodyLimitExceeded`, and a `Content-Length: 3_000_000` head refused at the 2 MiB default) plus
  the same-named `http2` unit test.
- **D-18 — a documented setting that did nothing.** `AetherConfig::x_request_id` was declared,
  documented, and read nowhere outside `config.rs`, so `false` changed no byte. It is now carried
  into `ServiceCore` and read by `post_process`; `false` drops the header. The doc was wrong in the
  other direction too — it claimed disabling avoided an allocation, but the id is still generated
  for the request context and the error reporter, so it saves the field only. Pinned by
  `the_request_id_header_follows_the_config_flag`.

Interface text corrected with the behaviour: the trait's `error_status` doc states the
`400..=599` boundary and what a value outside it costs; `AetherApp::ws` says the refusal covers the
informational class and names the three tests, including why `100 Continue` is exempt; the config
field's doc says what disabling a header does and does not save.

The edits moved code, so every citation they moved was re-derived rather than guessed: `app.rs`
grew by 4 lines in the trait doc and 10 inside the blanket impl, `serve.rs` by 8 at the exit and 4
in `post_process`, `http2.rs` by 1 import line. Root `docs/` citations were corrected in `docs/00`,
`02`, `05`, `07` and `09`, and the 24 pin keys in `cite_check.py` were moved with them — the gate
reports 0 findings. The matrix's coordinates were re-derived in the same pass (trait `121-163`,
impls `165-183` / `185-218` / `220-261`, clamp `233-244`, `http2.rs` `659` and `566`,
`serve.rs` `596-603`); that pass also found two **pre-existing** stale `http2.rs` coordinates
(`665` → `659`, `525` → `566`) that tracks 84–86 had moved without a citation gate noticing, which
is the D-12 gap in miniature.

One mechanical accident: `cargo fmt --all` normalised five touched `.rs` files to LF while the
tree is CRLF. They were rewritten byte-for-byte to CRLF and `cargo fmt --all -- --check` passes
under `newline_style = Auto`.

Recount from the per-target `test result:` lines: **546 passed / 0 failed / 1 ignored** over 52
lines = 309 unit + 231 integration across 17 targets + 6 doc. Track 89 adds 5 integration tests
(the new `response_exit` target) and 1 unit test (`http2.rs`, 13 → 14; the `aether` lib, 60 → 61).
`http_e2e` stays at 54 — one test was rewritten, not added — and `http2_e2e` stays at 40.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0 with 0 hunks
over 0 files; clippy `--workspace --all-targets -- -D warnings` exit 0 with 0 warning/error lines;
tests **546 / 0 / 1** over 52 `test result:` lines; govtests 77 / 0 (`Ran 77 tests … OK`);
cite 66 citations in 10 documents over 45 distinct spans, 0 findings; evidence 33 documents,
0 findings; validate exit 1 with exactly the four pinned findings and no fifth. Nothing pushed.

## Track 90 — the prototype loop learns what a byte stream is (2026-10-27)

Roadmap §19.7 row 90 (D-11). Two tests were written against the unmodified loop and both failed
for the reason the row names, before any production line changed:

- two pipelined requests in one read produced **one** answer — `matches("HTTP/1.1 ").count()`
  was `1` where the test required `2`, because the parser's `consumed` was discarded and the bytes
  of the second request went with it;
- one request split across two reads produced **`""`** — the second read began mid-request, the
  parser returned an error, and `?` ended the connection without writing a byte.

The loop now owns a `conn_buf`: it serves every complete request already buffered, then reads more
into the same buffer, and advances with `split_to(consumed)` after a response is written. A framing
error is answered — `431` for `HeaderTooLarge`, `413` for `BodyLimitExceeded`, `400` for anything
else, each with `Connection: close` — because the rest of the stream is undecidable once framing
breaks; previously the error path wrote nothing at all.

One shape constraint is worth recording: a `RequestView` borrows `conn_buf`, and the tuple holding
it has a destructor, so `split_to` cannot follow a still-live parse result. The router match moved
into `fn respond(&RadixRouter, &RequestView) -> Response`, which ends the borrow before the buffer
is advanced. The alternative — `unsafe` or an index arithmetic dance — buys nothing here.

A third test covers the new error path: a request line with no target must be one `400`, carry
`Connection: close`, and not be silence. `lib.rs` was also wrong about the crate it documents —
it said split requests are "silently dropped" — and now states what the loop does and what it
still does not (demo router, no `Host` enforcement, no I/O timeouts). The crate is repaired, not
adopted: `aether` still depends on it without calling it (register D6, an owner decision about the
publish payload), and `AetherApp` is still the supported path.

Recount: **549 passed / 0 failed / 1 ignored** over 53 `test result:` lines = 309 unit + 234
integration across 18 targets + 6 doc. The three new tests are the `aether-runtime` crate's first,
which also removes it from the zero-test column of the baseline reports (those stay dated snapshots).

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **549 / 0 / 1** over 53 `test result:`
lines; govtests 77 / 0; cite 66 citations in 10 documents over 45 spans, 0 findings; evidence 33
documents, 0 findings; validate exit 1 with exactly the four pinned findings. The first gate run on
this track failed **cite and govtests together** — the citation gate caught both spans this change
moved (`docs/02` → `worker.rs:23` became `:24`, `lib.rs:3-9` became `:3-11`), which is the gate
doing its job rather than a defect shipped; both were re-derived and the pins moved before the run
above. Nothing pushed.

## Track 91 — the facade learns to carry security and OpenAPI (2026-10-28)

Roadmap §19.7 row 91 (F-01, F-02), plus D-19, which the track itself exposed.

The test came first: `framework/tests/tests/facade_wiring.rs` imports only `aether::…`, and
against the old facade it did not compile. Every path the row promised was unresolved —
`aether::header`, `aether::JwtValidator`, `aether::JwtValidationOptions`, `aether::security`,
`aether::openapi`, `aether::constant_time_eq`, `aether::audit_request_desync` (E0432 / E0433 /
E0425). That is the failing-first evidence for a wiring change: the downstream-style import *is*
the behaviour under test, so "it compiles and passes" is the fix.

The wiring is two manifest lines and one block of re-exports. `aether` now depends on
`aether-security` and `aether-openapi`; the root exports `JwtValidator`,
`JwtValidationOptions`, `VerifiedJwt`, `constant_time_eq`, `audit_request_desync`, and the modules
`security`, `openapi` and `header`. `security/src/lib.rs` also exports the options and
verified-token types at its own root — until now `verify` could not be called without naming
`jwt::…`, so a facade re-export alone would still not have made JWT validation usable. The prelude
is unchanged: authentication is not request-path vocabulary.

Then the OpenAPI test read the generated document and failed: `operation_id` where OpenAPI 3.1
says `operationId`. D-19, and it had survived because no shipped crate had ever inspected the
builder's output. Fixed with a serde rename and pinned in the crate by
`the_document_uses_the_specs_field_names`, which asserts the spec key and the absence of the Rust
one.

Four facade claims, one per integration test:

- `a_jwt_verifies_through_the_facade` — a precomputed HS256 token (the test is not its own
  signer) verifies, its claims read back, and the same header/payload under another key is
  rejected;
- `constant_time_comparison_is_reachable_through_the_facade` — the root name and the
  `aether::security` module path both answer;
- `the_desync_audit_rejects_conflicting_framing_through_the_facade` — CL+TE and dual CL are
  `MalformedRequest`, an unambiguous head is accepted;
- `an_openapi_document_is_generated_through_the_facade` — a path and operation built by hand
  serialise as `openapi` 3.1.0 with the title and `operationId`.

Docs were corrected rather than left to drift: the matrix's four rows and its status table
(**105 / 9 / 19 / 0 / 0 / 0 / 2 = 135**, mechanically recounted), the header headline (540 → 554,
so the chain ends where the recount does), `AETHER_CORE_CAPABILITIES.md`'s wiring note, the
Arabic full documentation's crate table, wiring note and two examples, and the readiness report's
dated banner (its body stays frozen). The zero-dependent crates that remain are
`aether-observability` and `aether-serialization`. PROP-0001 is closed in the ledger with `done`,
and the proposal's 2026-10-20 caveat that `-openapi` stays out is superseded by F-02, which the
owner's plan authorized.

Recount: **554 passed / 0 failed / 1 ignored** over 54 `test result:` lines = 310 unit across 20
binaries + 238 integration across 19 targets + 6 doc. The five new tests are the four facade tests
and one `aether-openapi` unit test.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **554 / 0 / 1** over 54 `test result:`
lines; govtests 77 / 0; cite 66 citations in 10 documents over 45 spans, 0 findings; evidence 33
documents, 0 findings; validate exit 1 with exactly the four pinned findings. Nothing pushed.

## Track 92 — the teaching documents enter the citation gate (2026-10-28)

Roadmap §19.7 row 92 (D-12). The row's estimate was "21 citations in 5 teaching documents"; the
corpus had moved since: 27 citations over 24 spans in 5 live documents under `framework/docs/`, 21
of those spans never cited by the root set, plus 61 citations and 13 bare coordinates in the dated
records.

`cite_check.py` read `docs/*.md` only. `DOC_GLOBS` now also reads `framework/docs/**/*.md`, so the
teaching set is held to the same resolution, bounds and both-ends pin rules as the root set.
Correcting the live coordinates came first, because a gate added over wrong coordinates would have
had to be born red. The sweep corrected 10: `worker.rs:34` → `:32` (the `RequestView::parse` call,
not the match arm below it); `node.rs:41-43` → `:43` (the bitmask expression, not the attribute and
signature above it); the error guide's four `app.rs` spans (`121-214`→`121-218`,
`181-195`→`185-199`, `121-159`→`121-163`, `161-249`→`165-261`); its two test coordinates
(`http_e2e.rs:3012`→`:3016`, `:3048`→`:3052`); and the two in the Arabic full documentation
(`216-249`→`220-261`, `121-159`→`121-163`). The error guide also wrote
`` `core/src/error.rs:33,37` `` — a comma-separated pair neither citation form could see, so both
codes were unchecked; it is now two citations.

The legitimate exception is a dated record. `SNAPSHOTS` names the nine (six audit/benchmark reports
dated 2026-10-18, three perf-baseline notes dated 2026-10-19/20), and each carries
`<!-- cite-check: snapshot DATE -->` as its first line (after the BOM in
`CONSOLIDATED_FINAL_REPORT.md`). The checker requires both halves and refuses a marker in a
document that is not listed, so a live document cannot exempt itself; their 61 citations are
counted and printed but not validated, because the code moved on precisely because the record is
history, and re-deriving its coordinates would falsify it.

Seven governance tests pin the refusals (84 total, was 77): a listed snapshot is exempt; a listed
snapshot without its marker fails; a mismatched date fails; an unlisted document carrying the
marker is reported *and* still checked; a BOM before the marker is still read; the glob covers both
roots; every listed snapshot exists with the exact first line.

Recount: 93 citations over 66 distinct spans in 15 live documents, 0 findings — 66 citations over
45 spans in the root set (unchanged) plus the 27 above — and 61 citations in 9 dated snapshots, not
validated. No Rust changed, so the suite is unchanged at **554 / 0 / 1** over 54 `test result:`
lines.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **554 / 0 / 1**; govtests 84 / 0; cite
93 / 66 / 0 plus the snapshot line; evidence 33 documents / 0; validate exit 1 with exactly the
four pinned findings. Nothing pushed.

Each commit was also checked on its own exported tree. The docs commit is green under the checker
it ships with (66 / 45 / 0 — the old glob correctly ignores `framework/docs/`); the gate commit is
green under the new checker and all 84 tests. One failure appeared only in the export: four
`file:///C:/Users/hamza/...` paths in `PRODUCTION_READINESS_REPORT.md` are "outside the repository"
when the tree is extracted to `C:/tmp`, and the same four fail on the base commit's export — that
is the export location meeting `evidence_check`'s absolute-path rule, not this track. At the
repository's own path evidence is 33 / 0, above.

## Track 93 — the scheduling binary stops competing with itself (2026-10-29)

Roadmap §19.7 row 93 (D-13). The row's first step was to run `framework/tests/tests/scheduling.rs`
under a concurrent load twice before touching it, and this sitting it did not fail: 18 / 18 green at
3.51 s alone (default eight test threads), 2.25–2.50 s in two concurrent copies (twice), 2.14–2.41 s
under eight CPU-saturating processes, 2.63–2.89 s beside a full `http_e2e` binary (which was itself
54 / 0 both times), 2.12–2.21 s with all eighteen tests started at once (`--test-threads=18`), and
2.30–2.69 s in three copies at eighteen threads (twice). No reproduction is recorded, because there
was none.

The recorded failures are still explained, and the explanation is read from the code rather than
guessed: the 503 those nine clients received comes from connection admission (the `sched_conn.admit`
arm in `app.rs`), whose only clock-bound rejection is `AdmitFailure::TimedOut` after `max_queue_wait`
— 5 s in `scheduler/src/config.rs` — because `max_queue` is 1,024 and 120 clients cannot fill it,
and 120 × 32 KiB of connection overhead cannot approach the 256 MiB default budget.
`stress_120_concurrent_clients_over_scheduled_server` drains 120 clients through two handler slots
sleeping 5 ms, so its nominal critical path is about 300 ms: a 16–17× margin against that 5 s wait.
The recorded run took 48.65 s against 2.04 s — a 24× slowdown of the host — which lands past the
margin and makes the tail of the queue time out, and the framework answers with exactly the 503 it is
designed to answer. The same starvation is what pushed two tests into `Elapsed(())` at their 30 s
bounds. That failure was true to the host and false to the test's subject.

So the remedy is the row's: `SERVER_TURN`, a process-wide `tokio::sync::Mutex<()>` built with
`Mutex::const_new` so it needs no lazy initialisation, acquired at the top of `SchedServer::start`
and released by the `_turn` guard field when the server drops — which unwinding also runs, and which
`stop()` also reaches on its early `?`. All 17 start sites keep one server at a time and stop it
before the next begins (the policy sweep starts and stops in a loop), so the guard cannot deadlock.
Nothing else changed: the 30 s client bounds, the 5 s queue wait and every assertion are where they
were, because a looser bound stops catching the slowdown that is worth catching.

What the guard buys is measurable in the same binary: before, eighteen tests overlapped roughly three
times their serial critical path (2.1–3.5 s parallel against 7.3–7.8 s serialised, four runs each
way), and on a host with more cores libtest would start all eighteen at once — eighteen four-worker
runtimes, up to eighteen servers, up to eighteen 120-client surges. After, the binary asks a shared
host for one server's workload whatever its core count. It does not make a starved host fast; it
removes this binary's own contribution to the starvation, which is the only lever a test has. The
cost is the binary's wall time, 2.1–3.5 s to 7.3–7.8 s, on a suite whose totals are unchanged.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **554 / 0 / 1** over 54 `test result:`
lines; govtests 84 / 0; cite 93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents /
0; validate exit 1 with exactly the four pinned findings. Nothing pushed.

## Track 94 — per-route middleware without a second router (2026-10-29)

Roadmap §19.7 row 94 (F-03). The matrix's two per-route rows were `PARTIALLY_IMPLEMENTED` because
the only way to scope a chain was a `RouteGroup` with a prefix: an empty prefix (`app.group("", …)`)
produced the right wire result and the wrong contract — it also carried the group's state and could
not be attached to a route registered directly on `AetherApp`. The row's constraint was to preserve
the empty-prefix group's behaviour, not to replace it, and it is the control in one of the new
tests.

The API is registration-time binding, not path matching, so a chain attached to one route cannot
leak to another method on the same path:

- `middleware_entry` (private) is the single closure-to-`MiddlewareEntry` conversion, mapping
  whatever the user's middleware returns into the chain's response type. `AetherApp::use_middleware`
  and `RouteGroup::use_middleware` now go through it too, so the app-wide, group and per-route
  stacks share one conversion rather than three copies.
- `route_sync_with`, `route_async_with` and `ws_with` pass a one-entry slice to the existing
  `*_scoped` paths, so composition order (app stack, then the route's own chain, first registered
  outermost) and the WebSocket-upgrade path are the machinery the previous tracks already tested,
  with no parallel dispatch to keep in step.

Seven new tests, each on the wire. Five in `user_middleware.rs`: a per-route chain covers one
registration only (another method on the same path and a sibling route see none of it), it runs
inside the app stack and inside its group, a guard answers 401 and the handler counter stays at
zero, it composes with extractors, and a per-route chain agrees byte-for-byte with an empty-prefix
group's. Two in `http_e2e.rs` over a real WebSocket handshake: a per-route middleware wraps one WS
route's upgrade request and stamps a value the handler reads (with a sibling `/open` route as the
control), and a per-route guard refuses the upgrade with 401 before any session starts, then the
same route upgrades when the guard is satisfied. One `rust,no_run` doc example on `route_sync_with`
compiles as the seventh doctest.

**Sensitivity, run before the tests were trusted.** Two mutants were applied to the new API and
reverted: (a) `*_with` passing an empty slice, i.e. silently dropping the chain — all seven new
tests fail, so none of them can pass without the feature; (b) the per-route entry pushed onto the
app-wide stack, i.e. a chain that leaks to every later route — the three scope tests fail (one
registration only, inside the app stack, extractors) while the guard and group-agreement tests
still pass, which is the shape a leak produces and not an all-red mask. The final tree is green
with both mutations absent.

**The app.rs insertion moved 27 citations.** Adding 93 lines to `app.rs` (the shared conversion,
two sync/async methods with their doc comment, `ws_with`) and 133 to `http_e2e.rs` (the two WS
tests) shifted every pinned span below them: 18 distinct `app.rs` spans and 2 `http_e2e.rs` spans
across `00_framework_specification_{ar,en}.md`, `02_tcp_framing_and_buffering.md`,
`05_request_context_and_memory.md`, `07_limits_and_dos_mitigation.md` and
`ERROR_HANDLING_GUIDE_AR.md`. Each coordinate was re-derived from the checker's own
"pattern now matches at" line rather than by a blanket offset, and two spans the checker called
non-unique (`app.rs:2241`/`:2291`, the two textually identical `err_res.write_http1_to(&mut
write_buf)` exits) were resolved by the region the surrounding pins agree on. The pin table in
`cite_check.py` was moved with the docs in the same commit, because the pin is what makes a
coordinate edit a reviewed decision.

**A recount that first said "drift" — and the recount was the thing that was wrong.**
`FEATURE_COMPLETENESS_MATRIX.md`'s status table claimed 105 / 9 / 19 / 0 / 0 / 0 / 2 = 135 for the
track-91 tree. The first recount of this ticket was an `awk` requiring exactly seven pipe-delimited
fields and a feature name starting with a letter; it read 104 / 9 / 19 / 0 / 0 / 0 / 2 = 134 on
`f3b1e02`, `cba12f1` and `72459e6`, and the story written from it — track 88 carried forward a
wrong "134 before", track 91 added to a base of 102 no commit had — was published in this
repository for part of one sitting before the number was re-run. It was a tool artifact: one table
row, *HTTP/2 over TLS*, had lost its closing `|`, so it split into six fields and the `awk` dropped
it silently. The documented rule (strip only the outer pipes, require exactly five cells, take
`cells[1]` before its first parenthesis) reads **105 / 9 / 19 / 0 / 0 / 0 / 2 = 135** on those same
commits, and 134 (100 / 10 / 19 / 3 / 2) on `e60eca7` and 135 (102 / 8 / 19 / 4 / 2) on `72459e6`:
no drift existed, and the published table was right. The row now carries its closing pipe, so a
field-counting parser and a cell-counting parser agree. The lesson is the repository's existing one
in a new shape — a gate that filters is a gate that can drop evidence — and the seven-field rule
was replaced by the cell rule in the same pass. Track 94 moves two rows
(`Per-route middleware/guards` at line 274, `Per-route middleware` at line 326) from
`PARTIALLY_IMPLEMENTED` to `VERIFIED_IMPLEMENTED`, so the measured table is now
107 / 7 / 19 / 0 / 0 / 0 / 2 = 135, and the paragraph above it records the artifact rather than a
non-existent drift. The suite total in the same document moved 554 → 562 (245 integration + 310
unit + 7 doc, `user_middleware` 8 → 13, `http_e2e` 54 → 56).

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **562 / 0 / 1** over 54 `test result:`
lines; govtests 84 / 0; cite 93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents /
0; validate exit 1 with exactly the four pinned findings. Nothing pushed.

One pointer was wrong on arrival: `TRACK_INDEX.md` placed the track-93 heading at line 3535, and
`git show 1803c4e:.project-governance/notes/2026-10-21-improvement-register.md | grep -n` puts it
at 3536. Corrected in the same commit as this row's pointer (3579).

## Track 95 — the upgrade learns to name a subprotocol (2026-10-29)

Roadmap §19.7 row 95 (F-04), whose only constraint was "RFC 6455 §4.2.2 selection rule". The
matrix row was `PARTIALLY_IMPLEMENTED` because every `101` this server wrote carried
`Upgrade`/`Connection`/`Sec-WebSocket-Accept` and nothing else: a client that offered
`Sec-WebSocket-Protocol: chat.v2` was upgraded into a session with no agreed dialect, which §4.2.2
says a client must treat as a protocol failure. Three decisions were taken here and none of them are
forced by the RFC, so each is written down with the alternative it beat:

- **The route's declaration order decides, not the client's.** `select_subprotocol` walks the
  route's list outermost and returns the first name the client also offered. The alternative —
  first token of the client's `Sec-WebSocket-Protocol` that the server supports — is what many
  implementations do and it hands preference to the peer. A server that declares
  `["chat.v2", "chat.v1"]` is stating which one it would rather speak.
- **Only a name the route declared can ever be echoed.** The selected string is not a slice of the
  request: the returned variant borrows from the *supported* slice, and the `101` is formatted from
  that. Client-controlled bytes reach the response only if they are byte-equal to a server constant,
  so header injection through the offer is not a category that needs sanitising.
- **"offered nothing" and "offered nothing that matches" are different answers.** A `ws` route
  (no declaration) ignores the offer and upgrades — the track-86/94 behaviour, kept. A route that
  *does* declare names answers `400` when the client's offer matches none of them, because a client
  that asked for a subprotocol must fail a `101` that names none; refusing with a status is the
  server saying why while it can still speak HTTP. `ProtocolSelection::NotOffered` exists as its own
  variant so that no-offer can never be mistaken for no-match, including when the header is present
  but empty or all-OWS.

The API is one call on the existing registration paths rather than a new builder type:

- `aether_websocket::select_subprotocol(offered: Option<&str>, supported: &[&str]) ->
  ProtocolSelection<'_>`, with `is_http_token` (RFC 9110 §5.6.2 `tchar`) gating the server's own
  declarations — a route that declares `"bad name"` simply never matches, it cannot emit an
  invalid header.
- `AetherApp::ws_subprotocols(path, &["chat.v2"], handler)` and the same method on `RouteGroup`
  (which forwards to `ws_scoped`, so the group's prefix and middleware chain apply unchanged).
  `ws` and `ws_with` pass an empty declaration and keep their old behaviour.
- `ServiceCore` carries `ws_subprotocols: HashMap<u32, Arc<Vec<String>>>` keyed by route
  registration id; `ServeOutcome::WebSocketUpgrade` gained `protocol: Option<String>`, and the two
  frontends (HTTP/1.1 loop, h2) write the line only when it is `Some`. The unmatched-offer `400` is
  a normal `Response` produced *inside* the upgrade arm rather than an early return, so it still
  goes through `post_process` and carries `X-Request-ID`, the security headers and the log line.

**Nine new tests, none of which can pass on the old code.** Four unit tests in `handshake.rs`:
declaration order beats client order, OWS around tokens is ignored and comparison is case-sensitive,
no-offer is distinct from no-match, and only a declared name is ever echoed (a client token that is
not in the list never appears in the selection). Five wire tests in `http_e2e.rs` over real
handshakes: the echo follows the route's order (two connections, `chat.v2` then `chat.v1`), a route
without a declaration ignores the offer, an offer that matches nothing is answered `400` with the
handler counter still at zero **and a follow-up `GET /health` on a fresh connection answered `200`**
— the second half is what proves no framing switch happened on the refused hop, a client that
offers nothing still gets the route, and a group WebSocket route negotiates with the group's prefix.

**Sensitivity, run before the tests were trusted.** Two mutants were applied and reverted:
(a) always echo the first declared name regardless of the offer — killed by all four unit tests and
three of the five wire tests (the two survivors are the no-declaration and no-offer cases, which is
the correct shape: they assert *absence* of a header); (b) let client order decide — killed by
`selection_prefers_the_routes_declared_order` and the two-connection wire test. The real
implementation was restored from a byte copy and re-run: `aether-websocket` 35 / 0, the protocol
filter of `http_e2e` 6 / 0.

**The insertions moved 22 pins.** 91 lines in `app.rs`, 93 in `serve.rs`, 2 in `http2.rs` and 198 in
`http_e2e.rs` shifted every pinned span below the insertion points: 18 `app.rs` spans, 2 `serve.rs`
spans and the 2 `http_e2e.rs` spans the error guide cites. Every new coordinate was re-derived from
the checker's "pattern now matches at" line, each multi-line span's end pattern was confirmed to sit
at `new_first + (old_last - old_first)`, and the doc prose and the `PINS` table were moved in one
pass keyed on the old coordinate pair rather than by a blanket offset — a blanket offset would have
corrupted `05_request_context_and_memory.md`'s three `:NNNN` shorthand citations and
`02_tcp_framing_and_buffering.md`'s `:2288`, which have no `app.rs:` prefix to match on.

**Recount.** The measured table is 108 / 6 / 19 / 0 / 0 / 0 / 2 = 135: one row
(*WebSocket subprotocol negotiation*) moved `PARTIALLY_IMPLEMENTED` → `VERIFIED_IMPLEMENTED`. The
suite total moved 562 → 571 (314 unit + 250 integration across 19 checked-in targets + 7 doc, over
54 `test result:` lines; `aether-websocket` 31 → 35, `http_e2e` 56 → 61). Per-request and
per-connection costs are unchanged: the negotiation runs once per upgrade, on a `HashMap` lookup and
a scan of the route's own declaration.

**The index that points at this register was itself wrong, and that is the fourth time.** Adding a
paragraph to the register moved `## Track 95` and every heading after it, which is how the row that
names it went stale before it was written; but checking the whole table against the files found
drift that predates this track: `## Track 82` was at 2942, not 2885; `## Track 79`/`80`/`81` were at
2867/2783/2749, not 2810/2726/2692 (the row also listed them backwards); the WORKING_CONTEXT row
said `## Active Track 10` sat on line 331 when it sits on 268 and `## Active Track 15` on 658 when
it sits on 553; and the roadmap's §19 was on 453, not 448. Track 93 found one pointer off by one,
track 94 fixed it and was itself one line off, so the repair is now structural rather than another
edit: **a growing file is pointed at by quoting its heading, never by a line number**, and
`test_track_index.py` enforces it. Four rules, each with a test that fails on the real repository:
a line-number pointer after a quoted heading is refused; a quoted heading must be a prefix of
exactly one heading in the file its row names; every `## Track` heading must be claimed by a row
(so a track cannot be recorded without being indexed); and the frozen `WORKING_CONTEXT.md` ranges
keep their numbers, with both ends checked against the heading the row says sits there. The checker
is also run on synthetic text — including a compliant one that must stay silent — because a gate
that has only seen a compliant repository is not known to be able to fail. Its two mutants on this
repository: deleting the track-95 row fails `test_every_register_track_is_indexed` and the
all-agree test with a message naming the unindexed heading; restoring `` `## Track 95` (line
3661) `` fails the all-agree test naming the pointer. Ten new tests, and the index table now quotes
25 heading prefixes (24 register, 1 roadmap) instead of 25 line numbers.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **571 / 0 / 1**; govtests 94 / 0; cite
93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents / 0; validate exit 1 with
exactly the four pinned findings. Nothing pushed. Per RFC 6455 §4.2.2 the selection is now explicit;
permessage-deflate stays owner-deferred and remains the only unimplemented part of the WebSocket
extension list.

## Track 96 — the OPTIONS answer and the nonce a page and its policy share (2026-10-29)

Roadmap §19.7 row 96 (F-05, F-06), two matrix rows that were `PARTIALLY_IMPLEMENTED` for the same
reason: each described a behaviour the framework could *host* but did not *provide*. `OPTIONS` was
answered only when CORS was enabled (a preflight) or when the application registered its own
handler, so RFC 9110 §9.3.7's "what does this target support" had no answer for an ordinary route;
and `CSP` was a static header string with no way to authorise the one inline script a page needs
without putting `'unsafe-inline'` back, which removes the protection the policy exists for.

**F-05, three decisions.**

- **The `Allow` value is computed from what the target really serves, not from the router's method
  bitmask alone.** `RadixRouter::allow_header_for_target(bitmask, head_served)` always adds
  `OPTIONS` (the generic responder is what makes it true) and adds `HEAD` only when `head_served`.
  The alternative — deriving `HEAD` from the `GET` bit unconditionally — advertises a method that a
  WebSocket route refuses, because `app.ws` registers `GET` and the upgrade gate rejects `HEAD`
  before the router's `HEAD`-serves-`GET` rule can apply. The caller therefore computes
  `head_served` from a WS probe and passes it in; the WS gate's own refusal writes
  `"GET, OPTIONS"` with a comment saying why `HEAD` is absent there.
- **A registered `OPTIONS` route wins.** The generic answer is the last arm of
  `rejection.or(preflight).or(options_fallback)`, so a user handler, a CORS preflight and a host
  rejection all run first. The alternative — making the responder sit in the router — would have
  made every `app.get` implicitly register an `OPTIONS` route and changed the `405` bitmask that
  existing tests pin.
- **`204 No Content`, not `200`.** §9.3.7 asks for the capabilities, not a representation; a body
  would be a second thing to keep true. The `Allow` line is the answer.

**F-06, three decisions.**

- **The nonce is a value the *handler* and the *header* share, so it is drawn by the serving core
  before dispatch.** It is inserted into the request's extension map as
  `aether_security::CspNonce` and threaded through `PostFix` into `post_process`. The alternative —
  generating it inside `post_process` — is simpler by one parameter and impossible to use: the
  handler has already produced the body by then, and a nonce the markup cannot name authorises
  nothing.
- **A policy asks for a nonce by writing `{nonce}`; nothing else does.** `needs_nonce()` is that
  substring test, and the core reads it before dispatch, so an app with no policy — or one without
  the placeholder — draws no bytes and inserts no extension. The alternative, always generating,
  costs an RNG call plus an `Arc<str>` per request for a feature most apps do not use.
- **With no nonce available the source expression is removed, not emitted empty.** `'nonce-'` is an
  invalid source expression that browsers ignore, so leaving it in would keep every other directive
  enforced while silently dropping the one permission the author asked for; omitting the header
  would serve the page with no policy at all. Removing the expression is the failure that permits
  least: a page whose inline script cannot be authorised is served with a policy that blocks it. And
  because no OS entropy means no unpredictable token, generation failure refuses the request with
  `500` rather than substituting something guessable.

The API: `aether_security::CspNonce` (`generate`, `from_token` — checked against CSP3 §2.3
`base64-value` so a `'` cannot end the source expression —, `as_str`, `source_expression`, `Clone`
by `Arc<str>` refcount) and `csp_hash_sha256`; `CSP_NONCE_PLACEHOLDER`,
`SecurityHeadersMiddleware::{needs_nonce, apply_with_nonce}` in `aether-middleware` (no dependency
edge on the crypto crate: the middleware renders a token it is handed); and
`AetherApp::enable_security_headers_with(mw)` in the facade. No new crate dependency: `ring` was
already a dependency of `aether-security`.

**Twenty-one new tests, none of which can pass on the old code.** One `trie.rs` unit test pins the
`Allow` value (`GET, HEAD, OPTIONS` for a `GET` route, no `HEAD` for a `POST` route, no duplication);
seven `csp.rs` unit tests cover the token shape, two draws differing, a clone outliving its original,
the quoting of the source expression, a token that would break out of its quotes refused, and the
known SHA-256 of the empty source; six `security.rs` unit tests cover the renderer (verbatim without
the placeholder, substituted with it, expression removed when absent, a directive left with only the
nonce permitting nothing, an unquoted placeholder also removed, and `needs_nonce` itself); four
`http_e2e` wire tests cover `OPTIONS` on a `GET` route (204, empty body, `Allow`, a follow-up `GET`
answering `200` on the same connection — the framing control — and `OPTIONS /missing` answering
`404`), the `Allow` on a `405` for two different targets, `OPTIONS`/`HEAD` on a WebSocket route, and
a registered `OPTIONS` route winning; two more wire tests cover the nonce (the header's token equal
to the handler's rendered one, a second response drawing a different one, and no extension attached
when the policy names none); and one `app.rs` doctest compiles the `enable_security_headers_with`
example.

**Sensitivity, run before the tests were trusted.** Two mutants, applied and reverted each time:
(a) the core draws the nonce but does not attach it to the request — killed by
`a_csp_nonce_reaches_the_handler_and_the_header_that_authorises_it` with the message "the handler
rendered a different nonce than the header names", and by nothing else, which is the correct shape
(the opt-out test asserts absence); (b) `post_process` calls `apply` instead of `apply_with_nonce` —
killed by the same test at the point where it extracts the nonce from the header. The
implementation was restored and re-run: `http_e2e` 67 / 0, `aether-middleware` 65 / 0,
`aether-security` 14 / 0, `aether-router` 25 / 0.

**The insertions moved 24 pins and 33 citations.** app.rs grew by 29 lines, serve.rs by 55, trie.rs
by 17 and http_e2e.rs by 243, which put every pinned span below those points somewhere else: 18
`app.rs` pins, 2 `serve.rs`, 2 `trie.rs` and 2 `http_e2e.rs`. The migration was keyed on the resolved
`(path, start, end)` tuple and not on a blanket offset, and each of the 24 new coordinates was
required to match its pin's first and last line *before* any document was edited, so the table could
not drift silently into a new wrong number. Three pins are not unique on their own pattern: the two
identical `err_res.write_http1_to(&mut write_buf);` lines, which only their coordinate separates, and
the `Connection`-close line that also appears in the pre-acceptance rejection path. Each was resolved
by carrying the mapping of the whole run (`2397 → 2426`, `2447 → 2476`, `2445 → 2474`), which is the
relation the old coordinates already had. Four citations are the bare `:NNNN` form that inherits the
file named earlier on its line (`:2348` in the TCP chapter; `:2397`, `:2447`, `:2847` in the request
context chapter); a rewrite keyed on the *file token* would have missed them, so the rewrite ran on
the resolver's output — the same resolution the checker uses — and rewrote only the digits. The
result is 93 citations over 66 distinct spans, 0 findings.

**Recount.** The measured table is 110 / 5 / 18 / 0 / 0 / 0 / 2 = 135: two rows (*Automatic OPTIONS
/ CORS preflight*, *CSP*) moved `PARTIALLY_IMPLEMENTED` → `VERIFIED_IMPLEMENTED`. The 6/19 split this
register's track-95 entry carried for the *partial* and *not-implemented* columns was off by one
each — the same rule on the track-95 tree gives 108 / 7 / 18 / 2 — so both the roadmap's §19.1 row
and this entry carry the re-derived numbers rather than the carried-forward ones. The suite total
moved 571 → 592 (328 unit + 256 integration across 19 checked-in targets + 8 doc, over 54
`test result:` lines; `aether-router` 24 → 25, `aether-security` 7 → 14, `aether-middleware` +6,
`http_e2e` 61 → 67, +1 doctest). Per-request cost when the feature is not configured is unchanged:
`needs_nonce()` is a substring scan of a `&'static str` the app already held.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **592 / 0 / 1**; govtests 94 / 0; cite
93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents / 0; validate exit 1 with
exactly the four pinned findings. Nothing pushed.

## Track 97 — a directory a server will list, and the compressed sibling a client asked for (2026-10-29)

Roadmap §19.7 row 97 (F-07, F-08), two matrix rows that named the same route's absences:
`static_dir` served files but could not list a directory and could not serve a pre-compressed
`.br`/`.gz` sibling. Both were deliberate defaults, so the work was to make each available without
changing what an existing `static_dir` route answers.

**Decisions.**

- **Both behaviours are opt-in on one options struct, and its default is the old behaviour.**
  `StaticDirOptions { max_age_secs, directory_listing, precompressed }` defaults to
  3600 / false / false. The alternative — flipping a default — would publish every file name under
  a root, and select different bytes for a client, in every app that upgrades without asking.
- **A directory that has an `index.html` is still that page, listing on or off.** A listing must
  not replace an app's home page the moment the flag is set; the check runs before the listing.
- **The listing is text and the links are percent-encoded segments.** `encode_segment` leaves the
  RFC 3986 §2.3 unreserved characters alone and escapes every other byte, so a file named `q?x#y`
  cannot become a query string, and an href round-trips because the router percent-decodes a
  captured parameter in `serve.rs`. `html_escape` covers `& < > " '`, so an uploaded file name can
  never become markup. Dotfiles, Windows device and alternate-data-stream names, non-UTF-8 names
  and symlinks are not listed: the resolver refuses each of them on request, so listing one would
  advertise a link that answers `404`. The page states its own truncation at 10,000 entries.
- **The trailing-slash redirect is keyed on the raw request path, not the captured segment.** The
  trie trims every trailing slash, so `/assets/guide/` and `/assets/guide` produce the same
  `filepath` capture; only `RequestContext.path` keeps that distinction. The first version compared
  the capture, and `/assets/guide/` answered `301` to itself forever — the wire test that asked for
  the directory caught it before the pin migration.
- **The tree root gets a route of its own only when listing is on.** A catch-all needs at least one
  character (`/static/` is a `404` by design, pinned by
  `test_catchall_captures_full_remaining_path`), so `/assets` could never reach the listing. With
  `directory_listing` on, the registration adds the prefix route as well; with it off the route set
  is exactly what it was.
- **A pre-compressed sibling is served only when it is canonically inside the root.** The base file
  was already resolved, but `app.js.br` is a different inode and can be a symlink out of the tree;
  the containment is re-checked after canonicalisation.
- **`Content-Encoding` is attached only when the status allows a body, `Vary: Accept-Encoding` on
  every answer, and a client that refuses identity gets `406`.** A cache that stored the plain body
  must not hand it to a client that asked for Brotli, or the reverse (RFC 9111 §4.1). When the
  request excludes identity (RFC 9110 §12.5.1) and no variant is acceptable, there is nothing to
  serve, and the route says so instead of serving the representation the client refused.

**The API.** `StaticDirOptions` (with `with_max_age`, `Default`), `AetherApp::static_dir_with_options`;
`static_dir` and `static_dir_with_cache` delegate with the defaults, so their behaviour and their
signatures are unchanged. `aether_middleware::compression::accepts_identity` is new and public, and
the q-parameter name is now matched case-insensitively (RFC 9110 §5.6.6), so `;Q=0` weighs the same
as `;q=0`. The facade re-exports `StaticDirOptions` at the crate root and in the prelude.

**Twelve new tests, none of which can pass on the old code.** Seven `static_dir` unit tests: the
names that may not be listed, the variant siblings that are not entries of their own, segment
encoding (including a pre-encoded `%2e%2e` that must be double-escaped), a hostile name rendered as
text, the listing's parent link and trailing slash, the root listing having no parent link, and the
redirect naming the directory once (`""` → `/assets/`, not `/assets//`). One `accepts_identity`
unit test: empty and gzip-only fields accept identity, an explicit `identity;q=0`, `*;q=0` and
`IDENTITY;Q=0` refuse it, and an explicit `identity;q=1` overrides a wildcard zero. Three `http_e2e`
wire tests: the listing off and on (including a traversal attempt still `404`, an `&` in a name
rendered as `&amp;` and linked as `%26`, a `.hidden` file absent, and both the redirect and the
escaped-href round trip), a directory with an index page still serving that page, and variant
selection (br over gzip on a tie, gzip alone, identity with no header, a variant ETag answering
`304` with `Vary` and no `Content-Encoding`, `identity;q=0` answering `406`, and both codings
refused falling back to plain bytes). One `app.rs` doctest compiles the `static_dir_with_options`
example.

**Sensitivity: nine mutants, each killed by its named test.** Applied and reverted one at a time:
(a) `listable_name` drops the dot-file rule → `a_name_that_cannot_be_served_is_never_listed`;
(b) `is_variant_name` forgets `.gz` → `the_variant_siblings_are_not_entries_of_their_own`;
(c) `encode_segment` escapes `.` → `a_segment_escapes_every_byte_that_is_not_unreserved`;
(d) `html_escape` passes `<` through → `a_hostile_name_is_rendered_as_text`;
(e) the parent link is emitted at the root → `directories_are_listed_with_a_trailing_slash_and_a_parent_link`;
(f) the redirect always appends the slash → `a_redirect_names_the_directory_once`;
(g) `index.html` no longer wins → `a_directory_with_an_index_page_is_still_that_page`;
(h) identity is acceptable under `*;q=0` → `test_accepts_identity_only_refuses_explicit_zero`;
(i) gzip wins a tie with br → `precompressed_variants_are_selected_by_accept_encoding`. Every one
aborted on an assertion panic, not on a build error, and the control run of the same target was
green before and after.

**The insertions moved 34 pins and 46 citations.** app.rs grew by 64 lines across four regions and
http_e2e.rs by 322, so every pinned span below those points moved: 32 `app.rs` pins and the 2
`http_e2e.rs` pins, and 46 citations over 9 documents. The migration did not apply an offset: it
built the old-to-new line map from the unchanged blocks of each diff, required every new coordinate
to match its pin's first and last line *before* writing anything, and rewrote only the digits of
each citation, keyed on the checker's own resolver — which is what catches the bare `:NNNN` forms
that inherit the file named earlier on the line (`:2441` in the TCP chapter; `:2490`, `:2540` and
`:2940` in the request-context chapter; `:3948` in the error-handling guide; and four bare ranges
in the matrix's typed-error row). The write is all-or-nothing at the end of the run:
an early attempt aborted with the key formatted in the wrong quoting style, and the pin table was
still exactly as it had been — the failure mode is a refused migration, not a half-migrated table.
After the rewrite: 93 citations over 66 distinct spans, 0 findings, plus the 61-citation snapshot
line.

**Recount.** The measured table is 112 / 5 / 16 / 0 / 0 / 0 / 2 = 135: *Directory listing* and
*Pre-compressed `.gz`/`.br` assets* moved `PARTIALLY_IMPLEMENTED` → `VERIFIED_IMPLEMENTED`. The
recount also found the matrix's own *Status count* section lagging the file it describes: at track
96 it read 108 / 6 / 19 while the 135 rows were 110 / 5 / 18, one short in the verified column and
one long in each of the other two — the docs-drift class this register records as B11. Both the
matrix and roadmap §19.1 now carry the re-derived numbers, and the drift is written down there
rather than silently overwritten. The suite total moved 592 → 604 (336 unit + 259 integration across
19 checked-in targets + 9 doc, over 54 `test result:` lines; `aether` lib 61 → 68, `aether-middleware`
65 → 66, `http_e2e` 67 → 70, +1 doctest), measured against a baseline taken in an isolated worktree
at `cd7878a`, not against a remembered number.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **604 / 0 / 1**; govtests 94 / 0; cite
93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents / 0; validate exit 1 with
exactly the four pinned findings. Nothing pushed.
## Track 98a — the caller behind the proxy, and the line a client plants (2026-10-29)

Roadmap §19 row 98 carries three security features (F-12 CSRF, F-13 bearer auth, F-14 forwarded
identity) that share one attacker position but touch three different layers, so the row is split
into 98a/98b/98c and the threat note
`.project-governance/notes/2026-10-29-track-98-threat-note.md` was written before any code, which
was the roadmap's own precondition. This entry is 98a = F-14. Row 98 stays open for 98b and 98c.

**Decisions.**

- **A configured trust list, not a signature.** RFC 7239 `Forwarded` is unsigned: nothing in the
  header can be verified, so the only sound rule is to believe a hop when the address that
  *delivered* it is one the operator listed. `TrustedProxies::new` takes CIDRs, canonicalises
  IPv4-mapped IPv6 (adjusting an explicit prefix by −96 rather than rejecting `/108` after
  canonicalisation), and refuses an empty list and every `/0` — each of those would believe every
  claim while looking like configuration.
- **Right to left, stopping at the first unlisted hop.** The walk starts at the transport peer and
  takes the chain's rightmost entry, continuing only while the address it just accepted is itself
  a proxy. That is what makes a forged left entry useless: the honest proxy's own appended entry
  is read first, and the forged one is never reached. A malformed element stops the walk where it
  stands, so an attacker cannot blank a hop to shift the chain's meaning (`6.6.6.6, garbage`
  resolves to the peer). The chain is capped at 32 hops.
- **`Forwarded` wins over `X-Forwarded-For` when both are present**, rather than the two being
  merged. They do not share a semantics for "what came before this hop", and reading one while the
  other disagrees produces two different answers to "who is this".
- **Repeated field lines are joined in wire order before the walk** (RFC 9110 §5.2). This one was
  found by the mutant sweep, not by reading the code: the serving core keeps repeated field lines
  as separate entries and `header_first` returns only the first, so a client that planted
  `X-Forwarded-For: 6.6.6.6` and let a proxy append its own line had the forgery adopted as the
  client address — the proxy's appended line was never read. `header_joined` implements the
  joining; the two mutants that revert either field to `header_first` are killed by assertions.
- **The identity is published; the socket stays named.** The resolved address is inserted as a
  `ClientAddr` extension (only when a list is configured, so an app that never uses the feature
  pays nothing), read through `ctx.client_addr()`, and `ctx.peer_addr()` keeps returning the
  transport address. The rate limiter keys on the resolved identity in both cases: without a list
  that is the peer, with one it is the caller, which is what stops every client behind a proxy
  from sharing one bucket.
- **A bad list panics at registration**, the policy host validation already set: a typo must not
  silently widen the set of addresses the server believes.

**Evidence.** Ten unit tests in `framework/middleware/src/forwarded.rs` (CIDR parsing including a
mapped block, the untrusted-peer refusal for both header forms, rightmost-untrusted resolution,
the forged left entry, the malformed stop, port and bracket forms, a quoted comma that is not a
separator, `Forwarded` precedence, the hop cap) and three `http_e2e` wire tests:
`a_trusted_proxy_gives_each_forwarded_client_its_own_budget`,
`an_untrusted_peer_cannot_name_its_own_bucket` (both shapes of "untrusted": no list, and a list
that does not contain the peer) and `the_forwarding_chain_resolves_to_what_the_handler_sees`
(forged left entry, precedence, malformed stop, port form, and the repeated-line join).

**Sensitivity.** Nine mutants, applied one at a time to the real sources and restored from a
byte-for-byte copy, each run against a named test: (M1) always believe the claim →
`an_untrusted_peer…`; (M2) key the limiter on `peer.ip()` → `a_trusted_proxy…`; (M3) skip a
malformed hop instead of stopping → `the_forwarding_chain…`; (M4) read `X-Forwarded-For` first;
(M5) walk the chain left to right; (M6) refuse a hop carrying a port; (M7) resolve but never insert
the extension; (M8) read only the first `X-Forwarded-For` line; (M9) read only the first
`Forwarded` line. **M1 survived the first sweep**, and the reason was a coverage hole rather than a
weak assertion: an app with no proxy list never reaches `client_ip` at all, so the test proved only
that the facade ignores headers when the feature is off. The test now also configures a list the
peer is not in — the case where a direct connection forges a header — and M1 is killed. M8 and M9
exist because the joined-header defect was invisible to the design review; both now abort on an
assertion.

**The insertions moved 23 pins and 31 citations.** `app.rs` grew by 42 lines above the first
pinned span (the `enable_trusted_proxies` method, the `trusted_proxies` field and its
construction, `client_addr()`, `header_joined`), `serve.rs` by 18 and `http_e2e.rs` by 190 (two
insertion points, above two pinned test coordinates). Two defects in the migration tooling itself
were caught before anything was written, and both were refusals rather than half-applied writes:

- the pin table rewrite collided with itself, because keys are renumbered **chained**: the move
  `2490 → 2532` wrote the text of a key that the loop then met again as the *old* `2532 → 2574`,
  which the unique-occurrence check refused. The rewrite now edits one pin line at a time, so a
  substitution never becomes another key's input;
- the document rewrite keyed its output by document **line number**, so a line carrying three
  citations kept only the last edit and silently reverted the other two (`05_request_context…`
  line 129 carries three). The checker caught it as three unpinned citations; the repair paired
  each reader with the moved pin whose pattern still matched at its *committed* coordinate, with
  difflib breaking the one tie where two pins share a pattern.

After the rewrite: 93 citations over 66 distinct spans in 15 live documents, 0 findings, plus the
61-citation snapshot line — the same shape track 97 left.

**Docs corrected while recounting.** `DEPLOYMENT.md` said one bucket per proxy "until signed
forwarded-client identity exists", and the production-readiness report said the same; both now
describe the trust list, including the fact that the headers are unsigned and the list is what
makes them believable. The matrix's *Rate limiting* row claimed "Behind a proxy one bucket covers
every client", which is no longer true, and a new *Forwarded client identity* row records the
feature; the status table grows to **113 / 5 / 16 / 0 / 0 / 0 / 2 = 136** (it was 112 / 5 / 16 at
track 97, and this track added a row rather than moving one). The suite moved 604 → 617 (**346
unit + 262 integration across 19 checked-in targets + 9 doc**, over 54 `test result:` lines;
`aether-middleware` lib 66 → 76 for the ten `forwarded` tests, `http_e2e` 70 → 73), recounted from
the gate log rather than from arithmetic on the previous number.

Gates on the final tree (`bash gates.sh`, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **617 / 0 / 1**; govtests 94 / 0; cite
93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents / 0; validate exit 1 with
exactly the four pinned findings. Nothing pushed.

## Track 98b — a token a browser does not send on its own (2026-10-29)

Roadmap §19 row 98's second slice: F-13, bearer authentication, per
`.project-governance/notes/2026-10-29-track-98-threat-note.md` §F-13. Row 98 stays open for 98c
(F-12, CSRF).

**Decisions.**

- **Verification is a library layer; the facade only adapts it.** `aether-security` gains
  `BearerAuth`, which owns an `Arc<JwtValidator>` and the `JwtValidationOptions` and turns the
  request's `Authorization` field lines into a `Principal` (subject plus the verified claims). The
  facade's `require_bearer` is a chain entry that calls it and attaches the result. The split is
  where the failures are enumerable in unit tests, and it lets a WebSocket handshake, a CLI tool or
  a test reuse the same check without the serving core. `Principal` is not `PartialEq` on purpose:
  its claims are an arbitrary JSON value, and equality on it would invite a handler to compare a
  whole principal when it means one claim.
- **One credential or none.** `Authorization` is a singleton field (RFC 9110 §11.6.2), so the
  layer refuses an empty list *and* a list of two. "Whichever line is looked at first" is a
  smuggling-shaped answer the moment two layers disagree. The scheme is case-insensitive
  (RFC 9110 §11.1); exactly one SP separates it from a `token68` credential
  (RFC 7235 §2.1); spaces after the first are extra separation, as the grammar's `1*SP` permits;
  and anything else — `Basic`, a two-segment JWT, an empty token, a character outside the
  `token68` set — is refused before any HMAC is computed.
- **One refusal shape.** Missing field, wrong scheme, malformed credential, forged signature,
  expired token, `nbf` in the future, `alg: none`, `alg: HS512` against the HS256 pin: every one is
  `401` with `WWW-Authenticate: Bearer error="invalid_token"` and an empty body. Telling an
  attacker which guess was closer is free information; the issuer tells the legitimate client
  through the token itself. The facade's unit test pins the exact header set, so a future
  "helpful" message is a test failure rather than a quiet weakening.
- **The principal is priced before it is stored.** `RequestContext::insert_extension_sized` takes
  the payload's owned heap, so `Principal::owned_heap()` counts the subject string and walks the
  claims value — arrays, objects, keys, nested values — under the rule track 74 set for extension
  payloads. An authenticated request therefore keeps the connection's memory registration honest
  even when the token carries a large custom claim.
- **Opt-in per route or per group.** `require_bearer` is just a `Fn(RequestContext, Next)`, so it
  composes with `route_sync_with`, `route_async_with`, `ws_with` and `RouteGroup::use_middleware`.
  Registration order is chain order: an app-wide layer protects what is registered after it and not
  what came before, which is why the documentation points at the per-route form for leaving public
  routes alone. Authentication is not authorisation — a handler that needs a role reads
  `Principal::claims` — and replay (a stolen token is good until `exp`), revocation, refresh tokens
  and asymmetric algorithms / JWKS are out of scope, stated in the module docs rather than left to
  be discovered.

**Evidence.** Eight unit tests in `framework/security/src/auth.rs` (verified subject and claims;
scheme case and spacing; twelve malformed shapes including two credential lines; forged, expired,
not-yet-valid, `alg:none` and `alg:HS512`; the `token68` grammar including trailing `=` padding;
issuer and audience pins applied to this layer; the priced heap; a validator shared by two layers),
one facade unit test pinning the refusal, and three wire tests in `framework/tests/tests/bearer_auth.rs`:
nine invalid shapes each asserted to be the *same* 401 with a handler counter still at zero; a valid
token reaching the handler as `{subject, role}` (and a lowercase `bearer` scheme); and a per-route
layer over `/public`, `/private` and `/also-public` proving that routes registered without it stay
open. Two new doc tests compile the module and function examples.

**Sensitivity.** Nine mutants applied one at a time to the real sources and restored from a
byte-for-byte copy, each run against a named test: (M1) accept any scheme →
`every_malformed_credential_is_the_same_refusal`; (M2) take the first credential line instead of
refusing two → the same wire test's two-line case; (M3) skip the `token68` check → **survived the
first sweep**; (M4) refuse the extra spaces → `the_scheme_is_case_insensitive_and_extra_spaces_are_a_separator`;
(M5) stringify the subject → the verified-claims test; (M6) leave `owned_heap` at zero →
`the_owned_heap_counts_the_subject_and_nested_claims`; (M7) verify but do not attach the principal
→ the wire test that reads it in the handler; (M8) refuse without the challenge header → the facade
refusal test and every wire case; (M9) put a reason in the refusal → the facade refusal test. **M3
was a coverage hole, not a weak assertion**: once the scheme and count checks passed, no test fed a
credential containing a character outside the `token68` set, so
`the_credential_must_be_a_token68` was added and M3 is now killed. The sweep script also had a
defect of its own: it backed up both `auth.rs` files under one basename, so the restore after M7
wrote the facade's text into `aether-security` and destroyed the untracked module. It was rebuilt
with distinct backup names and a sha256 restore check — every later restore verified byte-identical
— but the lesson is the one track 98a also learned: the tooling that proves a tree is clean needs
at least as much scrutiny as the tree.

**The restore was byte-identical and still wrong.** The rebuilt sweep copied the backup back with
its *original* mtime (the pre-sweep write time), so cargo's freshness check — source older than
output — treated the test binary built from the last mutant as current, and a later
`cargo test -p aether-security` reused it. The owned-heap assertion failed on code that reads
correctly, and only `touch`ing the file made the failure disappear. Byte-identical content is not
a restored build tree: the sweep now touches every restored file, and the figure below was
re-established from a forced rebuild, not from the tree that had just carried nine mutants. The
lesson for future sweeps is in the same place: verify the *artifact*, not only the source hash.

**Pins.** The two `lib.rs` files and the new modules carry no pinned spans, so nothing moved; the
citation gate is unchanged at 93 citations over 66 spans in 15 documents, 0 findings. The test
harness gains `ring` as a **dev-dependency only** — the wire tests mint their own tokens with the
same HMAC the server verifies with, and the runtime graph does not gain a signer the server never
uses.

**Docs corrected.** The matrix's *Authentication middleware* row moves from PARTIAL to VERIFIED
(opt-in `require_bearer`, the singleton field, the `token68` grammar, one refusal, the priced
principal, the three test layers); the *JWT validation* row now names it as the consumer; the
remaining-work list drops it and keeps only CSRF. The production-readiness report's "no
authentication middleware" line becomes the opt-in description, and item 8 narrows to CSRF.
`docs/09_api_reference.md` gains `ctx.extension` / `ctx.insert_extension` and an Arabic bearer
section with a compiling example; `docs/04_security_and_rfc9112.md` gains the refusal-shape
section. Status table: **114 / 4 / 16 / 0 / 0 / 0 / 2 = 136** (98a was 113 / 5; 98b moved one row
rather than adding one). The recount also exposed the matrix's *Integration tests* row still
carrying track 97's 259 tests / 19 targets while the narrative had already moved to 262 / 19 at
98a; it now reads 265 tests / 20 targets with `http_e2e` at 73, re-derived from the same gate log
rather than by adding this track's delta to a stale number.

**Recount.** 631 tests / 0 failed / 1 ignored over 55 `test result:` lines: **355 unit + 265
integration across 20 checked-in targets + 11 doc**, up from 617 by eight `aether-security` unit
tests, one facade unit test, three `bearer_auth` wire tests and two doc tests (355 unit + 265 + 11
= 631).

Gates on the final tree (`bash gates.sh` after `cargo clean -p aether-security -p aether -p
aether-tests` forced the swept crates to rebuild, Windows 10.0.26200 x64 MSVC): fmt exit 0; clippy
`--workspace --all-targets -- -D warnings` exit 0; tests **631 / 0 / 1**; govtests 94 / 0; cite
93 / 66 / 0 plus the 61-citation snapshot line; evidence 33 documents / 0; validate exit 1 with
exactly the four pinned findings. Nothing pushed.

## Track 98c — a token the browser sends twice, one half a page cannot forge (2026-10-29)

Roadmap §19 row 98's third slice: F-12, CSRF, per
`.project-governance/notes/2026-10-29-track-98-threat-note.md` §F-12. Row 98 closes with this slice
after 98a (forwarded identity) and 98b (bearer authentication).

**Decisions.**

- **App-wide, not per route.** A per-route CSRF layer protects exactly the routes the operator
  remembered to list, and the route forgotten is the one that changes state. `enable_csrf` installs
  one layer on the serving core; a machine endpoint that must stay out is an explicit reviewed
  exemption (`exempt_prefixes`), not an omission.
- **Signed double-submit, not a session binding.** The cookie is `nonce.mac`: 16 random bytes and an
  HMAC-SHA256 over their base64url text under a deployment secret. Double-submit alone would accept a
  cookie the attacker planted by any means (a subdomain, a proxy that rewrites `Set-Cookie`); the MAC
  makes "a cookie this server did not issue" distinguishable from "one it did", so a planted cookie is
  replaced rather than trusted.
- **One value checks itself.** A safe request is handed the token its cookie carries — or a fresh one
  when the cookie is absent or unsigned — through the `CsrfToken` extension the page renders; an
  unsafe request must echo it in exactly one `x-csrf-token` field, compared in constant time against
  the cookie the same request carried. Page and check therefore read one value, and a request is never
  checked against a token it was not itself issued.
- **Reuse, not rotation.** Rotating on every safe response would invalidate every open form and break
  a second tab; a valid cookie is reused and only its absence or invalidity produces a new
  `Set-Cookie`. The consequence is stated rather than hidden: with `max_age` set, the window counts
  from issuance, because a reused cookie is not re-sent. The module comment that said the opposite
  ("refreshed by every safe response") was written with the design and was wrong — corrected here.
- **One refusal shape, before the handler.** Missing cookie, missing header, two headers, an unsigned
  or foreign-signed token, a one-byte near miss, a duplicated cookie (cookie tossing through a
  sibling domain): every one is `403` with an empty body, no reason, no token echoed and no cookie
  refreshed — the refusal leaves the browser with exactly what it had.
- **Safe methods exempt by the method, not the route** (GET/HEAD/OPTIONS/TRACE, RFC 9110 §9.2.1): a
  safe response is how a page receives its token, so gating one would deadlock the very first render.
- **Attributes that a browser enforces.** `SameSite=Lax`, `Secure`, `Path=/`, no `Domain`, and not
  `HttpOnly` — a script on the page must read the cookie to echo it. `SameSite=None` or a
  `__Host-`/`__Secure-` name without `secure` is a browser that drops the cookie, so the constructor
  panics instead; so do an empty secret, a name that is not an RFC 9110 `token`, and an exemption of
  `/` or of a path that does not start with `/` — each of those is whole-app protection silently
  turned off.
- **Placement in the serving core.** Decided in step 3 beside host validation and rate limiting,
  after routing and before CORS preflight and dispatch, so both frontends share one gate; issuance
  lives in `post_process`, which HTTP/1 and HTTP/2 already share, and is skipped for a preflight and
  for any status `>= 400`. The extension is inserted with `insert_sized`, so the token's heap is
  registered against the handler job the way track 74 requires of every pre-admission entry.

**Evidence.** Twenty unit tests in `framework/security/src/csrf.rs` (token shape and MAC; a fresh
token verifying once echoed; a new nonce per issue; missing, ambiguous, duplicated and near-miss
pairs; a foreign secret; reuse only of this secret's cookie; the extension handing the bytes over
unchanged; the non-`nonce.mac` shapes; defaults and lowercase header; `__Host-` accepted when secure;
raw-prefix exemptions; and the five constructor refusals), and nine wire tests in
`framework/tests/tests/csrf.rs`: a failed response issues no token; a safe response issues the token it
hands the page with the documented attributes and no `HttpOnly`/`Domain`; configured `SameSite` /
`Max-Age` / `Secure` follow `CsrfConfig`; seven invalid credentials each get the same empty-bodied
`403` with the handler counter at zero and no `Set-Cookie`; the page/cookie pair is accepted; a valid
cookie is reused rather than rotated; a planted cookie is replaced by a signed one; safe methods are
not gated; and an exempt prefix is not gated while its siblings are.

**Sensitivity.** Twelve mutants applied one at a time to `csrf.rs` and `serve.rs`, each run against a
named test and restored afterwards: (M1) drop the constant-time equality → `a_mismatched_pair_is_refused_including_a_one_byte_near_miss`;
(M2) skip the MAC when reusing a cookie → `only_a_cookie_this_secret_signed_is_reused`;
(M3) keep the first of two cookies → `a_missing_or_ambiguous_pair_is_refused`;
(M4) issue `nonce.nonce` instead of a MAC → `a_fresh_token_verifies_once_it_is_echoed`;
(M5) read the first header line instead of requiring one → `a_missing_or_ambiguous_pair_is_refused`;
(M6) trim the trailing slash off an exemption → `exemptions_match_the_raw_prefix`;
(M7) every method safe → `every_missing_or_mismatched_token_is_refused_before_the_handler`;
(M8) no method safe → `safe_methods_are_not_gated`; (M9) rotate a valid cookie → `a_valid_cookie_is_reused_and_not_reissued`;
(M10) issue on a failed response → `a_failed_response_does_not_issue_a_token`;
(M11) issue the cookie `HttpOnly` → `a_safe_response_issues_the_token_it_hands_the_page`;
(M12) ignore the exemption list → `an_exempt_prefix_is_not_gated`. All twelve were killed; sources were
restored byte-identically (sha256) and the crate was rebuilt with `cargo clean -p aether-security -p
aether -p aether-tests` before the passing run was taken again — track 98b's stale-artifact lesson,
applied rather than repeated.

**Pins.** The facade grew 41 lines in `app.rs` (one import, the `csrf` field and initializer, and the
35-line builder) and 88 in `serve.rs` (the field, the step-3 decision block, the rejection arm, the
`PostFix` field, the issuance block and `is_safe_method`), and `response.rs` gained `add_cookie` so the
pipeline can add a cookie through `&mut Response`. Six pins above the `app.rs` field moved by +1 and
five below it by +41, and the two `serve.rs` pins by +56. All 38 moved keys were re-derived from the
working diff's hunks — `git diff --unified=0` old→new line map, each anchor verified at its mapped
line — after an anchor-based sweep refused the job: the pattern `err_res.headers.push(("Connection",
"close"` matches three lines, and a 1-of-3 hit is not evidence. Nine documents carried coordinates
those moves shifted, and the CSRF sections add five spans of their own, so the table also gained an
`app.rs` pin, three `csrf.rs` pins and the step-3 span; the census is now 100 citations over 71
distinct spans in 15 live documents, 0 findings, plus the 61-citation snapshot line.

**Docs corrected.** The matrix's *CSRF protection* row moves NOT_IMPLEMENTED → VERIFIED with what the
layer issues, accepts and refuses; the status table reads **115 / 5 / 14 / 0 / 0 / 0 / 2 = 136**
(recounted from the rows); the narrative records that track 98b's **114 / 4 / 16** was arithmetic on
the wrong cell (it decremented `PARTIALLY_IMPLEMENTED`; the rows read 114 / 5 / 15) and then this
track's 115 / 5 / 14; the remaining-work list closes its CSRF item and names what stays open (no
session store to bind the token to); the suite claims move to 661 / 375 / 274 / 21. The
production-readiness report gets the doc's own supersession pattern on items 3 and 8 and the matrix's
recount in §3. `docs/09_api_reference.md` gains §7.5 with a compiling example and the operational
facts (shared secret across processes, exemption semantics, `Max-Age` from issuance);
`docs/04_security_and_rfc9112.md` gains §5 with the threat, the singleton rules and the stated limits.
Register B22 records the dated-record marker this file still carries.

**Recount.** 661 tests / 0 failed / 1 ignored over 56 `test result:` lines: **375 unit + 274
integration across 21 checked-in targets + 12 doc**, up from 631 by twenty `csrf.rs` unit tests, nine
`csrf` wire tests in a new target and one `enable_csrf` doc test.

**Gates on the final tree** (2026-10-29, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
661 passed / 0 failed / 1 ignored across 56 `test result:` lines; govtests 94 passed / 0 failed; cite
100 citations over 71 distinct spans in 15 live documents with 0 findings, plus 61 snapshot citations
marked and not validated; evidence 33 documents / 0 findings; validate exit 1 with exactly the four
pinned findings (PROB-0007 twice, PROB-0017 twice, REQ-0001 three times, REQ-0001 open under a past
ID) and nothing else. Nothing pushed.

## Track 99 — a body that is still arriving, and a file the disk keeps feeding (2026-10-29)

Roadmap §19 row 99: F-09 (streaming response bodies and chunked responses) and F-10 (a streamed
file answer). Row 99 closes with this track. Streaming *request* bodies stay open for row 100, and
the file answer is a 64 KiB read loop rather than a literal `sendfile(2)` call, which the matrix
row says in those words.

**Decisions.**

- **Pull, not push.** `BodySource::next_chunk` is asked for one chunk, the writer sends it, and
  only then is the next one asked for. A pushed channel would let a fast producer buffer the whole
  body inside the framework — the exact memory a streaming API exists to avoid. What a streamed
  response holds is one chunk plus whatever the socket or the h2 send buffer has not drained.
- **The head decides the framing, once.** `BodyFraming` is computed before the status line is
  written: `ContentLength` when the producer declared a length, `Chunked` for an undeclared
  HTTP/1.1 stream, `UntilClose` plus `Connection: close` for an HTTP/1.0 one (RFC 9112 §6.3, since
  1.0 has no transfer coding), and `None` for 1xx/204/304. After the head is on the wire no path
  renegotiates it.
- **A failed body ends the connection, not the message.** A source error, a file that shrank, a
  stream that overruns its declared length: each sends what it already produced and closes
  without the chunked terminator (HTTP/1) or resets the stream with `CANCEL` (HTTP/2). A client
  that was told a length, or given a chunked body, cannot be told mid-body that the framing is
  wrong; a terminator after a short body is the one lie the format cannot detect.
- **An empty chunk is a terminator, never data** (RFC 9112 §7.1), so the writers skip it. A
  source that answers empty without ever producing a byte would spin the task, so
  `MAX_EMPTY_CHUNKS = 1024` consecutive empty polls end the body exactly as an error does.
- **HEAD and body-forbidding statuses never poll the source.** The head still carries the GET's
  framing (a declared `Content-Length: 6` stream), and a `204` with an attached stream sends
  neither a length nor a coding. Both are asserted on the wire, and the HEAD test counts the
  source's polls at zero.
- **Priced one chunk at a time, on both emitters.** The scheduler's budget sees the block a chunk
  holds, not the length the stream promises: h2 sets the stream's registration to the chunk's
  block and clears it the moment the chunk is queued; HTTP/1 releases the write buffer to the
  chunk it now holds (`release_stream_write_buf`). A producer that pauses between chunks holds no
  budget while it pauses.
- **Compression declines a stream.** `CompressionMiddleware` returned early only for a
  body-forbidding status; it now also returns on `is_streaming()`, because the `body` it would
  compress is empty while the payload is still to come, and naming a coding for bytes the
  middleware never saw is the same defect the new h2 test found in the head writer.
- **The file answer is a metadata plan, not a read.** `plan_file` decides status, length, range
  and validator from `metadata`, so a `HEAD`, a `304` or a `416` never opens the payload, and a
  large file costs one 64 KiB `FileBody` read per write. A range seeks to its offset and reads
  only its slice.
- **The validator is weak on purpose.** Size and mtime do not prove byte equality, so the ETag is
  `W/"<len hex>-<mtime nanos hex>"` and `If-None-Match` compares weakly. `If-Range` accepts the
  published `Last-Modified` date — which this origin itself issued — but never the weak tag (RFC
  9110 §13.1.5), so a range request that carries the weak ETag gets the full `200`.

**Evidence.** A new integration target, `streaming.rs`, holds eleven wire tests: an undeclared
stream is chunked and the socket stays usable afterwards (the empty chunk the source produced was
skipped and the terminator is on the wire); a declared-length stream sends exactly that many bytes;
a source error sends what it produced and no terminator; a stream that ends early of its declared
length closes the connection; one that overruns is cut at the boundary; `HEAD` never polls the
source and still names the length; an HTTP/1.0 request gets a close-delimited body with no framing
field; a `204` sends none of an attached stream and leaves the connection in sync; a 300 KiB file
is streamed whole and by range; a streamed file revalidates weakly (304 on its ETag), refuses a
range a weak validator would authorise, and resumes when `If-Range` carries the published date; an
unsatisfiable range is `416` with `bytes */10`. Unit tests: five in `response.rs` (the chunk wire
format; declared-length framing; chunked on 1.1 and close-delimited on 1.0; a 204 ignores a
stream; a HEAD writes the framing and no payload), three in `static_files.rs` (the metadata
validator is weak; a planned answer publishes it with the length; a weak ETag fails `If-Range`
where the published date passes), and one in `static_dir.rs` (64 KiB reads, an offset start, an
over-long plan reporting once, a missing file erroring). Three `http2_e2e` tests cover the DATA
path: an undeclared stream arrives whole with no framing field, a declared length is the length
the peer is told, and a HEAD never polls the source. Three defects were found by writing these
tests, not by reading the code: an undeclared h2 stream advertised `content-length: 0` and then
sent bytes (the head writer measured the empty `body` instead of the declaration); `FileBody`
re-reported its truncation on every poll, so a drain loop never terminated (the error arms now
spend the source); and the e2e `If-Range` case asserted only the body, which a `206` of the whole
file also satisfies — it now asserts the status, and a date `If-Range` case proves the other
direction.

**Pins.** `app.rs` grew 212 lines (the stream writer, the empty-poll guard, the buffer release and
the `static_file` streaming route), `http2.rs` 174 (framing, `write_stream`, `price_stream_chunk`),
`static_dir.rs` 179 (`FileBody`, the plan, the weak validator), `response.rs` 291 (the body
types and the framing writer) and `static_files.rs` 282 (the plan and the two `If-Range`
comparisons). The formatter reflowed several of those additions after the first re-derivation, so
the sweep ran twice; the second pass was anchored on the checker's own report. Three statements
repeat verbatim in `app.rs` (`err_res.headers.push(("Connection", "close"...` and
`err_res.write_http1_to(&mut write_buf);`), and a 1-of-3 pattern hit is not evidence — the same
lesson track 98c recorded — so those three pins were moved by the coordinate the document
actually means, not by a search. Census: 100 citations over 71 distinct spans in 15 live documents,
0 findings, plus the 61-citation snapshot line.

**Docs corrected.** The matrix's *Response body streaming* row and its *Streaming / `sendfile`*
row move NOT_IMPLEMENTED → VERIFIED with what each actually delivers; *Bounded response memory*
learns the per-chunk pricing, *Strong ETag* names the weak metadata validator and why, and
*Range requests* states the `If-Range` rule. The status table reads **117 / 5 / 12 / 0 / 0 / 0 / 2
= 136** (recounted from the rows) and the narrative records that response streaming and static-file
`sendfile` left the remaining-work list with this track; only streaming *request* bodies and the
literal `sendfile(2)` syscall stay there. `framework/README.md` replaces a "not implemented" list
that still named the middleware API, auth and directory listings — three things that had shipped —
with the six that are still true, and rewrites the static-files section around the streamed reads
and the validator. `docs/09_api_reference.md` gains the Arabic *Response streaming* section, and
the teaching docs' citations were re-derived to the new coordinates.

**Recount.** 684 tests / 0 failed / 1 ignored over 57 `test result:` lines: **384 unit across 20
binaries + 288 integration across 22 targets + 12 doc tests over 15 doc runs**, up from 661 by
nine unit tests and fourteen integration tests, and one new integration target (`streaming`).

**Gates on the final tree** (2026-10-29, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
684 passed / 0 failed / 1 ignored across 57 `test result:` lines; govtests 94 passed / 0 failed;
cite 100 citations over 71 distinct spans in 15 live documents with 0 findings, plus 61 snapshot
citations marked and not validated; evidence 33 documents / 0 findings; validate exit 1 with
exactly the four pinned findings (PROB-0007 twice, PROB-0017 twice, REQ-0001 three times, REQ-0001
open under a past ID) and nothing else. Nothing pushed.

## Track 100 — a body the handler starts reading before it lands (2026-10-29)

Roadmap §19 row 100: F-11, streaming request bodies. The row closes for HTTP/1.1. The HTTP/2
frontend still collects DATA before it dispatches, so the matrix row moves
`NOT_IMPLEMENTED` → `PARTIALLY_IMPLEMENTED` and says so in those words; the h2 half is track 100b,
registered below rather than folded into this record.

**Decisions.**

- **Opt-in per route, keyed by handler id.** `AetherApp::route_stream` marks a registration's id in
  `ServiceCore.streaming_routes`; the HTTP/1 loop re-parses a head only when that set is non-empty
  (`has_streaming_routes`) and dispatches from it only when `streams_route(method, path)` names the
  routed handler. An app that registers none keeps the buffered loop exactly as it was — no
  second parse, no second head lifetime.
- **Dispatch before the body, not after it.** The routing decision is made from the parsed head;
  the handler is spawned with an empty `body` and a lane, so an upload whose handler refuses at
  byte 10 never pays for bytes 10..N.
- **Pull, one slot.** `request_body::lane()` gives the handler a `RequestBody` and the pump a
  `BodyWriter` over one shared slot. The writer polls `poll_room` before decoding another chunk and
  pushes exactly one; the reader takes it and wakes the writer. Memory is one chunk plus the
  connection's own buffer — the same shape as the streamed *response* lane, in the other direction.
- **One decoder, two framings.** A `Content-Length` body drains `conn_buf` directly; a chunked one
  feeds the same `ChunkedBodyDecoder` the buffered path uses, through a new `stream_feed` that
  appends decoded bytes and reports how many wire bytes it consumed, so nothing about chunk
  grammar is re-implemented for streaming.
- **Two ceilings, two owners.** `max_stream_body_bytes` is the transfer ceiling: a declared length
  over it is refused `413` **before dispatch**, because the handler exists for bodies that size and
  saying so afterwards would mean having already paid for a body the rule never allows. A chunked
  body that crosses it mid-stream fails the *read* (`BodyError::TooLarge`) and the connection closes
  after whatever the handler answers — the framework cannot answer for a handler that has not
  returned, and it will not pretend the body was framed when it was not.
- **The receive deadline, not the handler deadline.** `receive_timeout` counted from the head bounds
  the body; expiry fails the lane with `BodyError::Timeout` and leaves the handler to answer
  (the test maps it to `408`). Aborting the handler would make the deadline the frontend's decision
  to make a status, which it is not.
- **The socket's fate is decided after the answer.** `settle()` settles what is already buffered
  without touching the socket: a body that finished its framing on its own terms keeps the
  connection; a body that failed, or one that never finished, closes it — response, then FIN, then
  a bounded discard (`discard_after_close`) so the peer's remaining bytes are read and dropped
  instead of triggering an RST on Windows (the hazard already recorded on `refuse_over_budget`).
  A failed body is **never** reusable: its unread tail is still on the wire, and reading it as the
  next request is request smuggling.
- **Priced chunk by chunk.** `check_budget` = the connection's buffered charge + the bytes the lane
  currently holds (`BodyWriter::held`) + the decoder's capacity, re-priced at every chunk through
  the same `reprice_conn_memory` the buffered path uses. The charge follows the bytes, not the
  declared length.
- **`100 Continue` before the body, from the streaming branch.** A streamed request that expects it
  gets the interim response written before the pump starts, on the same `write_timeout` as every
  other write.
- **One response writer for both paths.** The buffered loop's response block became
  `write_http1_response(..., close_after_response) -> Option<Answered>`; the streamed branch and the
  buffered branch both call it. Duplicating ~250 lines of framing, HEAD handling and pricing would
  have been the obvious way to ship this and the obvious place for the two paths to drift.

**Defects found while testing, not while reading.**

- The lane's terminal state was *consumed* by the first read, so a writer could still push after a
  failure — the lane's own test `a_failure_is_reported_once_and_ends_the_body` failed. `poll_chunk`
  now reports the error without taking it, and the terminal state outlives every report.
- `settle()` first answered "reusable unless the peer is gone", which would have made a body that
  failed mid-framing leave its tail to be parsed as the next request. `ended_clean` is now set only
  by a body that reached its framing terminator.
- The dispatch loop's body-pending flag had to become an `AtomicBool`: as a `Cell<bool>` it was not
  `Sync`, and the spawned handler's `Send` bound failed through the h2 frontend's spawn site.

**Evidence.** Eight socket tests in `framework/tests/tests/streaming_request.rs`: the handler runs
before a 200 000-byte body is sent and reports an empty `body`
(`a_streamed_route_is_dispatched_before_its_body_arrives`, the test blocks on a `Notify` the handler
fires, so a buffering frontend deadlocks instead of passing); a chunked body delivered in 7 / 1000 /
16 384-byte chunks decodes to the same bytes (`a_chunked_body_reaches_the_handler_decoded`); a
declared length over the ceiling is `413` with `Connection: close` and the handler counter stays 0
(`a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch`); a chunked body over the
ceiling fails the read and closes (`a_body_past_the_ceiling_mid_stream_fails_the_read_and_closes`);
an unread 64 KiB body answers and closes (`an_unread_body_closes_the_connection_after_the_response`);
a 512-byte body that arrived with the head settles and the connection carries the next request
(`a_small_early_answered_body_keeps_the_connection_alive`); a body that stops arriving fails the
read as `Timeout` inside `max_receive_time` (`a_body_that_stops_arriving_times_out_for_the_handler`);
and `Expect: 100-continue` is answered before the handler's body
(`a_streaming_route_answers_100_continue_before_the_body`). Five lane unit tests in
`request_body.rs` (`a_chunk_handed_over_before_the_handler_asks_is_delivered_once`,
`the_lane_holds_one_chunk_and_reopens_when_it_is_taken`,
`a_failure_is_reported_once_and_ends_the_body`, `the_pump_cannot_run_ahead_of_the_reader`,
`read_to_end_refuses_a_body_past_the_cap`).

**Sensitivity.** Each test was falsified by a mutation that must make it fail, run and then
reverted: registering `route_stream` as buffered killed six of eight (everything except the chunked
decode and the keep-alive case, which are behaviour the buffered path also owes); `settle()`
returning `!gone` for a finished body killed the mid-stream-ceiling and timeout tests; disabling the
`413` pre-dispatch killed the ceiling test at its status assertion; delivering raw `conn_buf` bytes
instead of the decoded ones killed the chunked test at its checksum; a no-op `expire_body` killed
the deadline test on its 5 s read; and `settle()` answering false for every finished body killed the
two keep-alive assertions. The mutation that survived (`settle` false only on the already-settled
`Content-Length` branch) is the one that showed the pump settles bodies during the handler's own
await, which is why the finished-branch mutation was the one that mattered.

**Pins.** `app.rs` +693/−295 (the streaming branch, `parts_from_view`, the extracted
`write_http1_response` and `discard_after_close`), `serve.rs` +115/−15 (the dispatch loop's body
deadline), `parser.rs` +136/−30 (`parse_request_head`, `is_complete`, `stream_feed`), `limits.rs`
+17, `config.rs` +23/−1, `http2.rs` +3, `lib.rs` +4. New files: `body_pump.rs` 372 lines,
`request_body.rs` 320, `streaming_request.rs` 521. The design note is
`notes/2026-10-29-track-100-streaming-request-body-design.md`.

**Docs corrected.** The matrix's *Streaming request bodies* row moves to
`PARTIALLY_IMPLEMENTED` with the mechanism, the eight tests and the h2 limit named; item 6 of its
remaining-work list becomes "HTTP/2 request-body streaming", and the *Response body streaming* row
no longer says request bodies are not implemented. The PRR's remaining-work item 3 is struck through
and says what remains (h2 and `Json`/`Form`), its status table reads **117 / 6 / 11 / 0 / 0 / 0 / 2
= 136** recounted from the rows, and the not-delivered paragraph moves its counts and its wording.
`framework/README.md` gains a feature bullet and a *Streaming request bodies* section, and its
"not implemented" list now names the h2 half instead of the feature.
`AETHER_CORE_CAPABILITIES.md` gains the smuggling rule as a protected invariant with its tests.

**Recount.** 697 tests / 0 failed / 1 ignored over 58 `test result:` lines: **389 unit across 20
binaries + 296 integration across 23 targets + 12 doc tests over 15 doc runs**, up from 684 by the
five lane unit tests and the eight socket tests, and one new integration target
(`streaming_request`).

**Settling the tree.** The seven-gate ladder went red on `govtests` and `cite` before it went green:
track 100's own insertions moved 31 pinned spans across `app.rs`, `serve.rs`, `parser.rs` and
`http2.rs`. The sweep re-derived every span the checker could place uniquely, then read the six it
could not decide (three duplicate candidates, one range whose end moved 200 lines, the struct that
grew a field, and the handler-timeout arm, whose `Err(_) =>` became `None =>`). Two of those reads
found prose that had stopped being true, not just stale numbers: the per-request-version writer's
second call site is the pre-upgrade `503` refusal, not a "normal response" (`docs/05`), and the loop
excerpt in `docs/02` now passes through the `route_stream` head branch, which the note there says in
one sentence. `docs/05`'s `RequestContext` excerpt also gains the `body_stream` field the struct
grew. One code change came out of the sweep: the streaming `413` was the only refusal writing with
the request's version (`write_http1_to_for_request_version`), while every other refusal exit writes
`write_http1_to`; it now matches them, so the documented `HTTP/1.1`-on-refusal rule has no exception.
Census after the sweep: 101 citations over 71 distinct spans in 15 live documents, 0 findings, plus
the 61-citation snapshot line.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
697 passed / 0 failed / 1 ignored across 58 `test result:` lines; govtests 94 passed / 0 failed;
cite 101 citations over 71 distinct spans in 15 live documents with 0 findings, plus 61 snapshot
citations marked and not validated; evidence 33 documents / 0 findings; validate exit 1 with exactly
the four pinned findings (PROB-0007 twice, PROB-0017 twice, REQ-0001 three times, REQ-0001 open
under a past ID) and nothing else. Nothing pushed.

## Track 100b — the HTTP/2 handler is handed its body while it arrives (2026-10-30)

Roadmap §19 row 100b closes F-11's second half and the F-11 row itself. The HTTP/2 frontend no
longer collects DATA before dispatch: `serve_request` resolves the route from the parsed head and,
when `core.streams_route` names a `route_stream` registration, hands the stream to
`serve_streamed` instead of the buffered read below it. Every other route keeps that buffered path
byte for byte, so an app that registers no streaming route is untouched on both frontends.

**Decisions.**

- **The head decides, exactly as it does on HTTP/1.1.** Route, header maps, declared length and
  the lane are all settled before the first DATA frame; the handler is spawned with an empty
  `body` and `body_stream: Some(lane)`. A `Content-Length` over `max_stream_body_bytes` is `413`
  before dispatch for the reason track 100 recorded: the handler exists for bodies that size, and
  refusing afterwards would mean having already paid for a body the rule never allows. Header
  maps over the line budget are `431` on the same footing.
- **A second pump behind the same trait, not a second lane.** `Http2Body<'a>` implements
  `PeerWatch`, the interface the HTTP/1 dispatcher already polls, so the core does not learn that
  HTTP/2 exists: `poll_body` is one DATA frame, `poll_gone` is `response.poll_reset`, the body
  deadline is `receive_timeout` counted from the head, and `expire_body` fails the lane with
  `BodyError::Timeout` so the *handler* answers `408` (the test sends its own body to prove which
  party answered).
- **One frame per poll, after the lane has room.** `step` asks `BodyWriter::poll_room` before it
  asks `h2` for data, pushes exactly one frame, and stops. Backpressure is therefore the stream's
  flow-control window rather than a queue here: a handler that stops reading stops crediting the
  peer, and memory is one frame plus the window, never the body.
- **`release_capacity` per frame.** HTTP/2's receive window is the application's to credit, and a
  pump that never does reads one window (65,535 bytes by default) and then waits forever for
  bytes flow control forbids. This is the same defect the buffered path's `read_body_buffer`
  records, met again in a new place; the receive-windows test crosses four windows for that
  reason.
- **One ceiling on the whole body.** `max_stream_body_bytes` bounds cumulative payload across
  frames, not one frame; crossing it mid-stream fails the *read* with `BodyError::TooLarge` and
  the handler answers. An empty DATA frame carries no payload and no capacity to return, so it is
  skipped rather than counted.
- **Priced per frame.** `StreamCharge::buffer_set(writer.held())` re-sets the stream's
  registration to its base block plus what the lane holds, on every frame, through the same
  `Layer::adjust_memory` the buffered path uses. `buffer_set` adds `STREAM_BASE_MEMORY` itself, so
  the call states the variable half only.
- **The tail is abandoned by dropping the `RecvStream`.** A handler that answers without reading
  leaves DATA on the stream; on HTTP/2 that costs one stream, never the connection, because `h2`
  ignores DATA for a stream nobody reads and returns its capacity. The response is sent *after*
  the pump is dropped, so the abandoned stream cannot race the answer.
- **`RST_STREAM(NO_ERROR)` is deliberately not sent, and that is measured.** RFC 9113 §8.1's own
  way to end an answered request is a reset; `h2` implements a reset by dropping the stream's
  queued frames, so a reset issued just after the response is queued destroys the response it was
  meant to follow. Reintroducing it turned every early answer here into a client-side `Reset`
  error, which is what mutation M5 reproduces below. The note at the send site carries the
  measurement so the next reader does not "fix" it back.
- **A reset stream still cancels its handler.** `poll_reset` keeps the behaviour tracks 85 and 87
  bought: when the peer resets, `ServeOutcome::ClientGone` returns before anything is written. A
  DATA read error sets `gone` and fails the lane with `BodyError::Aborted`, so the handler stops
  instead of waiting on a body that cannot arrive.
- **The WebSocket arm stays unreachable by construction.** `allows_upgrade: false` makes the core
  answer `400` for an upgrade route, and the streamed branch keeps the buffered branch's `500`
  guard in case the core changes underneath this frontend.

**Defects found while testing, not while reading.**

- The first run of the dispatch mutation left three of the six tests green. They asserted output
  that the buffered path also produces — a 200, the body's checksum, a 408 — so they proved the
  bytes arrived, not that they arrived *while* the handler waited. Each now asserts something
  only the streamed path can print: the handler fires its `Notify` before any DATA is sent (a
  buffering frontend deadlocks instead of passing and prints `buffered=0`), the receive-windows
  answer carries `lane=true bytes=262144 sum=…`, the unread-body answer is `done-streamed`, and
  the timeout answer is the handler's own `{"by":"handler"}` body rather than a framework 408.
- The `serve_streamed` doc comment claimed the reset was sent while the code (correctly) did not.
  Prose ahead of the implementation is the failure mode the citation gate exists for; the comment
  now names the drop and points at the send-site note.

**Evidence.** Six wire tests in `framework/tests/tests/http2_e2e.rs`, all against a real TLS/h2
client, plus a `checksum` helper so a corrupted or reordered body cannot pass on length alone:
`a_streamed_h2_route_is_dispatched_before_its_body_arrives` (the handler signals before 100 000
bytes exist and carries `buffered=0`; the connection serves `GET /fast` afterwards);
`a_streamed_h2_body_arrives_whole_across_receive_windows` (256 KiB in 16 KiB frames across four
default windows, asserted as `lane=true bytes=262144 sum=…`);
`a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch_over_h2` (handler counter
never moves); `an_h2_body_past_the_ceiling_mid_stream_fails_the_read` (no declared length, four
50-byte frames against a 64-byte ceiling, handler answers `413`, connection survives);
`an_unread_h2_body_answers_and_the_connection_stays_usable` (a 64 KiB body nobody reads;
`done-streamed`, then `GET /fast` on the same connection);
`a_streamed_h2_body_that_stops_arriving_times_out_for_the_handler` (three of ten declared bytes,
`max_receive_time` 300 ms, the handler's `{"by":"handler"}` 408 inside the test's own 5 s bound).

**Sensitivity.** Five mutations, each applied to the final tree, run against the six tests, and
reverted (the restored file is byte-identical to the pre-mutation blob), 2026-10-30:

| mutation | what it changes | tests it kills |
| --- | --- | --- |
| M1 | `streams_route` branch disabled (`if false &&`) — route served buffered | 6 of 6 |
| M2 | `release_capacity` never called | 2 (`…dispatched_before_its_body_arrives`, `…whole_across_receive_windows`) |
| M3 | `expire_body` a no-op — deadline ignored | 1 (`…stops_arriving_times_out_for_the_handler`) |
| M4 | the pre-dispatch `413` disabled | 1 (`…413_before_dispatch_over_h2`) |
| M5 | `RST_STREAM(NO_ERROR)` after the response | 5 (every test but the pre-dispatch `413`, which returns before the send site) |

M1 killing all six is the property the earlier pass lacked: a buffered frontend now fails every
test rather than half of them. M2's two are the bodies larger than one window plus the dispatch
test's 100 000 bytes; the remaining four send less than 65,535 bytes and cannot see the mutation.
The test commit's body (`5783bcc`) says "six mutations"; this table is the record and lists five —
the six is the number of tests M1 kills.

**Pins.** `http2.rs` +329/−1 (`serve_streamed`, `declared_length`, `Http2Body` with its
`PeerWatch` impl, the branch in `serve_request`), `http2_e2e.rs` +363/−0 (six tests and
`checksum`), `app.rs` +18/−13 (rustdoc only: `body_stream` on both `RequestParts` and
`RequestContext`, `read_body`'s h2 tail and the h2 lane, `route_stream`'s bullets). No public API
changed — `route_stream` already existed; this track only makes h2 honour it.

**Docs corrected.** The matrix's *Streaming request bodies* row moves to
`VERIFIED_IMPLEMENTED` with the h2 mechanism, the shared ceiling and the six tests named, and item
6 of its remaining-work list is struck through. The PRR's §3 table reads **118 / 5 / 11 / 0 / 0 / 0
/ 2 = 136** rows, recounted from the rows after the row flipped — the table and the paragraph
below it had disagreed by one row each (117/6/11 versus 117/5/12) and both now come from one run;
§9 item 3 says both frontends and names the six tests; §10's `http2_e2e` count moves 40 → 43 → 49.
The matrix's own *Status count* section had been left at 115 / 5 / 14 by tracks 99 and 100 while the
rows above it already read 117 / 5 / 12 and then 117 / 6 / 11 — the same section-versus-table drift
that section records for tracks 96 and 98b — so it is re-run from the rows here and reads
118 / 5 / 11 / 0 / 0 / 0 / 2 = 136, with the three missed moves named.
`framework/README.md`'s streaming section, its *not implemented* list and its per-target counts are
re-derived (73 / 49 / 11 / 8 / 1 / 19 / 41 / 57 / 18), and the root `README.md`'s verified line
moves to 703 tests with 73 raw-socket and 49 HTTP/2 wire tests. `AETHER_CORE_CAPABILITIES.md`'s
no-smuggling row gains the h2 clause and the three tests that pin it.

**Recount.** 703 tests / 0 failed / 1 ignored over 58 `test result:` lines: **389 unit across 20
binaries + 302 integration across 23 targets + 12 doc tests over 15 doc runs**, up from 697 by the
six wire tests. `http2_e2e` holds 49 tests, up from 43.

**Settling the tree.** The ladder went red on `fmt` first: three of this track's lines were not
`rustfmt`'s shape (the streamed call, the `is_some_and` ceiling condition, one closure). Then
`govtests` and `cite` went red. `govtests` was the index row naming `## Track 100b` before this
entry existed — the heading is the fix, not a waiver. `cite` reported 32 findings: +329 lines of
`http2.rs` moved its three pinned spans, and the `app.rs` rustdoc corrections moved 21 more spans
by +1 (the `RequestParts::body_stream` line), +3 (by line 1064) and +5 (by line 2102). Every span
the checker could place uniquely was re-derived from its suggested coordinate; the three it could
not decide were read against the code: both `00_framework_specification` languages cite the
`Connection: close` line that follows the 431/413/400 match (`2830` → `2835`), and `docs/05`'s
chain of two `write_http1_to` exits resolves to `2787` and `2837` — the two sites are the same
line of code in two branches, so only the order the document lists them in decides which is
which. Census after the sweep: 101
citations over 71 distinct spans in 15 live documents, 0 findings, plus the 61-citation snapshot
line.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
703 passed / 0 failed / 1 ignored across 58 `test result:` lines; govtests 94 passed / 0 failed;
cite 101 citations over 71 distinct spans in 15 live documents with 0 findings, plus 61 snapshot
citations marked and not validated; evidence 33 documents / 0 findings; validate exit 1 with exactly
the four pinned findings (PROB-0007 twice, PROB-0017 twice, REQ-0001 three times, REQ-0001 open
under a past ID) and nothing else. Nothing pushed.

## Track 101a — a revoked certificate is refused before it is served (2026-10-30)

Roadmap §19 row 101 (F-15) opens with the offline half. TLS termination loaded whatever chain the
operator supplied: `TlsCertSet::with_ocsp` attached a response nothing verified, no CRL path
existed, and the module doc, `DEPLOYMENT.md` and the matrix row said so in the same words. A
certificate the CA had revoked kept being served until someone noticed and removed it by hand.

**Decisions.**

- **Enforced at load and at reload, never per handshake.** `with_crl` runs once, at the point the
  operator already chooses what is served, and a reload builds a new set through the same call
  (`replace_certificates`), so a list cannot be installed once and then ignored by an acceptor that
  never asks again. A per-handshake check would instead re-parse an ASN.1 list on the hot path or
  cache one with no invalidation story.
- **The issuer is the second certificate in the chain, or the leaf itself when the set is
  self-signed.** Anything else is a guess about which key to trust; a chain that cannot name an
  issuer fails the signature check rather than passing an unverified list.
- **Signature, then window, then membership, in that order.** A tampered list must not be able to
  revoke (or bless) a serial, so nothing is read out of it until its signature verifies against the
  issuer's key.
- **Its own window is required and checked both ways**: `thisUpdate` not in the future, and a
  `nextUpdate` that exists and is still ahead. A list that never expires is a list nobody
  refreshes — the same failure as no list at all arriving late.
- **PEM or DER.** Every CA and every tool emits one or the other; `decode_pem_or_der` normalises
  before parsing, and bytes that are neither are named (`RevocationUnusable("the CRL did not
  parse")`) rather than skipped as if there were nothing to enforce.
- **Nothing is fetched.** An offline build and an offline deployment behave identically; refresh
  is the operator handing a fresh list to the same load or reload call. This is the dependency the
  roadmap row named, and it is resolved by refusing to have it.

**Dependencies.** `x509-parser` (with `verify`) was already in the build graph through `rcgen`;
naming it makes CRL parsing a declared dependency rather than a transitive accident, and `time` is
a dev-dependency so the tests build issue times relative to the clock instead of committing
fixtures that expire.

**Mutation check** (run on the completed tree, after 101b): five mutants, each disabling one rule,
each killed by a named test — signature checked against the leaf instead of the issuer (four tests
fail), future window ignored, stale window ignored, revoked serial ignored (two tests), and an
issuer that is always the leaf (four tests). `restored: True`, `SURVIVORS/BROKEN: none`.

**Tests.** `a_crl_that_lists_the_leaf_refuses_the_set`,
`a_current_crl_that_lists_other_serials_leaves_the_set_servable`,
`a_stale_or_not_yet_valid_crl_is_refused`,
`a_crl_signed_outside_the_chain_or_tampered_with_is_unusable`,
`the_crl_reads_as_pem_and_garbage_is_named`, `a_self_signed_set_is_checked_against_its_own_key`.
The second and the sixth close the other direction: a current list about other serials does not
block service, and a self-signed chain is checked against its own key instead of being refused for
lacking a CA.

**Scope.** Code and dependencies only. The documentation pass (module doc, `DEPLOYMENT.md`, the
matrix row, PRR §3/§9/§10, `README.md`, both Arabic guides) is in 101b, so one set of sentences
describes the final behaviour instead of being rewritten twice.

**Commit.** `439f2c2` — `tls.rs` +297/−3, `Cargo.toml` +8, `Cargo.lock` +2.

## Track 101b — a staple is checked before it can vouch for the chain (2026-10-30)

Row 101 closes here. `with_ocsp` had taken bytes and stored them in `CertifiedKey.ocsp` for any
client that sent `status_request`. RFC 6066 §8 lets such a client hard-fail on a stapled response
it rejects, so the old behaviour could turn a server that meant to say nothing into one that says
something false — and nothing in this build had ever read the bytes it attached.

**Decisions.**

- **A basic response that answered `successful`, or nothing.** The state is read first (ENUMERATED,
  compared to 0), then the response type (`1.3.6.1.5.5.7.48.1.1`), then the OCTET STRING body.
  `tryLater`, `internalError` and the rest are named from their RFC 6960 §2.3 values instead of
  being reported as a parse failure.
- **Coverage is recomputed, never compared to itself.** The `SingleResponse` must carry this
  leaf's serial and both issuer hashes, hashed from this chain with the CertId's own declared
  algorithm — SHA-1 and SHA-256 are the two this build computes, anything else is refused. A
  response whose only entry is for another certificate is refused by the `covered` guard, not
  accepted because *a* status was present.
- **`good` only.** `revoked` is `CertificateRevoked` (the same variant the CRL path returns, so a
  caller does not have to know which source refused it); `unknown` is `RevocationUnusable`,
  because a responder that does not know is not evidence that the certificate is fine.
- **A window in both directions, with `nextUpdate` required.** `thisUpdate` may not be in the
  future, `nextUpdate` must exist and be ahead. An open-ended `SingleResponse` is refused for the
  reason the CRL path refuses a list without an expiry: a staple nobody has to refresh is one
  nobody notices going stale.
- **The signer is whichever `ResponderID` names — and only then is its key used.** If the ID names
  the issuer by key (SHA-1 of the SPKI, the algorithm RFC 6960 leaves unnamed and the field settled
  on) or by name, the issuer's key verifies the signature. If it names anyone else, that
  certificate must be included in the response, signed by this chain's issuer, and carry
  `id-kp-OCSPSigning`; otherwise the response is refused rather than checked against the issuer's
  key. Verifying the signature first and reading the identity afterwards would accept any
  issuer-signed blob that *claims* to be a responder.
- **The signature is verified over the exact bytes that were parsed.** `DerReader::element` returns
  each element's own encoding alongside its content, so `tbsResponseData` is verified as received;
  a re-encoding is a different value.
- **Every field states its tag before its content is read**, and every time field must be a
  GeneralizedTime — RFC 6960 §4.2.2.1's `YYYYMMDDHHMMSSZ`, not a UTCTime that happens to parse.
- **Refused at load, never sent, and a bad reload leaves the live set serving.** The staple reaches
  `CertifiedKey.ocsp` only after every check above passes. The track-86 rule for certificates
  applies to their revocation material too: a failed reload must not half-apply.

**Defects found while testing.**

- **A fixture defect, repeated across the first OCSP tests:** they failed with "the OCSP producedAt
  is not a time". `now()` kept nanoseconds, yasna wrote nine fractional digits, and asn1-rs
  accepts at most four. RFC 6960 mandates `YYYYMMDDHHMMSSZ`, so the fixture now truncates to whole
  seconds — the test data was wrong, not the rule.
- **`asn1-rs`' `Any::as_oid()` does not check the tag — a real hole.** Reading the CertId's
  `AlgorithmIdentifier` SEQUENCE (`06 05 2b 0e 03 02 1a 05 00`) through `as_oid()` returned OID
  `0.6.5.43.14.3.2.26.5.0`, because the accessor reinterprets whatever bytes it is handed instead
  of verifying the tag first. Every field read now goes through `check_tag`; a SEQUENCE sitting
  where an INTEGER belongs would otherwise have been read as an integer.
- **The CertId's hash algorithm is an `AlgorithmIdentifier`, not an `OID`.** The first fix decoded
  the wrapper's first element, which is a SEQUENCE; the hash is `AlgorithmIdentifier::from_der(…)
  .1.algorithm`, and that is what `ocsp_hash` matches on.
- **A note for the next reader:** the `as_*` accessors return values borrowing `&self`, so they
  cannot be returned alongside the input slice's lifetime. Three attempts at a generic `typed()`
  helper failed to borrow-check; per-field `check_tag` followed by an inline `as_*` both compiles
  and keeps each refusal's message specific.

**Mutation matrix.** Thirteen mutants, each disabling one rule; every one was killed by a named
test.

| mutant | killed by |
| --- | --- |
| M1 signature not verified | `a_forged_or_tampered_signature_is_unusable` |
| M2 leaf serial not matched | `a_staple_that_does_not_name_this_certificate_does_not_cover_it` |
| M3 issuer hashes not matched | `a_staple_that_does_not_name_this_certificate_does_not_cover_it` |
| M4 stale staple accepted | `a_stale_or_not_yet_valid_staple_is_refused` |
| M5 future `thisUpdate` accepted | `a_stale_or_not_yet_valid_staple_is_refused` |
| M6 revoked read as good | `a_staple_that_reports_revocation_or_unknown_refuses_the_set` |
| M7 responder EKU not required | `a_delegated_responder_must_be_authorised_by_this_chain` |
| M8 responder certificate signature not checked | `a_delegated_responder_must_be_authorised_by_this_chain` |
| M9 non-successful response accepted | `a_non_successful_responder_answer_is_named` |
| M10 responder identity ignored | `a_delegated_responder_must_be_authorised_by_this_chain` |
| M11 open-ended staple accepted | `a_stale_or_not_yet_valid_staple_is_refused` |
| M12 coverage guard removed | `a_staple_that_does_not_name_this_certificate_does_not_cover_it` |
| M13 time type not enforced | `a_utc_time_where_a_generalized_time_belongs_is_refused` |

**Tests.** Nine: `a_current_staple_that_clears_the_leaf_is_attached`,
`a_staple_that_does_not_name_this_certificate_does_not_cover_it`,
`a_staple_that_reports_revocation_or_unknown_refuses_the_set`,
`a_stale_or_not_yet_valid_staple_is_refused`,
`a_utc_time_where_a_generalized_time_belongs_is_refused`,
`a_forged_or_tampered_signature_is_unusable`,
`a_delegated_responder_must_be_authorised_by_this_chain`,
`a_non_successful_responder_answer_is_named`,
`garbage_is_named_as_unusable_revocation_material`. Each drives one rule and names the refusal it
expects, so the assertion is on the error, not on "something failed".

**Documentation.** The module doc now says what `with_ocsp` and `with_crl` enforce and that nothing
is fetched; `DEPLOYMENT.md`'s revocation bullet is rewritten (signature/window/serial for a CRL,
successful+coverage+window+signer for a staple, refusal at load, operator reload, and the fact that
a client sees a staple only when it sent `status_request`); PRR §3's counts move to
119 / 4 / 11 / 0 / 0 / 0 / 2, §9 item 8 is delivered and the Open list is 7/9/10, §10's history
paragraph says which row moved, and security gap 1 is rewritten; the matrix's *OCSP stapling /
revocation* row flips to `VERIFIED_IMPLEMENTED` with the mechanism, the 15 revocation tests and the
non-goal (fetching) named, and the TLS row's test count becomes 25 (15 revocation); `README.md`'s
*not implemented* list now names only fetching, and both Arabic guides carry the enforced-offline
behaviour; the TLS evidence line in `FULL_FRAMEWORK_DOCUMENTATION_AR.md` is re-derived (25 unit
tests, 15 of them revocation; 11 wire tests) and both `README.md` files move to 718 tests / 404 unit
on the track-101 tree; register item C is closed with the residual limit.

**Recount.** 718 tests / 0 failed / 1 ignored over 58 `test result:` lines — 703 from 100b, +6 in
101a, +9 here. `tls.rs` holds 25 unit tests, 15 of them revocation (six CRL, nine OCSP).

**Settling the tree.** `cite` reported 0 findings without a renumbering pass: no pinned span
points into `tls.rs`, so 830 added lines moved nothing (101 citations over 71 spans in 15 live
documents, plus 61 snapshot citations). `govtests` needed the two index rows to name headings that
exist — they are added with these sections.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
718 passed / 0 failed / 1 ignored across 58 `test result:` lines; govtests 94 passed / 0 failed;
cite 101 citations over 71 distinct spans in 15 live documents with 0 findings, plus 61 snapshot
citations marked and not validated; evidence 33 documents / 0 findings; validate exit 1 with
exactly the four pinned findings (PROB-0007 twice, PROB-0017 twice, REQ-0001 three times, REQ-0001
open under a past ID) and nothing else. Nothing pushed.

## Track 102a — eight properties, and the eleven mutants that had to break them (2026-10-30)

Row 102 opens here with F-16. Every parser and router test in the suite is an example: it names an
input the author thought of and checks the answer. A property quantifies over inputs instead, and
`proptest` shrinks a failing case to something small enough to keep in the test file. That makes the
suite's own sensitivity a measurable thing rather than an impression: the properties were written,
then the code was broken on purpose eleven times, and each break had to be named by a property.

**Decisions.**

- **New integration targets, not `#[cfg(test)]` modules.** `http/tests/parser_properties.rs` and
  `router/tests/router_properties.rs` drive only the public surface (`RequestView::parse`,
  `parse_request`, `parse_request_head`, `awaits_100_continue`, `ChunkedBodyDecoder`, `RadixRouter`),
  so they pin the exported API as well as the behaviour. `proptest` 1.12 is a workspace dev-dependency;
  no runtime crate depends on it.
- **Both parser entry points on every property that can carry them.** `RequestView::parse` is the
  Content-Length-only path embedded callers and the fuzz target use; `parse_request` is what the
  runtime calls. The first mutation run is why: two mutants of the *buffered* path survived in the
  first draft, because the round-trip and the prefix property only drove `parse_request`. The
  properties now drive both, and the two mutants were re-run against the strengthened property.
- **A zero-copy oracle, not only "no panic".** The arbitrary-bytes property asserts the consumed
  length is inside the input, that `split_at` re-joins `path` and `query_str` into the request
  target, and — when a body is present — that it is a borrow *inside the caller's buffer*. Without
  that last assertion a parser that copied the body would pass a "did it parse" oracle unseen.
- **The router property makes the shared parameter level hostile.** A radix level is shared by
  siblings, so `/api/{zz<name>}…/tail` is registered *first*: the parameter nodes then carry the
  sibling's names, and only `resolve_keys` restamping from the answering leaf can produce the
  target route's names. The static-versus-parameter property registers the parameter route first
  and disambiguates its generated token by appending `~`, so precedence cannot come from insertion
  order or from an accident of the alphabet.
- **`failure_persistence: None` in every `ProptestConfig`.** Proptest otherwise writes a
  `.proptest-regressions` file beside the source (in an integration test it warns that it cannot
  find `lib.rs`). A passing run should not leave untracked files in the tree, and the mutation
  driver — which runs the test binaries between a patch and its revert — must not either.

**Mutation matrix.** Eleven mutants, one rule each; every one was killed by a named property. The
driver patches the source, runs the two test binaries with `--test-threads=1`, restores the file and
verifies its hash (writes are retried: Windows can refuse a write while a test binary still holds
the file).

| mutant | killed by |
| --- | --- |
| P1 premature completion: clamp the required length to what arrived | `every_proper_prefix_of_a_request_is_incomplete` |
| P1b same, in the incremental path | `every_proper_prefix_of_a_request_is_incomplete` |
| P2 consumed length: report the head length instead of head+body | `a_valid_request_round_trips_through_the_parser` |
| P2b same, in the incremental path | `a_valid_request_round_trips_through_the_parser` |
| P3 query split: never split the request target at `?` | `a_valid_request_round_trips_through_the_parser` |
| P4 header collection: drop the first field line | `a_chunked_body_decodes_the_same_however_its_wire_is_split`, `a_valid_request_round_trips_through_the_parser`, `every_proper_prefix_of_a_request_is_incomplete` |
| P5 chunk payload: copy every buffered byte instead of the chunk remainder | `a_chunked_body_decodes_the_same_however_its_wire_is_split` |
| R1 param names: never restamp captures with the matched route's names | `declared_param_names_extract_their_segments_in_order` |
| R2 precedence: try single-segment parameters before static children | `a_static_segment_beats_a_parameter_segment` |
| R3 405 bitmask: overwrite the allowed set instead of unioning branches | `an_unserved_method_on_an_existing_path_is_405_with_the_union` |
| R4 404 vs 405: report `NotFound` for a path that exists under another method | `an_unserved_method_on_an_existing_path_is_405_with_the_union` |

**What the first run taught.** Two mutants survived, and neither was a test that asserted the wrong
thing — both were a property that never reached the code being broken. The fix was to strengthen
the properties (drive both entry points) and to add the two incremental-path mutants (P1b, P2b), so
the pair of paths is now covered on both sides. That is the rule this track leaves behind: a
surviving mutant is evidence about the *tests* first, and only a reason to doubt the code second.

**Tests.** Eight, all of them properties rather than examples:
`an_arbitrary_byte_sequence_never_panics`, `a_valid_request_round_trips_through_the_parser`,
`every_proper_prefix_of_a_request_is_incomplete`,
`a_chunked_body_decodes_the_same_however_its_wire_is_split`,
`a_registered_static_route_matches_its_own_path`,
`declared_param_names_extract_their_segments_in_order`,
`a_static_segment_beats_a_parameter_segment`,
`an_unserved_method_on_an_existing_path_is_405_with_the_union`. The arbitrary-bytes property runs
512 cases (each up to 4 KiB); the others use proptest's default 256 and generate from the same
grammar the parser accepts, so a failure is a request, not noise.

**Recount.** 718 → 726: eight integration tests in two new targets (25 checked-in targets now,
`parser_properties.rs` 4 and `router_properties.rs` 4). The unit split is unchanged.

**Documentation.** The matrix's *Property-based tests* row flips to `VERIFIED_IMPLEMENTED` with the
eight properties named and the eleven mutants cited; both `README.md` files carry the eight
properties in the suite sentence, and the matrix status section and PRR §3 move together.

**Settling the tree.** No pinned span points into either new test file, so `cite` has nothing to
re-pin; `govtests` needs the index row to name a heading that exists — this one and track 102b's are
added with these sections.

## Track 102b — a fuzz run whose oracle was proved to fire (2026-10-30)

Row 102 closes here with F-17. The matrix had been honest that the fuzz target compiled and had never
been run, and PRR §9 item 10 said the same. `cargo-fuzz` needs a nightly toolchain and libFuzzer's
sanitizer runtime; this host is stable Windows MSVC, where neither exists. The choice was between a
run that cannot happen here and one whose limits are written down.

**Decisions.**

- **An in-repo runner, with its limits in the module doc rather than implied.** `framework/fuzz`
  gains a library (`src/lib.rs`) whose `exercise` calls every public parse entry point on a byte
  string and asserts the parser's own framing invariants; the binary is a thin CLI around
  `campaign`. The doc states both limits: there is no coverage instrumentation (a "bucket" is an
  outcome hash, so this is outcome-guided, not coverage-guided), and a panic without memory
  corruption is what this catches — unsafe code and allocator misuse still need ASan.
- **Deterministic by seed.** SplitMix64 from one seed, no clock, no thread RNG: the same seed and
  corpus replay the same sequence, and a saved artifact replays as an ordinary seed (copy it into
  `fuzz/corpus/http_parse/`, which is exercised before any mutation).
- **Outcome buckets on a logarithmic size scale.** The first 120 s run filled the 4 096-input corpus
  cap in ten seconds because a linear length bucket made almost every mutated length its own
  outcome. `scale(n)` buckets by the order of magnitude of the consumed length instead; the final
  run's corpus growth decelerates instead of saturating.
- **The committed corpus is part of the repository.** Twelve seeds under `fuzz/corpus/http_parse/`:
  a plain GET, a query string, a Content-Length POST, a chunked POST with an extension and a
  trailer, two pipelined requests, duplicate `Content-Length`, `Content-Length` plus
  `Transfer-Encoding`, HTTP/1.0, `Expect: 100-continue`, a truncated head, a truncated chunked
  body, and a 256-byte binary blob with NUL and 0xFF runs. The campaign exercises all of them before
  it mutates anything, so a broken seed fails the run immediately.
- **A crash is an artifact and a non-zero exit.** The input is written to `fuzz/artifacts/` and the
  process exits 1, the way a sanitizer crash ends a libFuzzer run; usage errors exit 2. The
  directory is created on demand and does not exist in a clean tree.

**The oracle was proved to fire, not assumed to.** A one-line mutant dropped the `data.len() -
self.pos < 2` length guard on the chunk-terminating CRLF. At seed 1 the campaign panicked at
execution 6 507 with `range end index 6 out of range for slice of length 5`, wrote
`fuzz/artifacts/crash-seed1-iter6507.bin` (8 192 bytes — the maximum input length) and exited 1.
The source was restored and hash-verified, and the artifact deleted. Without this step "0 crashes"
would only have meant "the oracle never spoke".

**The run.** `cargo run -p aether-fuzz --release -- --seconds 120 --seed 20261030` on 2026-10-30:
**15 843 328 executions in 120.011 s** (132 016/s by the runner's own count), corpus grown from 12
to 719 inputs, 717 outcome buckets, **0 crashes**. The result is durable here and in the matrix row;
the log lived in `C:/tmp` and is not cited, because scratch directories are deleted.

**Limits, stated so the row is not overread.** No sanitizer and no edge coverage (see above);
single-threaded, in-process, and therefore blind to races; the seeds are hand-written, so the
campaign explores around the request grammar rather than proving the grammar complete; the corpus
cap means a late run spends its time near the cap rather than in new territory. What the run does
establish is that 15.8 million mutated requests crossed every public parse entry point without a
panic, and that the harness reports one when it happens.

**Tests.** Four unit tests in the crate: the generator is deterministic and bounded, every
committed seed passes, the mutation cap is honoured, and a bounded 2 000-iteration campaign finishes
without a crash (so the campaign loop itself is covered without a wall-clock dependency).

**Recount.** 726 → 730: four `aether-fuzz` unit tests. 408 unit + 310 integration + 12 doc over 62
`test result:` lines, 25 integration targets, 0 failed, 1 ignored.

**Documentation.** The matrix's *Fuzz target* row replaces "compiled, not fuzzed" with the campaign
result, the corpus, the injected-panic proof and the stated limits; PRR §9 item 10 and matrix
recommendation 12 are closed; PRR's "not verified" list now names memory-unsafety only; the
roadmap's F-17 row, row 102 and §19.1's matrix count move to 120 / 4 / 10; both `README.md` files
carry the campaign sentence; the Arabic guides' stale `http2_e2e` counts were re-derived while the
counts were being swept (37 → 43 → 49, with the per-track deltas named, and `http2.rs`'s 12 unit
tests → 14). The fuzz crate's unused `aether-core` dependency was removed rather than documented as
a wart.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
730 passed / 0 failed / 1 ignored across 62 `test result:` lines; govtests 94 passed / 0 failed;
cite 101 citations over 71 distinct spans in 15 live documents with 0 findings, plus 61 snapshot
citations marked and not validated; evidence 33 documents / 0 findings; validate exit 1 with
exactly the four pinned findings (PROB-0007 twice, PROB-0017 twice, REQ-0001 three times, REQ-0001
open under a past ID) and nothing else. Nothing pushed.

## Track 103 — secrets stay arguments, and no type renders one (2026-10-30)

Row 103 closes here with F-18. The roadmap left the shape open — a redacting `Secret<T>` wrapper or a
documented non-goal — so the track's first job was to enumerate what the framework holds and what it
can print, because that inventory is what decides the question.

**What holds a secret.** Secrets arrive as constructor arguments and are never fields of
`AetherConfig`: the TLS material (`TlsCertSet`, `TlsIdentity`), the bearer validator (`JwtValidator`,
`BearerAuth`), the CSRF key (`CsrfTokens`) — and the application's own values, which the framework
never sees. The deployment docs' environment variables are the intended route. A configuration struct
that carried them would put them in the one object the framework hands around, prints and clones.

**The decision: a documented non-goal, not a wrapper.** A `Secret<T>` protects only the code that
elects to use it. Nothing in the framework can require it, so it would be public API with no invariant
behind it: an adopter who forgets the wrapper is exactly as exposed as before. What the framework can
guarantee is the other half — **no framework type renders a secret**, whatever an application does with
its own values. The matrix's *Secret management* row keeps `NOT_IMPLEMENTED` and takes that guarantee
as its evidence cell; roadmap §19.6's non-goals table gains the row.

**What the inventory found, and the one leak.**

- **`CsrfToken` printed the live token.** It was `#[derive(Debug, Clone)]`, so any struct holding one
  that derived `Debug` — a form context, an extracted handler argument, a test's assertion message —
  carried the credential into a log. A form renders this token; a `{:?}` is not the place. `Debug` is
  now manual and prints `CsrfToken(<redacted>)`, and `Clone` is unchanged, because the token is legitimately
  copied into a markup layer. `as_str` stays the deliberate way to read it.
- **The three key-holding types implement no `Debug` at all.** `JwtValidator`, `BearerAuth` and
  `CsrfTokens` now each carry a `compile_fail,E0277` doc test that formats `{value:?}`; the snippet
  compiles only if a `Debug` impl is added, which turns the doctest red ("Test compiled successfully,
  but it's marked `compile_fail`"). This was verified by injecting `#[derive(Debug)]` on `CsrfTokens`
  and watching exactly that message appear for `csrf::CsrfTokens (line 161)`, then restoring the file.
  An autoref-specialization probe was the first attempt; on stable, method resolution stops at the
  by-value candidate when its bound fails, and the probe dies with E0599 — the doctest is the
  mechanism that works here.
- **`BearerError` carries neither a reason nor the credential.** It is a unit struct, so a refusal
  cannot echo the token that was refused. `CsrfError`'s rendering names the entropy source and nothing
  about the key. `Extensions` prints its count and charged bytes, never a value; `LoggerMiddleware`
  prints method, path and status.
- **The TLS types' `Debug` was checked against the dependency source, not assumed.** `TlsCertSet` and
  `TlsIdentity` derive `Debug` over `rustls::sign::CertifiedKey`, which derives it over
  `cert: Vec<CertificateDer>`, `key: Arc<dyn SigningKey>` and `ocsp`. The public chain is *supposed* to
  be printable; the question was the signing key. In `rustls` 0.23.46 the `ring` provider's
  `RsaSigningKey`, `EcdsaSigningKey` and `Ed25519SigningKey` each print only their `algorithm`
  (`src/crypto/ring/sign.rs:142,313,403`), and `ring`'s `hmac::Key` does the same. A new test in
  `tls.rs` pins the rendering rather than the claim: it takes the private key's DER and PEM, keeps
  12-byte windows that do not occur in the public chain (at least five must survive, or the scan is
  declared vacuous), and searches `format!("{set:?}")` and `format!("{identity:?}")` for every window in
  PEM text, lowercase hex (rustls's `Der` prints `0x` once for the whole payload, so the windows carry
  no prefix) and `[a, b, …]`-delimited decimal form. A positive control asserts a mid-certificate
  window *is* rendered, so the test cannot pass by searching an empty string.

**The first oracle was insensitive, and that is this track's most useful finding.** The TLS test as
first written passed against a mutant that leaked the key: it carried a `leaked_key_pem: Vec<u8>` field
rendered through `Debug`, and the scan still reported clean. The decimal probe had built
`format!("{:?}", window)` — brackets included — so it only matched a rendered slice that *began* at the
window. Probing with the bracketed and comma-delimited forms a real `Vec<u8>` rendering emits made the
same mutant fail at `aether\src	ls.rs:1173`, and the clean tree still pass. The lesson is the
register's standing one about oracles: a test that passes is not evidence until a mutant it should kill
has been killed by it.

**Tests.** One unit test in `aether/src/tls.rs` (the rendering scan with its positive control), two
integration tests in the new `aether-security` target `security/tests/secret_rendering.rs` — the CSRF
token renders the placeholder and not the sentinel; a bearer refusal renders no credential in its
`Display` or `Debug`, and the CSRF refusal names its entropy source — and three `compile_fail` doc
tests, one per key-holding type.

**Recount.** 730 → 736: one `tls.rs` unit test, two integration tests in a new target, three
`compile_fail` doc tests. 409 unit + 312 integration over 26 targets + 15 doc over 16 doc runs, 63
`test result:` lines, 0 failed, 1 ignored.

**Documentation.** The matrix's *Secret management* row becomes the non-goal with its evidence; the
matrix's unit/integration/doc counts, target list and per-file notes move to the track-103 tree; PRR §3
records that the row's status is unchanged and only its evidence moved; both `README.md` files carry the
guarantee and the new target; the Arabic guide gains a subsection stating where a secret is kept and
that redacting an application's own value stays the application's job; roadmap §19.1's matrix count is
re-stated as unchanged (no status moved, so 120 / 4 / 10 / 0 / 0 / 0 / 2 = 136 holds on this tree) and
§19.6 lists the refused wrapper. The doc comments added to `jwt.rs` and `csrf.rs` moved every span below
them: seven live citations in four documents failed with "the span's first line moved", and all seven
were re-derived — `jwt.rs:84` → `:95`, `csrf.rs:242-247` → `262-267`, `292-300` → `312-320`, `310-324` →
`330-344` — in the prose and in the pin table, which refuses a pin no citation uses. Re-running `cite`
also exposed two stale `100 citations` claims (the tool's own doc string and the matrix's *README /
docs accuracy* row) that had survived track 100's addition of the 101st; both now read 101. `govtests`
needs the index row, added with this section.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): the first run failed two gates — `cite`, on the seven moved spans above, and
`govtests` with it, because its repository-wide citation test runs the same check — and passed the
other five. After the pins and the two stale counts were corrected, the second run: fmt exit 0;
clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests 736 passed / 0 failed / 1
ignored across 63 `test result:` lines (409 unit over 21 targets + 312 integration over 26 targets +
15 doc over 16 doc runs); govtests 94 passed / 0 failed; cite 101 citations over 71 distinct spans
in 15 live documents with 0 findings, plus 61 snapshot citations marked and not validated; evidence
33 documents / 0 findings; validate exit 1 with exactly the four pinned findings (PROB-0007 twice,
PROB-0017 twice, REQ-0001 three times, REQ-0001 open under a past ID) and nothing else. Nothing
pushed.

## Track 104 — the readiness report becomes a checked live document (2026-10-30)

Register **B22**. `framework/docs/PRODUCTION_READINESS_REPORT.md` carried `<!-- cite-check: snapshot
2026-10-18 -->` and was named in `SNAPSHOTS`, so all 22 of its citations were exempt from
`cite_check.py` — and four later tracks had nonetheless edited its maintained sections, so it was
neither an honest record nor a checked live document. A hand re-derivation of the five citations
that read as current found all five stale: the HTTP/2 tally named `http2.rs:525` where it now sits
at `896`, and the four `app.rs` spans had each moved by two to fourteen lines. The marker was hiding
exactly the drift it was supposed to prevent.

**The decision: option B of B22, not the split.** B22 offered two resolutions. The split — a frozen
2026-10-18 report plus a separate live readiness page — was refused: two files carrying the same
live status is the documented drift hazard in this repository, and the matrix is already the
row-by-row authority for status. So the report stays one file and now says which of its sections is
which. §1, §4, §6, §7 and §10 describe the 2026-10-18 tree; §2 is its 2026-10-18 summary with the
dated re-measurements appended; §3, §5, §8 and §9 are maintained and checked; §11–§18 are dated
addenda describing the tree of their own date. The header gained `**Report date:** 2026-10-18` and
`**Last revised:** 2026-10-30 (track 104)`, and the banner blockquote records the section inventory
and why the marker had to go.

**What changed in the text.** The 22 former citations split on inspection. The historical ones —
statements about the tree of a named date — became "line N of `<file>`" prose, optionally anchored
to a commit (`on e81efed, the assertion sits at line 1502`); that is honest about being a reading of
that tree and cannot be mistaken for a current coordinate. The live ones were re-derived and pinned:
five spans now sit in `cite_check.py`, `framework/aether/src/http2.rs:896` and
`framework/aether/src/app.rs:123-165`, `:167-185`, `:187-220`, `:222-263`. `SNAPSHOTS` lost its
readiness-report entry, the document lost its marker, and the dated set is now eight documents
carrying 49 citations. The census moved from 101 citations over 71 spans in 15 live documents to
**106 citations over 71 distinct spans in 16 live documents**, 0 findings. Two live claims about the
census itself — the tool's own doc string and the matrix's *README / docs accuracy* row — were
updated in the same change, including the row's "nine dated records", now eight.
The banner's count was re-checked against `git log`: exactly twelve commits touch the file between
the marker's addition (track 92) and this track, and the sentence now says the twelve edited the
file with §3, §5, §8 and §9 among their edits, rather than implying every one touched all four.

**The last machine-absolute paths.** §17's four Markdown links were spelled
`file:///c/Users/hamza/...`, so they resolved on this machine and on no other — an export of the
tree failed `test_every_absolute_path_in_the_docs_resolves_inside_the_repository` on exactly those
four. They now name the repository-relative file, with their line numbers left in the prose, which
is how a dated addendum should carry a coordinate. That emptied the evidence gate's corpus:
`test_the_corpus_still_exercises_the_pattern` required at least one absolute token so that a pattern
matching nothing could not pass silently, and its own comment asked that the removal be deliberate.
It is replaced by `test_the_docs_carry_no_machine_absolute_path`, which asserts the stronger
invariant — no document names a path on the machine that wrote it, whether or not that path resolves
— while the fixture tests covering a dozen spellings keep the detector proved.

**A test threshold that had to move with it.** `test_every_listed_snapshot_is_present_and_marked`
refused a snapshot list shorter than nine, so that a removal would be a reviewed act rather than an
accident. Track 104 is that reviewed act: the baseline is now eight, the comment beside it says why,
and a further shrink must lower the number deliberately. The guard is kept, not removed.

**Two checks of the new arrangement.** The report's own §5 tally was re-read against the file before
it was pinned: `:896` is the `price_response_body` call in the HTTP/2 serve loop, and the pin names
it, so an insertion above the span fails the gate. And the marker's absence is enforced from both
sides — an unlisted document carrying the marker is a finding and a listed document without it is a
finding — so the removal cannot be silently undone.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): the first run passed six gates and failed `govtests` on one assertion —
`test_every_listed_snapshot_is_present_and_marked` refused the snapshot list for being shorter than
nine. That is the guard doing its job, and the reviewed answer is to lower the baseline to eight and
say why in the test, which is what this track did. Every later run passed all seven; the last one,
taken after the governance notes and this §17 link fix, reported: fmt exit 0;
clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests 736 passed / 0 failed / 1
ignored across 63 `test result:` lines (the split is unchanged from track 103 — 409 unit over 21
targets + 312 integration over 26 targets + 15 doc over 16 doc runs — because no Rust code changed);
govtests 94 passed / 0 failed; cite 106 citations over 71 distinct spans in 16 live documents with 0
findings, plus 49 citations in 8 dated snapshots marked and not validated; evidence 33 documents / 0
findings; validate exit 1 with exactly the four pinned findings and nothing else. Commit A
(`3b3f5b0`) was also exported to `C:/tmp` and checked as a plain tree, where the readiness report's
links resolve like any clone: cite 106 / 0, evidence 33 / 0, govtests 94 OK — the export failure
that reproduced at the parent commit no longer does. Nothing pushed.

## Track 105 — the request-target means what RFC 9112 §3.2 says it means (2026-10-30)

**No register item and no roadmap F-number named this.** The gap was read out of the spec against
the code: `framework/http/src/parser.rs` took `raw_uri`, split it at the first `?`, and handed
everything before the `?` to the router as the path. Nothing in that chain asked which of the four
request-target forms the client had sent, so every target was treated as origin-form. Each of the
following was true of the tree this track started from, and each is now false:

* `GET hello HTTP/1.1` matched the route `/hello`. Verified from the code, not from memory: both
  `RadixRouter::add_route` (`framework/router/src/trie.rs:104-108`) and `find` (`:222-226`) drop
  empty segments, so `hello` and `/hello` normalize to the same key. A target the client never
  meant as a path became a routable one.
* `GET ?x=1` matched `/`: the path was the empty string before the `?`, which the router reads as
  the root leaf.
* A `#` in the target became part of a segment. A fragment is never sent to a server, so a target
  carrying one is malformed, not a path with a literal `#` in it.
* An absolute-form target was neither honoured nor rejected: `GET http://host/p` became the path
  `http://host/p`, and the authority the client named reached neither host validation, CORS, nor a
  handler reading `Host`. RFC 9112 3.2.2 says "A server MUST accept the absolute-form in requests"
  and, for an origin server, "the origin server MUST ignore the received Host header field (if
  any) and instead use the host information of the request-target".
* Two `Host` lines collapsed into one value before any check could see them, so the field the
  client sent second and the first-field value the server decided on could name different hosts.
  RFC 9112 3.2: "A server MUST respond with a 400 (Bad Request) status code to any HTTP/1.1
  request message that lacks a Host header field and to any request message that contains more
  than one Host header field line."
* `OPTIONS *` - asterisk-form, the server-wide OPTIONS target (RFC 9112 3.2.4) - was routed as a
  path named `*` and answered 404.
* `CONNECT host:443` was answered 404 by the router, blaming the named host and port for a
  capability this server does not have. RFC 9110 9.3.6 gives a server without tunnel support a 405
  or a 501 for exactly this case.

**Decisions, and the weaker option each refused.**

* Absolute-form **replaces** the `Host` field rather than requiring the two to agree. Requiring
  agreement is the weaker rule: it makes a proxy that forwards absolute-form unserveable, and
  3.2.2 does not ask for a comparison, it asks the origin server to ignore the received field. The
  replacement happens once, after the header loop, so host validation, CORS and a handler's own
  read of `Host` all see one string.
* The duplicate-`Host` refusal counts **field lines**, before that replacement, on **every** HTTP/1
  version. Comparing values would let `Host: a` twice through, which is exactly the ambiguity the
  rule removes; and counting had to precede the replacement, or the two fields would have merged
  into one value before anyone could see there were two. HTTP/1.0 is included because the sentence
  covers "any request message that contains more than one Host header field line", while the
  missing-`Host` half of that same sentence is 1.1-only - and a missing `Host` stays a 400 even
  when the target supplied an authority, because the replacement would otherwise be a way to skip
  a check HTTP/1.1 requires.
* `OPTIONS *` answers 204 with one server-wide `Allow`, computed at build time by a new
  `RadixRouter::for_each_route` walk instead of per request: the answer cannot change while the
  server runs, and unioning every route for each asterisk request is work the router already did
  once. `HEAD` is listed only when a non-WebSocket `GET` route exists, because the serving core
  answers HEAD from GET while a WebSocket route refuses every non-GET before the handler runs.
* `CONNECT` is answered 501, not 405: 405 would say this target does not allow the method, and the
  truth is that this server cannot do the method anywhere. It sits after the security chain, so
  host validation, rate limiting and CSRF still answer first and an off-list `Host` keeps getting
  its 400.
* The h2 fold changed from "use `:authority` only when `Host` is absent" to "`:authority` replaces
  `Host`". The old rule was written as parity with HTTP/1 and was its opposite: once an
  absolute-form target's authority wins on HTTP/1, a client that sent both on h2 resolved a
  different host for the same request. RFC 9113 8.3.1 offers reject-or-use-`:authority`;
  replacing is what keeps one answer per protocol pair, and `insert` also collapses duplicate
  `Host` lines so exactly one host survives.

**Tests.** Fifteen added, fourteen net, because the h2 pair replaced the one test that pinned the
old precedence: five `aether-http` parser unit tests, one `trie.rs` test that the walk visits every
leaf once, two `aether/src/http2.rs` host-precedence tests, and seven raw-socket tests in the new
`framework/tests/tests/http_conformance.rs` - absolute-form routing on the same path and query as
origin-form, the authority replacing the field, host validation reading that authority, `OPTIONS *`
answering for the whole server, out-of-form targets refused on the wire, `CONNECT` answered 501,
and host policy outranking that 501.

**Sensitivity, proved by injection, each restored byte-identically.** The two h2 assertions fail
independently of each other: folding only when `host` is absent (the pre-track rule) fails
`the_authority_beats_an_explicit_host` while `a_host_without_an_authority_stands` stays green, and
replacing the single store without clearing the multi-value store fails only the second
assertion of the first test. `framework/aether/src/http2.rs` was restored to
`1f57cd0ae205dc3460afa62f5647e3df2ad00a88` before `cargo fmt` and to
`e4fd851b171d4b575fdb478d5c1e199648d07ee7` after it. On the serve side, dropping
`.or(connect_refusal)` made exactly `connect_authority_form_is_answered_not_implemented` fail with
`left: 404, right: 501` (restore `17922968ded966363890ae0dd97c0b5608e503b9`, then `cargo fmt --all`
reflowed the selection chain); moving that arm ahead of `rejection` made exactly
`host_validation_outranks_the_connect_refusal` fail with `left: 501, right: 400` and nothing else,
and `framework/aether/src/serve.rs` was restored to the hash that run began with,
`1df8e0bd5ebefbbe7d06d64a6b9ed3daafd79d06`, with the target green again at 7 passed / 0 failed.

**Citations, re-derived three times because the tree moved under them.** The `app.rs` insertion
that computes the server-wide `Allow` shifted every coordinate below it: 27 `cite_check.py` pins
and 39 document tokens. The `http2.rs` doc-comment then moved two pins (`:896` to `:901`, `:1014`
to `:1019`) and three tokens. The `connect_refusal` block moved the last two spans
(`framework/aether/src/serve.rs:935-940` to `:943-948`, `:868-875` to `:876-883`) plus the tokens
in `docs/00_framework_specification_ar.md:209`, `docs/00_framework_specification_en.md:211` and
`framework/docs/FEATURE_COMPLETENESS_MATRIX.md:552`. Each was re-derived by matching the span's own
lines in the moved file, not by adding the shift, and the census never changed: **106 citations
over 71 distinct spans in 16 live documents, 0 findings**, plus 49 citations in 8 dated snapshots -
prose was extended where a new coordinate would have moved the census.

**A line-ending defect this track created and fixed.** A patch script split
`framework/docs/FEATURE_COMPLETENESS_MATRIX.md` on newline and rejoined it on CRLF without
stripping the carriage return already on each line, which wrote `CR CR LF` on all 796 lines and
made `git diff` report the entire file as rewritten when 19 lines had real content changes. Found
by the diff's own line count, repaired by collapsing the doubled marker back to CRLF, and the file
now diffs 19 added / 18 removed. Recorded because a whole-file diff is how a real change hides in
this repository, and because it is the second time an in-session patch script was the culprit. The same session also truncated this register once while appending: an `open(path, wb).write(head + addition)` expression truncates the file the moment the handle opens, before the argument raises, and here the argument did raise. Nothing was lost because the file was restored from `HEAD` and verified prefix-identical to it before the append went in again - but the pattern is a standing hazard for every append in this repository, and the safe form is to build the full bytes first, then open.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
**750 passed / 0 failed / 1 ignored** across 64 `test result:` lines, split from those per-target
lines as 416 unit over 21 targets + 319 integration over 27 targets + 15 doc over 16 doc runs;
govtests 94 tests, OK; cite 106 citations / 71 spans / 16 live documents, 0 findings; evidence 33
documents / 0 findings; validate exit 1 with exactly the four pinned findings (PROB-0007 and
PROB-0017 recycled, REQ-0001 allocated three times, REQ-0001 open-vs-history) and nothing else.
Nothing pushed.

**What stayed unverified.** No HTTP/2 wire test sends a `Host` that disagrees with its
`:authority` - the precedence is proved inside `header_maps`, the h2 translation function, so a
regression in how the h2 serve loop calls it would not be caught here. `OPTIONS *` and
absolute-form have no h2 coverage either; `*` is not a valid h2 `:path`, so that half is a stated
non-applicability rather than a gap, but the server-wide `Allow` union is exercised over HTTP/1
only. The fuzz figure the matrix carries (15 843 328 executions, 0 crashes) was measured before
`classify_target` was in the parse path and has not been re-run over it. And the whole run remains
Windows-only (task 5).


## Track 106 — HTTP/2 reads the same request-target set (2026-10-30)

**Defect.** Track 105 closed RFC 9112 §3.2 on the HTTP/1 request line and named what it left
unverified: no h2 wire test disagreed with its `:authority`, and `OPTIONS *` had no h2 coverage.
Measured on this tree before touching anything, three of those gaps were live defects:

* `GET *` over h2 was routed as the path `*` and answered **404**. RFC 9113 §8.3.1 keeps
  asterisk-form legal (`":path" … the asterisk form is allowed for the OPTIONS method`), so a `*`
  carried by any other method is the same out-of-set request-target HTTP/1 refuses with 400
  (§3.2.4) - but it arrives here as a perfectly framed path string, so only this frontend can tell
  the two apart.
* A `CONNECT` over h2 was answered **only after the body wait**. RFC 9113 §8.5 gives END_STREAM
  the meaning of a TCP FIN, which a CONNECT client sends once the tunnel exists; a conformant
  client therefore leaves the stream half-open, and `read_body` sat in front of the refusal,
  holding it behind `receive_timeout`.
* The `:authority`-over-`Host` replacement had no wire coverage at all: track 105's injection
  proved it inside `header_maps`, which is the h2 translation function, so a regression in how the
  h2 loop *calls* it would have gone unnoticed.

One candidate turned out not to be a defect, and is recorded so the list is not padded: the
server-wide `OPTIONS *` answer already worked over h2 - `204` with
`allow: GET, HEAD, OPTIONS` - because both frontends share `ServiceCore::serve`, and track 105's
asterisk arm is in that shared chain. Injection I4 below proves the test is sensitive to that arm
rather than passing for some other reason.

**Decisions, and the weaker option each refused.**

* `*` on a non-`OPTIONS` method is answered `400` from this frontend, not reset. RFC 9113 §8.1.1
  lets a server either answer a malformed stream first or reset it with `PROTOCOL_ERROR`, and the
  two are not interchangeable here: a `*` target is the same *request* that HTTP/1 answers 400, so
  answering keeps one verdict per request-shape, and a reset code tells the peer nothing about why.
* A `:path` that is empty, absolute, or does not begin with `/` is left to `h2`. It refuses such a
  stream itself (§8.1.1, malformed → stream error `PROTOCOL_ERROR`) before this crate sees it, so
  the frontend has no branch to write; what is this server's job is that the *connection* keeps
  serving afterwards, and that is pinned on the wire instead.
* `CONNECT` is answered `501` from the head, and the head answer is placed where the HTTP/1 chain
  places it - after the security decisions, so host validation, rate limiting and CSRF still answer
  first. Reading a body first was not merely slow: it made the refusal unreachable for a client
  that behaves correctly.
* A `Host` that disagrees with `:authority` is still **replaced**, not rejected. §8.3.1 states the
  MUST as "MUST NOT use the `Host` header field to determine the target URI if `:authority` is
  present" and only *SHOULD*-malformed a differing field, and the same section permits an
  intermediary to "retain any `Host` header field" while rebuilding the authority - rejecting would
  break the proxied case the RFC explicitly allows, and replacing is what makes one request resolve
  one host whichever protocol carried it.
* The doc-comment that claimed a non-folded `:authority` would hand "every legitimate h2 client a
  421" was falsified and removed: 421 is this server's answer to a host-policy mismatch, which the
  missing fold turns into a *false* rejection of a valid request, not a 421. The rewrite states the
  MUST/SHOULD split the code actually implements.

**Residual, named not fixed.** A `#` in an h2 `:path` cannot be refused here. A temporary probe
(deleted after the run, `C:/tmp/t106/probe_h2_shapes.rs` in its last revision) forwarded
`/x#frag` and reached this frontend as `pq = Some("/x")`: `h2` removes the fragment before handing
the request over, so the field HTTP/1's `classify_target` refuses with 400 never exists by the time
this code could look at it. Both protocols still identify the same target resource, which is the
property §3.2 is about; the asymmetry is in the answer to a malformed message, one layer up. The
same is true of an absolute-form `:path`: `h2` refuses it before this crate sees it, so no h2 test
can reach the branch that replaces `Host` from a target authority - that half of §3.2.2 stays
HTTP/1-only by construction.

**Tests.** Six added, all in `framework/tests/tests/http2_e2e.rs` (+328 lines, target now 55 -
30 over TLS with ALPN `h2`, 21 over cleartext h2c, 4 hand-framing the socket):
`an_asterisk_on_a_method_that_is_not_options_is_answered_400_over_h2`,
`a_connect_is_answered_from_the_head_without_a_body`,
`a_path_that_is_not_origin_form_is_reset_and_the_connection_survives`,
`options_asterisk_answers_for_the_whole_server_over_h2`,
`the_authority_replaces_a_disagreeing_host_on_the_wire`,
`host_validation_reads_the_authority_and_not_the_field_it_replaces`. The last two read what a
*handler* and the *host policy* each resolve `Host` to through a `host_report_app` that prints both
stores, which is the coverage track 105 said was missing. A `request_raw` helper sends an arbitrary
`:method`/`:path` with a chosen END_STREAM and a wall-clock bound, because the shape that matters
here is a stream that stays half-open.

**Sensitivity, proved by injection, each restored byte-identically.**

* I1 - `if false` on the new `*` arm in `framework/aether/src/http2.rs`:
  `an_asterisk_on_a_method_that_is_not_options_is_answered_400_over_h2` failed with
  `left: 404, right: 400` and nothing else.
* I2 - `if false` on the `Method::Connect` branch, so CONNECT reads a body again:
  `a_connect_is_answered_from_the_head_without_a_body` failed with
  `response head within the bound: Elapsed(())` after 5.03 s - the refusal really was behind the
  body wait.
* I3b - the fold reverted to `entry(...).or_insert(...)` in **both** stores (the pre-track-105
  rule): `the_authority_replaces_a_disagreeing_host_on_the_wire` failed with
  `single=disagreeing.test|multi=["disagreeing.test"]` and
  `host_validation_reads_the_authority_and_not_the_field_it_replaces` failed with it, while the
  third `authority`-named test stayed green.
* I4 - track 105's server-wide arm in `serve.rs` matched against `"*off"` instead of `"*"`:
  `options_asterisk_answers_for_the_whole_server_over_h2` failed with `left: 404, right: 204`
  **and** `options_asterisk_answers_for_the_whole_server` failed the same way in
  `http_conformance` - one shared arm, two frontends, both tests sensitive to it.

`framework/aether/src/http2.rs` was restored to `00d3445657a4022fd02509e318901eb258902a04` and
`framework/aether/src/serve.rs` to `8c790015cb87e8e0d092f4cc2ce9b1e76cb3a819`, each verified by
`sha1sum` before the targets were re-run green: `http2_e2e` 55 passed / 0 failed,
`http_conformance` 7 passed / 0 failed.

**The one new test that injection cannot invert, stated as such.**
`a_path_that_is_not_origin_form_is_reset_and_the_connection_survives` asserts
`codes == vec![(1,1),(3,1),(5,1)]` (stream, `PROTOCOL_ERROR`) plus an answer on stream 7. The reset
is `h2`'s own §8.1.1 validation, which happens before this crate is handed the stream, so no
mutation of this repository can produce the 200/404 that would falsify the first half. What this
server owns, and what a mutation *could* break, is the second half - the connection serving the
well-formed stream behind them. It is kept as a contract pin on the dependency's behaviour plus a
proved pin on this server's survival, not counted as a defect test.

**Citations, re-derived again because the tree moved.** `cargo fmt` plus the doc-comment rewrite
shifted three spans in `framework/aether/src/http2.rs` by +26/+28 lines: `:803` → `:829`
(`07_limits_and_dos_mitigation.md:95`), `:901` → `:929` (matrix `:403`, PRR `:599`), `:1019` →
`:1047` (matrix `:403`), and the three `cite_check.py` pins with them. `serve.rs` was deliberately
edited comment-for-comment so it stayed at exactly **1068 lines**, which is what kept the two
`serve.rs` pins (`:876-883`, `:943-948`) and every doc token naming them valid. The census is
unchanged: **106 citations / 71 distinct spans / 16 live documents / 0 findings**, plus 49 citations
in 8 dated snapshots.

**The line-ending defect recurred, and was caught the same way.** A patch script read
`FEATURE_COMPLETENESS_MATRIX.md` and `PRODUCTION_READINESS_REPORT.md` as text, split on `\n` and
rejoined on `\r\n`, which wrote `CR CR LF` on all 796 and 1 648 lines of the two CRLF files. Caught
by printing `CR` and `LF` counts after each write (`2 ×` the line count is the tell), restored from
`HEAD`, and re-applied at byte level; the register's own append used a load/save pair that asserts
`CR == CRLF` before touching a file. Track 105 recorded this exact failure mode once already; the
rule now in practice is that every doc patch run ends by printing CR/LF counts for each file it
touched, not by trusting the editor.

**Fuzz, re-run over the changed parser.** Track 105 left this open ("measured before
`classify_target` was in the parse path"). `cargo run --release` over the committed corpus, seed
20261030, 120 s: **16 089 088 executions in 120.006 s** (134 069/s by the runner's own count),
corpus 12 → 711 inputs, 709 outcome buckets, **0 crashes**, exit 0. `classify_target` runs inside
the fuzzed path (`parse_request_head` / `parse_request` both call it), so the new target rules are
now covered by the campaign that closed F-17.

**Docs carried by this track.** `docs/04_security_and_rfc9112.md` gained the h2 half of §3.2 - all
six test names, the `PROTOCOL_ERROR` boundary, the head-answered `CONNECT` and the `#` asymmetry -
and its `:authority` sentence was corrected to quote the MUST and the SHOULD separately. Both
specification files gained the same set as an append to their existing target item (EN item 5, AR
item 4), so no coordinate below them moved. `FEATURE_COMPLETENESS_MATRIX.md`'s HTTP/2 row now names
the four forms it enforces and its counts (49 → 55, 319 → 325 integration, 750 → 756 total) are
re-read from the per-target `test result:` lines; `BASELINE_MEASUREMENTS.md` and the readiness
report's seventeenth reading carry the same recount; `framework/README.md` carries the fuzz re-run.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`): fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests
**756 passed / 0 failed / 1 ignored** across 64 `test result:` lines, split from those per-target
lines as 416 unit over 21 targets + 325 integration over 27 targets + 15 doc over 16 doc runs, with
`http2_e2e` at 55 and `http_conformance` at 7; govtests 94 tests, OK; cite 106 citations / 71 spans /
16 live documents, 0 findings, plus 49 citations in 8 dated snapshots; evidence 33 documents / 0
findings; validate exit 1 with exactly the four pinned findings (PROB-0007 and PROB-0017 recycled,
REQ-0001 allocated three times, REQ-0001 open-vs-history) and nothing else. Nothing pushed.

**What stayed unverified.** No h2 client here ever sends a `Host` with no `:authority` *and* a
non-empty authority-less `:path`, so the interaction of the fold with `HostValidation`'s missing-host
half is only covered by the unit pair. The `#` and absolute-form `:path` asymmetries above are
dependencies' behaviour, not ours, and nothing here proves what `h2` does with them beyond what the
probe observed on one version (0.4.20). The hand-framed malformed-`:path` test depends on `h2`'s
error codes staying `PROTOCOL_ERROR` for these three shapes. Nothing ran on Linux (task 5), and no
performance number was measured by this track at all.


## Track 107 — group-scoped state and a mountable sub-app (2026-10-30)

**Defect.** The matrix's *Nested routers / independent sub-apps* row carried
`NOT_IMPLEMENTED` and, since track 88 re-worded it (register line 3314, roadmap D-15 at
`2026-10-26-remediation-roadmap.md:502`), named its own missing halves: "prefix grouping
(`RouteGroup`) plus group-scoped middleware; no nested state and no mountable sub-app". Both halves
were live gaps in the code, not wording:

* `RouteGroup` held a prefix and a middleware list and nothing else, so every route in every group
  read the one `AppStateContainer` that `AetherApp::manage` fills. A group could put a guard in
  front of its routes but not a handle behind them: two tenants served by one process had to share
  one value per type, and the workaround was to write the prefix into every path of the tenant's
  routes and give up the grouping.
* There was no *value* that collects routes. `AetherApp::group` takes a `FnOnce(&mut RouteGroup)`,
  which is a closure, not something a feature crate can build, test on its own, and hand over; the
  only alternative was a function taking `&mut AetherApp`, i.e. a feature that knows the port, the
  TLS configuration and the limits of the server it happens to run in. Axum (`Router::nest`) and
  Actix (`Scope`) both ship this shape, so the row was also the last structural API gap in the
  routing layer.

Measured before touching anything: `AetherApp::manage` wrote into `self.state` (one container),
`RequestContext::state::<T>()` was one map lookup (`app.rs:773-775` at HEAD), `serve.rs` was 1068
lines with 3 pinned spans, and no type in the workspace collected registrations for later replay.

**Decisions, and the weaker option each refused.**

* The scope travels as **one synthetic `MiddlewareEntry`** at the head of the scoped chain
  (`attach_state_entry`, `framework/aether/src/app.rs:857`), not as a field the frontends fill in.
  Refused: teaching `ServiceCore::serve` about scopes — that means threading a container through the
  HTTP/1 loop, the h2 bridge and the WebSocket upgrade, three pinned spans in a file the last two
  tracks deliberately froze at 1068 lines, and a per-request lookup for *every* route including the
  ones with no scope. The chain already exists and already runs before the handler, so an entry is
  both the whole mechanism and the reason an unscoped route still pays nothing
  (`RequestContext::scoped_state` is `None`, `app.rs:429`) and a scoped middleware sees the value
  the same way its handler does, because the entry sits in front of that scope's middleware.
* `state::<T>()` consults the scope's container **first** and the app's after, **per type**
  (`app.rs:788-795`). Refused: swapping the container when a scope exists, which would hide every
  app service nobody in the scope mentioned (that variant is injection I3 below, killed by
  `a_scope_that_does_not_register_a_type_keeps_the_app_value`); refused the reverse order (I2),
  which is a shadow that never wins.
* A group's container is `Arc<AppStateContainer>` with `Arc::make_mut` on write
  (`RouteGroup::manage`, `app.rs:3113-3116`): a child starts sharing its parent's, copies it out on
  its first own value, and its siblings never see the copy. Refused: a fresh container per child
  (I5 — a nested group stops resolving its parent's types, so nesting becomes a wall instead of an
  overlay) and refused a shared mutable container (two sibling groups would then register *through*
  each other, and a `manage` in one would answer in the other).
* `Router` is a `Vec<MountAction>` — `Box<dyn FnOnce(&mut RouteGroup) + Send>` (`app.rs:3378`) —
  replayed into the mount scope by `AetherApp::mount` (`app.rs:1646`), `RouteGroup::mount`
  (`app.rs:3139`) or `Router::mount` (`app.rs:3692`). Deferred replay is what lets a router hold no
  `&mut AetherApp` at all and inherit whatever guards and state enclose it at mount time. Refused: a
  `Router` that owns its own `AetherApp` and merges at mount — a merge must renumber handler ids and
  copy the inner app's middleware onto each route, which loses "the app-wide stack runs first" and
  duplicates what a group already gives; refused an eager `RouteGroup`-shaped builder, which cannot
  be a `RouteGroup` because that type borrows the app it writes into.
* `mount` takes the router **by value** and `Router` is not `Clone`. Refused: `Rc<dyn Fn(&mut
  RouteGroup)>` actions so one router could mount twice — every use this track found needs one
  mount, and mounting one router at two prefixes would also register its `use_middleware` twice per
  route and make the chain order depend on the mount list. The constraint is stated in the rustdoc,
  in `framework/README.md` and in `docs/09_api_reference.md` rather than papered over.
* Empty and `/` prefixes use the same concatenation rule as `group`, so `app.mount("", router)`
  serves the router's paths absolutely instead of needing a special case
  (`an_empty_prefix_mount_serves_the_router_absolute`).

**Residual, named not fixed.**

* A scope's values enter `AppStateContainer`, which is what `RequestContext::state::<T>()` and the
  `State<T>` extractor read (`framework/aether/src/extract.rs:522`). `aether_dependency`'s resolver
  is not on the serve path and this track did not change that, so a scoped value is not a
  dependency-injection node.
* No performance number for the overlay exists: one `Option` field and one extra chain entry per
  scoped route is a shape, not a measurement, and the owner measures.
* Only cleartext h2c was tested with a mounted router (`a_mounted_router_answers_with_its_own_state_over_h2c`);
  no TLS/ALPN mount test was added, because the overlay lives entirely in the shared chain and the
  TLS arm would exercise the same code through a different socket.
* Nesting is proved two levels deep at most: group → router on the wire
  (`a_router_mounted_in_a_group_inherits_its_guards_and_shadows_its_state`) and group → group in a
  unit test (`a_child_group_copies_the_container_out_on_its_first_value`). A router inside a router
  inside a group is reachable through `Router::mount` and compiles, but nothing here serves it.
* Nothing ran on Linux (task 5).

**Tests.** 20 added:

* 4 unit in `framework/aether/src/app.rs`: `the_scoped_overlay_answers_its_own_type_and_the_app_answers_the_rest`
  (`app.rs:4850`), `a_scope_contributes_one_entry_for_state_and_none_for_nothing` (`app.rs:4874`),
  `the_overlay_reaches_the_scope_middleware_and_not_the_app_stack` (`app.rs:4897`),
  `a_child_group_copies_the_container_out_on_its_first_value` (`app.rs:4956`) — the second is the
  one that pins "an empty scope adds no entry", i.e. the no-cost claim, and the third pins the
  position of the entry in the chain rather than its existence.
* 14 wire in the new target `framework/tests/tests/nested_routers.rs` (614 lines):
  `a_mounted_router_serves_under_its_prefix_and_not_without_it`,
  `a_mounted_router_state_answers_before_the_app_state`,
  `a_scope_that_does_not_register_a_type_keeps_the_app_value`,
  `two_groups_can_register_the_same_type_and_answer_for_their_own_routes`,
  `a_nested_group_shadows_its_parent_and_keeps_its_other_types`,
  `a_router_mounted_in_a_group_inherits_its_guards_and_shadows_its_state`,
  `the_chain_order_is_app_then_group_then_router_then_handler`,
  `a_router_value_reaches_only_the_routes_collected_after_it`,
  `the_state_extractor_resolves_the_scoped_value`,
  `a_group_without_state_resolves_the_app_container_unchanged`,
  `an_empty_prefix_mount_serves_the_router_absolute`,
  `a_router_static_dir_serves_files_under_the_mount_prefix`,
  `a_route_stream_in_a_mounted_router_streams_and_sees_its_state`,
  `a_router_websocket_route_upgrades_under_the_prefix_with_its_state`. The last three are the
  registration kinds that do not go through the plain route path — a streamed head dispatch, a
  `static_dir` resolver and a `101` upgrade — and each stores its chain somewhere else, so each is
  where a dropped overlay entry would show up.
* 1 in `framework/tests/tests/http2_e2e.rs` (+47 lines, target 55 → 56):
  `a_mounted_router_answers_with_its_own_state_over_h2c`.
* 1 live doctest: the `Router` example in its rustdoc runs, which is why `Doc-tests aether` is 12 → 13.

**Sensitivity, proved by injection, each restored byte-identically.** The ladder mutated
`framework/aether/src/app.rs` (anchor uniqueness asserted before each write, sha256 asserted after
each restore) and re-ran `nested_routers`, `cargo test -p aether --lib -- app::tests` and the one
h2c mount test. Mid-track pristine and restored hash: `56402b846f8a33ef50064254af5f37e71a4090b5212de3465c86f8c15e901c8a`.

* I1 — `state::<T>()` ignores the overlay, so a mount becomes a no-op: 10 of the 14 wire tests
  failed (`4 passed; 10 failed`), both chain/state unit tests failed, and the h2c test failed.
* I2 — the app container is consulted first: `a_mounted_router_state_answers_before_the_app_state`,
  `the_state_extractor_resolves_the_scoped_value`,
  `two_groups_can_register_the_same_type_and_answer_for_their_own_routes` and the h2c test failed
  (`11 passed; 3 failed`).
* I3 — the overlay hides every type it did not register: `a_scope_that_does_not_register_a_type_keeps_the_app_value`
  failed (`13 passed; 1 failed`). The h2c test stayed green, which is expected: it registers the
  type it reads, so this mutation cannot reach it — recorded rather than counted as a kill.
* I4 — the entry runs behind the scope's own middleware: `the_overlay_reaches_the_scope_middleware_and_not_the_app_stack`
  failed (`20 passed; 1 failed`).
* I5 — a child group starts a fresh container: `a_nested_group_shadows_its_parent_and_keeps_its_other_types`
  and `a_child_group_copies_the_container_out_on_its_first_value` failed.
* I6 — the scoped chain is placed before the app-wide stack: `the_chain_order_is_app_then_group_then_router_then_handler`
  and `the_overlay_reaches_the_scope_middleware_and_not_the_app_stack` failed.
* I7 — a router's registrations are replayed in reverse: `a_router_mounted_in_a_group_inherits_its_guards_and_shadows_its_state`
  and `the_chain_order_is_app_then_group_then_router_then_handler` failed (`12 passed; 2 failed`).
* I8 — a group's `static_dir` registration drops the scope's chain: `a_router_static_dir_serves_files_under_the_mount_prefix`
  failed.
* I9 — a group's `ws` registration drops the scope's chain: `a_router_websocket_route_upgrades_under_the_prefix_with_its_state`
  failed.
* I10 — a group's `route_stream` registers the route as buffered (`… , true)` → `… , false)`):
  **this one survived its first run** — `nested_routers` was `14 passed; 0 failed`, because
  `a_route_stream_in_a_mounted_router_streams_and_sees_its_state` asserted only the status and the
  tenant. The test was rewritten to print the shape it is supposed to pin
  (`tenant=sub|lane=true|buffered=0|bytes=10`) and the same mutation re-run failed it with
  `left: "tenant=sub|lane=false|buffered=10|bytes=10"`, `13 passed; 1 failed`. The claim in the
  matrix cell is written after that re-run, not after the first one.

`framework/aether/src/app.rs` was then restored and the tree was re-verified green; its hash at the
end of the track (after the `Router` rustdoc sentence about consumption and the `cargo fmt` split of
two test closures) is `fc84640294b5fb8b1e3902bc32bcccbbfe5cb1b0234abe0590f47647e92fbed0`.

**Citations, re-derived because the tree moved.** `app.rs` grew 4272 → 4981 lines and the growth is
not one offset: `+6` above `RequestContext`, `+11` above `AetherApp::manage`, `+41` above the accept
loop, `+85` above the parse arms, `+535` above the response writers, so **each of the 33 `app.rs`
PINS keys was re-matched against the code independently** (22 moved, 11 already held their pattern
at their own coordinate). Two spans changed *length*, not just position, and their ends were read
from the source rather than computed: `RequestContext`'s block went 387-427 → 393-437 because the
new `scoped_state` field and its doc-comment live inside it, and `conn_buffer_charge` went
3268-3270 → 3803-3805 → 3805-3807. The 30 doc citation tokens that name a moved app.rs span (26 with
a file token in the backticks, 4 bare `:NNN` inherited from the file named earlier on the same line)
were renumbered across `docs/00_ar`, `docs/00_en`, `docs/02`, `docs/05`, `docs/07` and `docs/09`;
where a line names several coordinates the ordering came from the prose, not arithmetic — the error
exit list at `docs/05…:136` is written in ascending source order (`2190`, `:2801`, `:2851`, `:3660`
→ 2275, 2886, 2936, 4195), and `docs/00…:207`'s "every one of the three is written with
`Connection: close`" names the three-arm match, so its `app.rs:2849` became 2934 and not the
`2881` push in the neighbouring arm. The `Router` rustdoc sentence and the fmt split then added 6
lines *below* three of those spans, moving `3803-3805` → `3805-3807`, `3939` → `3941` and `4195` →
`4197` a second time; `serve.rs` is still exactly 1068 lines, which is what kept its 3 pinned spans
out of this sweep entirely. The census after the sweep: **106 citations / 71 distinct spans /
16 live documents / 0 findings**, plus 49 citations in 8 dated snapshots.

**The line-ending rule held.** Six documents and `cite_check.py` were patched at byte level with a
`CR == CRLF` assertion before the write and a `LF`/`CRLF` print after: `docs/00_ar` (283 LF),
`docs/00_en` (277), `docs/02` (101), `docs/05` (141) stayed LF-only and `docs/07`, `docs/09`,
`cite_check.py`, `framework/README.md` stayed pure CRLF — `docs/09` 483 → 534 CRLF, README 435 → 492.
The recount pass that followed patched six more live documents under the same before/after assertion:
`FEATURE_COMPLETENESS_MATRIX.md` 799 → 801, `PRODUCTION_READINESS_REPORT.md` 1 660 → 1 675,
`BASELINE_MEASUREMENTS.md` 342 → 342, the roadmap 614 → 623, `TRACK_INDEX.md` 79 → 82,
`WORKING_CONTEXT.md` 4 335 → 4 335 — each still CR == LF == CRLF, and `docs/07` is CRLF too (107
lines, 5/5 in place), which the four LF-only `docs/00`/`02`/`05` files are not.
Every changed line in the six citation documents is an in-place number swap: `git diff
--numstat` reads 3/3, 3/3, 5/5, 6/6, 5/5 and 52/1, where `docs/09`'s single deletion is the citation
line it replaced and its 52 insertions are the new subsection, and no file's CR count differs from
its LF count after the write — which is the shape a line-ending corruption does not have.

**Docs carried by this track.** `FEATURE_COMPLETENESS_MATRIX.md`: the *Nested routers / independent
sub-apps* row flipped `NOT_IMPLEMENTED` → `VERIFIED_IMPLEMENTED` with an evidence cell naming both
APIs, the per-type (not per-container) shadowing rule, the synthetic entry, the chain order, the
copy-on-write container, the `None` for unscoped routes, the six test names that pin them, the
counts (4 unit + 14 wire + 1 h2c + 1 doctest) and all ten injections including the honest note that
the stream test had to be rewritten before it could kill I10. HEAD was recounted **first** (120 / 4 /
10 / 0 / 0 / 0 / 2 = 136, no drift carried in), then the flip, then the same mechanical re-run:
**121 / 4 / 9 / 0 / 0 / 0 / 2 = 136**, with the census narrative extended to say that tracks 104-106
moved no status and this one moves exactly one row. `docs/09_api_reference.md` gained the Arabic
subsection "حالة المجموعة والموجّه القابل للتركيب" (52 lines: the group-state example, the `Router`
example mirroring the live doctest, five rules, the consume-on-mount constraint, the evidence line).
`framework/README.md` gained "Scoped state and sub-applications" (45 lines) and the feature-list
bullet. Its first draft of the example mounted one router twice, which does not compile — caught
while writing this section, replaced with two routers, and the constraint is now stated in the prose.
That paragraph was not the only thing the first draft left stale: three live documents still described
earlier trees, and `framework/README.md` was the worst of them, carrying the track-103 reading (736 /
63 lines / 26 targets) while the tree behind it had already gone through 750 and 756. The recount sweep
touched six documents plus the README. `FEATURE_COMPLETENESS_MATRIX.md`: evidence header and Unit/Integration
rows now read **776 / 0 / 1** over 65 `test result:` lines (420 unit over 21 targets, 340 integration
over 28, 16 doc over 16 runs), its `http2_e2e` line 55 → 56 with the 30 TLS / 22 h2c / 4 hand-framed
split, `aether` 92 → 96 with `app.rs` 17 → 21, and the chain sentence "Track 107 took it from 756 to
776 … (4 + 15 + 1 = 20)". Its per-target inventory had been naming **23 of the 27** targets and 294 of
the 325 integration tests; it now names all 28 and sums to exactly 340, because `nested_routers` 14 was
added and so were the four targets it had silently omitted (`streaming` 11, `streaming_request` 8,
`csrf` 9, `bearer_auth` 3 — 31 tests, which is the gap that closed 294 → 325). `PRODUCTION_READINESS_REPORT.md` gained an **eighteenth reading** and the seventeenth lost its "this
describes the tree now" sentence to "superseded"; its §3 census table moved 120 / 4 / 10 → **121 / 4 /
9**, re-derived by the awk command that section prints rather than by arithmetic, and the paragraph
beside it — which had carried 11 / 4 not-fully-delivered rows while its own table said 10 / 4, a drift
of exactly one row it had inherited from the pre-track-107 tree — now says 9 / 4 and says how it was
read. `BASELINE_MEASUREMENTS.md` carries the same recount, and its table sums to 420 unit with `aether`
at 96. The roadmap's §19.1 matrix reading, §19.7 order table (rows for 104–107) and §19.8's refusal
bullet were re-derived against the 9-row `NOT_IMPLEMENTED` set: six of the nine are refusals or the
owner-deferred compression item, and the three that are not — *Request arena*, *Thread-per-core
runtime*, *Bounded response memory* — are F-19/F-20-shaped work blocked on a profile the owner
measures, or a best-effort charge that deliberately never withholds an answer. `TRACK_INDEX.md` got row
107, its `plan 88–103` range became `88–107`, and its pointer rule gained the line-count consequence this
track learned from a failed gate. `WORKING_CONTEXT.md` stays frozen; only its one live pointer — "the
current per-target table is `BASELINE_MEASUREMENTS.md`, re-measured …" — moved, and it moved into the
single line it replaced, because that file's headings are pointed at by line number.
`framework/README.md`'s suite paragraph was re-derived in the same pass (70 insertions against 13
deletions, all of it in the count prose). Re-running its middleware-chain claim through a *written*
rule — a `#[test]` or `#[tokio::test]` function counts when its own body, or a same-file function it
calls transitively, registers a chain through `use_middleware`, `route_sync_with`, `route_async_with`
or `ws_with` — turned 41 into **50**, and showed the count had never been the same measurement twice:
the 13 it carried for `http_e2e.rs` is **15** under any rule that counts the per-route API, because
`per_route_middleware_wraps_the_upgrade_request_of_its_websocket_route` and
`a_per_route_guard_refuses_a_websocket_upgrade_before_the_handover` bind through `ws_with`, which is
not the call the old eye-count looked for, and `bearer_auth.rs`'s three were not in the set at all.
Track 107 itself contributes `nested_routers.rs` (3) and `http2_e2e.rs`'s 8th. The rule is written next
to the number now, so the next re-run re-measures instead of re-copying.
The register's **E8** row was corrected in the same pass: it still said "Response-body streaming /
`sendfile` — listed as missing parity, not as an optimization", but tracks 99, 100 and 100b delivered
the streaming half (and the matrix's streaming rows are `VERIFIED_IMPLEMENTED`), while the `sendfile`
half is not implemented — `framework/aether/src/static_dir.rs:25` is a `STATIC_CHUNK` read-then-write
loop, not a syscall. The gate column now reads "was 'parity gap, not perf work'; now perf work only,
and no number exists for it", so nobody picks it up as a feature gap.

**Gates on the final tree** (2026-10-30, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`). Three runs, and the first is recorded as a failure: it printed **6 PASS / 1 FAIL**,
and the FAIL was `govtests` — 1 of its 94 tests, `test_track_index.py`'s index-vs-files test, with
three findings that were one cause. This track's `WORKING_CONTEXT.md` pointer edit had wrapped one
line into two, which moved `## Active Track 10` from line 268 to 269, `## Track 11` from 332 to 333
and `## Active Track 15` from 553 to 554, and `TRACK_INDEX.md` points at that frozen file by two
numeric ranges whose both ends the test checks. The fix is the net-zero one — the pointer now occupies
the single line it replaced, so the log is back at 4 335 lines and both ranges are valid again —
rather than shifting the index by one, because the index's own rule is that those numbers are quoted
and not rewritten; the rule now states the consequence. Run two (the tree one pointer line away from
run one) and run three (this tree, after the rule sentence and these corrections) each
printed seven PASS lines: fmt exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`;
tests **776 passed / 0 failed / 1 ignored** across 65 `test result:` lines, split from those
per-target lines as 420 unit over 21 targets + 340 integration over 28 targets + 16 doc over 16 doc
runs, with `aether --lib` at 96, `nested_routers` at 14, `http2_e2e` at 56, `http_e2e` at 73,
`http_conformance` at 7 and `Doc-tests aether` at 13; govtests 94 tests, OK; cite 106 citations /
71 spans / 16 live documents, 0 findings, plus 49 citations in 8 dated snapshots; evidence 33
documents / 0 findings; validate exit 1 with exactly the four pinned findings (PROB-0007 and
PROB-0017 recycled, REQ-0001 allocated three times, REQ-0001 open-vs-history) and nothing else.
Nothing between the three runs touched a `.rs` file, which is why the tests gate read the same 65
result lines and the same totals all three times. Nothing pushed, no amend.

**What stayed unverified.** No allocation or latency figure for the overlay — the chain entry is an
`Arc` clone per scoped request and nothing here priced it. The h2 mount test is h2c only. The
`ws`-through-a-mount test proves the chain survives the upgrade split but does not prove a scoped
value reaches a *subprotocol* callback, and `static_dir` under a mount proves the prefix and the
chain, not the scope's values inside the file server (it has none). Nothing ran on Linux (task 5),
and the README/`docs/09` Rust blocks are illustrative: no gate compiles them, which is exactly why
the double-mount example had to be found by reading it.

**Commit split, and what each commit's tree was measured on.** Three commits, split by the real diff,
no amend, nothing pushed. Commit 1 (`59cb153`) carries `app.rs` +736/−27 and `lib.rs` 2/2 — the
mechanism with its 4 unit tests and its doctest. Its tree, exported with `git archive` plus exactly
those two files to `C:/tmp/t107/c1`, measured fmt exit 0, clippy exit 0 and `cargo test --workspace`
**761 passed / 0 failed / 1 ignored** over 64 `test result:` lines (420 unit over 21 targets, 325
integration over 27, 16 doc over 16 runs) — HEAD's suite plus the 4 unit tests and the 1 doctest that
ride in that commit. Commit 2 (`45ab60a`) carries the 15 wire tests: `nested_routers.rs` (614 lines,
a new target) and `http2_e2e.rs` +47 with no deletions. Its exported tree at `C:/tmp/t107/c2` measured
fmt exit 0, clippy exit 0 and **776 passed / 0 failed / 1 ignored** over 65 lines, `nested_routers` at
14 and `http2_e2e` at 56. Commit 3 is this section, the recount sweep, the re-derived coordinates and
the pins that moved with them.

The two intermediate trees were deliberately not run through `cite` or `evidence`. `app.rs` grew
4 272 → 4 981 lines and the growth is not one offset (see "Citations, re-derived because the tree
moved" above), so every coordinate below an insertion is stale until the docs commit re-derives it —
a gate that reads the docs cannot report honestly about a tree whose docs commit has not happened
yet. The seven-gate ladder belongs to the final tree, and its three runs are recorded above; after
this paragraph, `govtests`, `cite` and `evidence` were re-run because this document is one of the
files they read.

## Track 108 — the document is derived from the table that answers (2026-10-31)

**Defect.** The matrix's *OpenAPI generation* row had been `PARTIALLY_IMPLEMENTED` since track 91,
and the roadmap wrote the reason itself: "OpenAPI is reachable, still a manual builder" (§19.3's
track-91 note, and the F-02 row at §19.4 says only *reachable*). Reachable and derivable are
different claims. Measured before touching anything: `aether-openapi` was one `pub struct` pile with
**1 unit test** (`the_document_uses_the_specs_field_names`, which pins track 91's D-19 rename), no
function anywhere in the workspace that produced a document from a server, and a crate header that
promised "Generates full OpenAPI JSON Schemas at build time or startup" — nothing generated one, so
that sentence was a claim about a file nobody had run.

Two shapes in the type made derivation impossible rather than merely unwritten:

* `PathItem` was `{ get: Option<Operation>, post: Option<Operation> }` — **two fields**. The router's
  method vocabulary is nine named variants plus `Custom(u8)` (`framework/http/src/method.rs:10-21`),
  so a `PUT`, `DELETE` or `PATCH` registration had nowhere to be written, and a generator iterating a
  route table would silently omit every verb the struct did not name. The old
  `an_openapi_document_is_generated_through_the_facade` had to write `post: None` beside the route it
  was describing, which is the same kind of assertion that can only ever go green.
* Nothing could say where a path's parameters came from. `RadixRouter::for_each_route` reports a
  `HandlerId` and a bitmask — enough for the `Allow` value track 96 built, not enough to name a
  capture — and no operation carried `parameters`, and the only response set any caller could reach
  was a hand-written `200`. A derived `200` on a `ws` route documents an answer the server never
  sends.

Also measured: `framework/openapi/Cargo.toml` declared `aether-core = { path = "../core" }` and the
crate's 98 lines named `aether_core` **zero** times, so describing an API pulled the arena, the CCB
and the diagnostics engine into the build.

**Decisions, and the weaker option each refused.**

* **The router enumerates its own table.** `RadixRouter::route_listing() -> Vec<RegisteredRoute>`
  (`framework/router/src/trie.rs:245`, `RegisteredRoute` at `trie.rs:33`) reconstructs each
  registration's `(method, canonical path template, handler id)` by a DFS that records the dynamic
  levels on the way down. Refused: walking `AetherApp`'s own maps — `handlers`/`ws_handlers` are
  `HashMap<u32, …>` keyed by handler id, so the app holds *ids*, and the path, the verb set and the
  capture names exist only in the trie; a second table in the app is a second source of truth that
  can disagree with the one answering requests. Refused: widening `for_each_route`, whose callback
  cannot name a route's parameters and whose consumer is the `Allow` builder.
* **Each route is listed under its own names**, stamped per method from the leaf
  (`RouteNode::param_names_for(method)`), because one level holds one `Param` child for every route
  competing for it and that child keeps whichever registration created it — track 71's finding
  (PROB-0031) is exactly the hazard that makes node prefixes untrustworthy here. Refused: echoing
  the node prefix, which yields a document whose parameter name no request can fill (M1, M3).
* **The listing is the canonical form `find` matches**, not the source text: `/users`, `/users/` and
  `users` all list as `/users`, `""` and `/` list as `/`, and a bare `*` lists under the default name
  its registration gave it (`/x/*` → `/x/*wildcard`). Refused: carrying the declaration string, which
  would describe a path the matcher does not use.
* **The `(path, method)` sort is in the contract, not a courtesy.** Siblings are ordered by node kind,
  so an unsorted walk lists `/b` before `/a` according to which was registered first — and the
  `operationId` collision suffixes would then depend on that order too. A build artifact that gets
  diffed in review cannot move because a `use` statement moved.
* **`RouteDeclaration { method, path, websocket }` is the boundary**, three `&str` and a bool, so
  `from_routes` needs nothing from the runtime; the `aether-core` line left `Cargo.toml` in the same
  edit and the crate now depends on serde and serde_json alone. Refused: `from_routes(&RadixRouter)`,
  which puts a router implementation inside a documentation crate and makes every consumer of the
  document pay for the trie.
* **`PathItem` became `#[serde(flatten)] methods: BTreeMap<String, Operation>`** keyed by the
  lowercase verb OpenAPI spells. Refused: adding fields until the struct covers nine verbs, which is
  the same defect one `Option<Operation>` at a time; refused a `HashMap`, for the reason the `paths`
  map already gives in its own comment.
* **A catch-all is emitted as `{name}`**, because OpenAPI has no wildcard path form and a document
  saying `/assets/*filepath` names a template a client cannot fill in (M4).
* **`operationId` collisions get `_2`, `_3` from the sorted order** rather than being dropped or
  emitted twice — `/a/b` and `/a_b` both sanitise to `get_a_b`, and a duplicate key is an invalid
  document while a dropped route is a false one.
* **The `ws` half is decided by the handler, not the method**: `self.ws_handlers.contains_key(&route.handler)`
  (`framework/aether/src/app.rs:1674`), since a WebSocket route is registered as `GET` and answers the
  handshake. Refused: trusting `Method::Get`, which would promise `200` on an upgrade (M6, M7).
* **No handler is consulted, and the document says so.** The `200` description states the route table
  records no schema, media type or body. Refused: emitting `application/json` with an empty object
  schema for every route — that is a claim about code the generator never read.

**Tests.** 19 added, and the suite moved **776 → 795** (430 unit over 21 targets + 349 integration
over 29 + 16 doc over 16 runs, 66 `test result:` lines):

* 4 unit in `framework/router/src/trie.rs`: `route_listing_rebuilds_each_declared_template`,
  `route_listing_stamps_shared_levels_with_each_routes_own_names`,
  `route_listing_reports_the_canonical_form_of_each_declaration`,
  `route_listing_is_independent_of_registration_order` (`aether-router` lib 26 → 30).
* 6 unit in `framework/openapi/src/lib.rs`: `a_catch_all_is_derived_as_a_path_parameter`,
  `the_derived_document_does_not_depend_on_input_order`,
  `operation_ids_are_unique_when_two_paths_sanitise_alike`,
  `a_websocket_registration_answers_the_upgrade_and_its_refusals`,
  `nothing_in_a_derived_document_claims_a_body_schema`, `a_malformed_slot_declares_no_parameter`
  (`aether-openapi` lib 1 → 7, the crate's first real coverage).
* 1 proptest property in the new fifth property of
  `framework/router/tests/router_properties.rs`:
  `the_route_listing_returns_every_registration_with_its_own_names` — for generated trees it asserts
  the listing returns every registration, each under names the *matcher* really extracts, in sorted
  order (target 4 → 5).
* 8 wire in the new target `framework/tests/tests/openapi_derivation.rs` (262 lines):
  `the_document_covers_every_route_the_app_registered`,
  `a_shared_dynamic_level_is_named_per_route`,
  `a_group_and_a_mount_contribute_their_prefixed_routes`,
  `a_static_dir_is_listed_as_the_catch_all_it_registers`,
  `a_websocket_route_carries_the_upgrade_instead_of_a_twenty`,
  `the_document_omits_the_answers_that_have_no_registration`,
  `the_document_is_the_same_whatever_order_the_routes_were_registered_in`,
  `the_derived_json_carries_the_specs_keys`. Three of the eight go through the registration kinds
  that do not come out of the plain route path — a `group` prefix, a `Router` mount (track 107) and a
  `static_dir` catch-all — which is where a listing that reads the app instead of the trie would show
  up.
* `framework/tests/tests/facade_wiring.rs`'s `an_openapi_document_is_generated_through_the_facade`
  was rewritten for the verb-keyed item and gained an assertion on the emitted key
  (`"get": {`), because the old one could not fail for a document that lost a `PUT`.

**Sensitivity, proved by injection, each restored byte-identically.** The ladder
(`C:/tmp/t108/inject.py`, evidence `C:/tmp/t108/inject.json`) rewrote one line of
`framework/router/src/trie.rs`, `framework/openapi/src/lib.rs` or `framework/aether/src/app.rs` at a
time, ran `cargo test -p aether-router`, `-p aether-openapi` and
`--test openapi_derivation`, then restored the file and re-asserted its sha256 against the baseline
captured before the first mutant. **Eight mutants, eight kills, no forced restore, every hash
identical after every mutant.** No kill was a compile error (`could not compile` classified
separately), so every one is a test that ran and refused.

* M1 — the listing reads no per-route names: `route_listing_is_independent_of_registration_order`,
  `route_listing_stamps_shared_levels_with_each_routes_own_names` (`aether-router` 28 passed / 2
  failed) plus `a_shared_dynamic_level_is_named_per_route` and
  `the_document_is_the_same_whatever_order_the_routes_were_registered_in` (derivation 6 / 2).
* M2 — the listing is not sorted: all four `route_listing_*` tests (26 / 4). **The derivation target
  stayed 8 / 0** — see the honest half below.
* M3 — a catch-all is rendered as a parameter: three `route_listing_*` tests (27 / 3), and again
  **8 / 0 in the derivation target**.
* M4 — the generator leaves a catch-all unrewritten: `a_catch_all_is_derived_as_a_path_parameter`,
  `a_malformed_slot_declares_no_parameter`, `the_derived_document_does_not_depend_on_input_order`
  (openapi 4 / 3) and `a_static_dir_is_listed_as_the_catch_all_it_registers` (derivation 7 / 1).
* M5 — no parameters are derived: `a_catch_all_is_derived_as_a_path_parameter` and
  `nothing_in_a_derived_document_claims_a_body_schema` (openapi 5 / 2) plus five of the eight wire
  tests (derivation 3 / 5).
* M6 — every operation gets the HTTP status set: `a_websocket_registration_answers_the_upgrade_and_its_refusals`
  (openapi 6 / 1) and `a_websocket_route_carries_the_upgrade_instead_of_a_twenty` (derivation 7 / 1).
* M7 — the app classifies upgrades from `handlers` instead of `ws_handlers`:
  `a_websocket_route_carries_the_upgrade_instead_of_a_twenty` (derivation 7 / 1).
* M8 — the generator trusts the caller's order (`let sorted = routes.to_vec();`):
  `the_derived_document_does_not_depend_on_input_order` (openapi 6 / 1).

**The honest half, recorded not smoothed.** M2 and M3 — the two defects injected into the *router's*
listing — are killed only by `aether-router`'s own tests; the app-level derivation target reports
8 / 0 under both, because `from_routes` re-sorts the slice and re-templates each path itself, so the
document cannot see a listing that arrived unsorted or with a `*name` in it. That is a real property
of this design (the generator defends its own output shape) and it means a router regression of
those two kinds is caught one layer below the consumer, not at it. The reverse direction has no such
gap: M1 is caught in both layers, and every generator-side mutant (M4-M8) reaches the wire target.

**Citations, re-derived because the tree moved.** `app.rs` grew by exactly 30 lines (the
`openapi_spec` method and its rustdoc, inserted above `static_asset`), and `RegisteredRoute` adds 9
lines at the top of `trie.rs` — which is the only growth above any cited span there, since
`route_listing` and its four tests sit below both `trie.rs` pins. **20 PINS keys moved in
`cite_check.py`** and each was re-matched against the code at its new coordinate rather than offset by
arithmetic; 29 citation tokens in six live documents
(`docs/00_ar`, `docs/00_en`, `docs/02`, `docs/03`, `docs/05`, `docs/07`) were renumbered with them —
`app.rs:2275` → `:2305`, `2361-2567` → `2391-2597`, `:2886` → `:2916`, `:2936` → `:2966`,
`:4197` → `:4227`, `trie.rs:55` → `:64` and the rest by the same re-match. The census after the
sweep, from the gate itself: **106 citations / 71 distinct spans / 16 live documents / 0 findings**,
plus 49 citations in 8 dated snapshots; `evidence_check` reads 33 documents, 0 findings.

**Line endings: the rule, the break, the repair.** Eighteen documents plus `cite_check.py` carry
this track's edits and every one was written at byte level. The rule (`CR == LF` asserted before
the write and printed after) held for the rewrite-and-recount pass: `cite_check.py` 660/660
unchanged, `framework/README.md` 492 → 496, `FULL_FRAMEWORK_DOCUMENTATION_AR.md` 1 093 → 1 147,
`AETHER_CORE_CAPABILITIES.md` 90 → 94, `docs/09_api_reference.md` 534 → 557, and
`BASELINE_MEASUREMENTS.md`, `FEATURE_COMPLETENESS_MATRIX.md` and `PRODUCTION_READINESS_REPORT.md`
(1 675 → 1 687) each still CR == LF == CRLF.

It did not hold for the renumber pass, and the census over `git diff --name-only` — not each
script's own print — is what showed it: five of the six documents whose citation tokens moved
landed as pure LF. `docs/00_framework_specification_ar.md` CR 0 / LF 283,
`docs/00_framework_specification_en.md` CR 0 / LF 277, `docs/02_tcp_framing_and_buffering.md`
CR 0 / LF 101, `docs/03_routing_and_radix_trie.md` CR 0 / LF 88,
`docs/05_request_context_and_memory.md` CR 0 / LF 141, while `docs/07_limits_and_dos_mitigation.md`
(107/107), the sixth of that pass, kept its endings; the untouched siblings in the same directory
(`docs/01`, `docs/04`, `docs/06`, `docs/08`) were CRLF, so the drift was visible rather than
arguable. Repaired before the commit by `b"\n"` → `b"\r\n"` with `assert cr == 0` first and
`CR == LF` plus `out.replace(b"\r\n", b"\n") == raw` after: 283/283, 277/277, 101/101, 88/88,
141/141; byte counts 21 762 → 22 045, 17 500 → 17 777, 5 160 → 5 261, 3 885 → 3 973,
8 398 → 8 539; and `git diff --numstat` unchanged at 3/3, 4/4, 5/5, 2/2, 3/3 for those five
files, which is the proof that no content moved. What the drift could not do is corrupt the
committed tree: `git ls-files --eol` reports `i/lf` for every document under `docs/`
(`core.autocrlf=true` keeps the index LF), so the defect was a worktree-shape divergence and a
false claim in this paragraph's own earlier wording — not a wrong byte in the repository. The
three document gates were re-run after the repair and printed what they printed before it:
govtests Ran 94 tests OK, cite 106 citations / 71 spans / 16 live documents / 0 findings plus 49
citations in 8 dated snapshots, evidence 33 documents / 0 findings.

**Docs carried by this track.** `FEATURE_COMPLETENESS_MATRIX.md`: *OpenAPI generation* flipped
`PARTIALLY_IMPLEMENTED` → `VERIFIED_IMPLEMENTED` with an evidence cell naming both APIs, what the
document derives, the **Stated limits** (no schema because no handler is read; `HEAD` and the generic
`OPTIONS` absent because they have no registration; two `Method::Custom` routes on one path collapse
because the verb map keys by method name) and all fifteen pinning test names, and the census was
re-run through the PRR §3 command rather than added to: **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136**, the
three remaining partials being *README / docs accuracy*, *Zero-copy head parsing* and *SIMD-style
validation*. `PRODUCTION_READINESS_REPORT.md` gained a **nineteenth reading** and the eighteenth lost
its "describes the tree now" sentence to "superseded"; its §3 table and the paragraph beside it now
come from one run of the same command (121 / 4 → 122 / 3). `BASELINE_MEASUREMENTS.md` carries the
795 recount, `aether-router` 26 → 30, `aether-openapi` 1 → 7, `router_properties` 4 → 5, the new
`openapi_derivation` target, and its analysis bullet no longer lists `aether-openapi` among the
zero-test crates. `framework/README.md`'s suite paragraph was re-derived (795 / 430 / 349 / 66 lines /
29 targets, the new target named, `router_properties` 4 → 5, and the stale "736 while the tree said
750/756/776" chain extended to 795). `FULL_FRAMEWORK_DOCUMENTATION_AR.md` §13 taught a removed API —
`item.get = Some(get_users)` does not compile on this tree — and now teaches the derived document
first with its limits, keeping the hand-built path as (ب) rewritten for the verb-keyed item; the crate
table row (line 57) lost its "المخطط لا يُولَّد من المسارات تلقائياً" claim.
`AETHER_CORE_CAPABILITIES.md` gained a protection row for the derivation, including the router-only
half of the mutation result, and its wiring note now says `aether::openapi` is more than a data model.
`docs/09_api_reference.md` gained the `app.openapi_spec` entry (534 to 557 lines) under the
`AetherApp` methods: the derivation, the four things a route table cannot say (no body or response
schema because no handler is read, `HEAD` and the generic `OPTIONS` absent because they have no
registration, a `ws` route answering `101`/`400`/`426`, and no `GET /openapi.json` served because
publishing an internal API shape is the user's decision), the sorted-`(path, method)` promise, and
the evidence line naming the 8 wire tests, the 6 openapi unit tests, the 4 `route_listing` tests and
the router property.

**Compile-verified, because no gate does it.** Neither `cargo test` nor `cite_check` compiles a
document's rust blocks, so both §13 blocks were checked in a scratch crate under `C:/tmp` with a path
dependency on `framework/aether`: `cargo check` exit 0, no warnings, 2026-10-31. The same check
is what rejected the block's old line `item.get = Some(get_users)`, and the rejection was worth
reading carefully: that assignment *did* build on the tree the paragraph was written on, whose
`PathItem` really had `get` and `post` fields, so it was track 108's verb-keyed map that turned
it into a phantom API — which is why (ب) was rewritten in this same pass rather than left
teaching an example that no longer builds.

**Residual, named not fixed.**

* The document has no `servers`, `tags`, `security` or `components` — `OpenApiSpec` carries
  `openapi`, `info` and `paths` and nothing else. A user who wants them builds them onto the derived
  value; the derivation covers the part a hand-written document gets wrong.
* Nothing serves the document. There is no `GET /openapi.json` on the app, deliberately: what a route
  returns is a policy question (an internal API's shape is not something to publish by default), and
  `openapi_spec()` hands back the value so the choice stays the user's.
* No validator was run: the workspace has no OpenAPI schema validator, so the keys are pinned by name
  (`the_derived_json_carries_the_specs_keys`, `the_document_uses_the_specs_field_names`,
  `facade_wiring`) rather than proved against the 3.1 meta-schema.
* `HEAD` is not in the document even though the server answers it for every `GET`, and the generic
  `OPTIONS` is not either — both are frontend answers with no registration to list. `the_document_omits_the_answers_that_have_no_registration`
  pins that as the behaviour, not as an oversight.
* Two `Method::Custom` registrations with the same method name on one path collapse into one
  operation, because the verb map is keyed by name. Nothing in the router lets those two coexist
  usefully, but the document does not say so either.
* No performance number was measured by this track at all, and nothing ran on Linux (task 5). The
  listing is a `Vec` built on demand — an allocation shape, not a measurement.

**Gates on the final tree** (2026-10-31, Windows 10.0.26200 x64 MSVC, `bash gates.sh`, logs in
`target/gates/`). Run one printed seven PASS lines and no failure, so none is recorded as one: fmt
exit 0; clippy exit 0 over `--workspace --all-targets -- -D warnings`; tests **795 passed / 0 failed /
1 ignored** across 66 `test result:` lines, split from those per-target lines as 430 unit over 21
targets + 349 integration over 29 targets + 16 doc over 16 doc runs, with `aether-router --lib` at 30,
`aether-openapi --lib` at 7, `router_properties` at 5, the new `openapi_derivation` at 8,
`facade_wiring` at 4, `http_e2e` at 73, `http2_e2e` at 56, `nested_routers` at 14 and `scheduling` at
18; govtests 94 tests, OK; cite 106 citations / 71 spans / 16 live documents, 0 findings, plus 49
citations in 8 dated snapshots; evidence 33 documents / 0 findings; validate exit 1 with exactly the
four pinned findings (PROB-0007 and PROB-0017 recycled, REQ-0001 allocated three times, REQ-0001
open-vs-history) and nothing else. Five ladder runs sit behind this paragraph and every one printed
the same seven PASS lines with the same totals: run one on the tree one append away from this
section's two closing paragraphs, run two after them, run three after the
`docs/09_api_reference.md` entry the same recount produced, run four after the correction to the
compile-check sentence above, run five after the line-ending repair described there. `govtests`
(Ran 94 tests, OK), `cite` (106 citations / 71 spans / 16 live documents / 0 findings, plus 49
citations in 8 dated snapshots) and `evidence` (33 documents / 0 findings) were also run standalone
between and after those, because this document is one of the files they read. **Run six is the tree
this commit is made on:** the whole ladder, after the last change to these words. If any number in
it had moved, this section would have been rewritten and the ladder run again rather than the
difference committed.

**Commit split, and what each commit's tree was measured on.** Three commits, split by the real diff,
no amend, nothing pushed. Commit 1 (`c40ae9f`) carries the mechanism and the tests that can run
without a server: `app.rs` +30/-0, `trie.rs` +224/-0, `router/src/lib.rs` 2/2, `openapi/src/lib.rs`
+479/-18, `openapi/Cargo.toml` -1 (the unused `aether-core`), `Cargo.lock` -1, `router_properties.rs`
+137/-7 and `facade_wiring.rs` 19/13 — 8 files, 891 insertions / 42 deletions. Its tree, exported with
`git archive` of HEAD plus exactly those eight files to `C:/tmp/t108/c1`, measured fmt exit 0, clippy
exit 0 and `cargo test --workspace` **787 passed / 0 failed / 1 ignored** over 65 `test result:` lines
(430 unit over 21 targets, 341 integration over 28, 16 doc over 16 runs): HEAD's 776 plus the 11
tests that ride here — 4 `route_listing_*` unit, 6 `aether-openapi` unit, 1 router property — and the
unit column already carries the final tree's 430 because every generator-side test is a unit test.
Commit 2 (`93458ca`) is the 262-line wire target alone (262/-0), and its exported tree at
`C:/tmp/t108/c2` measured fmt exit 0, clippy exit 0 and **795 / 0 / 1** over 66 lines with
`openapi_derivation` at 8. Commit 3 is this section, the recount sweep, the re-derived coordinates and
the 20 pins that moved with them.

The two intermediate trees were deliberately not run through `cite` or `evidence`. `trie.rs` grew 1 095
to 1 319 lines and `app.rs` 4 981 to 5 011, so every coordinate below an insertion is stale until the
docs commit re-derives it — a gate that reads the documents cannot report honestly about a tree whose
documents have not been written yet. What the two trees do prove is the split's own claim: the
mechanism plus its unit tests compiles, lints and passes without the wire target, and the wire target
is exactly what moves 787 to 795 and 65 result lines to 66. `__pycache__/` under
`.project-governance/tools/` stayed untracked rather than being added by a sweep.

## Track 109 — a green citation can still teach an API that does not exist (2026-10-31)

**Defect.** `framework/docs/ERROR_HANDLING_GUIDE_AR.md` line 178 tells a reader that in `production`
mode the framework hides internal paths, redacts passwords, API keys and tokens, and returns a safe
JSON answer to the client that carries only `code` and `error_id`. The answer this tree builds, at
`framework/aether/src/app.rs:254`, carries `error` and `code`. There is no `error_id` anywhere in the
workspace: not a field, not a key, not a string literal. The page's own citation was green — the file
resolved, the lines were in bounds, the pinned regex matched its span. That is the defect class: a
coordinate states *where* to look and an anchor regex checks the *shape* of what is there, while the
sentence around it makes a claim about a *symbol*, and nothing in the gate read the symbol. A reader
following that guide writes a client that reads a field the server never sends.

**Measured before anything was written.** The prototype ran the name rules over the 16 documents the
citation pass already read and left 35 mentions the tree did not carry. Reading each one turned
exactly one of them into a document that was wrong — the `error_id` sentence above — and the other 34
into 28 per-document reasons, because they were one of four legitimate shapes: a name written in
order to deny it (`with_json`, `add_dependency`, `with_limits`), an example value rather than an
identifier (`req_000000000001`), a parameter in prose that spells a signature (`heap_bytes`,
`status_code`), or another crate's API cited by name (`validate_outbound_headers`, `status_request`,
`owned_is_unique`). Nine further mentions were attributed to an external crate inside their own span
and never judged. On the final tree the census is 361 qualified and 1256 bare names resolved over the
27 live documents, 34 mentions sheltered by 28 exceptions, 9 attributed, 1 document exempted as a
proposal, 0 findings.

**Decisions, and the weaker option each refused.**

* **The names pass lives inside `cite_check.py` and inside the existing `cite` ladder step.**
  Refused: an eighth gate with its own tool. Two tools would each need a corpus, a snapshot policy and
  an exemption table, and the thing that must not diverge between them is the corpus. A test
  (`test_both_document_gates_read_the_same_corpus`) now asserts `cite_check.DOC_GLOBS ==
  evidence_check.DOC_GLOBS`, which is also what pulled both READMEs inside: the repository's first
  page states its loudest claims and no gate had ever read it.
* **`Head::leaf` is judged only when `Head` names something this tree declares.** Refused: judging
  every `A::B` span, which would make the gate a catalogue of `std`, `tokio`, `h2` and `clippy` APIs
  and get itself muted within a week. Refused: the weaker version — skip the leaf wherever it appears
  once a span mentions an external head — because that is a laundering channel: a document could
  exempt any invented name by writing `tokio::` beside it. So the leaf is recorded as attributed
  *within that span only*, and a bare `snake_name` in the same span is still judged, which is exactly
  why `type_complexity` costs an exception instead of a silent skip.
* **A bare `snake_name` is judged in every other span.** This is the rule that catches an invented
  API, because an invented method is also an invented name. It is deliberately the stricter of the
  two rules and it is why the exception table is per-document: a reason that holds on one page says
  nothing about another.
* **Spans inside fenced blocks are skipped, and a span containing a slash is a path.** A fence is the
  document's own example, not a claim about the tree; a path is already resolved by the citation pass
  and refused outside the repository by `evidence_check.py`. Judging `fuzz_targets` as an identifier
  would only repeat their verdict in the wrong vocabulary.
* **Every unresolved name needs a recorded human reason, and no document can write its own.**
  `NAME_EXCEPTIONS` is keyed (document, name) with the sentence it excuses, and an entry that shelters
  no live mention is itself a finding — shelter cannot outlive the sentence it was written for. A
  design document is exempted only by a `PROPOSALS` listing *and* a marker in the document, so the
  exemption is a reviewed edit; an unlisted document carrying the marker is refused. The one
  exemption in the tree is `ADAPTIVE_TREE_SCHEDULING_DESIGN.md`, which names the fields it proposes to
  build.
* **The qualified rule asks only what the index can answer reliably.** Members are collected by
  brace-matching `impl`, `enum` and `struct` bodies without a parser, so the member set is not
  provably complete (a `r#"..."#` sample body, a derived impl, a trait impl in another file). A gate
  that invents findings is a gate that gets ignored, so `Head::leaf` asks whether the tree carries the
  leaf at all, and the bare-name rule is what actually catches an invented API.

**The gate had been sheltering itself through a channel nobody had looked at.** Its own docstring
already said the index is built only from the product's sources, because the sentence that reports
`error_id` as missing must not be the sentence that makes `error_id` exist. The implementation did not
hold that line: it stripped only string literals before harvesting identifiers, and a comment is a
prose channel too. Six genuine exception records — `with_json`, `base_dir`, `with_logger`,
`req_000000000001` — were reported as stale, because the tool had written those names down in the
reason column of its own table. The first repair (index nothing outside `framework/`) traded six false
findings for true findings on names the repository really has, which documents may legitimately cite:
`cite_check`, `evidence_check`, `test_recycling_ratchet`. The rule that closes both: a file outside
the product contributes its stem and its `def`/`class` names and nothing else; only sources under
`framework/` contribute declarations, identifiers and string-literal contents. That rule restored 0
findings and reproduced the census above exactly, which is how it was shown to be the right bound
rather than a quieter one.

**Sensitivity: nine injections, each reverted byte for byte.** A gate is only as good as the mutants
it rejects, so each of these was run against the live tree, its output matched against the expected
finding, and the file restored with its sha256 re-asserted; after all nine the gate re-read exactly
the baseline census.

* a phantom qualified member added to the Arabic guide's prose — rejected: `AetherApp::openapi_spec_typo`
  is not declared and no exception shelters it;
* a phantom bare name added to the same page — rejected: no name in the tree is `recycles_the_names_pass`;
* **the tree moved under an unchanged document**: `openapi_spec` renamed in both files that declare
  and call it — rejected, which is the case a document-only gate cannot see;
* a live document that added a proposal marker to exempt itself — rejected: only `PROPOSALS` may list
  one;
* a `NAME_EXCEPTIONS` entry that shelters nothing — rejected as a stale exception;
* a listed proposal with its marker deleted — rejected, so the two halves of the exemption are both
  load-bearing;
* the `error_id` sentence put back while the tool's own comment also wrote the name down — still
  rejected, which is the self-shelter rule proven rather than asserted;
* two negative controls that stayed clean, so the rules do not overreach: a phantom inside a fenced
  block, and a path span (`framework/fuzz/fuzz_targets`).

The fourth and fifth of these are the ones that matter most: before them, both escape hatches were
only described in comments.

**What the READMEs said, and whether it held.** Bringing them into the corpus made their claims
checkable, so they were checked rather than trusted: the root page's **795 passed / 0 failed / 1
ignored** re-measured true on this tree, its **18 crates** is the workspace's own `members` list, and
every per-suite figure it prints came out of the same run — 73 `http_e2e`, 56 `http2_e2e`, 18
`scheduling`, 4 + 5 property, 8 `openapi_derivation`, 2 `secret_rendering`.

**Gates.** `bash gates.sh` on the tree that carries both READMEs, the names pass, the guide's
correction and this record: fmt, clippy, tests, govtests, cite, evidence and validate all as expected
- `cargo test --workspace` **795 passed / 0 failed / 1 ignored** over 66 `test result:` lines,
govtests **112 OK** (cite tests 42 → 43), cite **106 citations in 16 live documents, 71 distinct
spans, 0 findings**, names census above, **49 citations in 8 dated snapshots** (marked, not
validated), evidence **35 documents, 0 findings**, validate exit 1 with only the four pinned
historical findings. The two intermediate commits moved no Rust code, so the cargo readings are the
same reading for all of them; the documents that the doc gates read are final in the first of those
commits, and the register — which no gate reads — is the only difference between them.

**Commit split.** Five commits, split by the real diff, no amend, nothing pushed: `f16e9f7` brought
both READMEs into both document globs and made the two corpora identical by test (4 files, 48
insertions / 11 deletions, measured on its own tree at 106 / 71 / 16 / 0 and 33 → 35 documents,
govtests 94 → 96); `94b9fba` is the names pass itself plus the guide's correction, the proposal
marker and the ladder's wording (5 files, 584 / 14); `231b94a` adds the source-rename sensitivity
test (+9); the fourth is this track's sweep of the two live documents it names (the matrix's *README /
docs accuracy* row and the readiness report's reading note); this record is the fifth.

**What this track does not claim.** The pass proves a live document does not *invent* an API — it does
not prove the sentence around the name is true, and a claim about behaviour is only as good as the test
that runs it. It reads backticked spans, so a name written in plain prose is not judged. It cannot say
whether an external crate really has the member a document attributes to it; it only declines to
judge, and the nine attributed mentions are counted so that a future reader can see the size of that
blind spot. The eight dated snapshots stay unvalidated on purpose, and two of them stay wrong behind a
superseded banner. No matrix row moved: *README / docs accuracy* remains
`PARTIALLY_IMPLEMENTED`, for the two reasons the row now states.

## Track 110 — the request `content-length` promise, audited on both frontends (2026-10-31)

**Question asked.** RFC 9113 §8.1.1 makes a request `content-length` that does not equal the sum of
its stream's DATA payload lengths a malformed request, and §8.2.1 forbids padding, non-digits, an
overflowing value and a repeated field whose lines disagree; RFC 9112 §6.3.2.2 states the same promise
for HTTP/1. Tracks 105 and 106 had taken the request-target forms and the `:authority` precedence off
that pair of documents and left the body rule alone. The suspicion was concrete: this frontend reads
`content-length` in exactly one place — `declared_length` (`framework/aether/src/http2.rs:594`) — and
uses it only as a budget, so a stream whose body did not match its head could reach a handler holding
the wrong number of bytes.

**Measured, before anything was written.** Answered on the wire, not by reading the dependency's
source. Every shape under test is illegal, so no conforming client will send it: the frames were
written by hand onto an h2c socket (`handframed`, `framework/tests/tests/http2_e2e.rs:2717`) and the
eight refusals share one helper, `lying_length_is_reset` (`framework/tests/tests/http2_e2e.rs:3466`),
which asserts the wire carries `RST_STREAM` with error code 1 (`PROTOCOL_ERROR`), that it is the only
frame after the head, that no `GOAWAY` follows, and that **nothing was answered** — `answered == 0`,
not merely "no body".

**Result: no defect, on either frontend.** Over HTTP/2 the `h2` 0.4.20 receive path already enforces
the whole list, each as a reset on that stream and never as an answer: non-digits
(`a_non_numeric_content_length_is_reset_over_h2`), padding
(`a_padded_content_length_is_reset_over_h2`), a value too large to name a length
(`an_unparseably_large_content_length_is_reset_over_h2` — refused, not wrapped), a repeated field whose
lines disagree (`conflicting_repeated_content_lengths_are_reset_over_h2`) where the agreeing pair is
served (`matching_repeated_content_lengths_are_served_over_h2`), `END_STREAM` on the head while bytes
are still owed (`a_head_that_promises_bytes_and_ends_where_it_is_reset_over_h2`), a body longer than
the promise (`a_body_longer_than_its_declared_length_is_reset_over_h2`) and shorter than it
(`a_body_shorter_than_its_declared_length_is_reset_over_h2`), and `content-length` beside
`Transfer-Encoding` (`a_content_length_beside_transfer_encoding_is_reset_over_h2`). Two more pin what
this frontend does own: a request needs no declared length to be served
(`an_h2_request_needs_no_declared_length_to_be_served`), and a declared length over
`max_stream_body_bytes` is answered `413` *before* dispatch, including when the promise arrives as a
repeated field (`a_repeated_declared_length_over_the_streaming_ceiling_is_413_before_dispatch`,
joining the single-value h2 case that already existed). Over HTTP/1 the rule is this framework's own and now
has five wire tests: any second `Content-Length` line is a `400` even when it agrees
(`a_repeated_content_length_is_refused_on_http_1_even_when_it_agrees`), OWS padding is read
(`an_ows_padded_content_length_is_served_on_http_1`) while a non-ASCII pad is refused
(`a_non_ascii_padded_content_length_is_refused_on_http_1`), and the body stops at the declared length
so the surplus **is** the next message
(`a_body_longer_than_its_declared_length_leaves_the_surplus_for_the_next_request`) unless it is not a
request, in which case the connection gets `400` and closes
(`bytes_past_the_declared_length_that_are_not_a_request_are_400_and_close`). The same stop-at-promise
on the streamed lane: `a_streamed_lane_stops_at_its_declared_length_and_the_surplus_is_the_next_request`
and `a_streamed_lane_whose_surplus_is_not_a_request_is_400_and_close`.

**What the audit found instead — three things, all real.**

* **A shipped rule with no test of its own, found only by mutation.** The buffered entry point
  `RequestView::parse` (`framework/http/src/parser.rs:44`) frames the body from the declared length,
  and the mutation that defeats that (`total = max(total, buf.len())`) left **all five new HTTP/1 wire
  tests green** — the serving loop reads through `parse_request_incremental` (`parser.rs:611`), so the
  wire cannot reach this function. What went red was one pre-existing unit test named for something
  else (`test_consumed_offset_is_exact_for_pipelining`) — the rule was pinned incidentally, by a test
  about the consumed-byte count, and nothing asserted that the *body* is the promise. That is now
  `test_content_length_surplus_is_left_for_the_next_request` (`parser.rs:975`). Refused: a wire test for
  it, which cannot exist — the entry point is off the wire, and inventing one would have meant
  pretending `RequestView::parse` is what the app calls.
* **`ows_trim`'s seam is narrower than its comment claimed.** Deleting the call so the declared length
  is read untrimmed stayed **green on both the wire and the parser unit test**: `httparse` already
  trims ASCII OWS on either side of a header value, so the helper is not what makes ` 5 ` parse. What
  does hold the line is the refusal of a *non*-ASCII pad: replacing it with `str::trim()` turned
  `a_non_ascii_padded_content_length_is_refused_on_http_1` and
  `test_framing_headers_accept_only_ascii_ows` red while the SP/HTAB acceptance stayed green. Recorded
  as the null arm it is, because a sensitivity run that only reports kills teaches nothing about which
  seam carries the rule.
* **Two documents were wrong, and one of them was wrong in a way coordinates could not see.**
  `framework/docs/AETHER_CORE_CAPABILITIES.md` said `RequestView::parse` "is called only by the unused
  `framework/runtime/src/worker.rs:32`". Both halves are false: that worker loop is exported by the
  runtime crate and driven by its own `connection.rs` test target, and the fuzz oracle
  (`framework/fuzz/src/lib.rs:87`), the `parser_properties` suite and the `aether-tests` harness helpers
  call the function directly. The readiness report said 49 HTTP/2 wire tests where the file holds 67,
  and 13 `http_e2e` tests that register a WebSocket route where the number is 15. The gate track 109
  built caught the *name* half of this only because the recount put the sentences in front of it.

**Decisions, and the weaker option each refused.**

* **Test the dependency's promise instead of re-implementing it.** Refused: a second `content-length`
  comparison inside `serve_request`. The bytes it would reject are never delivered, so the branch
  could not be tested from any client, and a check that agrees with the library always and disagrees
  never is not defense — it is dead code with a security comment on it. Mutation proved the same
  point from the other side: making `declared_length` read the *last* value of a repeated field instead
  of the first kept all 12 length-related h2 tests green, because a disagreeing pair is reset before
  the function is ever called. Recorded as a null arm, and it is the reason the doc comment now says
  the function's safety *rests on* the library rather than implying it stands alone.
* **Refuse to answer a malformed h2 body in Aether's own voice.** `400` would be the HTTP/1 answer and
  RFC 9113 names the reset; the reset already happened on that stream, so an answer here could only
  either mask it or race it. `an_h2_request_needs_no_declared_length_to_be_served` keeps the opposite
  case honest: absence of a promise is not a defect.
* **Name the division of responsibility in the code, not only in the tests.** The `declared_length`
  doc comment (`framework/aether/src/http2.rs:579-593`) now states which half is the dependency's and
  which is this frontend's, and cites the eight tests by name — so the next reader of this function
  does not run this whole audit again. Refused: a `// h2 validates this` one-liner, which is the
  sentence that reads as an assumption rather than a finding.
* **Pin the ceiling answer against the *repeated* field too.** `h2` hands a request with an agreeing
  pair to the application, so the pre-dispatch `413` reads through `declared_length`, which is exactly
  the function whose first-vs-last choice the null arm above says nothing can distinguish. The test
  (`a_repeated_declared_length_over_the_streaming_ceiling_is_413_before_dispatch`) therefore pins the
  ceiling, not the choice, and the choice stays named as untestable.

**Sensitivity: ten mutants and two null arms, every file restored byte for byte.**

* `I1a` the incremental site stops at the promise → `bytes_past_the_declared_length_that_are_not_a_request_are_400_and_close`
  and `a_body_longer_than_its_declared_length_leaves_the_surplus_for_the_next_request` FAILED (3 passed,
  73 filtered); the `aether-http` parser run stayed green, which is the proof that the wire and the
  unit tests cover different sites.
* `I1b` the buffered site stops at the promise → the 5 wire tests stayed green;
  `test_consumed_offset_is_exact_for_pipelining` and the new
  `test_content_length_surplus_is_left_for_the_next_request` FAILED.
* `I2` the duplicate-`Content-Length` refusal disabled →
  `a_repeated_content_length_is_refused_on_http_1_even_when_it_agrees` and
  `test_smuggling_dual_content_length_rejected` FAILED.
* `I3` **null arm** — `ows_trim` deleted → exit 0 on both runs (1 passed, 1 passed).
* `I3b` `ows_trim` swapped for Unicode `str::trim()` → `a_non_ascii_padded_content_length_is_refused_on_http_1`
  FAILED while `an_ows_padded_content_length_is_served_on_http_1` stayed ok, and
  `test_framing_headers_accept_only_ascii_ows` FAILED: the refusal and the acceptance are separately
  pinned.
* `I4` the streamed lane stops at the promise → both `a_streamed_lane_*` FAILED (0 passed, 8 filtered).
* `I5` the h2 pre-dispatch ceiling defeated → `a_repeated_declared_length_over_the_streaming_ceiling_is_413_before_dispatch`
  and `a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch_over_h2` FAILED while
  `an_h2_body_past_the_ceiling_mid_stream_fails_the_read` stayed ok — the before-dispatch and
  mid-stream answers are two different protections and each has its own test.
* `I6` the reset assertion read error code 0 instead of 1 → all eight hand-framed tests FAILED.
* `I6b` the "nothing was answered" leg flipped to expect one answer → the same eight FAILED. These two
  are the shape of the harness's own zero: a helper that only saw "a stream died" would have passed on
  a `CANCEL`, on a GOAWAY, or on a served response followed by a reset.
* `I7` **null arm** — `declared_length` reads the last value instead of the first → 12 passed, exit 0,
  and the reason is stated above.

Each case asserted a unique anchor before writing, re-read the file after, and reported
`sha_match=True`, so every mutation was restored byte-identically inside its own case. The
hashes are the pre-inject state of that sweep, not a claim about the committed tree: re-running
`sha256sum -c` over all four touched files (parser.rs `95bcb0c8…`, body_pump.rs `dc094b18…`,
http2.rs `cf53a94d…`, http2_e2e.rs `407bf00c…`) on the final tree reads 2 OK and 2 FAILED,
exit 1 - `body_pump.rs` and `http2.rs` match, while `parser.rs` and `http2_e2e.rs` do not
because `cargo fmt --all` re-flowed them after the sweep, which is the same formatting pass the
Gates paragraph records as the reason the docs/04 citation moved a second time.

**The recount this forced.** Re-derived from the tree, not carried: `cargo test --workspace` = **814
passed / 0 failed / 1 ignored** over 66 `test result:` lines — 431 in-crate (21 runs) + 367 integration
(29 targets) + 16 doc (16 runs) — classified by the nearest preceding `Running` header, because a
grep-filtered log hides the `Doc-tests` lines and the first pass misread all 16 as integration.
`http_e2e` 73 → 78, `http2_e2e` 56 → 67, `streaming_request` 8 → 10, `aether-http` lib 105 → 106 (+1
ignored). Six live documents carry the new reading: `FEATURE_COMPLETENESS_MATRIX.md` (its live total,
the per-target chain, the four framing rows, the Unit/Integration rows), `BASELINE_MEASUREMENTS.md`,
`PRODUCTION_READINESS_REPORT.md` (its twentieth reading, §9 and §10's evidence bullets), both READMEs,
and `AETHER_CORE_CAPABILITIES.md`. The recount then broke the citation gate on itself, seven findings,
all of them this track's own doing: three `http2.rs` coordinates shifted +10 when the doc comment grew
(`829→839`, `929→939`, `1047→1057`), the parser insertion plus `cargo fmt`'s own re-flow of the new
  test shifted `971→994`, a citation to
`framework/fuzz/src/lib.rs:87` that no pin covered, and the `worker.rs:32` pin left orphaned by the
sentence that had corrected it. Fixed by moving the coordinates in both the documents and the `PINS`
table, re-pinning the docs/04 span on the test's own `fn` line rather than on a `let limits` body line
that was never unique — the gate says so explicitly now — and adding the fuzz pin while putting the
`worker.rs:32` citation back into the sentence that denies it. Final reading: **107 citations in 16
live documents, 72 distinct spans, 0 findings**; names pass 364 qualified and 1303 bare names over 27
live documents, 34 sheltered by 28 exceptions, 9 attributed, 1 exempted proposal.

**Gates.** `bash gates.sh` on this tree: fmt, clippy, tests, govtests, cite, evidence and validate
all as expected - `cargo test --workspace` **814 passed / 0 failed / 1 ignored** over 66
`test result:` lines (431 unit over 21 runs, 367 integration over 29 targets, 16 doc over 16 runs,
re-derived from the run's own `target/gates/tests.log` by the same classifier that produced the
per-target table above), govtests **112 OK**, cite **107 citations in 16 live documents, 72 distinct
spans, 0 findings** with the names census recorded above, evidence **35 documents scanned,
0 findings**, validate exit 1 carrying only the four pinned historical findings. The first sweep of
the ladder did not pass, and that is recorded as failed rather than smoothed: `fmt` FAILED and
`govtests` FAILED - fmt on four of this track's own new `assert!` and `format!` wrappings, and
govtests on the repository index test, which had already read the new `TRACK_INDEX.md` row and
correctly refused a row that pointed at `## Track 110` before this record existed. `cargo fmt --all`
then re-flowed the new code, and its re-flow of the parser test moved the docs/04 coordinate a second
time (`971` to `991` for the insertion, `991` to `994` for rustfmt), which is the shift the recount
paragraph above carries. The ladder was re-run on the tree that holds this record; no gate reads the
register, so writing these numbers into it changes no gate input.

**Commit split.** Six commits, split by the real diff, each verified on its own tree, no amend, nothing
pushed: `36ec26d` the eleven h2 wire tests (1 file, 275 insertions; `http2_e2e` read 67 passed / 0
failed on that tree); `8c3984f` the five HTTP/1 wire tests, the parser unit test, and the docs/04
coordinate plus the pin its insertion moved (4 files, 196 / 3; `http_e2e` 78 and `aether-http` lib
106 + 1 ignored); `ca411d5` the streamed-lane pair (1 file, 93 / 2; 10 passed); `e964c09` the doc
comment naming the division of responsibility, with the three `http2.rs` citations and their pins it
shifted (5 files, 20 / 10; the twelve length tests green, `cargo clippy -p aether --all-targets -- -D
warnings` clean); `49cbe98` the recount of the six live documents plus the fuzz pin and the restored
worker.rs:32 citation (7 files, 107 / 54; cite 107 / 72 / 0, evidence 35 documents / 0, govtests 112
OK); and this record as the sixth - the register section, roadmap §19 row 110 and the
`TRACK_INDEX.md` row that points at both. Each earlier stage was built by reverting the later
stages' files to HEAD and reading the gate on that tree, so the per-commit numbers above are that
tree's readings, not the final tree's; the ladder the Gates paragraph reports ran on the final one.

**What this track does not claim.** It claims a *contract*, verified from bytes on two frontends, and
the contract is enforced by `h2` 0.4.20 on the h2 side — so the eleven h2 tests are as much a pin on
that dependency's behaviour as on this repository's, and a `h2` upgrade can turn them red without
anything in `framework/` having changed. That is the intended reading: the reset is the promise, and if
the library stops making it, this frontend must learn about it from a failing test rather than from a
CVE. It does not claim the dependency cannot be tricked — only that these eleven shapes were tried on
the wire and each produced the answer the RFC names. It adds no HTTP/2 request-line semantics beyond
the ceiling it already owned, and takes nothing away: the code change this track shipped is a doc
comment, because no defect was found in code. The `first`-vs-`last` choice inside `declared_length`
stays untested and is named as untestable. Response-side `content-length` (RFC 9113 §8.1's other half)
is a different promise and is not this track's: the tracks 39/52/67 rows hold it. Nothing here touches
HTTP/3, and nothing here is a performance claim. No matrix row moved to `VERIFIED_IMPLEMENTED` because
none of these rows was `NOT_IMPLEMENTED`: the four framing rows were rewritten with the measured
behaviour and the tests that hold it, which is what a negative audit is allowed to do.

## Track 111 — the artifacts an app-generator left inside the repository (2026-11-02)

**What was asked.** «أعمل Vibe Coding Cleanup». The owner named a kind of debt, not a list, so the
work began by finding what in this tree is the residue of a generated starter rather than a decision
someone made and defended. Three things were, and none was deleted until its own evidence said the
deletion was free: a presentation site at the repository root (111a, shipped as `34bc2a8`), two
workspace crates that nothing depended on (111b), and twelve dependency declarations no source line
named (111c). This track adds no feature, fixes no defect, and changes no behaviour: it removes.

**111a — the site at the root.** Ten files and 1820 lines (`index.html`, `metadata.json`,
`package.json`, `tsconfig.json`, `vite.config.ts`, `src/main.tsx`, `src/App.tsx`, `src/index.css` and
two `src/data/*.ts` files), added by `1116e1a first commit` and touched by nothing since: a Vite/React
page whose manifest is named `react-example`, whose `metadata.json` declares a server-side Gemini
capability no file imports, and whose benchmark tab prints 860 000 req/s, "47x Pydantic" and 42 MB for
100 000 connections — figures nothing in this repository produced, and which sit next to the only
throughput number this tree does carry, the 17 247 rps
`framework/docs/BASELINE_MEASUREMENTS.md` measured at 8 connections. The two are not the same
experiment, and that is the point: the page had no experiment. The owner approved the
deletion when asked, because a document inside the product tree that publishes unmeasured numbers is a
documentation defect with a build system attached. One consequence was not cosmetic: that
root `package.json` was the reason `framework/tests/examples/node_h2_interop.cjs` had to carry `.cjs`
at all (a nearest-package `type` field decides how Node reads an extensionless-ambiguous file). With
the manifest gone the tree reads a `.js` as CommonJS — re-measured with this tree's Node v24.18.0 —
so the three documents that explained the suffix were restated in the past tense and `node --check`
was run on the `.cjs` (exit 0). That commit also regenerated `PROJECT_MAP.md`, whose rows are derived
from the tree, so the site's own rows left with it.

**111b — the two crates that built alone.** `framework/observability` (`Cargo.toml` 8 lines,
`src/lib.rs` 21) and `framework/serialization` (`Cargo.toml` 11, `src/lib.rs` 5, `src/json.rs` 21):
five files, 66 lines, 47 of them Rust, declaring `aether-observability` and `aether-serialization`.
Evidence they were alone: `git grep` over the nineteen manifests of the tree before this commit (the
workspace root plus 18 members) returns four hits only — the two `members` entries in
`framework/Cargo.toml` and each crate's own `name = ` line. In a Cargo workspace a dependent must
declare the dependency, so an empty declarer set *is* the empty reverse-dependency set
`cargo tree -i` would have printed; the capabilities note had already recorded "zero dependents" for
these two. Evidence the deletion cost nothing measurable: the three source files hold no fenced code block, no
`#[test]`, and neither crate has a `tests/` directory, so the only test surface they had was the
doc-test lane, which counted their empty targets as runs. After the deletion `cargo test --workspace`
reads **814 passed / 0 failed / 1 ignored** over 62 `test result:` lines — 431 in-crate over 19 runs,
367 integration over 29 targets, 16 doc tests over 14 runs. Track 110 had measured the same 814 over
66 lines: 431 unit over 21 runs, the same 367 integration over 29 targets, and 16 doc over 16 runs. The
totals are identical and only the lanes that count targets moved, which is exactly what deleting two
untested members predicts. `members` went 18 → 16, and `sonic-rs` left `[workspace.dependencies]`
because the serialization wrapper was its only declarer — the claim that the workspace declared a
JSON engine nothing used was already in the Arabic document's own wiring note, and it is now closed
by deletion instead of by a caveat.

**111c — the manifests declare only what the sources name.** Two rules, both mechanical, then a
hand-check. Rule 1: every key in `[workspace.dependencies]` must be declared by at least one member
manifest (comment lines stripped first, and a dotted key reduced to its base name, because
`bytes.workspace = true` *is* a declaration of `bytes`). Rule 2: every dependency any member declares
must appear as a whole-word identifier in at least one `.rs` under `framework/`. The audit script
broke twice before it produced a true reading: its first regex could not match dotted keys and
reported all ten workspace keys orphaned, and its second used a back-joined glob (`R +
r"\*\*Cargo.toml"`) whose wildcards the backslashes escaped, so it read no member manifest and
agreed with itself that everything was declared. Fixed, it found exactly two orphaned keys —
`bitflags` and `tracing`, which no member declared and no source named — and ten member declarations
whose name appears in none of that member's sources: `aether-runtime` and `aether-dependency` in
`aether` (the facade imports `aether_runtime::` nowhere), `aether-core` in `benchmarks`, `middleware`
and `router`, `bitflags` in `core` and `http`, `smallvec` in `core` and `middleware`, `serde` in
`validation`. Each was read before it was cut: a name can be used through a re-export or a feature,
so the compiler, not the grep, is the acceptance test — `cargo check --workspace --all-targets` exit
0 and `cargo clippy --workspace --all-targets -- -D warnings` clean on the resulting tree.
`framework/core/src/bitset.rs` also stopped describing itself as a "`bitflags`" implementation, because
that word is a crate name this tree no longer declares and the comment read like a dependency claim.

**The two graph facts this track got wrong at first sight.** `Cargo.lock` still lists `bitflags`
after its workspace key is gone, because the lock is platform-agnostic and three packages hang it on
themselves — `proptest` (lock:1019), `redox_syscall` (lock:1117) and `rustix` (lock:1178), none of
which resolves to that edge on this Windows host. That is why `cargo tree -i bitflags` prints "nothing
to print" here while the entry survives: the reverse tree answers for the host, the lock answers for
all of them. And `tracing` stays in the graph legitimately — `cargo tree -i tracing` resolves
`tracing ← h2 0.4.20 ← aether ← aether-examples`, re-run on this tree — so removing the workspace key
removed a declaration, not a dependency. The lock moved twice and the two moves mean different things:
29 lines left with stage A — the two member entries, the five edges only they had, and
`tracing-attributes`. That last exit is the sharpest thing the lock said: observability declared
`tracing.workspace = true` against a key written plain (`tracing = "0.1"`), so it took tracing's
`default` features, which are `["std", "attributes"]`, and `attributes` means `tracing-attributes`.
`h2` also depends on `tracing` but asks for `features = ["std"], default-features = false`, so once the
only default-features user left the workspace the macro went and `tracing` stayed. Then 10 lines left
with stage B, every one an edge this audit cut and not a single package entry.
The package count reads 202 before the track and **199** on this tree: the two crates plus
`tracing-attributes`. And `sonic-rs` was never in the lock at all — an unused
`[workspace.dependencies]` key resolves no package — so its deletion changed no graph, and the lock's
silence is a second, independent proof that the key declared nothing.

**What the audit could not see.** Name-presence is not reachability: rule 2 would delete a dependency
used only through a macro path or a feature name, and rule 1 cannot tell a stale key from one kept for
an example a member adds next week. Both were answered by hand-checking all twelve removals and by
the compiler, not by the script. This track deliberately ships no new gate for it — an unused-dependency
check would be the eighth rung of `gates.sh`, and building a gate to protect a tree that has just been
cleaned by hand, without a defect that shows the cleaning rotting, is the kind of scaffolding this
same track is deleting. The reading is recorded here instead, and the next drift needs the same pass.

**The documents this moved.** Five live documents and one tool docstring: `README.md` (18 → 16 crates,
and the 814 claim re-measured); `framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md` (the two crate
table rows deleted, the wiring note rewritten for the eight packages `aether` really declares, §10
retitled «التحقق السريع بـ SIMD» with its serialization subsection deleted, §14 retitled «محرك
التشخيص الذكي» with its `RequestTimer` subsection deleted, and both §10 and §14 TOC anchors moved
with them); `framework/docs/AETHER_CORE_CAPABILITIES.md` (the wiring note that said the two crates
"still build and test inside the workspace with zero dependents" now says what happened to them);
`docs/01_architecture_and_lifecycle.md` (§2 listed five crates under four names that the tree does
not use — `http`, `router`, `middleware`, `observability` — and is now the eight that exist, with the
`aether-http` row stating that it holds no HTTP/2 code because `h2` does the framing and the bridge is
`aether/src/http2.rs`); `framework/docs/FEATURE_COMPLETENESS_MATRIX.md` (doc-test lane 16 runs → 14,
the gate census, and an explicit supersession sentence under the dated per-target breakdown so a
reader cannot take a 2026-10-23 table for the current one). Deleted crate names are written as prose,
never inside backticks: the names pass would otherwise turn a sentence about a removal into a phantom.

**Two stale numbers that were not this track's doing.** The matrix's *README / docs accuracy* row and
`cite_check.py`'s own docstring both still printed **106 citations over 71 spans**, which is track
104's census, while track 110's record and the gate on HEAD's tree both read **107 over 72**. The
re-count this track ran caught it; both now carry the measured reading, and the tool's docstring says
in words that track 110 moved the census and left the sentence behind. That is one more sighting of
the documents-count-drift family this project's notes already name, and it is worth saying why it keeps
happening: a citation census is a property of the *gate*, so the track that changes it has no reason
to open the sentence that quotes it.

**Gates.** Four trees ran the ladder, because four commits carried the crate and manifest work. Stage A's tree (the two crates
gone, every manifest still at HEAD): `bash gates.sh` green through all seven — tests **814 / 0 / 1**
over 62 `test result:` lines (431 unit over 19 runs, 367 integration over 29 targets, 16 doc over 14
runs), govtests **112 OK**, cite **107 citations over 72 distinct spans in 16 live documents, 0
findings** with a names census of 364 qualified and **1301** bare, evidence 35 documents / 0 findings,
validate exit 1 on the four pinned historical findings. Stage B's tree (all twelve declarations gone):
the same seven green, the identical test census, and names at 364 qualified and **1302** bare — the
one-name difference is the prose stage B adds, which is what a names census is: a property of the
documents, not of the code. Stage C could only move the document gates, and they read cite 0 findings,
evidence 35 / 0, govtests 112 OK. This record's tree ran all seven again after the governance files
were written — tests **814 / 0 / 1** over the same 62 `test result:` lines, govtests **112 OK**, cite
107 citations over 72 distinct spans in 16 live documents with 0 findings and a names census of 364
qualified and 1302 bare, evidence 35 documents / 0 findings, validate exit 1 on the four pinned
findings — because the index row and the register heading have to land together for
`test_track_index.py` to pass. Nothing failed and nothing was smoothed: validate's exit 1 is the
standing state the ladder already carried before this track — the four historical findings its own
allow-list pins — not a finding of this track.

**Commit split.** Five commits, split by the real diff, each verified on the tree it produces, no
amend, nothing pushed: `34bc2a8` the root site, its documents' claims about it and the regenerated map
(16 files, 56 / 1851); `e326844` the two crate deletions with their `members` entries, the `sonic-rs`
key, the lock and the documents that describe the removal (11 files, 28 / 149); `a835738` the twelve
dependency declarations no source names, the two orphaned workspace keys, and the `bitset.rs` wording
(13 files, 18 / 35); `6555c25` the recount cascade — README, the matrix's doc lane and gate census, and
the gate's own docstring (3 files, 13 / 8); and this record's commit, carrying the register section,
roadmap §19's row 111, the `TRACK_INDEX.md` pointer and the repository map regenerated over the smaller
workspace. The crate work is two commits and not one because A's tree still lets `aether` declare
`aether-runtime`: every sentence about the facade waits for B, and B's tree is the first in which the
manifests and the prose agree.
Each earlier stage was built by reverting the later stages' files to HEAD, gating that tree, and
restoring — so the numbers above are each stage's own reading, not the final tree's.

**What this track does not claim.** It claims nothing about behaviour: no test was added and none was
removed, and the identical 814 is the proof. It does not delete `aether-runtime`, `aether-validation`
or `aether-dependency` — they remain workspace members and their tests still run (`connection` 3,
`aether-dependency` 4, `aether-validation` 0 as before); removing a declaration from the facade
changes what a user installs, not what this workspace checks. It publishes no performance number, and
retiring the root site is not a verdict on dashboards — only on a page that carried figures nobody
measured. It touches no Cargo declaration that a member still needs — after the twelve removals every
key in `[workspace.dependencies]` has a declarer and every declared name has a source hit, which is
the reading the audit prints on the final tree, not the one it printed before the cuts. Finally, this is
a cleanup of *artifacts*, not of every document written by a model: the eight dated snapshots keep
their wrong numbers behind their markers on purpose, because rewriting a historical report turns it
into a different report — and this track's own register section will be read the same way by whoever
comes after it.

## Track 112 — the measurement path the owner was supposed to walk by hand (2026-11-02)

**What blocked the one open engineering line.** `PRODUCTION_READINESS_REPORT.md` §9 leaves item 9 —
"performance work driven by isolated before/after measurements" — and it is the only §9 line that is
neither struck, delivered, nor owner-deferred. It has stayed open for a reason that is not a missing
hypothesis: the measurements it asks for are ones only the owner may record, and the path to them was
walked by hand. The worker-count question is the clearest case. `AETHER_WORKERS` defaults to
`num_cpus::get().max(1)`, track 65 refused to change that default on unpinned evidence, and the one
harness that could pin it (`framework/tools/pgo_ab.ps1`) compares exactly two arms, of which one is
usually a build. So this track delivers no throughput number — it delivers the tool that makes the
owner's number cheap to take, and it fixes the two documented commands that would have failed on him
before he started.

**112a — the arm rules moved out of the copy, not into a second copy.** The four rules that make an
arm's row worth reading were lifted from `framework/tools/pgo_ab.ps1` into
`framework/tools/load_arm.ps1`: `Test-PortFree` (refuse to start on a port that already answers),
`Wait-Port` (a banner is not proof of readiness), `Stop-Server` (a survivor is a refusal, not a leak),
and `Invoke-LoadArm` — the whole start / prove-identity / offer-load / read-that-process'-CPU
sequence, now with every value it used to borrow from its parent script's scope passed as a
parameter. `Test-LoadShape` carries the generator's 500/s-per-connection floor and `Get-ArmMedian`
the exit-0-only median rule, so the two harnesses cannot drift apart in how they reject a shape they
cannot offer or how they summarise a rep that lied. `pgo_ab.ps1` dot-sources the file and its
`Invoke-Arm` is gone; the medians block calls `Get-ArmMedian`. The behaviour-preserving evidence is
the re-run, not the reading: `pgo_ab.ps1 -Self -Workers 1` at a 2 s / 500/s / 8-connection smoke shape
ran both arms, exited 0, printed the noise-floor line and wrote `ab.csv`. One column is added to that
CSV — `BannerWorkers`, the worker count the arm's own banner reported — because the sweep needs the
number the A/B tool had already read and thrown away.

**112b — `framework/tools/workers_sweep.ps1`.** One binary, N `AETHER_WORKERS` shapes, one session.
`-Shapes 1,2,4,8,0` by default, where `0` means *inherit*: the runtime's own default becomes an arm
that is measured instead of a number a reader guesses from a core count, and `banner_workers` in every
row is the proof of what it resolved to on that box. The rules it inherits are 112a's. The rules it
adds: the rep order rotates by a cyclic offset so an ordering effect cannot land on the same shape
every rep; each shape prints its own per-rep spread next to its median, because a median is one rep;
deltas are taken against the first shape with the CPU ratio beside the percentage (the sign convention
that made a regression read as a win is the one nobody memorised); and `-NullArm` adds one arm
identical to the first shape, whose printed delta is what this harness calls zero. Rows go to
`framework/target/workers-sweep/sweep.csv` and both tools close with the sentence that says whose
numbers these are.

**Two defects the smoke test caught, and neither was visible in the code.**

- `-Shapes` was declared `[int[]]`. Under `powershell -File`, the argv token `1,0` arrives as one
  string and the binder reads it through the locale's *group separator*: a probe printed
  `count=1 element '10'` where the same command under `-Command` printed two elements. The sweep ran
  one shape with ten workers, the server's banner agreed it had ten, and the table would have been
  labelled as a two-arm ladder. The parameter is now a `[string]` that this script splits on commas
  and spaces *before* any numeric conversion, refuses a token that is not a non-negative integer, and
  refuses a repeated shape. Both the rule and the measurement are in the parameter help, because the
  obvious next version of this file will be tempted back to a typed array.
- `$nullArm = @($usable | Where-Object { $_.Null })` — PowerShell variable names are case-insensitive,
  so that line assigned to the `[switch] $NullArm` **parameter** variable, and an array into a typed
  parameter throws `Cannot convert value "System.Object[]" to type "SwitchParameter"`. The throw
  fired in the last block of the script: the whole table printed, the disclaimer printed, and the exit
  code was 1. Renamed to `$nullArms`. A harness that prints a complete, plausible report and then
  fails is exactly the shape of defect the arm-identity guards exist for, and it survived a
  read-through.

**The `.ps1` files could not be run as documented, measured before fixing.** Every execution-policy
scope on this host is `Undefined`, which leaves the effective policy for a script file `Restricted`.
`powershell -NoProfile -File framework\tools\pgo_ab.ps1 -Self ...` exits 1 with "cannot be loaded
because running scripts is disabled on this system" — before any argument is parsed, so every
documented `framework\tools\pgo.ps1 -Action …` line in §5 was equally unrestappable as written. The
docs now carry the `-ExecutionPolicy Bypass -File` form. Re-measured on the final tree: the bare
`-File` form exits 1 with that sentence and the same line with `-ExecutionPolicy Bypass` exits 0.
The comment help inside the three scripts showed only the bare invocation, so their `.EXAMPLE` lines
carry the form that runs too — a reader who opens `pgo.ps1` instead of `BENCHMARKS.md` gets a
command that works. No machine or user policy was changed here; that is the owner's call and this
record does not recommend it.

**Two documented commands that were false.** `BENCHMARKS.md` §4 told the reader to run
`openloop_loadgen --url …` after `cargo build --example`, but `--example` writes an artifact and
installs nothing: the binary is `framework/target/release/examples/openloop_loadgen.exe`, under
`examples\` and not on `PATH`. §4 now gives the path form *and* the
`cargo run --release -p aether-tests --example openloop_loadgen -- …` form, verified by cargo's own
`Running target\release\examples\openloop_loadgen.exe` line followed by the generator's usage text.
The other path lives in the dated track-62 entry of `WORKING_CONTEXT.md`, which recipes
`framework\target\release\openloop_loadgen.exe` — no `examples\` segment. It was left as written: a
dated record corrected in place stops being evidence of what was believed then. The true path is in
the live document, which is where a reader goes to run something.

**What was measured in this track, and what is not claimed.** Mechanics, at shapes too small to mean
anything: the two harnesses exit 0 end to end, and five refusal paths were run rather than read — a
non-integer shape, a repeated shape, a rate above the per-connection floor, a missing binary, and a
port already held by a listener the script did not start (the guard fired from the shared file, at
`load_arm.ps1:103`, before any arm was measured). The sweep adds a sixth identity rule of its own — a
row whose banner reported no worker count is a refusal, not a blank cell, because `banner_workers` is
the only evidence that the inherit arm ran the shape its row claims — and its matcher was exercised on
both banner shapes: `Workers     : 8 (tokio work-stealing)` yields 8, a banner without the line yields
empty and throws. The throw line itself was read, not fired: no binary in this tree prints a banner
missing that line, so nothing here proves the `throw` executes, only that its condition becomes true.
A rep that exits 0 whose generator output does not parse is now excluded by a `Usable` flag the row
carries, because its rps would be the number 0 and a mean of zero turns every delta printed from it
into `Infinity`.

Then the reason the tool exists at all: across four smoke runs at one 2-second duration and one
shape, the null arm — identical to its baseline by construction — printed **+0.0%, +20.0%,
+44.4% and +20.0% µs CPU/request**. Those figures describe this harness at an unusable duration, not Aether. They are
recorded because they are the evidence that the tool's own warning ("a real effect has to be larger
than this") is not decoration, and it is why the sweep prints per-rep spread next to each median and
why the owner is handed a duration he chooses, not one this track picked. No worker-count ladder was
run. No throughput, latency or CPU figure from this track is a project result, and the default
`num_cpus::get().max(1)` is unchanged — track 65's refusal stands until the owner takes the numbers
himself.

**The documented line, re-run after the fixes.** `-Shapes 1,0 -NullArm -Affinity 1` was launched
through `-File`, which is the form that used to break, and printed `arms=w1, default, null_w1` with
the `default` row's `banner_workers` reading 8 — the inherit arm measured, not guessed. Both
fixed defects are therefore re-tested on the line the docs publish, not only on the code that
implements them.

**Gates.** `bash gates.sh` on the tree that carries the three commits above plus the governance
rows: fmt and clippy clean, tests **814 / 0 / 1** over 62 `test result:` lines, govtests **112 OK**,
cite **107 citations over 72 distinct spans in 16 live documents, 0 findings** with the names pass
at 364 qualified and 1302 bare over 27 live documents (34 sheltered, 9 attributed to another crate,
1 exempted proposal), evidence **35 documents / 0 findings**, validate exit 1 on the four pinned
historical findings, ladder exit 0. No `.rs` file changed in this track, so the 814 is unchanged by
construction and the ladder ran to prove that rather than to discover it; the two rungs this track
can move are cite and evidence, and both read 0 findings over the document it rewrote. Nothing was
smoothed: validate's exit 1 is the standing state the ladder already carried, not a finding of this
track. This paragraph was written after that run, and the four rungs that could read it (govtests,
cite, evidence, validate) were re-run over the tree carrying it and read the same — which is
also what the globs say: `.project-governance/notes/**` is not in either document gate's checked
set, so a register section cannot move a rung and this claim is a check, not a favour.

**Commit split.** Four commits, split by the real diff, no amend, nothing pushed: `919ad07` moved
the arm rules into `load_arm.ps1` and rewired `pgo_ab.ps1` onto them (2 files, 219 / 135);
`6cabcf7` added `framework/tools/workers_sweep.ps1` (1 file, 265 lines); `b0c9cdd` fixed every
published benchmark command (4 files, 107 / 18) — `BENCHMARKS.md` plus the `.EXAMPLE` lines
in the three scripts; and this record's commit carries the register section, roadmap §19's row
112, the `TRACK_INDEX.md` pointer and the map regenerated over the tree. The rules and the sweep are
two commits because the sweep is only readable once the rules are shared: one commit would show a
new harness without showing that the old one now answers to the same guards. The command fixes are a
third commit because their evidence is different in kind — it was produced by running lines
out of a document, not by running a script. Two files stayed out of every commit and were not
discarded: `.project-governance/tools/evidence_check.py` and its test show only line-ending
normalisation in `git status` and no content diff, which is the state this repository keeps them in.
## Track 113 — live documents that named a mechanism the tree does not have (2026-11-02)

**What opened it, and what did not.** The owner asked for the `my_cool_site` result and offered to
raise Track 113 from it if the consumer failed. It did not fail: `cargo check` in
`C:\Users\hamza\Desktop\my_cool_site` finished with exit 0, zero warnings, in 57.90 s (`threekway
v0.1.0` checked, `sqlx` with it). So no consumer fix and no re-export shim is owed, and the track that
opened instead is the one the previous track's own evidence exposed: while re-reading the delivered
lists to answer him, `framework/docs/PRODUCTION_READINESS_REPORT.md` was found naming a syscall the
repository does not contain.

**The defect, and why a correct matrix did not excuse it.** Two sentences of the readiness report
said that response streaming "plus static-file `sendfile`" had left the remaining-work list in track
99. Nothing in this tree calls `sendfile`: `grep -rn -iE 'sendfile|transmitfile' --include=*.rs
framework/` is zero hits, and the file answer is a userspace loop — `const STATIC_CHUNK: usize =
64 * 1024` and `FileBody` over a `tokio::fs::File` in `framework/aether/src/static_dir.rs`, pinned by
`a_file_body_reads_in_bounded_chunks_and_starts_at_its_offset`, which asserts the chunk sizes. The
completeness matrix and `framework/README.md` were already correct: the row's evidence cell states
"Literal `sendfile(2)` is not used: the reads are buffered 64 KiB chunks", and the README lists the
syscall under "Not implemented (selected)". What happened is narrower and worse than a typo — a live
document inherited the *row title* ("Streaming / `sendfile`") as a *feature name*, in the one document
a reader treats as the delivered list. The fix keeps the row title recognizable and states the
mechanism at both sites: `framework/docs/PRODUCTION_READINESS_REPORT.md:148` now says the streamed
static-file answer left the list, names the matrix row it came from, and says the literal syscall
stays on the not-implemented list; `:910` says the same in one clause.

**The same class, swept rather than stumbled on.** One finding in a document set this size is a
sampling result, so every mechanism *name* in the live documents was re-derived against the tree:
`io_uring|epoll|kqueue|SO_REUSEPORT|mmap|zero-copy|SIMD|splice` over `docs/**/*.md`,
`framework/docs/**/*.md` and both READMEs (54 lines), each verdict taken from the code, not from the
prose. Outcomes:

- **SIMD — three sites, one real defect.** `core::arch`, `std::simd` and `#[target_feature]` have zero
  hits in any `.rs` file. `validate_ascii_fast` (`framework/validation/src/simd.rs:8`) does
  `bytes.as_chunks::<8>()` and tests each word with one `0x8080808080808080` mask — word-at-a-time,
  not vector instructions. The module titled itself "SIMD-Optimized Validation Primitives", the Arabic
  reference said "بسرعة تعليمات SIMD" in the crate table and "عبر تعليمات المعالج" in its §10
  subheading, `docs/01_architecture_and_lifecycle.md:57` said the crate "تحقق SIMD", and
  `framework/docs/adr/ADR-001-algorithm-strategy.md:24` listed "SIMD where it pays" as an *adopted*
  strategy. All four now name the mask. The §10 heading itself keeps the word SIMD: it is the target of
  a table-of-contents anchor, so the correction is the note above the sample (`:509`), not the anchor.
- **The ADR row failed twice, and the second half is the more common defect here.** Its evidence cell
  read "validation tests". The validation crate has no `#[test]` at all (`grep -rn '#[test]'
  framework/validation/src/` is empty); the only test in the tree that touches it builds an `Email`
  inside `framework/tests/src/lib.rs`. The row now cites that, and says the crate has none.
- **`SO_REUSEPORT` / `wrk` — a banner that covered less than it seemed to.**
  `framework/docs/BENCHMARKS.md` §1 still asserts a loopback run "with `SO_REUSEPORT` and multi-queue
  RSS enabled", generated by `wrk`, on a 64-core EPYC node. Neither option name nor `wrk` appears in
  any `.rs`, `.ps1` or `.toml` under `framework/`. The document's own notice disclaims the *table*
  ("the throughput table below ... was not produced in this environment") and §3's observations were
  never named, so a reader could take §1 as the environment for §4-§5 — which are the real
  instructions. The notice now says §1 and §3 belong to that same unverified run and names the three
  absences.
- **io_uring / epoll — the diagram, caught by the banner.** `framework/docs/ARCHITECTURE.md:49`
  (47 before this track's own banner edit) draws its bottom box as "Thread-per-Core Async Core
  (io_uring / epoll / tokio)". The banner above it
  already refutes thread-per-core and the arena, so the claim was disclosed — but it did not name the
  two interfaces, and a reader looking for the reactor has only the diagram to go on. The banner now
  names io_uring and epoll as absent from the tree. The diagram was not redrawn: the ASCII boxes are
  width-aligned and a banner sentence is not a worse guard than a mended picture. That insertion added
  two lines to the top of the file, so every later coordinate in it moved; the citation pass re-read all
  16 live documents afterwards and nothing was left pointing at a shifted ARCHITECTURE line.
- **Left alone, with the reason.** The matrix's *Zero-copy head parsing* row is `PARTIALLY_IMPLEMENTED`
  and says so; *SIMD-style validation* is `PARTIALLY_IMPLEMENTED` and already reads "64-bit word ASCII
  scan"; `AETHER_CORE_CAPABILITIES.md:4` scopes the word "zero-copy" before using it;
  `BASELINE_MEASUREMENTS.md` and `FINAL_REPORT.md` are dated snapshots with the SUPERSEDED banner and
  their own line ("No SIMD for header parsing") is a gap list, not a claim; `ERROR_HANDLING_GUIDE_AR
  .md:36`'s "(SIMD Validator)" names the component the file is still called, after the module.

**Two refusals by the gates, both correct.** The name pass refused `io_uring` in backticks — the
checker treats a backticked token as a name the tree must contain, which is exactly the distinction
this track is about. The tempting fix was the `NAME_EXCEPTIONS` table; refused, because that table
shelters names a document *legitimately* proposes, and using it to shelter an absent syscall would turn
the gate's one honest signal into an exemption list. The backticks came off instead. Separately, the
citation pass refused both new `:line` spans as unpinned (`static_dir.rs` and `tests/src/lib.rs` have
no pins), so they became symbol-name citations in the form the matrix row already uses. `cite_check`
reads 107 citations in 16 live documents, 72 distinct spans, 0 findings — the same shape as the tree
this track started from.

**What is not claimed.** No behaviour changed: the only `.rs` edit is a doc comment, so the suite total
is unchanged by construction, and no throughput, latency or CPU number appears in this track. In
particular nothing here says the 64 KiB loop is *slower* than a `sendfile` would be — that A/B has
never been run and stays owner-measured perf work (register row E8). The remaining-work side of the
ledger is untouched: `PROB-0004` is paused by the owner, `PROB-0005`/`PROB-0011` are public-API
decisions, `PROB-0012` needs a Linux host, and `#69/F-19/F-20` wait on his numbers.

**Gates.** `bash gates.sh` exits 0 over the tree carrying these corrections, and every reading below is
transcribed from that run: fmt PASS; clippy `--workspace --all-targets -- -D warnings` PASS; tests **814
passed / 0 failed / 1 ignored over 62 `test result:` lines**; govtests **Ran 112 tests, OK**; cite **107
citations in 16 live documents, 72 distinct spans, 0 findings**, with the names pass over **27 live
documents resolving 364 qualified and 1304 bare names** (34 sheltered by `NAME_EXCEPTIONS`, 9 attributed
to another crate, 1 exempted as a proposal); evidence **35 documents scanned, 0 findings**; validate exit
1 on the four pinned historical findings and nothing else. The 814 is unchanged by construction, and the
reason is checkable: the only `.rs` file this track touched is `framework/validation/src/simd.rs` and the
change is its module doc comment. The bare-name count moved from the 1302 track 112 recorded to 1304
because the corrected sentences name symbols the pass can resolve (`STATIC_CHUNK`, `FileBody`,
`validate_ascii_fast`, `Email`). One earlier run of this track's ladder failed, and it is recorded rather
than smoothed: it ran while the register section had landed and the `TRACK_INDEX.md` row had not, and
`test_track_index.py` failed the govtests rung for exactly the coupling it exists to enforce.

**Commit split.** Four commits, split by the real diff, no amend, nothing pushed: `7512a01` moved the two
readiness-report sentences off the syscall name (1 file, 8 / 4); `ddb3bad` moved four documents and one
module title off SIMD (4 files, 14 / 5); `378f9cb` widened the two banners whose scope was narrower than
the claims sitting under them (2 files, 9 / 1); and this record's commit carries the register section,
roadmap §19's row 113, the `TRACK_INDEX.md` pointer and the map regenerated over the tree. The split is
by kind of evidence, not by file count: the first commit's proof is a grep of every `.rs` file for a
syscall name, the second's is a grep for instruction-set attributes plus a census of where the crate's
tests actually are, and the third's is a document's own notice failing to cover the sections beneath it.
No file appears in two of them. Two files stayed out of every commit and were not discarded:
`.project-governance/tools/evidence_check.py` and its test, which show only line-ending normalisation and
no content diff.

## Track 114 - a cookie the client wrote is not a cookie this server issued (2026-11-02)

**What opened it, and how it was chosen.** The owner's standing instruction is to take the next
unblocked work and decide the engineering details without asking («كمل», and «اتخذ قرار» to open this
track). Unblocked means: no owner measurement in front of it, no Linux host, no scope decision. That
leaves the parity rows in roadmap §19.4 that still say "not done", and the one this track took is the
shape a reader of the live documents cannot avoid: `RequestContext::cookie` returns the client's text
verbatim, while every session-flavoured example in the Arabic and English reference reads a cookie as
if the server had chosen its value. `aether-security` already signs and verifies twice over (`jwt`,
`csrf`) and had no way to sign a cookie. The gap was not on any list, so it is recorded as a new row,
**F-21**, in roadmap §19.4 and in §19.7's order.

**The design decisions, each with the alternative it refuses.**

- **Integrity, not confidentiality.** The value is `base64url(payload).base64url(HMAC-SHA256)` over
  `ring::hmac`, verified with `hmac::verify`, so the constant-time comparison is the verifier's own
  and there is no `constant_time_eq` call site a later edit can forget. The alternative - an AEAD -
  needs a rotation and algorithm story this crate has not designed, and it would let a document claim
  the client cannot read the value. That claim would be false here, so the matrix row, the PRR entry
  and the Arabic reference each say the payload is base64url and keep confidentiality on the
  remaining-work list instead.
- **The cookie name sits inside the MAC.** `mac_input` is name, one NUL, then the encoded payload
  (`framework/security/src/cookie.rs:183-189`). The separator is not decoration: base64url's alphabet
  lives inside the token alphabet a cookie name is written in, so a bare concatenation splits more
  than one way and one cookie's valid value becomes another's. Without the name, a value issued for
  `theme` presents itself as `role=admin`; mutant M1 below is that weaker design, and the suite
  rejects it.
- **Four outcomes, not two.** `Present` / `Absent` / `Invalid` / `Ambiguous`. Nothing sent is a
  login redirect, something-unverified is a forged-or-stale decision that should clear the cookie,
  and something-sent-twice is an attack that must not be resolved by taking the first or the last -
  the same rule `CsrfTokens::cookie_value` already applies for the same sibling-domain tossing
  reason, and the reason the facade accessor walks every repeated `Cookie` line instead of the first.
  Every verification failure answers the same way on purpose: a truncated value, a padded tag, a
  standard-alphabet tag, a wrong-length tag, a non-UTF-8 payload and a valid signature over another
  name all produce `Invalid`, because telling them apart tells the sender which half he got right.
- **No `Debug` on the value types.** Track 103's rule for key-holding types, extended: `CookieSigner`
  implements none (a `compile_fail,E0277` doc test at `framework/security/src/cookie.rs:27` refuses a
  future derive), and `SignedCookie`'s own `Debug` prints `Present(<redacted>)`, because a signed
  payload commonly holds the session or user id that logs collect.
- **No default secret.** The secret is a constructor argument and an empty one panics
  (`an_empty_secret_panics`), for the same reason `CsrfTokens` takes its key as an argument: a signer
  built from `""`, or from an unset environment variable, would verify its own output and look exactly
  like a working session layer.
- **`CookieNameError` refuses a name outside the token grammar.** `csrf.rs` already implemented
  `is_token` (RFC 9110 §5.6.2, the grammar RFC 6265 §4.1.1 wants for a cookie name); it became
  `pub(crate)` rather than being copied. The reason is a wire mismatch, not tidiness:
  `Response::set_cookie` percent-encodes the name it writes while a request lookup compares names
  literally, so a name outside the grammar sets a cookie this server can never read back. Refusing it
  moves that failure to the call site.
- **The facade accessor lives in a new file.** `framework/aether/src/cookie.rs` is an inherent
  `impl RequestContext` in its own module (`:31-91`), not lines added to `app.rs`, because inserting
  into `app.rs` shifts every live `app.rs:NN-MM` citation the gate pins - the coordinate-stability rule
  track 42 learned by re-deriving that whole set.

**What was added, and what the suite now measures.** `framework/security/src/cookie.rs` (401 lines),
`framework/aether/src/cookie.rs` (91), the new integration target
`framework/tests/tests/signed_cookies.rs` (322), the two `lib.rs` re-exports plus the prelude, and
`is_token`'s visibility change. Twenty-nine tests: **15 unit + 11 wire + 2 doctests + 1
`compile_fail`**, and the workspace total moves **814 → 843** - 446 unit, 378 integration across 30
targets, 19 doc over 14 doc runs, 0 failed, 1 ignored, 63 `test result:` lines. All eleven wire tests
run through `TestClient`, which the module header itself describes as "HTTP/1.1 over TCP", so they
exercise the real parser, the real header storage and the real response writer rather than a mocked
request.

**Sensitivity by injection: six killed, two survived, and the survivors are the informative half.**
Eight mutants over the two new source files, each restored by sha256 check afterwards
(`C:/tmp/t114/mutate2.py`, re-run on this tree after `cargo fmt`; the harness prints at most five of an
injection's failing names, so the record names which tests answer rather than how many): the name
dropped from the MAC input (the round-trip cases and every cross-name case fail,
`a_value_issued_for_one_name_is_refused_under_another` among them), the duplicate-name guard deleted
(`one_name_carried_twice_is_ambiguous` fails), `hmac::verify` computed but
its result discarded (five fail, every forged-tag case among them), a case-insensitive name match and
a prefix name match (both fail `a_name_that_is_a_prefix_or_a_case_variant_does_not_match`), and the NUL
separator removed (fails `the_mac_input_separates_name_from_payload`). **Two survived**: deleting
`mac.contains('=')` and deleting the post-decode `mac.len() != MAC_BYTES` check both leave the suite
green. Neither is a missing test - 43 base64url characters always decode to exactly 32 bytes, and a
padded tag is never 43 characters - so each guard is implied by the length rule that *is* tested. They
stay, because they document the wire shape a reader would otherwise re-derive, and the code now says
that at `:163-169`. The claim in this record is 6 of 8; there is no "all mutants killed" claim here.

**The documents, because a feature the docs do not carry is a feature nobody uses.** The corrected
claim propagates by grep, not by memory: every live sentence that pointed a reader at `cookie()` as a
session source was found by searching for the claim. `docs/09_api_reference.md` now says the raw value
is "as the client wrote it … so it is not evidence that anything is valid", gains a
`ctx.signed_cookie` section, gains §7 item 6 on `CookieSigner`, and its `.set_cookie` bullet stops
implying the attributes are a security property (they belong to the browser; the header signs
nothing). Both specification tables (`docs/00_framework_specification_ar.md`, `..._en.md`) gained a
`signed_cookie` row with the exact signature, and the Arabic teaching document's extractor sample
gained two comment lines pointing at it. The readiness report's remaining-work item 7 is now "the
signed half closed", with the encryption half left open in its own words.

**Two refusals by the gates, one self-inflicted number, and a stale claim found on the way.** The
citation pass rejected the four new `docs/09` spans as unpinned, so `cite_check.py` carries five new
`PINS` entries - each added only after reading the boundary line's exact text, since a pin is the
mechanised form of "re-derive both ends". The pin-injection harness then reported "2/5 SURVIVED" on a
gate that had in fact rejected all five: the strings I expected were `first line no longer holds` and
`last line no longer holds`, while the checker prints `the span's first line moved` and
`the span's last line moved`. With the expectations fixed the same run reports 5/5 rejected, baseline
exit 0, every restore hash verified. That is the harness's own zero being measured, the second time
this track family has caught it. Third, the recount patch wrote the matrix's bare-name figure as 1324
in the same run that added three more resolvable names to the corpus; the final gate reading is **376
qualified / 1327 bare**, corrected before this commit. Fourth: sweeping the claim rather than the file
list surfaced two live statements in `framework/docs/BASELINE_MEASUREMENTS.md` that predate this track
and were simply wrong - a "now reports 295 in-crate unit tests" reading from 2026-10-18 and a table
still listing the two crates track 111 deleted as zero-test targets. Both now carry the 2026-11-02
re-measurement. The dated readings elsewhere stay frozen: `PRODUCTION_READINESS_REPORT.md` §5 gains a
twenty-first reading and demotes the twentieth to "described the track-110 tree".

**What is not claimed.** No HTTP/2 wire test covers the accessor: it reads decoded headers, so h2
reaches the same code, but that is an argument from reading the shared serving core, not a test - and
`signed_cookies.rs` is HTTP/1 only by the sentence above. *(Closed by track 115 on 2026-11-03: five wire tests appended at `framework/tests/tests/http2_e2e.rs:3716-4163` read the accessor through that frontend's own `header_maps`, on h2c and on TLS with ALPN `h2`; its record is the one below. The sentence stands as written, because it is what this record knew on 2026-11-02.)* No replay defence, no expiry and no rotation
live inside the value: there is no timestamp and no key id, so an old-but-valid cookie stays valid
until the handler's own `Max-Age` (a browser-side instruction) or a secret change ends it. Nothing
about this track's cost is measured - no throughput, latency or CPU-per-request number appears in it,
and no benchmark was run. Confidentiality is explicitly not delivered. The remaining-work side is
otherwise untouched: `PROB-0004` stays paused by the owner, `PROB-0005`/`PROB-0011` are public-API
decisions, `PROB-0012` needs a Linux host, `F-19`/`F-20` wait on his numbers.

**Gates.** `bash gates.sh` exits 0 over the tree carrying all of the above, and every number below is
transcribed from that run: fmt PASS; clippy `--workspace --all-targets -- -D warnings` PASS; tests
**843 passed / 0 failed / 1 ignored over 63 `test result:` lines**; govtests **Ran 112 tests, OK**;
cite **111 citations in 16 live documents, 76 distinct spans, 0 findings**, names pass over 27 live
documents resolving **376 qualified and 1327 bare names** (34 sheltered by `NAME_EXCEPTIONS`, 28 in
the table, 9 attributed to another crate, 1 exempted as a proposal), 49 citations in 8 dated snapshots
marked and not validated; evidence **35 documents scanned, 0 findings**; validate exit 1 on the four
pinned historical findings (PROB-0007 x2, PROB-0017 x2, REQ-0001 x3, REQ-0001 open-under-reused-ID) and
nothing else. The ladder was then run a second time, because two Arabic sentences in `docs/09` were
re-worded after the first green run (a bullet that said the signer's absent `Debug` covered every
value type, when it covers the key holder and `SignedCookie` redacts instead, and a phrase that named
"the matrix" where it means the *Cookies* row). No count moved between the two runs: 111 citations in
16 live documents over 76 spans, and 376 qualified / 1327 bare names both times. `cargo fmt --all -- --check` was clean only after the formatter reflowed two spots in the
new wire-test file (import order, one collapsed builder chain); both are whitespace and both lanes were
re-run afterwards: 15 unit and 11 wire pass again.

**Commit split.** Four commits by kind of evidence, no amend, nothing pushed: `6dd7877` carries the security crate and
its tests (proof: the 15 unit tests plus the 6-of-8 mutation result), `3162900` the facade accessor and
the wire target (proof: 11 loopback exchanges and 2 doctests), `d5c278d` the documents and the citation
pins (proof: the re-measured census and each pin refusing its own injection), and this record's commit
carries roadmap §19.4's F-21, §19.7's row 114, the `TRACK_INDEX.md` pointer and the regenerated map.

The first commit's own tree was measured, not assumed: the two commit-2 files were moved aside and
`framework/aether/src/lib.rs` rewritten from its HEAD blob, which is the tree with the security crate
alone - fmt PASS, clippy `--workspace --all-targets` PASS, **830 passed / 0 failed / 1 ignored over 62
`test result:` lines** (446 unit, 367 integration, 17 doc). So the +15 unit and the +1 `compile_fail`
belong to commit 1 and the 11 wire tests plus 2 doctests to commit 2, and 814 + 16 + 13 = 843 closes
the arithmetic without a re-count later. The three files were then restored from the copies and each
sha256 re-checked before staging. Two files stayed out of every commit and were not discarded: the
evidence gate's own script and its test, which show only line-ending normalisation and no content diff.

## Track 115 - the signed cookie read over the second frontend (2026-11-03)

**What opened it.** Track 114 closed the integrity half of the *Cookies* row and then wrote, under its
own "What is not claimed": *No HTTP/2 wire test covers the accessor: it reads decoded headers, so h2
reaches the same code, but that is an argument from reading the shared serving core, not a test*. This
track turns that sentence into a test, on the owner's standing instruction to take the next unblocked
work and decide the details alone («كمل», «اتخذ قرار»). The argument was not weak - `header_maps`
(`framework/aether/src/http2.rs:883-922`) is the only h2-specific piece between a HEADERS block and a
handler - but it was an argument, and here a claim about a frontend is evidence only once something on
that frontend has been made to answer for it. No production code changed. The whole track is five wire
tests, appended at `framework/tests/tests/http2_e2e.rs:3716-4163`.

**Seven decisions, each with the alternative it refused.**

1. *Placement: the new tests went into `framework/tests/tests/http2_e2e.rs`, not a new target.* The
   obvious shape was `signed_cookies_h2.rs`, but `H2Client` is file-local to the h2 suite: a second
   target would have re-typed ~100 lines of TLS-with-ALPN, prior-knowledge-h2c, driver-task and
   bounded-close code, and would then have had to re-earn the assertion that the connection really
   negotiated `h2` - an assertion whose absence lets such a file silently measure the HTTP/1 frontend.
   Cost: the h2 target's own count and every coordinate below the insertion point. That cost is why
   decision 2 exists.
2. *Everything appended below the last existing line, and `begin_with_headers` (`:3822`) is a free
   function rather than a fifth `H2Client` method.* Twenty-one coordinates in the governance records
   point into this file - the register's eight (highest at line 3466) and `WORKING_CONTEXT.md`'s
   thirteen plus three spans - and none of them is protected by a gate: `cite_check.py` reads the live
   documents, and no live document cites this file at all (a grep for `http2_e2e.rs:` over `docs/`,
   `framework/docs/` and the two READMEs returns nothing). Inserting a method at 283, or an `use` line at
   the top, would have shifted all twenty-one and nothing would have reported it: a shifted coordinate
   turns a historical record into a wrong one silently. A free function in the appended block reaches
   `client.send` and `client.uri()` because the whole file is one module. The final diff is one hunk,
   `@@ -3714,0 +3715,449 @@`, purely additive, and `cite_check` still reports 0 findings on the tree.
3. *The two repeated-field shapes were measured before anything was asserted about them.* The
   accessor's `Ambiguous` answer depends on what the frontend hands it, and RFC 9113 lets a receiver
   coalesce repeated fields. A throwaway probe ran against a live h2c server: `cookie: a=1; b=2` arrived
   as one line with its value verbatim, `cookie: a=1` plus `cookie: b=2` as two lines in wire order, and
   two identical fields as two lines. The probe was then deleted and the file truncated back to its
   exact prior bytes (sha256 verified) before the real tests were written.
4. *The four statuses are the HTTP/1 suite's four statuses.* 200/401/403/409, byte for byte, so a
   difference between `tests/signed_cookies.rs` and this block can only come from the transport, never
   from a handler written differently for the h2 case.
5. *The concurrency premise is asserted per stream, not as one elapsed window.* Four handlers holding
   100 ms each answer in ~100 ms together and in >= 400 ms serially, so the wall clock alone would have
   accepted a chain that ran once per *connection*: one delayed stream, three instant answers, all four
   statuses still right. `begin_with_headers` therefore returns each stream's own latency measured from
   handing its request to draining its body, the loop at `:4091-4102` requires every one of the four to
   carry the 100 ms, and the ceiling at `:4109` (`< 350 ms`) is what refutes serialising them. `worker_threads
   = 4` does no work in that argument - `tokio::time::sleep` is a timer, so even one worker overlaps the
   four - the thing that does is the per-stream task `http2.rs` spawns for each accepted stream.
6. *The TLS/ALPN frontend gets the round trip plus one refusal, not the whole matrix.* The header store
   is the same `header_maps` call whether the stream arrived over cleartext or after ALPN, so repeating
   eight planted values there buys runtime, not a new code path. What the encrypted frontend has and h2c
   does not is a real rustls handshake, an `h2` ALPN assertion and a warm HPACK context before the
   negative arrives, which is what `:4120` is for. The docs say which half is h2c-only; see "not claimed".
7. *No new citation pins.* `framework/tests/tests/http2_e2e.rs:3715-4163` would be a ~450-line span
   frozen into `PINS`, and the file grows at the bottom, so the span would rot on the next append. The
   documents name the file and the count instead, which the names pass resolves and no coordinate shift
   can break.

**A green suite that was not exercising anything, and then a review that found the second half of the
same defect.** The overlap test wraps the app in a 100 ms middleware, because four handlers have to be
in the chain at the same time. The first version built the app and *then* called `use_middleware`, and
every assertion passed - because `use_middleware` wraps only the routes registered after it
(`framework/aether/src/app.rs:962-990`), so the hold never ran, the four streams never overlapped, and
the test was proving that a connection behaves when nothing is concurrent. What gave it away was the
runtime: five tests finishing in 0.03 s when one of them sleeps for 100 ms. The fix is structural
(`add_signed_cookie_routes` (`:3748`) is called after the middleware is installed, deliberately, and
`:3767-3773` says so) and the check became a floor on elapsed time. An independent sensitivity review
of this block then found that a floor plus a ceiling on one window is still not the premise: it is
satisfied by a chain that ran once per connection, which is decision 5 above. Both shapes were injected
after the fix and both now fail (M11 and M12 below), with the measured text naming the stream that
answered too fast. The rule this track takes away is the one track 102 wrote for assertions: a premise
has to be asserted in the shape that would fail if the premise were false, and "the whole thing took
about 100 ms" is not that shape when one held stream is enough to produce it.

**The same review's three other findings, each accepted and then measured.** (a) The accessor documents
that it does not percent-decode (`framework/aether/src/cookie.rs:44-46`) and no request in the block
contained a `%`: test 1's check was on the *outbound* value, which proves the setter did not encode and
says nothing about the reader. A store that decoded on the way in would turn `session=<payload>%2E<mac>`
into a well-formed signed value and answer 200 to a cookie nobody issued. That case is now in the refusal
loop at `:3946`, and mutant M9 is exactly the decoding store. (b) The block's own header comment claimed
the tests hold `header_maps` to "repeated fields stay separate lines". They do not: the accessor splits
each line on `;` (`framework/aether/src/cookie.rs:69`), so a store that joined two `cookie` fields with
`"; "` hands over the same pair multiset and the same four answers. The comment now says what is pinned -
the value arrives untouched, and a name carried twice stays refused whichever field shape it arrived in -
and the claim was tested rather than reworded: mutant M10 is that `"; "` join, and it survived. A
surviving mutant is only worth recording once it has been run. (c) One assertion in the overlap test
compared the body but not the status, so the single positive answer on a concurrent connection was half
pinned; it now asserts the pair, as the two round-trip tests already did. Two smaller gaps from the same
reading: `issued_pair` takes the first `set-cookie` field and nothing pinned it to one field - the HTTP/1
suite does (`tests/signed_cookies.rs`, "two Set-Cookie lines on one response") - so `set_cookie_fields`
(`:3810`) is asserted to be 1 in both round-trip tests.

**Sensitivity by injection: 12 mutants, 11 killed, and the survivor is the informative one.** Each
mutant is a plausible wrong implementation, not an absence. Anchor uniqueness was asserted before
writing and every file's sha256 verified against a backup after each restore (`aether/src/cookie.rs`
back to `c09dfba1f380`, the digest track 114 recorded for it; `aether/src/http2.rs` `cf53a94d2936`;
`security/src/cookie.rs` `89914f45`). The eight in the first round were re-run against the final file
after the review's fixes, so every number here describes the tree this record commits.

| # | wrong implementation | what failed, with the measured assertion |
|---|---|---|
| M1 | duplicated name resolved by taking the first value | the duplicated-name test - "one cookie field carrying the name twice"; the overlap test - "the stream that carried the name twice while its neighbours carried it once" |
| M2 | cookie-pair name matched case-insensitively | "Session matched `session` over h2c" |
| M3 | accessor reads `header()` instead of `header_values()` | "two cookie fields with the same name: the store joined them instead of keeping them apart" + the overlap test |
| M4 | `mac_input` drops the name binding | "a value this server signed, offered under another name was accepted over h2c: session=dXNlci00Mg.hV0Agl3Qk…" |
| M5 | `header_maps` keeps only the first line of a repeated field | as M3, plus the overlap test |
| M6 | `header_maps` coalesces repeated fields with ", " | 403 where 409 is required - the wrong decision about who the caller is |
| M7 | `FORBIDDEN_FIELDS` gains "set-cookie" | both round-trip tests panic in `issued_pair`: "a Set-Cookie field on the response" |
| M8 | `header_maps` never delivers `cookie` (the vacuity check) | all five tests |
| M9 | `header_maps` percent-decodes field values | "a percent-decoding store turned this into a well-formed signed value was accepted over h2c: session=dXNlci00Mg%2EZiPLvgiNm0VylGqAcUL3c5STyvYU1CaPvFEvrvV_854" |
| M10 | `header_maps` joins repeated `cookie` fields with "; " | **survived - 5 passed.** Not a missing test: through this accessor the two shapes are the same request. It is what decision (b) narrowed the comment to, and the reason the comment no longer claims separate lines are pinned. |
| M11 | middleware registered after the routes, so it wraps nothing | the overlap test at `:4097`: "the valid stream answered in 2.6445ms, faster than the 100 ms hold every request is supposed to wait through" (whole target: 0.02 s) |
| M12 | chain runs once per connection - one stream held, three instant | the same floor at `:4097`: "the valid stream answered in 3.0425ms, …" |

M7, M8 and M10 are the three that say what the suite is and is not: the first shows the positive tests
really read the field the server issued, the second shows all five watch the same header store rather
than passing by accident, and the third bounds the claim - the h2-specific property these tests pin is
the separator and the untouched value, not the number of lines the store happens to keep.

**Counts.** Five tests, all inside the h2 target: `http2_e2e` 67 -> 72, integration 378 -> 383, unit 446
and doc 19 unmoved. No production code changed, no target was added, so the census stayed at 63
`test result:` lines and the workspace total moves **843 -> 848 passed / 0 failed / 1 ignored**
(transcribed from the gate run below, and re-derived with two independent parsers of the `test result:`
lines rather than read off a summary: 446 + 383 + 19 = 848, and the lane sum over 30 integration targets
holds).

**Docs.** Six live documents carry the new evidence, patched as bytes for CRLF and re-censused after the
prose: root `README.md` (843 -> 848, 67 -> 72, "twenty-six tests for a signed cookie" -> "thirty-one …
sixteen on the wire, eleven over HTTP/1.1 and five over HTTP/2"), `framework/README.md` (the same counts
plus what the h2 half adds), `framework/docs/BASELINE_MEASUREMENTS.md` (the per-target table's h2 row and
the integration lane), `framework/docs/FEATURE_COMPLETENESS_MATRIX.md` (the *Cookies* row now says the
accessor is proven on both frontends and names which shapes are h2c-only; its §"gate" row re-measures the
census date on the track-115 tree), `framework/docs/PRODUCTION_READINESS_REPORT.md` (§5 gains its
twenty-second reading and the twenty-first's "This describes the tree in the repository now." is demoted
to what it now is; §8 item 7's evidence sentence gains the five tests and their h2c/TLS split), and
`docs/09_api_reference.md`'s Arabic الأدلة bullet. The citation gate reports the same corpus it did
before the prose: **111 citations in 16 live documents over 76 distinct spans, 0 findings**, names pass
**376 qualified / 1327 bare** (34 sheltered by 28 per-document reasons, 9 attributed to another crate, 1
exempted proposal), 49 citations in 8 dated snapshots, evidence **35 documents scanned, 0 findings**. No
number moved, because no citation was added and no file the gate reads moved above its own coordinates.

**What is not claimed.** The duplicate-name shapes and the four-stream overlap are h2c results: the one
TLS test round-trips the issued cookie and refuses a foreign MAC, so nothing here says those two behave
after ALPN - they reach the same `header_maps`, and that is the same kind of argument track 114 refused
to publish as a test. No test distinguishes the store keeping two `cookie` fields as two lines from
joining them with `"; "`, because the accessor splits each line itself and mutant M10 proves the two are
one behaviour through this API; what is claimed is the answers, not the internal shape. Nothing measured
here is a claim about any other HTTP/2 implementation - the shapes were observed against this server and
an `h2`-crate client, and the coalescing that RFC 9113 permits a receiver to do is left to each. The
timing half of the overlap test is the flaky half: the 100 ms floors cannot go wrong from load, while the
`< 350 ms` ceiling can, on a machine that serialises four streams because nothing else is available;
measured here at 0.12 s for all five tests. The *Cookies* row's confidentiality half is still withheld -
the payload is base64url, readable by whoever holds the cookie, and an AEAD needs a rotation and
algorithm story this crate has not designed, which is an owner scope decision and stays on
`PRODUCTION_READINESS_REPORT.md` §8 item 7's remaining-work wording. Nothing about cost is measured: no
throughput, latency or CPU-per-request number appears in this record and no benchmark was run. The HTTP/1
suite was not touched (`tests/signed_cookies.rs` stays at eleven tests) and no Linux behaviour is claimed,
which stays blocked on `PROB-0012`; `PROB-0004` remains paused by the owner and `F-19`/`F-20` wait on his
numbers.

**One refusal by a gate, recorded as refused.** The first full ladder on this tree reported clippy FAIL:
`manual implementation of split_once` at `tests/tests/http2_e2e.rs:3868-3872`, in the attribute assertion
of the first new test (`splitn(2, ';').nth(1)`). It is a lint on the new code, not a pre-existing lane, so
the run was stopped, the spot rewritten as `split_once(';')`, the five tests re-run green, and the ladder
started again on the fixed tree. The Gates paragraph below is that second run.

**Gates.** `bash gates.sh` exits 0 over the tree carrying all of the above and every number below is
transcribed from that run: fmt PASS; clippy `--workspace --all-targets -- -D warnings` PASS; tests **848
passed / 0 failed / 1 ignored over 63 `test result:` lines** - 446 unit + 383 integration across 30
targets + 19 doc over 14 doc runs, with `tests/http2_e2e.rs` itself at **72 passed** - parsed by two
independent readers of `target/gates/tests.log` that agree on all three lanes and on the 446 + 383 + 19 =
848 sum; govtests **Ran 112 tests, OK**; cite **111 citations in 16 live documents, 76 distinct spans, 0
findings**, names pass over 27 live documents resolving **376 qualified and 1327 bare names** (34
sheltered by `NAME_EXCEPTIONS`, 28 in the table, 9 attributed to another crate, 1 exempted as a
proposal), 49 citations in 8 dated snapshots marked and not validated; evidence **35 documents scanned, 0
findings**; validate exit 1 on the four pinned historical findings (PROB-0007 x2, PROB-0017 x2, REQ-0001
x3, REQ-0001 open-under-reused-ID) and nothing else. The refusal above means the ladder ran twice on this
tree and only the second run reached the tests lane, so the 848 is one measurement, not two agreeing; the
citation census was re-read after that run and reports the same figures as the standalone pass.

**Commit split.** Three commits by kind of evidence, no amend, nothing pushed. `3468f85` carries the five
wire tests and nothing else: the twelve-mutant table above was measured against exactly that file, and the
new names re-run on that tree answer 5 passed in 0.11 s - the target's 72 and the workspace's 848 come
from the gate run on the same bytes, and the only thing commit 1's tree lacks is six markdown files, which
no crate embeds (`include_str!` over the workspace sources names none). `e9aee44` carries the six
documents: its proof is the census re-read on the tree with that prose and no other change, 111 citations
in 16 live documents over 76 spans with 0 findings, and evidence 35 documents with 0 findings. This
record's commit carries roadmap 19.7 row 115, the `TRACK_INDEX.md` pointer, the dated pointer inside track
114's own "What is not claimed", and the regenerated map.

Two files stayed out of all three commits and were not discarded: the evidence gate's own script and its
test, which show line-ending normalisation and no content diff.

## Track 116 - `Expect` honoured on both frontends (2026-11-04)

**What this track changed, and what it did not.** Measured against `HEAD` before the first edit:
HTTP/1 already answered a supported `Expect: 100-continue` with an interim on both dispatch paths
(`framework/aether/src/app.rs` had two `100 Continue` write sites) and already latched the buffered
path's answer per request through `awaits_100_continue`. **Nothing** in HTTP/2 looked at `Expect` at
all, and no code anywhere produced `417`: the `AetherError::UnsupportedExpectation` variant, its
`E3003UnsupportedExpectation` code and its diagnostic are new here. So this track's delta is:
(1) `417 (Expectation Failed)` for an expectation-extension the server cannot meet, refused from the
head on both frontends (RFC 9110 §10.1.1); (2) an HTTP/2 interim — a real informational `100` carried
on the stream (RFC 9113 §8.2.3), the first time this frontend called `send_informational`; (3) the
streamed HTTP/1 path no longer sends an interim for a request whose body already arrived in full;
(4) the HTTP/2 field-line ceiling now decides `431` before the expectation decides anything; (5) the
`max_headers_count == 0` guard became observable and got pinned.

**One classifier, both frontends.** `classify_expectation` / `expectation_of`
(`framework/http/src/parser.rs`) read the `Expect` value as a comma-separated token list (RFC 9110
§10.1.1, §5.5), so a list that contains one unsupported element refuses the whole request instead of
answering `100` for its supported half — and because both frontends call it, that reading cannot
differ by protocol.

**New shared helper.** `aether_http::parser::body_is_pending` is the streamed path's equivalent of the
buffered path's `Ok(None)`: `BodyFraming::None` is never pending, a declared length is pending until
`header_len + len` bytes have arrived, and `Chunked` is deliberately `true` — `ChunkedBodyDecoder` is
the only place chunk framing rules live and duplicating that state machine in a helper would create a
second, weaker owner.

**Independent review (separate reviewer agent, limited-review protocol).** Verdict: the buffered-latch
test OK, the h2 interim test OK, the count-vs-expectation ordering test SHOULD-FIX. It found defects
the author had missed; each shipped as a fix with its own failing-first wire test.

1. **h2 answered `417` before the field-line ceiling decided `431`.** Measured before the fix:
   `PROBE_H2_VERDICT=417 lines=9` — a 9-line head carrying `Expect: foo=bar` over h2 got the
   expectation refusal while the same head over HTTP/1 got `431`. The `header_maps` ceiling check was
   hoisted above the `Expect` block in `framework/aether/src/http2.rs`, and
   `the_field_line_ceiling_outranks_the_expectation_over_http2`
   (`framework/tests/tests/http2_e2e.rs:4355`) was written red-first at its assert `:4385`
   (`left: 417 / right: 431`), green after the hoist with 79 others passing.
2. **A streamed route sent an interim for a request that had already arrived in whole.** Measured
   before the fix: `PROBE_STREAMED_FIRST_READ="HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK ..."`.
   `a_streaming_route_sends_no_interim_for_a_request_that_arrived_whole`
   (`framework/tests/tests/streaming_request.rs:553`) was written red-first at its assert `:571`
   (`left: (100, [])` / `right: (200, [53])`); the fix is the `body_is_pending` conjunct on the
   streamed interim site in `framework/aether/src/app.rs`.
3. **`max_headers_count == 0` guard was unpinned.** Deleting the in-function count guard in
   `framework/http/src/parser.rs` left 109 lib tests + 89 http_e2e tests green — the SHOULD-FIX. That
   guard is reachable only for a zero budget with a one-field-line head (the ceiling-sized retry in
   `parse_head` answers every taller head first), so
   `test_a_zero_header_budget_refuses_the_one_line_head_that_otherwise_parses`
   (`framework/http/src/parser.rs:2222`) pins exactly that shape and now kills the deletion.
4. The review also flagged the streamed interim site's **missing per-request latch** as latent and
   unreachable (`split_to(header_len)` runs immediately after it). The latch was added — and then the
   **gate disproved it**: `cargo clippy --workspace --all-targets -- -D warnings` refused it as
   `unused_assignments`, because the value written at the streamed site is unconditionally overwritten
   by the completed-request reset before anything reads it. Recorded decision: the latch stays only
   where it is load-bearing, in the buffered head loop, whose head really is re-examined after every
   socket read; the streamed site documents the structural reason instead — it splits the head off
   before the pump, so one head is examined exactly once and the interim decision cannot repeat.
   `body_is_pending` and the write path are unchanged by that correction.

**Sensitivity table (measured; kill sites are the `test result` panic coordinates).**

| ID | Injection | Target run | Result |
|---|---|---|---|
| MUT-A1 | `framework/aether/src/http2.rs`: `&& !core.streams_route(...)` on the 417 condition | `--test http2_e2e` | KILLED by exactly 1 test: `a_streamed_h2_route_answers_its_expectation_before_it_dispatches`; 78 passed |
| MUT-A2 | same conjunct on the interim condition | `--test http2_e2e` | KILLED by exactly 1 test: the same; 78 passed |
| MUT-A3 | h2 ceiling left below the `Expect` block (the shipped defect, reversed) | `--test http2_e2e` | KILLED by `the_field_line_ceiling_outranks_the_expectation_over_http2` at http2_e2e.rs:4385, `left: 417 / right: 431`; 79 passed |
| MUT-B | `framework/aether/src/app.rs` buffered site `if !continue_sent` to `if true` (latch deleted) | `--test http_e2e` | KILLED by exactly 1 test: `a_body_that_arrives_in_pieces_earns_one_interim_not_one_per_piece`; 88 passed |
| MUT-C1 | `framework/http/src/parser.rs` header-slot estimate ceiling `header_budget` to `usize::MAX` | `-p aether-http --lib` | GREEN (109 passed + 1 ignored) — recorded as a surviving probe: uncapping the estimate is behaviourally equivalent under the shipped defaults |
| MUT-C2 | C1 plus moving the expectation check above the header-count check | `-p aether-http --lib` | 107 passed, 2 failed, 1 ignored; the failures are `test_unsupported_expectation_is_refused_from_the_head_ahead_of_the_body_budget` and `test_header_count_refusal_outranks_the_expectation_refusal`, so the ordering is falsifiable only in that configuration |
| MUT-C3 | `framework/http/src/parser.rs` `if headers.len() > limits.max_headers_count` deleted | `-p aether-http --lib`, `--test http_e2e` | BEFORE this fix: 109 lib + 89 http_e2e green. AFTER: KILLED by `test_a_zero_header_budget_refuses_the_one_line_head_that_otherwise_parses` |
| MUT-D1 | `framework/aether/src/app.rs` streamed site: drop the `body_is_pending` conjunct (always interim) | `--test streaming_request` | KILLED by exactly 1 test: `a_streaming_route_sends_no_interim_for_a_request_that_arrived_whole` (streaming_request.rs:571); 11 passed. `-p aether-http --lib` alone stays green, which is why the fix needed a wire test |
| MUT-D2 | streamed site: `body_is_pending(...)` to a never-true condition | `--test streaming_request` | KILLED by the pre-existing `a_streaming_route_answers_100_continue_before_the_body` (streaming_request.rs:523, `no interim response: Elapsed`); 11 passed |
| MUT-D3 | `framework/http/src/parser.rs` `body_is_pending` off-by-one (`<` to `<=`) | `--test streaming_request` | KILLED by `a_streaming_route_sends_no_interim_for_a_request_that_arrived_whole`; lib unaffected (re-measured on the final tree) |

D1 and D2 were re-measured **after** the clippy-forced collapse of the streamed `if`, so both
directions of the shape that ships are pinned on the final tree, not on an intermediate one.

**Reviewer's caveats, kept in the record.**
- `a_body_that_arrives_in_pieces_earns_one_interim_not_one_per_piece`
  (`framework/tests/tests/http_e2e.rs:1787`) pins the once-per-request half only together with
  `expect_100_continue_is_answered_before_the_body_arrives`
  (`framework/tests/tests/http_e2e.rs:1722`); on its own it does not assert the latch across a
  keep-alive pair. No live document may credit it alone — the readiness report's `Pinned by` list now
  says so explicitly.
- An h2 interim relocated inside the client's 0-2 s window is wire-equivalent to a single one, so
  "exactly one informational frame" is not pinable at that resolution.
- `filter` to `find` in the expectation classifier survives this track's tests and is caught only by
  `repeated_expectation_fields_are_one_list` (`framework/tests/tests/http_e2e.rs:2004`), which this
  track added for that reason; the HTTP/2 twin
  `repeated_expectation_fields_are_one_list_over_http2_too`
  (`framework/tests/tests/http2_e2e.rs:4462`) pins the same reading on the other frontend.
- Reviewer **not verified**: the h2 informational frame arriving after the final response; the
  flakiness ceiling of the 240 ms polling window under a loaded CI (6 local runs only); whether
  `max_headers_count == 0` is reachable from a production configuration; `parser_properties` behaviour
  under the mutants; the argument that a substring-style `awaits_100_continue` cannot misfire was
  structural, not measured.
- One earlier review claim — "nothing asserts the `100 Continue` phrase" — was **wrong** and is
  recorded as wrong: the phrase is asserted on the HTTP/1 wire by the tests named above.
- The reviewer's word "pre-existing" for
  `test_unsupported_expectation_is_refused_from_the_head_ahead_of_the_body_budget` meant "already in
  the tree at that moment of this track": it belongs to this track, not to `HEAD`.

**The defect this track found by measuring its own fix, and how far it got recorded.** While probing
the interim, the same harness showed a *different* rule failing: a buffered request whose body stops
arriving earns no `408`. Re-measured on the final tree with `AETHER_MAX_RECEIVE_TIME_SECS` at its
default and at 2 s: EOF after 5.005 s with no body byte sent, 5.003 s with three of five bytes, and
5.002 s even at the default 30 s budget, while a head that trickles inside the budget is answered `408`
and a streamed body that stops is failed by its lane. The cause is structural: the receive deadline is
consulted on the parser's need-more-data branch, and a socket that simply stops is ended by the 5 s
per-read timeout arm that closes the connection without writing an answer. It was **not** fixed inside
track 116 — widening a core connection loop deserves its own measurements and its own mutants — so it
is registered as PROB-0034 with those four timings, and every document that taught the blanket promise
was narrowed to the three shapes that are true: the `ServerLimits::receive_timeout` doc comment in
`framework/http/src/limits.rs`, the matrix's *Slow-client protection* evidence cell, the `408`
paragraph of `docs/07_limits_and_dos_mitigation.md`, §9 item 9 and §4.2's superseded bullet of the
readiness report. The probe file was deleted after the measurement so it cannot enter the test census.

**Gate ladder.** Run 1 refused and is recorded as refused: **fmt FAIL** (the new
`framework/tests/tests/parser_properties.rs` import was hand-formatted) and **clippy FAIL**
(`unused_assignments` on the streamed `continue_sent = true;`, the finding that produced correction 4).
Both closed. Run 2, on the tree this record describes: `bash gates.sh` reports PASS for fmt, clippy,
tests, govtests, cite and evidence, and PASS for validate in its designed state (exit 1 with exactly
the four pinned findings and no fifth). The citation gate reads 112 citations in 16 live documents
over 77 distinct spans with 0 findings, and its name pass resolves 382 qualified and 1366 bare names;
the evidence gate scans 35 documents with 0 findings.

**Measured counts on the final tree.** Suite: **875 passed / 0 failed / 1 ignored** over 63
`test result:` lines — unit 450 over 19 unit runs, integration 406 across 30 targets, doc 19 over 14
doc runs — cross-checked by the second rule (63 `running N tests` lines sum to 876 = 875 plus the one
ignored test). The targets this track moved: `aether-http --lib` 106 + 1 ignored to 110 + 1 ignored,
`--test http_e2e` 78 to 89, `--test http2_e2e` 72 to 80, `--test streaming_request` 10 to 12,
`--test parser_properties` 4. The prior value of each of the three wire targets was re-derived by
counting `#[test]` attributes in that file at `HEAD`, and the same count on this tree reproduces the
log's number for all three, so the split is measured rather than inferred from the total. Feature
matrix: **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136**, unchanged since track 108 — checked by diffing every
(feature, status) pair against that track's commit, where the pair set is identical, and re-derived by
two parsers over the same line set. The *HTTP/2 over TLS* row's own count moved 79 to 80 with this
track's eight h2 wire tests.

**Refused alternatives.** (a) Keep the streamed-path latch and silence clippy with
`#[allow(unused_assignments)]` — refused: the value is provably never read, so the annotation would
preserve dead state and teach the next reader that a latch is load-bearing there. (b) Collapse the
streamed interim into the buffered loop — refused: the streamed path dispatches from the head, so the
interim must be decided before the pump exists. (c) Answer `Expect` in the router or handler layer —
refused: HTTP/1 must refuse before a body byte and only the head-only parser stage has that guarantee.
(d) Read the h2 expectation after the body — refused, that inversion is the defect fixed in point 1.
(e) Make the chunked half of `body_is_pending` exact by pulling `ChunkedBodyDecoder` state into it —
refused: it would put a second copy of chunk framing outside the decoder that owns it. (f) Fix
PROB-0034 inside this track — refused: it changes the connection loop's read arm and needs its own
deadline measurement, its own 408-vs-close mutants and its own matrix evidence cell, which a track
whose subject is `Expect` would have bundled dishonestly. (g) Flip the *Slow-client protection* row to
`PARTIALLY_IMPLEMENTED` — refused: the row's promises (per-read timeout, receive deadline on a
trickling request, both frontends) are all verified; the defect is one shape the row did not claim,
and the honest move was to narrow the claim's wording and open the problem, not to redefine the row.

**What remains open after this track.**
- PROB-0034 (the paused buffered body earns no `408`), with its fix shape recorded: bound the waiting
  read by the shorter of `read_timeout` and the receive budget's remaining time, and answer `408`.
- The HTTP/1 `417` wire test does not yet assert an empty error body; the h2 one does. Recorded, not
  widened here.
- `to_diagnostic` is still called nowhere in the workspace, so the new `E3003` hint/title joins an
  already-registered untested surface.
- `OWNER_VISION.md` gained a dated agent record (2026-11-04) that states, subsystem by subsystem,
  which of the vision's six demands the tree meets; the SIMD half, the request arena on the served
  path and a non-cryptographic hasher are named there as not delivered, and the owner-gated
  measurements (F-19, F-20, the release-shape decision behind PROB-0005/0011, the Linux run of
  PROB-0012) as the reason they are not advancing here.


## Track 117 - the receive budget answers a body that stops arriving (2026-11-04)

**The defect, measured before the first edit.** PROB-0034 was registered out of track 116's own
probe: a buffered HTTP/1 request whose body stopped arriving was closed by the 5 s per-read
`read_timeout` with **no answer at all**, even when `receive_timeout` (`max_receive_time`) was
shorter. Four timings on the track-116 tree: with a 2 s budget and `Content-Length: 5`, a head with
no body byte ended in EOF after 5.005 s with 0 bytes sent; three of five bytes then paused, 5.003 s
with 0 bytes; the default 30 s budget, head only, 5.002 s — so the per-read bound won whatever the
budget said, which made it a paused-body hole rather than a short-budget artifact. A body trickling
inside the budget was already answered `408`, and a streamed body that stops was already failed by
its lane; only the buffered, silent peer fell through. The cause was structural: the receive deadline
was consulted on the parser's need-more-data branch after a read returned (`app.rs:2918`), while the
read itself was bounded by `limits.read_timeout` and its timeout arm `break`ed the connection loop
without writing (`app.rs:3043` before this track).

**The fix: two clocks, one answer.** A pending read is now bounded by the shorter of `read_timeout`
and what is left of the receive budget whenever a request has begun arriving (`app.rs:3005-3026`):
`recv_started` is set on the request's first byte, so a read begun just inside the budget cannot
outlast it by a whole idle timeout. Both expiry sites — the parser's `Ok(None)` branch after a read
returned, and the read's own timeout arm — call one helper, `answer_receive_timeout`
(`framework/aether/src/app.rs:4222-4233`), which writes `408 Request Timeout` with
`Connection: close` (RFC 9110 §15.5.9), bounds the write by `write_timeout`, and closes with a FIN.
The answer carries no body, so the write buffer cannot grow past its initial block and nothing on
this path is charged or released.

**What did not change, deliberately.** An idle keep-alive socket still closes with no answer, and so
does a connection that has not sent a byte: neither has a request to answer. The pre-first-response
socket keeps the plain `read_timeout`. The streamed lane's deadline, the handler deadline that starts
only after a request has arrived, and every HTTP/2 path are untouched.

**Red first.** The reproduction was written before the fix and failed for the right reason: the
budget test timed out as `Elapsed(())` at its 2 s outer wrap, and the default-budget test read
"server closed the connection without answering the paused body" at the pre-fix EOF.

**The four wire tests** (`framework/tests/tests/http_e2e.rs`):
- `a_request_that_stops_arriving_is_answered_408_by_the_receive_budget` (`:1724`): a 600 ms budget;
  four shapes — half a head, 0 of 5 body bytes, 3 of 5, and a chunked body missing its last chunk —
  each answered `408` with `Connection: close` and then closed.
- `a_stopped_body_waits_no_longer_than_the_budget_it_started_with` (`:1788`): a 1500 ms budget, one
  byte at 1100 ms; the answer must arrive in `[1400 ms, 2000 ms)`, which pins the remainder cap
  rather than merely the fact of an answer.
- `a_body_that_stops_arriving_is_answered_408_by_the_idle_timeout` (`:1830`): the default 30 s
  budget; a paused body is answered after at least 4 s — the per-read clock now answers instead of
  closing silently.
- `an_idle_keep_alive_connection_is_closed_without_an_answer` (`:1869`): after a served `/ok`, an
  idle socket under a 300 ms `keep_alive_timeout` delivers no bytes at all; it also asserts the
  served 200 left nothing coalesced in the client buffer, so the silence is not an artifact of an
  unread earlier response.

**Sensitivity by injection (six mutants, each killed by a named test).**

| ID | Injection | Result |
|---|---|---|
| MUT-A | drop the remainder cap (wait the full `read_timeout`) | KILLED by `a_request_that_stops_arriving_is_answered_408_by_the_receive_budget` |
| MUT-F | cap by the whole budget instead of what is left (wait `receive_timeout` from each read) | KILLED by `a_stopped_body_waits_no_longer_than_the_budget_it_started_with`; measured 1.004 s on the first test shape, then 2.605413 s after the late-byte test was given its own 1500 ms budget |
| MUT-B | restore the silent `break` on a half-received read timeout (the pre-fix behaviour) | KILLED by both 408 tests |
| MUT-C | answer `400` instead of `408` | KILLED by both 408 tests |
| MUT-D | omit `Connection: close` from the answer | KILLED by both 408 tests |
| MUT-E | answer `408` on every read timeout, including an idle keep-alive socket | KILLED by `an_idle_keep_alive_connection_is_closed_without_an_answer`; the wire text it read was `HTTP/1.1 408 Request Timeout\r\nConnection: close\r\nDate: ...` |

**Two independent reviews (separate reviewer agent, limited-review protocol).** The first returned
SHOULD-FIX on the tests and the documents: the keep-alive test could pass vacuously on a coalesced
408, its comment said 300 ms while the configured budget was 600 ms, an `elapsed() < 2 s` assertion
was unreachable behind a 2 s outer wrap, and a `< 900 ms` bound was a flake risk; several live
documents still taught the pre-fix promise. Fixes: `client.buffered.is_empty()` after the served 200,
the 408+close assertions moved into the 408 tests, the outer wrap reduced to 1500 ms with the
unreachable assertion dropped, the late-byte shape moved to its own 1500 ms-budget test with 500 ms
margins on both sides, and the documents re-derived. The confirmation review returned **PASS** with
residuals kept in the record: the late-byte test's `>= 1400 ms` has 100 ms of slack in the safe
direction; an "answer immediately on the first timeout" mutant passes the four-shape test but is
caught by the pre-existing `a_slow_upload_is_not_bounded_by_the_handler_timeout` and by the late-byte
test's lower bound; delivery of the `408` over TLS and the RST race below are unverified. A second
tautological `buffered.is_empty()` the confirmation review spotted after the raw read was removed.

**Refused alternatives, and why.**
- Drain the socket after the `408` (`discard_after_close`) — refused: this path is reached because
  the peer is **silent**, so there is by definition nothing queued to discard; the generic parser
  refusal (`400`/`413`/`431`) does not drain either, and the `413` drain exists because that client
  is still sending. The residual RST race in the window between the answer and the close is named
  below, not hidden.
- Expose a separate per-read timeout knob — refused: the budget was already configurable
  (`max_receive_time`), and the defect was that the loop did not honour the shorter clock it already
  had. A knob would have made the two-clock relationship an operator's problem instead of the
  loop's.
- Answer `408` on every read timeout — refused, and killed by the keep-alive test: an idle socket
  between requests has no request to answer, and inventing one would put a failure on the wire for
  a healthy connection.
- Narrow the documents instead of fixing the loop — refused: the streamed and trickling shapes were
  already true, and the remaining shape was a real defect in the connection loop, not a claim to
  reword.

**Gate ladder.** The reproduction run is red by design and recorded above. The ladder on this
track's committed tree reports PASS for fmt, clippy, tests, govtests, cite and evidence, and PASS
for validate in its designed state (exit 1, exactly the four pinned findings, no fifth). The citation
gate reads **112 citations in 16 live documents over 77 distinct spans, 0 findings**, and its name
pass resolves 382 qualified and **1378** bare names; the evidence gate scans 35 documents with 0
findings.

**Measured counts on the final tree.** Suite: **879 passed / 0 failed / 1 ignored** over **63**
`test result:` lines — unit 450 over 19 unit runs, integration 410 across 30 targets, doc 19 over 14
doc runs — cross-checked by the second rule (63 `running N tests` lines sum to **880** = 879 + the
one ignored test). Only one target moved: `--test http_e2e` 89 -> 93. No crate gained a unit test and
no lane entered or left the census. Feature matrix: **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136**, unchanged —
the only matrix cell this track touched is the *Slow-client protection* row's evidence, and no status
cell moved (verified by diffing the matrix against the previous commit). PROB-0034 was closed through
`project_governance_bootstrap.py done`, which logs the removal in `event_log.jsonl`. The open
problem list is now four items — PROB-0004, PROB-0005, PROB-0011 and PROB-0012 — and every one
of them is in the owner-gated list `OWNER_VISION.md` names.

**What remains open after this track.**
- Delivery of the `408` over TLS is unverified: the new tests run on cleartext sockets, and no TLS
  wire test asserts this timeout answer.
- In the narrow window between writing the answer and the FIN, a peer that resumes sending could
  make Windows reset the connection over unread bytes (the same class the `413` drain exists for).
  The peer is silent by definition when this path is entered, so the window is small but not zero;
  it is named here rather than measured.
- The late-byte test's lower bound carries 100 ms of slack in the safe direction; a slower host
  would need the margin re-examined.
- PROB-0012 (a Linux run of the suite) still blocks any cross-platform claim, and the owner-gated
  measurements listed in `OWNER_VISION.md` are unchanged.


## Track 118 - the developer-facing diagnostic stops describing a runtime failure as a bind failure (2026-11-04)

**The defect.** `AetherError::to_diagnostic` was written when `AetherError::Io` meant a startup
failure: its `Io` arm told the reader to check whether the port was taken, change `AETHER_PORT`, and
kill the process listening there. The 2026-10-24 C′ error contract
(`impl IntoResponseError for AetherError`, `framework/aether/src/app.rs:188-222`) moved `Io` to a
request-time failure answered **500 / E5002** (status table `:189-204`), and the startup bind failure
still builds and prints its own `E1001` diagnostic at `framework/aether/src/app.rs:2002` before
returning `AetherError::Io` at `:2010` (`bind_with_config`, `:1992`). So the one report a developer
could read about a failed handler write sent them to restart a healthy server. Nothing in the
workspace called `to_diagnostic`, and no test read it, which is why the stale port advice survived
tracks 89-117 (track 116's own record named the method as still uncalled).

**Three adjacent defects found while pinning it, in the same method.**
- `HeaderTooLarge` is raised for **two** budgets: the field-line count (`framework/http/src/parser.rs:428`,
  `framework/aether/src/http2.rs:324`, knob `AETHER_MAX_HEADERS_COUNT` at
  `framework/aether/src/config.rs:181`) and the head's bytes (`AETHER_MAX_HEADER_BYTES`, `config.rs:131`).
  An arm that names only the byte budget repeats the wrong-knob failure this track fixes for `Io`.
- `BodyLimitExceeded` named only `ServerLimits::max_body_bytes`, a type the facade never lets an app
  set — `AetherApp` maps `AetherConfig` into it at `app.rs:2037-2049` and exposes no setter. The
  actionable knob is `AETHER_MAX_BODY_BYTES` (`config.rs:110`).
- `Timeout`'s `Display` said "Connection timeout". A 504 is the **handler** deadline; the receive
  deadline answers `408` itself and never becomes an `AetherError` (track 117). The diagnostic now
  names `max_request_time` / `AETHER_MAX_REQUEST_TIME_SECS` and the `Display` says
  "Request handler timed out".

**The fix.** `framework/core/src/error.rs` (`to_diagnostic`, `:58`): the `Io` arm (`:69`) names a
runtime I/O failure, the 5xx redaction, `aether::set_error_reporter` (detail arrives with the
response's `X-Request-ID`) and the file, directory or socket the failed operation named; the
`HeaderTooLarge` arm (`:82`) names both budgets; `BodyLimitExceeded` (`:95`) names
`AETHER_MAX_BODY_BYTES`; `Timeout` (`:102`) names the handler deadline; `Display` (`:142`) says
"Request handler timed out". The doc comment (`:50-58`) states that `bind()` returns a boxed `Io` after
printing E1001, so a caller that feeds that value into `to_diagnostic` reads runtime wording.

**Red first.** The eight new `error.rs` tests were written before the fix: three failed for the right
reasons — `a_handler_io_failure_is_not_told_to_move_the_listening_port` (`:235`) printed the old
`AETHER_PORT` advice, and the payload-budget (`:255`) and timeout (`:304`) tests found no hint on
`HeaderTooLarge` and the wrong clock name on `Timeout`. The other five passed as pins.

**Tests that pin the contract.**
- `framework/core/src/error.rs:159` — eight tests: one value of every variant (`:165`) plus a count
  guard (`:183`), the exact twelve-pair code table (`:194`, which also pins that the diagnostic's code
  and reason are the ones `error_code`/`Display` produce), the Io report (`:235`), both payload
  budgets (`:255`), the timeout clock (`:304`), the 100-continue teaching (`:323`) and the structured
  serialization context (`:336`).
- `framework/core/src/diagnostic.rs:214` — four tests: every code string agrees with its own variant
  name (`:240`), every title is present and distinct (`:258`), the terminal form renders every field
  the builder accepts (`:271`, exact plain and coloured strings), and an empty builder renders no
  empty sections (`:315`).
- `framework/aether/src/app.rs:5071` — `every_aether_error_answers_the_status_class_its_code_names`
  walks all twelve variants through the **conversion**: `error_status`, `error_code`, the body's
  `code`, the 4xx detail and 5xx redaction, and then `Result::<&'static str, AetherError>::Err(..).into_response()`'s
  status, JSON body and absent internal detail. Before this track the status/code agreement was a
  comment and four variants on the wire.
- `framework/tests/tests/http_e2e.rs:2077` — the HTTP/1 `417` refusal now reads its declared body
  (`response(true)`, `:2088`) and asserts it is empty (`:2098-2103`) with `Content-Length: 0`
  (`:2104`), the same emptiness the h2 twin has asserted since track 116. The previous form used
  `response(false)`, which hardcodes a zero-length body, so the assertion could not fail.

**Sensitivity by injection (eleven mutants, each killed by a named test; `C:/tmp/t118/mutants.py`,
log `mutants_final2.log`).**

| ID | Injection | Result |
|---|---|---|
| M1 | restore the old port-advice `Io` arm | KILLED by `a_handler_io_failure_is_not_told_to_move_the_listening_port` |
| M2 | delete the `HeaderTooLarge` arm | KILLED by `the_two_payload_budgets_name_their_own_limit` |
| M3 | delete the `Timeout` arm | KILLED by `a_timeout_points_at_the_deadline_that_expired` |
| M4 | `Timeout` Display back to "Connection timeout" | KILLED by `a_timeout_points_at_the_deadline_that_expired` |
| M5 | `UnsupportedExpectation` mapped to E3001 | KILLED by `every_variant_keeps_the_code_the_error_contract_names` |
| M6 | E3003 shares E3001's title | KILLED by `every_title_is_present_and_distinct` |
| M7 | the coloured prefix loses its colour | KILLED by `the_terminal_form_renders_every_field_the_builder_accepts` |
| M8 | the 417 answers 400 | KILLED by `every_aether_error_answers_the_status_class_its_code_names` |
| M9 | the 417 refusal carries a body | KILLED by `an_unsupported_expectation_is_refused_before_the_body_is_sent` (the private-client variant; `response(true)` reads the body the mutant declares) |
| M10 | body refusal points at `ServerLimits` again | KILLED by `the_two_payload_budgets_name_their_own_limit` |
| M11 | head refusal names only the byte budget | KILLED by `the_two_payload_budgets_name_their_own_limit`; failure text names the missing `AETHER_MAX_HEADERS_COUNT` suggestion |

M11 was first written as a whole-block deletion, which was killed by a **compile error** rather than
the assertion; the case was rewritten to keep the arm syntactically valid, and the re-run above is
the assertion failure (`error.rs:267`). That is recorded because a mutant killed by a broken build
proves nothing about the test.

**Two independent reviews (separate reviewer agents, limited-review protocol), both SHOULD-FIX.**
- Review 1: `client.response(false)` made the new empty-body assertion vacuous, and the new lines in
  `http_e2e.rs` shifted two `cite_check.py` pins (4705/4741) and the Arabic guide's citation.
  Applied: `response(true)`, pins moved to 4717/4753 (`cite_check.py:425-426`,
  `framework/docs/ERROR_HANDLING_GUIDE_AR.md:122`).
- Review 2: the `HeaderTooLarge` hint named a budget that cannot fix a field-count refusal; the
  `BodyLimitExceeded` suggestion named a knob the facade cannot set; the record must attribute
  `bind_with_config` to `app.rs`, not `serve.rs`; the matrix sentence claimed a knob the code did not
  yet name (it did after the same pass); and the cross-product's code/reason equivalence is a
  derivation pin, not an independent oracle. Applied: both head budgets named (M11), the body knob
  corrected (M10), coordinates attributed to `framework/aether/src/app.rs:1992`, matrix wording
  corrected, and the derivation pin kept deliberately and described here as such. Review 2's snapshot
  also predated the final census; the 892/893 figures below were read from the completed
  `cargo test --workspace` log, not inferred.

**Refused alternatives, and why.**
- Branch the `Io` diagnostic on whether a listener exists — refused: the type carries no context that
  distinguishes startup from request time, and the bind failure has already printed its own E1001; a
  hedge would keep sending runtime failures to a port.
- Replace `bind()`'s `Io` with a typed startup variant — refused: `AetherApp::bind` returns
  `Box<dyn Error + Send + Sync>`, so this is a public breaking change and not a diagnostics track's
  call; it is named as an open residual instead.
- Make `to_diagnostic` the single source of the response body — refused: the body is client JSON and
  the diagnostic is developer terminal prose; the part that must not diverge is the code and reason,
  which the cross-product pins.
- Name only the byte budget on `HeaderTooLarge` — refused, and killed by M11.
- Drop the `to_diagnostic`/`error_code` assertions as tautological — refused: the table is the
  oracle, and those two assertions are the pin that a future rewrite cannot decouple the developer
  report from the wire code. They are described as a derivation pin, not counted as defect detection.

**Gate ladder.** `bash gates.sh` on this tree: fmt, clippy, tests, cite, evidence and validate PASS
(validate in its designed state: exit 1 with exactly the four pinned findings, no fifth). The first
govtests run failed with one finding — the track-index row below pointed at a `## Track 118` heading
that did not exist yet because this section was written after the row; on the re-run after this
record was added, govtests passes. The citation gate reads **112 citations in 16 live documents over
77 distinct spans, 0 findings** and resolves 382 qualified and 1378 bare names; the evidence gate
scans 35 documents with 0 findings. No gate was silenced and nothing was staged.

**Measured counts on the final tree.** Suite: **892 passed / 0 failed / 1 ignored** over **63**
`test result:` lines — unit 463 over 19 unit runs, integration 410 across 30 targets, doc 19 over 14
doc runs — cross-checked by the second rule (63 `running N tests` lines sum to **893** = 892 + the
one ignored test). Unit grew 450 -> 463 in exactly two lanes: `aether-core`'s lib line 8 -> 20
(eight `error.rs` + four `diagnostic.rs` tests) and `aether`'s lib line 96 -> 97 (the cross-product);
integration held at 410 because the wire change adds assertions to an existing test, and the doc lane
held. Per-file `#[test]` counts reproduce the log: `error.rs` 0 -> 8, `diagnostic.rs` 0 -> 4,
`app.rs` 20 -> 21, `http_e2e.rs` 93 -> 93. Feature matrix: **122 / 3 / 9 / 0 / 0 / 0 / 2 = 136**,
unchanged — this track edited the *Request diagnostics* row's evidence only, and no status cell
moved. The open problem list is the same four owner-gated items — PROB-0004, PROB-0005, PROB-0011 and
PROB-0012.

**What remains open after this track.**
- `to_diagnostic` still has no production caller: the tests are its only readers. Rendering it at the
  two places the framework builds an `AetherError` would be a UX decision with its own output-format
  question, not a silent extra print.
- `bind()` returns a boxed `Io` after printing E1001, so that value's `to_diagnostic` reads runtime
  wording. The doc comment states it; a typed startup variant is the public API change refused above.
- `framework/docs/FEATURE_COMPLETENESS_MATRIX.md:23`, `README.md:431` and
  `framework/docs/BASELINE_MEASUREMENTS.md:91,145` still print **848** from the track-115 dateline;
  tracks 116-118 did not sweep them. The live counters are the readiness report's readings and
  `OWNER_VISION.md`, which this track updated to 892.
- Delivery of these answers over TLS is not re-verified here; the tests are cleartext, as in track 117.

## Track 119 - every printed count is checked against the run that produced it (2026-11-04)

**The defect.** Tracks 60, 104 and 114 each re-derived the live test counts and each left the next
one to rediscover the same drift. On this tree five live documents still printed the track-115
reading - `README.md`, `framework/README.md`, the completeness matrix, the baseline and the
readiness report - and track 118's own closing note listed them as still saying 848. The readiness
report was worse than stale: its section 5 said 80 HTTP/2 tests while its section 10 bullets still
said 78 raw-socket and 67 HTTP/2, both right on track 110's tree and untouched since; the matrix's
live reading claimed 111 citations over 76 spans while the citation gate read 112 over 77. No gate
looked at a number: `cite_check.py` checks where a document points and `evidence_check.py` checks
that a path resolves, and a drifted count satisfies both. The structural fault is that every track
copied these numbers by hand from a log it then deleted with its scratch directory.

**The rule.** Every suite count in the five pinned documents equals the count in the run the ladder
just performed, and every citation count equals what the citation gate just resolved. The gate does
not compare two documents - that only tells two files to agree - it asks the tree.

**The gate.** `.project-governance/tools/count_check.py` (619 lines) carries **31 pins over 5
documents**: 12 `field` pins, 16 `target` pins, one `target_list`, one `crate_row` and one
`crate_row_list`. A pin is a regex for the sentence plus the `Census` field or fields it states; the
number itself is never stored, so a track that adds tests updates the documents and leaves the tool
alone, and a pin whose sentence is reworded away fails instead of silently losing the claim.

The census is read from `target/gates/tests.log`: 892 passed / 0 failed / 1 ignored over 63
`test result:` lines; unit 463 over 19 runs, integration 410 across 30 targets, doc 19 over 14 doc
runs. The citation figures come from `target/gates/cite.log`: 112 citations in 16 live documents over
77 spans, and 383 qualified / 1382 bare names, 34 sheltered (28 in the table), 9 attributed to
another crate, 1 exempted. `gates.sh` writes both before this gate runs.

The lane rule is the project's own: `Doc-tests` names the doc lane, `Running tests/<name>.rs`
names that integration target, and anything else is unit - with `where` reset after every result
line, so a summary no runner can place is a finding (`unplaced`) rather than a silent unit test.

It fails closed on: a missing log; a stale log (either log older than the newest
`.rs`/`Cargo.toml`/`Cargo.lock` under `framework/`, with `docs/` and `target/` skipped, so an
uncompiled doc example cannot redden it); an unparsed citation summary; a missing document; a
sentence matching zero or more than one time; and coverage in both directions - a nonzero crate the
run measured that no row names, and a target the run measured that no list names. `by_crate` reads
`deps/<crate>-<hash>(.exe)?`, so a Linux binary name maps to the same crate.

**What was corrected.**

| Document | Before | Now |
|---|---|---|
| `README.md` | 848 / 0 / 1; 78 raw-socket; 72 HTTP/2; "track-115 tree, 2026-11-03" | 892 / 0 / 1; 93; 80; "track-118 tree, 2026-11-04" |
| `framework/README.md` | 848 = 446 + 383 + 19 over 63 lines; 78 / 72 | 892 = 463 + 410 + 19; 93 / 80; the history paragraph now ends "tracks 116-118 added the 44 ... for 892 on this tree" |
| matrix | 848; 446; 383; rows 446 and 383; 78 / 72 / 10; 111 / 76 | 892; 463; 410; 463 and 410; 93 / 80 / 12; 112 / 77; the track-110 sentence now reads "At that reading `http_e2e` was 78, `http2_e2e` 67" |
| baseline | 446; 848; `aether-http` 106; `aether` 96 (`app.rs` 21); `aether-core` 8; total 446 + 383; target list 78 / 72 / 10 / 5 / 3 and 383 | 463; 892; 110; 97 (`app.rs` 22); 20; 463 + 410; 93 / 80 / 12 / 6 / 4 and 410; the zero-crate row unchanged |
| PRR | section 10: 78 and 67; section 5: "49 tests" dated 2026-10-30; `app.rs` 20 -> 21 | 93 and 80 with their per-track histories; 80 dated 2026-11-04; `app.rs` 21 -> 22 |

The matrix's present-tense "On this tree `http_e2e` is 78, `http2_e2e` 67 ... `streaming_request`
10" became past tense, because it holds the track-110 reading inside a paragraph whose live numbers
now sit in the row above it.

**The arithmetic error the recount found.** The committed baseline row read `aether` (lib) 96 with
`app.rs` 21, summing to 96, while the crate's own `test result:` line measured 97. The re-derived
split is `tls.rs` 26 + `app.rs` 22 + `extensions.rs` 17 + `request_body.rs` 5 + `transport.rs` 3 +
`static_dir.rs` 8 + `auth.rs` 1 + `http2.rs` 15 = 97, and `git show` of track 118's behaviour commit
counts 22 `#[test]` attributes in `app.rs` against 21 at the commit before it. So the file went
21 -> 22, not 20 -> 21 as the track-118 record said: the cross-product is the twenty-second test.
The live row and the PRR sentence were corrected here; the track-118 record is left as written
history.

**Tests that pin the gate.** `.project-governance/tools/tests/test_count_check.py` (376 lines, **36
tests**): five census tests (the lane split, a summary with no runner, a Linux binary name, the
citation summary with and without its "against the tree's own sources" clause, and an unparsed
summary leaving zero); twenty-eight synthetic-tree tests that make the gate fail on a wrong total,
lane split, per-target count, target-list row, crate row, citation figure, a reworded sentence, a
duplicated sentence, an empty list, a missing document, a missing log, a stale test log, a stale
cite log, an unparsed summary, a runner-less log, a wrong `failed`/`ignored`, a pin whose target did
not run, a crate the table does not name, a target the list does not name, and a docs-only source
newer than the log (which must stay green), plus the line number every finding carries and `main()`
returning 0 then 1; and three repository tests - every pin names exactly one live sentence, the pin
set covers exactly the five documents, and the documents agree with the last `gates.sh` run unless
it has not happened yet. The governance suite is 112 tests before this track's file and **148** with
it.

**Sensitivity by injection (eighteen mutants, each caught; `C:/tmp/t119/mutants.py`, log
`C:/tmp/t119/mutants_final.log`).**

| ID | Injection | Result |
|---|---|---|
| M1 | README total 892 -> 893 | CAUGHT at `README.md:21` |
| M2 | `framework/README.md` unit lane 463 -> 462 | CAUGHT at `framework/README.md:431` |
| M3 | matrix `http2_e2e` 80 -> 79 | CAUGHT at `FEATURE_COMPLETENESS_MATRIX.md:448` |
| M4 | baseline `aether-core` row 20 -> 19 | CAUGHT at `BASELINE_MEASUREMENTS.md:137` |
| M5 | PRR `http2_e2e` 80 -> 79 | CAUGHT at `PRODUCTION_READINESS_REPORT.md:765` |
| M6 | reword the matrix citation sentence | CAUGHT: "this claim is not in the document any more" |
| M7 | duplicate the baseline total row | CAUGHT: "this claim matches 2 times" |
| M8 | corrupt the citation summary | CAUGHT: "this log has no citation summary to compare against" |
| M9 | delete the tests log | CAUGHT: "there is no run to compare the documents against" |
| M10 | touch a source newer than the logs | CAUGHT: "this log predates the tree it would be compared with" |
| M11 | baseline target list `http_e2e` 93 -> 92 | CAUGHT at `BASELINE_MEASUREMENTS.md:143` |
| M12 | README ignored 1 -> 2 | CAUGHT at `README.md:21` |
| M13 | remove the `aether-core` crate row | CAUGHT: "the run measured 20 tests in `aether-core` and no table row names it" |
| M14 | remove `http_e2e` from the target list | CAUGHT: "the run measured the `http_e2e` target and the target list does not name it" |
| M15 | PRR raw-socket bullet 93 -> 92 | CAUGHT at `PRODUCTION_READINESS_REPORT.md:1109` |
| M16 | PRR live reading 892 -> 891 | CAUGHT at `PRODUCTION_READINESS_REPORT.md:718` |
| M17 | remove a result line's runner line | CAUGHT: "1 `test result:` line(s) have no preceding runner line" |
| M18 | the zero-crate row claims 1 | CAUGHT at `BASELINE_MEASUREMENTS.md:144` |

The exported copy with no mutant is silent (0 findings), and the repository was byte-identical after
the run.

**Harness isolation, and the incident it answers.** The first harness mutated the repository in
place. One exact-restore failed on Windows with `OSError: [Errno 22] Invalid argument`, leaving the
real matrix at `(79, five of them` for `http2_e2e`. It was restored to `(80,` at once and verified
to the byte - CRLF count 817, 0 bare LF, counts gate green - and the harness was rewritten to export
the five documents and both logs to `C:\tmp\t119\tree` and mutate only that copy, per the owner's
isolation rule. All eighteen results above come from the exported copy; the repository was not
written again.

**One independent review (limited-review protocol, no-write), SHOULD-FIX.** Seven findings; every
one was applied:
1. PRR section 10's 78 and 67 were stale while section 5 said 80 - corrected to 93 and 80 with their
   histories, and pinned as targets (M15, M5).
2. PRR's live reading had no pin, so a later track could update four documents and leave it behind -
   a ten-field pin now reads it (M16).
3. `failed` and `ignored` were parsed but never compared, so an `#[ignore]` added to the suite would
   leave five "1 ignored" claims false and the gate green - both are fields of the README and PRR
   pins (M12).
4. Coverage was one-directional: a row was checked only if a regex matched it, so a crate or target
   gaining its first tests could leave a stale `0` or a missing entry - coverage now runs both ways,
   and the zero-crate row is pinned through `crate_row_list` (M13, M14, M18).
5. `DEP_CRATE` ended in `\.`, so a Linux dep line never matched and every crate row would report
   "no unit run" - fixed to `(?:\.exe)?` with a Linux-name fixture test.
6. `test_an_unknown_target_is_reported` stopped its pin from matching instead of exercising the
   unknown-target branch - it now uses a synthetic pin whose sentence matches and whose target the
   log lacks.
7. Freshness read the uncompiled `framework/docs/*.rs`, and `main()`'s exit code and a stale *cite*
   log were untested; the `census_from_log` docstring said a runner-less line is "refused" where
   `check` refuses the whole log - `docs/` and `target/` are now skipped by freshness (with a test
   that a docs-only edit stays green), two tests were added (a stale `cite.log` reported on its own,
   and `main()` returning 0 then 1), and the docstring's wording is the whole-log refusal `check`
   performs. The reviewer is read-only and ran nothing; the 36 tests and the eighteen mutants were
   run here.

**Refused alternatives, and why.**
- Keep the numbers in one checked-in manifest every document quotes - refused: two documents
  agreeing with each other is exactly what the gate exists not to trust, and the manifest would
  drift from the run just as the prose did.
- Run `cargo test` inside the gate - refused: the ladder's `tests` gate already does, and a
  comparison should read the log that run wrote rather than rebuild the workspace a second time.
- Pin a hash of each document - refused: any wording edit fails the gate while saying nothing about
  whether the number is right.
- Skip when a log is missing (the fresh-checkout case) - refused: a skipped comparison is the
  vacuous green this track removes; `gates.sh` writes both logs before `counts` runs.
- Require a row for every crate the workspace builds, zero included - refused in one direction:
  `aether-runtime` and `aether-validation` build no test binary, so only nonzero crates require a
  row; the zero row is still pinned with its claimed count (M18).
- Pin the per-file `#[test]` splits the baseline states - refused: that would compile a second
  census out of source text; the crate row's total and the log agree, and the split is prose
  re-derived by hand - which is how the `app.rs` error above was found.
- Pin the historical sentences in the register, `WORKING_CONTEXT.md` and the dated reports -
  refused: they are history, and rewriting them would make them different records.

**Gate ladder.** `bash gates.sh` on this tree: fmt, clippy, tests, cite, evidence, counts and
govtests PASS, and `validate` exits 1 with exactly its four pinned findings and no fifth - the
ladder's designed state, recorded rather than silenced. The new gate reads `5 documents scanned,
0 finding(s)`; the citation gate reads 112 citations in 16 live documents over 77 distinct spans
and resolves 383 qualified and 1382 bare names; the evidence gate scans 35 documents with 0
findings; the governance suite ran 148 tests, all passing; and the `tests` log holds 892 passed /
0 failed / 1 ignored over 63 `test result:` lines. The two evidence-tool files whose only diff is
line-ending normalisation are left unstaged, as they have been for every track since.

**What remains open after this track.**
- The gate checks numbers, not prose: a sentence whose number is right can still misdescribe it (the
  matrix's "five of them the signed cookie read" qualifier is unchecked).
- The per-file splits in the baseline's prose are not gated; only the crate totals they sum to are.
- The citation pins compare the documents with the citation gate's own summary, so a pin can agree
  with a wrong summary; that summary is itself derived from the corpus, not independently measured.
- The Linux binary-name lane is tested synthetically; no Linux run has been made (PROB-0012 stays
  owner-gated).
- The dated records keep their 848 figures behind their snapshot markers, by policy.

## Track 120 - the matrix row that contradicted its own evidence cell (2026-11-04)

**The defect.** The completeness matrix's *Aether* column marked **Bounded response memory**
`NOT_IMPLEMENTED`, while the same row's evidence cell had described the priced response since tracks
74-75 and named the tests that hold it. No count could see it: the summary table (122 / 3 / 9 = 136)
agreed with the rows, and both disagreed with the tree. The summary had drifted from its rows three
earlier times (tracks 96, 98b and 99-100), which is why track 119 built a gate for printed counts;
this defect sat one level below that gate - a status wrong about the product rather than about the
table.

**The verification, before the flip.** By substring, the three HTTP/2 tests the row names pass
(`a_response_body_is_registered_by_its_block_while_the_stream_holds_it`,
`a_response_body_over_the_budget_is_sent_anyway` and
`a_reader_that_stops_gets_a_stream_reset_without_killing_the_connection`), and the four functions the
cell names exist (`price_response_body`, `price_stream_chunk`, `reprice_conn_memory`,
`release_stream_write_buf`). The status cell was the last stale copy in a row whose prose had already
been corrected twice.

**What was corrected.**

| Copy | Before | Now |
|---|---|---|
| matrix row 422 | `NOT_IMPLEMENTED` | `VERIFIED_IMPLEMENTED` |
| matrix summary table | 122 / 3 / 9 | 123 / 3 / 8 |
| matrix current-reading line | absent | `123 / 3 / 8 / 0 / 0 / 0 / 2 = 136 rows`, re-derived by the gate |
| matrix history paragraph | ended at track 116 | a track-120 sentence; the pair-set sentence now says "unchanged through track 116" |
| matrix *README / docs accuracy* | "checked mechanically on two axes" | "four axes", with the `counts` gate named in the evidence cell |
| PRR section 3 table and parenthetical | 122 / 3 / 9; "recounted mechanically on 2026-10-31" | 123 / 3 / 8; 2026-11-04 |
| PRR recount sentence | "The 2026-10-31 recount reads 122 / 3 / 9" | "The 2026-11-04 recount reads 123 / 3 / 8 ... (the 2026-10-31 run read 122 / 3 / 9)" |
| PRR not-delivered paragraph | "9 `NOT_IMPLEMENTED` ... on 2026-10-31" | "8 ... on 2026-11-04" |
| OWNER_VISION live paragraph | 122 / 3 / 9; the gate "reads 31 sentences" | 123 / 3 / 8; the sentence names no pin count (see below) |
| roadmap section 19.8 | 9 rows, *Bounded response memory* listed among them | 8 rows; the row left the list |

The vision's "31 sentences" is the drift this track exists to stop, one document over: track 119
wrote it, this track's first pass made it 42, and the review found it still saying 31. Rather than add
a pin that counts pins, the sentence now says the gate "reads every pinned suite, citation and status
count in six live documents", and the register carries the gate's size per track.

**The gate.** `count_check.py` carries **52 pins over six documents**: the 31 log pins from track 119,
17 status pins, and 4 pins on `OWNER_VISION.md`'s live paragraph. `status_census` recounts the matrix
file-wide - its 13 category tables share the five-column shape - with the rule the matrix states:
exactly five cells, header and separator skipped, the status taken before the first space or
parenthesis, and a misspelled status counted in the total only, so a typo surfaces as a mismatch.

Each reading pin carries its own status order in `names`, because the matrix prints BROKEN and
UNVERIFIED before UNWIRED while the readiness report prints UNWIRED first. All three are zero today,
which is exactly why one shared order would have stayed invisible until one of them moved; the review
caught it, and a test now moves a row to BROKEN to pin the order. The three zero cells are pinned in
both documents, so a row moved into one cannot leave a printed 0 behind.

**Tests that pin the gate.** `test_count_check.py` is **48 tests** (46 after the first pass, two added
by the review): the documented rule, a misspelled status, a correct table left silent, a wrong cell, a
wrong total, a wrong reading, a reading in the matrix's own order, a zero cell, a row that moves caught
in both the cells and the reading, a six-cell row ignored, a lost summary row, a missing source, and
the repository tests that every pin names one live sentence, that the pin set is exactly the six live
documents, and that those documents agree with the last ladder run. The governance suite is **160
tests** (`Ran 160 tests in 81.821s` / `OK`).

**Sensitivity by injection.** Eighteen document mutants on the exported tree
(`C:/tmp/t120/mutants2.py`), seventeen caught and the six-cell null arm survived: matrix VERIFIED
123 -> 122 (`:498`), NOT 8 -> 9 (`:500`), total 136 -> 135 (`:506`), reading 123 -> 122 and total
136 -> 135 (`:508`), PRR VERIFIED (`:114`) and reading (`:135`), a row flipped VERIFIED -> NOT
(`:498`, `:500`, `:508`), a five-cell row added without a recount (`:499`, `:507`, `:509`), a
summary row removed, a misspelled status (`:498`, `:508`, and PRR `:114`), matrix BROKEN 0 -> 1
(`:501`), PRR BROKEN 0 -> 1 (`:118`), and the vision's 892 -> 891 (`:234`), 123 -> 122 (`:237`),
112 -> 111 (`:238`) and 1382 -> 1381 (`:239`). Three code mutants on a copy of the tools tree were
each killed by a named test: the header guard removed and the separator guard removed both redden
`test_the_documented_rule_counts_five_cell_rows_only` (the fixtures now carry the real five columns),
and restoring `zip(STATUS_ORDER, ...)` in the reading branch reddens
`test_a_reading_follows_the_matrix_tables_order` with the exact message the review predicted
(`UNWIRED_IMPLEMENTATION is printed as 1; the rows count 0`). The first harness aborted at its fourth
mutant because the pinned reading string now occurs in the history sentence as well; it was anchored
to the current-reading line and re-run. The clean tree reads `6 documents scanned, 0 finding(s)`, and
the repository was not written: each mutant is restored from a backup copy of the exported tree.

**The ladder.** `bash gates.sh`: fmt, clippy, tests, cite, evidence, counts and govtests PASS;
validate exits 1 with exactly the four pinned findings. `counts` reads six documents with 0 findings;
`govtests` runs 160 tests, OK.

**One independent review (limited-review protocol, no-write), SHOULD-FIX.** Every finding was
applied:
1. The reading branch zipped one global order, which is the PRR's, against the matrix's table, which
   prints BROKEN and UNVERIFIED before UNWIRED - invisible while all three are zero. Each pin now
   names its own order, with `MATRIX_STATUS_ORDER`, and a test moves a row to BROKEN.
2. Only the four nonzero statuses had cell pins; a row edited into BROKEN, UNVERIFIED or UNWIRED in
   either document left its printed 0 unchecked. Six zero-cell pins were added (mutants M13, M14).
3. The synthetic header and separator were four-cell, so deleting either guard left every test green.
   The fixture now uses the real five-column shape, and both guards are killed by code mutants.
4. The row-flip test could not tell a cell finding from a reading finding because the messages are
   identical; the dedicated wrong-reading and order tests now cover that branch on their own.
5. Three printed copies were still stale: the PRR's section 3 date, its "9 `NOT_IMPLEMENTED`"
   paragraph, and the vision's "31 sentences". All three were corrected; the vision's track-119
   history entry keeps its 31, by the register's rule that dated records are not rewritten.
6. `OWNER_VISION.md` was not in the pinned set at all, so its four copied figures were checked by
   nobody. Four pins now hold them to the same run (M15-M18).
7. The module docstring said the table "drifted from its own rows four times ... and 120"; track 120's
   cell agreed with its rows, so it was a different class. The docstring now says three times and
   states the distinction. `status_census`'s docstring also now says the recount is file-wide over 13
   tables and takes the text before the first space or parenthesis, which is what the code does.
8. The matrix's "the pair set is unchanged" sat in the same paragraph as track 120's first pair change
   since track 108; it now reads "unchanged through track 116".

The reviewer ran nothing and could not re-diff the pair set against `9329a19`; the tests, the
mutants and the ladder above were run here after applying the fixes.

**Refused alternatives.**
- A `pin_count` kind so the vision could keep stating the gate's size - refused: the sentence now
  names no number, and a pin that counts pins makes the sentence's own value depend on itself.
- Gate the row statuses against the tree - refused: there is no mechanical definition of "implemented"
  independent of the tests the row names. What can be gated is that the summary equals the rows (now
  done) and that the named tests exist and pass (`cite_check` plus the suite).
- Recount each of the 13 tables separately - refused: the summary is over all rows, and thirteen
  per-table totals would be thirteen numbers with no reader.
- Compare the matrix's summary with the readiness report's copy - refused: two documents agreeing is
  exactly what the count gate exists not to trust; both are held to the row recount instead.

**What this track does not claim.** The gate keeps the summary equal to the rows; it cannot tell
whether a row's status is true. Track 120's defect was found by reading the row against its evidence
and the tree, and that remains a review step. No performance number was produced.

## Track 121 - the figures around the figures are pinned too (2026-11-05)

**What was open.** Tracks 119 and 120 pinned every suite total and every status-cell count to the run
that wrote them, but three printed figures still had no mechanical source: the workspace crate count
(the root README and the completeness matrix), the document-gate corpus size (the matrix), and the
`running N tests` cross-check beside the suite total (the readiness report and `OWNER_VISION.md`).
A gate that is green is not evidence that every number in the documents is checked.

**What is now compared, and against what.**
- `workspace_members`, a new pin kind: counts the quoted entries of `members = [...]` in
  `framework/Cargo.toml`. It pins the root README's "Cargo workspace: 16 crates" and the matrix's
  "16 workspace crates".
- `corpus`, a new pin kind: reads "N documents scanned" from `target/gates/evidence.log`. It pins the
  matrix's "an identical corpus (33 -> 35 documents)".
- a `field` pin over `("running", "passed")`: the census sums every `^running N tests$` line of the
  run (893) and compares it with the printed passed count (892) in the same sentence, in both the
  readiness report and the vision. The explanation beside a total can no longer keep an old half.

That is five pins, 52 -> 57 over six documents. The gate's tests went 48 -> 60, and `gates.sh`'s
`counts` header now names the three logs and the manifest instead of "the two logs".

**Sensitivity.** On an exported tree under `C:/tmp/t121`, twelve document and source mutants each
went red with the expected message: crates 16 -> 15 in both documents; corpus 35 -> 36; running
893 -> 894 and passed 892 -> 891 in both documents; the README sentence reworded away; one member
removed from the manifest; the evidence corpus read as 34; the evidence sentence removed; the
evidence log deleted. Two code mutants were killed by the gate's own suite: dropping the `running`
accumulation failed five tests, and a greedy members pattern that counts `default-members` failed
five. The exported tree returned to 0 findings after every restore.

**Measured values.** 16 workspace crates, 35 documents in the evidence corpus, and 893 running lines
= 892 passed + 1 ignored. No document changed in this track: the three printed sets were already
true, which is the point of pinning them rather than trusting them.

**Refused alternatives.**
- Count every quoted string in `Cargo.toml` - refused: `default-members = ["examples"]` is not a
  workspace member, and a count that includes it is off by one.
- Derive the corpus from `cite.log` - refused: the citation gate and the evidence gate scan different
  sets, and the sentence names the evidence gate's own reading.
- Compare the running sum with `result_lines` - refused: the cross-check states
  `running = passed + ignored`, so the passed count is the half it has to agree with.
- Pin the same figures inside the dated records - refused: a dated record keeps the numbers of its
  own run; only live sentences are re-derived.

**What this track does not claim.** A matching digit is not a true sentence. The pins match a
sentence shape, so a reworded or deleted claim fails closed rather than being understood. No
performance number was produced.

## Track 122 - the locks on the request path, and the false red that produced a guard (2026-11-05)

**What was open.** Reading the path, not timing it, named process-wide locks per response: two in
the scheduler's admission fast path, one in the Date cache, and the scheduler's release. The second
admission acquisition and the Date cache's lock guarded values that did not need a shared lock at
all. The owner's rules governed the track: no delta is attributed without a controlled A/B, no
average that hides a P99 change, and nothing merges on a microbenchmark alone.

**Change 1 - the fast path re-read a state it had just set (`24f9805`).** `admit` took the engine
lock, enqueued, pumped, released, then re-acquired the lock to ask whether the job had become
active. That state can only change by release or cancellation, both of which need the id - which
`admit` has not returned yet - so the second acquisition could not observe anything the first did
not. The state is now read under the admitting lock, and the wait loop's `pump_now` plus separate
state check became one `pump_and_state`, removing one acquisition per wake-up as well. The
scheduler's 11 tests and aether's 97 pass.

**Change 2 - the Date cache serialised every response (`de67910`).** The cache reused one formatted
IMF-fixdate per second, but behind a process-wide `Mutex`: every response queued for a string that
was identical for all of them. It is now a thread-local; a miss costs one `format!` per second per
thread that writes responses, and the existing cached-vs-uncached test still pins byte equality.
Aether-http's 110 tests (one ignored) and aether's 97 pass.

**The false red, and the guard it produced (`f8ee1aa`).** A `gates.sh` run interrupted from the
terminal kept writing `target/gates/tests.log` while the next run wrote the same file: the log
spliced a truncated `inished in 0.00s` line and a second doctest tail into the run, and `counts`
reported 22 findings (892 printed against 896 measured) against a tree whose figures were
unchanged; `govtests` failed its repository test for the same reason. Re-running on a clean log was
green, which proved the corruption rather than a regression. `gates.sh` now takes an atomic `mkdir`
lock: a live holder is refused with exit 2 and its pid, a dead pid is taken over, and the lock is
removed on exit and on INT/TERM. All three states were exercised by hand.

**Sensitivity, and what is not testable.** No test observes a lock count; the evidence for changes
1 and 2 is behaviour equivalence (the suites above and the cached-date byte-equality test) plus the
code shape, not a timing claim. The guard was exercised with a live holder (refused), a dead pid
(taken over), and a TERM mid-run (no lock left behind).

**Measured values.** The clean run is unchanged from track 120: 892 passed / 0 failed / 1 ignored
over 63 result lines, the `running N tests` lines summing to 893; unit 463 over 19 suites,
integration 410 over 30, doc 19 over 14. The ladder passes its eight gates; validate exits 1 with
exactly its four pinned findings.

**Measurement handoff, not a result.** Four release binaries were built and their hashes differ:
`framework/target/ab/t122-base.exe` (`72b27e4`), `t122-admit.exe` (`24f9805`), `t122-dateonly.exe`
(base plus the Date change alone) and `t122-date.exe` (`de67910`). The owner measures one change per
pair, arms rotated, at eight workers:

```
powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo_ab.ps1 `
  -Base framework\target\ab\t122-base.exe -Pgo framework\target\ab\t122-admit.exe `
  -Workers 8 -Reps 3
powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\pgo_ab.ps1 `
  -Base framework\target\ab\t122-base.exe -Pgo framework\target\ab\t122-dateonly.exe `
  -Workers 8 -Reps 3
```

**Refused alternatives.**
- Keep the second admission lock for clarity - refused: the value is unobservable before the id is
  public, and a lock that guards nothing is a cost with no invariant behind it.
- Mirror the job state in an atomic outside the lock - refused: a second source of truth for a
  value the same lock already holds.
- Put the Date cache behind `ArcSwap` or an atomic pointer - refused: more machinery (and unsafe)
  for a string that a thread-local regenerates once per second.
- Give each run its own log filenames instead of a lock - refused: the FAIL lines, the count gate
  and the documentation all name `target/gates/*.log`.
- Use `flock` for the guard - refused: not portable to Git Bash on Windows; `mkdir` is the atomic
  primitive both platforms have.
- Commit both lock changes as one diff - refused: the owner asked for each effect separately, and
  the arms above exist to keep that possible.

**The native-code note.** `e2fd36a` records the owner's instruction to keep C/C++ as a measured,
narrow kernel behind an FFI boundary, with the four conditions and the candidate classes. Nothing
in it is scheduled; it is a target of opportunity, not a direction.

**What this track does not claim.** No average, P99, rps, CPU or memory number was produced here;
the owner's run decides whether each change is kept. The guard changes no gate's verdict logic.
The two evidence-tool files stay unstaged: their diff is line-ending normalisation with no content
change.

## Track 123 - two request-dependent answers that omitted `Vary`, and an identity body sent to a client that refused identity (2026-11-05)

**What was open.** Reading the shared response path (not timing it) named two defects, both of the
same kind: an answer whose correct form depends on the request was written without telling a shared
cache that it does. In `CompressionMiddleware::apply`, a handler that refused identity
(`Accept-Encoding: identity;q=0`, or `*;q=0` with no more specific entry) was answered `200` with
the stored body on every path that skipped the transform: a body below the size floor, a
precompressed media type, a streamed body, a `206`/`Content-Range` answer and a
`Cache-Control: no-transform` answer. RFC 9110 §12.5.1 makes identity acceptable only because it is
not excluded, and its exclusion with no coding left to send is a `406` (§15.5.7), not an identity
body. In `CorsMiddleware`, a denied origin returned before any `Vary`, and a refused preflight
returned before the three fields that decided the refusal were named, so a shared cache could
replay a denial (or an approval) to a request it was never made for (RFC 9111 §4.1).

**Change commit.** Commit d4c72dd carries both fixes, the fifteen unit tests, the four
HTTP/1.1 wire pins and the re-derived live documents; this record is the commit that
follows it.

**Fix 1 - content negotiation (`compression.rs`).** `accepts_identity` decides once whether the
stored representation is acceptable. A handler-set `Content-Encoding` returns first: that coding is
the handler's own answer to `Accept-Encoding`, and this middleware neither overwrites nor refuses
it. An empty body (and a bodyless status, as before) returns before negotiation: a redirect or an
error has no representation whose coding could be refused. `Vary: Accept-Encoding` is written
before every decision, so a refusal keeps it too. On the untransformable path (stream, `206`,
`Content-Range`, `no-transform`) an identity refusal becomes `406`; when identity is acceptable the
body is served as stored. The size floor and precompressed-MIME skip now apply only when identity
is acceptable - the coding the client named is the only body left to send otherwise - and a selected
coding is sent even when it is larger than the original. `refuse_not_acceptable` answers a bodyless
`406` and strips the metadata that describes the withheld representation (`Content-Type`,
`Content-Length`, `Content-Encoding`, `Content-Range`, `Content-Disposition`,
`Content-Language`, `ETag`, `Last-Modified`, `Transfer-Encoding`), keeping `Vary`.

**Fix 2 - CORS refusals (`cors.rs`, `serve.rs`).** A denied origin now adds `Vary: Origin` before
returning. `apply_preflight` writes `Vary: Origin, Access-Control-Request-Method,
Access-Control-Request-Headers` before it resolves the origin or validates either requested field,
so every denial carries the key it depends on; under a wildcard origin the `Origin` token is still
written, because the `204`/`403` outcome depends on the request fields regardless of the advertised
origin. In `serve.rs` the preflight answer runs the middleware unconditionally (the previous
short-circuit skipped it entirely when the requested method was not routable) and an approval
requires both; a rebuilt `403` keeps the middleware's `Vary` lines and nothing else, so an approval
header cannot leak onto a refusal.

**Tests.** Fifteen `aether-middleware` unit tests (76 -> 91): identity refused with no coding and
with `*;q=0`; a selected coding sent even when larger than the original (gzip header/trailer growth,
round-tripped); below the size floor still coded; precompressed MIME still coded; range, stream and
`no-transform` refused; handler-chosen coding left alone; bodyless status and empty body kept; the
`406` strips the withheld metadata; and the three CORS variances (denied origin, refused preflight,
unlisted origin). Four HTTP/1.1 wire pins in `tests/http_e2e.rs` (93 -> 97): a denied origin still
varies on `Origin`, a refused preflight varies on the fields that decided it, an approved preflight
for an unroutable method is answered without CORS headers, and a refused identity is answered
bodyless `406` with `Content-Length: 0` and `Vary` while the coded sibling gets the gzip body. The
wire test opens a fresh connection for its second request: the harness does not reconnect after
`Connection: close`, and a reused socket is the test's defect, not the server's.

**Sensitivity by injection.** Seven mutants, each run against the targeted tests and each restored
with a byte-identical file checked by sha256: disabling `accepts_identity` killed six identity
tests; removing the denied-origin `Vary` killed its unit and wire pins; moving the preflight `Vary`
after the checks killed two; removing the `serve.rs` `Vary` copy killed the refusal wire pin;
restoring the old floor/precompressed skip killed three; disabling the handler-coded guard and
disabling the empty-body guard killed one each. A test that only asserted statuses without a gzip
round-trip was rewritten after the review below; the shape-only version was green on a garbled
body.

**Independent review.** One limited review (read-only, one verdict) returned SHOULD-FIX and every
finding was applied: an empty body (redirect, framework error) must not become `406`; the `406`
must strip `ETag`, `Last-Modified`, `Content-Disposition` and `Content-Language`; two shape-only
tests needed a round-trip; range, streamed and metadata tests were missing; and the two wire pins
above were missing. What stayed unverified in that review is restated below.

**Measured values.** The clean run is 911 passed / 0 failed / 1 ignored over 63 `test result:`
lines, with the `running N tests` lines summing to 912; unit 478 over 19 unit suites
(`aether-middleware` 91), integration 414 over 30 targets (`http_e2e` 97), doc 19 over 14.
Citations: 112 over 77 distinct spans in 16 live documents; names: 383 qualified and 1384 bare, 0
findings. The ladder passes its eight gates; `validate` exits 1 with exactly its four pinned
findings.

**Refused alternatives.**
- Keep answering identity on every skip path - refused: RFC 9110 §12.5.1 makes the body
  unacceptable, and the old answer made the client's explicit refusal silently ineffective.
- Compress a response whose handler already chose a coding - refused: that field is the handler's
  answer; silently replacing it would corrupt a signature or a precompressed byte range.
- Answer `406` for every empty body - refused: there is no representation whose coding is refused,
  and the first version broke redirects and framework errors until the review caught it.
- Serve a stream/range as stored even when identity is refused - refused: the client excluded the
  only coding that path can produce.
- Keep the `Content-Type`/`ETag`/`Last-Modified` of the withheld representation on the `406` -
  refused: they describe a body the answer does not carry.
- Vary on `Origin` only in a preflight - refused: the `204`/`403` outcome changes with both
  `Access-Control-Request-Method` and `Access-Control-Request-Headers`.
- Short-circuit `apply_preflight` when the requested method is not routable - refused: the refusal
  then carries no `Vary`, which is the defect this track closed.
- Rebuild the `403` in `serve.rs` without copying the middleware's `Vary` - refused: that discards
  the cache key the middleware had just written.
- Omit the `Origin` token under a wildcard `*` - refused: the outcome still depends on the request
  fields, and one rule is easier to verify than a conditional one.

**What this track does not claim.** No performance number was produced; nothing here was timed. The
wire pins are HTTP/1.1 only - the middleware is frontend-agnostic and unit-tested, but no HTTP/2
CORS or compression wire test was added, so the h2 path is not claimed by these pins. No real
shared cache was exercised: the tests pin the headers and the RFC argument is stated, not
demonstrated against a deploying cache. The two evidence-tool files stay unstaged: their diff is
line-ending normalisation with no content change.

## Track 124 - the stage ladder rebuilt on this tree, and the harness that had been inflating every production figure (2026-11-05)

**What was open.** The principal directive (2026-11-05) forbids an optimization without the profile
or measurement that justifies it, and the ranked audit that followed it left one agent-actionable
measurement item: the stage ladder had not been rebuilt since 2026-10-20, while its source comments
still cited `app.rs` line numbers the tree has long since moved. This track rebuilds it against
989f488, re-prices every rung, and - before reading any number - asks whether the instrument still
measures what it claims.

**Method.** `cost_probe`, `aether_server` and `aether_sched_server` rebuilt from the working tree in
`aether-bench-compare` (`cargo build --release`), then `layer_ladder.py --reps 3 --conns 8
--per-conn 3000 --count 1` on that box: 8 connections, 3000 requests each, 24 000 measured per cell,
cell order rotated every rep, `RL_THREADS=6`, every arm counted (`counting=1`). The request is
`GET /plaintext` with four field lines (Host, User-Agent, Accept, Connection), the response
`Hello, World!`, and the client fails a run whose body differs (0 failures in every cell). The
historical trees were exported with `git archive` and built the same way into `C:/tmp/t124`, which is
scratch and does not outlive this record; the two commands just given rebuild any of them.

**A harness defect, found before the numbers were read.** The counted window opens before the
measured client invocation and closes after it, so it covers that invocation's own warmup as well as
its measured requests, and `rlbench` sends `warmup = 200` per connection inside every run. The probe
prints a `served` counter and so divided by the truth; the production servers print none, and
`layer_ladder.py` fell back to the client's `completed` count - 24 000 where the server had served
25 600. Every production figure it printed was therefore 1 + 200/3000 = **6.67 % high**. It surfaced
the only way it could: the tree at 23999a7 (G15, the commit that produced the 2026-10-24 allocation
baseline) read **33.07 allocs/req** under the old denominator while the register attests **31.00**
for that tree. The denominator was the whole difference: `denom = served if served > 0 else reqs +
conns * WARMUP_PER_CONN` (with `WARMUP_PER_CONN = 200` beside `rlbench`'s own constant) prints
**31.01** for the same binary, the 0.01 being its per-connection intercept. `us_cpu_per_req` was
corrected the same way, so every CPU/req figure this harness printed before 2026-11-05 is 6.67 %
high - which changes none of the conclusions drawn from it, because the CPU column was already
recorded as unusable on this box.

**Rung table** (allocs/req and bytes/req, with the per-request delta from the rung below; spread
0.00 over three reps on every allocation cell):

| rung | adds | allocs/req | delta | bytes/req | delta |
| --- | --- | --- | --- | --- | --- |
| L0 | socket + static write (the ceiling) | 0.00 | - | 18.7 | - |
| L1 | the real parser | 0.00 | +0.00 | 18.7 | +0.0 |
| L2 | header materialization (`header_stores`) | 18.00 | +18.00 | 1286.7 | +1268.0 |
| L3 | request identity (`next_request_id` + clones) | 22.00 | +4.00 | 1342.7 | +56.0 |
| L4 | response writer | 25.01 | +3.00 | 1609.7 | +267.0 |
| L5 | radix-trie lookup | 25.01 | +0.00 | 1609.7 | +0.0 |
| L6 | scheduling rung | 27.01 | +2.00 | 1673.7 | +64.0 |

**What the table says.** The scheduling rung is the ladder's own self-check and it still holds:
L6-L5 is +2.00 allocs/req and +64.0 B/req, and the production `aether_sched_server` arm is +2.00
allocs/req and +64.1 B/req over the unscheduled one in the same session. A rung that had drifted from
the tree would show up here as a scheduling delta the production mirror does not have. The parser
rung is now **+0.00**, where 2026-10-20 recorded +1.00 allocs/+64 B: that allocation was the 100-slot
`httparse` array and G15 (track 46) removed it. Header materialisation is unchanged at **+18.00
allocs/+1268 B**, still the largest single site and 60 % of the per-request count, and still
PROB-0005 - an owner decision, not a task. Route lookup is +0.00, the value the 2026-10-20
prediction gave.

**The production series, one client and one harness** (allocs/req and bytes/req; spread 0.00):

| tree | arm | allocs/req | bytes/req |
| --- | --- | --- | --- |
| b70ce7c (pre-G15) | plain | 44.01 | 6593.3 |
| 23999a7 (G15, the attested baseline) | plain | 31.01 | 2554.3 |
| 989f488 (this tree) | plain | 30.01 | 2676.0 |
| 989f488 (this tree) | scheduling on | 32.01 | 2740.1 |

So since the baseline the framework gives back **one allocation per request** and spends
**121.8 B/req more**. The byte side is **unattributed**, and this record does not guess: the
candidates are the boxed handler future growing with the receive and handler deadlines and the peer
watch (tracks 84, 85, 117) and the per-response `X-Request-ID` push made unconditional by track 89;
nothing was profiled to separate them. The pre-G15 row was measured before the denominator fix, so
its figures are that run corrected arithmetically (46.94 and 7032.8 divided by 1.06667); it is not a
re-run.

**The gap the ladder does not model.** This tree's unscheduled serve path spends **30.01** allocs/req
where every rung together accounts for **25.01**: **+5.00 allocs/req and +1066.3 B/req** outside the
ladder - the per-request spawn and its guard, the middleware chain, the response head assembly and
post-process, and the request-id field. It is a named, reproducible gap, not a defect: the ladder
prices stages, and the runtime's own per-request plumbing was never one of them. Splitting it needs
a bench-only variant per shape or a host that can resolve CPU, and neither exists here today.

**Decision: no framework change.** The measurement names no stage as a bottleneck. Allocations fell
by one per request against the baseline, no rung's price moved, and the CPU column cannot resolve
anything on this box: today's rung deltas are +0.00, +2.44, +7.32, -1.83 and -0.61 us/req against
per-rung spreads of 3.05-14.04, and the two production deltas are +5.49 against a 10.38 spread and
+12.21 against 23.80. Under the directive an optimization with no measured cost behind it is not
made. Track 122's four arms remain the owner's to measure; this harness is not a substitute for that
protocol.

**Refused alternatives.**
- Report 33.07 against the recorded 31.00 as a regression - refused: the gap was the harness, and
  the same binary reproduces 31.01 under the corrected denominator.
- Correct only the allocation denominator and leave `us_cpu_per_req` on the client's count - refused:
  both quantities cover the same counted work, and a reading 6.67 % high in one column and exact in
  the other invites the wrong comparison.
- Re-measure track 122's arms here and call it the owner's A/B - refused: `pgo_ab.ps1` carries that
  protocol (arms, rotated order, null arm), and a published before/after is the owner's to run.
- Add ladder rungs for the unmodelled five - refused: an instrument that splits a gap nobody is
  acting on is work spent on a number.
- Reduce the +121.8 B/req now - refused (directive, priority 4): no profile names the site, the
  allocation count went down, and trading behaviour for an unattributed byte count is exactly the
  assumption-led optimization the directive forbids.
- Rewrite the register's 31.00 into today's convention - refused: it is an attested dated
  measurement. It gains a dated note naming the convention it was taken under instead.
- Leave the probe's comments citing `app.rs` line numbers - refused: they had already rotted. They
  name symbols now, and the rung's self-check is stated where the rung is defined.

**What this track does not claim.** No benchmark, no throughput and no latency number; rps is
quoted nowhere as a result, because the arms were measured minutes apart and the box drifts. No CPU
claim: the CPU column is recorded as unusable here since 2026-10-20, and today's spreads are wider
than today's deltas. Allocations are a capacity metric, not a latency one. Everything was measured
on this Windows laptop with the client on the same box; nothing here is a Linux or macOS reading.
The harness changes live in the unversioned `aether-bench-compare` crate, so the `denom` line and the
warmup constant are quoted above in full. The 121.8 B/req increase is stated as unattributed rather
than explained.

## Track 125 - track 123's two answers on the multiplexed frontend (2026-11-05)

**What was open.** Track 123's record names the gap itself: its four wire pins were HTTP/1.1 only, so "the h2 path is not claimed by these pins". Both frontends run the same `ServiceCore` response exit - `app.rs` builds the `cors` and `compression` middleware into it and `http2.rs::serve_streamed` calls `core.serve(...)` - so the corrections should travel the multiplexed path unchanged. Nothing checked that they do: a frontend-specific exit added later could drop the `Vary` of a denial or serve an identity body to a client that refused identity while every existing pin stayed green.

**Change commits.** Commit f896d30 carries the first four HTTP/2 wire pins, the `raw` payload on `H2Response` and the re-derived live documents. Commit 56d9ef4 carries the independent review's fixes and the recount they forced. This record is the commit that follows both.

**The pins.** Five tests in `tests/http2_e2e.rs` (80 -> 85), each over TLS with ALPN `h2`, sharing a `cors_app` helper that builds the same allow-list app the HTTP/1.1 pins use:
- `a_denied_origin_varies_on_origin_over_h2` - an origin outside the allow-list gets the simple response with no `Access-Control-Allow-Origin` and a `Vary` naming `Origin`; the allow-listed origin is still reflected on the same connection, so the pin cannot pass by refusing every request.
- `a_refused_preflight_varies_on_the_fields_that_decided_it_over_h2` - `OPTIONS /echo` requesting routable `POST` from an allowed origin, against an allow-methods list of `GET`, answers `403` with no CORS allow headers and a `Vary` naming `Origin`, `Access-Control-Request-Method` and `Access-Control-Request-Headers`.
- `an_approved_preflight_for_an_unroutable_method_is_answered_without_cors_headers_over_h2` - `PATCH` is allowed by the config but no route serves PATCH, so the rebuilt `403` carries none of the four CORS fields and still carries the middleware's `Vary`.
- `an_approved_preflight_is_answered_with_its_allow_fields_over_h2` - a routable allowed preflight (`POST /echo`, `authorization` requested) answers `204` with an empty body, the reflected origin, the configured allow-methods and allow-headers, and the three `Vary` tokens; a denied origin on the same connection still gets `403`. Without this test a server that refused every preflight would satisfy every refusal pin.
- `a_refused_identity_is_answered_over_h2` - `identity;q=0` on a dynamic 8,000-byte body answers `406` with an empty raw payload, no `Content-Encoding`, and no non-zero `Content-Length`; `gzip, identity;q=0` answers `200` with `Content-Encoding: gzip` and a payload that starts with the member magic, is longer than header plus trailer, and ends with a trailer whose ISIZE is 8,000. `H2Response` gained `raw: Vec<u8>` because the existing `body` field is a lossy UTF-8 string, and a gzip assertion on it would have been a shape assertion that passes on a garbled body.

**Independent review.** One limited, read-only review returned SHOULD-FIX with no blocker. Four findings were applied in 56d9ef4: the refused-preflight pin requested `DELETE`, which is both disallowed and unroutable, so deleting the middleware's method check left it green - it now requests routable `POST` and mutant M5 below proves the isolation; the slice had no approval pin at all, which the fourth test adds; the gzip pin checked only the two-byte magic, and now checks the trailer's decoded length and the content-length agreement; and the bodyless `406` now rejects a stale non-zero length. Two findings were accepted as limits and are restated below (a blanket `Vary` satisfies the denial pin; `header()` returns the first line only). The review confirmed what the pins rest on: `H2Client::connect` asserts ALPN `h2`, `http2.rs` dispatches through `core.serve`, and the unroutable test's 403 does come from the rebuilt response rather than from the middleware's own refusal.

**Sensitivity by injection.** Six mutants, each run against its targeted test, each restored from a byte-identical backup and checked by `sha256sum -c` against the baseline recorded before the first mutant (`cors.rs` 53caf025..., `serve.rs` 67e30f2a..., `compression.rs` da2d12c6...):
- M1: remove `add_vary_origin` from `CorsMiddleware::apply_for_origin`'s denial branch -> `a_denied_origin_varies_on_origin_over_h2` failed: "a refusal must vary on Origin, got None".
- M2: remove the preflight `add_vary(...)` call -> the refused-preflight pin failed: "origin missing from None on an h2 refusal".
- M3: delete the `Vary` copy loop in `serve.rs`'s rebuilt `403` -> the unroutable-approval pin failed: "the rebuilt refusal is still origin-dependent".
- M4: make `accepts_identity` return `true` unconditionally -> the identity pin failed: status `200` where `406` was required.
- M5: delete the `method_allowed` check in `apply_preflight` -> the amended refused-preflight pin failed with `204` against its expected `403`, which is the isolation the review asked for.
- M6: delete the `Access-Control-Allow-Methods` write -> the approval pin failed: `None` against `Some("GET, POST")`.

**Measured values.** This run: 916 passed / 0 failed / 1 ignored over 63 `test result:` lines, with the `running N tests` lines summing to 917; unit 478 over 19 unit suites, integration 419 over 30 targets (`http_e2e` 97, `http2_e2e` 85), doc 19 over 14. The ladder passes its eight gates; `validate` exits 1 with exactly its four pinned findings. The count sweep ran twice - once for the first four pins (24 findings, then 3 after the patch, green once the pins were made to name one sentence each) and once for the fifth, with the same documents red until the recount. The three findings the gate raised against the first patch were real defects in it, not noise: the matrix parenthetical no longer matched its own pin shape, and the superseded twenty-sixth PRR reading still matched the live-reading pins.

**Refused alternatives.**
- Re-argue the RFCs in the h2 tests - refused: track 123 already states the argument; these pins exist to prove the same response exit answers the multiplexed request, and their comment says so instead of repeating the prose.
- Assert gzip on the lossy `body` string - refused: a coded body is not UTF-8, and a check on replacement characters would stay green on a corrupted member.
- Keep the refused-preflight pin on `DELETE` - refused after the review: a method that is both disallowed and unroutable cannot tell the middleware's check from the rebuild's, and M5 showed the pin stayed green without the check.
- Leave the slice with refusal pins only - refused after the review: every refusal pin is satisfied by a server that refuses everything.
- Change the middleware or `serve.rs` - refused: no behaviour was found missing; the injections exist to prove the pins can fail, not because the tree failed them.
- Add the pins to `http_e2e.rs` under an h2 comment - refused: that file's client is HTTP/1.1, and an h2 claim needs the h2 client and ALPN.

**What this track does not claim.** No performance number was produced; nothing was timed. The pins cover the track-123 answers, not every CORS or compression shape over h2: wildcard reflection, credentials, expose-headers, precompressed variants and `Content-Range` remain HTTP/1.1-pinned. No shared cache was exercised, the same limit track 123 states. No cleartext-`h2c` run of these pins was added. A blanket `Vary: Origin` on every response would satisfy the denial pin, and a corrupted deflate stream inside a well-framed member would satisfy the trailer check; neither was measured, and the isolation that would separate them was not built. The tests ran on this Windows host only. The two evidence-tool files stay unstaged: their diff is line-ending normalisation with no content change.
## Track 126 - the plain `cookie()` accessor read one `Cookie` line and answered "absent" for a jar a client split across fields (2026-11-05)

**What was open.** Found by reading the accessors beside each other, not by a failing gate. `RequestContext::signed_cookie` (`aether/src/cookie.rs`) loops `self.header_values("cookie")` and the CSRF check in `serve.rs` reads `header_multi.get("cookie")`, so both treat repeated `Cookie` fields as one jar; `RequestContext::cookie` in `app.rs` called `self.header("cookie")`, which returns the first line only. RFC 9113 §8.2.3 lets an HTTP/2 client split cookies across repeated fields and RFC 6265 §5.4 wraps the whole jar in one field for that reason, so the second shape is not exotic: a name the client sent on the second line was reported as never sent. The inconsistency is the defect: an application that read its own cookie through `cookie()` and checked a signature through `signed_cookie()` could see "absent" from one and a verified value from the other on the same request. It was red on both frontends.

**Evidence, red first.** Two wire tests were written against the unfixed tree and failed there: `cookie_lookup_reads_every_cookie_field_line` (`tests/http_e2e.rs`) sends `Cookie: a=1; dupe=first` then `Cookie: theme=dark; dupe=second` and reads five answers, and `a_cookie_split_across_fields_is_read_by_the_plain_accessor_over_h2` (`tests/http2_e2e.rs`) sends the same jar as two repeated field entries through `H2Client::connect_plain`. With the fix reverted, the first fails at `:475` with `left: Null, right: "dark"` and the second with `left: "second=<absent>|..."`.

**The change.** `cookie()` now loops `self.lines_for("cookie")?` - the existing borrowed accessor that yields every line without allocating - and applies the same `split(';')` / `split_once('=')` / `trim` rule per line. The first occurrence still wins when a name is carried more than once, across lines exactly as within one; `signed_cookie` remains the accessor that refuses that ambiguity with `Ambiguous` instead of choosing. No allocation was added to the request path, and no other accessor changed.

**Sensitivity by injection.** Three mutants over `app.rs`, each run against both new tests, each restored from a byte-identical backup and verified by `sha256sum -c` against the baseline recorded before the first mutant (`app.rs` 6c28b67c585c917bc0958dd58baf9870c95d5cf083f96df69d742783e7d3347a, `http_e2e.rs` 119aece2cdd1e26f2909e97bff1da5c302b91e3dd2ceb18df842b45fa742dde3, `http2_e2e.rs` 38e3da7fca092209495bee9a99ef346b4ecbbb07207ba917c60751ed32343f3f):
- M1, back to the first line only (`self.header("cookie")?`): both pins failed - the h1 one at `:475` with `Null` against `"dark"`, the h2 one with `second=<absent>`.
- M2, iterate the lines in reverse: both pins failed on the duplicate-name assertion (`second` against `first`), which is the first-occurrence rule they claim.
- M3, compare names case-insensitively (`.eq_ignore_ascii_case(name)`): the h1 pin failed on its `THEME` probe (`"dark"` against `Null`); the h2 pin stayed green and is not claimed to cover case sensitivity - the h1 pin is the one that names RFC 6265 §5.2.

**Review.** No independent review was commissioned for this track: the change is nine lines in one accessor, is proven by two pins that fail without it, and the three mutants above were run by me. The record names that as a limit rather than implying a review ran.

**Citations and counts.** The `app.rs` fix moves every coordinate at or after old line 736 by +9 and the `http_e2e.rs` insertion moves every coordinate at or after old 443 by +50. Seven documents' coordinates were shifted mechanically, and `cite_check.py`'s own pin table was remapped the same way - from the files' actual diffs (`difflib` against `HEAD`), not by adding a constant - after the first pass left 52 "this span is not pinned" findings. The suite reading is re-derived in all six pinned documents (918 passed / 0 failed / 1 ignored over 63 `test result:` lines, `running N tests` summing to 919; unit 478 over 19 runs, integration 421 over 30 targets, doc 19 over 14), and `PRODUCTION_READINESS_REPORT.md` carries a twenty-eighth dated reading beside the twenty-seventh, which was demoted to the "in that lane" / "the one ignored test" wording so each count pin still names exactly one sentence. The API reference's `cookie` entry now states the multi-line read; that added one bare name to the name pass (1384 -> 1385), and the two pinned copies of that figure were re-derived from the same run.

**Refused alternatives.**
- Return the last occurrence instead of the first - refused: RFC 6265 §5.2 and every browser jar resolve a duplicate by the first stored value, `signed_cookie` already refuses rather than choosing, and a handler that wants the refusal has that accessor.
- Make `cookie()` return `Ambiguous` too - refused: it returns `Option<&str>`, and changing that shape is an API break for a case the signed path already covers; the doc comment names `signed_cookie` as the accessor that refuses.
- Decode percent-escapes while reading - refused: nothing in the current contract decodes, and a signed value's alphabet needs none; adding it would change every existing caller's answer.
- Join the lines into one string and reuse the existing loop - refused: that allocates on every cookie read, and `lines_for` already yields the lines borrowed.
- Add the test only to `http_e2e.rs` - refused: the split-jar shape is the one an h2 client produces (`RFC 9113 §8.2.3` says so), and the h2 frontend is where the reported-as-absent answer was most likely to appear.
- Cite the RFCs a second time in the test comments - refused: track 114 and 123 already state them; the comment says which property is pinned instead of re-arguing it.

**What this track does not claim.** No performance number was produced; nothing was timed. Case sensitivity over h2 is stated by the accessor's contract and pinned only on HTTP/1.1. The change does not make `cookie()` validate anything: an unsigned cookie is still returned as sent, and a duplicate name is still resolved to its first occurrence rather than refused. The tests ran on this Windows host only, in the same session as the mutant runs. The two evidence-tool files stay unstaged: their diff is line-ending normalisation with no content change.

## Track 127 - the decoded query lookup applied the form rule to the value and not to the key, so a browser-encoded parameter name was "absent" (2026-11-05)

**What was open.** Found by reading the accessors beside each other, not by a failing gate. `RequestContext::query_decoded` decoded the value with `form_decode` (`+` is a space, `%D9%85` is `م`) and compared the key after `percent_decode_cow` alone, which leaves `+` a literal. Three siblings reading the same query string do it the other way: `queries_decoded`, `form_data`, and the `Query` extractor all call `form_decode` on the key as well. A browser submits a GET form as `application/x-www-form-urlencoded`, so a field named `first name` arrives as `first+name=...`: `query_decoded("first name")` answered `None` for a parameter `queries_decoded()` returned from the same bytes. The accessor's own doc comment already promised the form rule; only the key side of the comparison ignored it.

**Evidence, red first.** `decoded_query_lookup_decodes_the_key_by_the_form_rule` (`tests/http_e2e.rs`) was written against the unfixed tree and failed there: `panicked at tests	ests\http_e2e.rs:526:5: assertion failed: an encoded space in the key is the space the caller asked for` with `left: Null, right: "plus"`. It reads six answers from one request (`?first+name=plus&second%20name=escaped&a%2Bb=literal&x+y%2Bz=both`): the `+` key, the `%20` key, a literal `%2B`, a key carrying both encodings, the map from `queries_decoded`, and the raw accessor.

**The change.** `aether_http::url` gained `form_decode_cow`, the borrowing form of `form_decode` (and `form_decode` is now `form_decode_cow(input).into_owned()`), and `query_decoded` compares `form_decode_cow(k) == key`. The Cow form is what keeps the miss path allocation-free: a raw key usually holds neither `+` nor a `%`, and the existing `percent_decode_cow` already returns that borrowed. The substitution still happens before percent-decoding, so `%2B` stays a literal plus; the new unit `test_form_decode_cow_matches_form_decode_and_borrows_when_plain` pins both the borrowing and that order (`a+b%2Bc` is `a b+c`).

**Sensitivity by injection.** Three mutants, each run against both new tests, each restored from a byte-identical backup verified by `sha256sum -c` against the baseline recorded before the first mutant (`app.rs` 2772f48c397861d1494d03ad55477621272d1fbeecd79f4c33503d550ff7abe5, `url.rs` 46fda40b3323ce765cef621ba2143b9600753f5c101ee32cca4f1497675c0634):
- M1, no key decoding (`k == key`): the wire pin failed at `:527` with `Null` against `"plus"`.
- M2, drop the `+` branch from `form_decode_cow`: both pins failed - the unit one on `matches!(form_decode_cow("a+b"), Cow::Owned(_))`, the wire one at `:527` with `Null` against `"plus"`.
- M3, percent-decode before substituting `+`: both pins failed - the unit one on `a+b%2Bc` with `left: "a b c", right: "a b+c"`, the wire one at `:536` on the both-encodings key with `Null` against `"both"`. M3 is why that probe exists: a test that only sent `first+name` would have stayed green under a decoder that turns a literal `%2B` into a space.

**Review.** No independent review was commissioned for this track: the change is one comparison plus a ten-line borrowing helper, it is proven by a wire pin and a unit pin that fail without it, and the three mutants were run by me. The record names that as a limit rather than implying a review ran.

**Citations and counts.** The `app.rs` doc comment adds one line, `url.rs` gains twenty net, the `http_e2e.rs` test adds fifty-six. `cite_check.py`'s pin table was remapped from the files' actual diffs (`difflib` against `HEAD`, refusing any span that overlaps a changed hunk), not by adding a constant: 36 keys were rewritten by that mapping, 23 of which moved. The documents' own coordinates were re-derived the same way - 26 references that repeat the file name plus five shorthand `:NNNN` continuations that do not. The suite reading is re-derived in all six pinned documents (920 passed / 0 failed / 1 ignored over 63 `test result:` lines, `running N tests` summing to 921; unit 479 over 19 runs, integration 422 over 30 targets, doc 19 over 14; `http_e2e` 98 -> 99, `aether-http` lib 110 -> 111), and `PRODUCTION_READINESS_REPORT.md` carries a twenty-ninth dated reading beside the twenty-eighth, which was demoted to the "in that lane" / "the one ignored test" wording so each count pin still names exactly one sentence. The new reading's code spans added three bare names to the name pass (1385 -> 1388); the two pinned copies of that figure were re-derived from the same run.

**Refused alternatives.**
- Decode the value with percent-decoding only, matching the old key side - refused: browsers encode a space as `+` in GET form data, the accessor's own comment promises the form rule, and `queries_decoded`, `form_data` and the `Query` extractor already implement it; this change aligns the odd accessor with the documented contract rather than rewriting the contract and three callers to match a bug.
- Compare with `form_decode(k) == key` - refused: it allocates a `String` per pair on the miss path even when the key holds neither `+` nor `%`, which is the common case.
- Add a private helper inside `aether` - refused: `aether` cannot reach `aether_http`'s private items, and reimplementing the decode in `app.rs` would put a second copy of the rule in the crate that does not own it.
- Split the raw key on `+` and compare without percent-decoding - refused: `?a%2Bb=...` would then never match the literal key `a+b`, and a percent-escaped name is as common in browser submissions as `+`.
- Make `query_decoded` last-value-wins like the `queries_decoded` map - refused: the two singular accessors are first-match and consistent with each other, and a duplicate-key rule change is a separate decision no measurement asked for; the map form remains the last-wins one.
- Add an h2 pin for the same lookup - refused: the query string is produced by the shared request translation and there is no per-frontend decoder, so a second pin would restate one rule on a second transport with no divergence to catch.

**What this track does not claim.** No performance number was produced; nothing was timed. Keys remain case-sensitive, and nothing decodes a key twice. This is a behaviour change for the shape that relied on the old asymmetry: a non-browser client that meant a literal `+` in a key must now send `%2B` (the raw `query()` accessor still matches the raw string). Invalid UTF-8 in a key still becomes U+FFFD rather than an error. The tests ran on this Windows host only, in the same session as the mutant runs. The two evidence-tool files stay unstaged: their diff is line-ending normalisation with no content change.

## Track 128 - the static root already refused a reparse point that leaves it; the boundary had no wire pin, so track 128 adds one (2026-11-05)

**What was suspected.** Found by reading the resolver rather than by a failing gate. `static_dir.rs` is the only route that turns a URL segment into a filesystem path, and `safe_resolve_path` refuses `..`, `%2e%2e`, `%2f`, `%5c`, absolute paths, NUL, and Windows ADS/reserved names lexically, before the filesystem is touched. The one shape that survives every lexical rule is a link: a directory junction (or symlink) whose name is spelled under the root and whose target is outside it. That is exactly the shape the precompressed-sibling and `index.html` paths document as a hazard, so the question was whether the direct static route checked it too.

**Evidence, red first - and the hypothesis died.** `static_dir_does_not_follow_a_link_out_of_its_root` (`tests/http_e2e.rs`) was written to fail on the unfixed tree: it creates a directory link `public/escape` -> `base/outside`, a link `public/inner` -> `public/real`, and asks the wire for `/assets/escape/secret.txt`, `/assets/escape`, and `/assets/inner/ok.txt`. The first run passed: the escape was answered `404` with an empty body before any change was made. Reading past the lexical block at `static_files.rs:126-145` explains it - `safe_resolve_path` joins the target, canonicalises the root and the target, refuses a target that does not `starts_with` the canonical root, and returns the *canonical* path so the caller opens exactly what was checked. No defect existed, so no fix was applied; the test stays as the pin that boundary never had.

**What the pin covers.** Three answers from one server: a link out of the root is `404` with an empty body (the refusal must not carry the file it refused), the link's own directory is `404` (resolving the directory itself leaves the root too), and a link that stays inside the root is `200` with its bytes (a link is a name, not by itself a refusal). The helper is `cmd /C mklink /J` on Windows - a junction is a reparse point an unprivileged account can create, while a file symlink needs developer mode or elevation, which this host does not have (`WinError 1314` was observed) - and `std::os::unix::fs::symlink` on unix, compiled here but not executed.

**Sensitivity by injection.** Two mutants of the containment check, each run against the new pin and each restored from a byte-identical backup verified by `sha256sum -c` against the baseline recorded before the first mutant (`framework/http/src/static_files.rs` 058e9f69a1ebdfe068ffdb84ff31eb3b4631a88a52a4f8184bde61c56371067e; `framework/tests/tests/http_e2e.rs` 907bbff175b39b8aeac7344fb80f010ec8103c5eb204e3ea94cf77e8fa27ea65):
- M1, drop the containment rejection (serve the lexical join when both canonicalise): the pin failed with `left: 200, right: 404` - the outside file was served.
- M2, compare the lexical join instead of the canonical target (`full_path.starts_with(root_dir)`): the pin failed the same way, because the lexical name is under the root even when its target is not. M2 is why the pin tests a link at all: a test that only sent `..` would stay green under a resolver that trusts the spelling.

**Review.** No independent review was commissioned for this track: no production line changed, the only new code is one test and two platform helpers, and the central claim - that the boundary already held - is a run rather than an argument. The record names that as a limit rather than implying a review ran.

**Citations and counts.** The test and its helpers add 87 lines to `tests/http_e2e.rs`; no crate source changed. `cite_check.py`'s two `http_e2e.rs` pins below the insertion moved by +87 from the file's actual diff (4972 -> 5059, 5008 -> 5095: the `a_handler_error_answers_400_and_says_so_in_the_body` and `typed_handler_errors_answer_their_own_class` spans), and `ERROR_HANDLING_GUIDE_AR.md:122`'s two shorthand coordinates moved with them. The suite reading is re-derived in all six pinned documents (921 passed / 0 failed / 1 ignored over 63 `test result:` lines, `running N tests` summing to 922; unit 479 over 19 runs unchanged, integration 422 -> 423 over 30 targets, doc 19 over 14 unchanged; `http_e2e` 99 -> 100, `aether-http` lib unchanged at 111), and `PRODUCTION_READINESS_REPORT.md` carries a thirtieth dated reading beside the twenty-ninth, which was demoted to the "in that lane" / "the one ignored test" wording so each count pin still names exactly one sentence. The new reading's code span `safe_resolve_path` added one bare name to the name pass (1388 -> 1389); the two pinned copies of that figure were re-derived from the same run.

**Refused alternatives.**
- Add a second canonical-containment check in `serve_from_dir` - refused: `safe_resolve_path` is the one boundary every static entry passes through (direct file, `index.html`, precompressed sibling), and repeating the comparison at a caller would be the same check one layer up with a second copy of the rule to keep in step.
- Move the check into `aether`'s `serve_from_dir` - refused: path resolution lives in `aether_http::static_files`, which owns the lexical rules as well; moving half of it into the routing crate would split one rule across two crates for no new coverage.
- Refuse every reparse point, inside the root or not - refused: a link that stays inside the root is a legitimate deployment shape (one shared asset directory linked under a prefix), and the `inner` arm asserts it is still served; containment is the property, not the absence of links.
- Add a Windows file-symlink arm - refused: creating one needs developer mode or elevation, so the test would be red on the default account; the directory junction already exercises the same canonicalisation and containment code.
- Claim the same result on Linux/macOS - refused: the unix arm compiles but was not executed here, and PROB-0012 (no Linux/macOS run) is still open; the record claims this host only.
- Touch the `static_dir.rs:307` comment that calls the sibling re-check "not redundant" - refused: the claim was verified true (the sibling is a different inode with its own canonicalisation through `contained_child`), so there was nothing to correct.

**What this track does not claim.** No TOCTOU test: the resolver returns the canonical path so the open uses the checked name, but the pin does not race a link re-targeted between canonicalisation and open. No test for a writer inside the root replacing the canonical target after the check - that path is still inside the root and is not a boundary escape. No Windows file-symlink arm and no unix execution. No performance number was produced; nothing was timed. The tests and mutant runs were on this Windows host only, in the same session. The two evidence-tool files stay unstaged: their diff is line-ending normalisation with no content change.

## Track 129 - track 117 left the delivery of its 408 over TLS unverified; track 129 pins it (2026-11-05)

**What was open.** Found by reading the register's own open list, not by a failing gate. Track 117's section ends with four residuals, and the first is exact: "Delivery of the `408` over TLS is unverified: the new tests run on cleartext sockets, and no TLS wire test asserts this timeout answer." A grep of `framework/tests` confirms it today: every `408` assertion lives in `http_e2e.rs` (four tests over cleartext sockets), `streaming_request.rs` (a streamed lane) or `http2_e2e.rs` (the multiplexed frontend), and `tls_e2e.rs` had eleven tests, none of them a receive-budget test.

**Why the tree was expected to hold.** The answer is not written per frontend. `answer_receive_timeout` (`aether/src/app.rs:4232`) writes through `crate::transport::Stream`, an enum whose `Tls` arm wraps `tokio_rustls::server::TlsStream` (`aether/src/transport.rs:27-30`), and `establish` hands a TLS connection to the same HTTP/1.1 loop a plaintext one enters. So the shared clock, the same `408` and the same `Connection: close` reach both. What was missing was evidence, and the TLS-specific half is real: the answer has to leave as TLS application data, and the end has to be a `close_notify` rather than a bare FIN.

**The pin.** `a_partial_request_over_tls_is_answered_408_and_closed` (`framework/tests/tests/tls_e2e.rs:524`), one server with `max_receive_time` at 600 ms against the default 5 s `read_timeout`, two shapes - a complete head whose body never starts, and three of five body bytes followed by silence. Each must be answered `408` with `Connection: close`, no earlier than 400 ms (an answer at the first pause fails that bound), and then deliver a clean end: a read that returns 0 rather than a rustls unexpected-EOF error (M4 fails exactly there). A third connection then serves `/ping`, so the refusal is shown to belong to the connection and not to the listener.

**Sensitivity by injection.** Four mutants over `aether/src/app.rs`, each run against the new pin and each restored from a byte-identical backup verified by `sha256sum` (`2772f48c397861d1494d03ad55477621272d1fbeecd79f4c33503d550ff7abe5`, the same baseline track 127 recorded):
- M1, drop the answer on the read-timeout arm (`app.rs:3046`): the pin died inside `read_response` on the TLS read, which is the silent-close signature.
- M2, answer `400` instead of `408`: the status assertion failed at `:544`.
- M3, omit `Connection: close`: the header assertion failed at `:549`.
- M4, omit `stream.shutdown()` so the socket ends with a bare FIN: the close-notify assertion failed at `:562`. This is the one mutant only the TLS frontend can catch, and it is the reason the pin reads the end explicitly instead of trusting the helper.

**Review.** No independent review was commissioned for this track: no production line changed, the only new code is one test, and the four mutants were run by me in the same session. The record names that as a limit rather than implying a review ran.

**Citations and counts.** `tests/tls_e2e.rs` gains 77 lines and no crate source changed. The suite is **922 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit 479 over 19 unit runs, integration **424** across 30 targets, doc 19 over 14 doc runs - with the 63 `running N tests` lines summing to **923** = 922 + the one ignored test. Only one target moved: `--test tls_e2e` 11 -> 12, and no lane entered or left the census. The six counted documents were re-derived from this run, and `PRODUCTION_READINESS_REPORT.md` carries a thirty-first dated reading beside the thirtieth, which was demoted to the "in that lane" / "the one ignored test" wording so each count pin still names exactly one sentence. `cite_check.py` is unchanged at 112 citations in 16 live documents over 77 spans, 0 findings, and its name pass resolves the same 383 qualified and 1389 bare names; `evidence_check.py` scans 35 documents with 0 findings.

**Refused alternatives.**
- Put the pin in `http2_e2e.rs` - refused: the gap track 117 named is the buffered HTTP/1.1 loop, the h2 lanes already carry their own 408 tests, and a TLS test on the h2 client would answer a different question.
- Assert the status alone - refused: `Connection: close` and the clean end are separate promises, and M3 and M4 each break one while the status stays `408`.
- Treat an empty read from the existing helper as "closed" - refused: over plaintext any FIN reads as zero, but over TLS a bare FIN is an error, and that difference is the point; the explicit read names it.
- Drop the lower time bound - refused: without it a server that answers at the first pause passes, and 400 ms still leaves 200 ms of slack in the safe direction on a 600 ms budget.
- Reuse all four shapes of the cleartext test - refused: the head-never-finishes and chunked shapes add nothing the two chosen shapes do not exercise on this frontend, and the target's runtime is kept down without losing either arm of the receive loop.
- Change `answer_receive_timeout` or the transport - refused: nothing was found missing; this is a verification gap, and an edit on suspicion would be an unmeasured change to the path every request passes through.

**What this track does not claim.** No performance number was produced; nothing was timed. The TLS end is asserted by a rustls client on this Windows host only. Track 117's second residual - the RST race in the window between the answer and the FIN for a peer that resumes sending - is unchanged and applies here as well. PROB-0012 (no Linux/macOS run) still blocks a cross-platform claim, and the lower bound's 200 ms of slack would need re-examining on a slower host.

## Track 130 - two adjacent multipart delimiter lines panicked the handler (2026-11-05)

**What was open.** Found by a maintenance audit of the request-body parsers, not by a failing gate: the ranked audit's agent-actionable list is empty and the four ledger problems still open are owner-gated. `find_delimiter` reports a delimiter's `delimiter_start` as the index of the CRLF that precedes it (`multipart.rs:219`, `index.saturating_sub(2)`), and `parse_with_boundary` sliced a part as `&body[cursor..next.delimiter_start]` (`multipart.rs:112` before this track). Two delimiter lines with nothing between them make the next delimiter start exactly at `cursor`, so the range runs backwards and the slice panics.

**Reproduction.** `MultipartForm::parse("multipart/form-data; boundary=b", b"--b\r\n--b--")` panicked with `slice index starts at 5 but ends at 3` at `multipart.rs:112` before any edit. The body is 10 bytes, and the parser is public: `RequestContext::multipart()` (`app.rs:718`) calls it on the buffered body, which is already bounded at 2 MiB (`limits.rs:76`), so the trigger costs a client nothing. The panic happens inside the handler task, and `serve.rs:949` maps the task's `JoinError` to a 500, so the observable failure is a logged panic per request plus a 500 - not a process abort, and not a status the parser chose.

**The fix.** Clamp the part's end at the cursor (`multipart.rs:117`):

```rust
let part_end = next.delimiter_start.max(cursor);
parts.push(parse_part(&body[cursor..part_end])?);
```

An empty part then reaches `parse_part`, which already refuses a part with no header block (`MissingPartHeaders`, `multipart.rs:248`) - the same answer `--b\r\n\r\n--b--` gets. The clamp fires only when the next delimiter begins at the cursor: a candidate at `cursor + 1` would require `body[cursor - 1] == '\r'`, but that byte is the `'\n'` that ended the previous delimiter line. For every legal part `delimiter_start >= cursor`, so `.max(cursor)` is a no-op there, and `delimiter_start` has no other consumer.

**Tests.** `adjacent_delimiter_lines_are_refused_without_panicking` (`multipart.rs:744`) is the red-first unit pin over four bodies - two adjacent delimiter lines with and without a trailing CRLF, a blank line where the header block belongs, and the same shape after a well-formed part (so the second pass of the loop is covered too) - each refused with `MissingPartHeaders`. The wire pin rides inside `multipart_form_data_is_parsed_end_to_end` (`http_e2e.rs:750`): the same 10-byte body through a real socket to a handler that maps parser errors to 400 must answer 400 with the header-block refusal in the body; the mutant observation is why this is a 400 rather than a 500 - the panic would have been answered by the serving loop, not by the parser.

**Sensitivity by injection.** Two mutants over `multipart.rs`, each run against both pins and each restored from a byte-identical backup verified by `sha256sum` against the committed file (`b4f388971a60136a1276e614f87499ffd5e3bc397bcf9f0296472fb9fa690aca`, checked after the mutants and again before the commit):
- M1, restore the pre-fix slice `&body[cursor..next.delimiter_start]`: the unit pin dies with the original `slice index starts at 5 but ends at 3` panic, and the wire pin receives 500 instead of 400.
- M2, skip the empty part and carry on with the next delimiter instead of parsing it: the body is accepted, so the unit pin's `MissingPartHeaders` assertion fails and the wire pin loses its 400. This is the mutant that shows the pins hold the refusal and not merely the absence of a panic.

**Review.** A limited independent review was run read-only against the diff and the pins before the record was written; its verdict was PASS. It confirmed the clamp can only fire at `index == cursor` (a `cursor + 1` start is impossible because that byte is the previous terminator's `'\n'`), that `delimiter_start` has one consumer, that the panic is reachable only on buffered routes (`route_stream` hands an empty `body`, which returns `MalformedBody`), that the 500 claim about `serve.rs:949` is accurate, and that no empty part is silently accepted; it ran the targeted `aether-http` multipart tests (14 passed). Its one optional suggestion, a `debug_assert!(next.delimiter_start >= cursor)`, was not taken: the clamp is the enforcement, and an assertion in debug builds only would add a second statement of the same invariant, not a property the release build has.

**Citations and counts.** `multipart.rs` gains 37 lines (the clamp and its comment, plus the unit test) and `http_e2e.rs` gains 26. The suite is **923 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **480** over 19 unit runs, integration 424 across 30 targets, doc 19 over 14 doc runs - with the 63 `running N tests` lines summing to **924** = 923 + the one ignored test. Only `aether-http`'s lib lane moved, 111 -> 112; `http_e2e` stays at 100 because its pin extends an existing test. `PRODUCTION_READINESS_REPORT.md` carries a thirty-second dated reading beside the thirty-first, demoted to the "in that lane" / "the one ignored test" wording so each count pin still names exactly one sentence. `cite_check.py`'s two `http_e2e.rs` pins moved +26 from the file's actual diff (5059 -> 5085, 5095 -> 5121) and `ERROR_HANDLING_GUIDE_AR.md:122` moved with them. The citation pass is unchanged at 112 citations in 16 live documents over 77 spans, 0 findings; its name pass measured 383 qualified and 1390 bare names. That second figure is a pre-existing drift, not this track's: the pass counts only snake-case tokens inside document code spans, neither new identifier appears in one, and the two printed copies said 1389 while the incoming tree already measured 1390 - both were re-derived from this run.

**Refused alternatives.**
- Return `MissingPartHeaders` directly when the range would run backwards - refused: `parse_part` already owns the "a part with no header block is refused" rule, and a second copy of it at the call site would be the same decision in two places; the clamp routes the body to the existing rule.
- Make `find_delimiter` report `delimiter_start = index` instead of `index - 2` - refused: that CRLF belongs to the delimiter, and attributing it to the part would append a trailing CRLF to the payload of every legal part, corrupting binary parts to fix an edge case.
- Silently skip the empty part - refused: M2 shows the effect - a malformed body would be accepted as a form with fewer parts, which is a different request than the client sent; a form with no parts is legal only as the closing delimiter alone (`--b--`), which already returns an empty form.
- Add the `debug_assert` the review suggested - refused: see the review paragraph; the clamp is the enforcement and the assertion would only restate it.

**What this track does not claim.** No performance number was produced; nothing was timed. The wire pin ran on this Windows host over cleartext HTTP/1.1 only: the panic is in the parser, so it is frontend-independent, but the 500 observation was made on the HTTP/1.1 loop, and no h2 or TLS pin was added. No fuzz campaign was run over the new clamp; the four unit shapes and the two mutants are the coverage. The 1390-vs-1389 name drift was re-derived, not traced to the commit that produced it. PROB-0012 (no Linux/macOS run) still blocks a cross-platform claim. Nothing else in the multipart parser was changed.

## Track 131 - a compressed body kept the validator and the range promise of the bytes it replaced (2026-11-05)

**What was open.** Found by a maintenance audit of the response middleware, not by a failing gate. `CompressionMiddleware::apply` replaced `response.body` with the coded bytes and rewrote `Content-Length`, but left every other field that described the identity representation in place. `serve_static` is the framework's own strong-validator path - `serve_static_with_range` (`framework/http/src/static_files.rs:386`) answers with a content hash `ETag` and `Accept-Ranges: bytes` - and it is non-streaming, so this middleware does transform it. (`static_dir` and `App::static_file` stream through `BodyStream`, and a streamed body is untransformable here, so they were never affected.) A client could therefore store a gzip body under the identity `ETag`; a later `If-None-Match` would be answered `304` for the identity representation, and a later `Range` with `If-Range: <identity ETag>` would be answered from the stored identity bytes and spliced into the compressed cache entry. The same fields survived on the bodyless `406` track 123 added: a refusal named a representation the client had just refused.

**Reproduction.** Before any edit, the new wire pin received `Content-Encoding: gzip` beside `ETag: "1db0-5d946ca2553dfc15"` and `Accept-Ranges: bytes` on the coded body: the validator named bytes that were not on the wire. The new unit pin failed with `etag survived a body the response no longer carries` (`compression.rs:1168`), and the extended `406` pin failed with `accept-ranges survived on a 406` (`compression.rs:1115`).

**The fix.** One helper, `strip_stale_representation_metadata` (`compression.rs:220`), drops the fields that assert what the bytes were - `ETag`, `Content-MD5`, the legacy `Digest` of RFC 3230, and `Content-Digest`/`Repr-Digest` of RFC 9530 - and the promise that they can be range-served, `Accept-Ranges`. It is called from the transform arm (`compression.rs:140`) and from `refuse_not_acceptable` (`compression.rs:196`). `Last-Modified` stays: it dates the resource, not the coding, and a cache can still use it when no validator is present. Removal, not weakening, is the decision: a weak `ETag` would still satisfy `etag_matches`, which strips `W/` for the weak comparison, and a `304` would re-attach the handler's strong tag to the next answer; and the middleware cannot mint a replacement validator, because the handler is the only party that compares one and it never sees the coded bytes. Nginx's gzip filter clears the same field for the same reason; that comparison is background rationale, not evidence for this fix.

**Tests.** Three unit pins and one wire pin. `a_transformed_body_loses_the_metadata_that_described_the_original_bytes` (`compression.rs:1135`) asserts that the fixture really was transformed, that all six fields are gone, and that `Last-Modified` survives. Its control, `an_untransformed_body_keeps_its_validator` (`compression.rs:1184`), applies the same middleware to a sub-floor text body and to incompressible LCG noise and asserts the `ETag` and `Accept-Ranges` remain; the incompressibility precondition is asserted, not assumed. `test_the_refusal_strips_the_withheld_representation_metadata` (`compression.rs:1075`) was extended with `Accept-Ranges` and the four integrity fields. The wire pin, `a_compressed_static_answer_drops_the_validator_the_identity_answer_keeps` (`http_e2e.rs:5311`), serves a 7 600-byte `serve_static` asset: the gzip answer carries `Content-Encoding: gzip`, no `ETag`, no `Accept-Ranges`, the same `Vary` on `Accept-Encoding` and a `Content-Length` equal to the coded body, while the identity answer carries the content-hash `ETag`, `Accept-Ranges: bytes` and the original payload. The track also repaired a pre-existing vacuous test: `test_incompressible_body_is_left_alone` (`compression.rs:809`) generated its "noise" as `i * const >> 13`, which deflated 4 096 -> 931 bytes, so its "left alone" assertion never ran; a full-state LCG (4 107 coded bytes against 4 096 input) replaced it and the assertions are now unconditional.

**Sensitivity by injection.** Five mutants over `compression.rs`, each restored from bytes whose `sha256` was checked against the committed file (`193458a8df163ad1fe197821f06f875f92fa25f060112a8bef16bb889e6a875a`, verified after the last restore):
- M1, the transform arm keeps the metadata: both pins die - the unit pin on `etag` at line 1168, the wire pin on the present `ETag` at line 5331.
- M2, `accept-ranges` is not stripped: the unit transform pin (line 1168) and the `406` pin (line 1115) both fail, and the wire pin fails on `Accept-Ranges` at line 5337.
- M3, `Last-Modified` is stripped too: the unit transform pin fails at line 1175; the wire pin stays green because it does not assert `Last-Modified`, which is why that boundary is owned by the unit pin.
- M4, the strip runs on the untransformed path as well: the control pin fails at line 1215; the wire pin stays green because it only checks the identity path keeps the fields.
- M5, `refuse_not_acceptable` no longer calls the helper: the `406` pin fails at line 1115; the wire pin stays green.
All five are killed; two are killed on the wire.

**Review.** A limited independent review was run read-only against the diff and the pins before this record was written; its verdict was PASS. It confirmed that the defect is real and reachable, that removal (rather than a weak tag or a minted one) is the correct answer, that the early returns for `status_forbids_body`, handler-chosen `Content-Encoding`, streamed bodies and `206` leave no untransformed path exposed, and that the pins fail on the wrong variants rather than on the right ones. It raised two non-blocking items, both applied: `Digest` was attributed to RFC 9530 and is RFC 3230 (with `Content-Digest`/`Repr-Digest` added to the helper, the fixtures and the assertions), and the incompressible control was guarded by a conditional that would have skipped its assertion silently - making the precondition unconditional exposed the vacuous LCG fixture described above. What the reviewer could not verify: it did not build or run anything itself, so the sha256, the mutant outcomes and the gate results are this track's own evidence; and the nginx comparison was not checked against nginx.

**Citations and counts.** `compression.rs` grew by the helper and three unit pins; `http_e2e.rs` grew by one pin. The suite is **926 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **482** over 19 unit runs, integration **425** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **927** = 926 + the one ignored test. Unit grew 480 -> 482 in one crate (`aether-middleware` 91 -> 93, two new pins); integration grew 424 -> 425 in one target (`http_e2e` 100 -> 101). Six live documents were re-derived: the two READMEs, the completeness matrix (three printed copies plus the names-pass tree label), the baseline (header, total, the `aether-middleware` row and the per-target table), the readiness report (a new thirty-third dated reading, the thirty-second demoted to the "in that lane" / "the one ignored test" wording so each `counts` pin still names exactly one sentence, and the `http_e2e` evidence bullet) and `OWNER_VISION.md`. Two derived figures in that prose were re-derived as well, because no pin reads them: the matrix's count of tests added since the 2026-10-23 recount of 474 is now 452 (it still said 437, which was true when 474 + 437 = the track-123 total of 911) and the baseline's "track 131 is the reading this table carries" (it still named track 126). The `counts` gate reports 0 findings over six documents. No citation pin moved: the `cite` gate is unchanged at 112 citations in 16 live documents over 77 spans, 0 findings, and the name pass is unchanged at 383 qualified and 1 390 bare names.

**Refused alternatives.**
- Weaken the `ETag` to `W/` instead of removing it - refused: `etag_matches` strips the weakness for comparison, so the handler's strong tag still matches, and a weak validator cannot serve an `If-Range` (RFC 9110 §8.8.3); the client would keep a validator that names bytes it does not hold.
- Mint a validator for the coded bytes - refused: the handler is the only party that compares one and it never sees the coded bytes; a validator nobody can check is worse than an absent one.
- Keep `Accept-Ranges` and serve ranges from identity - refused: a byte range addresses the selected representation, and identity offsets spliced into a compressed cache entry are exactly the splice this track removes.
- Strip `Last-Modified` as well - refused: it dates the resource, not the coding; M3 shows the unit pin holds that boundary.
- Strip on every answer, transformed or not - refused: an untransformed body is the stored representation and its validator is exact; M4 shows the control pin holds that boundary.
- Decompress the coded body to serve a range from it - refused: that is a different mechanism with its own cost, and the specification answer is to stop advertising ranges on a representation the server does not have stored.

**What this track does not claim.** No performance number was produced; nothing was timed. The wire pin is cleartext HTTP/1.1 only - no h2 or TLS wire pin was added; the middleware runs in `post_process` for both frontends, but that is reasoning from the call site, not a wire observation. The coded bytes are not decompressed in the pin: it asserts framing, field presence and the declared length, not the content after inflate. `304` and `206` keep their strong validator by design, because `status_forbids_body` and the `206` guard return before the transform; this track did not change that, and no test in it exercises a conditional or range request against a coded response. The nginx comparison is background, not evidence. PROB-0012 (no Linux/macOS run) still blocks a cross-platform claim. Nothing else in the compression middleware was changed.

## Track 132 - an empty JWT signing secret verified every token minted under it (2026-11-05)

**What was open.** Found by a maintenance audit of the security surface, not by a failing gate. `JwtValidator::new` (`framework/security/src/jwt.rs:69`) handed whatever bytes it was given to `ring`'s `hmac::Key::new`, and `ring` accepts a zero-length HMAC key: it zero-pads a key shorter than the block length (RFC 2104) instead of refusing it. A deployment built as `BearerAuth::new(b"", ...)` - the shape a missing environment variable takes when the program does not check it - therefore verified every token signed with that same empty key, and anyone can compute such a token. The crate's two other key-holders already refuse the value: `CookieSigner::new` (`framework/security/src/cookie.rs:123`) and `CsrfTokens::new` (`framework/security/src/csrf.rs:181`). The validator was the outlier, and the most security-critical of the three.

**Reproduction.** Before any edit, a scratch probe built `JwtValidator::new(b"")`, signed a token with a MAC computed under the same empty key, and asked the validator to verify it; it read `sub="admin"` back out of a token the server never issued. The red-first unit pin `an_empty_secret_panics` failed with "test did not panic as expected". The probe was scratch and is not checked in; the pin is the checked-in evidence.

**The fix.** `JwtValidator::new` now asserts `!secret.is_empty()` with the message "the JWT signing secret must not be empty", before the key is built (`jwt.rs:69-75`). `BearerAuth::new` has no key handling of its own - it wraps `JwtValidator::new` (`auth.rs:99-104`) - so it inherits the refusal; its doc comment now says so. Panic at construction, not a new fallible signature: the crate's other two key-holders fail closed the same way, every caller in this tree constructs a validator at setup, and a misconfiguration is cheaper to discover at startup than at the first forged request. The refusal is emptiness, not length: a short secret is still a secret the issuer chose, and the type documents no minimum.

**Tests.** Three unit pins in `aether-security`'s lib target, all checked in:
- `jwt::tests::an_empty_secret_panics` (`jwt.rs:442`) - `#[should_panic(expected = "must not be empty")]`, so the message is held too, not just the abort.
- `jwt::tests::a_short_but_non_empty_secret_is_accepted` (`jwt.rs:450`) - the control: a one-byte secret verifies a token signed under it, which is what makes the pair a statement about emptiness rather than about a minimum length.
- `auth::tests::the_bearer_layer_refuses_an_empty_secret` (`auth.rs:357`) - the layer's constructor reaches the validator's refusal instead of building a validator anyone can sign for.

**Sensitivity by injection.** Three mutants over the two files, each restored from bytes whose sha256 matched the file the campaign started from (`fe0a9e5ae3a18c23cbd91a8403c97f6b8c73c3e3cb0e6bb28346a01a347910ac` for `jwt.rs`, `a5bc3b77345f1a722c10f998bc5f16a014893353e49eb94ad3279b42c87c6075` for `auth.rs`):
- M1, the pre-fix constructor (no assertion): 58 passed / 2 failed - both `jwt::tests::an_empty_secret_panics` and `auth::tests::the_bearer_layer_refuses_an_empty_secret` die.
- M2, a length rule (`secret.len() >= 32`) instead of emptiness: 59 passed / 1 failed - only the control dies, which is why the control exists.
- M3, the check moved into `BearerAuth::new` only: 59 passed / 1 failed - only `an_empty_secret_panics` dies, so each pin holds its own construction site.
The review's one applied edit was a comment-only deletion; the committed `jwt.rs` hashes `b92a12cb785899df6688fbda24995c58f370cb34ea63e5bcc62bf6492b0eb7ea`, and the mutant campaign ran against the byte-identical behaviour of the previous text.

**Review.** A limited independent review ran read-only against the diff, the pins and the test run; its verdict was PASS with two non-blocking notes. It confirmed against `ring` 0.17.14 that `hmac::Key::new` accepts a zero-length key, that the assertion runs before the key is built, that `new` is the only way to obtain a `JwtValidator` (the `key` field is private and there is no `Default`, `Deserialize` or second constructor, so `BearerAuth::with_validator` cannot be handed a validator that skipped the check), and that the three pins distinguish emptiness from length and the validator's check from the layer's. It re-ran `cargo test -p aether-security --lib` at 60 passed. Both notes were applied: the test comment's reference to the scratch probe was removed so nothing checked in cites evidence that is not checked in, and the missing governance record is what this section writes. The reviewer did not run the workspace suite, the citation gates or the runtime probe; those remain this track's own evidence, re-derived below.

**Citations and counts.** The suite is **929 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **485** over 19 unit runs, integration **425** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **930** = 929 + the one ignored test. Unit grew 482 -> 485 in one crate (`aether-security` 57 -> 60, the three new pins); integration held at 425 and no lane entered or left the census. Six live documents were re-derived: the two READMEs, the completeness matrix (three printed copies, the names-pass tree label and its qualified-name count), the baseline (header, total and the `aether-security` row), the readiness report (a new thirty-fourth dated reading, with the thirty-third demoted to "in that lane" and "the one ignored test" so each `counts` pin still names exactly one sentence) and `OWNER_VISION.md`. Three teaching documents gained a bullet naming the refusal (`docs/04_security_and_rfc9112.md:162`, `docs/09_api_reference.md:495`, `framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md:465`), and three citations of `jwt.rs:95` moved to `jwt.rs:106` with the matching `cite_check.py` pin; the citation gate is unchanged at 112 citations in 16 live documents over 77 spans, 0 findings, while the name pass moved 383 -> 393 qualified names because those bullets name `JwtValidator::new`, `BearerAuth::new`, `CookieSigner::new` and `CsrfTokens::new` ten times between them; bare names held at 1 390. Two prose defects outside the pins were corrected while re-deriving: the readiness report's track-128 reading announced a "thirty-first reading below" that does not exist (the next one is the thirty-second), and the `tls_e2e` evidence bullet still printed 11 tests after track 129's pin made it 12. The `counts` gate reports 0 findings over six documents.

**Refused alternatives.**
- A fallible constructor (`Result<Self, _>`) - refused: the crate's three key-holders all fail closed at construction, every caller in this tree builds one at setup, and the new signature would only move the decision to callers that would panic or refuse to serve anyway.
- A minimum-length rule (`>= 32` bytes) - refused: that is a different policy the type does not document, and M2 shows the control pin holds that boundary; an empty key is not a weak key, it is a public one.
- Refuse the empty key at verify time instead - refused: verification would pay on every request for a construction-time mistake, and a validator that exists but refuses everything is a worse failure than a startup panic that names the misconfiguration.
- Leave the refusal undocumented - refused: it is now a public contract, and the three teaching documents that list the crate's fail-closed guarantees must name it.

**What this track does not claim.** No performance number was produced; nothing was timed. No wire test was added: the refusal is a construction-time invariant and cannot be observed on the socket. The panic aborts the process by design; this is a startup misconfiguration, not a per-request condition. `BearerAuth::with_validator` still accepts a validator built elsewhere and this track did not change it - it cannot be handed an unchecked validator through the public API only because `new` is the sole constructor, which the review verified. The run is Windows-only; PROB-0012 still blocks a cross-platform claim. Nothing else in the JWT validator, its claim checks or the bearer layer was changed.

## Track 133 - the Json extractor refused a legal +JSON media type (2026-11-06)

**What was open.** Found by the maintenance audit of the extractor surface that followed track 132, not by a failing gate. `Json<T>`'s `FromRequest` (`framework/aether/src/extract.rs:544`) compared the exact type with `mime.eq_ignore_ascii_case("application/json")` and the structured-syntax suffix with a case-sensitive `ends_with("+json")`. RFC 9110 §8.3.1 makes a media type case-insensitive, and `+json` is part of the subtype token, so `application/vnd.api+JSON` names the same type as `application/vnd.api+json`. The extractor's own type doc promised that "a `+json` media type" is accepted; the case-sensitive suffix answered a legal request with `415`.

**Reproduction.** A red-first wire test in `framework/tests/tests/extractors.rs` posted a valid body to `/json` under five spellings. Before any edit, `json_media_type_is_case_insensitive_on_both_sides` failed at `application/vnd.api+JSON` with `415` where the run expected `200`; `application/json`, `APPLICATION/JSON` and `application/vnd.api+json` already passed, which isolated the defect to the suffix comparison rather than the equality.

**The fix.** The media type is still split at `;` and trimmed once. The suffix is read through `mime.len().checked_sub("+json".len()).and_then(|start| mime.get(start..))`, so a value shorter than the suffix or one whose fifth-from-last byte splits a multi-byte character yields `None` instead of panicking, and both comparisons use `eq_ignore_ascii_case` (`extract.rs:545-559`). No allocation is added: the check never lowercases the string. The `415` shape, message and the lenient behaviour for an absent `Content-Type` are unchanged. `Form<T>` was checked and needed nothing: it compares one literal with `eq_ignore_ascii_case` and has no structured suffix to match.

**Tests.** Two new integration pins in `tests/extractors.rs`, both over the real loopback HTTP/1.1 client:
- `json_media_type_is_case_insensitive_on_both_sides` (`extractors.rs:255`) - seven accepted spellings: both exact forms with either case, lowercase and mixed-case `+json` suffixes including `application/vnd.api+Json`, a parameterised mixed-case type, and a type with optional whitespace before the `;`.
- `json_without_a_content_type_is_parsed_leniently` (`extractors.rs:284`) - the documented leniency had no pin anywhere; a missing field is now held to the type's own doc.

The existing `json_rejects_wrong_content_type_with_415` was widened from one row to five, so a sloppier matcher cannot pass: `text/plain`, `application/json-seq` (a framed sequence, not one document), `text/plain; format=json` (a parameter is not the type), `application/vnd.foo+jsonx` (the suffix must end at the type's end) and `application/x-json` (bare `json` is not the `+json` suffix).

**Sensitivity by injection.** Seven mutants over `extract.rs`, each restored from bytes whose sha256 matched the file the campaign started from (`431c270bc2ac92e222e0bc82f33ed58c3fcd130abb24ce06c3d1317c7c29b829`):
- M1, the pre-fix case-sensitive suffix (`ends_with("+json")`): `application/vnd.api+JSON` answers `415`.
- M2, case-sensitive equality (`mime == "application/json"`): `APPLICATION/JSON` answers `415`.
- M3, `mime.to_ascii_lowercase().contains("json")`: `application/json-seq` is accepted and reaches the parser (`400` instead of `415`).
- M4, a non-anchored `contains("+json")` on the lowercased type: `application/vnd.foo+jsonx` is accepted.
- M5, `to_ascii_lowercase().ends_with("json")`: `application/x-json` is accepted.
- M6, two literals (`ends_with("+json") || ends_with("+JSON")`): `application/vnd.api+Json` answers `415`.
- M7, `.trim()` dropped from the media-type split: `application/vnd.api+json ; charset=utf-8` answers `415`.
M1-M3 died on the first campaign. M4-M7 survived that campaign and were named by the review; each of the four new rows was added to kill one, and all seven now fail the suite. The committed `extract.rs` hashes `431c270b...29b829` after the campaign and after the comment reword; `extractors.rs` hashes `259566231053b96390b4acd307b97930ae4a0b956e663b2b560e394eda654ea9`.

**Review.** A limited independent review ran read-only against the diff and the test run; its verdict was SHOULD-FIX, with four points. It confirmed the RFC reading, the absence of panic or index hazards (`checked_sub` plus `get`, both byte-based), the `Form` consistency and the now-truthful type doc. It named four surviving mutants (M4-M7), the stale count census (19 -> 21 and 425 -> 427 in six documents), the untested missing-`Content-Type` leniency, and the two stat-dirty governance tool files that must stay unstaged. All three substantive points were closed: the four rows were added and each mutant re-run red, the leniency pin was written, and every pinned count was re-derived from this run. The reviewer did not execute the suite or run any gate itself; those remain this track's own evidence. The two tool files stayed unstaged.

**Citations and counts.** The suite is **931 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **485** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **932** = 931 + the one ignored test. Integration grew 425 -> 427 in one target (`tests/extractors.rs` 19 -> 21, the two pins above); unit, the doc lane and every other target held. Six live documents were re-derived: the two READMEs, the completeness matrix (three printed copies, the extractors target count in the integration row, and the name-pass tree label), the baseline (header, per-target row and total), the readiness report (a new thirty-fifth dated reading, with the thirty-fourth demoted to "in that lane" and "the one ignored test" so each `counts` pin still names exactly one sentence) and `OWNER_VISION.md`. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans, and the name pass is unchanged at 393 qualified and 1390 bare names, both re-run on this tree. The `counts` gate reports 0 findings over six documents.

**Refused alternatives.**
- Lowercase the whole media type into a `String` before comparing - refused: `to_ascii_lowercase` allocates on every extracted JSON body, and M3/M4 show that a substring search over the lowercased value is also the wrong rule; the suffix check is already allocation-free.
- Accept any type whose subtype contains `json` - refused: `application/json-seq` frames a stream of texts and `text/plain; format=json` carries a parameter, not a JSON document; M3 is that implementation and it is now red.
- Pin only the mixed-case suffix and leave the equality side untested - refused: M2 shows the two comparisons are independent decisions, and `APPLICATION/JSON` is a client a case-insensitive media type explicitly permits.
- Make the check a free function with its own unit test - refused: the behaviour is observable at the wire boundary the extractor is, and the existing target already drives it through a real socket; a second entry point would test the same call from a narrower angle.

**What this track does not claim.** No performance number was produced; nothing was timed, and the check adds no allocation. No HTTP/2-specific pin was added: the extractor runs before dispatch in the shared core and reads the `Content-Type` field the h2 frontend already folds, so the HTTP/1.1 wire pin is the boundary this track claims; the h2 fold is track 106's evidence, not this track's. The type doc's promise is unchanged in wording and is now true. The run is Windows-only; PROB-0012 still blocks a cross-platform claim. Nothing else in the extractor module, `Form<T>`, the `415` shape or the missing-field leniency was changed.

## Track 134 - the h2 header budget wrapped when it was configured above the SETTINGS ceiling (2026-11-06)

**What was open.** Found by the maintenance pass that followed track 133, sweeping narrowing casts over the crates the server runs. `http2.rs` advertises the framework's header byte budget as the HTTP/2 `SETTINGS_MAX_HEADER_LIST_SIZE` field, which is a `u32`, with `limits.max_header_bytes as u32` (`http2.rs:241` before this track). `AETHER_MAX_HEADER_BYTES` parses any `usize` (`config.rs:131-135`) and `AetherConfig::max_header_bytes` is a public `usize` field, so a configured budget at or above 2^32 is accepted and then truncated modulo 2^32. `4294967296` becomes `0`, and `h2` reads a zero cap as "no header list is acceptable": every h2 request on that listener is answered 431 (h2 0.4.20 `proto/streams/recv.rs:221` returns `Oversize`, which sends 431 and `REFUSED_STREAM`) while the HTTP/1 frontend on the same listener honours the configured budget. The failure is silent, fails closed, and makes the two frontends disagree about a limit the framework presents as one.

**Reproduction.** The seam is a pure function of the configured value, so the pin is a unit test rather than a socket: driving the wrap on the wire would require the same 4 GiB configured budget, and that budget is reserved per connection (`app.rs:3061`), so the test would be measuring a server that cannot allocate. Against the pre-fix expression, `header_list_cap(4 * 1024 * 1024 * 1024)` returned `0` and `header_list_cap(u32::MAX as usize + 1)` returned `0`; the fix returns `u32::MAX` for both.

**The fix.** `header_list_cap(usize) -> u32` returns `max_header_bytes.min(u32::MAX as usize) as u32`, and `builder` calls it (`http2.rs:241`, `:245-253`). Above the ceiling the advertised and enforced cap is the ceiling itself, which is the largest value the protocol can express and still accepts every header list the HTTP/1 frontend would accept up to that size. No other call site narrows a configured value: the two `periods as u32`/`projected as u32` casts in `rate_limit.rs:123`/`:126` are reached only under `projected < burst`, and `burst` is already a `u32`, so both are exact by construction.

**Tests.** Two unit pins in `http2.rs`'s existing `tests` module:
- `the_h2_header_cap_saturates_instead_of_wrapping` (`http2.rs:1451`) - an 8 KiB budget passes through unchanged, `u32::MAX` stays `u32::MAX`, and `usize::MAX` clamps to `u32::MAX`.
- `the_h2_header_cap_does_not_wrap_just_past_the_ceiling` (`http2.rs:1459`, `#[cfg(target_pointer_width = "64")]`) - `u32::MAX + 1` and 4 GiB are both clamped to `u32::MAX`; the 64-bit gate is explicit because on a 32-bit target those two values are not representable in `usize`, and the first test already covers `usize::MAX` there.

**Sensitivity by injection.** Two mutants over `http2.rs`, each restored from bytes whose sha256 matched the file the campaign started from (`92a3911f3b18df3331f035d160f3d17e0297e0cce5fe78225369adca75dbe942`):
- M1, the pre-fix expression (`max_header_bytes as u32`): both tests fail; the first reports `left: 0, right: 4294967295`.
- M2, the clamp off by one (`min(u32::MAX as usize - 1)`): both tests fail at the boundary (`left: 4294967294, right: 4294967295`), which proves the pin holds the exact ceiling rather than merely "some large value".
The committed `http2.rs` hashes `92a3911f...dbe942` after both restores.

**Review.** No independent review was run for this track. It is a one-expression clamp with no new public surface, the claim about `h2`'s zero-cap behaviour is read from the vendored `h2` 0.4.20 source rather than inferred, and the record names that reading as the evidence class instead of presenting a socket result it does not have. The maintenance pass, not a review, is what this track's confidence rests on; that is weaker than tracks 130-133 and is stated here rather than glossed.

**Citations and counts.** The suite is **933 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **487** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **934** = 933 + the one ignored test. Unit grew 485 -> 487 in one crate (`aether` lib 97 -> 99, `http2.rs` 15 -> 17); integration, the doc lane and every other crate held. Six live documents were re-derived: the two READMEs, the completeness matrix (three printed copies), the baseline (header, `aether` row and total), the readiness report (a new thirty-sixth dated reading, with the thirty-fifth demoted to "in that lane" and "the one ignored test") and `OWNER_VISION.md`. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans, and the name pass at 393 qualified and 1390 bare names, both re-run on this tree.

**Gate history.** The first full ladder run on this tree failed `tests` on `graceful_shutdown_drains_streams_in_flight` (`GOAWAY`/`NO_ERROR` on the response head), and `counts` and `govtests` then failed on the truncated log (653 of 933 tests measured). That failure is recorded as failed rather than smoothed; track 135 fixed the test's missing synchronization, and this record's counts come from the later, fully green ladder run.

**Refused alternatives.**
- Reject a configured header budget above `u32::MAX` in `from_env` - refused: the struct field is public and is not built only through `from_env`, so the validation would be bypassable; a clamp at the protocol boundary holds for every construction path.
- Clamp the budget everywhere it is read - refused: HTTP/1's budget is a `usize` and has no ceiling, so lowering it to `u32::MAX` bytes would change a limit that is not broken. Only the `u32` protocol field needs the clamp.
- Return `u32::MAX` for any value above the ceiling by comparing at the wire budget instead - refused for the same reason: the wire budget is not the header-list cap, and the two are already separate settings inside `ServerLimits`.
- Leave the cast and document the 4 GiB boundary - refused: the wrap also produces small non-zero caps (4 GiB + 1024 becomes 1024), so the documented rule would be "any value above the ceiling is undefined", which is a contract no limit should have.

**What this track does not claim.** No performance number was produced; nothing was timed, and the clamp is one `min`. No wire test was added, for the reason in the reproduction: the trigger reserves 4 GiB per connection, so the socket path cannot reach it in a test. The behaviour of `h2` under a zero cap is quoted from the vendored crate's source, not observed on a socket. The HTTP/1 frontend was not changed and its budget semantics are unchanged. The run is Windows-only; PROB-0012 still blocks a cross-platform claim. The `#[cfg(target_pointer_width = "64")]` row is not exercised on a 32-bit target by this run.

## Track 135 - the graceful-drain test called an unread request an in-flight stream (2026-11-06)

**What was open.** Track 134's first ladder run failed a test this track did not touch: `graceful_shutdown_drains_streams_in_flight` panicked with `response head: Error { kind: GoAway(b"", NO_ERROR, Remote) }` (`tests/http2_e2e.rs:382`, surfaced at the caller's `expect("stream task")`), 85 of 86 `http2_e2e` tests passing and the run stopping after 653 of 933 tests. The `counts` and `govtests` gates then failed on that truncated log. The failure is recorded here as failed; nothing about it was smoothed.

**Root cause, read from the vendored `h2` 0.4.20 source.** `Connection::graceful_shutdown` sends an initial `GOAWAY` with `last_stream_id = 2^31-1` and `NO_ERROR`, then a shutdown `PING` (`proto/connection.rs:656-678`). When the `PING` ACK arrives it sends the final `GOAWAY` naming `streams.last_processed_id()` (`proto/connection.rs:598-609`). That value starts at zero and moves only when a request `HEADERS` frame is decoded (`proto/streams/recv.rs:105`, `:168`). A client errors every locally initiated stream whose id exceeds the `GOAWAY`'s last id (`proto/streams/streams.rs:812-825`). The observed error is therefore proof that the server had not processed that stream's `HEADERS` when the final `GOAWAY` was decided: the request was still in the socket. That is the protocol's documented outcome - RFC 9113 §6.8 says a stream above the last stream id may be retried - and this test client does not retry. The server path was correct; the test's premise was not.

**The defect.** The test began the request, slept 80 ms, then dropped the shutdown sender. A sleep is not a happens-before with "the server has read this request": under the load of a full workspace run either the client's driver or the server's connection task could still be scheduled late, and the test then observed the retryable outcome as a failure.

**The fix.** The test now registers `/slow` itself with a handler that sets an `AtomicBool` before its 400 ms sleep, and waits for that flag (5 ms poll, 1 s bound) instead of sleeping (`tests/http2_e2e.rs:1563-1596`). The handler cannot run before `conn.accept()` has returned the stream, and `accept()` cannot return a stream whose `HEADERS` were not decoded, so the flag is a happens-before with `last_processed_id >= stream id`. The assertion names the premise it needs: "the handler never started, so no stream was in flight to drain". The property the test pins - one stream inside its handler when shutdown starts finishes with `200 slow` while the listener drains - is unchanged, and is now genuinely the state the test claims.

**Evidence.** Before the fix, 48 isolated runs at 12-way parallelism and 12 full-suite runs (four parallel copies of `http2_e2e`, three rounds) did not reproduce the failure; the one observed failure is the track-134 gate run (86-test suite, 11.09 s, `target/gates/tests.log`). After the fix, 60 isolated runs at 12-way parallelism passed, 0 failures. The flake could not be reproduced on demand, so the causal chain above - the failure text, the `h2` source, and the absence of any synchronization with `last_processed_id` - is this track's evidence, not a repeatable red run. That limitation is stated rather than replaced by a count.

**Refused alternatives.**
- Raise the sleep to a "safe" value - refused: it is the same race with a smaller probability, and a test that passes because a machine was fast enough is not evidence.
- Retry the request when the client sees `GOAWAY/NO_ERROR` - refused: the test pins that *this* stream finishes during the drain, not that a retry of it does; a retry would let a server that aborts in-flight streams pass.
- Accept `GOAWAY/NO_ERROR` as a pass - refused: it would admit either outcome and pin nothing.
- Send only the initial `GOAWAY` from the server - refused: the second `GOAWAY` is `h2`'s and RFC 9113's graceful-shutdown contract; changing it would make the server lie about which streams it has seen.
- Add a synchronization hook or a delay to the server's drain - refused: the server's behaviour is spec-correct, and the missing happens-before was in the test.

**What this track does not claim.** No production line was touched and no framework behaviour changed. The flake was observed once; the fix is justified by construction (a handler-started write is ordered after `HEADERS` decoding) and by the failure's only possible source in the `h2` source, not by a repeatable reproduction. The suite is unchanged at **933 passed / 0 failed / 1 ignored** over 63 `test result:` lines (487 unit + 427 integration + 19 doc), `http2_e2e` 86; no document needed a count edit. The run is Windows-only; PROB-0012 still blocks a cross-platform claim. No independent review was run for this track.

## Track 136 - the desync audit accepted framings the parser refuses (2026-11-06)

**What was open.** Found by the maintenance audit of the security surface that followed track 135, not by a failing gate. `audit_request_desync` (`framework/security/src/smuggling.rs:16`) refused a repeated `Content-Length`, and a `Content-Length` beside a `Transfer-Encoding`, and nothing else, while its module doc claimed it also rejected "malformed chunked transfer encodings". The parser's framing rules are stricter: `body_framing` (`framework/http/src/parser.rs:667`) refuses any HTTP/1.0 request and any request whose `Transfer-Encoding` field count is not one or whose OWS-trimmed value is not exactly `chunked` (`:667-683`, counted in the head loop at `:399-408`), and that loop refuses a `Content-Length` that is not all digits (`:383-398`). So the public helper returned `Ok(())` for heads the server answers with a 400: two `transfer-encoding: chunked` fields, `transfer-encoding: gzip`, `transfer-encoding: chunked, gzip`, and `content-length: abc`. It is exported at `aether::audit_request_desync` and `aether::security::audit_request_desync` (`framework/aether/src/lib.rs:58`), so a caller using it as a pre-check over its own header handling would accept a request this server refuses.

**Reproduction.** A red-first unit module in `smuggling.rs` built the shapes against the real `HeaderMap`. Before any edit, the accepted matrix, the CL+TE refusal and the repeated-`Content-Length` refusal passed; `a_repeated_transfer_encoding_is_refused` (`smuggling.rs:140`), `a_coding_other_than_a_single_chunked_is_refused` (`:154`) and `a_content_length_that_is_not_a_decimal_number_is_refused` (`:178`) failed on their first case - `chunked` twice, `gzip`, `abc` - which is exactly the three gaps.

**The fix.** The one pass over the fields now remembers each framing field's value instead of only whether it appeared (`smuggling.rs:16-65`): a second `Content-Length` is still refused in the loop; `Content-Length` with `Transfer-Encoding` is still refused after it; a single `Content-Length` must be non-empty ASCII digits that fit a `usize`; a single `Transfer-Encoding` must be UTF-8 and OWS-trimmed to exactly `chunked`; more than one `Transfer-Encoding` field is refused rather than merged. `ows_str` (`:69-72`) is the one helper: UTF-8 or `MalformedRequest`, then space/tab trim, the same rule the parser's `ows_trim` applies. The module doc now states what is checked, that `Ok(())` means "the parser would accept this framing", and the two limits the input imposes: it is an audit helper, not the enforcement point, and headers carry no version, so a `Transfer-Encoding` on HTTP/1.0 cannot be refused here.

**Tests.** Six unit pins in the new `smuggling.rs` module, all against `HeaderMap` directly:
- `a_head_with_one_unambiguous_framing_is_accepted` (`:96`) - no framing fields, `0`, `5`, OWS-padded `5`, `0005`, and `chunked`/`Chunked`/tab-padded `CHUNKED`.
- `a_length_and_a_transfer_coding_together_are_refused` (`:111`) - both field orders.
- `a_repeated_content_length_is_refused` (`:123`) - same value, and a case-varied name.
- `a_repeated_transfer_encoding_is_refused` (`:140`) - two `chunked` fields, and `gzip` + `chunked`.
- `a_coding_other_than_a_single_chunked_is_refused` (`:154`) - `gzip`, `identity`, `deflate`, three list forms, a parameter, empty, two tokens, non-UTF-8.
- `a_content_length_that_is_not_a_decimal_number_is_refused` (`:178`) - letters, a comma list, a leading `+`, a negative, a blank, hex, an inner space, non-UTF-8 and a value no `usize` can hold.
The facade test `the_desync_audit_rejects_conflicting_framing_through_the_facade` (`tests/facade_wiring.rs:73`) gained the `gzip` and repeated-field shapes at `:98` and `:107`, so the fix is also pinned through the re-export a user actually calls.

**Sensitivity by injection.** Five mutants over `smuggling.rs`, each restored from bytes whose sha256 matched the file the campaign started from (`76efd19c34688dddf8824799c79bdb35e1c4e9cb54babce9d7322b98435a4f53`):
- M1, drop the duplicate-`Transfer-Encoding` refusal: `a_repeated_transfer_encoding_is_refused` fails.
- M2, accept any coding: `a_coding_other_than_a_single_chunked_is_refused` fails.
- M3, drop the digits-only check but keep the `usize` parse: `a_content_length_that_is_not_a_decimal_number_is_refused` fails on `+5`, which `usize::from_str` accepts - so the check is load-bearing, not redundant.
- M4, drop the CL+TE refusal: `a_length_and_a_transfer_coding_together_are_refused` fails.
- M5, drop the OWS trim: `a_head_with_one_unambiguous_framing_is_accepted` fails on the padded and tab-padded spellings.
The committed `smuggling.rs` hashes `76efd19c...5a4f53` after the campaign; `facade_wiring.rs` hashes `2f967622...2bab32`.

**Review.** No independent review was run for this track. It is one private helper and one public function with no new public surface, no allocation, and five mutants that each die on the assertion written for them; the parser's rule is the specification the tests read, and the track follows it rather than inventing a policy. The usual review target - a surviving mutant or an over-claimed sentence - was addressed by walking the module doc against the checked facts.

**Citations and counts.** The suite is **939 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **493** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **940** = 939 + the one ignored test. Unit grew 487 -> 493 in one crate (`aether-security` 60 -> 66); integration, its target count and the doc lane held, because the facade change adds assertions inside an existing test. Six live documents were re-derived: both READMEs, the completeness matrix (the evidence line, the unit row, the integration row's tree label, the name-pass tree label and the `re-measured true here` figure), the baseline (header, total row and the `aether-security` crate row), the readiness report (a new thirty-seventh dated reading, with the thirty-sixth demoted to "and the thirty-seventh reading below supersedes it") and `OWNER_VISION.md`. Every tree label moved from track-134 to track-136, which also closes the label drift track 135 left: its tree carried the same 933 count, so no gate could see it. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans (nothing cites `smuggling.rs`), and the name pass resolves one more qualified name - 394 against 1390 bare - because the matrix row now names the resolved `smuggling::tests` module.

**Refused alternatives.**
- Re-parse the head inside the audit - refused: the function takes a `HeaderMap`, not bytes; rebuilding a request line and version to reach the parser would invent the half of the request it was not given, and would make a header check cost a parse.
- Refuse every `Transfer-Encoding` field outright - refused: `chunked` is the one coding this server frames, so a blanket refusal would answer an unambiguous legal request as a desync; the audit's claim is "exactly what the server accepts".
- Enforce the HTTP/1.0 half unconditionally - refused: headers carry no version, and refusing `Transfer-Encoding` on every request would be wrong for HTTP/1.1; the limit is stated in the doc instead.
- Return a distinct error per shape - refused: the caller's decision is the same for all of them, and the parser itself answers every one with `MalformedRequest`.
- Pin only through the facade test - refused: the facade test lives in the socket-capable integration crate for what is a pure header function, and the unit module covers the accepted/refused matrix directly; the two names are one re-export, so one function is being tested, not two implementations.

**What this track does not claim.** No performance number was produced; nothing was timed, and the change adds no allocation (the accepted path reads each framing field at most once). No wire test was added: `audit_request_desync` is not in the serving path (nothing in `aether` calls it, as the matrix row already stated), so a socket test would only re-pin the parser. The chunked body decoder is untouched: this function checks the declaration, not the bytes that follow it, which the module doc now says. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 137 - a padded JWT signature was a second spelling of the same token (2026-11-06)

**What was open.** Found by the maintenance audit of the security surface that followed track 136, not by a failing gate. `JwtValidator::verify_signature` (`framework/security/src/jwt.rs:87`) validated the tag with `ring::hmac::verify` but put no constraint on the signature segment's spelling: `decode_base64url` trims trailing `=` (`jwt.rs:240-252`), so `header.payload.<43 chars>=`, `<43 chars>==` and any longer run of padding all decoded to the same 32-byte tag and verified. The tag is the one segment the MAC does not cover, so nothing else in the token distinguishes the spellings: an application that keys a deny-list, a replay cache or a rate limiter by token text would hold one credential under several strings, and a caller that validates a token then stores the original text could store the padded copy and later fail to recognise the canonical one.

**Reproduction.** Red first, before any edit: a unit test built a canonical HS256 token, asserted its signature was 43 characters and that it verified, then asked `verify` for the three padded spellings. All three verified. The same shape was added to the facade test after the fix, so the refusal is also pinned through the re-export a user calls.

**The fix.** A constant `SIGNATURE_CHARS: usize = 43` (`jwt.rs:11`) and one guard after the empty-component check and before decoding (`jwt.rs:107`): `if sig.len() != SIGNATURE_CHARS { ValidationFailed(...) }`. HMAC-SHA256 always produces a 32-byte tag, and 32 bytes have exactly one unpadded base64url spelling of 43 characters: any padding makes the segment longer, and no other length decodes to 32 bytes, so the rule accepts every genuine tag and rejects only aliases. The doc comment on `verify_signature` now states the canonical spelling and why the tag needs it. The check is on the signature alone: the header and payload are covered by the MAC, so a padded spelling of either changes the signed bytes and fails verification; there is no equivalent alias to close there, and applying the same fixed length to them would refuse valid tokens (M3/M4 below).

**Tests.** The new unit pin `a_padded_signature_is_not_another_spelling_of_the_token` (`jwt.rs:419`) and one assertion inside `a_jwt_verifies_through_the_facade` (`tests/facade_wiring.rs:61`). The unit test also asserts the 43-character length, so a future encoder change that shortened the tag cannot make the padded cases pass vacuously. The `aether-security` lib moves 66 -> 67 tests.

**Sensitivity by injection.** Five variants over the guard, each restored from a backup whose sha256 matched before and after (`96040d7f5f5c7f7fd974892bc4bec45d8c2cedb82adb37ef2ea9d2070c4bee5c` for `jwt.rs`; `facade_wiring.rs` hashes `7ca0b58b759cadf403625ad9e54abfb55fc908226edbc021b8994426943fff4e`):
- M1, disable the guard (`if false && ...`): the new test fails (8 passed, 1 failed) - the pin is load-bearing.
- M2, keep only the upper bound (`>`): 9 passed. Survived by equivalence: a shorter segment is not an alias, because it decodes to fewer than 32 bytes and `hmac::verify` refuses it; the equality rule states the whole property, the upper bound states half of it.
- M3, measure the header instead (`header.len()`): four tests fail - the header's length is not fixed, so this is not a conservative substitute.
- M4, change the constant to 44: four tests fail - the constant is the canonical length, not a loose ceiling.
- M5, reject only `sig.contains('=')` instead of the length: 9 passed. It is an equivalent alternative for every input the public result distinguishes (every padded spelling contains `=`, and an unpadded non-43-character segment still fails decode or verify); the length rule is kept because it states the canonical form directly rather than inferring it from a character that base64url never permits. Recorded as surviving-by-equivalence, not as a test gap.

**Review.** No independent review was run for this track. The change is one public-method guard with no allocation and no new surface; the mutants above are the adversarial check, and the equivalence argument for M2/M5 is written out rather than assumed.

**Citations and counts.** The suite is **940 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **494** over 19 unit runs, integration **427** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **941** = 940 + the one ignored test. Unit grew 493 -> 494 in one crate (`aether-security` 66 -> 67); integration, its target count and the doc lane held, because the facade assertion rides inside an existing test and no example changed. Six live documents were re-derived: both READMEs, the completeness matrix (the evidence line, the unit row, the integration row's tree label, the name-pass tree label and the `re-measured true here` figure), the baseline (header, total row and the `aether-security` crate row), the readiness report (a new thirty-eighth dated reading, with the thirty-seventh demoted to "in that lane" and "the one ignored test") and `OWNER_VISION.md`. The citation pins that named `jwt.rs:106` moved to `jwt.rs:123` in three teaching documents and in `cite_check.py`, because the guard and its doc comment shifted the `hmac::verify` line; the citation gate is unchanged at 112 citations in 16 live documents over 77 spans, and the name pass at 394 qualified and 1390 bare names. Two prose defects outside the pins were corrected while re-deriving: the matrix's JWT row still said "7 unit tests" when the file held 8 before this track (now 9), and the matrix evidence paragraph still said "the 455 tests added since" the 474-test recount, which is 466 on this tree.

**Gate history.** The first full-suite run on this tree failed `an_h2_body_past_the_ceiling_mid_stream_fails_the_read` (`tests/http2_e2e.rs:1397`, `chunk accepted: Error { kind: User(InactiveStreamId) }`), stopping the run after 35 of 63 result lines. That failure is recorded as failed rather than smoothed; it is not caused by this track. The test writes four 50-byte chunks against a 64-byte ceiling and expects every client write to be accepted, but once the server has failed the handler's read it may reset the stream, and a later write on that stream is legitimately refused by the `h2` client. A rerun was fully green (**940 / 0 / 1** over 63 lines), and the test race is track 138's scope. The `counts` and `cite` gates failed on the first post-edit run for the two reasons above (the thirty-eighth reading's cross-check phrase and the moved `jwt.rs` span) and both are green after the corrections.

**Refused alternatives.**
- Keep trimming padding and compare the decoded tag only - refused: that is the defect; trimming is what makes one credential have many spellings.
- Trim the segment before checking its length - refused for the same reason: it would accept every alias and then have nothing left to check.
- Apply the fixed 43-character rule to every segment - refused: only the HMAC tag has a fixed length; M3/M4 show the rule breaks valid tokens when applied to the header.
- Make `decode_base64url` globally strict about padding - refused: it would be a wider behavioural change to the facade's decoding for no security gain on the header or payload, which the MAC already covers; the tag is the only segment where padding is invisible.
- Reject any segment containing `=` without a length rule - refused as the shipped rule, though it is observably equivalent today (M5): the canonical length is the property, and the rule should name it rather than a character that happens to be present.
- Add a new public option to accept padded tokens - refused: RFC 7515 §2 spells the segments unpadded, and an option to admit aliases would create the attack surface the fix closes.

**What this track does not claim.** No performance number was produced; nothing was timed, and the guard is one length comparison before an existing decode. No wire test was added: the JWT path is exercised through the facade test, not a socket, and this track adds an assertion to that existing test rather than a new target. The fix does not make token text canonical anywhere else (a caller still handles the original string), and it does not touch the signed cookie or CSRF paths, which already require a 43-character MAC and reject `=`. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 138 - the h2 mid-stream ceiling test asserted a scheduling order (2026-11-06)

**What was open.** Surfaced as a failed gate during track 137's first full-suite run, recorded there as failed and deferred here. `an_h2_body_past_the_ceiling_mid_stream_fails_the_read` (`framework/tests/tests/http2_e2e.rs:1368`) sends four 50-byte DATA frames without END_STREAM against a 64-byte `max_stream_body_bytes` ceiling, then expects the handler's 413. It asserted that every client write was accepted (`.expect("chunk accepted")`), and the suite failed at `http2_e2e.rs:1398` with `chunk accepted: Error { kind: User(InactiveStreamId) }`. The failure is timing-dependent: that run stopped after 35 of 63 result lines, five isolated reruns passed, and the rerun used for track 137's census was green.

**Root cause.** The framework's answer is correct: past the ceiling, `read_body` fails with `TooLarge`, the handler returns 413, and the server resets the request stream so the client stops uploading. What the client observes is when that RST_STREAM lands relative to its own queued writes: the four `send_data` calls are enqueued without awaiting the connection, so usually all four succeed locally, but if the reset is processed between two calls the next one legitimately returns `InactiveStreamId`. The test asserted "all four writes are accepted", which is not a property of the refusal; it is a property of the race between client queueing and the server's reset. No other assertion in the test covered an error-raising write, so the shape was never chosen deliberately.

**The fix.** The loop counts the bytes the client accepted, stops at the first refused write, and asserts `sent > 64` before reading the response (`http2_e2e.rs:1395-1408`). Crossing the ceiling stays a real precondition: a reset that arrives before 64 bytes were enqueued fails the test instead of making it vacuous. The rest is unchanged and still strict: the response must be 413 and the same connection must serve a following GET, so a broken connection cannot hide behind the tolerance.

**Reproduction and sensitivity.** Red first, before the edit: a 100 ms sleep was inserted after the second chunk with the old loop, and the test failed deterministically at `chunk accepted: Error { kind: User(InactiveStreamId) }` (0 passed, 1 failed).
- S1, the old loop with that forced reset: fails as above.
- S2, the new loop with the same forced reset: passes (1 passed), so the tolerance covers exactly the write the old shape rejected.
- S3, ceiling injected to 0 and the same 100 ms sleep before the third chunk: the precondition fires - "the body must cross the 64-byte ceiling before any reset: only 50 bytes were enqueued" - proving the guard rejects a reset that precedes the crossing.
- S4, at the shipped 64-byte ceiling a reset before the crossing is unreachable by construction (the first chunk is 50 bytes and the server cannot fail the read before more than 64 arrive), so removing the guard leaves the test green today. The guard is recorded as a defensive precondition, not a live assertion; S3 is the injection that shows it fires.
- Without any injected sleep the fixed test passed 10 of 10 consecutive runs.

The file was restored byte-identically after each injection; `http2_e2e.rs` hashed `d8086c280aa6af4888a77e47526a302b995a894ec02d08914c799d453a7f6ff3` before the change and `260d91a16ea04d7f650dd7ab3e38b0c45dcd5d20d38e67a00b80fa3994bc9295` after it.

**Counts and gates.** No test was added, renamed or removed: the suite stays 940 passed / 0 failed / 1 ignored over 63 result lines (494 unit over 19 runs, 427 integration across 30 targets, 19 doc over 14 runs), so no count document was re-derived. The full ladder is green on this tree, including a fresh `cargo test --workspace`.

**Refused alternatives.**
- Keep `.expect("chunk accepted")` and accept the flake - refused: a gate that fails on scheduling teaches nothing and hides real failures.
- Insert a yield or sleep between chunks so every write lands before the reset - refused: that is the same scheduling dependence written the other way, and it makes the test slower without making it true.
- Tolerate any send error without the byte precondition - refused: a reset before the crossing would then pass silently.
- Match only `User(InactiveStreamId)` - refused as the shipped rule: it couples the test to one h2 error mapping, while the 413 answer and the following request already distinguish a dead connection from a stream reset.
- Make the server drain the remaining body instead of resetting - refused: that reads an unbounded upload to prove a refusal it has already given; the reset is the correct signal to stop sending.

**What this track does not claim.** No product code changed: the refusal, the reset and the 413 were already correct, and this is a test-shape fix. No performance number was produced. The tolerance does not weaken the assertions about the refusal; it removes one assertion about client-side write ordering that was never part of the contract. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 139 - the CSRF gate ran above the refusals that are cheaper than it (2026-11-06)

**What was open.** Found by the maintenance audit of the request prologue in `serve.rs` that followed track 138, not by a failing gate. The three security checks in the serving path are documented as ordered by cost: the host allow-list is a header comparison, the rate limiter takes a lock, and CSRF is the only one that does cryptographic work - verifying the cookie's HMAC and, on a safe request without a cookie, minting a token from OS entropy. Since commit 589b199 introduced CSRF, that block sat above host validation and the limiter, so every request paid the cryptographic work before a cheaper check could refuse it, and a flood absorbed by the limiter was answered with one HMAC verification per request. The step-3 comment above the block still claimed "Host first" and that a refused request "never pays for router work either"; the second half had been false since the serving-core extraction, because the route match runs above the security block and its result is reused by the CORS, OPTIONS and dispatch branches.

**The fix.** Host validation and the limiter now run first and fold into one `rejection` option (`framework/aether/src/serve.rs:406-433`). The CSRF block (`serve.rs:435-486`) keeps its single-arm structure under `Some(csrf) if rejection.is_none() && !csrf.is_exempt(&path_str)`, and the 403 is chained after the two cheaper answers with `rejection.or_else(|| csrf_refused.then(|| Response::status(403, "Forbidden")))`. The comment above the block was rewritten to state the order actually shipped, to name the one routing work a refusal still pays for - the route match, a deterministic side-effect-free lookup whose result three later branches reuse - and to say why CSRF is last. The comment also records the one cryptographic draw this ordering cannot defer: an app whose CSP policy names `{nonce}` draws it above this point, because a refused response carries the policy header too. No wire behaviour changed: the statuses (400 for the host list, 429 with `Retry-After` for the limiter, 403 for CSRF), the empty CSRF refusal body, and the cookie rules (a token only on `status < 400` and not a preflight) are identical before and after.

**Tests.** One wire pin, `the_rate_limiter_outranks_the_csrf_gate` (`framework/tests/tests/csrf.rs`, target 9 -> 10 tests): with a burst of 1 the first `GET /form` is answered 200 and carries exactly one cookie; the next `POST /submit` is the limiter's 429 rather than the gate's 403; a further `GET /form` over budget is 429 and carries no token, because a refused request has no page to protect. The test name and its doc comment state the precedence it preserves.

**Sensitivity by injection.** Recorded honestly: inverting the refusal chain alone does not turn the test red, because the `rejection.is_none()` guard already skips CSRF on a refusal - the chain order is only observable when both pieces are mutated. The combined mutant (guard removed, chain inverted) makes the test fail with `left: 403, right: 429`, so the pin does reject an observable regression and is a preservation pin for chain precedence rather than a red-first test of the reorder. The work-avoidance the reorder buys is not observable through the wire at all (a 429 carries no cookie either way), so it was measured with a temporary `eprintln!("CSRF-CONSULTED")` placed in the guarded arm: the shipped order printed the marker once for three requests, and the old order (guard removed) printed it three times. The marker was removed and `serve.rs` restored byte-identically - sha256 `e984d3e670e67cea952d3eb1c54846df85db66daf557f8bf6cb77d43588c2038` before and after. `csrf.rs` hashes `d596789b6dd09fdb460710b1251cd8d50f6d444c9d92a14baf727bddf7253ba9` after the change; `http_e2e.rs` hashes `4aaaa541fca780a26d8aa783fb080e8e876b339fde425d1dd2e55af0113477a3` after its one doc line and the test rename.

**Also corrected.** `host_validation_refuses_an_off_list_host_before_the_handler` (`framework/tests/tests/http_e2e.rs:3346`) was named `host_validation_refuses_before_routing`, which overstated the guarantee - the route match runs above the host check. The test and its doc comment now say what is actually pinned: the handler does not run and the rejection body does not echo the hostile Host value.

**Review.** No independent review was run for this track. The change is a reorder of three existing blocks plus one guard, with no new surface and no allocation; the mutants above are the adversarial check, and the unobservable work-avoidance is reported as measured through a temporary marker rather than claimed from the code alone.

**Citations and counts.** The suite is **941 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **494** over 19 unit runs, integration **428** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **942** = 941 + the one ignored test. Integration grew 427 -> 428 in exactly one target, `tests/csrf.rs` 9 -> 10; unit and the doc lane held, because this track changed no in-crate test and no example. Six live documents were re-derived: both READMEs, the completeness matrix (evidence line, unit row, integration row and its per-target count, the name-pass tree label and the "re-measured true here" figure), the baseline (header sentence, the integration per-target list and total row), the readiness report (a new thirty-ninth dated reading, with the thirty-eighth demoted to "in that lane" and "the one ignored test") and `OWNER_VISION.md`. Seven coordinates moved in six documents because the reorder shifted everything after it: the 504 span `950-955` -> `958-963` in both `docs/00` documents, the CSRF block `402-439` -> `440-477` in `docs/04` and `docs/09`, `within_request` `883-890` -> `891-898` in the matrix, and the two `http_e2e` test attributes `5085`/`5121` -> `5086`/`5122` in `ERROR_HANDLING_GUIDE_AR.md`; `cite_check.py`'s pin table moved the same five spans. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans, and the name pass at 394 qualified and 1390 bare names.

**Gate history.** The first ladder run after the code change failed `fmt` (one `assert_eq!` needed wrapping; fixed with `cargo fmt --all`), `cite` (11 findings: the seven moved coordinates plus the four pins they left stale), `counts` (1 finding: the new reading made the pinned sentence shape match two paragraphs until the thirty-eighth reading was given "in that lane") and `govtests` (the same findings, re-reported). `clippy`, `tests` and `evidence` passed on that run; `validate` kept exactly its four pinned findings. Those failures are recorded as they happened rather than smoothed, and all of them are green after the repairs above. A review of the new comment after the commits found an overreach in it: a refusal also pays the CSP nonce draw when the policy names `{nonce}` (`serve.rs:347`, above the route match, because `post_process` applies the policy to every response). The claim was narrowed to "the one routing work a refusal still pays for", the nonce draw was named in the same comment, and the line count was kept at eight so no coordinate or pin moved; the comment and this record were corrected in their own commit.

**Refused alternatives.**
- Leave CSRF above the cheaper checks and only correct the comment - refused: the order is the defect the comment was hiding, and a request the limiter is about to refuse should not cost an HMAC verification first.
- Move the route match below the security block as well - refused: its result is reused unconditionally by CORS, OPTIONS and dispatch, and the lookup is deterministic and side-effect-free; moving it would need a second lookup or entangle three branches for no security gain.
- Drop the `rejection.is_none()` guard and rely on chain order alone - refused: a refused safe request would mint a cookie again the moment the chain is reordered, and the new pin would become vacuous.
- Answer 403 ahead of 429, or collapse every refusal into one status - refused: the statuses carry different retry semantics (429 names `Retry-After`, 403 deliberately distinguishes nothing) and are already documented that way.
- Add a counter or metric for skipped CSRF work - refused: no consumer exists, and it would be a feature inside a maintenance change.

**What this track does not claim.** No performance number was produced; nothing was timed, and the change only avoids work that was already being done before a refusal - whether that moves a benchmark is the owner's to measure. Nothing changed on the wire, so no new target was added; the one pin preserves the precedence rather than demonstrating a new answer. CSRF work on the admitted path is unchanged: it still verifies or mints exactly as before. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 140 - the CSP nonce was drawn before a refusal could be decided (2026-11-06)

**What was open.** The review of track 139 named the one draw that ordering still could not defer: an app whose policy names `{nonce}` had its token generated in the request prologue (`framework/aether/src/serve.rs:344-361` before this track), above the route match and above every security refusal. That placement was chosen in track 96 so one value could serve both the handler and `post_process`, but it had two consequences the tree never needed. First, every answer the framework writes itself - a host or limiter refusal, a CSRF `403`, a `404`, a `405`, a preflight, the generic `OPTIONS` - paid a `ring::rand::SystemRandom` draw for markup it would never render. Second, and worse, the failure mode of the entropy source became the failure mode of the *refusals*: a draw that fails returns `500` from the prologue, so a limiter that was about to answer `429` (or a host check about to answer `400`) answered `500` instead, hiding the real decision behind an unrelated failure. The middleware already defines the answer to the second half: `apply_with_nonce(response, None)` removes the `'nonce-…'` source expression rather than emitting it empty (`framework/middleware/src/security.rs:95-161`), which is the fail-closed rendering a response with no nonce should carry.

**The fix.** The token is declared where the dispatch decision is made and drawn only on the branch that will run something: inside `MatchResult::Found`, behind `reaches_dispatch` - true for any route that is not a WebSocket route, and for a WebSocket route only on a `GET` over HTTP/1.1 (`aether/src/serve.rs:585-586`). The draw and its `500`-on-entropy-failure answer moved with it (`serve.rs:591-604`), so an entropy failure can now only affect a request that was going to reach a handler. Every framework answer keeps the configured policy and goes out with the nonce expression removed, which is what the middleware's documented `None` path already produces. The step-3 comment states the new rule instead of the old one: no nonce is drawn for a refusal, because no dispatched markup can name one.

**What the first test version got wrong.** The first attempt extended the `http_e2e` nonce test with a WebSocket route reached by a plain `GET`, expecting the nonce-free `400` there. It failed: `allows_upgrade` is `version == Http11`, not a property of the request headers, so on HTTP/1.1 a `GET` without upgrade fields *does* enter the middleware chain and its `400` comes from the handshake validation inside it. That is a dispatched answer - a middleware could have rendered markup naming the nonce - so drawing there is correct, and the test claim was wrong. The reachable pre-chain case is HTTP/2, where the frontend passes `allows_upgrade: false` for every stream; `a_websocket_route_reached_over_h2_answers_400` (`framework/tests/tests/http2_e2e.rs:954`) already pins that gate and its chain counter, and it now also pins the CSP half.

**Tests.** Two existing tests, extended, so no census figure moved. `a_csp_nonce_reaches_the_handler_and_the_header_that_authorises_it` (`framework/tests/tests/http_e2e.rs:1714`) now runs with a limiter at `burst: 3` and a `fetch_framework_answer` helper: a `GET /missing` is a `404` whose `Content-Security-Policy` is exactly `default-src 'self'; script-src 'self'` - no `nonce-`, no `{nonce}` - then the two dispatched fetches keep their existing nonce assertions (same value in header and body, a second response drawing a different one), and the fourth request is the limiter's `429`, which must be the limiter's answer rather than a `500` and must carry the same nonce-free policy. `a_websocket_route_reached_over_h2_answers_400` now enables the same policy and asserts its `400` also carries `default-src 'self'; script-src 'self'` while its middleware counter still reads zero.

**Sensitivity by injection.** Two mutants, both killed. Restoring an eager draw in place of the `None` initializer (drawing for every response, as before this track) turns the `404` assertion red with `no handler ran, yet the policy names a nonce`. Forcing `reaches_dispatch = true` turns the h2 assertion red with `no chain ran, yet the policy names a nonce` while the h2c variant stays green, so the pin is watching the gate and not the transport. Both mutants were reverted; `serve.rs` hashes `9e1e3148a06822349c9783003df926eedaddce75093b996946082f4095003114`, `http_e2e.rs` `ee9aa6e88b8338ac18adb496f3ddc7cbc60ff264534e39d15640e30eb8b42a53` and `http2_e2e.rs` `bc141079fcda5fbede369544b7aa19f67dffc93f7d2f0ce9796a3ad5c371bd79` after the restoration.

**Review.** No independent review was run. The change moves one draw and its failure branch into an existing match arm, adds one two-condition reachability predicate, and extends two tests; the mutants above are the adversarial check, and the h1 WebSocket case is the review finding this track found on itself and corrected in the test and the prose.

**Citations and counts.** The suite is unchanged at **941 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **494** over 19 unit runs, integration **428** across 30 targets, doc **19** over 14 doc runs - because both tests were extended rather than added. Five coordinates moved. Removing the 18-line prologue block and inserting the guarded draw above dispatch shifted the CSRF block `440-477` -> `422-459` in `docs/04` and `docs/09`, the 504 span `958-963` -> `972-977` in both `docs/00` documents, and `within_request` `891-898` -> `905-912` in the matrix; extending `http_e2e` shifted the two test-attribute pins `5086`/`5122` -> `5119`/`5155` in `ERROR_HANDLING_GUIDE_AR.md`. The `cite_check.py` pin table moved the same five spans. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans; the name pass grew 1390 -> 1391 bare names because the CSP row now names `a_websocket_route_reached_over_h2_answers_400`, and both documents that print that figure were updated. The readiness report's item 8 and the matrix's CSP row now say "a request that reaches a handler" and list the framework answers that carry the policy with the expression removed.

**Also corrected.** The roadmap's §19.7 table had a blank line between rows 138 and 139, which ended the table before its last row; the blank line is gone and row 140 follows row 139 in the same table.

**Gate history.** The first `cite` run after the code and test changes reported two findings - the `http_e2e` attribute pins had moved - and the repair left a second run with two findings because the error guide spells its second coordinate as a shorthand `:5122` and the first substitution only matched the full `http_e2e.rs:5122` form; both were fixed. The full ladder then passed all eight gates: `fmt`, `clippy`, `tests`, `cite`, `evidence`, `counts`, `govtests`, and `validate` with exactly its four pinned findings. No gate failed on the final tree, and none was skipped.

**Refused alternatives.**
- Keep the eager draw and document it, as track 139's comment did - refused: it is the coupling this track removes, and an entropy failure would keep masking a `429` or `400` as a `500`.
- Skip the `Content-Security-Policy` header on framework answers - refused: a page served with no policy is weaker than one whose policy names no nonce, and the middleware already defines the fail-closed rendering.
- Emit the `'nonce-'` expression empty when there is no token - refused: browsers ignore an invalid source expression, so the header would look enforced while authorising nothing.
- Move generation inside `dispatch` and return the token through its `Option<Response>` - refused: two call sites and the upgrade path would need a new signature for no extra coverage, because every remaining refused-before-dispatch answer is already excluded by the reachability predicate.
- Add a separate test target for the h2 half - refused: extending the existing test keeps the census, the pins and the documentation honest without count churn.

**What this track does not claim.** No performance number was produced; the avoided work is one `SystemRandom` draw plus an allocation per framework answer, and nothing was timed. The wire changed for framework answers - their policy now omits the nonce expression - which is stricter, not weaker, and is pinned. A WebSocket route reached over HTTP/1.1 without upgrade fields still draws, because its `400` is produced after the middleware chain and a middleware there may legitimately render markup that names the nonce. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 141 - a static site's index page was reachable only when directory listing was on (2026-11-06)

**What was open.** Found by the adversarial review of the static-file path, not by a failing gate. Track 97 recorded the decision that a directory holding an `index.html` is still that page "listing on or off", and `serve_directory` did implement the lookup - but `serve_from_dir` returned `404` for a directory before it ever reached that function whenever `directory_listing` was off, and the lookup itself sat below the listing branch. With the defaults every `static_dir` uses, a request for a directory that held an `index.html` was answered `404`, so a plain static site's home page and every sub-directory page were unreachable. The tree root had a second, independent hole: the registered catch-all is `{prefix}/*filepath`, and a catch-all requires at least one character after the prefix (`test_catchall_captures_full_remaining_path`), so `/assets` and `/assets/` reach a handler only through the extra prefix route `static_dir_scoped` registered when `directory_listing` was on. A tree served with defaults whose root shipped an `index.html` therefore had no route to that page at all.

**The fix.** `serve_from_dir` (`framework/aether/src/static_dir.rs:94-108`) now looks for a contained `index.html` before it branches on listing, and returns that page when one exists; the listing branch is what remains for a directory that holds no page. The hoist is what makes the page reachable without the flag - the comment says so at the branch. `serve_directory` (`static_dir.rs:117`) lost its `root` parameter and its index block and is now only the listing for such a directory. The registration in `static_dir_scoped` (`framework/aether/src/app.rs:1816-1822`) reads `tree.root.join("index.html").is_file()` once at registration and registers the prefix route when the listing is on or the root ships that page; `find` trims trailing slashes, so the one registration answers both `/assets` and `/assets/`, with the `301` still applying only to a directory that is about to be listed.

**Test.** `an_index_page_is_served_when_listings_are_off` (`framework/tests/tests/http_e2e.rs:1201`, target 101 -> 102) writes `site/index.html` (`<h1>home</h1>`), `site/other.txt` and `site/app/index.html` (`<h1>app</h1>`), registers `app.static_dir("/assets", …)` with defaults, and asserts `200` with the exact body and `text/html; charset=utf-8` for `/assets/app/`, `/assets/app`, `/assets/` and `/assets`; then `404` for `/assets/missing/` and `200` with `other` for `/assets/other.txt`, so the hoist cannot turn a missing directory or a plain file into a page.

**Red first, and sensitivity.** Before the fix the test failed at `/assets/` with `assertion left == right failed: /assets/ left: 404 right: 200`. Two mutants, each killed on a different path, so each half of the change is watched separately:
- M1: remove the hoisted check and restore the index lookup inside the listing branch only - `/assets/app/` answers `404` where the test wants `200`, because listings are off.
- M2: register the prefix route only when `tree.options.directory_listing` - `/assets/` answers `404`, so the root half of the pin is real.
Both were reverted. Hashes after restoration: `static_dir.rs` `daee56dcfffeb55f92d91c5ea15c6b15a02d2c8506d20aa77d886cad1f91cf8d`, `app.rs` `ba75cf9a6297a0d7eb8575f170a4238a5a2e0249d686559c7cb4b25a827cd1ff`, `http_e2e.rs` `fed9f34ac025297bf216dccfbc9c16409632f51bd9587a9aabf036feb18eb3be`.

**Citations and counts.** The suite is **942 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **494** over 19 unit runs, integration **429** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **943** = 942 + the one ignored test. Integration grew 428 -> 429 in exactly one target, `tests/http_e2e.rs` 101 -> 102; unit and the doc lane held, because this track added no in-crate test and no example. Ten printed figures were re-derived across six live documents: both READMEs (suite total and the end-to-end count), the completeness matrix (evidence line, unit row, integration row and its per-target count, the name-pass tree label, the "re-measured true here" figure and the two name counts), the baseline (header sentence, per-target table row 101 -> 102, total row), the readiness report (a new fortieth dated reading, with the thirty-ninth demoted to "in that lane" and "the one ignored test"), and `OWNER_VISION.md`. Twenty-one coordinates moved because the insertion adds three lines at `app.rs:1816`: nineteen `app.rs` pins and their citations in six documents shifted **+3** (2613 -> 2616, 3015-3036 -> 3018-3039, 2968-2976 -> 2971-2979, 2977 -> 2980, 2897-2901 -> 2900-2904, 3875-3877 -> 3878-3880, 2406-2618 -> 2409-2621, 2888 -> 2891, 2928 -> 2931, 2967 -> 2970, 2974 -> 2977, 4011 -> 4014, 2778 -> 2781, 2317 -> 2320, 4240 -> 4243, 2979 -> 2982, 4289 -> 4292, 3061 -> 3064, 2248-2255 -> 2251-2258), and the two `http_e2e` test-attribute pins in `ERROR_HANDLING_GUIDE_AR.md` shifted **+55** (5119 -> 5174, 5155 -> 5210). `cite_check.py`'s pin table moved the same 21 keys. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans; the name pass grew 1391 -> 1393 bare names, and the only new prose in the live set is the matrix row that now names the new pin.

**Documents corrected.** The matrix's directory-listing row now states that the index is served in either mode, from the moment the path resolves to a directory, and that the prefix route exists when the listing is on or the root ships an `index.html` at registration; its OpenAPI row names that extra `{prefix}` operation. The readiness report's static-dir bullet records the defect and its closure (before track 141 only a listing-enabled tree could serve a site's home page).

**Gate history.** The first ladder run after the code change passed `fmt`, `clippy`, `tests` and `evidence`; `cite` failed with exactly the 21 predicted findings, `counts` with the 25 printed figures, and `govtests` re-reported the same stale documents. After the migrations above, `cite` reports 0 findings at 112 citations / 77 spans, `counts` reports 0 findings over six documents, and `govtests` passes. The full ladder then passed all eight gates: `fmt`, `clippy`, `tests`, `cite`, `evidence`, `counts`, `govtests`, and `validate` with exactly its four pinned findings and no fifth.

**Refused alternatives.**
- Always register the prefix route, whatever the options - refused: a default `static_dir` over a tree with no `index.html` would then publish an operation the tree cannot answer, and `a_static_dir_is_listed_as_the_catch_all_it_registers` pins the derived document to exactly one operation for that case. It would also make a user route at the prefix panic through `reject_duplicate`, since the framework route and the user's would collide.
- Keep the index lookup inside `serve_directory` and call it from both branches - refused: the function is the listing; hanging a page lookup that has nothing to do with listing off it is what hid the defect. Two call sites for one branch is more surface than one check at the branch that decides.
- Serve `index.html` by rewriting the request path and re-entering `serve_from_dir` - refused: a second resolution pass would re-run `safe_resolve_path` on a path the server itself built, and the direct containment check is one `contained_child` call.
- Stat the root index on every request instead of once at registration - refused: the route table is fixed at listen time, so a route cannot appear later without a restart; the registration check is the only point where the answer can be acted on. The cost is stated below.
- Make the default a listing when an index is absent - refused: that is the opt-in policy track 97 decided, and a listing leaks filenames.

**What this track does not claim.** A root `index.html` added to a running server is not picked up: the route table is fixed at registration, so the operator restarts. A sub-directory index needs no restart, because it is resolved per request. No performance number was produced; the only cost is one `stat` at registration per static tree. The wire answer for a directory with no page and no listing is still `404`. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 142 - a streaming route refused a declared upload larger than the buffered ceiling (2026-11-06)

**What was open.** Found by the adversarial review of the request paths, not by a failing gate. `max_stream_body_bytes` exists so a streamed body may be larger than the buffered cap: `limits.rs` says so, `AetherConfig` repeats it, and the HTTP/2 frontend honours it - `http2.rs:544` refuses only a `declared_length` over `max_stream_body_bytes`. On HTTP/1 the shared head parser refused any `Content-Length` over `limits.max_body_bytes` before the route was known, so `parse_request_head` - the streaming entry point - answered `413` for every declared length above 2 MiB. The only way a streaming route could carry more than the buffered ceiling was chunked framing, the opposite of the documented contract and inconsistent with the same route over h2.

**The fix.** `parse_head` (`framework/http/src/parser.rs:99-106`) and `parse_head_into` (`:339-345`) now take `declared_body_ceiling`, and the comparison at `:462-467` is against that parameter. The buffered callers name the buffered ceiling (`RequestView::parse` at `:63`, `parse_request_incremental` at `:754`), while the streaming entry point (`parse_request_head` at `:730`) and `awaits_100_continue` (`:599`) pass `usize::MAX`, because which ceiling a declared length must fit is the route's decision and the route is resolved after the head. No new limit was introduced: `app.rs:2451-2458` already refused a declared length over `max_stream_body_bytes` before dispatch, and that refusal remains the HTTP/1 streaming ceiling; every buffered path is unchanged.

**Test.** Unit `test_streaming_head_leaves_the_declared_length_ceiling_to_the_route` (`parser.rs:1433`) sets `max_body_bytes: 10`, `max_stream_body_bytes: 4096` and `Content-Length: 100`; it asserts the streaming head parses with `BodyFraming::ContentLength(100)`, that `parse_request` and `RequestView::parse` still answer `BodyLimitExceeded` for the same bytes, and that `awaits_100_continue` answers true only for the paused request. Wire `a_declared_length_over_the_buffered_ceiling_streams_to_its_route` (`framework/tests/tests/streaming_request.rs:376`, target 12 -> 13) uploads 1 000 bytes with `max_body_bytes: 64` and `max_stream_body_bytes: 4096`, reads them through the handler, and asserts the exact length and checksum body with the connection kept.

**Red first, and sensitivity.** Before the fix the wire test failed with `left: (413, [])` against `right: (200, [49, 48, 48, 48, ...])`. Three mutants, each killed:
- M1: make `parse_request_head` pass `limits.max_body_bytes` - the unit test panics waiting for a complete head, and the wire test sees `413`.
- M2: make `parse_request_incremental` pass `usize::MAX` - killed at `assert!(matches!(parse_request(raw, &limits), Err(AetherError::BodyLimitExceeded)))`.
- M3: make `awaits_100_continue` pass `limits.max_body_bytes` - killed at `assert!(awaits_100_continue(expect, &limits))`.
All three were reverted and the restored text was re-verified by diff and by re-running both tests. Hashes after restoration: `parser.rs` `d0d12350cb3ec8a3318a404bcd59fb6f3868335c`, `limits.rs` `5835b57fcf863512c50f79b0081125ff8848c3e9`, `config.rs` `cbb1bf4469aca117cce1f3406f2a2bf8a72a7cab`, `streaming_request.rs` `5f7ca9f711a7db36ae2cbdf1345a1392e9acb9f3`, `fuzz/src/lib.rs` `8ebd3c8117fb7105d4b6dddc8a7b61f5d1392695`.

**Citations and counts.** The suite is **944 passed / 0 failed / 1 ignored** over **63** `test result:` lines - unit **495** over 19 unit runs, integration **430** across 30 targets, doc **19** over 14 doc runs - with the 63 `running N tests` lines summing to **945** = 944 + the one ignored test. Unit grew 494 -> 495 in one crate (`aether-http` 112 -> 113) and integration grew 429 -> 430 in one target (`tests/streaming_request.rs` 12 -> 13); the doc lane held. Every printed figure the count gate can see was re-derived in six live documents: both READMEs, the completeness matrix (evidence line, unit row, integration row, the streaming target count, the name-pass reading and the "re-measured true here" figure), the baseline (header sentence, current reading, the `aether-http` row, the per-target list and the total row), the readiness report (a new forty-first reading, with the fortieth demoted to "in that lane" and "the one ignored test"), and `OWNER_VISION.md`. Two statements the gate does not pin were corrected in the same pass: the matrix's unit row printed `941 passed` (it should have read 942 before this track), and the matrix's streaming row said "Ten socket tests" in a target that held 12, then 13; both now match the file. Nine coordinates and their pins moved: `parse_request_incremental` 725 -> 748 (three documents), the smuggling span 370-452 -> 382-468, the CL.TE test 1107 -> 1132, the declared-length comparison 449 -> 465 (its pin now watches `declared_body_ceiling`, so the buffered ceiling cannot be restored without moving it), the two chunked bounds 1042 -> 1067 and 1059 -> 1084, and the fuzz oracle `RequestView::parse` 87 -> 86. The citation gate is unchanged at 112 citations in 16 live documents over 77 spans; the name pass grew 1393 -> 1397 bare names - two from the rewritten limits page and two from the matrix streaming row.

**Documents corrected.** `docs/07_limits_and_dos_mitigation.md` no longer says every declared `Content-Length` is checked against `max_body_bytes`; its decoded bullet now names the split - the buffered entry points refuse against `max_body_bytes`, the streaming head applies no ceiling, and the route applies `max_stream_body_bytes` before dispatch. The completeness matrix's streaming row states the same rule and names the new pin; its unit row's stale `941` is corrected. The fuzz oracle's comment in `framework/fuzz/src/lib.rs` records why no declared-length ceiling is asserted on `parse_request_head`.

**Gate history.** The first ladder after the code change and `cargo fmt` passed `fmt`, `clippy`, `tests` and `evidence`; `cite` failed with exactly nine findings (all parser/fuzz coordinates), `counts` with 27 stale figures, `govtests` re-reported the same documents, and `validate` kept its four pinned findings. After the migrations above: `cite` reports 0 findings, `counts` reports 0 findings over six documents, and `govtests` passes; `fmt`, `clippy`, `tests` and `evidence` stay green, and `validate` exits 1 with exactly its four pinned findings.

**Refused alternatives.**
- Keep the refusal in the parser and pass the route's ceiling into it - impossible: the head must parse before the route is resolved, and re-parsing the buffered form to check a second time would double that work for the common case. Passing "no ceiling" to the streaming parse and letting the route enforce its own is the same decision taken in the only place that knows it.
- Remove the declared-length check from the head for both entry points - refused: a buffered request must be refused before a body byte is read, and track 70's 413 plus the parser property tests pin that.
- Raise the buffered ceiling whenever a streaming route exists - refused: one global field would silently raise the memory price of every request the server buffers.
- Let `awaits_100_continue` keep the buffered ceiling - refused: the interim answer is owed to a paused client whose route may allow the length, it is sent before dispatch, and the h2 frontend never gates it on the buffered ceiling either.

**What this track does not claim.** No performance number: the change adds one private parameter and one comparison, with no allocation or new state. A declared length over `max_stream_body_bytes` is still `413` before dispatch, and a chunked body over it still fails mid-stream; every buffered route is still bounded by `max_body_bytes`. The run is Windows-only; PROB-0012 still blocks a cross-platform claim.

## Track 143 - release 0.2.0: by-value builders and one private header store (2026-11-06)

**What was open.** The two ledger items the 0.2 plan reserved for one bounded breaking pass. PROB-0011: every registration and configuration method on `AetherApp` took `&mut self` and returned `&mut Self`, so the chained form the README teaches could not be written at all and the normal failure mode of the builder API was a half-converted call site. PROB-0005: a request carried two header representations - the parsed lines the framework decided on and a public map a middleware could rewrite - so a handler could read a different field than the check above it had acted on, and the HTTP/1 and HTTP/2 frontends had to keep two shapes in agreement.

**The fix.** Thirty-eight methods now take `self` and return `Self`: `use_middleware`, `manage`, `enable_cors`, `enable_security_headers`, `enable_security_headers_with`, `enable_compression`, `enable_host_validation`, `enable_rate_limit`, `enable_csrf`, `enable_trusted_proxies`, `enable_request_logging`, `get`, `get_async`, `post`, `post_async`, `put`, `put_async`, `delete`, `delete_async`, `patch`, `patch_async`, `route_sync`, `route_sync_with`, `route_async`, `route_async_with`, `route_stream`, `ws`, `ws_subprotocols`, `ws_with`, `group`, `mount`, `static_asset`, `static_file`, `static_dir`, `static_dir_with_cache`, `static_dir_with_options`, `with_scheduling`, `with_scheduling_resources`. The ten that already consumed `self` - `tls`, `tls_from_files`, `tls_from_pem`, `require_tls`, `with_websocket_limits`, `enable_h2c`, `run`, `listen`, `bind`, `serve` - are unchanged, and `RouteGroup` and `Router` keep `&mut self`, so a configure closure is unchanged.

The request now holds one private store: `header_multi: HashMap<String, HeaderLines>` (`framework/aether/src/app.rs:410`), with three read-only accessors - `header` (`:671`, the first field line for a name), `headers` (`:693`, the enumeration, first line per name) and `header_values` (`:709`, every line in wire order) - and the private `lines_for` (`:717`) that lowercases only a mixed-case lookup. There is no public field and no mutable accessor; a middleware that must pass a decision to the handler uses `extensions_mut`, which no framework check consults. Both frontends build that one store: the HTTP/2 header conversion in `framework/aether/src/http2.rs` was rewritten over it, which is where the 44 removed lines in that file come from.

The version is `0.2.0` in `framework/Cargo.toml:24` (`[workspace.package]`), and all 16 members inherit it with `version.workspace = true`, so the release is one number. `docs/11_migration_0_1_to_0_2.md` is the migration note: the 38-method list, a chain and a rebind example, the 0.1 statement form that now fails with `E0382`, the header-store mapping table, and `my_cool_site` named as the consumer that was compiled against the new surface. The root `docs/README.md` lists it as item 14. Every teaching page and example that wrote the 0.1 statement form was converted: `docs/00` (ar and en), `docs/02`, `docs/05`, `docs/06`, `docs/07`, `docs/08`, `docs/09`, `framework/README.md`, `README.md`, and in `framework/docs/` the capabilities page, baseline, deployment, Arabic error guide, full documentation, getting-started and readiness report.

**Test.** The header store's own pin is a doctest: `compile_fail,E0609` on `headers` (`app.rs:685`) writes `ctx.headers.insert(...)` and fails because no such field exists, so a public field or a mutable map cannot be reintroduced without turning that doctest red. A `no_run` example was added on `tls_from_files` (`:1882`) because the by-value form changed what it returns before `?`. The two renamed tests are the behavioural pair: `repeated_lines_share_one_lowercase_key` (`http2.rs:1474`) asserts that `X-Marker` and `x-marker` land under one key with both lines in wire order, and `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` (`framework/tests/tests/http_e2e.rs:4153`) counts the fields a middleware can see with `req.headers()` while showing that the only power it has over a handshake is to answer instead of calling the chain. No suite count moved outside the doc lane: the tests this slice rewrote replaced existing entries.

**Sensitivity.** This slice's falsifier is the compiler, not a runtime mutant. The 0.1 statement form (`app.get(...); app.listen(...)`) is a `compile_fail` block in the migration note, and every converted call site is a compile-time proof that the method consumes `self`; a method reverted to `&mut self` stops the tree compiling at the first chained call. The header field's `compile_fail,E0609` doctest is the same shape: adding `pub headers` makes the doctest compile and the doctest suite reports a failure. I did not run injection mutants this track, and the record does not claim any.

**Census and documents.** The suite is **946 passed / 0 failed / 1 ignored** over **64** `test result:` lines - unit **495** over 19 unit runs, integration **430** across 30 targets, doc **21** over 15 doc runs - with the 64 `running N tests` lines summing to **947** = 946 plus the single ignored test. The doc lane grew 19 -> 21 in one crate (`aether` 15 -> 17) and its run count went 14 -> 15 because the crate's `compile_fail` example is compiled as a second doctest binary. That second binary exposed a blind spot in the count gate: cargo prints it under the same `Doc-tests` marker with no new runner line, so `census_from_log` saw one unplaced summary and refused the log. It now inherits the doc lane after a `running N tests` header, and `test_the_compile_fail_binary_joins_its_doc_lane` pins the rule with a synthetic log (`(doc, doc_runs) == (4, 2)`, `unplaced == 0`, `(result_lines, running) == (6, 22)`). The citation gate reads 112 citations in 16 live documents over 77 distinct spans with 0 findings; the name pass resolves 391 qualified and 1449 bare names over 28 live documents, 36 sheltered by `NAME_EXCEPTIONS` (30 in the table), 10 attributed to another crate and 1 exempted proposal. The six pinned documents carry the same figures, including a new forty-second reading in the readiness report; the forty-first was demoted to "in that lane" and "the one ignored test" so each field pin names exactly one live sentence.

**Coordinates and pins.** The by-value pass and the store rewrite moved 35 citations. Twenty-three pins were renamed by the re-anchor script; four were resolved by reading the code - the parse-error `Connection: close` 2980 -> 3000, the `RequestContext` span 394-438 -> 394-428, and the two `write_http1_to` results 4243 -> 4256 and 4292 -> 4305 - and the `enable_csrf` pin moved 1120 -> 1130 with its regex rewritten to `pub fn enable_csrf(mut self, csrf: CsrfTokens) -> Self {`, because the old pattern named the removed signature. Two `NAME_EXCEPTIONS` were added: the renamed `header_keys_are_lowercased_and_the_first_line_wins` in the readiness report, which is history and must keep its old name, and `my_cool_site` in the migration note, which is a path outside this workspace.

**Gate history.** The first ladder after the code, format and document edits passed `fmt`, `clippy`, `cite` and `evidence` and failed `tests`: `link.exe` exited 1285 with `corrupt PDB file 'target\debug\deps\http2_e2e-32211196755fe44a.pdb'`. The failed run's `tests.log` held no result lines, so `counts` reported 107 stale figures against it and `govtests` failed on the same comparison; that is recorded here as it happened, not as a code failure. Removing that one corrupt build artifact and re-running produced no failure at link or in any test. The final ladder is all eight green - `fmt`, `clippy`, `tests`, `cite`, `evidence`, `counts`, `govtests` - with `validate` exiting 1 on exactly its four pinned findings.

**Consumers.** `my_cool_site` (`C:\Users\hamza\Desktop\my_cool_site`) has its 27 statement-form call sites rebound to `app = app.method(...)` and `cargo check` exits 0 against 0.2.0, including the lockfile update to `version = "0.2.0"`. That repository is separate: nothing was committed there, and none of its files are part of this change set.

**Refused alternatives.**
- Keep `&mut self` and add a second, chainable facade - refused: two surfaces to keep in agreement, and PROB-0011 asked for the one the README already teaches.
- Take `&mut self` but return `Self` by cloning - refused: a silent copy is the failure this pass exists to remove, and the memory and state duplication is unpriced.
- Expose `headers_mut()` or keep the public map behind a deprecation - refused: that is PROB-0005's split reopened; a guard that must change the request now has `extensions_mut`, which no framework check reads.
- Bump only the facade crate - impossible as stated: the members inherit `version.workspace`, and a second number would be a second release identity.
- Keep the migration note out of the live document set - refused: a reader upgrading from 0.1 needs the table the gate can check, and its method list is the only complete statement of the break.

**What this track does not claim.** No performance number: the by-value change moves an ownership token and the header store removes a map, and neither was benchmarked. The header store's memory price is unchanged in kind - one map of lines per request - and the release is not claimed faster. The run is Windows-only; PROB-0012 still blocks a cross-platform claim. The `my_cool_site` result is a compile check, not a run. The 38-method list is the complete set for this pass only; methods outside `AetherApp` did not change, and the enumeration of the changed signatures was cross-checked name by name because the first extractor pass missed `get` where a `RequestContext::get` signature collided with it.
## Track 144 - the 0.2 header-store pass left four live documents teaching the removed public map (2026-11-06)

**What was open.** Found by the post-release audit of the live documents, not by a failing gate. Track 143 removed `RequestContext`'s public header map and kept one private store, and corrected the matrix and the new migration note - but four live documents still described the 0.1 API. `docs/05_request_context_and_memory.md` printed `pub headers: HashMap<String, String>,` in a struct excerpt it labels as `app.rs:394-428`, said a public `headers` map is built from the same loop, and - separately - omitted the private `request_serial` and `scoped_state` fields the excerpt claims to quote. `PRODUCTION_READINESS_REPORT.md` §4.11 said the public map "holds one entry per field ... keyed by the casing the field first arrived with". `FULL_FRAMEWORK_DOCUMENTATION_AR.md` said a middleware cannot revoke an upgrade by editing `RequestContext::headers` because the validator reads the private store, and that merging the two stores was still the owner's open decision (PROB-0005). `framework/README.md` still called the `header()`/`headers` behaviour unchanged legacy, and the readiness report's public-API compatibility section still said no public signature was removed and described the `headers` map as keyed by wire casing. None of the four carries a snapshot marker, so each is checked as a current statement.

**The fix.** Documentation only; no code, test or pin changed. `docs/05`'s excerpt now matches `app.rs:394-428` field for field (including `request_serial` and `scoped_state`, which the earlier excerpt had silently dropped) and its prose names the one store and the three read-only surfaces, with `extensions_mut()` as the way a middleware passes a decision to the handler. The readiness report's bullet now states the lowercase key, the wire order and the accessor surface. The full documentation's upgrade paragraph now says 0.2 removed the field, names the `compile_fail,E0609` block as the compile-time pin and `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade` as the wire pin, and no longer calls PROB-0005 open. The framework README names the three accessors instead of a legacy behaviour, and the compatibility section records the two deliberate 0.2 breaks next to the 0.1 additions it already listed. Two nearby matches were verified and left alone: `docs/05`'s response-writer excerpt iterates `Response::headers`, a different struct's public field, and `docs/04`'s `req.headers.iter()` is `httparse::Request::headers` inside the parser at `parser.rs:389`.

**Test.** No test was added or changed, because the rule this track restores is already pinned twice: the `compile_fail,E0609` doctest fails if a public field returns, and the wire test asserts the chain sees the request's lines and can only answer. The falsifiable claim is therefore the corpus census, not a mutant: the suite was expected to hold exactly and did - 946 passed / 0 failed / 1 ignored over 64 `test result:` lines, cross-checked by 947 = 946 plus the ignored test - while the name pass moved as soon as the prose changed and would have failed the counts gate had the sentences not matched the tree.

**Census.** Unit 495 over 19 unit runs, integration 430 across 30 targets, doc 21 over 15 doc runs. Cite: 112 citations in 16 live documents over 77 distinct spans, 0 findings. Name pass over 28 live documents: 390 qualified and 1455 bare names, 36 sheltered by 30 per-document reasons, 10 attributed to another crate, 1 design document exempted, 0 findings. Against track 143 that is qualified 391 -> 390 (the qualified `RequestContext::headers` token is gone) and bare 1449 -> 1455 (the rewrites name `header_values`, `headers`, `extensions_mut` and the wire test). Govtests: 173 tests OK.

**Documents and pins.** The counts gate's two name sentences were re-derived: the matrix's documentation row and `OWNER_VISION.md` now read 390 / 1453, and both tree-name phrases moved to track 144. No source coordinate and no citation pin moved, because no cited line changed; the citation gate still reads 112 / 77 / 0. Separately, the roadmap's §19.7 table had rows 140 and 141 concatenated on one line with a doubled pipe; it was split into two rows without changing either cell.

**Gate history.** The first ladder after the `docs/05` edit passed `fmt`, `clippy`, `tests`, `cite` and `evidence`, while `counts` reported two findings and `govtests` failed on the same two: bare printed 1449, measured 1450. After the readiness-report and full-documentation edits, `cite` re-ran clean at 0 findings but the name pass read 390 / 1453, so `counts` reported four findings. A third round followed the README and compatibility edits: `cite` stayed clean, `counts` reported two findings (1453 printed, 1455 measured), and after the same two sentences were re-derived the full ladder is green - `fmt`, `clippy`, `tests`, `cite`, `evidence`, `counts`, `govtests` - with `validate` exiting 1 on exactly its four pinned findings.

**Refused alternatives.**
- Leave the three passages as history - refused: none is a dated snapshot; a reader following the request-context page would write `ctx.headers.insert(...)`, which no longer compiles, and the full documentation would send them looking for an owner decision that is closed.
- Rewrite the dated 2026-10-20 entries in `.project-governance/WORKING_CONTEXT.md` that still describe two stores - refused: that file is a log outside the gate corpus, and rewriting a record makes a different record; PROB-0005's closure is stated in the register instead.
- Restore a read-only public map so the old examples compile - refused: a public map is the two-view hazard PROB-0005 closed, and a read-only clone would still be a second copy.

**What this track does not claim.** No code, test, performance or cross-platform claim. The audit covers the token patterns actually searched - `headers` field access, public-map prose, `-> &mut Self`, 0.1 version strings and consecutive `app.method(...)` statements - so a false prose claim containing none of those tokens would still be live.
