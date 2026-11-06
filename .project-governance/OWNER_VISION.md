I am working on Aether, a Rust web framework in the repository I have access to. Your job is to bring it to a production-ready state, one that can compete with mature frameworks on feature completeness, performance, resource use, security, and developer experience. Do not stop at a plan: inspect the code, make the changes, run the tests, and measure the results.

Work in this order, and do not claim any item is done without evidence from the code, tests, or measurements:

1. **Audit the repository first.** Read the source, dependencies, feature flags, build configuration, tests, and examples. Treat the implementation as the source of truth and verify every documented capability against it.

2. **Build a feature-completeness matrix.** Compare Aether against Axum and Actix Web (and Hyper where relevant) across HTTP/1.1 handling, routing (path parameters, wildcards, correct 404 and 405 with `Allow` headers), request extraction (path, query, header, cookie, JSON body with configurable limits), shared state, error handling, middleware ordering, CORS, security headers, timeouts, graceful shutdown, compression, static files with path-traversal protection, and testing utilities. Mark each item `VERIFIED_IMPLEMENTED`, `PARTIALLY_IMPLEMENTED`, `NOT_IMPLEMENTED`, `BROKEN`, `UNVERIFIED`, or `NOT_APPLICABLE`. Do not mark anything implemented because a type or config field exists; verify the full behavior. Prioritize by practical value, security, and engineering cost, and do not copy features blindly. Ensure HTTP/2 and HTTP/3 support is claimed only if it actually works.

3. **Establish baselines before changing anything.** Measure requests per second, p50/p95/p99 latency, CPU time per request, peak memory, allocation counts, startup time, build time, and release binary size. Use Criterion, a flamegraph or allocation profiler, and a reproducible load tool. Separate framework overhead from application logic, the network, and the load client.

4. **Fix correctness and security first.** Audit request parsing against RFC 9112, especially conflicting `Content-Length` and `Transfer-Encoding` handling and request smuggling. Check header validation, parser limits, overflow in body-size enforcement, malformed input, slow-client and resource-exhaustion behavior, cancellation, disconnects, and cleanup. Add negative and fuzz tests for each of these. Never trade a security guarantee for speed.

5. **Implement the highest-priority missing features,** incrementally and at the correct architectural layer. For each one: explain its purpose, check how mature frameworks provide it, design the smallest sound implementation that fits Aether's architecture, add unit, integration, and negative tests, update documentation and examples, and confirm it works in a real sample application. Preserve all existing public APIs. If a breaking change is unavoidable, document it and test it first.

6. **Optimize performance based on measurements.** Review the full request path: I/O, parsing, routing (consider radix tries or compressed routing structures only if benchmarks justify them), middleware, handler dispatch, serialization, buffer reuse, backpressure, and lock contention. Do not assume that zero-copy or fewer allocations improves end-to-end results; confirm each change with before-and-after measurements across several input sizes and concurrency levels. Keep a safe fallback for any replacement until it is validated. If a new algorithm seems necessary, define the bottleneck, formalize it, compare against the strongest existing options, and test correctness and adversarial inputs before integrating it. Record failed experiments.

7. **Reduce the release executable size,** which may be near 2 GB. First find out what it contains: the framework, a consuming application, bundled assets, debug symbols, duplicated monomorphized code, or dependencies with unneeded features. Check the dependency tree and enabled features, then consider profile and LTO settings, symbol stripping, separating debug info, and externalizing large static assets. Do not remove framework features to shrink the binary. Measure the baseline and the result, and verify the final artifact runs correctly.

8. **Benchmark fairly against reference frameworks** on equivalent applications with the same hardware, runtime settings, and documented versions. Cover plaintext and JSON responses, routing across different table sizes, middleware-enabled apps, persistent connections, varied body sizes, streaming, malformed requests, and sustained high load. Repeat runs, report variance, and state both strengths and weaknesses. Do not claim universal superiority.

9. **Run the full test suite and build,** including unit, integration, security, regression, and stress tests where relevant. Report failures and limitations honestly.

10. **Finish with a final audit** of the complete diff, public API compatibility, dependency changes, security implications, and whether the documentation matches the code.

Deliver a final report containing: the feature-completeness matrix; a list of changed files with a short description of each change; before-and-after benchmark results with environment details; memory and CPU measurements; before-and-after executable sizes and their causes; the exact test commands and their actual results; security and compatibility findings; rejected optimizations and failed experiments; and remaining work with recommended priorities. If any step is blocked by the environment, say so explicitly and continue with what can be verified.



قبل كل شيء لازم نصلح مشاكل الفريم وروك بحيث يكون
بقوة actix او اقوة

نريد نطبق ذول:
You are a Principal Systems Architect obsessed with resource efficiency,
BUT you never sacrifice a feature that every web framework must have.

You are NOT a feature builder who adds random features.
You are NOT a minimalist who deletes required features to save RAM.
You are a resource governor who makes the SAME features use less RAM, less disk, less CPU, faster response.

Rule: Feature parity first, efficiency second. 
If a feature exists in Express, Next.js, Axum - it must exist here too, but with better resource usage.

Cross-Platform Rule: The framework MUST run natively and smoothly across Windows and Linux (absolute priority). macOS support is highly desired, but Windows and Linux parity is non-negotiable.

Absolute Scale & Code Volume Mandate:
No shortcuts. No stubs. No compromised implementations. If achieving absolute feature completeness across all necessary web networking protocols requires writing 20,000 lines of code—or refactoring/deleting 20,000 lines of code—you will do it without hesitation. The only metric that matters is building an uncompromising, full-stack, hyper-performance technical dragon.

---

### Objective: Architecting "Aether" as the Ultimate, Production-Ready, Multi-Protocol Enterprise Web Engine

We are building "Aether" to be an elite, multi-protocol web framework that outperforms standard industry solutions while maintaining 100% safety, zero-allocation memory models, and seamless cross-platform execution (Linux via epoll/io_uring, Windows via IOCP, and macOS via kqueue). 

The developer must implement the following core subsystems with absolute precision and production-grade robustness:

---

### 1. Multi-Protocol Universal Engine (HTTP/1.1, HTTP/2, & WebSockets)
- **Unified Multiplexing Layer:** Implement a unified connection state machine capable of handling traditional HTTP/1.1 (with pipeline/keep-alive draining), HTTP/2 (multiplexed streams, HPACK header compression, flow control), and bi-directional WebSockets on the exact same port/listener.
- **Zero-Downtime Upgrade Paths:** Seamless protocol negotiation (ALPN / Upgrade headers) shifting raw TCP streams instantly into active WebSocket frames or HTTP/2 framing without dropping connection contexts or leaking socket descriptors.

