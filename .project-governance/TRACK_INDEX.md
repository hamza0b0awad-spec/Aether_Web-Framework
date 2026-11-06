# Track index

Added 2026-10-27. Start here instead of reading `WORKING_CONTEXT.md` end to end: that file is more
than 4,300 lines and its headings stop at `## Last Update`, after which it is one append-only
section. The index below names, for every track, the file and section that actually records it.

## Current state, in reading order

0. `bash gates.sh` — one command runs all eight gates and exits with the truth; run it before
   believing anything below. The gate logs it writes are the only figures this index trusts.
1. `problems.json` — the open set, each item carrying the evidence that keeps it open. As of
   2026-11-06 (track 143) it holds PROB-0004 (the owner-paused comparison sweep) and PROB-0012 (no
   Linux/macOS run); PROB-0005 and PROB-0011 were closed by the 0.2 release, and PROB-0009's
   residual was closed by track 83. Read the file, not this sentence, when the sets disagree.
2. `notes/2026-10-26-remediation-roadmap.md` — the sequenced plan and the gate table (§0).
3. `notes/2026-10-21-improvement-register.md` — the per-track record (`## Track N`) and the
   concern register (A–G).
4. `WORKING_CONTEXT.md` — the historical log. The early headings run to `## Active Track 15`;
   `## Last Update` is the last heading, and everything after it is one append-only section.

A stage is finished when it has all four: the ladder run recorded in that section, one
`## Track N` section in the register, one row in the table below, and — if it closed a problem —
`done PROB-XXXX`. A stage that skips one of the four is not a stage; it is a note.

## Where each track is recorded

Pointer rule (track 95): a growing file is pointed at by quoting its heading, never by a line
number. `test_track_index.py` reads this table against the three files and fails on a quoted
heading that is absent or ambiguous, on an unindexed `## Track`, and on a line-number pointer.
`WORKING_CONTEXT.md` is frozen, so its rows keep numeric ranges — and both ends of each range are
checked against the heading the row says sits there. The consequence, learned the hard way in
track 107: an edit to `WORKING_CONTEXT.md` must not change its line count — one pointer sentence
that wrapped from one line to two moved `## Active Track 10` off line 268 and failed `gates.sh`.

