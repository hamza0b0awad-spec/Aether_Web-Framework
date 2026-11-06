# Native code (C / C++) at a measured boundary - owner directive 2026-11-05

The owner's instruction, recorded so it is not lost: keep the option of writing a component in
modern C or C++ on the table for the place where it would make a large, measurable difference and
deliver high performance. This is **not** a licence to rewrite Aether, and no component moves
without the evidence rules already in force.

## What it is

A targeted native kernel behind a narrow, stable FFI boundary, vendored or linked as a dependency,
called only where a profile shows the stage dominates. Aether stays a Rust framework; the native
code is a leaf, not an architecture.

## When the option becomes real

All four must hold, and each is checked in this order:

1. **A measured bottleneck.** A stage's cost is shown by the owner-run measurement, not by reading
   the code. A plausible hot spot is a candidate, not a finding.
2. **A pure kernel shape.** The work is a stateless, data-in/data-out kernel with a narrow ABI:
   parsing, compression, hashing, allocation, encoding. Control flow, async, I/O orchestration and
   anything that would need a call across the boundary per request keep their Rust implementation.
3. **A priced boundary.** The FFI copy or serialization cost is measured and is smaller than the
   gain. A kernel that wins in isolation and loses after the boundary is not a win.
4. **A safe Rust surface.** No new `unsafe` reaches Aether's public API; the wrapper owns the
   invariants, and the native code is pinned, licensed, reproducible and covered by the same tests.

## Candidate classes, not commitments

- SIMD JSON parsing (for example simdjson) - only if a JSON-heavy profile shows parsing dominates
  and the boundary can avoid a second copy.
- Compression (libdeflate / zlib-ng) - only for a measured compression path, with the same
  wire-compatibility tests as the Rust implementation.
- Allocator (mimalloc) - already scheduled as an independently feature-flagged arm; it is linked
  native code, and its A/B decides it.
- Hashing and cryptographic primitives - already reached through Rust bindings; no new boundary
  unless a measured profile asks for one.

## Rules

- No rewrite in C or C++ as a goal in itself. The language is not the result; the measured number
  at realistic load is.
- No performance claim without the owner's A/B, at realistic load, reading P50, P99, achieved rps,
  CPU per request and stability together. A microbenchmark alone does not merge a change.
- No default flip before that A/B, and every native arm stays behind an independent feature flag
  with the default build unchanged.
- A failed gate is recorded as failed; a native component that cannot be built reproducibly on
  this platform is not adopted on the strength of another platform's number.

## Status

Recorded, not scheduled. The order that produces the evidence first is unchanged: close the proven
lock work, count real allocations per request, then run the allocator A/B. A native kernel is
revisited only when one of those measurements names it.