---

### 2. High-Performance Zero-Copy & SIMD Parsing Pipeline
- **Raw Byte Processing:** All incoming HTTP headers, request lines, paths, and query strings must be parsed directly from the raw TCP receive buffer as zero-copy byte slices (&[u8]). No intermediate heap allocations, no dynamic String conversions, and no HashMap overhead during the hot path.
- **SIMD-Vectorized Scanning:** Implement SIMD-accelerated byte-scanning routines (using Rust-native intrinsics or secure wrappers) to scan delimiter blocks, chunk boundaries, and line endings (\r\n) in parallel across 32-64 byte chunks per CPU cycle.

---

### 3. The "Dragon" Memory Subsystem: Custom Pure-Rust Arena & Thread-Local Allocators
- **Eradicating Global Heap Contention:** Completely bypass standard global allocator bottlenecks (`malloc`/`free`) on hot paths by implementing a custom thread-local Arena / Bump Allocator (or leveraging `bumpalo` pools per worker thread).
- **Contiguous Lifespan Management:** Allocate a fixed, contiguous block of memory for each connection worker pool. Request lifespans advance internal pointers via fast arithmetic. Upon request/connection termination, the entire arena pointer resets back to baseline instantly, guaranteeing absolute 0% memory fragmentation and strict zero memory leaks over multi-day soak tests.

---

### 4. O(1) Bitwise Routing & High-Throughput Hash-Based Dispatch
- **Fast Hashing:** Replace slow default hash functions with non-cryptographic, collision-resistant, ultra-fast hash algorithms (e.g., FxHash, wyhash, or ahash) for internal URI/Route lookup tables.
- **Bitmask State Engines:** Represent HTTP methods, header flags, permission layers, and middleware guards as packed Bitmasks. Route evaluation, guard checks, and dispatching must complete within 1-2 CPU clock cycles using pure bitwise operations (AND, OR, XOR).

---

### 5. Enterprise Resilience, Backpressure, and Security Hardening
- **Defensive Networking (Slowloris & Flood Mitigation):** Enforce strict, non-blocking timeouts on read/write phases, maximum header byte limits, and frame size caps. Unresponsive or stalling connections are forcefully dropped.
- **Orphan-Free Task Scheduling:** Ensure that all asynchronous worker tasks, stream handlers, and queued jobs are fully cancel-safe (`Drop` guard pattern enforced). Aborted requests must instantly release locks, close file descriptors, and clean up queues with zero orphan tasks left behind.
- **Cross-Platform Parity:** Core event loops must abstract system calls via `mio`/`Tokio` primitives, translating natively to `epoll`/`io_uring` on Linux, `IOCP` on Windows, and `kqueue` on macOS, with zero performance degradation or platform-specific compilation failures.

---

### 6. Mandatory Multi-Agent Architectural Analysis & Execution Directive
Before writing any code or modifying the core implementation, you must convene an internal panel of expert systems and architecture agents to analyze this entire specification:
1. **Feasibility Analysis:** Critically evaluate whether all specified requirements can be implemented robustly in safe Rust without compromising cross-platform stability (Windows/Linux).
2. **Optimization & Smarter Alternatives:** Determine if direct implementation of any point will yield sub-optimal results, and engineer smarter, more efficient, or cleaner alternatives where applicable.
3. **Execution Decision:** 
   - If a requirement is optimal as-is, execute it precisely.
   - If a smarter/better approach exists, implement the superior approach.
   - If any requirement introduces unmitigated risks or architectural flaws, formulate an intelligent modification or bypass strategy.
4. **Final Action:** Proceed with the absolute best, highly optimized, and thoroughly vetted implementation plan.

---

## Research vision (owner-supplied 2026-10-24, verbatim)

Appended on 2026-10-24 exactly as the owner delivered it. It governs how improvement work is
discovered, validated and recorded; it does not delete or soften any rule above. Two of its lines
touch standing constraints that the owner had already fixed by hand (who runs a measurement, and
whether an independent reviewer is required) — recorded in `WORKING_CONTEXT.md` rather than
reconciled silently here.

You are an algorithm research and engineering system. Your job is to find real improvement opportunities in the codebase I give you, design candidate solutions, and validate them through rigorous experiments. Treat an idea as a genuine improvement only when experiments show it achieves its intended goal without compromising correctness, reliability, security, or other essential properties. Use reasoning (deduction, induction, first-principles analysis, attempts to falsify your own hypotheses) to guide discovery, but let empirical evidence decide.

The source code, tests, and measurements I provide are the basis for your work. If something I need is missing, design the experiment to uncover it rather than assuming it. Never fabricate code details, measurements, test results, or conclusions.

Work through the following in order.

**1. Understand the existing system.** Inspect the relevant code and execution paths. Identify the specific problem, bottleneck, or limitation, and verify its root cause rather than optimizing a component on intuition. Separate verified facts from assumptions. Establish a baseline using existing tests and measurements, and note compatibility, security, resource, and maintainability constraints.

**2. Research existing solutions.** Before claiming novelty, examine established algorithms, data structures, and alternative approaches, including their assumptions, complexity, and limitations under the target conditions. Prefer a proven solution when it adequately solves the problem. If novelty cannot be established, label the proposal a potential improvement or an unverified innovation.

**3. Generate and evaluate hypotheses.** Propose several distinct candidates: optimizing the existing algorithm, adapting an established one, changing the architecture, combining techniques, or designing something new. For each, state the suspected mechanism, assumptions, expected time and space complexity, expected benefits and trade-offs, edge cases, and failure modes. Select the most promising candidate on technical merit, not novelty or complexity. If an existing solution is already adequate, say so.

**4. Design a reproducible experiment before changing code.** Define the hypothesis, the baseline, the proposed alternative, primary and secondary metrics, correctness and compatibility requirements, normal, boundary, and stress scenarios, expected failure modes, and explicit acceptance and rejection criteria. Set success criteria before observing results.

**5. Implement in isolation.** Preserve the stable implementation, work in an experimental branch or workspace, and make the smallest change that tests the hypothesis. Run correctness tests before performance tests, then integration, regression, and stress tests as appropriate. Never weaken tests or remove correctness requirements to make an experiment pass, and never replace the stable implementation automatically. If execution is unavailable, provide the implementation and a concrete validation plan, and state clearly what remains untested.

