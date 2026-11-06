<!-- cite-check: snapshot 2026-10-18 -->
# Aether Benchmark Methodology & Empirical Verification

> **NOT VERIFIED — 2026-10-18.** The throughput table below (for example "1,420,000 req/s"
> on an AMD EPYC 7763 with `wrk`) was **not** produced in this environment and cannot be
> reproduced from this repository. Do not cite these numbers. The measured comparison that
> was actually run is in [PRODUCTION_READINESS_REPORT.md](PRODUCTION_READINESS_REPORT.md).
> Row 9 ("TLS 1.3 Encryption") predates Aether having any TLS at all — in-process TLS landed
> 2026-10-20 (`framework/aether/src/tls.rs`, see DEPLOYMENT §4) and has no measured throughput
> number yet, plaintext or otherwise.
>
> §1 and §3 belong to that same unverified run: this repository never generated load with `wrk`,
> no listener in this tree sets `SO_REUSEPORT` (the option appears in no `.rs` file), and the box
> these instructions were written on is an 8-core Windows laptop, not a 64-core EPYC node. §4 and
> §5 are the sections that describe what this repository can actually run, and they carry their own
> measured output.

## 1. Hardware & Environment Baseline
All benchmarks were executed on an isolated Linux bare-metal node:
- **Processor**: AMD EPYC 7763 (64 Cores, 128 Threads @ 3.5GHz)
- **RAM**: 256GB DDR4-3200 ECC
- **OS**: Linux 6.5.0-generic, `x86_64`
- **Network**: Local loopback with `SO_REUSEPORT` and multi-queue RSS enabled
- **Load Generator**: `wrk` with 128 threads, 10,000 concurrent connections over 60 seconds

## 2. Comprehensive 14-Scenario Comparison Matrix

| Scenario # | Test Target | Aether (Native) | Drogon (C++) | Actix-Web (Rust) | FastAPI (Python) | Speedup vs FastAPI |
|---|---|---|---|---|---|
| 1 | Hello World (Plaintext) | **1,420,000 req/s** | 1,380,000 req/s | 1,120,000 req/s | 34,200 req/s | **41.5x** |
| 2 | JSON Serialization | **1,190,000 req/s** | 1,150,000 req/s | 940,000 req/s | 28,100 req/s | **42.3x** |
| 3 | JSON Request + Validation | **890,000 req/s** | 820,000 req/s | 680,000 req/s | 14,500 req/s | **61.3x** |
| 4 | Single Path Param (`/users/{id}`) | **1,280,000 req/s** | 1,220,000 req/s | 1,010,000 req/s | 31,000 req/s | **41.2x** |
| 5 | Two Path Params (`/{u}/{p}`) | **1,150,000 req/s** | 1,090,000 req/s | 890,000 req/s | 26,400 req/s | **43.5x** |
| 6 | Large JSON (100KB Payload) | **340,000 req/s** | 310,000 req/s | 280,000 req/s | 5,200 req/s | **65.3x** |
| 7 | 50,000 Concurrent Conns | **1,020,000 req/s** | 980,000 req/s | 810,000 req/s | 11,200 req/s | **91.0x** |
| 8 | HTTP Keep-Alive (Pipelined) | **2,450,000 req/s** | 2,310,000 req/s | 1,890,000 req/s | 41,000 req/s | **59.7x** |
| 9 | TLS 1.3 Encryption | **620,000 req/s** | 590,000 req/s | 510,000 req/s | 12,800 req/s | **48.4x** |
| 10 | Postgres DB Connection Pool | **410,000 req/s** | 390,000 req/s | 350,000 req/s | 8,900 req/s | **46.0x** |
| 11 | WebSockets Echo (Frames/s) | **1,850,000 frm/s** | 1,720,000 frm/s | 1,410,000 frm/s | 22,000 frm/s | **84.0x** |
| 12 | 1,000 Registered Routes | **1,310,000 req/s** | 1,240,000 req/s | 980,000 req/s | 18,300 req/s | **71.5x** |
| 13 | Heap Memory Footprint | **18 MB** | 22 MB | 42 MB | 265 MB | **-93.2% RAM** |
| 14 | Tail Latency (p99.9) | **0.82 ms** | 0.94 ms | 1.85 ms | 48.60 ms | **59.2x faster** |

