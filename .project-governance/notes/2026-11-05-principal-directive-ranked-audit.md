# Principal directive record and ranked problem audit — 2026-11-05

## 1. What was recorded, and where

The owner supplied a principal directive on 2026-11-05 ("دون للموجه الريسي لك") that fixes the
order of work: (1) fix the major problems the measurements identify; (2) code quality; (3) security
and reliability; (4) performance, only with fair, reproducible, owner-measured evidence; (5)
features and innovation last, kept out of the current change set.

- Verbatim text: `.project-governance/OWNER_VISION.md`, "Principal directive (owner-supplied
  2026-11-05)".
- Short binding summary: `AGENTS.md`, "Principal directive (owner-supplied 2026-11-05)".
- It adds an order of work; it does not relax the 2026-10-24 rulings (owner measures, independent
  reviewer per cycle, `C:/tmp` isolation) or the standing prohibitions (no push, no amend, no
  history rewrite, no merge or out-of-scope change without approval).
- **Parked by priority 5:** wiring `aether-validation` into route declaration so one declaration
  yields both a structured `400` and an OpenAPI `requestBody`, and a built-in latency-percentile
  metric. Both are proposals, not work. `aether-validation` is still not a dependency of `aether`
  (its only dependent is `framework/tests`, `framework/tests/Cargo.toml:12`).

## 2. Audit method, and the evidence class of every row

Four evidence classes are kept apart, because the register already burned itself once by mixing
them:

- **[CODE]** read in this tree today.
- **[READING]** an instrument reading taken here today (allocation counters, `cargo fmt`, gate
  logs).
- **[RECORDED]** a dated measurement or decision already in the governance record, not re-run here.
- **[OWNER]** a measurement or decision only the owner can supply.

Nothing in this note is a new performance number, and nothing here authorises a code change that a
profile has not named.

## 3. The ranked list

### Tier A — the directive's priority 1, blocked on an owner reading

**A1. The track-122 lock changes are merged and unmeasured.** [RECORDED]
The register's track-122 section states plainly: "No average, P99, rps, CPU or memory number was
produced here; the owner's run decides whether each change is kept." Four release arms were built
and still exist (`framework/target/ab/t122-base.exe`, `t122-admit.exe`, `t122-dateonly.exe`,
`t122-date.exe`), with the two `pgo_ab.ps1` commands recorded beside them. This is the only item in
the repository where code has already changed and the evidence that justifies it has not been
taken. Nothing else in the tree has a stronger claim on "first".

**A2. The Actix gap is unattributed, and no fair comparison is re-runnable from this repository.**
[RECORDED] The readiness report calls the three-framework comparison inconclusive with 33–59 %
run-to-run noise (`PRODUCTION_READINESS_REPORT.md:88-90`). `BENCHMARKS.md` §2 prints a 14-scenario
table that its own notice says was not produced in this environment and must not be cited
(`BENCHMARKS.md:1-16`). The in-repo harnesses (`pgo_ab.ps1`, `workers_sweep.ps1`, the open-loop
generator) compare one Aether binary against another; they have no Actix arm. The core objective
therefore cannot be verified from this tree. This is a benchmark-setup defect before it is a
performance question.

**A3. The static-file payload is a userspace read-then-write loop, not zero-copy.** [CODE]
`STATIC_CHUNK` is 64 KiB (`static_dir.rs:25`) and the body is read into a buffer and written out
(`static_dir.rs:258-260`); no `sendfile`/`TransmitFile` call exists. Track 113 corrected the
documents that called it `sendfile`, so the mechanism claim is already honest — the efficiency gap
is not measured. It stays at rank 3 until an A/B on a large static file prices it.

### Tier B — agent-actionable now (no profile and no owner gate)

