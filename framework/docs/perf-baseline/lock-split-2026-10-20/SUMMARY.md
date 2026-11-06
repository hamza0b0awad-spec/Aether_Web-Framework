<!-- cite-check: snapshot 2026-10-20 -->
# Engine Mutex: WAIT vs HOLD, measured separately (2026-10-20)

Instrument: `src/bin/sched_lock_split.rs` (bench-only; **no production code changed**).
Mirrors the six lock acquisitions one request-pair makes in `SchedulingLayer`
(`framework/aether/src/scheduling.rs:187-199`, `207-224`) and times attempt-to-ownership (WAIT)
apart from ownership-to-release (HOLD). Raw logs: `run1.txt`, `run2_controls.txt`,
`run3_sites.txt`, `run4_pacer_control.txt`, `run5_paced_fixed.txt`.

## Instrument design and its limits

* Timing: two `rdtsc` reads per acquisition. Calibrated against the wall clock on this box:
  **2.2221 ticks/ns** (2.22 GHz invariant TSC).
* Floor: `null` mode runs the same six acquisitions around a single field read. At 1 thread that
  floor is **wait 15 ns / hold 15 ns per acquisition**. Any `real` HOLD near 15 ns is instrument.
* Counters are per-thread and cache-line padded (`#[repr(align(64))]`), merged only at the end.
  This replaces the earlier `sched_direct` `lockAvg` (~94 us), which was produced by shared atomic
  accumulators and by measuring WAIT while the queue was saturated — that number is withdrawn.
* Mirror, not the real layer, because instrumenting inside a critical section changes its length.
  Cross-validation: 1 thread `real` = 837k-842k cycle/s vs `sched_lock_probe` (tokio, same job
  shape) 738k; 8 threads 256k-382k vs 269k. Same order, mirror ~15-40% faster (no tokio task
  switch, no sockets, no parser, no handler body, no `Notify`).
* Excluded by design: `Notify::notify_waiters()` (cheap when nothing is parked: measured earlier at
  1.00 wakeup per completed handler, `parkedOnNotify=0`), tokio's task wake/poll, socket I/O.

## First paced run was wrong, and the reason matters

Staggering thread phases by `interval * i` while each thread's period is also `interval` puts every
thread on the **same arrival grid**: all 8 threads knock at the same instants. That manufactured
convoy produced WAIT means of 12-45 us at 25k cycle/s. Fixed to stagger on the aggregate
inter-arrival grid (`1/target * i`), the same cells give WAIT means of 17-167 ns. Every paced
number below is from the corrected build (`run5_paced_fixed.txt`); `run4` is kept as the record of
the artifact. Provenance checked: `run4` ran the 02:23:xx binary, `run5`/`run6` the 02:25:17 binary
that contains the fix (source line `let stagger = ...1_000_000_000.0 / target...`).

## Measured, corrected, at the server's operating rate (256 live connections)

| threads | target cycle/s | WAIT mean /acq | WAIT <5 us share | HOLD mean /acq | cycle WAIT+HOLD |
|---|---|---|---|---|---|
| 1 | 25,000 | 17 ns | 100.0% | 162 ns | 1.07 us |
| 2 | 25,000 | 20 ns | 100.0% | 214 ns | 1.41 us |
| 4 | 25,000 | 22 ns | 100.0% | 257 ns | 1.68 us |
| 8 | 12,000 | 34 ns | 100.0% | 299 ns | 2.00 us |
| 8 | 25,000 | 167 ns | 100.0% | 315 ns | 2.89 us |
| 8 | 100,000 | 186 ns | 99.8% | 308 ns | 2.97 us |
| 8 | unpaced (256k) | 2,944-4,372 ns | 89-91% | 506-733 ns | 20.7-30.6 us |

Per call site (HOLD mean, 256 live connections):

