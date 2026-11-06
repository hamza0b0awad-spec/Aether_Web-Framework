"""Regression tests for `count_check.py` (track 119).

A gate that compares a document to a run is only worth its exit code if it fails when the two
disagree. The tests here drive every failure path on a synthetic tree: a wrong total, a wrong lane
split, a wrong per-target or per-crate row, a sentence that was reworded away, a sentence that now
appears twice, a log that is missing, a log older than the tree, and a citation summary that did not
parse. Each one asserts the finding exists, so a gate that stops reporting one of them goes red here
before it goes green there.

The last group runs the real pins against the real documents. Its sentence check needs no logs, so
it holds on a fresh clone; the comparison against the last `gates.sh` run is skipped only when that
run has not happened yet.

    python -B -m unittest discover -s .project-governance/tools/tests
"""
import os
import re
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import count_check  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
TESTS_LOG = REPO / "target" / "gates" / "tests.log"
CITE_LOG = REPO / "target" / "gates" / "cite.log"

LOG_TEXT = """\
   Compiling aether-core v0.1.0
     Running unittests src/lib.rs (target/debug/deps/aether_core-0123456789abcdef)
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running unittests src/lib.rs (target/debug/deps/aether-0123456789abcdef.exe)
running 7 tests
test result: ok. 6 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/wire.rs (target/debug/deps/wire-0123456789abcdef.exe)
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/http2.rs (target/debug/deps/http2-0123456789abcdef.exe)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
   Doc-tests aether
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"""

CITE_TEXT = """\
20 citations in 4 live documents, 9 distinct spans, 0 finding(s)
names pass over 3 live documents: 5 qualified names and 6 bare names resolved against the tree's \
own sources, 1 sheltered by NAME_EXCEPTIONS (1 in the table), 2 attributed to another crate, 0 \
exempted as a proposal
"""

DOC_TEXT = """\
Suite: **20 workspace tests pass, 0 fail, 1 ignored**, with 2 HTTP/2 wire tests.
Split: **20 tests, 0 failures, 1 ignored** - 10 unit + 7 integration + 3 doc tests over 5 `test result:` lines.
Citations: 20 citations over 9 spans in 4 live documents; 5 qualified and 6 bare names.
Wired: `wire` 5, `http2` 2, all 7 across 2 targets.
| `aether` (lib) | 6 |
| `aether-core` (lib) | 4 |
"""

PINS = (
    count_check.Pin(
        "DOC.md",
        r"\*\*(\d+) workspace tests pass, (\d+) fail, (\d+) ignored\*\*",
        "field",
        ("passed", "failed", "ignored"),
    ),
    count_check.Pin(
        "DOC.md",
        r"\*\*(\d+) tests, 0 failures, 1 ignored\*\* - (\d+) unit \+ (\d+) integration \+ "
        r"(\d+) doc tests over (\d+) `test result:` lines",
        "field",
        ("passed", "unit", "integration", "doc", "result_lines"),
    ),
    count_check.Pin("DOC.md", r"with (\d+) HTTP/2 wire tests", "target", ("http2",)),
    count_check.Pin(
        "DOC.md", r"Wired: (.*?)all (\d+) across (\d+) targets", "target_list", ("integration", "targets")
    ),
    count_check.Pin("DOC.md", r"\| `([a-z0-9-]+)` \(lib\) \| (\d+)", "crate_row"),
    count_check.Pin(
        "DOC.md",
        r"(\d+) citations over (\d+) spans in (\d+) live documents",
        "field",
        ("citations", "spans", "citation_docs"),
    ),
    count_check.Pin(
        "DOC.md",
        r"(\d+) qualified and (\d+) bare names",
        "field",
        ("qualified", "bare"),
    ),
)