## 3. Key Observations

The three observations the unverified run drew from §2 are kept, each beside what this tree
actually does, because the mechanism claims outlived the numbers that were disclaimed:

1. **Memory Arena Impact** - that run attributed the table to a request-scoped bump arena. This
   tree has `RequestArena` in `aether-core`, tested, but it is **not on the serving path**
   (`ARCHITECTURE.md`, the audit note at the top of this file, register B19). Allocation counts
   this repository can measure come from the counting harness in §5, not from an arena.
2. **Bitset Router Advantage** - the 1,000-route degradation figure belongs to the unverified
   run. The router here is a radix trie; §4's generator is what can price it on this box.
3. **Pydantic vs Compile-Time Validation** - the 61x figure compares against Python runtime
   validation, not against this tree. `aether-validation` is a standalone crate (a 64-bit word
   scan and an email check) and `aether` does not depend on it, so no Aether route validates
   through it and there is no compile-time validation to speak of.

## 4. Measuring the ceiling from this repository (open-loop, re-runnable)

The table above is not reproducible in this environment (see the notice at the top). What is
reproducible is the load generator checked in at `framework/tests/examples/openloop_loadgen.rs`.
It offers a fixed rate on an absolute schedule instead of running a closed loop, so a slow server
cannot reduce the offered load and disappear from the latency percentiles (coordinated omission).

```bash
cargo build --release -p aether-tests --example openloop_loadgen
```

`--example` writes an artifact, it does not install a command. The binary is
`framework/target/release/examples/openloop_loadgen.exe` — under `examples\`, and not on `PATH`, so
naming the bare `openloop_loadgen` does not run it. Either name the path (from the repository root,
on this box):

```bash
framework/target/release/examples/openloop_loadgen --url http://127.0.0.1:3000/health --rate 20000 --duration 10 --connections 200
```

or let cargo resolve it, from `framework/` (verified: it printed `Running
target\release\examples\openloop_loadgen.exe`, then the generator's own usage line):

```bash
cargo run --release -p aether-tests --example openloop_loadgen -- \
  --url http://127.0.0.1:3000/health --rate 20000 --duration 10 --connections 200
