#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Is the suite number a user-facing document prints the number this tree measures?

Tracks 60, 104 and 114 each re-derived the live test counts and each left the next one to
rediscover the same drift. The 2026-11-04 record named three documents still claiming the
track-115 reading as current, and no gate looked at a count: `cite_check.py` checks where a
document points, `evidence_check.py` checks that a path resolves, and a number that drifted
satisfied both.

The rule this file enforces:

    Every suite count in the pinned documents equals the count in the run the ladder just did,
    and every citation count equals what the citation gate just resolved.

The suite figures are read from the `test result:` lines of `target/gates/tests.log`; the citation
figures from the summary `target/gates/cite.log` prints; the corpus the document gates read from
`target/gates/evidence.log`; and the workspace's crate count from its own manifest,
`framework/Cargo.toml`. `gates.sh` writes the three logs earlier in the same run. There is no
checked-in copy of any of these numbers: a gate that compares a document to a second document only
tells two files to agree, while this one asks the tree. If a log is missing or older than the newest
framework source, this fails rather than skipping - a comparison against a stale run is the vacuous
pass the rule exists to prevent.

The cross-check sentence those documents print is pinned too: the `running N tests` lines of the
run sum to one more than the passed count, and both halves are compared, so the explanation beside
a total cannot keep an old total.

The pins carry no numbers, only the sentence that states one and which figure it is, so a track
that adds tests updates the documents and leaves this file alone. A pin whose sentence is reworded
away also fails: silently losing the claim is how the drift starts. Coverage runs in both
directions: a crate or test target this run measured must have a row or list entry, so a count the
documents never name is a finding rather than an invisible one.

The matrix's status table is the same rule over a second source: its cells are compared with a
recount of the rows they summarise, not with another document. That table drifted from its own rows
three times (tracks 96, 98b and 99-100) while every printed total stayed plausible. Track 120's
defect sat one level below it - a row marked `NOT_IMPLEMENTED` whose evidence cell had described the
priced answer since track 74 - and no count can see that; what this gate keeps is the summary and
the rows agreeing, so the next track that corrects a row cannot leave the table behind.

Run it from anywhere:

    python -B .project-governance/tools/count_check.py
