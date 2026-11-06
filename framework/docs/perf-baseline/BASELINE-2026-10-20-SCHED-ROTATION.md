<!-- cite-check: snapshot 2026-10-20 -->
# BASELINE 2026-10-20 — scheduler rotation index fix (`tree.active` → `tree.divisible`)

Purpose: this is the **revert anchor** for the one production code change landed in this
perf session. Everything measured after it is measured against *this* state. If a future
change makes the server slower, `git revert` of this commit is the known-good restore point,
and the numbers below are what "good" was worth.

---

## 1. The defect

`JobTree::active` was documented as "Active divisible jobs in rotation order" but the engine
pushed **every job kind** into it at admission, before this change. The site was
`promote_active(id)` at `framework/scheduler/src/engine.rs:403` **on the pre-fix tree**; the fix
renamed the helper and the same promotion is now the `if kind.is_divisible()`-gated
`promote_divisible(id)` inside `Engine::pump`, so that coordinate is deliberately not repeated as
a current pointer. The pushed kinds included long-lived keep-alive connections and handler jobs —
which can never receive a work grant.

Consequences, all inside the single global engine `Mutex`, twice per HTTP request:

| site | what the wrong index cost |
|---|---|
| `pump()` round start | cleared `granted_this_round` by scanning an index as long as the **connection count** |
| `rotate_active_divisible()` / `first_active_divisible()` | probed the same whole-server list per round |
| `complete()` / `cancel()` | `remove_active(id)` = linear **search + shift** over that list |

So `pump()` was O(live connections) per call while it should have been O(live **divisible**
jobs) — normally zero on an HTTP server, since `run_slices` is not used by the serve path.

## 2. The change

- `framework/scheduler/src/tree.rs`: field renamed `active` → `divisible`, made the
  divisible-only invariant explicit in the doc comment; `promote_active`→`promote_divisible`,
  `remove_active`→`remove_divisible`, `active_len`→`divisible_len`, plus `divisible_ids()` for
  the invariant test.
- `framework/scheduler/src/engine.rs`: promotion and removal are now gated by
  `if kind.is_divisible()` at the single admission point and the single release point, so a
  connection/handler/CPU job never enters or is searched for in the index; the two round loops
  and the cursor advance iterate `divisible_len()`.
- Tests added (`engine.rs #[cfg(test)] mod tests`):
  `only_divisible_jobs_join_the_rotation_index`,
  `divisible_removal_keeps_the_remaining_order_and_leaves_no_orphans`.
- Bench/diagnostic tools added (no production effect):
  `framework/scheduler/examples/pump_cost.rs`,
  `framework/scheduler/examples/active_trace.rs`.

## 3. Environment (all numbers below are this box only)

- `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1`, host `x86_64-pc-windows-msvc`,
  Windows 10.0.26200, **8 logical cores**.
- TSC calibration for every nanosecond figure in the archive: **2.2221 ticks/ns** (2.22 GHz
  invariant TSC); every instrument re-asserts the value at run time.
- Background load: chrome / telegram / Qoder running. Cross-session comparisons are **invalid**
  on this box; only same-session A/B pairs are reported.

## 4. Verification level 1 — engine cost curve (`pump_cost`, before vs after)

One request cycle = `admit + pump + complete + pump`, measured against N live connections.
Raw: `lock-split-2026-10-20/before_pump_cost.txt`, `after_pump_cost.txt`.

| live connections | before | after |
|---|---|---|
| 1 | 0.70 µs | 1.88 µs |
| 32 | 2.41 µs | 1.23 µs |
| 64 | 4.28 µs | 1.42 µs |
| 128 | 9.80 µs | 1.56 µs |
| 256 | 22.15 µs | 1.36 µs |
| 512 | 48.90 µs | 1.17 µs |
| 1024 | 96.32 µs | 1.15 µs |

- **256 ÷ 32 = 9.20× before → 1.11× after.** The curve is now flat in the connection count,
  which is the structural claim; it is not a constant-factor improvement.
- Breakdown at 256 live connections: `pump only` **10.77 µs → 74.23 ns** (145×),
  `admit + cancel (no pump)` 95.5 ns → 276 ns, `complete of a fresh job` **8.19 µs → 863.6 ns**.
- Honest caveat: the `active=1` after-cell (1.88 µs) is *worse* than before (0.70 µs). That is
  the first cell of a single run (warm-up / CPU freq ramp), not a regression signal — every
  cell from 32 up is 1.15–1.56 µs and repeat runs of the flat region agreed to ±0.2 µs.

## 5. Verification level 2 — decision equivalence (`active_trace`)

Question: did the fix change any scheduling **decision** it was not supposed to change?
Raw: `before_trace.txt`, `after_trace.txt`, `trace.diff`.

- Main AdaptiveTree scene: **lines 1–872 are byte-identical.** Admissions, grants, quotas,
  class moves, capacity changes, metrics, baseline — unchanged. The fix is behaviour-preserving
  where it should be.