class CensusTest(unittest.TestCase):
    """The classification the project uses: the runner line decides the lane."""

    def test_the_lanes_and_their_totals(self):
        census = count_check.census_from_log(LOG_TEXT)
        self.assertEqual((census.passed, census.failed, census.ignored), (20, 0, 1))
        # `running` counts what was scheduled, so it carries the ignored test.
        self.assertEqual(census.running, 21)
        self.assertEqual(census.result_lines, 5)
        self.assertEqual((census.unit, census.unit_runs), (10, 2))
        self.assertEqual((census.integration, census.targets), (7, 2))
        self.assertEqual((census.doc, census.doc_runs), (3, 1))
        self.assertEqual(census.by_target, {"wire": 5, "http2": 2})
        self.assertEqual(census.by_crate, {"aether": 6, "aether-core": 4})

    def test_an_absent_run_line_is_refused(self):
        # A `test result:` with no preceding runner cannot be placed in a lane. The totals still
        # include it, but `unplaced` marks the log unreadable rather than letting a guess stand.
        census = count_check.census_from_log("test result: ok. 2 passed; 0 failed; 0 ignored\n")
        self.assertEqual((census.passed, census.result_lines, census.unplaced), (2, 1, 1))

    def test_the_compile_fail_binary_joins_its_doc_lane(self):
        # Cargo compiles `compile_fail` doctests as a second binary under the same `Doc-tests`
        # marker: its summary has a `running N tests` header but no new marker, and it belongs
        # to the doc lane rather than making the log unreadable.
        log = LOG_TEXT + (
            "running 1 test\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; "
            "finished in 0.01s\n"
        )
        census = count_check.census_from_log(log)
        self.assertEqual((census.doc, census.doc_runs), (4, 2))
        self.assertEqual(census.unplaced, 0)
        self.assertEqual((census.result_lines, census.running), (6, 22))

    def test_a_linux_binary_name_still_maps_to_its_crate(self):
        # The `aether_core` runner line in the fixture has no `.exe`; the crate row still checks.
        census = count_check.census_from_log(LOG_TEXT)
        self.assertEqual(census.by_crate.get("aether-core"), 4)

    def test_the_citation_summary_parses_with_and_without_the_clause(self):
        census = count_check.Census()
        count_check.citations_from_log(census, CITE_TEXT)
        self.assertEqual(
            (census.citations, census.citation_docs, census.spans), (20, 4, 9)
        )
        self.assertEqual((census.qualified, census.bare), (5, 6))
        self.assertEqual(
            (census.sheltered, census.sheltered_reasons, census.attributed, census.exempted),
            (1, 1, 2, 0),
        )
        short = count_check.Census()
        count_check.citations_from_log(
            short, CITE_TEXT.replace(" resolved against the tree's own sources,", " resolved,")
        )
        self.assertEqual(short.citations, 20)

    def test_an_unparsed_summary_leaves_zero(self):
        census = count_check.Census()
        count_check.citations_from_log(census, "nothing to see here\n")
        self.assertEqual(census.citations, 0)


