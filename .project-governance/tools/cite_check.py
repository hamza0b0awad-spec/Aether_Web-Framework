#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Do the `path:line` citations in the docs still hold the code they name?

Track 80 measured this repository's human-facing documents against its own source tree: 61 citations
in 10 of the 14 files under `docs/` pointed at 40 distinct spans of Rust, and roughly a quarter of them
no longer held what the prose claimed - a `trie.rs:84` that is a closing brace, an `app.rs:1946-1950`
that is a different branch of the same loop, a `parser.rs:262` inside `parse_head_into` where the
prose names `parse_request_incremental`. A reader with the file open cannot tell those from correct
ones, and nothing in the gate ladder noticed.

This file is the check that notices. It reads every citation in `docs/**/*.md`,
`framework/docs/**/*.md` and both READMEs, resolves the file, requires the span to be inside the
file, and then requires the span's first line - and, when the span is a range, its last line too -
to match a regular expression pinned to that exact span, holding something the surrounding prose
names. A citation that resolves to an unpinned span fails too, so a coordinate may not be edited in
the docs without a human deciding what the span is supposed to contain - the pin table is the review
record. After track 80's sweep of the root set: 66 citations over 45 spans, and 36 of the original
coordinates replaced with the span their sentence describes (8 of the 14 documents).

Track 92 extended the sweep to the teaching set under `framework/docs/`: 27 citations over 24
spans, 21 of them spans the root set never cited; 10 coordinates were corrected, and a
comma-separated pair (`error.rs:33,37`) rewritten as two citations the parser can read. Track 98c
taught the CSRF layer in `docs/04` and `docs/09` and pinned the five spans those sections name.
Track 104 made the production readiness report a live document — it had carried the snapshot marker
while later tracks edited its maintained sections — so the census became 106 citations over 71 distinct
spans in 16 live documents. Track 110 moved that census to 107 over 72 and left this sentence behind;
on the track-111 tree the gate re-measures 107 citations over 72 distinct spans in 16 live documents.
Track 109 brought both READMEs inside that set. They are the repository's first sentence and they
state its loudest claims (the suite totals, the crate count, `AetherApp::enable_h2c`,
`AetherApp::openapi_spec`) while sitting outside every gate, so the one page a reader is certain to
open was the one page nothing checked. They carry no coordinate, which is why the citation census did
not move; the corpus did - 27 live documents plus the 8 dated records below. `docs/*.md` became
`docs/**/*.md` in the same pass so that a document added under a future `docs/` subdirectory cannot
join one gate's set and stay invisible to the other's, and `evidence_check.py` reads the identical
list (a test asserts the two constants agree).

A dated record is the one legitimate exception. It states the tree as it stood on its date, so its
coordinates cannot be re-checked against today's source, and "fixing" them would falsify the
record. Such a document must be named in `SNAPSHOTS` *and* carry

    <!-- cite-check: snapshot 2026-10-18 -->

as its first line (after any BOM). Both halves are required so that exemption is a reviewed edit:
a listed document without its marker fails, a marker whose date disagrees with the list fails, and
a document that is not listed may not carry the marker at all. The eight current snapshots hold 49
further citations, reported separately and never validated.

Two citation forms exist and both are read:

    `framework/http/src/parser.rs:647`      a file and a line (or an `a-b` / `a–b` range)
    `:664`                                  a bare line, inherited from the file named
                                            earlier on the same line

Bare coordinates are inherited per line only. A bare `:NNN` on a line that names no file is a
finding rather than a guess, because guessing is how a citation silently comes to point at the
wrong file. Write the file explicitly when the line also names a different one - the inheritance
follows the *last* file token on the line, not the one the sentence is about.

The pin table is keyed by the *resolved* path plus the span, so a doc may write the short form
(`app.rs:1771`) or the full one and both are checked against the same pin. A multi-line pin states
what the span's first line and its last line must say, so an insertion anywhere above the span fails
(the first line moves) and an insertion *inside* it fails too (the last line moves): this
repository's "re-derive both ends" rule, mechanised. A single-line pin states its one line.

Run it from anywhere:

    python -B .project-governance/tools/cite_check.py
    python -B .project-governance/tools/cite_check.py --list      # print the whole census

Track 109 added a second pass to the same gate: a citation can point at a real file and a real
line and still teach an API that does not exist, because the span's *prose* names a symbol while
the *anchor regex* only checks the line's shape. `framework/docs/ERROR_HANDLING_GUIDE_AR.md:178`
promises the client a redacted JSON body carrying `code` and `error_id`; the response built at
`framework/aether/src/app.rs:254` has `error` and `code` and no `error_id`, and the sentence's own
citation anchor stayed green. So the gate now builds an index from the tree's own sources (Rust
declarations plus their impl methods, enum variants and struct fields; Python definitions; every
string literal; every Cargo target name with hyphens mapped to underscores; every test/bench/example
file stem; the Node interop scripts) and reads every code span in the live documents against it.
Two rules, each deliberately narrow so that it only claims what it can see:

    `Head::leaf`    checked when `Head` names something this tree declares - a type, a crate, a
                    module. Then `leaf` must be a name the tree carries. A head the index cannot
                    see - `tokio`, `h2`, `std`, `bytes` - is not this repository's API and is not
                    judged, and neither is that same leaf when it appears alone in the *same* span
                    (`clippy::unused_io_amount`), because the span already said which crate owns it.
    `snake_name`    checked in every other span: the document asserts a name, and the tree must
                    carry it. A bare name is this repository's claim even beside an external call,
                    which is why a lint named alone (`type_complexity` in the readiness report's
                    clippy row) sits in `NAME_EXCEPTIONS` with its reason.

A span inside a fenced code block is the document's own example, not a claim about the tree, so
fences are skipped. A span with a slash is a path - the citation pass and `evidence_check.py` already
resolve files, and judging `fuzz_targets` as a name would only repeat their verdict in the wrong
vocabulary. A name the index cannot resolve needs a human decision: either the document is
wrong (fix it) or the mention is legitimate - a negation, a name the tree carried at a past commit,
an external crate's API, a design proposal - and goes into `NAME_EXCEPTIONS` with its reason, which
is the review record, exactly like `PINS`. An exception no longer cited is a finding, so the table
cannot accumulate shelter. Nor can a document shelter itself: the index is built only from the
product's sources under `framework/`, because the sentence that reports `error_id` as missing must
not be the sentence that makes `error_id` exist.