| site | 1 thread | 2 threads | 8 threads @25k | 8 threads unpaced |
|---|---|---|---|---|
| `admit()` + `pump()` | 328 ns | 464 ns | 718 ns | 1,253-1,984 ns |
| `job_state()` | 33 ns | 35 ns | 41 ns | 78-151 ns |
| `complete()` + `pump()` | 124 ns | 143 ns | 186 ns | 191-269 ns |

## Repeatability (`run6_repeats.txt`, two further reps of the load-bearing cells)

| cell | rep A | rep B | stable quantity |
|---|---|---|---|
| 8 threads, paced 25k | wait mean 167 ns | 79 ns / 49 ns | fast-path wait 24 ns, 100% under 5 us in all three |
| 8 threads, paced 25k | hold mean 315 ns | 301 ns / 288 ns | hold is the reproducible term |
| 2 threads, paced 25k | wait 20 ns / hold 214 ns | 20/222, 27/223 | both terms stable to ~5% |
| 8 threads, unpaced | 256k cycle/s, wait 2,944 ns | 330k/328k cycle/s, wait 3,418/3,469 ns | ceiling 256-382k, i.e. 10-15x the demand |

The mean WAIT at 8 threads is therefore **not a measured property of the lock**: it is 24 ns for
every acquisition but one, plus one to two acquisitions per 5 s that run into milliseconds (max
1.5-24.5 ms, and HOLD max 4.5 ms in the same runs). Those single-shot events are the
deschedule-inside-the-section mechanism in item 6 below, arriving roughly once per thread per 5 s;
they cannot move throughput at 150k acquisitions/s, but they are exactly the shape of a tail-latency
event.

## What this says

1. **WAIT is not the cost at any operating point the server actually occupies.** At 1-4 threads it
   is at the instrument floor (17-22 ns) for 100% of acquisitions; at 8 threads and 25k cycle/s,
   100% of acquisitions wait under 5 us with a 24 ns fast-path mean. The 167 ns mean is one
   pathological wait of 24.5 ms out of 150k acquisitions, not queueing.2. **HOLD is the cost, and it grows with sharing, not with waiting**: 162 ns/acq at one thread
   to 315 ns/acq at eight, while WAIT stays ~20 ns. That is cache-line ownership of the engine
   state moving between cores, which a `Mutex` change or a lock split would not remove.
3. **The cost is concentrated in `admit()+pump()`** (328 ns of the ~162 ns average, i.e. two of the
   six acquisitions carry half the time; `job_state()` is a flat 33-41 ns). If scheduling overhead
   is ever reduced further, the target is what `admit` does under the lock (job record insert,
   resource registration, the bounded admission walk) - not the mutex, not `Notify`, and not the
   tree rotation, which is already fixed.
4. **Serialized share of the real server**: a request on a keep-alive socket makes 3 acquisitions
   (handler `admit+pump`, `job_state`, `complete+pump`) = ~0.9 us of HOLD at 8 threads; at the
   measured 24.5k req/s ceiling the mutex is held ~2.2% of wall clock. Queueing at that utilisation
   is ~0.03 us. This matches the independently measured plain-vs-scheduled CPU/req delta of
   +1.4 us (`101.07 -> 102.54`) to within instrument resolution.
5. **Verdict against the pre-declared criteria: REFUTED.** The lock is not the bottleneck. WAIT
   neither dominates HOLD at the operating point nor approaches the ~40 us CPU a real request has;
   the mirror's throughput ceiling (256k-382k cycle/s unpaced at 8 threads) is 10-15x the demand.
   Contention only appears when the instrument itself saturates every core (`unpaced`), which is a
   capacity bound, not a defect: at that point 89-91% of acquisitions still take under 5 us.
6. **Not proven, and not measured here**: whether a Tokio worker is ever descheduled *inside* one
   of these sections on the live server (`hold max` reached 20.7 ms in the corrected 8-thread cell
   and 30.8 ms in the 1-thread unpaced cell, so the mechanism is real under oversubscription).
   Proving it would need instrumentation inside the production critical section, which changes the
   quantity being measured; the alternative is a tail-latency (p99.9/p99.99) comparison on the real
   server, not a lock measurement.
