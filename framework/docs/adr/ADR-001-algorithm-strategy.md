# ADR-001 — Algorithm & data-structure strategy (2026-10-20)

Owner instruction (2026-10-20, external advisory note): do not chase one super-algorithm or
per-component reinvention. Pick the right structure per layer, gate every choice on
measurement, and prefer studied existing solutions over bespoke ones unless a specific
measured problem justifies innovation. This file records which of those strategies Aether
**already** implements, which are **evidence-gated** (may be adopted only after a
before/after measurement), and which are **rejected** — so the same question is not
re-litigated from scratch in a later session.

## Already implemented (verified against code, not docs)

| Strategy | Where it lives | Evidence |
|---|---|---|
| Radix/trie routing with shared prefixes | `framework/router/src/trie.rs`, `node.rs` | 20 router tests; per-method param names (PROB-0007) |
| Bitmask method/guard state, `Allow` from bitmask | `framework/http/src/method.rs` (`bit`/`bitmask`), `trie.rs::bitmask_to_allow_header` | router + `http_e2e` 405 tests |
| Async I/O on a fixed worker pool (work-stealing delegated to Tokio) | `framework/runtime`, `aether::app` connection tasks | banner reports `Workers: 8 (configured; tokio work-stealing)` |
| Bounded queues + backpressure + shedding | `framework/scheduler` (admission, 503 + `Retry-After`, adaptive per-kind capacity) | 57 scheduler tests, `stress.rs` no-leak soak |
| Zero-copy-ish parsing on `&[u8]`, deferred materialisation | `framework/http/src/parser.rs`, `RequestArena` in `core/src/arena.rs` | `test_request_arena_zero_copy`, allocs/req ladder |
| Buffer reuse, per-connection scratch | `conn_buf`/`write_buf`/`temp_buf` reused across keep-alive requests | allocs/req measurement at 8 conns (track 3) |
| Cancellation safety / orphan-free cleanup | RAII `AdmissionGuard`, `QueueToken` pin, `JoinSet` drain | `aborting_a_queued_admit_leaves_no_stranded_job` |
| Slow-client + flood defenses | read/write/request timeouts, header count & byte budgets, body caps, resumable chunked decode, rate limiter, Host allow-list | 45 `http_e2e` wire tests, 21 middleware wire tests (8 in `user_middleware.rs` + 13 that register a WebSocket route *and* a middleware in `http_e2e.rs`; recounted 2026-10-22 from the test attributes — this row said 41/18 and predated the `101` and PROB-0014 slices) |
| Keep-alive / connection reuse | HTTP/1.x persistence with correct close semantics incl. 1.0 | `http10_*`, `repeated_connection_lines_still_signal_close` |
| Word-at-a-time masking where it pays (no vector intrinsics exist anywhere in this tree) | `framework/validation/src/simd.rs` — `validate_ascii_fast` tests 8 bytes with one `u64` mask | one `aether-tests` test builds an `Email` (in `framework/tests/src/lib.rs`); the validation crate itself has no `#[test]` |

## Evidence-gated (do NOT implement without a measurement that justifies it)

1. **Direct-match fast path for static routes vs pure radix.** The advisory suggests
   comparing both by route-table size. Our router bench (`framework/benchmarks/benches/
   router_bench.rs`) is the instrument. Decision requires: same table, both shapes, several
   sizes (10 / 100 / 1000 routes). Routing is *not* a measured bottleneck today, so this
   stays parked.
2. **SIMD in the HTTP head scan.** Our parser is `httparse`-backed and the measured cost
   centre was header *materialisation*, not scanning (track 3: 22 → 19 allocs/req after
   fixing ownership). SIMD scanning must not be added before a profile shows the scan itself
   is hot. Portable fallback required.
3. **Sharding / replacing the scheduler `Mutex`.** REFUTED by measurement: WAIT is at the
   instrument floor (17–34 ns), the cost is HOLD (~0.9 µs/req). See
   `feedback-lock-wait-vs-hold-refuted` and `aether-bench-compare/results/lock_split/SUMMARY.md`.
4. **A bespoke allocator / runtime replacing Tokio.** `arena.rs` exists and is tested for
   request-scoped allocation; the default path deliberately keeps the system allocator. Any
   change needs A/B evidence at equal work, and the closed-loop-latency caveat applies
   (p50 vs concurrency is a client artifact — compare rps ceiling and µs CPU/req).
   The same rule bounds the *throughput* number: an interpreted closed-loop client (the Python
   keep-alive probe used for ad-hoc checks reports ~10²–10³ req/s on this box) caps at its own
   cost, so a ceiling it prints is not the server's until raising `-n` and `-c` stops changing
   the answer. Compiled clients flipped a result on this machine before; see the withdrawn
   topology finding.
5. **A custom scheduler beyond the delivered adaptive tree.** Tokio work-stealing stays the
   task scheduler; `aether-scheduler` governs *admission*, which is a different layer. Do not
   conflate them or rebuild the former.

## Rejected as stated in the advisory

- "O(1) dispatch must complete in 1–2 CPU cycles" as an architectural requirement: kept as
  direction (bitmask checks exist), rejected as an acceptance criterion, because O(1) in theory
  is not faster in practice at our table sizes (branch prediction, cache line count and thread
  contention dominate). Consistent with the advisory's own §5.
- Per-component reinvention (own H2/HPACK/TLS stacks). The owner chose `h2` + `rustls`
  (2026-10-20) for HTTP/2 + TLS, matching "use proven implementations, don't reimplement
  complex protocol machinery to claim ownership".

## Standing order of work (the advisory's §4, restated for this repo)

1. locate the bottleneck by measurement → 2. routing only if measured → 3. memory/allocations
→ 4. concurrency/queues under balanced *and* skewed load → 5. pressure & security soak
→ 6. algorithmic innovation only for a residual measured bottleneck → 7. verify on Windows
**and** Linux against the recorded baseline.

Steps 1–5 have each been performed at least once this cycle; step 6 currently has no
qualifying residual bottleneck except the unattributed 256-connection gap (PROB-0004 context),
and step 7 is what the HTTP/2 + TLS track will newly require.