class SyntheticTreeTest(unittest.TestCase):
    """Every way the documents can disagree with a run, on a tree small enough to read."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "framework" / "src").mkdir(parents=True)
        (self.root / "framework" / "src" / "lib.rs").write_text("// tree\n", encoding="utf-8")
        (self.root / "gates").mkdir()
        self.log = self.root / "gates" / "tests.log"
        self.cite_log = self.root / "gates" / "cite.log"
        self.log.write_text(LOG_TEXT, encoding="utf-8")
        self.cite_log.write_text(CITE_TEXT, encoding="utf-8")
        self.write_doc(DOC_TEXT)

    def tearDown(self):
        self._tmp.cleanup()

    def write_doc(self, text):
        (self.root / "DOC.md").write_text(text, encoding="utf-8")

    def run_check(self, doc=None, **kwargs):
        if doc is not None:
            self.write_doc(doc)
        return count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False, pins=PINS, **kwargs
        )

    def messages(self, doc=None, **kwargs):
        _, findings = self.run_check(doc, **kwargs)
        return [finding.message for finding in findings]

    def test_a_correct_tree_is_silent(self):
        self.assertEqual(self.messages(), [])

    def test_a_wrong_total_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("**20 workspace", "**19 workspace"))
        self.assertTrue(any("passed is printed as 19" in p for p in problems), problems)

    def test_a_wrong_lane_split_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("10 unit + 7 integration", "11 unit + 7 integration"))
        self.assertTrue(any("unit is printed as 11" in p for p in problems), problems)

    def test_a_wrong_per_target_count_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("with 2 HTTP/2", "with 3 HTTP/2"))
        self.assertTrue(any("`http2` is printed with 3" in p for p in problems), problems)

    def test_an_unknown_target_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("with 2 HTTP/2", "with 2 GRPC"))
        self.assertTrue(any("not in the document any more" in p for p in problems), problems)

    def test_a_wrong_target_list_row_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("`wire` 5", "`wire` 4"))
        self.assertTrue(any("`wire` is printed with 4" in p for p in problems), problems)

    def test_a_target_list_that_names_an_unrun_target_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("`wire` 5, `http2` 2", "`wire` 5, `ghost` 2"))
        self.assertTrue(any("`ghost` has no test run" in p for p in problems), problems)

    def test_a_wrong_crate_row_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("| `aether-core` (lib) | 4 |", "| `aether-core` (lib) | 5 |"))
        self.assertTrue(any("`aether-core` is printed with 5" in p for p in problems), problems)

    def test_a_crate_the_run_did_not_build_is_reported(self):
        problems = self.messages(DOC_TEXT + "| `ghost` (lib) | 1 |\n")
        self.assertTrue(any("`ghost` has no unit run" in p for p in problems), problems)

    def test_a_wrong_citation_count_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("20 citations over", "21 citations over"))
        self.assertTrue(any("citations is printed as 21" in p for p in problems), problems)

    def test_a_removed_sentence_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("with 2 HTTP/2 wire tests.\n", "\n"))
        self.assertTrue(any("not in the document any more" in p for p in problems), problems)

    def test_a_duplicated_sentence_is_reported(self):
        problems = self.messages(DOC_TEXT + "Again: **20 workspace tests pass, 0 fail, 1 ignored**.\n")
        self.assertTrue(any("matches 2 times" in p for p in problems), problems)

    def test_an_empty_target_list_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("`wire` 5, `http2` 2, all", "none at all, all"))
        self.assertTrue(any("target list this pin names is empty" in p for p in problems), problems)

    def test_a_missing_document_is_reported(self):
        (self.root / "DOC.md").unlink()
        _, findings = self.run_check()
        self.assertTrue(any("document this pin names is missing" in f.message for f in findings))

    def test_a_missing_log_is_reported(self):
        (self.root / "DOC.md").write_text(DOC_TEXT, encoding="utf-8")
        self.log.unlink()
        _, findings = count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False, pins=PINS
        )
        self.assertEqual(len(findings), 1)
        self.assertIn("no run to compare", findings[0].message)
        self.assertIn("gates.sh", findings[0].detail)

    def test_a_stale_log_is_reported(self):
        source = self.root / "framework" / "src" / "lib.rs"
        newer = time.time() + 60
        os.utime(source, (newer, newer))
        _, findings = count_check.check(self.root, self.log, self.cite_log, pins=PINS)
        self.assertEqual(len(findings), 1)
        self.assertIn("predates the tree", findings[0].message)

    def test_a_fresh_log_passes_the_freshness_check(self):
        _, findings = count_check.check(self.root, self.log, self.cite_log, pins=PINS)
        self.assertEqual(findings, [])

    def test_a_stale_cite_log_is_reported_on_its_own(self):
        source = self.root / "framework" / "src" / "lib.rs"
        os.utime(source, (time.time() + 60, time.time() + 60))
        os.utime(self.log, (time.time() + 120, time.time() + 120))
        _, findings = count_check.check(self.root, self.log, self.cite_log, pins=PINS)
        self.assertEqual(len(findings), 1)
        self.assertIn("predates the tree", findings[0].message)
        self.assertIn("cite.log", findings[0].where)

    def test_main_returns_the_exit_code_the_finding_demands(self):
        argv = ["--root", str(self.root), "--log", str(self.log), "--cite-log", str(self.cite_log)]
        self.assertEqual(count_check.main(argv, pins=PINS), 0)
        self.write_doc(DOC_TEXT.replace("**20 workspace", "**19 workspace"))
        self.assertEqual(count_check.main(argv, pins=PINS), 1)

    def test_an_unparsed_citation_summary_is_reported(self):
        self.cite_log.write_text("no summary here\n", encoding="utf-8")
        _, findings = count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False, pins=PINS
        )
        self.assertEqual(len(findings), 1)
        self.assertIn("no citation summary", findings[0].message)

    def test_every_finding_names_its_line(self):
        problems = [
            (finding.where, finding.message)
            for finding in self.run_check(DOC_TEXT.replace("**20 workspace", "**19 workspace"))[1]
        ]
        self.assertTrue(problems)
        self.assertRegex(problems[0][0], r"^DOC\.md:\d+$")

    def test_a_runner_less_log_is_a_finding(self):
        self.write_doc(DOC_TEXT)
        self.log.write_text("test result: ok. 2 passed; 0 failed; 0 ignored\n", encoding="utf-8")
        _, findings = count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False, pins=PINS
        )
        self.assertEqual(len(findings), 1)
        self.assertIn("no preceding runner line", findings[0].message)

    def test_a_wrong_failed_or_ignored_count_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("0 fail, 1 ignored", "0 fail, 2 ignored"))
        self.assertTrue(any("ignored is printed as 2" in p for p in problems), problems)

    def test_a_pin_whose_target_did_not_run_is_reported(self):
        pins = PINS + (count_check.Pin("DOC.md", r"GRPC (\d+)", "target", ("grpc",)),)
        (self.root / "DOC.md").write_text(DOC_TEXT + "GRPC 2 tests.\n", encoding="utf-8")
        _, findings = count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False, pins=pins
        )
        self.assertTrue(any("`grpc` has no test run" in f.message for f in findings), findings)

    def test_a_crate_row_list_checks_every_name_it_lists(self):
        pins = PINS + (
            count_check.Pin("DOC.md", r"\| ((?:`[a-z0-9-]+`(?:, )?)+) \| (\d+)", "crate_row_list"),
        )
        (self.root / "DOC.md").write_text(
            DOC_TEXT + "| `aether`, `aether-core` | 6 |\n", encoding="utf-8"
        )
        _, findings = count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False, pins=pins
        )
        self.assertTrue(
            any("`aether-core` is printed with 6 tests" in f.message for f in findings), findings
        )

    def test_a_crate_the_document_does_not_name_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("| `aether-core` (lib) | 4 |\n", ""))
        self.assertTrue(
            any("no table row names it" in p and "aether-core" in p for p in problems), problems
        )

    def test_a_target_the_document_does_not_list_is_reported(self):
        problems = self.messages(DOC_TEXT.replace("`wire` 5, ", ""))
        self.assertTrue(any("target list does not name it" in p for p in problems), problems)

    def test_a_documentation_source_newer_than_the_log_does_not_redden_the_gate(self):
        doc_source = self.root / "framework" / "docs" / "notes.rs"
        doc_source.parent.mkdir(parents=True)
        doc_source.write_text("// an example, not built\n", encoding="utf-8")
        newer = time.time() + 60
        os.utime(doc_source, (newer, newer))
        _, findings = count_check.check(self.root, self.log, self.cite_log, pins=PINS)
        self.assertEqual(findings, [])


MATRIX_TEXT = """\
| Feature | Aether | Axum | Actix Web | Evidence |
| --- | --- | --- | --- | --- |
| One | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `x` |
| Two | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | `y` |
| Three | PARTIALLY_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED | NOT_IMPLEMENTED | `z` |
| Four | NOT_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | `w` |
| Five | NOT_APPLICABLE | NOT_APPLICABLE | NOT_APPLICABLE | `v` |