**6. Compare fairly.** Run the candidate and baseline under equivalent conditions. Measure correctness and behavioral equivalence, throughput, latency (P50, P95, P99 where relevant), memory and allocation counts, CPU utilization, fairness and starvation where relevant, stability across workloads, and complexity and maintainability. Repeat runs, control the environment, account for noise, and apply statistical analysis when data allows. Never declare success because one metric improved while essential metrics deteriorated.

**7. Iterate based on results.** If the candidate succeeds, verify reproducibility, check for regressions and side effects, and retain it as an acceptance candidate. If the result is inconclusive, investigate noise and whether the targeted component is truly the bottleneck, then refine or gather more evidence. If it fails, identify the likely cause, revert the change, record the lesson, and revise a specific part of the hypothesis rather than repeating the same experiment. Start with two or three justified attempts per hypothesis, and go beyond that only for a concrete technical reason.

**8. Keep an auditable log.** For each experiment, record its identifier and hypothesis, files changed, environment and test conditions, baseline and candidate measurements, variability and limitations, tests passed and failed, the decision with supporting evidence, and lessons for future work. Do not generalize beyond the conditions actually tested.

**9. Apply the acceptance gate.** A candidate is accepted only if correctness is preserved, required tests pass, the improvement is reproducible (or any trade-off is explicitly justified), resource, security, and compatibility constraints hold, no unacceptable regression appears in essential metrics, the change is reviewable and reversible, and project governance is satisfied. Classify inconclusive results as inconclusive. Use independent tests, automated checks, and code review wherever practical, since you should not be the sole authority approving your own proposal.

**10. Respect autonomy and safety limits.** Work autonomously within the scope I authorize for analysis, hypothesis design, and safe experiments. Do not publish, merge into the main branch, modify production systems, or delete important data without explicit authorization. Clearly distinguish proposed ideas, justified hypotheses, experimental implementations, measured results, demonstrated improvements within a defined scope, and novelty claims that still need independent verification.

**11. Report.** For each research cycle, report: the problem and the evidence for its significance; the existing solutions and alternatives; the hypotheses considered and why one was selected; the proposed design and mechanism; the actual changes made; tests, measurements, and reproducible results; comparison with baseline and statistical limitations; failure modes and regressions found; the decision (retain as candidate, reject, or inconclusive); and the next most valuable action the evidence supports. Keep completed work separate from proposed work, and measured results separate from estimates.

The aim is not to prove that an idea is correct, but to discover whether it is correct, when it helps, when it fails, and what evidence justifies keeping it. Systematically test, and retain only the improvements that deliver measurable, reliable, and reproducible value.

### Owner rulings on the three clauses this vision left open (2026-10-24)

Asked in the same session the text was supplied; these are owner answers, not agent readings.

1. **§6 "run the candidate and baseline" — the owner measures.** The agent designs the experiment,
   hands over the exact command and tool, and the owner runs the throughput/latency/CPU/RAM
   measurement. No performance number enters a governance record or a document unless it came from
   an owner-run measurement. This keeps the earlier standing rule ("I test, you don't") in force;
   the vision adds rigor to the design of the experiment, not ownership of the measurement.
2. **§9 "not the sole authority" — an independent reviewer per cycle.** Every research cycle is
   reviewed by an independent agent before a candidate is retained, rejected or called inconclusive,
   and the record states whether that review actually ran. A cycle whose review did not run says so.
3. **§5 "work in an experimental branch or workspace" — the workspace is `C:/tmp`.** Experiments are
   built on an exported tree under `C:/tmp` (the shape already used by `C:/tmp/t*/` scratch crates),
   not on a git branch. No commit, no push, no history rewrite and no merge into `main` without the
   owner's explicit approval, which still follows the report of results.

### Where the protocol currently stands (agent record, 2026-10-24)

Appended after the owner's text, which is unchanged above. One line per ruling, so the next cycle can
see whether the process is actually in force rather than only written down:

- **§6 / ruling 1.** Every performance number recorded since is either an instrument reading taken here
  (allocations/req from the counting harness) or absent; no throughput, latency, CPU or RAM figure has
  entered a record without an owner run. The experiment designs and their exact commands are handed
  over with each report.
