"""Regression tests for `TRACK_INDEX.md` (track 95).

The index is the pointer a reader follows from a track number to the record that holds it, and it
had gone stale three times running: track 93 found its own row one line off, track 94 corrected
that value and was itself one line off by the time track 95 inserted a paragraph into the register,
and the WORKING_CONTEXT rows quoted `lines 6-331` for a heading that had moved to 268. Each number
was right the day it was written. The register (and this index) only grow, so a line number in a
pointer rots without anyone touching the pointer - the repository's own "invariants, not
coordinates" rule, applied to its own bookkeeping.

The rule the index follows now, and what this file enforces:

- a register or roadmap heading is named by quoting it (`## Track 95`), never by a line number;
- each quoted heading is a prefix of exactly one heading in the file that row names, so
  `grep -n '^## Track 95'` finds the record - and an ambiguous or vanished prefix is a finding;
- every `## Track N` heading in the register is claimed by some row, so a track cannot be recorded
  without being indexed;
- the WORKING_CONTEXT ranges stay numeric (that file is frozen history), but both ends of every
  range are checked against the heading the row says sits there.

The checker runs on synthetic text as well as on the repository: a rule that has only ever seen a
compliant input is not known to be able to fail.

    python -B -m unittest discover -s .project-governance/tools/tests
"""
import re
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

REPO = Path(__file__).resolve().parents[3]
GOV = REPO / ".project-governance"
INDEX = GOV / "TRACK_INDEX.md"
REGISTER = GOV / "notes" / "2026-10-21-improvement-register.md"
ROADMAP = GOV / "notes" / "2026-10-26-remediation-roadmap.md"
WORK_CONTEXT = GOV / "WORKING_CONTEXT.md"

QUOTED_HEADING = re.compile(r"`(## [^`]+)`")
LINE_POINTER = re.compile(r"`## [^`]+`\s*\(line \d+\)")
ANCHOR = re.compile(r"lines (\d+)[-\u2013](\d+) \(`([^`]+)`\s*(?:\u2026|\.\.\.)\s*`([^`]+)`\)")


def headings(text):
    """Every `## ` heading, stripped, in file order."""
    return [line.strip() for line in text.splitlines() if line.startswith("## ")]


def rows(text):
    """The cells of every body row of the tables in `text`."""
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 3 or cells[0] in ("Tracks", "") or set(cells[0]) <= set("-: "):
            continue
        yield cells


def target_of(where):
    """"register" / "roadmap" / "work_context", from the row's `Where` cell."""
    if "register heading" in where:
        return "register"
    if "roadmap heading" in where:
        return "roadmap"
    if "WORKING_CONTEXT.md" in where:
        return "work_context"
    return None


def findings(index_text, sources):
    """Every way the index disagrees with the files it points at."""
    problems = []
    claimed = set()

    for cells in rows(index_text):
        where, how = cells[1], cells[2]
        target = target_of(where)

        for pointer in LINE_POINTER.findall(how):
            problems.append(
                f"line-number pointer in row {cells[0]}: {pointer!r} - the register grows, so a "
                "number there is stale by default; quote the heading instead"
            )

        quoted = [m.group(1).strip() for m in QUOTED_HEADING.finditer(how)]
        if target in ("register", "roadmap"):
            pool = headings(sources[target])
            for quote in quoted:
                hits = [heading for heading in pool if heading.startswith(quote)]
                if not hits:
                    problems.append(f"row {cells[0]} points at {quote!r}, which is not a heading in the {target}")
                elif len(hits) > 1:
                    problems.append(
                        f"row {cells[0]} points at {quote!r}, which names {len(hits)} headings in the "
                        f"{target}: {hits}"
                    )
                else:
                    claimed.add(hits[0])
        elif target == "work_context":
            lines = sources["work_context"].splitlines()
            for start, end, first, last in ANCHOR.findall(how):
                for lineno, expected in ((int(start), first), (int(end), last)):
                    actual = lines[lineno - 1].strip() if 0 < lineno <= len(lines) else "<no such line>"
                    if not actual.startswith(expected):
                        problems.append(
                            f"row {cells[0]} says line {lineno} of WORKING_CONTEXT.md is "
                            f"{expected!r}, but it holds {actual[:60]!r}"
                        )

    for heading in headings(sources["register"]):
        if heading.startswith("## Track") and heading not in claimed:
            problems.append(f"{heading!r} is recorded in the register but no row of the index names it")

    return problems