- Differences are confined to the deliberately-added rotation scene
  (`########## scene: rotation with live connections present ##########`, from line 873),
  where the old code starved the divisible set by always finding job 0 first.
  Per-job grants / granted work in that scene:
  - before: job0 **18 / 2800**, job1 8 / 1000, job2 7 / 1000
  - after: job0 13 / 1800, job1 10 / 1000, job2 **14 / 2000**
  - **total granted work identical: 4800 both.** The fix redistributes, it does not create work.
- One side-effect to keep on record: in that scene `grants 24 → 28` and `slices 12 → 11`.
  Fair round-robin means more, smaller grant events for the same aggregate work. This is the
  intended semantics, but it is a measurable change in the engine's event count.

## 6. Verification level 3 — real HTTP server, same-session A/B

Two Release binaries, same route, same client, alternating order, medians/means of the reps.
Raw: `after_ladder.txt`, `after_ladder_tight.txt`, `after_ladder_256x8.txt`.

Before the fix the scheduled server lost **−2.8 / −8.8 / −34.7 / −55.6 %** at
32/64/128/256 connections. After the fix:

| connections | Aether (plain) | AetherSched | delta | reps |
|---|---|---|---|---|
| 32 | 29,695 | 28,549 | −3.9 % | 4 |
| 64 | 26,039 | 24,808 | −4.7 % | 4 |
| 128 | 23,282 | 23,900 | **+2.7 %** | 4 |
| 128 (tight) | 22,811 | 22,442 | −1.6 % | 6 |
| 256 | 26,111 | 24,691 | −5.4 % | 4 |
| 256 (tight) | 24,591 | 21,413 | −12.9 % | 6 |
| 256 (8 reps) | mean 26,645 / median 26,497 | mean 24,454 / median 23,557 | **−8.2 % mean** | 8 |

- The 256×8 cell is the most-replicated one: plain higher in 6 of 8 reps, paired mean difference
  +2,191 rps (sd 2,564, sem 906), **two-sided exact sign-test p ≈ 0.29**. With this box's spread
  (single-cell spreads of 5,000–14,000 rps) **that is not a resolved result**, and it is not
  claimed as one.
- What *is* resolved: the −55.6 % cell no longer exists, and the loss no longer scales with the
  connection count. The `predicted scheduling work` annotations in those logs (2.3–23.2 µs/req of
  serialized engine time, from the **before** curve) are now stale — the after-curve predicts
  ~1.4 µs/req flat.

## 7. What is NOT proven by this baseline

1. **Server-level attribution of the residual.** ~1.4 µs of lock-held engine work per request
   (measured separately, see `lock-split-2026-10-20/SUMMARY.md`) is the same order as the
   ~1.4 µs/req CPU delta, but order-of-magnitude agreement is not attribution; the delta may
   include other costs. The Mutex itself was **REFUTED** as the bottleneck; splitting, sharding,
   or replacing `Mutex`/`Notify` is explicitly **not** recommended on current evidence.
2. **Deschedule-while-holding the lock on the live server.** The instrument observed max HOLD of
   20.7–30.8 ms, but it is a mirror (no Tokio task switch, sockets, parser, or handler body).
   The right test is a p99.9/p99.99 plain-vs-scheduled comparison on the real server, not another
   lock measurement.
3. **A stable −x % cost of the scheduling layer at 256 connections.** See the p ≈ 0.29 above.
4. The `active=1` after-cell anomaly in §4 is unexplained and un-replicated.

## 8. Test suite state at this baseline

`cargo test --workspace` in `framework/`: **46 suites, 328 passed, 0 failed, 1 ignored.**
Full log: `lock-split-2026-10-20/suite-2026-10-20-precommit.txt`.

## 9. Provenance

Code hashes (sha256, first 16): `framework/docs/perf-baseline/code-hashes-2026-10-20.txt`
— `tree.rs c579af3702a192da`, `engine.rs 62f67bafb07cb524`, `policy.rs e8092878efb7c124`,
`resources.rs 11968064364a0a4e`, `tests/pump_head_of_line.rs 4cd675ce61297d13`,
`tests/engine.rs 630309d1fd84976e`, `examples/pump_cost.rs 84b80a3aec8fb3eb`,
`examples/active_trace.rs 5d428256e2bfad97`, `aether/src/scheduling.rs 70acaa23e1911939`,
`aether/src/app.rs 00ba612e8104aadc`. Base commit before this change: `51bba9b`.
**Landed as `f909c78`** (source + examples + this archive only; the ~58 staged
`framework/target/` build-artifact paths were deliberately left out of the commit).

Raw artifact archive: `framework/docs/perf-baseline/lock-split-2026-10-20/` (16 files).
The withdrawn contaminated numbers and the pacer-aliasing bug that produced them are recorded in
that directory's `SUMMARY.md`, with the raw `run4_pacer_control.txt` beside it.

## 10. Revert

```bash
git revert f909c78          # restores tree.active and the O(connections) pump()
# then re-anchor:
cargo build --release --example pump_cost -p aether-scheduler
cargo run   --release --example pump_cost  -p aether-scheduler
```

Revert is expected to restore the pre-fix numbers in §4 (9.20× slope, 10.77 µs `pump only` @256)
and re-open the −55.6 % 256-connection cell. That reproduction is the confirmation that §4 and §6
measured the same defect.