Exit status is 0 only when every live citation resolves in bounds, is pinned and matches its anchor,
*and* every name the live documents assert is either carried by the tree or explained by a recorded
reason. Measured on the tree this track was written against, over the 27 live documents above: 361
qualified and 1256 bare names resolved, 34 mentions sheltered by 28 exceptions, 9 attributed to
another crate, 1 document exempted as a proposal, 0 findings - and one of those 27 documents, the
error-handling guide, was wrong before the pass existed.
"""
from __future__ import annotations

import argparse
import os
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import NamedTuple

REPO_ROOT = Path(__file__).resolve().parents[2]
# Track 109: `docs/**/*.md` so a future subdirectory cannot join one gate's corpus and not the
# other's, and both READMEs, which state the framework's loudest numbers while no gate read them.
# `evidence_check.DOC_GLOBS` must stay identical - a test asserts it.
DOC_GLOBS = ("docs/**/*.md", "framework/docs/**/*.md", "README.md", "framework/README.md")

# `target/` and `.git/` make a basename index slow and can hold generated duplicates of a source
# file, which would turn every short citation into an ambiguity.
SKIP_DIRS = {".git", ".hg", ".svn", "target", "node_modules", "__pycache__", ".venv", "venv"}

FILE_TOKEN = r"[A-Za-z0-9_./-]+\.(?:rs|toml|md|json|jsonl|py)"
ATTACHED = re.compile(rf"`({FILE_TOKEN}):(\d+)(?:[-–](\d+))?`")
BARE = re.compile(r"`(?::(\d+)(?:[-–](\d+))?)`")

# A dated record states the tree as it stood on its date, so its coordinates cannot be re-checked
# against today's source: the code moved on precisely because the record is history. Exempting such
# a document requires naming it here *and* carrying the marker below, because the alternative - a
# document declaring itself exempt - is how a live document would quietly stop being checked.
SNAPSHOT_MARKER = re.compile(
    r"^<!--\s*cite-check:\s*snapshot\s+(\d{4}-\d{2}-\d{2})\s*-->\s*$", re.M
)

SNAPSHOTS: dict[str, str] = {
    "framework/docs/ACTIX_FAIR_COMPARISON.md": "2026-10-18",
    "framework/docs/BASELINE_MEASUREMENTS.md": "2026-10-18",
    "framework/docs/BENCHMARKS.md": "2026-10-18",
    "framework/docs/CONSOLIDATED_FINAL_REPORT.md": "2026-10-18",
    "framework/docs/FINAL_REPORT.md": "2026-10-18",
    "framework/docs/perf-baseline/BASELINE-2026-10-19.md": "2026-10-19",
    "framework/docs/perf-baseline/BASELINE-2026-10-20-SCHED-ROTATION.md": "2026-10-20",
    "framework/docs/perf-baseline/lock-split-2026-10-20/SUMMARY.md": "2026-10-20",
}

# --- the names pass (track 109) ---------------------------------------------------------------

# The sources whose identifiers a document may legitimately name. `.cjs`/`.js` are in because
# `framework/tests/examples/node_h2_interop.cjs` is evidence the documents cite, and prose about it
# names Node's own modules.
NAME_SOURCE_EXTS = {".rs", ".py", ".cjs", ".js", ".mjs", ".toml"}

CODE_SPAN = re.compile(r"`([^`\n]+)`")
PATH_PAIR = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)::([A-Za-z_][A-Za-z0-9_]*)")
SNAKE_NAME = re.compile(r"\b([a-z][a-z0-9]*(?:_[a-z0-9]+)+)\b")
FENCE = re.compile(r"^\s*(?:```|~~~)")

# Heads the index cannot see because they are not this repository's API: standard-library and
# dependency crates, plus the primitive and prelude names that appear on the left of `::`. A
# document that attributes a member to one of these is not claiming the workspace declares it.
EXTERNAL_HEADS = frozenset({
    "std", "core", "alloc", "self", "super", "crate", "test", "proc_macro", "tokio", "bytes",
    "h2", "ring", "clippy", "rustfmt", "serde", "serde_json", "thiserror", "criterion", "rand",
    "libc", "windows", "futures", "hyper", "http", "httparse", "rustls", "rcgen", "num_cpus",
    "actix_web", "axum", "tiny_http", "crossbeam", "parking_lot", "log", "env_logger", "anyhow",
    "String", "Vec", "Box", "Arc", "Rc", "Option", "Some", "None", "Ok", "Err", "Result", "bool",
    "char", "str", "usize", "u8", "u16", "u32", "u64", "u128", "isize", "i8", "i16", "i32", "i64",
    "i128", "f32", "f64", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "Duration", "Instant",
    "Send", "Sync", "Debug", "Clone", "Copy", "Default", "PartialEq", "Eq", "Ord", "PartialOrd",
})

# A design document names the fields and bounds it proposes, not the ones the tree carries; holding
# it to the current index would demand that a plan describe only what already exists. Like a
# snapshot, the exemption needs a listing *and* a marker in the document, so that exempting a live
# document stays a reviewed edit rather than a document declaring itself exempt.
PROPOSAL_MARKER = re.compile(
    r"^<!--\s*cite-check:\s*proposal\s+(\d{4}-\d{2}-\d{2})\s*-->\s*$", re.M
)

PROPOSALS: dict[str, str] = {
    "framework/docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md": "2026-10-18",
}

# (document, name) -> why the tree does not have to carry it. This table is the review record for
# every mention the index cannot resolve, and it is per-document on purpose: a reason that holds in
# one document says nothing about another. An entry whose name no longer appears is a finding, so a
# shelter cannot outlive the sentence it was written for.
NAME_EXCEPTIONS: dict[tuple[str, str], str] = {
    # A sentence that names a function to say it does not exist is the most useful kind of correct
    # documentation, and the names pass cannot tell it from a claim without reading the sentence.
    # Each of these was read, and the reason quotes the sentence's refusal.
    ("docs/00_framework_specification_ar.md", "state_container"):
        "line 164 names it to deny it: لا وجود لحقل عام باسم `app.state_container`",
    ("docs/00_framework_specification_ar.md", "with_state"):
        "the same sentence; `manage` is the real surface and the line says so",
    ("docs/00_framework_specification_ar.md", "with_json"):
        "line 280: `Response::status(..).with_json(..)` do not exist; `json` takes Bytes",
    ("docs/00_framework_specification_en.md", "state_container"):
        "line 164: \"There is no public `app.state_container` field and no `AetherApp::with_state`\"",
    ("docs/00_framework_specification_en.md", "with_state"): "the same sentence, same denial",
    ("docs/00_framework_specification_en.md", "with_json"):
        "line 274: the builders are `json` and `json_status`, and the line says `with_json` is absent",
    ("docs/03_routing_and_radix_trie.md", "allowed_methods_mask"):
        "line 72 names the real field and rejects this one: the node's field is `allowed_methods`",
    ("docs/06_middleware_and_performance.md", "with_logger"):
        "line 17: there is no `with_logger` function and `LoggerMiddleware` is not passed in by hand",
    ("docs/06_middleware_and_performance.md", "base_dir"):
        "the argument of the prose `starts_with(base_dir)` containment check, not a declared name",
    ("docs/08_getting_started_and_examples.md", "with_json"):
        "line 67: `Response::json` takes `Bytes` and there is no `.with_json(..)`",
    ("docs/09_api_reference.md", "req_000000000001"):
        "an example value of `ctx.request_id`, introduced by \"مثل\" (\"for example\")",
    ("docs/09_api_reference.md", "heap_bytes"):
        "the parameter in the document's own signature prose `insert_extension_sized(value, heap_bytes)`",
    ("docs/09_api_reference.md", "status_code"):
        "the parameter in `Response::redirect(location, status_code)`; the status codes follow in line",
    ("framework/docs/DEPLOYMENT.md", "status_request"):
        "the RFC 6066 `status_request` TLS certificate-status extension the stapling answer uses",
    ("framework/docs/FEATURE_COMPLETENESS_MATRIX.md", "with_limits"):
        "line 633 says `AetherApp::with_limits` was NOT added; the bounds arrive through AetherConfig",
    ("framework/docs/FEATURE_COMPLETENESS_MATRIX.md", "validate_outbound_headers"):
        "python-hyper's h2 `H2Config(validate_outbound_headers=...)`, the interop client's own option",
    ("framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md", "max_headers"):
        "lines 120 and 123 quote the superseded text: the field is `max_headers_count`, not this",
    ("framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md", "add_dependency"):
        "line 576: \"neither `DependencyContainer` nor these functions exist in the crate\"",
    ("framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md", "resolve_build_order"):
        "same quoted block; line 577 names what the crate really does",
    ("framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md", "get_users"):
        "a handler name inside the block line 792 quotes as no longer translating",
    ("framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md", "include_bytes"):
        "`std`'s `include_bytes!` macro; the sentence explains there is no such file to embed here",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "a_hand_built_101_from_a_middleware_is_not_an_upgrade"):
        "line 1359 reports its rename to `..._is_refused_with_500`; the tree carried this name at a7be6cb",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "shutdown_aborts_a_live_websocket_session_without_a_close_frame"):
        "lines 559 and 1360 report the test track 86 replaced one for one; present at 4146197",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "a_websocket_route_reached_over_h2"):
        "line 1430 abbreviates `a_websocket_route_reached_over_h2_answers_400` with an ellipsis",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "answers_400"): "the other half of that ellipsis",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "owned_is_unique"):
        "the `bytes` 1.12.1 internal the sentence cites; present in that crate's own test names",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "aether_e2e_probe"):
        "the scratch probe the 2026-10-18 row ran; the paragraph under the table dates it as an earlier round",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md", "type_complexity"):
        "the `clippy::type_complexity` lint name, which no workspace source declares",
    ("framework/docs/PRODUCTION_READINESS_REPORT.md",
     "header_keys_are_lowercased_and_the_first_line_wins"):
        "line 764 records its 0.2 rename to `repeated_lines_share_one_lowercase_key`; "
        "the tree carried the old name before the 0.2 pass",
    ("docs/11_migration_0_1_to_0_2.md", "my_cool_site"):
        "the downstream application, a separate repository outside this tree; the note "
        "names it to report that it was ported and compiled against 0.2",
}

# (resolved path, first line, last line) -> what those two lines must still say.
#
# A single-line citation pins one regex, matched at that line. A multi-line citation pins a
# pair - the first line and the last line - and both must match. Pinning both ends is the
# mechanical form of this repository's "re-derive both ends" rule: the first version pinned one
# line inside the span, so an edit *inside* it moved the span's end while the check stayed green
# and the reader's last line was someone else's code. A pin is either a str (single line) or a
# 2-tuple (multi-line); `check` refuses the mismatch rather than letting a span go half-checked.
#
# Where the pinned line is too generic on its own (a lone `}` or `loop {`), the coordinate in the
# key supplies the specificity and the regex only has to detect that line moving. Weaker ends are
# called out in their comment.
PINS: dict[tuple[str, int, int], str | tuple[str, str]] = {
    # docs/00 - the incremental parser's entry point.
    ("framework/http/src/parser.rs", 748, 748): r"pub fn parse_request_incremental",
    # docs/00 - where the connection loop calls that parser.
    ("framework/aether/src/app.rs", 2639, 2639): r"let parse_result = match aether_http::parser::parse_request_incremental\(",
    # docs/00, docs/07 - the receive read: the shorter of the idle bound and what is left of the
    # receive budget, racing shutdown. The end is a bare `};`; the coordinate is what names it.
    ("framework/aether/src/app.rs", 3038, 3059): (
        r"^ {20}let half_received = recv_started\.is_some\(\);$",
        r"^ {20}\};$",
    ),
    # docs/00, docs/03 - the method-to-bit table that makes the node bitmask possible.
    ("framework/http/src/method.rs", 25, 43): (r"pub const fn bit\(self\) -> u8 \{", r"^ {4}\}$"),
    # docs/00, docs/03 - O(1) allowed-method test against that bitmask.
    ("framework/router/src/node.rs", 42, 44): (
        r"pub fn supports_method\(&self, method: Method\) -> bool \{",
        r"^ {4}\}$",
    ),
    # docs/00, docs/04 - the error enum the status mapping is exhaustive over.
    ("framework/core/src/error.rs", 5, 19): (r"pub enum AetherError \{", r"Custom\(String\),$"),
    # docs/00 - the 431/413/400 arm of the parse-error response.
    ("framework/aether/src/app.rs", 2991, 2999): (
        r"let mut err_res = match err \{",
        r"^ {32}\};$",
    ),
    # docs/00 - the close decision that follows it.
    ("framework/aether/src/app.rs", 3000, 3000): r"err_res\.headers\.push\(\(\"Connection\", \"close\"",
    # docs/00 - the handler-timeout arm: abort the task, answer 504, close.
    ("framework/aether/src/serve.rs", 972, 977): (
        r"^ {12}None => \{$",
        r"^ {12}\}$",
    ),
    # docs/00, docs/03 - route-segment classification in the trie.
    ("framework/router/src/trie.rs", 126, 126): r"seg\.starts_with\('\{'\) && seg\.ends_with\('\}'\)",
    # docs/02 - the buffer that shrinks back after an oversized request.
    ("framework/aether/src/app.rs", 2920, 2924): (
        r"^ {32}if conn_buf\.is_empty\(\)$",
        r"^ {32}\}$",
    ),
    # docs/02 - the charge a connection's buffer carries.
    ("framework/aether/src/app.rs", 3891, 3893): (
        r"fn conn_buffer_charge\(buf: &BytesMut\) -> u64 \{",
        r"^\}$",
    ),
    # docs/02 - the prototype worker loop's arena sizing.
    ("framework/runtime/src/worker.rs", 24, 24): r"RequestArena::with_capacity\(4096\)",
    # docs/02 - the banner marking the runtime crate's loop as not the serve path.
    ("framework/runtime/src/lib.rs", 3, 11): (
        r"//! Legacy prototype connection loop",
        r"//! `aether` crate's `AetherApp`\. Do not treat it as supported API\.$",
    ),
    # docs/02 - the parse loop's head and its call into the incremental parser, which the §3
    # excerpt reproduces with the request body and the error reasons deleted.
    ("framework/aether/src/app.rs", 2432, 2644): (
        r"^ {20}loop \{$",
        r"Ok\(Some\(\(req, owned_body, consumed\)\)\) => \{$",
    ),
    # docs/02 - where that loop hands its buffer forward by exactly the consumed bytes. The
    # indentation is part of the pin: the file holds a second, nested `split_to(consumed)`, and a
    # generic pin silently followed the wrong one when an insertion above shifted both.
    ("framework/aether/src/app.rs", 2911, 2911): r"^ {32}let _ = conn_buf\.split_to\(consumed\);$",
    # docs/02 - the `Ok(None)` arm the excerpt shows as the partial-request break.
    ("framework/aether/src/app.rs", 2951, 2951): r"^ {28}Ok\(None\) => \{$",
    # docs/02 - the `Err` arm the excerpt shows as 431 / 413 / 400 then close.
    ("framework/aether/src/app.rs", 2990, 2990): r"^ {28}Err\(err\) => \{$",
    ("framework/aether/src/app.rs", 2997, 2997): r"^ {36}AetherError::UnsupportedExpectation => Response::status\(417, aether_http::response::canonical_reason\(417\)\),",
    # docs/02 - the non-incremental `parse` signature; the anchor only separates it from the
    # incremental function above, the pinned line is what makes it unambiguous.
    ("framework/http/src/parser.rs", 59, 59): r"^\s*pub fn parse\($",
    # docs/03 - the 405 variant that carries which methods the route does allow.
    ("framework/router/src/trie.rs", 24, 26): (r"MethodNotAllowed \{", r"^ {4}\},$"),
    # docs/03 - turning that bitmask into an `Allow` header value.
    ("framework/router/src/trie.rs", 64, 64): r"pub fn bitmask_to_allow_header\(bitmask: u64\) -> String \{",
    # docs/04 - the smuggling checks over Content-Length and Transfer-Encoding.
    ("framework/http/src/parser.rs", 382, 468): (
        r"let mut headers = HeaderMap::new\(\);",
        r"^ {4}\}$",
    ),
    # docs/04 - the CL.TE unit test whose line the page cites; pinned on the `fn` line because a
    # `let limits = ServerLimits::default();` body line is not unique in this file.
    ("framework/http/src/parser.rs", 1132, 1132): r"fn test_smuggling_cl_te_conflict_rejected\(\) \{",
    # docs/04, docs/10 - HMAC verification, constant time by construction of `ring`.
    ("framework/security/src/jwt.rs", 123, 123): r"hmac::verify\(&self\.key, signed_data, &sig_bytes\)",
    # docs/04, docs/10 - the project's own constant-time comparison helper.
    ("framework/security/src/constant_time.rs", 6, 6): r"pub fn constant_time_eq\(a: &\[u8\], b: &\[u8\]\) -> bool \{",
    # docs/05 - the request context the accessors are the API for.
    ("framework/aether/src/app.rs", 394, 428): (r"pub struct RequestContext \{", r"^\}$"),
    # docs/05 - the small-vector type behind repeated field lines.
    ("framework/aether/src/app.rs", 308, 308): r"pub\(crate\) type HeaderLines",
    # docs/05 - the private accessor that hands them out.
    ("framework/aether/src/app.rs", 717, 717): r"fn lines_for\(&self, name: &str\)",
    # docs/05 - the three public writers, all delegating to one framing function.
    ("framework/http/src/response.rs", 394, 424): (
        r"pub fn write_http1_to\(&self, out: &mut BytesMut\) \{",
        r"^ {4}\}$",
    ),
    # docs/05 - the single authority for response framing.
    ("framework/http/src/response.rs", 426, 426): r"fn write_http1_response\(&self, out: &mut BytesMut, minor: u8, include_body: bool\) \{",
    # docs/05 - the version-echoing writer.
    ("framework/http/src/response.rs", 415, 424): (
        r"pub fn write_http1_to_for_request_version\(",
        r"^ {4}\}$",
    ),
    # docs/05 - its two call sites in the connection loop.
    ("framework/aether/src/app.rs", 4027, 4027): r"response\.write_http1_to_for_request_version\(",
    ("framework/aether/src/app.rs", 2801, 2801): r"r\.write_http1_to_for_request_version\(",
    # docs/05 - the pre-acceptance rejection path's plain writer.
    ("framework/aether/src/app.rs", 2343, 2343): r"r\.write_http1_to\(&mut rej_buf\);",
    # docs/05 - the other three plain-writer exits: the 408 receive-timeout answer (now the
    # `answer_receive_timeout` helper), the parse-error response, and the 413 cap refusal. The
    # first and third lines are textually identical, so the pin's coordinate separates them.
    ("framework/aether/src/app.rs", 4256, 4256): r"err_res\.write_http1_to\(write_buf\);",
    ("framework/aether/src/app.rs", 3002, 3002): r"err_res\.write_http1_to\(&mut write_buf\);",
    ("framework/aether/src/app.rs", 4305, 4305): r"err_res\.write_http1_to\(write_buf\);",
    # docs/05 - the sanitiser every header value passes through.
    ("framework/http/src/response.rs", 490, 490): r"fn write_sanitized_field_value",
    # docs/07, docs/00 - the buffer budget check before a read is charged.
    ("framework/aether/src/app.rs", 3084, 3084): r"conn_buf\.len\(\) \+ n > limits_clone\.max_header_bytes \+ limits_clone\.max_body_bytes",
    # docs/07 - connection admission racing shutdown.
    ("framework/aether/src/app.rs", 2274, 2281): (
        r"let permit = tokio::select! \{",
        r"^ {12}\};$",
    ),
    # docs/07 - the declared-length bound. The comparison is against the ceiling the *entry point*
    # names: the buffered callers pass `max_body_bytes`, the streaming one passes none because the
    # route applies `max_stream_body_bytes`. Pinned on `declared_body_ceiling`, not on a limit
    # field, so a future edit cannot restore the buffered ceiling without moving this pin.
    ("framework/http/src/parser.rs", 465, 465): r"cl > declared_body_ceiling",
    # docs/07 - the per-chunk DATA bound on chunked framing.
    ("framework/http/src/parser.rs", 1067, 1067): r"size > limits\.max_body_bytes as u64",
    # docs/07 - the cumulative bound; this one line is generic, so the pin's line is the coordinate
    # and `checked_add` at 659-661 is what makes it cumulative. Keep the pair when editing.
    ("framework/http/src/parser.rs", 1084, 1084): r"^\s*> limits\.max_body_bytes$",
    # docs/07 - the h2 DATA bound that mirrors it.
    ("framework/aether/src/http2.rs", 891, 891): r"buf\.len\(\) \+ chunk\.len\(\) > limits\.max_body_bytes",
    # docs/09 - the error-to-response contract, including the three deliberate omissions and the
    # diagnostics opt-in (`redacted_detail`) that closed PROB-0009's residual.
    ("framework/aether/src/app.rs", 123, 166): (r"pub trait IntoResponseError: Send \{", r"^\}$"),
    # framework/docs - the teaching set (track 92). Same rules, same pins: every live coordinate
    # under `framework/docs/` is held to its span here, and the dated records are exempted in
    # SNAPSHOTS instead. ERROR_HANDLING_GUIDE_AR's contract table cites overlapping sub-spans of
    # app.rs, so each shape the prose names gets its own pin rather than one broad anchor.
    # ERROR_HANDLING_GUIDE_AR - the whole handler-error contract, declaration through impls.
    ("framework/aether/src/app.rs", 123, 222): (
        r"pub trait IntoResponseError: Send \{",
        r"^\}$",
    ),
    # ERROR_HANDLING_GUIDE_AR - `AetherError`'s own status choice (Io/Timeout 500/504).
    ("framework/aether/src/app.rs", 188, 203): (
        r"impl IntoResponseError for AetherError \{",
        r"^ {4}\}$",
    ),
    # ERROR_HANDLING_GUIDE_AR - the impls that execute the table, through the blanket response.
    ("framework/aether/src/app.rs", 168, 265): (
        r"impl IntoResponseError for &'static str \{",
        r"^\}$",
    ),
    # ERROR_HANDLING_GUIDE_AR - the two `IntoResponse` impls that make `Err` a 200 if confused.
    ("framework/aether/src/app.rs", 76, 80): (r"impl IntoResponse for &'static str \{", r"^\}$"),
    ("framework/aether/src/app.rs", 82, 87): (r"impl IntoResponse for String \{", r"^\}$"),
    # ERROR_HANDLING_GUIDE_AR - the wire tests that pin the table on TCP.
    ("framework/tests/tests/http_e2e.rs", 5183, 5183): r"async fn a_handler_error_answers_400_and_says_so_in_the_body\(\)",
    ("framework/tests/tests/http_e2e.rs", 5219, 5219): r"async fn typed_handler_errors_answer_their_own_class\(\)",
    # ERROR_HANDLING_GUIDE_AR - the two 5xx codes in the enum's table.
    ("framework/core/src/error.rs", 37, 37): r"AetherError::Timeout => ErrorCode::E5002InternalServerError,",
    ("framework/core/src/error.rs", 41, 41): r"AetherError::Io\(_\) => ErrorCode::E5002InternalServerError,",
    # AETHER_CORE_CAPABILITIES - the two parser entry points and the two callers outside
    # `aether-http`'s own tests: the runtime crate's exported worker loop and the fuzz oracle.
    # The bitmask line the O(1) method test compiles to.
    ("framework/runtime/src/worker.rs", 32, 32): r"RequestView::parse\(&conn_buf, &limits\)",
    ("framework/fuzz/src/lib.rs", 86, 86): r"match RequestView::parse\(data, &limits\) \{",
    ("framework/router/src/node.rs", 43, 43): r"allowed_methods & method\.bitmask\(\)",
    # FEATURE_COMPLETENESS_MATRIX - the four shapes of the typed-error mapping, and the 5xx
    # `error_detail` the reporter path depends on.
    ("framework/aether/src/app.rs", 168, 186): (
        r"impl IntoResponseError for &'static str \{",
        r"^\}$",
    ),
    ("framework/aether/src/app.rs", 188, 222): (
        r"impl IntoResponseError for AetherError \{",
        r"^\}$",
    ),
    ("framework/aether/src/app.rs", 224, 265): (
        r"impl<T, E> IntoResponse for Result<T, E>",
        r"^\}$",
    ),
    ("framework/aether/src/app.rs", 237, 248): (
        r"let declared = err\.error_status\(\);",
        r"let detail = if in_range \{ err\.error_detail\(\) \} else \{ None \};$",
    ),
    ("framework/aether/src/app.rs", 209, 214): (
        r"fn error_detail\(&self\) -> Option<String> \{",
        r"^ {4}\}$",
    ),
    # FEATURE_COMPLETENESS_MATRIX - the request serial crossing `tokio::spawn` as a task-local.
    ("framework/aether/src/serve.rs", 905, 912): (
        r"let request_serial = ctx\.request_serial\(\);",
        r"let result = crate::diagnostics::within_request\(request_serial, handler\(ctx\)\)\.await;",
    ),
    # FEATURE_COMPLETENESS_MATRIX - the h2 response block charge, and the trickle floor's constant.
    ("framework/aether/src/http2.rs", 1099, 1099): r"price_response_body\(out\.body, charge\.as_deref_mut\(\)\)",
    ("framework/aether/src/http2.rs", 981, 981): r"const MIN_PROGRESS_FOR_RENEWAL: usize = 1024;",
    # FULL_FRAMEWORK_DOCUMENTATION_AR - the 64-byte arena alignment it names.
    ("framework/core/src/arena.rs", 15, 15): r"const ARENA_ALIGN: usize = 64;",
    # GETTING_STARTED_AR - the literal log line it quotes.
    ("framework/middleware/src/logger.rs", 44, 44): (
        r'println!\(".*\[Aether\] \{\} \{\} -> \{\}", method, path, status\)'
    ),
    # docs/04, docs/09 - the CSRF layer: what it issues, what it accepts, and where it gates.
    ("framework/security/src/csrf.rs", 262, 267): (
        r"pub fn issue\(&self\) -> Result<String, CsrfError> \{",
        r"Ok\(format!\(\"\{nonce\}\.\{\}\", encode_base64url\(mac\.as_ref\(\)\)\)\)$",
    ),
    ("framework/security/src/csrf.rs", 312, 320): (
        r"pub fn checks\(&self, cookies: &\[String\], headers: &\[String\]\) -> bool \{",
        r"^ {4}\}$",
    ),
    ("framework/security/src/csrf.rs", 330, 344): (
        r"fn mac_verifies\(&self, token: &str\) -> bool \{",
        r"^ {4}\}$",
    ),
    ("framework/aether/src/serve.rs", 422, 459): (
        r"let \(csrf_refused, csrf_token, csrf_minted\) = match &self\.csrf \{",
        r"let csrf_cookie = csrf_minted\.then\(\|\| csrf_token\.clone\(\)\)\.flatten\(\);",
    ),
    # docs/09 - the facade method that installs the layer app-wide.
    ("framework/aether/src/app.rs", 1130, 1130): (
        r"pub fn enable_csrf\(mut self, csrf: CsrfTokens\) -> Self \{"
    ),
    # docs/09 section 7.6 - the signer that issues and verifies a cookie value, and the tag
    # input that binds the cookie name into its MAC.
    ("framework/security/src/cookie.rs", 115, 190): (r"impl CookieSigner \{", r"^\}$"),
    ("framework/security/src/cookie.rs", 183, 189): (
        r"fn mac_input\(&self, name: &str, encoded: &str\) -> Vec<u8> \{",
        r"^ {4}\}$",
    ),
    # docs/09 section 3 and 7.6 - the accessor that decides what the request carried.
    ("framework/aether/src/cookie.rs", 31, 91): (r"impl RequestContext \{", r"^\}$"),
    # docs/09 section 7.6 - the fifteen unit tests the section cites as its evidence.
    ("framework/security/src/cookie.rs", 192, 401): (r"^#\[cfg\(test\)\]$", r"^\}$"),
}


class Citation(NamedTuple):
    doc: Path
    line: int
    text: str  # as written, e.g. `app.rs:1771` or `:664`
    file_token: str | None
    start: int
    end: int


class Finding(NamedTuple):
    where: str
    message: str
    detail: str = ""


def citations_in(doc: Path) -> tuple[list[Citation], list[Finding]]:
    """Every citation in one document, plus any bare coordinate that names no file."""
    found: list[Citation] = []
    broken: list[Finding] = []
    for lineno, line in enumerate(doc.read_text(encoding="utf-8").splitlines(), 1):
        hits: list[tuple[int, str | None, int, int, str]] = []
        for m in ATTACHED.finditer(line):
            end = int(m.group(3)) if m.group(3) else int(m.group(2))
            hits.append((m.start(), m.group(1), int(m.group(2)), end, m.group(0)))
        for m in BARE.finditer(line):
            end = int(m.group(2)) if m.group(2) else int(m.group(1))
            hits.append((m.start(), None, int(m.group(1)), end, m.group(0)))
        hits.sort()
        last_file: str | None = None
        for _, token, start, end, text in hits:
            if token is None:
                if last_file is None:
                    broken.append(
                        Finding(
                            f"{doc.name}:{lineno}",
                            f"bare coordinate {text} names no file on its line",
                            "write the file explicitly, or put it earlier on the same line",
                        )
                    )
                    continue
                token = last_file
            else:
                last_file = token
            found.append(Citation(doc, lineno, text, token, start, end))
    return found, broken


def basename_index(root: Path) -> dict[str, list[Path]]:
    index: dict[str, list[Path]] = defaultdict(list)
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            index[name].append(Path(dirpath) / name)
    return index


def resolve_file(token: str, root: Path, index: dict[str, list[Path]]) -> tuple[Path | None, str]:
    """An exact relative path wins; otherwise a basename must be unique."""
    direct = root / token
    if direct.is_file():
        return direct, ""
    hits = sorted(index.get(Path(token).name, ()))
    if len(hits) == 1:
        return hits[0], ""
    if not hits:
        return None, f"no file named {Path(token).name!r} exists in the tree"
    rels = ", ".join(p.relative_to(root).as_posix() for p in hits)
    return None, f"{Path(token).name!r} is ambiguous: {rels}"


def anchor_lines(anchor: str, text: str) -> list[int]:
    """Line numbers where `anchor` matches in a whole file (multi-line anchors included)."""
    return [text.count("\n", 0, m.start()) + 1 for m in re.finditer(anchor, text, re.M)]


def line_finding(
    where: str, key: tuple[str, int, int], lineno: int, pattern: str, line: str, whole: str
) -> Finding | None:
    """None when `pattern` still matches `line`; otherwise the finding, naming where it went."""
    if re.search(pattern, line):
        return None
    elsewhere = anchor_lines(pattern, whole)
    listed = ", ".join(str(n) for n in elsewhere[:3])
    if not elsewhere:
        moved = "the pattern matches nowhere in this file"
    elif len(elsewhere) == 1:
        moved = f"the pattern now matches only at {key[0]}:{listed}"
    else:
        # A last line like `}` is never unique, so the report lists where the pattern lives
        # instead of pretending it names one place.
        moved = (
            f"the pattern now matches at {key[0]}:{listed}"
            + (" (first three)" if len(elsewhere) > 3 else "")
            + ", and it is not unique, so re-derive the coordinate from the code"
        )
    first = lineno == key[1]
    if elsewhere:
        message = (
            "the span's first line moved"
            if first
            else "the span's last line moved"
        )
        detail = (
            f"pinned at {key[0]}:{lineno}; {moved}. "
            + (
                "the lines above this span moved, so re-derive the citation"
                if first
                else "an edit inside the span moved its end, so re-derive both ends"
            )
        )
    else:
        message = (
            "the span's first line no longer holds what the pin records"
            if first
            else "the span's last line no longer holds what the pin records"
        )
        detail = (
            f"pinned at {key[0]}:{lineno} must match /{pattern}/; {moved} "
            "- the code this citation named was replaced"
        )
    return Finding(where, message, detail)


def documents(root: Path) -> list[Path]:
    """Every document the gate reads, in a stable order and once each."""
    seen: set[Path] = set()
    found: list[Path] = []
    for pattern in DOC_GLOBS:
        for path in sorted(root.glob(pattern)):
            resolved = path.resolve()
            if resolved not in seen:
                seen.add(resolved)
                found.append(path)
    return found


def snapshot_findings(doc: Path, root: Path, text: str) -> tuple[bool, list[Finding]]:
    """Whether this document is an allow-listed snapshot, plus any marker disagreement."""
    rel = doc.relative_to(root).as_posix()
    markers = SNAPSHOT_MARKER.findall(text.removeprefix("\ufeff"))
    listed = SNAPSHOTS.get(rel)
    if listed is None:
        if markers:
            return False, [
                Finding(
                    rel,
                    "this document may not declare itself a snapshot",
                    "the marker exempts a document from citation checks; only the dated records "
                    "in SNAPSHOTS may carry it, and a document cannot exempt itself",
                )
            ]
        return False, []
    if not markers:
        return True, [
            Finding(
                rel,
                "a listed snapshot carries no cite-check marker",
                f"add '<!-- cite-check: snapshot {listed} -->' as the document's first line, "
                "or remove the document from SNAPSHOTS",
            )
        ]
    if len(markers) > 1 or markers[0] != listed:
        return True, [
            Finding(
                rel,
                "the snapshot marker disagrees with SNAPSHOTS",
                f"the document says {', '.join(markers)}; the list says {listed}",
            )
        ]
    return True, []


DECL_RE = re.compile(
    r"\b(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+|static\s+|extern\s+\w+\s+)*"
    r"(?:async\s+)?(?:fn|struct|enum|trait|type|mod|union|macro_rules!)\s+([A-Za-z_][A-Za-z0-9_]*)"
)
IMPL_RE = re.compile(
    r"\bimpl(?:\s*<[^{};]*>)?\s+(?:[\w:]+\s+for\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^{};]*>)?\s*\{"
)
ENUM_RE = re.compile(r"\b(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)[^{;]*\{")
STRUCT_RE = re.compile(r"\b(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)[^{;]*\{")
ANY_IDENT = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\b")
STRING_LIT = re.compile(r"\"(?:[^\"\\]|\\.)*\"|'(?:[^'\\]|\\.)*'")
CARGO_NAME = re.compile(r'^name\s*=\s*"([^"]+)"', re.M)


def _block_body(text: str, start: int) -> str:
    """The `{ ... }` that begins just before `start`, balanced by hand (no parser here)."""
    depth, i = 1, start
    while i < len(text) and depth:
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
        i += 1
    return text[start:i]


def source_index(root: Path) -> tuple[set[str], set[str], dict[str, set[str]]]:
    """(every name the tree carries, every type it declares, type -> its members).

    A document names things in three ways - a member of a type it owns, a name inside a crate it
    owns, or a bare identifier - so the index answers all three. String literals count as carried
    names, because a wire header, a route path or a JSON key is a name the code really uses even
    though no declaration spells it. Crate names are read with hyphens mapped to underscores, which
    is how `aether-dependency` becomes `use aether_dependency::` in prose and in code alike.
    """
    idents: set[str] = set()
    types: set[str] = set()
    members: dict[str, set[str]] = defaultdict(set)
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix not in NAME_SOURCE_EXTS:
            continue
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        rel_parts = path.relative_to(root).parts
        # A test/bench/example file's stem is a target name the documents cite (`http_e2e.rs`).
        idents.add(path.stem.replace("-", "_"))
        if path.name == "Cargo.toml":
            idents.update(n.replace("-", "_") for n in CARGO_NAME.findall(
                path.read_text(encoding="utf-8", errors="replace")))
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if rel_parts[0] != "framework":
            # Only the product's sources are evidence that a name exists. A governance tool's prose
            # is documentation of the same class as `docs/`: indexing its comments or its reason
            # strings would let the gate shelter a phantom by writing about it, and the exception
            # table is the worst case because it spells out every name the gate refused. The tool's
            # own files and definitions stay in the index, because a document may legitimately name
            # `cite_check.py` or `test_recycling_ratchet` as a thing this repository has.
            idents.update(re.findall(
                r"^\s*(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)", text, re.M))
            continue
        if path.suffix != ".rs":
            code = STRING_LIT.sub(" ", text)
            idents.update(ANY_IDENT.findall(code))
            idents.update(x for s in STRING_LIT.findall(text) for x in ANY_IDENT.findall(s))
            idents.update(re.findall(
                r"^\s*(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)", text, re.M))
            continue
        code = STRING_LIT.sub(" ", text)
        idents.update(ANY_IDENT.findall(code))
        idents.update(x for s in STRING_LIT.findall(text) for x in ANY_IDENT.findall(s))
        types.update(DECL_RE.findall(code))
        for m in IMPL_RE.finditer(code):
            ty, body = m.group(1), _block_body(code, m.end())
            fns = set(re.findall(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)", body))
            members[ty].update(fns)
            members[ty].update(re.findall(
                r"^\s{4,}(?:pub(?:\([^)]*\))?\s+)?(?:const|type|static)\s+([A-Za-z_][A-Za-z0-9_]*)",
                body, re.M))
        for m in ENUM_RE.finditer(code):
            members[m.group(1)].update(re.findall(
                r"^\s{4}([A-Z][A-Za-z0-9_]*)", _block_body(code, m.end()), re.M))
        for m in STRUCT_RE.finditer(code):
            members[m.group(1)].update(re.findall(
                r"^\s{4}(?:pub(?:\([^)]*\))?\s+)?([a-z_][A-Za-z0-9_]*)\s*:",
                _block_body(code, m.end()), re.M))
    idents.update(types)
    return idents, types, members


def proposal_findings(doc: Path, root: Path, text: str) -> tuple[bool, list[Finding]]:
    """Whether this document is an allow-listed proposal, plus any marker disagreement."""
    rel = doc.relative_to(root).as_posix()
    markers = PROPOSAL_MARKER.findall(text.removeprefix("\ufeff"))
    listed = PROPOSALS.get(rel)
    if listed is None:
        if markers:
            return False, [Finding(
                rel,
                "this document may not declare itself a proposal",
                "the marker exempts a document from the names pass; only the design documents in "
                "PROPOSALS may carry it, and a document cannot exempt itself",
            )]
        return False, []
    if not markers:
        return True, [Finding(
            rel,
            "a listed proposal carries no cite-check marker",
            f"add '<!-- cite-check: proposal {listed} -->' as the document's first line, or remove "
            "the document from PROPOSALS",
        )]
    if len(markers) > 1 or markers[0] != listed:
        return True, [Finding(
            rel,
            "the proposal marker disagrees with PROPOSALS",
            f"the document says {', '.join(markers)}; the list says {listed}",
        )]
    return True, []


def prose_lines(doc: Path) -> list[tuple[int, str]]:
    """The document's lines that state claims, with fenced code blocks left out.

    A fenced block is an example the document is writing, not a name the tree is obliged to carry,
    so the whole block is dropped rather than judged. The fence toggles on any ``` or ~~~ line,
    which is how the documents already delimit them.
    """
    out: list[tuple[int, str]] = []
    in_fence = False
    for lineno, line in enumerate(
        doc.read_text(encoding="utf-8", errors="replace").split("\n"), 1
    ):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            out.append((lineno, line))
    return out


def name_findings(
    doc: Path,
    root: Path,
    index: tuple[set[str], set[str], dict[str, set[str]]],
    exceptions: dict[tuple[str, str], str],
    used: set[tuple[str, str]],
    stats: dict[str, int],
) -> list[Finding]:
    """Every name this document asserts that the tree does not carry and no reason shelters."""
    idents, types, members = index
    rel = doc.relative_to(root).as_posix()
    findings: list[Finding] = []
    for lineno, line in prose_lines(doc):
        for span in CODE_SPAN.findall(line):
            attributed: set[str] = set()
            if "/" in span:
                # A span with a slash is a path, not an identifier. Paths belong to the citation
                # pass (which resolves the file and holds its span) and to `evidence_check.py`
                # (which refuses an artifact outside the repository); judging `fuzz_targets` as a
                # name would only duplicate their verdict in the wrong vocabulary.
                continue
            for head, leaf in PATH_PAIR.findall(span):
                if head in EXTERNAL_HEADS and head not in types:
                    # The document says which crate the member belongs to, so it is not claiming
                    # the workspace declares it. Remember the leaf so the bare rule below does not
                    # report the same external API twice.
                    attributed.add(leaf)
                    continue
                if head not in types and head not in idents:
                    # The document attributes the member to something this tree does not declare;
                    # the gate has no evidence about it either way and says nothing.
                    continue
                # A member index built by brace-matching `impl` blocks cannot be trusted to be
                # complete (a `r#"..."#` sample body, a `#[derive(Default)]`, a trait impl in
                # another file), and a gate that invents findings is a gate that gets ignored. So
                # the qualified rule asks the question the member index answers reliably - is this
                # leaf a name the tree carries at all - and the bare-name rule below is what
                # catches a document inventing an API, which is always also a name it invents.
                carried = leaf in idents
                if carried:
                    stats["qualified"] += 1
                    continue
                key = (rel, leaf)
                if key in exceptions:
                    used.add(key)
                    stats["excepted"] += 1
                    continue
                findings.append(Finding(
                    f"{rel}:{lineno}",
                    f"this document names `{head}::{leaf}`, which this tree does not declare",
                    f"the span reads `{span.strip()[:90]}` and no name in the sources is `{leaf}`; "
                    "fix it, or add (document, leaf) to NAME_EXCEPTIONS with the reason it is "
                    "legitimate",
                ))
            for token in SNAKE_NAME.findall(span):
                if token in attributed:
                    stats["attributed"] += 1
                    continue
                if token in idents:
                    stats["snake"] += 1
                    continue
                key = (rel, token)
                if key in exceptions:
                    used.add(key)
                    stats["excepted"] += 1
                    continue
                findings.append(Finding(
                    f"{rel}:{lineno}",
                    f"no name in this tree is `{token}`",
                    f"the span reads `{span.strip()[:90]}`; fix the name, or add "
                    "(`{doc}`, `{token}`) to NAME_EXCEPTIONS with its reason".replace(
                        "{doc}", rel).replace("{token}", token),
                ))
    return findings


