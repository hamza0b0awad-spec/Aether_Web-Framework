"""Regression tests for governance ID allocation.

The tool deletes an item from its list when it is closed, and `next_id` used to
take the maximum over the items still present. Closing the highest-numbered
item therefore made its number available again, and the event log already holds
three such reuses (PROB-0007, PROB-0017, REQ-0001). These tests pin the fix:
allocation also considers every ID the ledger has ever recorded, so a closed
number is never handed out twice.

The same file pins the refusal contract: promotion creates the replacement
before deleting the source, closure rolls back when its record cannot be
written, and an unreadable or malformed file is a `GovernanceError` rather than
a traceback after a list has changed.
"""
import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

# Importing the tool would otherwise leave a __pycache__ inside tools/. This
# covers the tool; run the suite with `python -B -m unittest` so the cache for
# this test module itself is never written either.
sys.dont_write_bytecode = True

TOOL = Path(__file__).resolve().parents[1] / "project_governance_bootstrap.py"
spec = importlib.util.spec_from_file_location("project_governance_bootstrap", TOOL)
pgb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pgb)


class IdAllocationTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / ".project-governance").mkdir()

    def tearDown(self):
        self._tmp.cleanup()

    def write_json(self, name, data):
        (self.root / ".project-governance" / name).write_text(
            json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8"
        )

    def write_log(self, records):
        path = self.root / ".project-governance" / "event_log.jsonl"
        with path.open("w", encoding="utf-8") as f:
            for rec in records:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    def add(self, kind, title):
        self.assertEqual(pgb.add_item(self.root, kind, title), 0)
        return json.loads(
            (self.root / ".project-governance" / (kind + "s.json")).read_text(encoding="utf-8")
        )[-1]["id"]

    def read_list(self, name):
        return json.loads(
            (self.root / ".project-governance" / name).read_text(encoding="utf-8")
        )

    def problem(self, title="the problem"):
        return {"id": "PROB-0001", "title": title, "description": "details",
                "created_at": "", "status": "open", "type": "problem"}

    def test_closing_the_highest_id_does_not_recycle_it(self):
        self.write_json("problems.json", [])
        self.write_log([
            {"time": "2026-10-20T00:00:00Z", "action": "new_problem", "id": "PROB-0001", "title": "first"},
        ])
        self.assertEqual(self.add("problem", "first replacement"), "PROB-0002")
        self.assertEqual(pgb.done_item(self.root, "PROB-0002"), 0)
        self.assertEqual(self.add("problem", "second replacement"), "PROB-0003")

    def test_the_real_prob_0017_shape(self):
        # Two different problems were created under this ID: the second must not
        # reuse it even though the ledger's newest entry for it was a deletion.
        self.write_json("problems.json", [])
        self.write_log([
            {"time": "2026-10-21T12:45:14Z", "action": "new_problem", "id": "PROB-0017", "title": "websocket bypass"},
            {"time": "2026-10-21T12:57:10Z", "action": "done_and_deleted", "id": "PROB-0017", "from": "problems"},
            {"time": "2026-10-23T13:43:59Z", "action": "new_problem", "id": "PROB-0017", "title": "write buffer"},
            {"time": "2026-10-23T18:59:37Z", "action": "done_and_deleted", "id": "PROB-0017", "from": "problems"},
        ])
        self.assertEqual(self.add("problem", "next problem"), "PROB-0018")

    def test_open_items_and_the_ledger_both_count(self):
        self.write_json("problems.json", [
            {"id": "PROB-0005", "title": "open one", "description": "", "created_at": "", "status": "open", "type": "problem"},
        ])
        self.write_log([
            {"time": "2026-10-20T00:00:00Z", "action": "new_problem", "id": "PROB-0009", "title": "closed high"},
            {"time": "2026-10-20T00:01:00Z", "action": "done_and_deleted", "id": "PROB-0009", "from": "problems"},
        ])
        self.assertEqual(self.add("problem", "another problem"), "PROB-0010")

    def test_prefixes_do_not_borrow_each_other_numbers(self):
        self.write_json("requirements.json", [])
        self.write_log([
            {"time": "2026-10-20T00:00:00Z", "action": "new_problem", "id": "PROB-0009", "title": "a problem"},
        ])
        self.assertEqual(self.add("requirement", "a requirement"), "REQ-0001")

    def test_without_a_ledger_the_first_id_is_still_0001(self):
        self.write_json("problems.json", [])
        self.assertEqual(self.add("problem", "only problem"), "PROB-0001")

    def test_a_malformed_ledger_line_does_not_break_allocation(self):
        self.write_json("problems.json", [])
        path = self.root / ".project-governance" / "event_log.jsonl"
        path.write_text(
            '{"action": "new_problem", "id": "PROB-0004", "title": "valid"}\n'
            "not json at all\n"
            '{"action": "new_problem", "id": "PROB-0005", "title": "also valid"}\n',
            encoding="utf-8",
        )
        self.assertEqual(self.add("problem", "after garbage"), "PROB-0006")

    def test_a_malformed_line_is_reported_and_the_valid_lines_still_count(self):
        # The declared behaviour, not just "allocation survived": the corrupt
        # line is named on stderr, the records around it are still read, and a
        # reader is never told the ledger was empty.
        self.write_json("problems.json", [])
        path = self.root / ".project-governance" / "event_log.jsonl"
        path.write_text(
            '{"action": "new_problem", "id": "PROB-0004", "title": "valid"}\n'
            "not json at all\n"
            '{"action": "new_problem", "id": "PROB-0005", "title": "also valid"}\n',
            encoding="utf-8",
        )
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            ids = pgb.ledger_ids(self.root, "PROB")
        self.assertEqual(ids, {"PROB-0004", "PROB-0005"})
        self.assertIn("line 2", err.getvalue())

    def test_non_object_lines_and_non_string_ids_are_ignored(self):
        self.write_json("problems.json", [])
        path = self.root / ".project-governance" / "event_log.jsonl"
        path.write_text(
            "[1, 2]\n"
            '{"action": "new_problem", "id": 7, "title": "numeric id"}\n'
            '{"action": "new_problem", "id": "PROB-0003", "title": "valid"}\n',
            encoding="utf-8",
        )
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            ids = pgb.ledger_ids(self.root, "PROB")
        self.assertEqual(ids, {"PROB-0003"})
        self.assertIn("line 1", err.getvalue())
        # A numeric id is a well-formed record, only not one of ours: no warning.
        self.assertNotIn("line 2", err.getvalue())
        self.assertEqual(self.add("problem", "continues after the noise"), "PROB-0004")

    def test_promote_item_allocates_through_the_ledger(self):
        # `promote_item` deletes the problem and then calls `add_item`, so the
        # new requirement's number must clear the ledger, not just the (now
        # empty) requirements list.
        self.write_json("requirements.json", [])
        self.write_json("problems.json", [
            {"id": "PROB-0001", "title": "the problem", "description": "details",
             "created_at": "", "status": "open", "type": "problem"},
        ])
        self.write_log([
            {"time": "2026-10-18T00:00:00Z", "action": "new_requirement", "id": "REQ-0001", "title": "old requirement"},
            {"time": "2026-10-18T00:01:00Z", "action": "done_and_deleted", "id": "REQ-0001", "from": "requirements"},
            {"time": "2026-10-18T00:02:00Z", "action": "new_problem", "id": "PROB-0001", "title": "the problem"},
        ])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(pgb.promote_item(self.root, "PROB-0001", "the promoted work"), 0)
        reqs = json.loads(
            (self.root / ".project-governance" / "requirements.json").read_text(encoding="utf-8")
        )
        self.assertEqual([r["id"] for r in reqs], ["REQ-0002"])
        self.assertEqual(reqs[0]["description"], "details")
        problems = json.loads(
            (self.root / ".project-governance" / "problems.json").read_text(encoding="utf-8")
        )
        self.assertEqual(problems, [])

    def test_promote_refuses_a_corrupt_file_before_deleting_the_source(self):
        # The old order deleted the source, logged the promotion, and only then
        # let `add_item` read the files; a corrupt file therefore cost the item
        # while the ledger claimed the promotion had happened.
        self.write_json("problems.json", [self.problem()])
        reqs = self.root / ".project-governance" / "requirements.json"
        reqs.write_text("{ not a list", encoding="utf-8")

        with self.assertRaises(pgb.GovernanceError):
            pgb.promote_item(self.root, "PROB-0001", "promoted work")

        self.assertEqual(self.read_list("problems.json"), [self.problem()])
        self.assertEqual(reqs.read_text(encoding="utf-8"), "{ not a list")
        self.assertFalse((self.root / ".project-governance" / "event_log.jsonl").exists())

        err = io.StringIO()
        old_argv = sys.argv
        sys.argv = ["governance", "--root", str(self.root), "promote", "PROB-0001", "promoted work"]
        try:
            with contextlib.redirect_stderr(err):
                code = pgb.main()
        finally:
            sys.argv = old_argv
        self.assertEqual(code, 2)
        self.assertIn("requirements.json", err.getvalue())
        self.assertNotIn("Traceback", err.getvalue())
        self.assertEqual(self.read_list("problems.json"), [self.problem()])

    def test_promote_rolls_back_the_requirement_when_the_ledger_refuses(self):
        # The replacement is written before its ledger record. If that record
        # cannot be written, the replacement must not survive: the promotion
        # did not happen and the source is still open.
        self.write_json("requirements.json", [])
        self.write_json("problems.json", [self.problem()])
        with mock.patch.object(pgb, "log", side_effect=pgb.GovernanceError("ledger is unwritable")):
            with self.assertRaises(pgb.GovernanceError):
                pgb.promote_item(self.root, "PROB-0001", "promoted work")
        self.assertEqual(self.read_list("requirements.json"), [])
        self.assertEqual(self.read_list("problems.json"), [self.problem()])

    def test_promote_keeps_the_source_when_the_new_title_is_taken(self):
        # The duplicate-title check used to run after the source was deleted.
        existing = {"id": "REQ-0005", "title": "taken title", "description": "",
                    "created_at": "", "status": "open", "type": "requirement"}
        self.write_json("requirements.json", [existing])
        self.write_json("problems.json", [self.problem()])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(pgb.promote_item(self.root, "PROB-0001", "taken title"), 2)
        self.assertEqual(self.read_list("requirements.json"), [existing])
        self.assertEqual(self.read_list("problems.json"), [self.problem()])

    def test_promote_accepts_an_unchanged_title(self):
        # The duplicate-title check sees the source itself; excluding that one
        # ID is what lets a promotion keep the item's wording.
        self.write_json("requirements.json", [])
        self.write_json("problems.json", [self.problem()])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(pgb.promote_item(self.root, "PROB-0001", "the problem"), 0)
        reqs = self.read_list("requirements.json")
        self.assertEqual([(r["id"], r["title"]) for r in reqs], [("REQ-0001", "the problem")])
        self.assertEqual(self.read_list("problems.json"), [])

    def test_log_reports_a_path_it_cannot_append_to(self):
        # An unwritable ledger used to raise a traceback after the caller had
        # already changed a list; it is a `GovernanceError` like a bad read.
        path = self.root / ".project-governance" / "event_log.jsonl"
        path.mkdir()
        with self.assertRaises(pgb.GovernanceError) as raised:
            pgb.log(self.root, "new_problem", {"id": "PROB-0001", "title": "x"})
        self.assertIn("cannot be written", str(raised.exception))

    def run_validate(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = pgb.validate(self.root)
        return code, out.getvalue()

    def test_validate_flags_a_recycled_id_even_when_one_use_is_open(self):
        self.write_json("requirements.json", [
            {"id": "REQ-0001", "title": "current mission", "description": "",
             "created_at": "", "status": "open", "type": "requirement"},
        ])
        self.write_log([
            {"time": "2026-10-18T16:09:29Z", "action": "new_requirement", "id": "REQ-0001", "title": "first mission"},
            {"time": "2026-10-18T16:17:47Z", "action": "done_and_deleted", "id": "REQ-0001", "from": "requirements"},
            {"time": "2026-10-18T17:11:49Z", "action": "new_requirement", "id": "REQ-0001", "title": "current mission"},
        ])
        code, out = self.run_validate()
        self.assertEqual(code, 1)
        self.assertIn("[RECYCLED ID] REQ-0001", out)
        self.assertIn("[OPEN VS HISTORY] REQ-0001", out)

    def test_validate_flags_a_closed_reuse_too(self):
        # PROB-0017's real shape: both uses are closed, so no open item is
        # involved and only the ledger can expose it.
        self.write_json("problems.json", [])
        self.write_log([
            {"time": "2026-10-21T12:45:14Z", "action": "new_problem", "id": "PROB-0017", "title": "websocket bypass"},
            {"time": "2026-10-21T12:57:10Z", "action": "done_and_deleted", "id": "PROB-0017", "from": "problems"},
            {"time": "2026-10-23T13:43:59Z", "action": "new_problem", "id": "PROB-0017", "title": "write buffer"},
            {"time": "2026-10-23T18:59:37Z", "action": "done_and_deleted", "id": "PROB-0017", "from": "problems"},
        ])
        code, out = self.run_validate()
        self.assertEqual(code, 1)
        self.assertIn("[RECYCLED ID] PROB-0017", out)
        self.assertNotIn("[OPEN VS HISTORY]", out)

    def test_validate_accepts_a_clean_ledger(self):
        self.write_json("problems.json", [
            {"id": "PROB-0002", "title": "still open", "description": "",
             "created_at": "", "status": "open", "type": "problem"},
        ])
        self.write_log([
            {"time": "2026-10-18T00:00:00Z", "action": "new_problem", "id": "PROB-0001", "title": "closed"},
            {"time": "2026-10-18T00:01:00Z", "action": "done_and_deleted", "id": "PROB-0001", "from": "problems"},
            {"time": "2026-10-18T00:02:00Z", "action": "new_problem", "id": "PROB-0002", "title": "still open"},
        ])
        code, out = self.run_validate()
        self.assertEqual(code, 0, out)
        self.assertIn("[VALID]", out)

    def test_validate_still_flags_duplicate_titles(self):
        self.write_json("problems.json", [
            {"id": "PROB-0001", "title": "same words", "description": "",
             "created_at": "", "status": "open", "type": "problem"},
            {"id": "PROB-0002", "title": "Same   Words", "description": "",
             "created_at": "", "status": "open", "type": "problem"},
        ])
        code, out = self.run_validate()
        self.assertEqual(code, 1)
        self.assertIn("[DUPLICATE TITLE]", out)

    def test_validate_keeps_reading_the_ledger_around_a_corrupt_line(self):
        # The owner's question, as a test: one corrupt line must not discard the
        # valid records that surround it.
        self.write_json("problems.json", [])
        path = self.root / ".project-governance" / "event_log.jsonl"
        path.write_text(
            '{"action": "new_problem", "id": "PROB-0017", "title": "first"}\n'
            '{"action": "done_and_deleted", "id": "PROB-0017", "from": "problems"}\n'
            "garbage\n"
            '{"action": "new_problem", "id": "PROB-0017", "title": "second"}\n',
            encoding="utf-8",
        )
        out = io.StringIO()
        err = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = pgb.validate(self.root)
        self.assertEqual(code, 1)
        self.assertIn("[RECYCLED ID] PROB-0017", out.getvalue())
        self.assertIn("line 3", err.getvalue())

    def test_validate_flags_an_open_id_with_no_allocation_record(self):
        # Hand-inserted or merged-in IDs are still counted while open, but they
        # carry no allocation record, so the high-water mark does not know them.
        self.write_json("problems.json", [
            {"id": "PROB-0009", "title": "merged in by hand", "description": "",
             "created_at": "", "status": "open", "type": "problem"},
        ])
        self.write_log([
            {"time": "2026-10-18T00:00:00Z", "action": "new_problem", "id": "PROB-0002", "title": "closed"},
            {"time": "2026-10-18T00:01:00Z", "action": "done_and_deleted", "id": "PROB-0002", "from": "problems"},
        ])
        code, out = self.run_validate()
        self.assertEqual(code, 1)
        self.assertIn("[UNLOGGED ID] PROB-0009", out)
        self.assertNotIn("[RECYCLED ID]", out)

    def test_a_corrupt_list_file_refuses_instead_of_restarting_numbering(self):
        # An unparsable list used to be read as [], so the next allocation
        # started again at PROB-0001 and then overwrote the damaged file: a
        # silent reset of the high-water mark and a loss of open items.
        path = self.root / ".project-governance" / "problems.json"
        path.write_text("{ not a list", encoding="utf-8")
        with self.assertRaises(pgb.GovernanceError):
            pgb.add_item(self.root, "problem", "after corruption")
        self.assertEqual(path.read_text(encoding="utf-8"), "{ not a list")

        # Through the CLI the refusal must be visible and non-zero, not a traceback.
        err = io.StringIO()
        old_argv = sys.argv
        sys.argv = ["governance", "--root", str(self.root), "new", "problem", "after corruption"]
        try:
            with contextlib.redirect_stderr(err):
                code = pgb.main()
        finally:
            sys.argv = old_argv
        self.assertEqual(code, 2)
        self.assertIn("problems.json", err.getvalue())
        self.assertEqual(path.read_text(encoding="utf-8"), "{ not a list")

    def test_a_non_utf8_ledger_is_refused_not_crashed(self):
        # The decode step raises `UnicodeDecodeError`, which is a ValueError:
        # the reader guarded only `OSError`, so the CLI printed a traceback.
        self.write_json("problems.json", [])
        path = self.root / ".project-governance" / "event_log.jsonl"
        path.write_bytes(b'{"action": "new_problem", "id": "PROB-0001"}\n\xff\n')
        with self.assertRaises(pgb.GovernanceError) as raised:
            pgb.ledger_ids(self.root, "PROB")
        self.assertIn("cannot be read", str(raised.exception))

        err = io.StringIO()
        old_argv = sys.argv
        sys.argv = ["governance", "--root", str(self.root), "validate"]
        try:
            with contextlib.redirect_stderr(err):
                code = pgb.main()
        finally:
            sys.argv = old_argv
        self.assertEqual(code, 2)
        self.assertIn("[ERROR]", err.getvalue())
        self.assertNotIn("Traceback", err.getvalue())

    def test_a_list_item_that_is_not_an_object_is_refused(self):
        # The file-level shape (a list) was checked; the element-level shape
        # was not, so a number in the list crashed every reader downstream.
        path = self.root / ".project-governance" / "problems.json"
        path.write_text("[1]", encoding="utf-8")
        with self.assertRaises(pgb.GovernanceError) as raised:
            pgb.load_json_list(self.root, "problems")
        self.assertIn("item 1", str(raised.exception))
        self.assertEqual(path.read_text(encoding="utf-8"), "[1]")

    def test_a_list_item_must_carry_a_string_id_and_title(self):
        cases = [
            {"title": "no id"},
            {"id": 7, "title": "numeric id"},
            {"id": "PROB-0001"},
            {"id": "PROB-0001", "title": 5},
            {"id": "PROB-0001", "title": "t", "description": None},
        ]
        for bad in cases:
            with self.subTest(item=bad):
                self.write_json("problems.json", [bad])
                with self.assertRaises(pgb.GovernanceError):
                    pgb.validate(self.root)
        self.assertEqual(self.read_list("problems.json"), [cases[-1]])

    def test_a_bad_element_reaches_the_cli_as_an_error(self):
        self.write_json("problems.json", [{"id": 7, "title": "numeric"}])
        err = io.StringIO()
        old_argv = sys.argv
        sys.argv = ["governance", "--root", str(self.root), "status"]
        try:
            with contextlib.redirect_stderr(err):
                code = pgb.main()
        finally:
            sys.argv = old_argv
        self.assertEqual(code, 2)
        self.assertIn("problems.json", err.getvalue())
        self.assertNotIn("Traceback", err.getvalue())

    def test_done_keeps_the_item_when_the_ledger_refuses(self):
        # `done_item` removed the item and then wrote the record; a refused
        # write left the item gone with no `done_and_deleted` line.
        self.write_json("problems.json", [self.problem()])
        with mock.patch.object(pgb, "log", side_effect=pgb.GovernanceError("ledger is unwritable")):
            with self.assertRaises(pgb.GovernanceError):
                pgb.done_item(self.root, "PROB-0001")
        self.assertEqual(self.read_list("problems.json"), [self.problem()])


if __name__ == "__main__":
    unittest.main()
