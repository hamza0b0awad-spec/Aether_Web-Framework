"""Ratchet on the repository's own ID history.

`validate` on this repository exits 1 and always will: three IDs
(`PROB-0007`, `PROB-0017`, `REQ-0001`) were handed out more than once before
track 53 fixed the allocator, and those facts live in the append-only
`event_log.jsonl`. Removing them means rewriting history, which is refused, so
the red is permanent (see the track-53/54 entries in `WORKING_CONTEXT.md`).

A gate that is permanently red stops carrying information: a *fourth* reuse, or
a second open item sitting on a reused ID, hides inside the same exit code and
the same four lines. That is the defect this file closes. It does not soften
`validate` - the command, its output and its exit code are untouched - it pins
the standing set of violations and fails if the set changes in either
direction, so an owner or agent who adds a violation is told which ID is new.

Updating `STANDING_RECYCLED` / `STANDING_OPEN_ON_RECYCLED` is a deliberate act:
an entry may join only when the ledger itself records the reuse (the first test
recomputes the set from the ledger rather than trusting the printed line), and
an entry may leave only when history has been rewritten, which this project does
not do. The refusal contract is asserted both ways here, not just the pass.
"""
import contextlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

TOOL = Path(__file__).resolve().parents[1] / "project_governance_bootstrap.py"
spec = importlib.util.spec_from_file_location("project_governance_bootstrap", TOOL)
pgb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pgb)

REPO = Path(__file__).resolve().parents[3]

STANDING_RECYCLED = {"PROB-0007", "PROB-0017", "REQ-0001"}
STANDING_OPEN_ON_RECYCLED = {"REQ-0001"}
STANDING_UNLOGGED = frozenset()


def validate_output(root):
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = pgb.validate(root)
    return code, out.getvalue()


def flagged(text, marker):
    """The IDs named by `marker` lines, e.g. '[RECYCLED ID] PROB-0007 was ...'."""
    found = set()
    for line in text.splitlines():
        if line.startswith(marker):
            remainder = line[len(marker):].split()
            if remainder:
                found.add(remainder[0].upper())
    return found


def open_ids(root):
    """Every ID the open lists hold, uppercased the way `ledger_allocations` does."""
    found = set()
    for key in ("requirements", "problems", "proposals", "criticisms"):
        found |= {item["id"].upper() for item in pgb.load_json_list(root, key)}
    return found


def recycled_from_data(root):
    """Every ID the ledger records as allocated twice or more, computed here.

    Deliberately independent of `validate`'s own printing, so the baseline is
    checked against the data and not against the report of the data.
    """
    return {
        item_id.upper()
        for item_id, uses in pgb.ledger_allocations(root).items()
        if len(uses) >= 2
    }


def forged_pair(root):
    """One ID the open lists hold and one they do not, each allocated exactly once.

    This is the fixture for `test_a_new_reuse_is_not_absorbed_by_the_standing_set`,
    derived rather than named. The first version hard-coded PROB-0028 as "an ID
    that is open today", and the day a track closed it the test read its own
    missing `[OPEN VS HISTORY]` line as a detector failure — a red gate whose fix
    would have been to re-open an item or widen the standing set. What the
    fixture needs is the relationship (one reuse only the ledger can see, one the
    open lists also hold), not the names.
    """
    single = {
        item_id
        for item_id, uses in pgb.ledger_allocations(root).items()
        if len(uses) == 1
    } - STANDING_RECYCLED
    open_now = open_ids(root)
    closed, reused_open = sorted(single - open_now), sorted(single & open_now)
    if not closed or not reused_open:
        raise AssertionError(
            "this test cannot state which IDs to forge: it needs at least one ID "
            f"allocated once and closed (found {closed}) and one allocated once and "
            f"still open (found {reused_open}). An empty open ledger would make the "
            "sensitivity it pins untestable, so say so instead of skipping."
        )
    return closed[0], reused_open[-1]