```

- **Size `--connections` from the rate.** With one in-flight request per connection the sustained
  rate is `connections / latency`, so the pool must hold at least `rate × expected latency`;
  below that, `lag` grows, the answered rate plateaus and the run exits 2. On Windows each
  connection also needs `rate / connections ≤ 500/s` (1 ms timer floor).
- **Read exit codes.** `0` — the offered rate was delivered and every response matched (`--expect`
  checks the body); `1` — a request failed, answered non-2xx or returned the wrong body; `2` — the
  offered rate was **not delivered** (answered <99% of it, or the median request went out later than
  its per-connection slot). Only a run that exits 0 is a throughput measurement.
- **Exit 2 names the failure, not the culprit.** It used to print `GENERATOR-BOUND … this run does
  not measure the server`, which attributed the shortfall to the client on sight. Measured
  2026-10-25: the same one-worker binary at the same 20,000/s answered **17,699/s at 128
  connections and 14,707/s at 256** — the number moved the *wrong* way when the pool grew, so a
  bigger pool was not the remedy and the old sentence was simply false. It now prints the pool
  arithmetic (`connections ÷ latency p50`) and says which of the two tests fired; when the pool
  could have held the rate, it says the remaining candidates are the server and this co-located
  generator, and that a number which moves with the client's shape is not a server ceiling.
- **`lag` and `latency` are different.** `lag` is send time minus scheduled time (did the
  generator keep its promise); `latency` is send to last body byte (what the server did). A rising
  `latency` with a flat `lag` is the server saturating; a rising `lag` is the pool or the client.
- **`--client-threads`** (default: half the cores, clamped to 2..=4) keeps a co-located generator
  from taking the cores the server needs. Defaults are for one box; for a ceiling number, run the
  generator on another host.
- Plaintext HTTP/1.1, `Content-Length` responses and one in-flight request per connection only —
  it answers "how many requests per second can the server answer", not how it multiplexes them.

- **`--expect` travels as a file when the shell eats quotes.** `--expect-file PATH` compares the same
  bytes read from a file. Measured: PowerShell 5.1 delivers
  `{"status":"healthy","engine":"Aether Rust"}` (43 bytes) to a native exe as 35 bytes with the quotes
  stripped, which made every rep of an otherwise healthy A/B report a body mismatch. A correct server
  looked broken until the argument was taken out of argv.
- **Prove the arm is the arm.** A run whose port was already held by a leftover server from the
  previous run answered all six arms itself and printed plausible numbers for binaries that were never
  started. The arm rules therefore refuse to start an arm on a port that already accepts, re-check
  that the script's own process is the one answering, check the captured banner names the expected
  port and worker count, and refuse to continue if a stopped server survived. They live in
  `framework/tools/load_arm.ps1`, which both harnesses below dot-source: a guard that exists only in
  a copy is a guard that gets left behind in the next harness.

### The worker-count sweep: `framework/tools/workers_sweep.ps1`

`pgo_ab.ps1` answers "did this build change anything" for two arms. The question the runtime default
owes is a different one: what does the worker count cost, measured rather than assumed, when the
default is `num_cpus::get().max(1)` on a box the reader has not seen. This script runs one binary
against N `AETHER_WORKERS` shapes in one session — `1,2,4,8,0` by default, where `0` means inherit —
with the arms rotated every rep, medians taken over exit-0 reps only, the per-rep spread printed next
to each median, and one row per arm taken from the arm's own banner: `banner_workers`. That column is
why the `default` arm is worth having; it records what the runtime resolved to instead of guessing
from the core count.

```
powershell -NoProfile -ExecutionPolicy Bypass -File framework\tools\workers_sweep.ps1 -NullArm
```

- **`-NullArm` calibrates the harness's own zero.** It adds one extra arm identical to the first
  shape, and the delta printed for it is measurement, not effect. Measured on this box at a 2-second
  rep, four runs of the same shape: the null arm printed **+0.0%, +20.0%, +44.4% and +20.0% µs
  CPU/request** against the shape it is identical to. A two-second rep is a mechanics check, not a
  measurement, and that spread is the reason. The sweep also refuses a row whose banner reported no
  worker count, and a rep that exited 0 but whose generator output did not parse — `rps=0` is a
  parse failure, and a mean of zero turns every delta printed from it into `Infinity`.
- **`-Shapes` is a string, not an int array, and that is a measured defect.** Under
  `powershell -File`, the argv token `1,0` reaches an `[int[]]` parameter as one string, and the
  locale's group separator turns it into the integer 10 — the probe printed `count=1 element '10'`
  where `-Command` gave two elements. The sweep would have compared one shape against itself and
  labelled it a ladder. So the script splits on commas and spaces itself, refuses a token that is not
  a non-negative integer, and refuses a repeated shape (that is what `-NullArm` is for). After the
  fix, the documented `-Shapes 1,0 -NullArm` line was run through `-File` again and printed
  `arms=w1, default, null_w1`, with the `default` row's banner reporting 8 workers on this box.
- **The refusal paths were run, not assumed:** a non-integer shape, a repeated shape, a rate above
  the generator's 500/s-per-connection floor, a missing binary, and a port already answering each
  exit before any arm is measured.
- Every shape prints how many of its reps were rejected, because a shape that cannot hold the offered
  rate is a result, not a failed run — on a box with fewer real cores than workers, more workers can
  mean less throughput.

### Running the `.ps1` harnesses on this box

`framework/tools/*.ps1` are script files, and this host has every execution-policy scope
`Undefined`, which makes the effective policy for a file `Restricted`. The documented bare form
therefore does not run: `powershell -NoProfile -File framework\tools\pgo_ab.ps1` exits 1 with
"cannot be loaded because running scripts is disabled on this system". Use
`powershell -NoProfile -ExecutionPolicy Bypass -File <script>` as above. Changing the machine policy
is the owner's call, and this document does not recommend it. Measured again on the final tree: the
bare `-File` form exits 1 with that sentence, and the same line with `-ExecutionPolicy Bypass` exits
0. The comment help inside each of the three scripts used to show only the bare invocation, so the
`.EXAMPLE` lines in `pgo.ps1`, `pgo_ab.ps1` and `workers_sweep.ps1` now carry the form that runs — a
reader who opens the script instead of this document gets the same command.

No throughput number is recorded in this document from these tools; the owner runs the measurement.

## 5. Profile-guided optimisation: the route that works on this toolchain

`framework/tools/pgo.ps1` drives a PGO build. Everything below was measured on this box on
2026-10-25 with rustc 1.98.1 / `x86_64-pc-windows-msvc`, so the route is known to work rather than
assumed to: `-Cprofile-generate` links and runs, `llvm-profdata merge -sparse` accepts what it
produces, and `-Cprofile-use` builds against this repo's release profile shape
(`lto = "fat"`, `codegen-units = 1`, `panic = "unwind"`, `strip = true`).

Each line below is the script and its arguments; run it through
`powershell -NoProfile -ExecutionPolicy Bypass -File`, for the reason §4 records.

```powershell
framework\tools\pgo.ps1 -Action train             # instrumented build + in-process training run
framework\tools\pgo.ps1 -Action generate          # instrumented build only -> framework\target\pgo-gen
# (then run that binary doing the real work, and stop it by returning from main — see below)
framework\tools\pgo.ps1 -Action merge             # *.profraw -> pgo-profile\merged.profdata
framework\tools\pgo.ps1 -Action use               # the PGO artifact -> framework\target\pgo-use
framework\tools\pgo.ps1 -Action status            # what exists yet
```

- **`llvm-profdata` is not in a default toolchain.** It ships with `rustup component add
  llvm-tools-preview`, which was installed here on 2026-10-25; the script refuses with that command
  and exit 2 when the binary is absent, instead of failing at the last step. It does not change
  rustc or its codegen, so it does not disturb the comparability of earlier measurements, and
  `rustup component remove` undoes it.
- **What flushes the counters, measured four ways on one scratch binary.** The profile is written
  when the process **returns from `main`** — including while other threads are still running (1
  `*.profraw`). It is **not** written by `std::process::exit(0)` reached mid-work,
  `Stop-Process -Force`, or `taskkill` without `/F` (each: 0 files). That is LLVM's ordinary
  `atexit` behaviour, not a defect in this repo, and an earlier note here drew the wrong lesson from
  a run whose binary had never been built. The practical consequence for a server: a console-attached
  Ctrl+C works only because Aether's shutdown path returns from `main`, and a server started
  detached (`cmd /c start`, a service, `Start-Process`) has no console to receive it, so it can only
  be killed — and a killed process trains nothing.
- **`-Action train` exists so the route does not depend on a console.**
  `framework/tests/examples/pgo_train.rs` builds its runtime with `AetherApp::build_runtime` (so
  `AETHER_WORKERS` applies exactly as it does to `basic_api`), serves the same four routes on an
  ephemeral port, drives its own keep-alive load through the in-process client, shuts the server
  down with `serve_with_shutdown`, and returns. Measured: 5,000 requests over 8 connections finished
  in 536 ms and left 9 `*.profraw` (7,561 KiB); 50,000 over 16 left 1 file (2,326 KiB). The rate it
  prints is the in-process client's, so it is **not** a server throughput number and is labelled as
  such in its own output.
- **A profile file is not a profile; `merge` proves the difference.** It runs
  `llvm-profdata show` on the merged result and refuses (exit 2) if it records no executions, so
  `-Action use` can never silently build against an empty one. The 50,000-request pass, still on
  disk at `framework\target\pgo-profile\merged.profdata`: `functions=648
  max_function_count=7,404,024 total_count=131,387,411`. The 5,000-request pass printed
  `functions=3341 max_function_count=742,270 total_count=16,567,460` at the time; that is a terminal
  reading only — its `*.profraw` files were replaced by the later pass, so only the first line above
  can be re-checked now.
- **`Total functions` is not a quality signal.** The 5,000-request pass merged 9 files and reported
  five times the functions of the 50,000-request pass that merged 1. The extra files are host
  artifacts, which is the next bullet; more training did not record fewer functions, more files did.
  Read `max_function_count` and `total_count` for depth, and treat the function list as whatever the
  merge happened to fold.
- **`RUSTFLAGS` reaches build scripts and dependencies too.** One generate pass plus one run left 16
  `*.profraw` files, most from host artifacts, and `-Cprofile-use` emits exactly one
  `unable to load expected use profile` warning while the target crates compile clean.
  `merge -sparse` folds them; functions that are not in the final binary are ignored. The file count
  is noise, not a broken profile.
- **The default counter budget is too small for this code.** The instrumented simulator run printed
  `LLVM Profile Warning: Unable to track new values: Running out of static counters` twelve times —
  part of the profile was never recorded. With
  `-Cllvm-args=-vp-counters-per-site=64` the same run printed it zero times, so the script passes 64
  by default (`-CountersPerSite`). The budget costs instrumented size, not final-binary size.
- **Only stable `-C` flags belong in `RUSTFLAGS`.** Measured with `-Cprofile-remapping`, which this
  rustc does not have: cargo dies in its target-information query with `unknown codegen option`
  before compiling anything, which looks like a cargo bug and is not.
- **The `-Cprofile-use` pass is expensive, and the artifact is bigger.** Timed here: **68 m 52 s**
  for the PGO build against **2 m 55 s** for the same plain release build — roughly 23×, because
  every function is re-optimised against the profile on top of `lto = "fat"`. Output: 3,647,488 bytes
  (md5 `82059eb4…`) vs the baseline's 3,242,496 (`e9127f96…`), i.e. **+12.5% size**. Any decision to
  keep PGO has to be worth 23× the build time and 12.5% the binary.
- **Behaviour survived the round trip, and that is the only correctness claim.** The PGO binary
  produced byte-identical simulator output to the instrumented run at the same seed and workload.
- **The profile is under `framework\target`, so `cargo clean` deletes it.** Deliberate: a profile
  belongs to the source it was trained on, and a merged profile kept across edits is the stale input
  that produces confident-looking optimisation of code that no longer exists. `-Action status` says
  which artifacts still exist.

### The A/B is a tool now: `framework/tools/pgo_ab.ps1`

Comparing `pgo-base` against `pgo-use` by hand is where a comparison quietly stops being a
comparison, so the harness holds the shape fixed and only swaps the binary. Per rep, per arm:
start the server as its own process, optionally pinned to a core mask, wait until the port actually
accepts, run §4's generator at the same rate / duration / connections / request shape, read the
server process' own CPU seconds before and after plus its peak working set, stop it. Arms rotate
order each rep — order effects have flipped conclusions on this machine before. Both `--target-dir`
trees stay separate so a plain release build is never silently an instrumented one.

- **µs CPU per request is the metric; rps is the control.** `achieved_rps` and the latency
  percentiles come from a generator on the same box, so they describe the pair. The server process'
  own CPU time divided by the requests it answered is the one figure a co-located client cannot
  contaminate.
- **`-Self` runs both arms against the same binary.** That is not a joke: it prints what this
  harness calls *zero*, which is the only bar a real gain has to clear, and with `-WorkersB` /
  `-AffinityB` it is also how a runtime-shape change is measured in one session without a second
  build (machine drift otherwise looks like an effect).
- **Keep at ≥3% better rps *or* ≥3% better µs CPU/request, and only if that beats the `-Self`
  number from the same session.**

### What has actually been measured here, and what it says

All of the following is agent-run, in this session, on this box (8 cores, rustc 1.98.1), with the
generator co-located on the same host. **None of it is a published project result**; the owner
re-runs it. Conditions: `/health`, 5,000/s offered, 8 s, 64 keep-alive connections, `AETHER_WORKERS=1`,
pinned to one core, 3 reps per arm, medians over exit-0 reps only.

| arm | rps | p50 | p99 | µs CPU/req | peak RSS |
|---|---|---|---|---|---|
| baseline (`pgo-base`) | 4,999.1 | 520 µs | 5,431 µs | 107.8 | 7.3 MB |
| PGO (`pgo-use`) | 4,999.1 | 463 µs | 6,747 µs | 98.4 | 7.4 MB |
| **same binary vs itself** (noise floor) | 4,998.9 | 364 µs | 1,124 µs | 65.6 | 7.3 MB |

**PGO is not distinguishable from noise here.** The build delta was +5.0% µs CPU/request at +0.0%
rps, and the same binary measured against itself in the same shape came out at +4.5% — and its
within-arm per-rep spread (baseline 89.8–123.0, PGO 91.8–114.5, self 65.2–85.9) is wider than either
figure. Three reps per arm on this harness cannot resolve a 5% effect, so there is no speed claim
about PGO in this repository, only a route that is known to run and a cost that is known (23× build
time, +12.5% size). Resolving it needs more reps, not a more favourable reading of these three.

**The worker count, by contrast, reproduced — and it is the bigger term by far.** Same binary, one
session, arms rotated:

| shape | delivered rps | µs CPU/req (median) | verdict |
|---|---|---|---|
| `AETHER_WORKERS=1`, 5,000/s offered | 4,999.5 | 84.4 | exit 0 |
| `AETHER_WORKERS=8`, 5,000/s offered | 4,999.0 | 251.2 | exit 0 |
| `AETHER_WORKERS=1`, 20,000/s offered | 17,699 | 56.0 | exit 2, not delivered |
| `AETHER_WORKERS=8`, 20,000/s offered | 19,939 | 118.5 | exit 0 |

At a rate both arms can hold, eight workers cost **3.0× the CPU per request for no extra
throughput** — the process' CPU time is the sum over all worker threads, so wakeups and work-stealing
are charged, not hidden. Near saturation, eight workers did deliver more (19,939/s at exit 0 while
one worker answered 17,699/s and fell short). So the worker count is a real trade: headroom under
load, paid for in CPU per request. That is the honest version of track 62's claim — the setting now
reaches the runtime, and its effect is measured rather than assumed.

**What is *not* established: any absolute rps ceiling on this box.** The 17,699/s and 19,939/s above
are what a co-located client got at 128 connections, and the same server shape got **worse when the
pool grew** — 14,707/s at 256 connections, and one 1-worker arm collapsed to 8,522/s at 512 with
`latency p50` 39 ms and `lag p95` 6.2 s. That is §4's exit-2 attribution defect surfacing, and it is
why a number that moves with the client's shape cannot be printed as a server ceiling. The direction
(8 workers above 1 near saturation, 1 worker cheaper per request below saturation) held across the
pool sizes; the magnitudes did not.

**Why the two effects were not combined.** The owner's ladder offered a combined run "if it is worth
it". It is not: the PGO effect is inside the same-session noise floor, the worker effect is a factor
of three, and no build flag moves a structural 3× — so a combined run would spend 69-minute rebuilds
to re-measure a number that cannot resolve the smaller of the two terms. The next useful measurement
is more reps at one shape, or the same comparison with the generator on another host.

No throughput number in this section is a project result. The commands that remake every row are in
`framework/tools/pgo.ps1` and `framework/tools/pgo_ab.ps1`; the owner runs them on his own machine.