"""
from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import NamedTuple

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_LOG = REPO_ROOT / "target" / "gates" / "tests.log"
DEFAULT_CITE_LOG = REPO_ROOT / "target" / "gates" / "cite.log"
DEFAULT_EVIDENCE_LOG = REPO_ROOT / "target" / "gates" / "evidence.log"

RESULT = re.compile(r"^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored")
RUNNING = re.compile(r"^running (\d+) tests?$")
CORPUS = re.compile(r"(\d+) documents scanned")
MEMBERS = re.compile(r"^members\s*=\s*\[(.*?)\]", re.MULTILINE | re.DOTALL)
DEP_CRATE = re.compile(r"deps/([A-Za-z0-9_]+)-[0-9a-f]{8,}(?:\.exe)?\)")
CITATIONS = re.compile(r"(\d+) citations in (\d+) live documents, (\d+) distinct spans")
NAMES = re.compile(
    r"names pass over \d+ live documents: (\d+) qualified names and (\d+) bare"
    r" names resolved(?: against [^,]*)?, "
    r"(\d+) sheltered by NAME_EXCEPTIONS \((\d+) in the table\), (\d+) attributed to another crate, "
    r"(\d+) exempted"
)

# The readiness report prints the statuses in this order; the matrix table prints BROKEN and
# UNVERIFIED before UNWIRED, so each reading pin carries its own order in `names`.
STATUS_ORDER: tuple[str, ...] = (
    "VERIFIED_IMPLEMENTED",
    "PARTIALLY_IMPLEMENTED",
    "NOT_IMPLEMENTED",
    "UNWIRED_IMPLEMENTATION",
    "BROKEN",
    "UNVERIFIED",
    "NOT_APPLICABLE",
)
MATRIX_STATUS_ORDER: tuple[str, ...] = (
    "VERIFIED_IMPLEMENTED",
    "PARTIALLY_IMPLEMENTED",
    "NOT_IMPLEMENTED",
    "BROKEN",
    "UNVERIFIED",
    "UNWIRED_IMPLEMENTATION",
    "NOT_APPLICABLE",
)


class Finding(NamedTuple):
    where: str
    message: str
    detail: str


class Pin(NamedTuple):
    """One sentence that states a count, and which figure it must state.

    `kind` decides how the captured groups are read:
      field          - one group per name, each a `Census` field
      target         - one group, compared with `targets[name]`
      target_list    - one group holding `name N, name N, ...` pairs
      crate_row      - two groups (crate, count); every match in the file is checked
      crate_row_list - a row naming several crates (group 1) and one shared count (group 2)
      status_table   - one group, compared with the rows carrying `names[0]` as their status
      status_total   - one group, compared with the number of status rows
      status_reading - seven counts in `names` order then the total, all compared
      corpus         - one group, compared with the document count the evidence gate scanned
      workspace_members - one group, compared with the crates `framework/Cargo.toml` lists

    `source` is the document whose rows the status kinds recount; it defaults to `file`.
    """

    file: str
    pattern: str
    kind: str
    names: tuple[str, ...] = ()
    source: str = ""


@dataclass
class Census:
    passed: int = 0
    failed: int = 0
    ignored: int = 0
    running: int = 0
    result_lines: int = 0
    unit: int = 0
    unit_runs: int = 0
    integration: int = 0
    targets: int = 0
    doc: int = 0
    doc_runs: int = 0
    by_target: dict[str, int] = field(default_factory=dict)
    by_crate: dict[str, int] = field(default_factory=dict)
    unplaced: int = 0
    citations: int = 0
    citation_docs: int = 0
    spans: int = 0
    qualified: int = 0
    bare: int = 0
    sheltered: int = 0
    sheltered_reasons: int = 0
    attributed: int = 0
    exempted: int = 0


PINS: tuple[Pin, ...] = (
    # The root README's opening evidence sentence and the three target counts it repeats.
    Pin(
        "README.md",
        r"\*\*(\d+) workspace tests pass, (\d+) fail, (\d+) ignored\*\*",
        "field",
        ("passed", "failed", "ignored"),
    ),
    Pin("README.md", r"(\d+) checked-in raw-socket end-to-end tests", "target", ("http_e2e",)),
    Pin("README.md", r"(\d+) HTTP/2 wire tests", "target", ("http2_e2e",)),
    Pin("README.md", r"(\d+) scheduling wire tests", "target", ("scheduling",)),
    # The framework README states the total, the split and the result-line count in one sentence.
    Pin(
        "framework/README.md",
        r"is \*\*(\d+) tests, (\d+) failures, (\d+) ignored\*\* — (\d+) unit \+\s*"
        r"(\d+) integration \+ (\d+) doc tests over (\d+) `test result:` lines",
        "field",
        ("passed", "failed", "ignored", "unit", "integration", "doc", "result_lines"),
    ),
    Pin("framework/README.md", r"(\d+) raw-socket end-to-end tests in", "target", ("http_e2e",)),
    Pin("framework/README.md", r"(\d+) HTTP/2 wire tests in", "target", ("http2_e2e",)),
    Pin("framework/README.md", r"(\d+) nested-state and", "target", ("nested_routers",)),
    Pin("framework/README.md", r"(\d+) TLS wire tests in", "target", ("tls_e2e",)),
    Pin("framework/README.md", r"(\d+) request-extension wire tests in", "target", ("extensions_e2e",)),
    Pin(
        "framework/README.md",
        r"(\d+) signed-cookie wire tests in",
        "target",
        ("signed_cookies",),
    ),
    Pin("framework/README.md", r"(\d+) scheduling wire tests in", "target", ("scheduling",)),
    # The matrix's evidence line carries the total and the lane split.
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"— \*\*(\d+) tests, (\d+) failed, (\d+) ignored\*\* on the current tree \(\s*re-measured\s+"
        r"\d{4}-\d{2}-\d{2} on the track-\d+ tree: (\d+) in-crate,\s*(\d+) integration across "
        r"(\d+) targets, (\d+) doc over\s*(\d+) doc\s+runs",
        "field",
        ("passed", "failed", "ignored", "unit", "integration", "targets", "doc", "doc_runs"),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| Unit tests \| VERIFIED_IMPLEMENTED \| VERIFIED_IMPLEMENTED \| VERIFIED_IMPLEMENTED \| "
        r"(\d+) in-crate tests",
        "field",
        ("unit",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"(\d+) tests across (\d+) checked-in targets",
        "field",
        ("integration", "targets"),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"`tests/http_e2e\.rs` \((\d+) raw-socket",
        "target",
        ("http_e2e",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"`tests/http2_e2e\.rs` \((\d+), five of them",
        "target",
        ("http2_e2e",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"`tests/streaming_request\.rs` \((\d+), three of them",
        "target",
        ("streaming_request",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"pins (\d+) citations over (\d+) spans in (\d+) live documents",
        "field",
        ("citations", "spans", "citation_docs"),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"at (\d+) qualified and (\d+) bare names, (\d+) mentions sheltered by (\d+) per-document "
        r"reasons, (\d+) attributed to another crate, (\d+) design document exempted",
        "field",
        ("qualified", "bare", "sheltered", "sheltered_reasons", "attributed", "exempted"),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"re-measured true here: (\d+) / (\d+) / (\d+),",
        "field",
        ("passed", "failed", "ignored"),
    ),
    # The baseline's header sentence, its total row and its per-target table.
    Pin(
        "framework/docs/BASELINE_MEASUREMENTS.md",
        r"`cargo test --workspace` command now reports \*\*(\d+) in-crate unit tests\*\*",
        "field",
        ("unit",),
    ),
    Pin(
        "framework/docs/BASELINE_MEASUREMENTS.md",
        r"(\d+) result lines, (\d+) passed / (\d+) failed / (\d+) ignored",
        "field",
        ("result_lines", "passed", "failed", "ignored"),
    ),
    Pin(
        "framework/docs/BASELINE_MEASUREMENTS.md",
        r"\| \*\*total\*\* \| \*\*(\d+) passed / (\d+) failed / (\d+) ignored\*\* "
        r"\((\d+) unit \+ (\d+) integration \+ (\d+) doc\) \|",
        "field",
        ("passed", "failed", "ignored", "unit", "integration", "doc"),
    ),
    Pin(
        "framework/docs/BASELINE_MEASUREMENTS.md",
        r"integration \(`framework/tests/tests`.*?: (.*?)\| *(\d+) across (\d+) targets",
        "target_list",
        ("integration", "targets"),
    ),
    Pin(
        "framework/docs/BASELINE_MEASUREMENTS.md",
        r"\| `([a-z0-9-]+)` \(lib\) \| (\d+)",
        "crate_row",
    ),
    Pin(
        "framework/docs/BASELINE_MEASUREMENTS.md",
        r"\| ((?:`[a-z0-9-]+`(?:, )?)+) \| (\d+)",
        "crate_row_list",
    ),
    # The readiness report's live reading, and the two per-target bullets of its evidence section.
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"unit \*\*(\d+)\*\* over (\d+) unit runs, integration \*\*(\d+)\*\* across (\d+) targets,\s*"
        r"doc \*\*(\d+)\*\* over (\d+) doc runs, (\d+) `test result:` lines, total "
        r"\*\*(\d+) passed / (\d+) failed / (\d+) ignored\*\*",
        "field",
        (
            "unit",
            "unit_runs",
            "integration",
            "targets",
            "doc",
            "doc_runs",
            "result_lines",
            "passed",
            "failed",
            "ignored",
        ),
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"`framework/tests/tests/http_e2e\.rs` — (\d+) raw-socket HTTP/1\.1 end-to-end tests",
        "target",
        ("http_e2e",),
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"`framework/tests/tests/http2_e2e\.rs` — (\d+) HTTP/2 wire tests",
        "target",
        ("http2_e2e",),
    ),
    # The readiness report's statement that HTTP/2 is no longer unverified.
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"it has (\d+) tests in\s*`tests/tests/http2_e2e\.rs`",
        "target",
        ("http2_e2e",),
    ),
    # The matrix's status table is compared with a recount of the rows it summarises, not with the
    # prose beside it. The current-reading line is rewritten whenever a row moves; the table cells,
    # the total and the readiness report's copy are all checked against that same recount.
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| VERIFIED_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("VERIFIED_IMPLEMENTED",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| PARTIALLY_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("PARTIALLY_IMPLEMENTED",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| NOT_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("NOT_IMPLEMENTED",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| NOT_APPLICABLE \| (\d+) \|",
        "status_table",
        ("NOT_APPLICABLE",),
    ),
    # The three zero cells are pinned too: a row moved to BROKEN, UNVERIFIED or UNWIRED without a
    # recount would otherwise leave the table claiming zero while the rows hold one.
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| BROKEN \| (\d+) \|",
        "status_table",
        ("BROKEN",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| UNVERIFIED \| (\d+) \|",
        "status_table",
        ("UNVERIFIED",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"\| UNWIRED_IMPLEMENTATION \| (\d+) \|",
        "status_table",
        ("UNWIRED_IMPLEMENTATION",),
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"Total: (\d+) rows\.",
        "status_total",
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"current reading, re-derived from the rows above by the `counts` gate: "
        r"\*\*(\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) = (\d+) rows\*\*",
        "status_reading",
        MATRIX_STATUS_ORDER,
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| VERIFIED_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("VERIFIED_IMPLEMENTED",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| PARTIALLY_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("PARTIALLY_IMPLEMENTED",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| NOT_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("NOT_IMPLEMENTED",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| NOT_APPLICABLE \| (\d+) \|",
        "status_table",
        ("NOT_APPLICABLE",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| BROKEN \| (\d+) \|",
        "status_table",
        ("BROKEN",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| UNVERIFIED \| (\d+) \|",
        "status_table",
        ("UNVERIFIED",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"\| UNWIRED_IMPLEMENTATION \| (\d+) \|",
        "status_table",
        ("UNWIRED_IMPLEMENTATION",),
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"recount reads (\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) over (\d+) rows",
        "status_reading",
        STATUS_ORDER,
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    # The vision's live paragraph repeats the same figures, so it is held to the same run: a copy
    # of a checked number with no reader of its own is the drift this gate exists to catch.
    Pin(
        ".project-governance/OWNER_VISION.md",
        r"The suite is \*\*(\d+) passed / (\d+) failed / (\d+) ignored\*\* over (\d+)\s+"
        r"`test result:` lines",
        "field",
        ("passed", "failed", "ignored", "result_lines"),
    ),
    Pin(
        ".project-governance/OWNER_VISION.md",
        r"The citation gate reads (\d+) citations in (\d+) live documents, (\d+) distinct spans",
        "field",
        ("citations", "citation_docs", "spans"),
    ),
    Pin(
        ".project-governance/OWNER_VISION.md",
        r"resolves (\d+) qualified and (\d+) bare names",
        "field",
        ("qualified", "bare"),
    ),
    Pin(
        ".project-governance/OWNER_VISION.md",
        r"is \*\*(\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) = (\d+) rows\*\*",
        "status_reading",
        MATRIX_STATUS_ORDER,
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
    ),
    # The figures around the figures: the crate count two documents print, the corpus size the
    # document gates read, and the cross-check that sits beside the suite total. Each has a source
    # a reader can open, and none of them was read by any gate.
    Pin("README.md", r"Cargo workspace: (\d+) crates", "workspace_members"),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"(\d+) workspace crates",
        "workspace_members",
    ),
    Pin(
        "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
        r"read an identical corpus \(\d+ → (\d+) documents\)",
        "corpus",
    ),
    Pin(
        "framework/docs/PRODUCTION_READINESS_REPORT.md",
        r"lines sum to \*\*(\d+)\*\*, which is (\d+) plus the\s+single ignored test",
        "field",
        ("running", "passed"),
    ),
    Pin(
        ".project-governance/OWNER_VISION.md",
        r"`running N tests` lines sum to (\d+), which is (\d+) plus the one ignored test",
        "field",
        ("running", "passed"),
    ),
)


def census_from_log(text: str) -> Census:
    """Read the census out of `cargo test --workspace` output, by the project's own rules.

    Unit, integration and doc are told apart by the `Running`/`Doc-tests` line that precedes each
    `test result:`; a summary line without a preceding runner cannot be placed and is refused by
    `check` instead of being counted as unit. One runner owns two summaries: cargo compiles the
    `compile_fail` doctests as a second binary under the same `Doc-tests` marker, so a summary with
    no new marker after a `running N tests` header belongs to that doc lane (track 143).
    """
    census = Census()
    where = ""
    doc_lane = ""
    merged = False
    for raw in text.splitlines():
        line = raw.strip().replace("\\", "/")
        if line.startswith(("Running ", "Doc-tests ")):
            where = line
            doc_lane = line if line.startswith("Doc-tests ") else ""
            merged = False
            continue
        running = RUNNING.match(line)
        if running:
            census.running += int(running.group(1))
            merged = True
            continue
        match = RESULT.match(line)
        if not match:
            continue
        passed, failed, ignored = (int(group) for group in match.groups())
        census.passed += passed
        census.failed += failed
        census.ignored += ignored
        census.result_lines += 1
        lane = where or (doc_lane if merged else "")
        if lane.startswith("Doc-tests"):
            census.doc += passed
            census.doc_runs += 1
        elif lane.startswith("Running tests/"):
            census.integration += passed
            census.targets += 1
            name = lane.split("/", 1)[1].split(".rs", 1)[0]
            census.by_target[name] = census.by_target.get(name, 0) + passed
        else:
            if not lane:
                census.unplaced += 1
            census.unit += passed
            census.unit_runs += 1
            crate = DEP_CRATE.search(lane)
            if crate:
                census.by_crate[crate.group(1).replace("_", "-")] = (
                    census.by_crate.get(crate.group(1).replace("_", "-"), 0) + passed
                )
        where = ""
        merged = False
    return census


def citations_from_log(census: Census, text: str) -> None:
    """Fold the citation gate's own summary into the census.

    `cite_check.py` prints the two lines this reads; a summary that does not parse leaves the
    citation figures at zero, and `check` refuses to compare a document against a zero it cannot
    tell from a real one.
    """
    citations = CITATIONS.search(text)
    names = NAMES.search(text)
    if not citations or not names:
        return
    census.citations, census.citation_docs, census.spans = (int(g) for g in citations.groups())
    (
        census.qualified,
        census.bare,
        census.sheltered,
        census.sheltered_reasons,
        census.attributed,
        census.exempted,
    ) = (int(g) for g in names.groups())


def status_census(text: str) -> tuple[dict[str, int], int]:
    """Count the matrix's five-column rows by the status in the Aether cell.

    The matrix splits its features across 13 tables that share one shape, so the recount is
    file-wide: every row that strips to exactly five cells, is not a `Feature` header and is not a
    separator. The status is the text before the first space or parenthesis in `cells[1]`, so
    `PARTIALLY_IMPLEMENTED (ecosystem)` counts as partial, and the total is every such row -
    including one whose status word is misspelled, which is how a typo shows up as a mismatch
    instead of quietly leaving the table.
    """
    counts: dict[str, int] = {}
    total = 0
    for raw in text.splitlines():
        line = raw.strip()
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if len(cells) != 5 or cells[0] == "Feature":
            continue
        if all(cell and set(cell) <= set("-: ") for cell in cells):
            continue
        total += 1
        status = re.split(r"[ (]", cells[1])[0]
        counts[status] = counts.get(status, 0) + 1
    return counts, total


def workspace_members(root: Path) -> int | None:
    """Count the crates `framework/Cargo.toml` lists as workspace members.

    The pattern anchors on a line that starts with `members`, so `default-members` is not read as
    the member list, and the count is of quoted entries in its bracket.
    """
    path = root / "framework" / "Cargo.toml"
    if not path.is_file():
        return None
    match = MEMBERS.search(path.read_text(encoding="utf-8", errors="replace"))
    if not match:
        return None
    return len(re.findall(r'"[^"]+"', match.group(1)))


def newest_tree_mtime(root: Path) -> float:
    """The newest mtime among the sources the suite is built from."""
    newest = 0.0
    for path in root.rglob("*"):
        if "target" in path.parts or "docs" in path.parts or not path.is_file():
            continue
        if path.suffix == ".rs" or path.name in ("Cargo.toml", "Cargo.lock"):
            newest = max(newest, path.stat().st_mtime)
    return newest


def _number(census: Census, name: str) -> int | None:
    if name.startswith("target:"):
        return census.by_target.get(name.split(":", 1)[1])
    return getattr(census, name)


def _line_of(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def check(
    root: Path,
    log: Path,
    cite_log: Path | None = None,
    *,
    require_fresh: bool = True,
    pins: tuple[Pin, ...] = PINS,
    evidence_log: Path | None = None,
) -> tuple[int, list[Finding]]:
    """Compare every pinned claim against the runs in `log` and `cite_log`."""
    documents = sorted({pin.file for pin in pins})
    logs = [log] if cite_log is None else [log, cite_log]
    evidence = evidence_log or (log.parent / "evidence.log")
    if any(pin.kind == "corpus" for pin in pins):
        logs.append(evidence)
    for path in logs:
        if not path.is_file():
            return len(documents), [
                Finding(
                    path.relative_to(root).as_posix() if path.is_relative_to(root) else str(path),
                    "there is no run to compare the documents against",
                    "run `bash gates.sh` (its tests, cite and evidence gates write these logs)",
                )
            ]

    if require_fresh:
        source = newest_tree_mtime(root / "framework")
        for path in logs:
            if path.stat().st_mtime < source:
                return len(documents), [
                    Finding(
                        path.relative_to(root).as_posix()
                        if path.is_relative_to(root)
                        else str(path),
                        "this log predates the tree it would be compared with",
                        "re-run `bash gates.sh`; a comparison against a stale run reports numbers "
                        "the tree no longer produces",
                    )
                ]

    census = census_from_log(log.read_text(encoding="utf-8", errors="replace"))
    if census.unplaced:
        return len(documents), [
            Finding(
                log.relative_to(root).as_posix() if log.is_relative_to(root) else str(log),
                f"{census.unplaced} `test result:` line(s) have no preceding runner line",
                "a summary that cannot be placed in a lane is not compared; re-run "
                "`cargo test --workspace` and read the whole log",
            )
        ]
    if cite_log is not None:
        citations_from_log(census, cite_log.read_text(encoding="utf-8", errors="replace"))
        if census.citations == 0:
            return len(documents), [
                Finding(
                    cite_log.relative_to(root).as_posix()
                    if cite_log.is_relative_to(root)
                    else str(cite_log),
                    "this log has no citation summary to compare against",
                    "the gate reads the `N citations ...` and `names pass ...` lines; a zero "
                    "would make every citation pin look satisfied",
                )
            ]
    corpus: int | None = None
    if any(pin.kind == "corpus" for pin in pins):
        match = CORPUS.search(evidence.read_text(encoding="utf-8", errors="replace"))
        if not match:
            return len(documents), [
                Finding(
                    evidence.relative_to(root).as_posix()
                    if evidence.is_relative_to(root)
                    else str(evidence),
                    "this log has no corpus summary to compare against",
                    "the gate reads the `N documents scanned` line; an unparsed summary would make "
                    "a corpus pin look satisfied",
                )
            ]
        corpus = int(match.group(1))
    members = workspace_members(root)
    findings: list[Finding] = []
    listed_crates: set[str] = set()
    listed_targets: set[str] = set()
    status_cache: dict[Path, tuple[dict[str, int], int] | None] = {}
    for pin in pins:
        path = root / pin.file
        if not path.is_file():
            findings.append(
                Finding(pin.file, "the document this pin names is missing", pin.pattern)
            )
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        matches = list(re.finditer(pin.pattern, text, re.DOTALL))
        if not matches:
            findings.append(
                Finding(
                    pin.file,
                    "this claim is not in the document any more, so nothing is checked",
                    f"update or remove the pin for: {pin.pattern}",
                )
            )
            continue
        if pin.kind != "crate_row" and len(matches) != 1:
            findings.append(
                Finding(
                    pin.file,
                    f"this claim matches {len(matches)} times; a pin must name one sentence",
                    pin.pattern,
                )
            )
            continue
        for match in matches:
            line = _line_of(text, match.start())
            where = f"{pin.file}:{line}"
            if pin.kind == "crate_row":
                crate, claimed = match.group(1), int(match.group(2))
                listed_crates.add(crate)
                actual = census.by_crate.get(crate)
                if actual is None:
                    findings.append(
                        Finding(
                            where,
                            f"`{crate}` has no unit run in this log",
                            "the table names a crate the run did not build a test binary for",
                        )
                    )
                elif claimed != actual:
                    findings.append(
                        Finding(
                            where,
                            f"`{crate}` is printed with {claimed} tests; the run measured {actual}",
                            "re-derive the row from this run's `test result:` line",
                        )
                    )
                continue
            if pin.kind == "crate_row_list":
                names = re.findall(r"`([a-z0-9-]+)`", match.group(1))
                claimed = int(match.group(2))
                for crate in names:
                    listed_crates.add(crate)
                    actual = census.by_crate.get(crate)
                    if actual is None:
                        findings.append(
                            Finding(
                                where,
                                f"`{crate}` has no unit run in this log",
                                "the table names a crate the run did not build a test binary for",
                            )
                        )
                    elif claimed != actual:
                        findings.append(
                            Finding(
                                where,
                                f"`{crate}` is printed with {claimed} tests; the run measured {actual}",
                                "re-derive the row from this run's `test result:` line",
                            )
                        )
                continue
            if pin.kind == "target_list":
                pairs = re.findall(r"`([a-z0-9_]+)` (\d+)", match.group(1))
                if not pairs:
                    findings.append(
                        Finding(where, "the target list this pin names is empty", pin.pattern)
                    )
                listed_targets.update(name for name, _ in pairs)
                for name, claimed in pairs:
                    actual = census.by_target.get(name)
                    if actual is None:
                        findings.append(
                            Finding(
                                where,
                                f"`{name}` has no test run in this log",
                                "the document names a target the suite did not run",
                            )
                        )
                    elif int(claimed) != actual:
                        findings.append(
                            Finding(
                                where,
                                f"`{name}` is printed with {claimed} tests; the run measured {actual}",
                                "re-derive it from this run's per-target `test result:` line",
                            )
                        )
                for name, group in zip(("integration", "targets"), match.groups()[1:]):
                    claimed, actual = int(group), _number(census, name)
                    if actual is not None and claimed != actual:
                        findings.append(
                            Finding(
                                where,
                                f"the {name} total is printed as {claimed}; the run measured {actual}",
                                "re-derive it from this run",
                            )
                        )
                continue
            if pin.kind in ("status_table", "status_total", "status_reading"):
                source = pin.source or pin.file
                source_path = root / source
                if source_path not in status_cache:
                    if source_path.is_file():
                        status_cache[source_path] = status_census(
                            source_path.read_text(encoding="utf-8", errors="replace")
                        )
                    else:
                        status_cache[source_path] = None
                counted = status_cache[source_path]
                if counted is None:
                    findings.append(
                        Finding(
                            pin.file,
                            f"the rows these numbers count are missing: {source}",
                            "a status count with no table behind it is not compared",
                        )
                    )
                    continue
                counts, total = counted
                if pin.kind == "status_table":
                    expected = counts.get(pin.names[0], 0)
                    claimed = int(match.group(1))
                    if claimed != expected:
                        findings.append(
                            Finding(
                                where,
                                f"`{pin.names[0]}` is printed as {claimed}; the rows count {expected}",
                                f"re-derive the cell from the five-column rows in {source}",
                            )
                        )
                elif pin.kind == "status_total":
                    claimed = int(match.group(1))
                    if claimed != total:
                        findings.append(
                            Finding(
                                where,
                                f"the row total is printed as {claimed}; the table has {total} rows",
                                f"re-derive the total from the five-column rows in {source}",
                            )
                        )
                else:
                    groups = [int(group) for group in match.groups()]
                    for name, claimed in zip(pin.names, groups[:-1]):
                        expected = counts.get(name, 0)
                        if claimed != expected:
                            findings.append(
                                Finding(
                                    where,
                                    f"`{name}` is printed as {claimed}; the rows count {expected}",
                                    f"re-derive the reading from the five-column rows in {source}",
                                )
                            )
                    if groups[-1] != total:
                        findings.append(
                            Finding(
                                where,
                                f"the row total is printed as {groups[-1]}; the table has {total} rows",
                                f"re-derive the total from the five-column rows in {source}",
                            )
                        )
                continue
            if pin.kind == "workspace_members":
                claimed = int(match.group(1))
                if members is None:
                    findings.append(
                        Finding(
                            where,
                            "the manifest this number counts is missing or lists no members",
                            "read `framework/Cargo.toml`; a crate count with no manifest behind it "
                            "is not compared",
                        )
                    )
                elif claimed != members:
                    findings.append(
                        Finding(
                            where,
                            f"the workspace lists {members} crates; this prints {claimed}",
                            "re-derive the count from `framework/Cargo.toml`",
                        )
                    )
                continue
            if pin.kind == "corpus":
                claimed = int(match.group(1))
                if claimed != corpus:
                    findings.append(
                        Finding(
                            where,
                            f"the corpus is printed as {claimed}; the evidence gate scanned {corpus}",
                            "re-derive the sentence from this run",
                        )
                    )
                continue
            if pin.kind == "target":
                claimed, actual = int(match.group(1)), _number(census, f"target:{pin.names[0]}")
                if actual is None:
                    findings.append(
                        Finding(
                            where,
                            f"`{pin.names[0]}` has no test run in this log",
                            "the document names a target the suite did not run",
                        )
                    )
                elif claimed != actual:
                    findings.append(
                        Finding(
                            where,
                            f"`{pin.names[0]}` is printed with {claimed} tests; the run "
                            f"measured {actual}",
                            "re-derive it from this run's per-target `test result:` line",
                        )
                    )
                continue
            for index, name in enumerate(pin.names):
                claimed, actual = int(match.group(index + 1)), _number(census, name)
                if actual is None:
                    findings.append(
                        Finding(where, f"the tool has no metric named {name}", pin.pattern)
                    )
                elif claimed != actual:
                    findings.append(
                        Finding(
                            where,
                            f"{name} is printed as {claimed}; the run measured {actual}",
                            "re-derive the sentence from this run",
                        )
                    )
    # A row for every crate and target the run measured: a new test binary that the table does
    # not name is a count no sentence states, and an updated total can hide it.
    crate_table = next((pin.file for pin in pins if pin.kind.startswith("crate_row")), None)
    if crate_table is not None:
        for name, count in sorted(census.by_crate.items()):
            if count and name not in listed_crates:
                findings.append(
                    Finding(
                        crate_table,
                        f"the run measured {count} tests in `{name}` and no table row names it",
                        "add the crate's row (or the combined zero row) so the count is checked",
                    )
                )
    target_list_file = next((pin.file for pin in pins if pin.kind == "target_list"), None)
    if target_list_file is not None:
        for name in sorted(census.by_target):
            if name not in listed_targets:
                findings.append(
                    Finding(
                        target_list_file,
                        f"the run measured the `{name}` target and the target list does not name it",
                        "add the target to the list so its per-target count is checked",
                    )
                )
    return len(documents), findings


def main(argv: list[str] | None = None, *, pins: tuple[Pin, ...] = PINS) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=str(REPO_ROOT), help="repository root to audit")
    parser.add_argument("--log", default=str(DEFAULT_LOG), help="cargo test log to read")
    parser.add_argument(
        "--cite-log", default=str(DEFAULT_CITE_LOG), help="citation gate log to read"
    )
    parser.add_argument(
        "--evidence-log", default=str(DEFAULT_EVIDENCE_LOG), help="evidence gate log to read"
    )
    args = parser.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    scanned, findings = check(
        Path(args.root).resolve(),
        Path(args.log).resolve(),
        Path(args.cite_log).resolve(),
        pins=pins,
        evidence_log=Path(args.evidence_log).resolve(),
    )
    for finding in findings:
        print(f"FAIL  {finding.where}")
        print(f"      {finding.message}")
        print(f"      {finding.detail}")
    print(f"{scanned} documents scanned, {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