class RealLedgerRatchetTest(unittest.TestCase):
    def setUp(self):
        self.root = pgb.resolve_root(REPO)
        self.assertIsNotNone(self.root, "no governance ledger above this test file")

    def copy_ledger(self, target):
        gov = target / ".project-governance"
        gov.mkdir()
        for name in pgb.LEDGER_MARKERS:
            source = self.root / ".project-governance" / name
            if source.exists():
                shutil.copyfile(source, gov / name)
        return gov

    def test_standing_reuses_are_exactly_the_reuses_the_ledger_supports(self):
        code, out = validate_output(self.root)
        printed = flagged(out, "[RECYCLED ID] ")
        self.assertEqual(
            printed,
            STANDING_RECYCLED,
            "the set of recycled IDs changed. A new member is a real integrity break: "
            "an ID was handed out twice after track 53 made that impossible, so find the "
            "ledger line that allocated it and correct the allocation, never this test. "
            f"A vanished member means history was rewritten. Full output:\n{out}",
        )
        self.assertEqual(printed, recycled_from_data(self.root))
        self.assertEqual(
            code,
            1,
            "the ledger holds recycled IDs, so `validate` is red; recording it green "
            "would be false, and a silent exit here means the detector broke",
        )

    def test_one_open_item_on_a_reused_id_is_the_standing_state(self):
        # The live ambiguity of this repository, pinned rather than renumbered: REQ-0001
        # is the open mission and also the number two earlier, different requirements
        # held. Renumbering it would rewrite 20+ dated citations across the notes for
        # one fewer flag line, so the standing set names it instead and any *second*
        # open item on a reused ID shows up here as an addition.
        _, out = validate_output(self.root)
        self.assertEqual(
            flagged(out, "[OPEN VS HISTORY] "),
            STANDING_OPEN_ON_RECYCLED,
            f"an open item now sits on a reused ID beyond the known one:\n{out}",
        )
        self.assertEqual(STANDING_OPEN_ON_RECYCLED, STANDING_OPEN_ON_RECYCLED & STANDING_RECYCLED)

    def test_no_other_integrity_flag_is_standing(self):
        _, out = validate_output(self.root)
        self.assertEqual(flagged(out, "[UNLOGGED ID] "), STANDING_UNLOGGED)
        self.assertNotIn("[DUPLICATE ID]", out)
        self.assertNotIn("[DUPLICATE TITLE]", out)

    def test_a_new_reuse_is_not_absorbed_by_the_standing_set(self):
        # Sensitivity, on a copy of the real ledger with two forged second
        # allocations: one of a closed ID (only the ledger can see it) and one of an
        # ID that is open today (so both detectors must fire). The standing set must
        # be reported unchanged around them, and the new names must appear as
        # themselves rather than blending into the three red lines already there.
        # Which IDs play those two roles comes from `forged_pair`, not from a literal.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            gov = self.copy_ledger(root)
            closed_id, open_id = forged_pair(root)
            with (gov / "event_log.jsonl").open("a", encoding="utf-8") as log:
                for forged, title in ((closed_id, "closed, reused"),
                                      (open_id, "open, reused")):
                    log.write(
                        json.dumps(
                            {
                                "time": "2026-10-26T00:00:00Z",
                                "action": "new_problem",
                                "id": forged,
                                "title": title,
                            }
                        )
                        + "\n"
                    )
            code, out = validate_output(root)
            printed = flagged(out, "[RECYCLED ID] ")
            self.assertEqual(
                printed,
                STANDING_RECYCLED | {closed_id, open_id},
                f"the forged reuses must appear as themselves and the standing set must "
                f"be reported unchanged around them:\n{out}",
            )
            self.assertEqual(
                flagged(out, "[OPEN VS HISTORY] "),
                STANDING_OPEN_ON_RECYCLED | {open_id},
                f"only the reused-and-open ID should join the open set:\n{out}",
            )
            self.assertEqual(
                code, 1, "a recycled ID must keep the gate red, not merely be listed"
            )
            self.assertNotIn(
                f"[OPEN VS HISTORY] {closed_id}",
                out,
                f"{closed_id} is closed, so only the ledger can expose its reuse",
            )

    def test_pruning_the_history_is_the_only_way_to_silence_the_gate(self):
        # The alternate exit, stated as a fact rather than an assumption: keep one
        # allocation per ID and `validate` goes green. That is why the red cannot be
        # fixed by a code change and why rewriting `event_log.jsonl` is refused.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            gov = self.copy_ledger(root)
            lines = (gov / "event_log.jsonl").read_text(encoding="utf-8").splitlines()
            seen = set()
            kept = []
            dropped = 0
            for line in lines:
                if not line.strip():
                    continue
                record = json.loads(line)
                item_id = record.get("id")
                if record.get("action") in pgb.ALLOCATION_ACTIONS and isinstance(item_id, str):
                    if item_id.upper() in seen:
                        dropped += 1
                        continue
                    seen.add(item_id.upper())
                kept.append(line)
            self.assertGreater(dropped, 0, "the real ledger must hold more than one allocation of some ID")
            with (gov / "event_log.jsonl").open("w", encoding="utf-8") as log:
                for line in kept:
                    log.write(line + "\n")
            code, out = validate_output(root)
            self.assertEqual(code, 0, out)
            self.assertIn("[VALID]", out)


if __name__ == "__main__":
    unittest.main()