| Tracks | Where | How to find it |
|---|---|---|
| 1–10 | `WORKING_CONTEXT.md` headings | lines 6–268 (`## Active Track` … `## Active Track 10`) |
| 11–15 | `WORKING_CONTEXT.md` headings | lines 332–553 (`## Track 11` … `## Active Track 15`) |
| 16–68 | `WORKING_CONTEXT.md`, `## Last Update` | one dated paragraph per track; `grep -n "(track N)" WORKING_CONTEXT.md` |
| 69 | register heading | `## Track 69` |
| 70 | register heading | `## Track 70` |
| 71 | register heading | `## Track 71` |
| 72 | register heading | `## Track 72` |
| 73 | register headings | `## Track 73, slice 1`, `## Track 73, slice 2`, `## Track 73, slice 3` |
| 74–75 | roadmap §3–§4 plus the dated paragraphs in `WORKING_CONTEXT.md` | no register heading of their own |
| 76–78 | not executed | roadmap §5–§7 |
| 79 | register heading | `## Track 79`; roadmap §12 mirrors it |
| 80 | register heading | `## Track 80`; roadmap §11 mirrors it |
| 81 | register heading | `## Track 81`; roadmap §10 mirrors it |
| 82 | register heading | `## Track 82`; roadmap §13 mirrors it |
| 83 | register heading | `## Track 83`; roadmap §14 mirrors it |
| 84 | register heading | `## Track 84`; roadmap §15 mirrors it |
| 85 | register heading | `## Track 85`; roadmap §16 mirrors it |
| 86 | register heading | `## Track 86`; roadmap §17 mirrors it |
| 87 | register heading | `## Track 87`; roadmap §18 mirrors it |
| plan 88–109 | roadmap heading | `## 19.` — the open-problem inventory; each row becomes a register heading when executed |
| 88 | register heading | `## Track 88`; roadmap §19 D-01…D-06, D-14…D-17 |
| 89 | register heading | `## Track 89`; roadmap §19 D-07…D-10, D-18 |
| 90 | register heading | `## Track 90`; roadmap §19 D-11 |
| 91 | register heading | `## Track 91`; roadmap §19 F-01, F-02, D-19 |
| 92 | register heading | `## Track 92`; roadmap §19 D-12 |
| 93 | register heading | `## Track 93`; roadmap §19 D-13 |
| 94 | register heading | `## Track 94`; roadmap §19 F-03 |
| 95 | register heading | `## Track 95`; roadmap §19 F-04 |
| 96 | register heading | `## Track 96`; roadmap §19 F-05, F-06 |
| 97 | register heading | `## Track 97`; roadmap §19 F-07, F-08 |
| 98a | register heading | `## Track 98a`; roadmap §19 F-14 (row 98, F-12 still open as 98c) |
| 98b | register heading | `## Track 98b`; roadmap §19 F-13 (row 98, F-12 still open as 98c) |
| 98c | register heading | `## Track 98c`; roadmap §19 F-12 (row 98, closes the row) |
| 99 | register heading | `## Track 99`; roadmap §19 F-09, F-10 (row 99, closes the row) |
| 100 | register heading | `## Track 100 — a body`; roadmap §19 F-11 (row 100, closes the HTTP/1.1 half; the h2 half is 100b) |
| 100b | register heading | `## Track 100b`; roadmap §19 F-11 (row 100b, closes the HTTP/2 half and the F-11 row) |
| 101a | register heading | `## Track 101a`; roadmap §19 F-15 (row 101, closes the CRL half) |
| 101b | register heading | `## Track 101b`; roadmap §19 F-15 (row 101, closes the OCSP half and the F-15 row) |
| 102a | register heading | `## Track 102a`; roadmap §19 F-16 (row 102, closes the property-test half) |
| 102b | register heading | `## Track 102b`; roadmap §19 F-17 (row 102, closes the fuzz half and the row) |
| 103 | register heading | `## Track 103`; roadmap §19 F-18 (row 103, closes the row) |
| 104 | register heading | `## Track 104`; register B22 (closed) |
| 105 | register heading | `## Track 105`; no register item and no roadmap F-item named this defect |
| 106 | register heading | `## Track 106`; closes what `## Track 105` named unverified — the four request-target forms over HTTP/2, the `:authority`-over-`Host` fold on the wire — and re-runs the F-17 fuzz figure over `classify_target` |
| 107 | register heading | `## Track 107`; roadmap §19 D-15's follow-through — closes the matrix *Nested routers / independent sub-apps* row |
| 108 | register heading | `## Track 108`; roadmap §19 — closes the *OpenAPI generation* row track 91 left partial (derive the document from `RadixRouter::route_listing`) |
| 109 | register heading | `## Track 109`; roadmap §19 row 109 (no F-item and no register B-item named this defect - the defect was that nothing could name it) |
| 110 | register heading | `## Track 110`; roadmap §19 row 110 — the request `content-length` promise over both frontends, the body rule that tracks 105 and 106 left unexamined |
| 111 | register heading | `## Track 111`; roadmap §19 row 111 — the AI-generated site, the two zero-dependent crates and the twelve unused dependency declarations removed from the tree |
| 112 | register heading | `## Track 112`; roadmap §19 row 112 — the measurement path the owner had to walk by hand: the shared arm-identity rules, the worker-count sweep, and the two documented commands that could not run as written |
| 113 | register heading | `## Track 113`; roadmap §19 row 113 — live documents that named a mechanism the tree does not have: static-file `sendfile`, SIMD instructions, io_uring/epoll and a `SO_REUSEPORT` run environment |
| 114 | register heading | `## Track 114`; roadmap §19.4 F-21 (new row), §19.7 row 114 - signed cookie values, the *Cookies* row's integrity half |
| 115 | register heading | `## Track 115`; roadmap §19.7 row 115 - the signed cookie read over the second frontend: five HTTP/2 wire tests for `RequestContext::signed_cookie`, on cleartext h2c and on TLS with ALPN `h2` |
| 116 | register heading | `## Track 116`; roadmap §19.7 row 116 - `Expect` honoured on both frontends, plus PROB-0034 (a paused buffered body earns no `408`) registered out of the track's own probe |
| 117 | register heading | `## Track 117`; roadmap §19.7 row 117 - the receive budget answers a buffered body that stops arriving: two clocks, one `408` helper, four wire tests, six mutants killed, PROB-0034 closed |
| 118 | register heading | `## Track 118`; roadmap §19.7 row 118 - the developer-facing diagnostic stopped describing a runtime `Io` as a bind failure: four arms corrected, twelve variants cross-checked against the wire contract, eleven mutants killed |
| 119 | register heading | `## Track 119`; roadmap §19.7 row 119 - every printed suite and citation count checked against the run that produced it: 31 number-free pins over the five live documents, a stale `app.rs` count corrected by the recount, 18 mutants caught, one SHOULD-FIX review applied, the ladder now eight gates |
| 120 | register heading | `## Track 120`; roadmap §19.7 row 120 - the completeness matrix's *Bounded response memory* cell contradicted its own evidence cell: row flipped to `VERIFIED_IMPLEMENTED`, every printed count re-derived (123 / 3 / 8 = 136), `count_check.py` extended to recount the matrix's own status rows and pin the summary in three documents (52 pins over six documents), 48 tests, 18 document mutants plus three code mutants killed, one SHOULD-FIX review applied |
| 121 | register heading | `## Track 121`; roadmap §19.7 row 121 - the crate count, the evidence corpus and the running-lines cross-check pinned to their sources: two new pin kinds, five pins (52 -> 57), 60 tests, fourteen mutants killed |
| 122 | register heading | `## Track 122`; roadmap §19.7 row 122 - the admission double lock and the Date-cache mutex removed, four A/B arms built for the owner, and `gates.sh` now refuses a concurrent run after a spliced log produced 22 false findings |
| 123 | register heading | `## Track 123`; roadmap §19.7 row 123 - content negotiation answers `406` when identity is refused, every CORS refusal carries `Vary`, and `serve.rs` keeps the preflight middleware's `Vary` on a rebuilt `403` |
| 124 | register heading | `## Track 124`; roadmap §19.7 row 124 - the stage ladder rebuilt on this tree: every rung re-priced (parser +0.00 after G15, header materialization +18.00 still the largest, route lookup +0.00), the scheduling rung matching the production mirror at +2.00 allocs / +64 B, a runner denominator that had inflated every production arm by 6.67 %, and a decision of no framework change because no stage was named |
| 125 | register heading | `## Track 125`; roadmap §19.7 row 125 - five HTTP/2 wire pins for track 123's CORS-refusal and identity answers, the approval side included, with six injected mutants |
| 126 | register heading | `## Track 126`; roadmap §19.7 row 126 - the plain `cookie()` accessor reads every `Cookie` field line instead of the first, two wire pins (HTTP/1.1 and h2c) red before the fix, three injected mutants, case sensitivity pinned on HTTP/1.1 only |
| 127 | register heading | `## Track 127`; roadmap §19.7 row 127 - the decoded query lookup applies the form rule to the key as well as the value, so a browser-encoded name (`first+name`) is found; a borrowing `form_decode_cow` added, one wire pin and one unit pin red before the fix, three injected mutants killed, and the `+`-before-percent order pinned with `%2B` |
| 128 | register heading | `## Track 128`; roadmap §19.7 row 128 - the static root already refused a junction that leaves it; a wire pin now proves it (out -> 404 empty body, the link's directory -> 404, a link that stays inside -> 200), with two injected mutants of the containment check killed and no production change |
| 129 | register heading | `## Track 129`; roadmap §19.7 row 129 - track 117's TLS `408` gap closed by one wire pin: two stopped shapes answered at the budget, `Connection: close` and a clean `close_notify`; four mutants, one of them TLS-only |
| 130 | register heading | `## Track 130`; roadmap §19.7 row 130 - adjacent multipart delimiter lines panicked the handler on a 10-byte body; the part end is clamped at the cursor so the empty part takes the existing header-block refusal, with a four-shape unit pin, a wire 400 pin, two mutants and a limited independent PASS review |
| 131 | register heading | `## Track 131`; roadmap §19.7 row 131 - a compressed body kept the identity representation's `ETag`, integrity digests and `Accept-Ranges`; one helper strips them on the transform path and the bodyless `406`, `Last-Modified` stays, three unit pins + one wire pin, five mutants killed, a limited independent PASS review and a repaired vacuous incompressible fixture |
| 132 | register heading | `## Track 132`; roadmap §19.7 row 132 - an empty JWT signing secret was accepted by `JwtValidator::new` and verified every token minted under the same empty key; the constructor now panics on emptiness (not length), `BearerAuth::new` inherits it, three unit pins, three mutants killed, a limited independent PASS review, and the six count documents plus three teaching docs re-derived |
| 133 | register heading | `## Track 133`; roadmap §19.7 row 133 - the `Json` extractor compared the `+json` suffix case-sensitively while comparing the exact type case-insensitively, so `application/vnd.api+JSON` was answered `415`; an allocation-free case-insensitive suffix check, two new pins, four widened rejection rows, seven mutants killed, a SHOULD-FIX review applied, and the six count documents re-derived |
| 134 | register heading | `## Track 134`; roadmap §19.7 row 134 - the h2 header-list cap truncated a configured budget above 2^32 (4 GiB became 0, so `h2` answered 431 to every request while HTTP/1 accepted the same heads); `header_list_cap` saturates at `u32::MAX`, two unit pins and two mutants, no independent review, and the six count documents re-derived |
| 135 | register heading | `## Track 135`; roadmap §19.7 row 135 - the h2 graceful-drain test slept instead of synchronizing with the server having received the request, so `GOAWAY`/`NO_ERROR` could fail it; it now waits for the handler to start, no production line changed, and the flake could not be reproduced on demand |
| 136 | register heading | `## Track 136`; roadmap §19.7 row 136 - the desync audit refused a repeated `Transfer-Encoding` and any coding that is not exactly one `chunked`, though the parser refuses both: the rule set aligned with `body_framing`, six unit pins (three red first), two more facade shapes, five killed mutants |
| 137 | register heading | `## Track 137`; roadmap §19.7 row 137 - a padded JWT signature segment verified as a second spelling of the same token; a canonical 43-character tag length is enforced before decoding, one unit pin + one facade assertion, three killed variants and two surviving-by-equivalence, and the six count documents plus the moved `jwt.rs` citation re-derived |
| 138 | register heading | `## Track 138`; roadmap §19.7 row 138 - the h2 mid-stream ceiling test expected every client write to succeed, though a server reset after the read fails legitimately refuses one; it now stops at the first refusal and requires more than 64 bytes enqueued, so the crossing precedes any reset |
| 139 | register heading | `## Track 139`; roadmap §19.7 row 139 - the CSRF gate ran above the host allow-list and the rate limiter although it is the only one of the three doing cryptographic work; host and rate rejections now fold into one `rejection` the CSRF block is skipped on, the 403 chained after them, the false step-3 comment corrected, one wire pin (csrf target 9 -> 10) whose combined mutant proves precedence, and the seven moved coordinates plus five pins re-derived |
| 140 | register heading | `## Track 140`; roadmap §19.7 row 140 - the CSP nonce was drawn in the request prologue, so every framework answer paid an OS-entropy draw and an entropy failure turned a 429/400/403/404 into a 500; the draw now happens only in `MatchResult::Found` behind a reachability predicate, framework answers carry the policy with the nonce expression removed, two existing tests extended (h1 404/429, h2 upgrade 400), two killed mutants, five coordinates and their pins re-derived, census unchanged |
| 141 | register heading | `## Track 141`; roadmap §19.7 row 141 - a default `static_dir` answered `404` for a directory holding `index.html` because the lookup sat inside the listing branch, and the tree root had no route unless listings were on; the index is served in either mode from the directory resolution, the prefix route now exists when the listing is on or the root ships an index at registration, one wire pin (http_e2e 101 -> 102) red first at `/assets/`, two mutants killed on distinct paths, 21 coordinates and their pins re-derived, census 942 passed / 0 failed / 1 ignored with integration 429 |
| 142 | register heading | `## Track 142`; roadmap §19.7 row 142 - the HTTP/1 head parser applied the buffered `max_body_bytes` ceiling to a declared `Content-Length` before routing, so a streaming route refused every declared upload over 2 MiB although the h2 frontend and the config both say `max_stream_body_bytes` governs it; the ceiling is now the caller's parameter, the streaming head passes none, and the route's pre-dispatch refusal remains; one unit + one wire pin red first, three killed mutants, nine coordinates re-derived, census 944 passed / 0 failed / 1 ignored |
| 143 | register heading | `## Track 143`; roadmap §19.7 row 143 - the 0.2 bounded breaking pass closed PROB-0011 (38 `AetherApp` builders now take `self` and return `Self`) and PROB-0005 (one private header store behind `header`, `header_values` and `headers`, the public map removed, both frontends built from it), bumped the workspace to 0.2.0 with all 16 members inheriting it, and added `docs/11_migration_0_1_to_0_2.md`; one `compile_fail,E0609` doctest plus a `no_run` example took the doc lane 19 -> 21 over 15 runs, the count gate learned cargo's merged `compile_fail` doctest binary, 35 citations and 27 pins moved, and the census is 946 passed / 0 failed / 1 ignored with integration 430 and doc 21 |
| 144 | register heading | `## Track 144`; roadmap §19.7 row 144 - the post-release audit found four live documents still teaching the 0.1 public header map, all four were corrected against `app.rs:394-428`, no code or test changed, the suite held at 946 passed / 0 failed / 1 ignored, and the name pass re-derived to 390 qualified / 1455 bare names |

## Rule from 2026-10-27

A track is recorded **once**: a `## Track N` section in the improvement register. The roadmap row is
the plan, and the index above is the pointer. No new narrative is added to `WORKING_CONTEXT.md`; it
stays frozen as the historical log, which is why its line numbers are quoted above and not rewritten.