## Status count (Aether only)

| Status | Rows |
|---|---|
| VERIFIED_IMPLEMENTED | 2 |
| PARTIALLY_IMPLEMENTED | 1 |
| NOT_IMPLEMENTED | 1 |
| NOT_APPLICABLE | 1 |
| BROKEN | 0 |
| UNVERIFIED | 0 |
| UNWIRED_IMPLEMENTATION | 0 |

Total: 5 rows.

The current reading, re-derived from the rows above by the `counts` gate: **2 / 1 / 1 / 0 / 0 / 0 / 1 = 5 rows**.
"""

STATUS_PINS = (
    count_check.Pin(
        "MATRIX.md",
        r"\| VERIFIED_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("VERIFIED_IMPLEMENTED",),
    ),
    count_check.Pin(
        "MATRIX.md",
        r"\| PARTIALLY_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("PARTIALLY_IMPLEMENTED",),
    ),
    count_check.Pin(
        "MATRIX.md",
        r"\| NOT_IMPLEMENTED \| (\d+) \|",
        "status_table",
        ("NOT_IMPLEMENTED",),
    ),
    count_check.Pin(
        "MATRIX.md",
        r"\| NOT_APPLICABLE \| (\d+) \|",
        "status_table",
        ("NOT_APPLICABLE",),
    ),
    count_check.Pin("MATRIX.md", r"\| BROKEN \| (\d+) \|", "status_table", ("BROKEN",)),
    count_check.Pin("MATRIX.md", r"\| UNVERIFIED \| (\d+) \|", "status_table", ("UNVERIFIED",)),
    count_check.Pin(
        "MATRIX.md",
        r"\| UNWIRED_IMPLEMENTATION \| (\d+) \|",
        "status_table",
        ("UNWIRED_IMPLEMENTATION",),
    ),
    count_check.Pin("MATRIX.md", r"Total: (\d+) rows\.", "status_total"),
    count_check.Pin(
        "MATRIX.md",
        r"current reading, re-derived from the rows above by the `counts` gate: "
        r"\*\*(\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) / (\d+) = (\d+) rows\*\*",
        "status_reading",
        count_check.MATRIX_STATUS_ORDER,
    ),
)


class StatusTableTest(unittest.TestCase):
    """The status table has a writer and now a reader: its cells are recounted from its own rows."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "gates").mkdir()
        self.log = self.root / "gates" / "tests.log"
        self.cite_log = self.root / "gates" / "cite.log"
        self.log.write_text(LOG_TEXT, encoding="utf-8")
        self.cite_log.write_text(CITE_TEXT, encoding="utf-8")
        self.matrix = self.root / "MATRIX.md"
        self.write_matrix(MATRIX_TEXT)

    def tearDown(self):
        self._tmp.cleanup()

    def write_matrix(self, text):
        self.matrix.write_text(text, encoding="utf-8")

    def messages(self, text=None):
        if text is not None:
            self.write_matrix(text)
        _, findings = count_check.check(
            self.root,
            self.log,
            self.cite_log,
            require_fresh=False,
            pins=STATUS_PINS,
        )
        return [finding.message for finding in findings]

    def test_the_documented_rule_counts_five_cell_rows_only(self):
        counts, total = count_check.status_census(MATRIX_TEXT)
        self.assertEqual(total, 5)
        self.assertEqual(counts["VERIFIED_IMPLEMENTED"], 2)
        self.assertEqual(counts["PARTIALLY_IMPLEMENTED"], 1)
        self.assertEqual(counts["NOT_APPLICABLE"], 1)

    def test_a_misspelled_status_counts_toward_the_total_only(self):
        text = MATRIX_TEXT.replace("| Four | NOT_IMPLEMENTED |", "| Four | NOT_IMPLEMENTD |")
        counts, total = count_check.status_census(text)
        self.assertEqual(total, 5)
        self.assertEqual(counts.get("NOT_IMPLEMENTED", 0), 0)

    def test_a_correct_status_table_is_silent(self):
        self.assertEqual(self.messages(), [])

    def test_a_wrong_cell_is_reported(self):
        problems = self.messages(MATRIX_TEXT.replace("| VERIFIED_IMPLEMENTED | 2 |", "| VERIFIED_IMPLEMENTED | 3 |"))
        self.assertTrue(
            any("`VERIFIED_IMPLEMENTED` is printed as 3; the rows count 2" in p for p in problems),
            problems,
        )

    def test_a_wrong_total_is_reported(self):
        problems = self.messages(MATRIX_TEXT.replace("Total: 5 rows.", "Total: 6 rows."))
        self.assertTrue(
            any("row total is printed as 6; the table has 5 rows" in p for p in problems), problems
        )

    def test_a_wrong_reading_is_reported(self):
        problems = self.messages(
            MATRIX_TEXT.replace("**2 / 1 / 1 / 0 / 0 / 0 / 1 = 5 rows**", "**3 / 1 / 1 / 0 / 0 / 0 / 1 = 5 rows**")
        )
        self.assertTrue(
            any("`VERIFIED_IMPLEMENTED` is printed as 3; the rows count 2" in p for p in problems),
            problems,
        )

    def test_a_row_that_moves_is_caught_in_the_cells_and_the_reading(self):
        flips = MATRIX_TEXT.replace(
            "| Four | NOT_IMPLEMENTED |", "| Four | VERIFIED_IMPLEMENTED |"
        )
        problems = self.messages(flips)
        self.assertTrue(any("`VERIFIED_IMPLEMENTED` is printed as 2; the rows count 3" in p for p in problems), problems)
        self.assertTrue(any("`NOT_IMPLEMENTED` is printed as 1; the rows count 0" in p for p in problems), problems)

    def test_a_zero_cell_is_reported_when_a_row_moves_into_it(self):
        problems = self.messages(MATRIX_TEXT.replace("| BROKEN | 0 |", "| BROKEN | 1 |"))
        self.assertTrue(
            any("`BROKEN` is printed as 1; the rows count 0" in p for p in problems), problems
        )

    def test_a_reading_follows_the_matrix_tables_order(self):
        moved = (
            MATRIX_TEXT.replace("| Four | NOT_IMPLEMENTED |", "| Four | BROKEN |")
            .replace("| NOT_IMPLEMENTED | 1 |", "| NOT_IMPLEMENTED | 0 |")
            .replace("| BROKEN | 0 |", "| BROKEN | 1 |")
            .replace("**2 / 1 / 1 / 0 / 0 / 0 / 1 = 5 rows**", "**2 / 1 / 0 / 1 / 0 / 0 / 1 = 5 rows**")
        )
        self.assertEqual(self.messages(moved), [])

    def test_a_six_cell_row_is_not_counted(self):
        extra = "| Six | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `u` | extra |\n"
        self.assertEqual(self.messages(MATRIX_TEXT + extra), [])

    def test_a_removed_table_row_is_reported_as_a_lost_claim(self):
        problems = self.messages(MATRIX_TEXT.replace("| PARTIALLY_IMPLEMENTED | 1 |\n", ""))
        self.assertTrue(any("not in the document any more" in p for p in problems), problems)

    def test_a_missing_source_is_reported_rather_than_skipped(self):
        self.write_matrix(MATRIX_TEXT)
        _, findings = count_check.check(
            self.root, self.log, self.cite_log, require_fresh=False,
            pins=(count_check.Pin("MATRIX.md", r"\| VERIFIED_IMPLEMENTED \| (\d+) \|", "status_table",
                                  ("VERIFIED_IMPLEMENTED",), "NOWHERE.md"),),
        )
        self.assertEqual(len(findings), 1)
        self.assertIn("rows these numbers count are missing", findings[0].message)