**B1. `BENCHMARKS.md` §3 still states mechanisms the tree does not have.** [CODE] **(closed in this note's parent commit — see §6)**
Line 46 says "the request-scoped bump arena reduces allocator lock contention to zero"; the arena
exists in `aether-core` but is not on the serving path (register B19, matrix row "Request arena",
`ARCHITECTURE.md:8`). Line 48 presents "monomorphic code generation" for validation as the reason
for the table's speedup; `aether-validation` is not wired into `aether` at all. The notice at the
top covers the provenance of §1 and §3 but not the falsity of these mechanism claims, and track
113's sweep skipped this file because it carries a snapshot marker. Fix: rewrite §3 as observations
about the unverified run and name what the tree actually does; keep the historical table and its
marker untouched.

**B2. The Arabic error guide names a "SIMD Validator" for `E4002`.** [CODE] **(closed in this note's parent commit — see §6)**
`ERROR_HANDLING_GUIDE_AR.md:37`. `validate_ascii_fast` is a 64-bit word scan
(`validation/src/simd.rs`), the matrix already says "SIMD-style … not in the HTTP hot path", and
the error can be raised by JWT validation and application code too. Fix: drop the mechanism name
from the table cell, or name the scan accurately.

**B3. Register D4's standing fmt-debt line is stale.** [READING]
D4 says "`cargo fmt` is not a gate". Today, from `framework/`, `cargo fmt --all --check` exits 0
with 0 `Diff in` lines over 0 files, and `gates.sh:80` runs that exact command as the first gate.
The debt was closed by track 73's fmt half; D4's historical readings stay, but the line must say
so, or it will be read as a live problem the next time someone audits the tree.

### Tier C — owner decisions, not agent work

**C1. PROB-0005, two header stores.** [CODE] `RequestParts` carries `pub headers` and
`pub header_multi` (`app.rs:326-327`); `RequestContext` carries `pub headers` and a private
`header_multi` (`app.rs:420-421`), both built from one call (`app.rs:2614`). One copy per request
and one drift hazard. The fix is a breaking public-field change, so the release-shape decision
gates it.

**C2. PROB-0011, builder style split.** [CODE] `get`/`post` take `&mut self` and return
`&mut Self` (`app.rs:1161`, `:1184`) while `tls`/`require_tls` consume `Self` (`app.rs:1850`,
`:1880`), so a single chained expression cannot compile. Breaking, and therefore the owner's.

**C3. PROB-0012, no Linux/macOS run.** [RECORDED] Every green result is Windows-only; eleven of
thirteen crates type-check against the Linux target, the two that fail do so inside `ring`'s C build.
Owner-gated environment.

**C4. PROB-0004, the scheduler comparison tables are historical.** [RECORDED] The owner paused the
sweep; regenerating them needs pre-declared confirm/refute criteria.

### Tier D — hypotheses that must not be started

E1 (route-lookup shape), E3 (per-request buffer reuse), E6 (WebSocket reassembly/pooling), F-19
(request arena / zero-copy head / SIMD in the hot path) and F-20 (thread-per-core) all need a
profile that names a stage before any code moves. Register E's guardrails apply unchanged:
closed-loop p50 against concurrency is Little's law; client shape flips results on this box;
release-vs-release, same session, repeats, rotated order, null arm.

## 4. What this audit does not do

- It makes no code change and no optimization claim. The ranking is an argument about where the
  next measurement belongs, not a licence to edit the hot path.
- It does not re-open any refused alternative (register E5's scheduler sharding remains refuted).
- It does not claim the Actix comparison is close or far: the only honest statement today is that
  it is unattributable from this repository.

## 5. The next action the directive implies

1. Hand A1 to the owner unchanged — the two `pgo_ab.ps1` commands are already written down in the
   track-122 record and the four arms are on disk. Until that reading exists, no claim attaches to
   the lock changes.
2. Fix B1–B3, each small enough to review alone, with the gates re-run afterwards.
   **(already done when this note was committed — §6.)**
3. Only then consider an A3 measurement arm. A2 (a comparison harness that lives in this
   repository) is a separate decision because an external bench repository already exists; it is
   not started here.

## 6. Correction, same day: Tier B was already empty

This note (`989f488`) was committed four seconds after `53e02e8` ("docs: stop two live pages from
teaching mechanisms the tree lacks"), but it describes the tree as it stood before that commit.
Read against `989f488`:

- **B1 is closed.** `framework/docs/BENCHMARKS.md:45-59` no longer asserts the arena, the bitmask
  router or monomorphic validation; it keeps the three observations as the unverified run's, each
  beside what this tree does. Re-checked here: `RequestArena` exists in `core/src/arena.rs:24` and
  `aether/src/lib.rs:17-18` says it "is not used by the default server path"; `aether/Cargo.toml:9`
  depends on `aether-core` and has no `aether-validation` line.
- **B2 is closed.** `framework/docs/ERROR_HANDLING_GUIDE_AR.md:37` now reads that `E4002` comes
  from explicit application validation or `aether-validation` with "no vector scan in the HTTP
  path". Re-checked: `validation/src/simd.rs:1-8` states the module name is historical and the scan
  is one 64-bit mask over eight bytes.
- **B3 was closed in the commit that carries this note** (register D4's closing paragraph).

So "Fix B1-B3" in §5 was not the next action; it was the previous one. The rows above are kept as
written and annotated rather than rewritten, because the misreading they produced is the finding:
an audit committed in the same breath as its own fixes still described the pre-fix tree. **The
agent-actionable list is empty**: every remaining Tier A and Tier C item is an owner measurement or
an owner decision, and Tier D stays barred until a profile names a stage.