def load():
    return (
        INDEX.read_text(encoding="utf-8"),
        {
            "register": REGISTER.read_text(encoding="utf-8"),
            "roadmap": ROADMAP.read_text(encoding="utf-8"),
            "work_context": WORK_CONTEXT.read_text(encoding="utf-8"),
        },
    )


class RepositoryIndexTest(unittest.TestCase):
    def test_the_repository_index_and_the_files_it_points_at_agree(self):
        index_text, sources = load()
        self.assertEqual(findings(index_text, sources), [])

    def test_the_coverage_rule_has_something_to_cover(self):
        # A compliant index is only meaningful if the register headings were actually read.
        _, sources = load()
        register_tracks = [h for h in headings(sources["register"]) if h.startswith("## Track")]
        self.assertGreaterEqual(len(register_tracks), 20, "the register parse found too few tracks")

    def test_every_register_track_is_indexed(self):
        index_text, sources = load()
        quoted = set()
        for cells in rows(index_text):
            if target_of(cells[1]) == "register":
                quoted.update(m.group(1).strip() for m in QUOTED_HEADING.finditer(cells[2]))
        register_tracks = [h for h in headings(sources["register"]) if h.startswith("## Track")]
        missing = [h for h in register_tracks if not any(h.startswith(q) for q in quoted)]
        self.assertEqual(missing, [])

    def test_the_freezing_claim_about_working_context_holds(self):
        index_text = INDEX.read_text(encoding="utf-8")
        self.assertNotIn("headings stop at line 751", index_text)
        # The one sentence that states where the append-only section starts must name its heading.
        self.assertIn("`## Last Update`", index_text)


class CheckerCanFailTest(unittest.TestCase):
    """The rules above are only rules if a violation is reported."""

    REGISTER_TEXT = "# Register\n\n## Track 1 - one\n\n## Track 2 - two\n"
    ROADMAP_TEXT = "# Roadmap\n\n## 19. the plan\n"
    WORK_CONTEXT_TEXT = "## Active Track\nbody\n## Track 11 - eleven\nbody\n"

    def sources(self, work_context=WORK_CONTEXT_TEXT):
        return {"register": self.REGISTER_TEXT, "roadmap": self.ROADMAP_TEXT, "work_context": work_context}

    def index(self, how_for_track_2, extra=""):
        return (
            "| Tracks | Where | How to find it |\n"
            "|---|---|---|\n"
            "| 1 | register heading | `## Track 1` |\n"
            f"| 2 | register heading | {how_for_track_2} |\n"
            f"| plan | roadmap heading | `## 19.` |\n"
            f"{extra}"
        )

    def test_a_correct_index_is_silent(self):
        text = self.index("`## Track 2`") + "| wc | `WORKING_CONTEXT.md` headings | lines 1-3 (`## Active Track` \u2026 `## Track 11`) |\n"
        self.assertEqual(findings(text, self.sources()), [])

    def test_a_line_number_pointer_is_reported(self):
        problems = findings(self.index("`## Track 2` (line 3579)"), self.sources())
        self.assertTrue(any("line-number pointer in row 2" in p for p in problems), problems)

    def test_a_heading_that_is_not_there_is_reported(self):
        problems = findings(self.index("`## Track 9`"), self.sources())
        self.assertTrue(any("not a heading in the register" in p for p in problems), problems)

    def test_an_ambiguous_prefix_is_reported(self):
        sources = self.sources()
        sources["register"] += "\n## Track 2, slice 1 - again\n"
        problems = findings(self.index("`## Track 2`"), sources)
        self.assertTrue(any("names 2 headings" in p for p in problems), problems)

    def test_an_unindexed_track_is_reported(self):
        text = self.index("`## Track 2`").replace("| 1 | register heading | `## Track 1` |\n", "")
        problems = findings(text, self.sources())
        self.assertTrue(any("no row of the index names it" in p for p in problems), problems)

    def test_a_working_context_range_that_moved_is_reported(self):
        shifted = "filler\n" + self.WORK_CONTEXT_TEXT
        text = self.index("`## Track 2`") + "| wc | `WORKING_CONTEXT.md` headings | lines 1-3 (`## Active Track` \u2026 `## Track 11`) |\n"
        problems = findings(text, self.sources(work_context=shifted))
        self.assertTrue(any("line 1 of WORKING_CONTEXT.md" in p for p in problems), problems)


if __name__ == "__main__":
    unittest.main()