- **§9 / ruling 2.** Tracks 43–46 each state in `WORKING_CONTEXT.md` whether an independent review ran.
  Track 48 is the first cycle where it did: a fresh-context reviewer agent rebuilt the bench binary,
  re-ran the four gates and the allocation probe from zero, and reproduced 31.00 allocs/req and
  2595 B/req. Its verdict was **RETAIN**, so G15 (track 46) is now recorded as *retained* — with the
  residual risk it named (the retry's accept branch has no end-to-end test) kept open in the register.
- **§5 / ruling 3.** No branch, no exported-tree rebase and no commit: the tree carries the track-45/46
  work uncommitted, probes live under `C:/tmp`, and the counted measurements run from the sibling
  bench repo whose dependency is a path to this working tree.

### Where the vision currently stands (agent record, 2026-11-04)

Appended by the agent, after the owner's text above (unchanged) and after the 2026-10-24 record
(unchanged). It is a hand-off, not a claim of completion: it says what the tree does, which of the
subsystem demands below are still not met, and which of those is blocked on the owner rather than on
an idea. Every number here was re-derived on 2026-11-04 from this working tree and the last full gate
run, not copied from another document.

**The six subsystems, read against the code.**

1. *Multi-protocol universal engine* — **delivered.** One listener decides the protocol per
   connection rather than per socket: `transport::classify` peeks the first bytes and tells a TLS
   record header and the 24-byte HTTP/2 preface apart, ALPN offers `h2` before `http/1.1`, and both
   frontends hand the same `ServiceCore` the same request. HTTP/1.1 (pipeline and keep-alive drain),
   HTTP/2 over cleartext prior knowledge and over TLS, and WebSockets live on one port. What was
   declined is declined on RFC grounds and named in the matrix, not overlooked: `Upgrade: h2c`
   (RFC 9113 §3.1 deprecates it and prior-knowledge h2c serves the clients that need it) and server
   push (RFC 9113 §8.2 and §10.8 deprecate it).
2. *Zero-copy and SIMD parsing* — **the zero-copy half is delivered; the SIMD half does not exist.**
   `RequestView` borrows `&'req str` and `&'req [u8]` straight out of the connection buffer, its
   header slots start on the stack, and framing values stay bytes until something decides what they
   mean. But `core::arch` has no hit in any `.rs` file in this tree: there is no intrinsic anywhere.
   `validation/src/simd.rs` is a 64-bit word-at-a-time ASCII scan and its own header says the module
   name is historical. The matrix prices the row `PARTIALLY_IMPLEMENTED`, and track 113 spent itself
   deleting four documents that claimed processor instructions.
3. *Custom arena and thread-local allocation* — **not on the served path.** `aether_core::RequestArena`
   exists and is unit-tested, and `aether_runtime::handle_connection` builds one per connection — but
   the facade exports that worker and only its own integration target drives it; `AetherApp`'s serving
   loop never calls it. The request path allocates against the system allocator, and no
   `#[global_allocator]` is declared in the framework (the repository's only occurrence is a bench
   artefact under `framework/docs/perf-baseline/`). So vision §3's "bypass malloc on the hot path" is
   still a wish, and the copy the parser avoids is paid again when `RequestContext` owns the header
   field lines — which is PROB-0005, open because fixing it breaks a public field.
4. *O(1) bitwise routing and fast hashing* — **the bitmask half is delivered, the hashing half is
   not.** The radix trie matches a method with a `u64` bitmask and derives `Allow` from that same
   mask, HEAD and OPTIONS folding included. No non-cryptographic hasher is a dependency anywhere in
   the workspace, so every internal map hashes with the standard library's default. The "1–2 CPU
   clock cycles" in vision §4 has never been measured here and no document claims it.
5. *Resilience, backpressure, security hardening* — **the largest delivered area, with two named
   holes.** Delivered and pinned on the wire rather than in prose: the four RFC 9112 request-target
   forms on both frontends, the `Content-Length` and `Transfer-Encoding` promises including the
   smuggling shapes, header and body budgets, `413`/`431` refusals that actually reach the client,
   per-connection memory priced by capacity, the receive deadline separated from the handler deadline,
   a handler cancelled when its client goes away, graceful shutdown (GOAWAY drain, WebSocket
   going-away close frame), offline CRL and OCSP enforcement, trusted-proxy identity, CSRF, bearer
   authentication, signed cookies over both frontends, the `Expect` contract including the `417`
   refusal (track 116), and a body that stops arriving answered `408` by the shorter of the per-read
   idle bound and what is left of `max_receive_time` (track 117; before it, measured on this tree on
   2026-11-04: EOF after 5.005 s with a 2 s budget and no body byte sent, 5.003 s with three of five
   bytes, and 5.002 s even at the default 30 s budget — the paused-body hole PROB-0034 registered and
   this track closed). The hole that remains is **platform**, not shape: all of it is verified on
   Windows only. PROB-0012 stays open because no Linux or macOS host exists on this machine, and the
   Cross-Platform Rule at the top of this file makes that a requirement, not a footnote.
6. *Multi-agent architectural analysis* — **in force as process, not as decoration.** Each research
   cycle records whether an independent reviewer actually ran; experiments are built under `C:/tmp`
   on an exported tree, per owner ruling 3; and no throughput, latency, CPU or RAM figure enters a
   record unless the owner measured it, per owner ruling 1. That is precisely why the optimisation
   half of this vision (steps 6–8, and the F-19/F-20 rows of roadmap §19.4) is paused rather than unsolved: what
   is missing is a measurement, not an idea.

**What the tree measures at, today.** The suite is **946 passed / 0 failed / 1 ignored** over 64
`test result:` lines, re-measured on the track-144 tree and cross-checked by a second rule (the
`running N tests` lines sum to 947, which is 946 plus the one ignored test). The completeness matrix
is **123 / 3 / 8 / 0 / 0 / 0 / 2 = 136 rows**, recounted by two independent parsers that agree on the
same line set. The citation gate reads 112 citations in 16 live documents, 77 distinct spans, 0
findings, and its name pass resolves 390 qualified and 1455 bare names. These figures are no
longer copied: the `counts` gate (tracks 119-121) reads every pinned suite, citation and status
count in six live documents against this run's `tests.log`, this run's `cite.log`, the evidence
log's document census, the manifest's own member list and a recount of the matrix's own
five-column rows; it stores no number itself, and fails on a missing or stale log,
a reworded sentence, or a crate or target the run measured that the documents do not name.
The gate ladder
(`bash gates.sh`) passes fmt, clippy, tests, cite, evidence, counts and govtests; `validate` exits 1 with
exactly the four findings it pins — the two recycled PROB ids, REQ-0001 allocated three times, and the
open-vs-history consequence of that — and the ladder records that as its designed state. The
behaviour commits of the last track are `24f9805` (the admitting lock), `de67910` (the
thread-local Date cache) and `f8ee1aa` (the concurrent-run guard); this record rides in the
commit that carries it, and the native-code policy note in `e2fd36a`. The
working tree is clean apart from two governance tool files left deliberately unstaged, whose diff is
line-ending normalisation and no content change.

**Closed on 2026-11-04 (track 116).** The `Expect` contract is delivered: an `Expect` the framework
cannot honour is refused with `417` from the head on both frontends, `Expect: 100-continue` answers a
latch-free interim for a streamed body and a latched one for a buffered body, HTTP/1.0 and unknown
expectations are refused, and the header-count refusal outranks the expectation refusal over h2. The
track's own probe then found a defect it refused to bundle — a buffered body that stops arriving is
closed with no `408` (PROB-0034) — and the four documents that taught the blanket promise were
narrowed to the three shapes that hold. Two items are recorded as refused rather than fixed: the
HTTP/1 `417` wire test does not assert that the error body is empty (the h2 one does), and
`to_diagnostic` is still uncalled workspace-wide.

**Closed on 2026-11-04 (track 117).** PROB-0034 is closed in the code: a buffered HTTP/1 request
that began arriving and then stopped is answered `408` + `Connection: close` when the shorter of
`read_timeout` and the remaining receive budget expires, from one helper called by both expiry
sites; an idle keep-alive socket still closes with no answer. Four wire tests pin the shapes, six
mutants were each killed by a named test, and two independent reviews (SHOULD-FIX, then PASS) are in
the register. What stays unverified is named there: delivery of the `408` over TLS, a residual RST
race in the close window, and 100 ms of slack on the late-byte test's lower bound.

**Closed on 2026-11-04 (track 118).** The developer-facing diagnostic no longer describes a
runtime failure as a startup one: `AetherError::to_diagnostic`'s `Io` arm names the 500/E5002 it
now is (5xx redaction, the error reporter, the file or socket the operation named), both head
budgets and the body budget name the knobs an app can actually set, and `Timeout` names the
handler deadline instead of a connection timeout. The status/code agreement is pinned as a
twelve-row cross-product rather than a comment, and the HTTP/1 `417` wire test now reads the
body it asserts empty. Eleven mutants were each killed by a named test, and two independent
reviews returned SHOULD-FIX; every item they raised was applied — the byte-budget-only head
advice, the non-actionable body knob, the vacuous empty-body assertion, and two citation pins
shifted by the new lines.

**Closed on 2026-11-04 (track 119).** A printed count is now a checked claim. `count_check.py`
pins the 31 sentences that state a suite or citation figure in the five live documents to the
run the ladder just performed, and stores none of the numbers itself: a track that adds tests
updates the prose and leaves the tool alone. It fails closed on a missing or stale log, an
unparsed citation summary, a sentence reworded or duplicated out of the pins, a result line no
runner can place, and a crate or target the run measured that no document names. Re-deriving
the five documents corrected the readiness report's stale 78/67 bullets and its "49 tests"
line, and found that the committed baseline's `aether` row summed to 96 while the crate measured
97 (`app.rs` holds 22 tests, not 21). Eighteen mutants were all caught on an exported tree, and
one independent review returned SHOULD-FIX with seven findings, every one applied. The ladder is
now eight gates.

**What remains, and to whom it belongs.**

- Owner-gated, cannot be advanced here: F-19 (request arena, zero-copy head, SIMD validation in the
  hot path — needs a profile that names the stage, and the owner measures), F-20 (thread-per-core
  placement — needs the worker sweep run, `framework/tools/workers_sweep.ps1` exists for exactly
  this), the release-shape decision that unblocks PROB-0005 and PROB-0011 (both are breaking API
  changes), PROB-0004 (the owner paused the scheduler comparison sweep), PROB-0012 (a
  Linux run of the suite — needs a machine or a CI runner), PRR item 9 and E1–E9 (the throughput gap
  vs Actix is still unattributed), permessage-deflate (deferred by the owner until the framework
  problems are closed and Aether beats Actix by a measured margin), HTTP/3 (a scope decision, not a
  patch — no QUIC engine exists in this workspace), and the confidentiality half of the *Cookies* row.
- Agent-side, next in line: the improvement register's open items, which the register and
  `TRACK_INDEX.md` enumerate — read those two, not this file, for the item-by-item list.
- Deliberately refused: handler macros, hot reload, a `Secret<T>` wrapper, `Upgrade: h2c`, server
  push, and per-request stderr logging for a refused upgrade. The reasons are in roadmap §19.6 and
  they are reasons, not oversights.

**How to pick this up.** `.project-governance/TRACK_INDEX.md` maps every track to where it is
recorded; `.project-governance/notes/2026-10-21-improvement-register.md` is the live register (newest
`## Track N` heading at the end); `.project-governance/notes/2026-10-26-remediation-roadmap.md` §19 is
the inventory of what is open and blocked; `WORKING_CONTEXT.md` is frozen history and should not be
appended to. `bash gates.sh` from the repository root is the verification ladder, and `validate`
exiting 1 with those four findings is green, not a regression.

## Principal directive (owner-supplied 2026-11-05, verbatim)

> You are a senior systems engineer working on Aether, a Rust web framework. Your job is to diagnose and fix its existing performance and quality problems, measure every change, and only then consider new features.
>
> The core objective: make Aether consistently outperform Actix under fair, reproducible, realistic benchmarks, without sacrificing stability, security, or correctness. Treat the benchmark gap as a real engineering problem, not something to hide behind new features.
>
> Work in this order of priority:
>
> 1. **Fix major problems.** Find the actual bottlenecks and defects: bugs, inefficient code paths, unnecessary allocations, lock contention, excess memory use, and architectural weaknesses. Profile first (for example with `cargo flamegraph`, `perf`, or allocation counters) and identify the hot path before changing anything.
> 2. **Maintain code quality.** Keep changes clean, readable, modular, and free of duplication or needless complexity. Prefer proven, well-understood solutions over clever ones.
> 3. **Security and reliability.** Keep safe defaults, robust error handling, correct behavior under load, and protection against common web vulnerabilities. Do not trade these away for speed.
> 4. **Performance.** Design benchmarks that are fair and reproducible: same hardware, same workload, same configuration for Aether and Actix, with the methodology documented. Report before-and-after numbers from the same environment, including variance across runs.
> 5. **Features and innovation.** Only after the above is addressed, propose new features or competitive ideas, and keep them out of the current change set.
>
> Rules you must follow:
> - Do not add features to make the framework look more advanced.
> - Do not hide, skip, or work around existing problems; fix the cause.
> - Do not optimize on assumption. Show the profile or measurement that justifies each change.
> - Do not claim a performance improvement without reproducible benchmark evidence.
> - Do not sacrifice code quality, security, or reliability for benchmark numbers.
> - Every change must include tests that cover it and must not introduce regressions. Run the existing test suite and any relevant benchmarks before and after.
>
> For each change you propose, provide: the problem and the evidence that identifies it, the fix, the tests added or updated, the benchmark result with methodology, and any trade-offs or risks. Keep each change small enough to review in isolation.
>
> Begin by auditing the codebase and benchmark setup to produce a ranked list of the highest-impact problems, with the measurements that support the ranking. Then work through them in that order.

**How this sits beside the earlier rulings.** The 2026-10-24 rulings are unchanged: the owner
measures throughput/latency/CPU/RAM, every research cycle keeps an independent reviewer, and
experiments stay isolated under `C:/tmp`. This directive adds the order of work, not a new
permission, and it does not relax the standing prohibitions (no push, no amend, no history
rewrite, no merge or out-of-scope change without approval).

**What it parks.** A feature proposal that was in flight when this text arrived — wiring
`aether-validation` into route declaration so a 400 body and an OpenAPI `requestBody` come from
one declaration — is parked under priority 5: it is not started, and it does not enter the
current change set. The same applies to the built-in latency-percentile idea. Both are recorded
as proposals, not work.

---

## نظام ضغط وفك ضغط HTTP وWebSocket — مؤجَّل بقرار المالك (2026-10-24)

> **قبل أي شيء: هذا النظام لا يُطبَّق الآن، ولا يُبدأ تصميمه أو تنفيذه أو قياسه.** كلام المالك حرفيًا:
> «ما يتم تطبيقو الا لما نحل كل مشاكل الفريمورك ويصير اداء الفريمورك اقوى من actix باضعاف».
>
> يعني **شرطان معًا**، وليس أحدهما:
> 1. **تُحَلّ كل مشاكل الفريمورك** المسجّلة — لا يبقى بند مفتوح في `problems.json` ولا في سجل التحسينات.
> 2. **يصبح أداء Aether أقوى من Actix بأضعاف** — بقياس يُشغّله المالك؛ لا رقم من وكيل، ولا تقدير، ولا
>    مضاعف يُثبَّت من طرفي.
>
> حتى يتحقق الشرطان: لا تصميم ضغط، ولا كود، ولا اعتماديات ضغط جديدة، ولا شريحة قياس لهذا الموضوع، ولا
> بند يُفتح باسمه في سجل التحسينات. النص الكامل الذي سلّمه المالك محفوظ بالأسفل **كما هو**، ويكون المرجع
> الوحيد عند بدء العمل بعد تحقق الشرطين.

# المطلوب: تصميم وتنفيذ نظام ضغط وفك ضغط متكامل في Aether Web Framework لدعم HTTP وWebSocket

أريد إضافة نظام ضغط وفك ضغط اختياري وآمن وقابل للتخصيص في Aether Web Framework، يدعم HTTP وWebSocket، بهدف تقليل حجم البيانات المنقولة وتحسين الأداء عند التعامل مع البيانات الكبيرة أو المتكررة، دون فرض تكلفة إضافية أو تغيير السلوك الحالي للتطبيقات التي لا تحتاج إلى هذه الميزة.

يجب أن يبدأ العمل بمراجعة بنية Aether الحالية، ثم تصميم النظام وتنفيذه واختباره تدريجيًا، مع المحافظة على توافق البروتوكولات وسلامة واجهات Aether الحالية.

## 1. ميزة اختيارية بالكامل

- يجب أن يستطيع المطوّر تفعيل ضغط HTTP الصادر وفك ضغط HTTP الوارد كلٌّ على حدة.
- يجب أن يستطيع المطوّر تفعيل ضغط رسائل WebSocket بصورة مستقلة عن HTTP.
- يجب أن تبقى جميع الميزات معطّلة افتراضيًا، ما لم تكشف مراجعة التصميم الحالي سببًا موثقًا لتوصية مختلفة.
- يجب ألا تُضاف تكلفة تشغيلية أو اعتماديات أو تخصيصات ذاكرة غير ضرورية للتطبيقات التي لا تفعّل الميزة.
- توفير إعدادات واضحة للتحكم في الخوارزميات والأحجام الدنيا وحدود الموارد ومستويات الضغط وسياسات المحتوى.
- الحفاظ على السلوك الحالي للتطبيقات وواجهات API والتوافق مع العملاء عند عدم تفعيل الضغط.

## 2. دعم ضغط HTTP وفك ضغطه

### ضغط الردود الصادرة

- ضغط البيانات التي يرسلها Aether إلى عملاء HTTP وفق تفاوض HTTP الصحيح، واختيار ترميز يقبله العميل.
- احترام ترويسات `Accept-Encoding` و`Content-Encoding` و`Vary: Accept-Encoding` عند انطباقها.
- التعامل الصحيح مع الترميزات المدعومة وغير المدعومة، ومع الردود التي تحتوي أصلًا على `Content-Encoding`.
- منع الضغط المزدوج للمحتوى نفسه.
- احترام دلالات HTTP، بما فيها الردود التي لا يسمح سياقها أو حالتها بوجود جسم، والتعامل الصحيح مع `Content-Length` والبث.
- عدم ضغط الردود التي لا تستفيد بصورة معقولة من الضغط.

### فك ضغط الطلبات الواردة

- دعم استقبال طلبات HTTP المضغوطة وفك ضغطها قبل وصول البيانات إلى معالجات التطبيق، وفق بنية قراءة الطلب الحالية في Aether.
- الفصل بين إعدادات ضغط الردود وفك ضغط الطلبات.
- التعامل الصحيح مع `Content-Encoding` المتعدد عندما يكون مدعومًا، ومع الترميزات غير المدعومة والبيانات التالفة.
- تطبيق الحدود الأمنية أثناء فك الضغط، وليس بعد استهلاك الموارد.
- عدم تجاوز حدود حجم الطلبات أو ضوابط الموارد الموجودة في Aether.
- توثيق سلوك الأخطاء والاستجابات المناسبة وفق متطلبات HTTP.

## 3. دعم ضغط رسائل WebSocket

- مراجعة تنفيذ WebSocket الحالي في Aether، بما في ذلك التفاوض أثناء المصافحة، والإطارات، والرسائل المجزأة، والإرسال والاستقبال والإلغاء.
- تنفيذ دعم `permessage-deflate` وفق مواصفات WebSocket، باعتباره نقطة البداية الأساسية للتوافق.
- التفاوض على الامتداد مع العميل بصورة صحيحة، وعدم تفعيل الضغط ما لم يتم الاتفاق عليه.
- ضغط الرسائل الصادرة وفك ضغط الرسائل الواردة مع الحفاظ على المحتوى الأصلي وترتيب الرسائل وسلوك البروتوكول.
- دعم الرسائل النصية والثنائية، والرسائل الفارغة والصغيرة والكبيرة، والرسائل المقسمة إلى عدة إطارات.
- الحفاظ على سلامة UTF-8 للرسائل النصية وعدم تغيير بيانات الرسائل الثنائية.
- منع فك الضغط أو تطبيقه مرتين على الرسالة نفسها.
- توفير إعداد مستقل لتفعيل ضغط WebSocket أو تعطيله، دون ربطه بتفعيل ضغط HTTP.
- التعامل الصحيح مع الامتدادات غير المدعومة، والمصافحات غير الصالحة، والرسائل التالفة، وانقطاع الاتصال أثناء المعالجة.
- عدم اعتبار gzip أو Zstandard أو Brotli امتدادات WebSocket مدعومة تلقائيًا لمجرد دعمها في HTTP؛ لا تُضف خوارزميات أخرى على مستوى البروتوكول إلا عبر آلية تفاوض متوافقة ومبررة.

## 4. تصميم طبقة ضغط مشتركة

قيّم إمكانية إنشاء طبقة داخلية مشتركة لإعادة استخدام المكونات المناسبة بين HTTP وWebSocket.

- إعادة استخدام مكتبات الضغط وفك الضغط والمكونات الأساسية عندما يكون ذلك مفيدًا هندسيًا.
- إبقاء تفاوض البروتوكول وتنسيق البيانات والترويسات والإطارات وقواعد الأمان الخاصة بكل بروتوكول مستقلة.
- تجنّب إنشاء طبقة تجريد عامة معقدة إذا لم تقدم فائدة واضحة.
- تجنّب إضافة اعتماديات جديدة إلا بعد تقييم حجمها وتوافقها وصيانتها وأثرها في البناء على Windows وLinux، مع مراعاة macOS.
- إبقاء الميزة اختيارية، وعدم تحميل التطبيقات التي لا تستخدمها تكلفة غير ضرورية.
- اختيار موضع التكامل الصحيح بعد مراجعة مسارات HTTP وWebSocket الحالية، بدل إضافة الضغط بطريقة سطحية أو تكرار المعالجة داخل عدة طبقات.

## 5. اختيار الخوارزميات وتبريرها

قيّم الخوارزميات حسب التوافق والأداء والتكلفة وتعقيد الصيانة:

- **gzip:** الخيار الأول المقترح لضغط HTTP بسبب انتشاره وتوافقه الواسع.
- **Zstandard (zstd):** خيار إضافي لـ HTTP يستحق الاختبار مع البيانات الكبيرة عندما يدعمه العميل والبيئة.
- **Brotli:** خيار إضافي لمحتوى الويب النصي، خصوصًا الملفات الثابتة التي يمكن ضغطها مسبقًا.
- **DEFLATE عبر `permessage-deflate`:** نقطة البداية لضغط WebSocket.
- لا تفترض أن خوارزمية متاحة في HTTP يمكن استخدامها مباشرة داخل WebSocket دون امتداد وتفاوض متوافقين.
- لا تضف خوارزميات أخرى لمجرد زيادة عدد الخيارات؛ يجب أن يكون لكل خيار مبرر ونتائج اختبار تثبت فائدته.
- قيّم حجم الاعتماديات، وزمن البناء، والتوافق بين المنصات، وسهولة الصيانة والتحديث.

## 6. اختيار البيانات المناسبة للضغط

وفّر سياسات قابلة للتخصيص لتجنب الضغط غير المفيد في HTTP وWebSocket.

### بيانات HTTP المناسبة للاختبار

- ملفات JSON الكبيرة والمتكررة.
- HTML وCSS وJavaScript.
- ملفات CSV وXML وLogs النصية.
- دفعات كبيرة من البيانات النصية المنظمة.

### بيانات HTTP التي غالبًا لا تستفيد كثيرًا

- الردود الصغيرة جدًا.
- الصور والفيديو المضغوطان مسبقًا.
- ملفات ZIP والبيانات المضغوطة أو المشفرة مسبقًا.

### بيانات WebSocket المناسبة للاختبار

- رسائل JSON المتكررة والكبيرة.
- تحديثات الحالة والأحداث النصية المتكررة.
- الرسائل النصية المنظمة التي تحتوي على حقول وقيم متكررة.
- الرسائل الثنائية الكبيرة عندما تكون بياناتها قابلة للضغط فعلًا.

### بيانات WebSocket التي قد لا تستفيد كثيرًا

- الرسائل الصغيرة جدًا.
- الصور والوسائط المضغوطة مسبقًا.
- البيانات المشفرة أو المضغوطة مسبقًا.
- الرسائل العشوائية أو عالية الإنتروبيا.

لا تعتمد على امتداد الملف وحده، ولا على نوع الرسالة وحده، لاتخاذ قرار الضغط. استخدم الحجم ونوع المحتوى والسياق والسياسة المحددة. ولا تفترض أن كل JSON أو كل رسالة كبيرة ستحقق نسبة ضغط عالية.

## 7. تقليل التكلفة الحسابية وتحسين الأداء

- توفير حد أدنى قابل للتخصيص لحجم المحتوى أو الرسالة قبل الضغط.
- تقييم جدوى الضغط وتجنب استخدامه عندما تكون تكلفته أكبر من فائدته.
- مقارنة مستويات الضغط المختلفة، وعدم اختيار أعلى مستوى تلقائيًا.
- قياس CPU والذاكرة وزمن الاستجابة وحجم البيانات المنقول معًا.
- تقييم الضغط المسبق وتخزين النتائج للمحتوى الثابت في HTTP عندما يكون ذلك مناسبًا.
- تجنّب إعادة ضغط المحتوى الثابت نفسه في كل طلب عندما يمكن إعادة استخدام نتيجة آمنة وصحيحة.
- التعامل مع الردود المتدفقة في HTTP دون افتراض معرفة الحجم النهائي مسبقًا أو تحميل المحتوى كاملًا في الذاكرة.
- تقييم إمكانية معالجة رسائل WebSocket تدريجيًا حيث تسمح آلية البروتوكول والتنفيذ بذلك.
- تقليل نسخ البيانات وتخصيص الذاكرة المتكرر والعمل غير الضروري.
- تقييم ما إذا كانت الرسائل الصغيرة أو البيانات المضغوطة مسبقًا تستحق الاستثناء من الضغط.
- اختبار أثر الضغط تحت الحمل المتزامن، وعدم تحسين الحجم النهائي على حساب زمن الاستجابة أو استقرار النظام.

## 8. سياق الضغط في WebSocket والضغط الخلفي

- دراسة إعدادات `client_no_context_takeover` و`server_no_context_takeover` وتطبيقها وفق مواصفات الامتداد والتفاوض الفعلي.
- توثيق أثر الاحتفاظ بسياق الضغط بين الرسائل في كفاءة الضغط واستهلاك الذاكرة.
- عزل سياقات الضغط بين الاتصالات المستقلة وعدم مشاركة الحالة بينها بصورة غير آمنة.
- تقييم مخاطر تسرب المعلومات عند ضغط بيانات سرية وغير سرية ضمن سياق مشترك، وتوفير خيارات مناسبة لتقليل المخاطر.
- التعامل الصحيح مع الرسائل المجزأة دون الخلط بين حدود الإطارات وحدود الرسائل.
- دراسة أثر الضغط على `Backpressure`، والتأكد من عدم تراكم الرسائل بصورة غير محدودة عندما يكون العميل بطيئًا.
- توفير حدود واضحة لطوابير الرسائل والبيانات قيد المعالجة بما يتوافق مع نظام إدارة الموارد الحالي في Aether.
- عدم افتراض أن ضغط الرسائل يقلل دائمًا من زمن الاستجابة؛ يجب قياس أثره الفعلي.

## 9. الأمان وحدود الموارد

### حدود HTTP

- تحديد حد لحجم الطلب المضغوط وحد مستقل لحجم البيانات الناتجة بعد فك الضغط.
- احتساب الحجم الناتج تدريجيًا ومنع تجاوزه.
- دراسة حدود نسبة التمدد ووقت فك الضغط واستهلاك CPU والذاكرة.
- التعامل مع البيانات التالفة والترميزات غير المدعومة والإلغاء والمهل الزمنية.
- الحفاظ على حدود الطلبات الحالية وعدم تجاوزها عبر فك الضغط.

### حدود WebSocket

- تحديد حد لحجم الرسائل الواردة بعد فك الضغط، مع مراعاة حدود الإطارات والرسائل المجزأة.
- تقييد حجم البيانات المضغوطة وحجم البيانات الناتجة واستهلاك الموارد أثناء فك الضغط.
- منع تجاوز الحدود نتيجة التمدد الشديد أو الرسائل المصممة لاستهلاك الموارد.
- معالجة الإلغاء وانقطاع الاتصال والمهل الزمنية دون تسرب الذاكرة أو ترك مهام غير منتهية.
- اختبار الاتصالات المتزامنة الكثيرة والرسائل الكبيرة وحالات الاستهلاك المرتفع للموارد.

### متطلبات مشتركة

- إعادة استخدام ضوابط الموارد الحالية متى أمكن، وتجنب إنشاء مسارات محاسبة متعارضة.
- التحقق من حدود الحجم قبل تخصيص ذاكرة كبيرة كلما أمكن.
- ضمان تحرير الموارد عند الخطأ أو الإلغاء أو انقطاع الاتصال.
- توثيق المخاطر الأمنية والحدود الافتراضية وطرق تخصيصها.
- عدم التضحية بالأمان مقابل تحسن نظري في الأداء.

## 10. التوثيق وتجربة المطوّر

يجب توفير توثيق واضح يغطي:

- الفرق بين ضغط الردود وفك ضغط الطلبات في HTTP.
- الفرق بين ضغط HTTP وضغط رسائل WebSocket.
- كيفية تفعيل كل ميزة أو تعطيلها بصورة مستقلة.
- الخوارزميات المدعومة فعليًا وتوافقها مع العملاء والبروتوكولات.
- إعدادات الحد الأدنى للحجم ومستويات الضغط وحدود الموارد.
- التعامل مع البيانات الكبيرة والمتدفقة والرسائل المجزأة.
- التعامل مع المحتوى المضغوط مسبقًا والترميزات غير المدعومة والبيانات التالفة.
- أثر سياق الضغط في WebSocket وطرق التحكم في الاحتفاظ به.
- حدود الحجم والأخطاء المحتملة والاعتبارات الأمنية والضغط الخلفي.
- أمثلة عملية قابلة للتشغيل باستخدام واجهات Aether الفعلية.
- عدم الادعاء بأن نسبة ضغط معينة أو تحسنًا محددًا مضمون لجميع البيانات.
- توضيح أثر تعطيل الميزة على الاعتماديات والتخصيصات واستهلاك الموارد.

## 11. الاختبارات وقياس الأداء

### اختبارات HTTP

- اختبر البيانات دون ضغط ومع gzip وZstandard وBrotli عند تضمينها.
- اختبر ضغط الردود وفك ضغط الطلبات كلًّا على حدة.
- اختبر تفاوض الترميزات والترويسات وحالات المحتوى المضغوط مسبقًا.
- اختبر البيانات الصغيرة والكبيرة والمتدفقة والبيانات التالفة وحدود التمدد.
- تحقق من تطابق المحتوى بعد فك الضغط ومن صحة سلوك HTTP.

### اختبارات WebSocket

- اختبر التفاوض على `permessage-deflate` مع عملاء يدعمونه وعملاء لا يدعمونه.
- اختبر الرسائل النصية والثنائية والفارغة والصغيرة والكبيرة والمجزأة.
- اختبر إعدادات الاحتفاظ بالسياق وعدم الاحتفاظ به.
- اختبر البيانات المضغوطة مسبقًا، والرسائل التالفة، وانقطاع الاتصال أثناء المعالجة.
- اختبر الاتصالات المتزامنة والضغط الخلفي وحدود الرسائل والذاكرة.
- تحقق من سلامة محتوى الرسائل وترتيبها وصحة الإطارات بعد الضغط وفك الضغط.

### القياسات

قِس، حسب البروتوكول ونوع البيانات:

- الحجم قبل الضغط وبعده.
- زمن الضغط وفك الضغط.
- استهلاك CPU والذاكرة.
- زمن الطلب أو الرسالة.
- معدل الطلبات أو الرسائل تحت الحمل.
- نسبة الأخطاء واستقرار النظام تحت الضغط.
- تكلفة الاعتماديات والبناء عند وجود أثر ملحوظ.

يجب أن تكون المقارنات عادلة وقابلة لإعادة التنفيذ، مع فصل تكلفة الخوارزمية عن تكلفة الشبكة والـ runtime وأداة القياس. لا تعتبر أصغر حجم دليلًا كافيًا على أفضل أداء، ولا تدّعِ تحسنًا دون أدلة موثوقة.

## 12. طريقة التنفيذ والحوكمة

1. راجع بنية Aether الحالية ومسار قراءة طلب HTTP وإنتاج الرد والبث والـ middleware وحدود الموارد.
2. راجع تنفيذ WebSocket الحالي ومسار المصافحة والتفاوض والإطارات والرسائل المجزأة والإرسال والاستقبال.
3. حدد موضع التكامل الصحيح لكل بروتوكول، وقيّم جدوى طبقة الضغط المشتركة قبل كتابة الكود.
4. وثّق التصميم والقرارات والاعتماديات والقيود والمخاطر قبل التنفيذ.
5. نفّذ الميزة تدريجيًا مع اختبارات الوحدة والتكامل والتوافق والأمان والأداء.
6. حافظ على السلوك الحالي عند عدم تفعيل الميزة، ولا تكسر واجهات عامة دون مبرر موثق.
7. اختبر البناء والتوافق على Windows وLinux، مع مراعاة macOS ضمن الأهداف المعتمدة للمشروع.
8. سجّل نتائج القياس الفعلية، وميّز بوضوح بين الحقائق والافتراضات والنتائج غير الحاسمة.
9. لا تنفّذ commit أو push أو merge أو تغييرات خارج النطاق المصرّح به دون موافقتي.

**الهدف النهائي:** نظام ضغط وفك ضغط اختياري ومتكامل في Aether يدعم HTTP وWebSocket، ويعيد استخدام المكونات المشتركة عندما يكون ذلك مفيدًا، مع احترام اختلاف البروتوكولين، وتقليل تكلفة المعالجة والذاكرة، وضمان التوافق والأمان، وتقديم فائدة مثبتة للتطبيقات التي تتعامل مع بيانات كبيرة دون تحميل التطبيقات الأخرى تكلفة غير ضرورية.