SOURCE_LOG = """     Running unittests src/lib.rs (target/debug/deps/aether-0123456789abcdef.exe)
running 6 tests
test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/wire.rs (target/debug/deps/wire-0123456789abcdef.exe)
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"""

CARGO_TEXT = """[workspace]
members = [
    "core",
    "http",
    "router",
]
default-members = ["examples"]
"""

SOURCE_DOC = """The tree: Cargo workspace: 3 crates, and the matrix's 3 workspace crates.
The corpus: the two document gates read an identical corpus (33 → 3 documents).
The cross-check: the 2 `running N tests` lines sum to **10**, which is 9 plus the one ignored test.
"""

SOURCE_PINS = (
    count_check.Pin("DOC.md", r"Cargo workspace: (\d+) crates", "workspace_members"),
    count_check.Pin("DOC.md", r"(\d+) workspace crates", "workspace_members"),
    count_check.Pin("DOC.md", r"read an identical corpus \(\d+ → (\d+) documents\)", "corpus"),
    count_check.Pin(
        "DOC.md",
        r"lines sum to \*\*(\d+)\*\*, which is (\d+) plus the one ignored test",
        "field",
        ("running", "passed"),
    ),
)


class TreeSourceTest(unittest.TestCase):
    """The crate count, the corpus size and the cross-check each read a source of their own."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "gates").mkdir()
        (self.root / "framework").mkdir()
        self.log = self.root / "gates" / "tests.log"
        self.evidence = self.root / "gates" / "evidence.log"
        self.log.write_text(SOURCE_LOG, encoding="utf-8")
        self.evidence.write_text("3 documents scanned, 0 finding(s)\n", encoding="utf-8")
        (self.root / "framework" / "Cargo.toml").write_text(CARGO_TEXT, encoding="utf-8")
        self.doc = self.root / "DOC.md"
        self.write_doc(SOURCE_DOC)

    def tearDown(self):
        self._tmp.cleanup()

    def write_doc(self, text):
        self.doc.write_text(text, encoding="utf-8")

    def messages(self, text=None):
        if text is not None:
            self.write_doc(text)
        _, findings = count_check.check(
            self.root,
            self.log,
            None,
            require_fresh=False,
            pins=SOURCE_PINS,
            evidence_log=self.evidence,
        )
        return [finding.message for finding in findings]

    def test_the_crate_and_corpus_sources_are_read(self):
        self.assertEqual(count_check.workspace_members(self.root), 3)
        self.assertEqual(count_check.census_from_log(SOURCE_LOG).running, 10)

    def test_a_correct_tree_is_silent(self):
        self.assertEqual(self.messages(), [])

    def test_a_wrong_crate_count_is_reported(self):
        problems = self.messages(SOURCE_DOC.replace("Cargo workspace: 3 crates", "Cargo workspace: 4 crates"))
        self.assertTrue(any("the workspace lists 3 crates; this prints 4" in p for p in problems), problems)

    def test_the_matrix_copy_is_checked_too(self):
        problems = self.messages(SOURCE_DOC.replace("the matrix's 3 workspace", "the matrix's 4 workspace"))
        self.assertTrue(any("the workspace lists 3 crates; this prints 4" in p for p in problems), problems)

    def test_a_missing_manifest_is_reported(self):
        (self.root / "framework" / "Cargo.toml").unlink()
        problems = self.messages()
        self.assertTrue(any("manifest this number counts is missing" in p for p in problems), problems)

    def test_a_manifest_without_members_is_reported(self):
        (self.root / "framework" / "Cargo.toml").write_text(
            "[workspace]\ndefault-members = [\"examples\"]\n", encoding="utf-8"
        )
        problems = self.messages()
        self.assertTrue(any("manifest this number counts is missing" in p for p in problems), problems)

    def test_a_wrong_corpus_is_reported(self):
        problems = self.messages(SOURCE_DOC.replace("(33 → 3 documents)", "(33 → 4 documents)"))
        self.assertTrue(
            any("the corpus is printed as 4; the evidence gate scanned 3" in p for p in problems), problems
        )

    def test_a_missing_evidence_log_is_reported(self):
        self.evidence.unlink()
        problems = self.messages()
        self.assertTrue(any("there is no run to compare the documents against" in p for p in problems), problems)

    def test_an_unparsed_evidence_log_is_reported(self):
        self.evidence.write_text("nothing to read here\n", encoding="utf-8")
        problems = self.messages()
        self.assertTrue(any("this log has no corpus summary" in p for p in problems), problems)
    def test_a_stale_evidence_log_is_refused(self):
        stale = time.time() - 3600
        os.utime(self.evidence, (stale, stale))
        (self.root / "framework" / "src.rs").write_text("fn main() {}", encoding="utf-8")
        _, findings = count_check.check(
            self.root,
            self.log,
            None,
            require_fresh=True,
            pins=SOURCE_PINS,
            evidence_log=self.evidence,
        )
        self.assertTrue(any("predates the tree" in f.message for f in findings), findings)


    def test_a_wrong_running_sum_is_reported(self):
        problems = self.messages(SOURCE_DOC.replace("sum to **10**", "sum to **11**"))
        self.assertTrue(any("running is printed as 11; the run measured 10" in p for p in problems), problems)

    def test_the_explanation_beside_the_total_is_checked(self):
        problems = self.messages(SOURCE_DOC.replace("which is 9 plus", "which is 8 plus"))
        self.assertTrue(any("passed is printed as 8; the run measured 9" in p for p in problems), problems)


class RepositoryPinsTest(unittest.TestCase):
    def test_every_pin_names_one_live_sentence(self):
        for pin in count_check.PINS:
            path = REPO / pin.file
            self.assertTrue(path.is_file(), f"{pin.file} is missing")
            text = path.read_text(encoding="utf-8")
            matches = list(re.finditer(pin.pattern, text, re.DOTALL))
            if pin.kind == "crate_row":
                self.assertTrue(matches, f"{pin.file}: no crate rows match {pin.pattern!r}")
            else:
                self.assertEqual(
                    len(matches), 1, f"{pin.file}: {pin.pattern!r} matched {len(matches)} times"
                )

    def test_the_pin_set_covers_every_live_count_document(self):
        files = {pin.file for pin in count_check.PINS}
        self.assertEqual(
            files,
            {
                "README.md",
                "framework/README.md",
                "framework/docs/FEATURE_COMPLETENESS_MATRIX.md",
                "framework/docs/BASELINE_MEASUREMENTS.md",
                "framework/docs/PRODUCTION_READINESS_REPORT.md",
                ".project-governance/OWNER_VISION.md",
            },
        )

    def test_the_documents_agree_with_the_last_gates_run(self):
        if not TESTS_LOG.is_file() or not CITE_LOG.is_file():
            self.skipTest("no `bash gates.sh` run yet; its tests and cite gates write these logs")
        scanned, findings = count_check.check(REPO, TESTS_LOG, CITE_LOG, require_fresh=False)
        self.assertEqual(scanned, 6)
        self.assertEqual(findings, [])


if __name__ == "__main__":
    unittest.main()