def check(
    root: Path = REPO_ROOT,
    docs: list[Path] | None = None,
    pins: dict[tuple[str, int, int], str | tuple[str, str]] | None = None,
    stats: dict[str, int] | None = None,
    exceptions: dict[tuple[str, str], str] | None = None,
) -> tuple[list[Citation], list[Finding]]:
    """Audit the live documents against the tree; returns their citations and every finding."""
    pins = PINS if pins is None else pins
    exceptions = NAME_EXCEPTIONS if exceptions is None else exceptions
    if docs is None:
        docs = documents(root)
    index = basename_index(root)
    names = source_index(root)
    all_citations: list[Citation] = []
    findings: list[Finding] = []
    used_pins: set[tuple[str, int, int]] = set()
    used_exceptions: set[tuple[str, str]] = set()
    checked = {
        "qualified": 0, "snake": 0, "excepted": 0, "attributed": 0,
        "live_docs": 0, "proposal_docs": 0,
    }

    for doc in docs:
        text = doc.read_text(encoding="utf-8", errors="replace")
        is_snapshot, marker_findings = snapshot_findings(doc, root, text)
        findings.extend(marker_findings)
        if is_snapshot:
            # Its coordinates describe the tree at its date. Checking them against today's source
            # would demand that history be rewritten; fixing them would falsify the record.
            continue
        checked["live_docs"] += 1
        citations, broken = citations_in(doc)
        findings.extend(broken)
        is_proposal, p_findings = proposal_findings(doc, root, text)
        findings.extend(p_findings)
        if is_proposal:
            checked["proposal_docs"] += 1
        else:
            # A proposal names what it intends to build; every other live document names what is.
            findings.extend(
                name_findings(doc, root, names, exceptions, used_exceptions, checked)
            )
        for c in citations:
            all_citations.append(c)
            where = f"{doc.name}:{c.line} -> {c.file_token}:{c.start}" + (
                f"-{c.end}" if c.end != c.start else ""
            )
            src, error = resolve_file(c.file_token, root, index)
            if src is None:
                findings.append(Finding(where, "cannot resolve the cited file", error))
                continue
            lines = src.read_text(encoding="utf-8", errors="replace").splitlines()
            if c.start < 1 or c.end < c.start or c.end > len(lines):
                findings.append(
                    Finding(
                        where,
                        f"span {c.start}-{c.end} is outside the file",
                        f"{src.relative_to(root).as_posix()} has {len(lines)} lines",
                    )
                )
                continue
            key = (src.relative_to(root).as_posix(), c.start, c.end)
            pin = pins.get(key)
            if pin is None:
                siblings = sorted(k for k in pins if k[0] == key[0])
                near = ", ".join(f"{a}-{b}" for _, a, b in siblings) or "none"
                findings.append(
                    Finding(
                        where,
                        "this span is not pinned, so nothing checks what it holds",
                        f"pins for {key[0]}: {near}; add or move the pin when the coordinate moves",
                    )
                )
                continue
            if isinstance(pin, tuple) != (c.start != c.end):
                findings.append(
                    Finding(
                        where,
                        "this pin does not match the span's shape",
                        "a single-line span pins one regex; a multi-line span pins "
                        "(first line, last line) so neither end can drift unobserved",
                    )
                )
                continue
            used_pins.add(key)
            first_re, last_re = pin if isinstance(pin, tuple) else (pin, pin)
            whole = src.read_text(encoding="utf-8", errors="replace")
            finding = line_finding(where, key, c.start, first_re, lines[c.start - 1], whole)
            if finding is None and c.start != c.end:
                finding = line_finding(where, key, c.end, last_re, lines[c.end - 1], whole)
            if finding is not None:
                findings.append(finding)

    for key in sorted(pins):
        if key not in used_pins:
            findings.append(
                Finding(
                    f"{key[0]}:{key[1]}-{key[2]}",
                    "stale pin: no citation in docs/ points here",
                    "remove it, or point a document at it",
                )
            )
    for key in sorted(exceptions):
        if key not in used_exceptions:
            findings.append(
                Finding(
                    f"{key[0]}: {key[1]}",
                    "stale name exception: no unresolved mention of it is left",
                    "remove it, or the document gained a name the table no longer explains",
                )
            )
    if stats is not None:
        stats.update(checked)
        stats["exceptions"] = len(exceptions)
    return all_citations, findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--list",
        action="store_true",
        help="print every citation and the first line of its span, then run the checks",
    )
    parser.add_argument(
        "--root", default=str(REPO_ROOT), help="repository root to audit (default: this repo)"
    )
    args = parser.parse_args(argv)
    root = Path(args.root).resolve()
    # The documents quote Arabic and Unicode prose; a Windows console still defaults to cp1252, and
    # a finding that cannot be printed is a crash where a report was owed. Degrade, never raise.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")

    stats: dict[str, int] = {}
    citations, findings = check(root, stats=stats)
    index = basename_index(root)
    snapshot_docs = [d for d in documents(root) if d.relative_to(root).as_posix() in SNAPSHOTS]
    if args.list:
        by_doc: dict[str, list[Citation]] = defaultdict(list)
        for c in citations:
            by_doc[c.doc.name].append(c)
        for name in sorted(by_doc):
            for c in by_doc[name]:
                src, _ = resolve_file(c.file_token, root, index)
                first = ""
                if src is not None:
                    lines = src.read_text(encoding="utf-8", errors="replace").splitlines()
                    if 1 <= c.start <= len(lines):
                        first = lines[c.start - 1].strip()
                print(f"{name}:{c.line}\t{c.text}\t{first[:120]}")
        print()

    for f in findings:
        print(f"FAIL  {f.where}")
        print(f"      {f.message}")
        if f.detail:
            print(f"      {f.detail}")
    spans = set()
    for c in citations:
        src, _ = resolve_file(c.file_token, root, index)
        if src is not None:
            spans.add((src.relative_to(root).as_posix(), c.start, c.end))
    docs = len({c.doc for c in citations})
    print(
        f"{len(citations)} citations in {docs} live documents, {len(spans)} distinct spans, "
        f"{len(findings)} finding(s)"
    )
    print(
        f"names pass over {stats.get('live_docs', 0)} live documents: "
        f"{stats.get('qualified', 0)} qualified names and {stats.get('snake', 0)} bare names "
        f"resolved against the tree's own sources, {stats.get('excepted', 0)} sheltered by "
        f"NAME_EXCEPTIONS ({stats.get('exceptions', 0)} in the table), "
        f"{stats.get('attributed', 0)} attributed to another crate, "
        f"{stats.get('proposal_docs', 0)} exempted as a proposal"
    )
    if snapshot_docs:
        historical = sum(len(citations_in(d)[0]) for d in snapshot_docs)
        print(
            f"{historical} citations in {len(snapshot_docs)} dated snapshots "
            "(marked, not validated)"
        )
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
