"""Regression tests for how the governance tool finds the ledger it acts on.

PROB-0025 was measured on 2026-10-25: `project_governance_bootstrap.py new` run with the shell's
current directory inside `framework/` found no ledger there, created a second
`framework/.project-governance/` tree, and allocated **PROB-0001** from its own empty `problems.json`
- a number the real ledger had already used and closed. The root ledger was untouched, `validate` at
the root could not see the stray tree, and nothing failed: the write simply went to a parallel ID
space.

PROB-0026 is the other half of the same root cause, measured on a scratch copy of this repository the
same day: `init` run *on top of an existing ledger* exits 0 and prints `[SUCCESS]` while replacing
`AGENTS.md` and `SYSTEM_PROMPT.md` with the blank template - the files that hold the owner's rules.

These tests pin the two refusals (locate by walking up; never start or overwrite a ledger that exists)
rather than a print statement. Run with `python -B -m unittest discover` from `tools/tests`, or:

    python -B -m unittest discover -s .project-governance/tools/tests
"""
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

# Importing the tool would otherwise leave a __pycache__ inside tools/.
sys.dont_write_bytecode = True

TOOL = Path(__file__).resolve().parents[1] / "project_governance_bootstrap.py"
spec = importlib.util.spec_from_file_location("project_governance_bootstrap", TOOL)
pgb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pgb)


def temp_root():
    """A temporary directory with no ledger in it or above it.

    `locate_ledger` walks to the filesystem root, so a temp tree that inherits someone else's
    `.project-governance` would make every "refuses" assertion pass for the wrong reason.
    """
    path = Path(tempfile.mkdtemp())
    if pgb.locate_ledger(path) is not None:
        raise unittest.SkipTest(f"{path} already has a ledger in an ancestor directory")
    return path


class LedgerLocationTest(unittest.TestCase):
    def setUp(self):
        self.root = temp_root()
        self.gov = self.root / ".project-governance"
        self.gov.mkdir()
        self.write_json("problems.json", [{"id": "PROB-0001", "title": "Real item", "status": "open"}])
        self.write_log([
            {"time": "2026-10-20T00:00:00Z", "action": "new_problem", "id": "PROB-0001", "title": "Real item"},
            {"time": "2026-10-21T00:00:00Z", "action": "done_and_deleted", "id": "PROB-0001", "from": "problems"},
        ])
        self.deep = self.root / "framework" / "aether" / "src"
        self.deep.mkdir(parents=True)

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def write_json(self, name, data):
        (self.gov / name).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    def write_log(self, records):
        with (self.gov / "event_log.jsonl").open("w", encoding="utf-8") as handle:
            for record in records:
                handle.write(json.dumps(record, ensure_ascii=False) + "\n")

    def run_tool(self, *args, cwd):
        return subprocess.run(
            [sys.executable, "-B", str(TOOL), *args],
            cwd=str(cwd), capture_output=True, text=True,
        )

    # --- resolve_root: the walk up -------------------------------------------------------------

    def test_resolve_root_walks_up_from_a_nested_directory(self):
        self.assertEqual(pgb.resolve_root(self.deep), self.root.resolve())

    def test_resolve_root_accepts_the_directory_that_holds_the_ledger(self):
        self.assertEqual(pgb.resolve_root(self.root), self.root.resolve())

    def test_an_empty_governance_directory_does_not_count_as_a_ledger(self):
        # A half-created tree must not shadow the real ledger above it.
        stray = self.deep / ".project-governance"
        stray.mkdir()
        self.assertEqual(pgb.resolve_root(self.deep), self.root.resolve())
        self.assertEqual(list(stray.iterdir()), [])

    def test_a_directory_with_no_ledger_above_it_is_a_refusal(self):
        empty = temp_root() / "scratch"
        empty.mkdir()
        with self.assertRaises(pgb.GovernanceError) as refused:
            pgb.resolve_root(empty)
        message = str(refused.exception)
        self.assertIn("no governance ledger", message)
        self.assertIn("init", message)
        # The refusal must not leave a tree behind - that is the defect, not the message.
        self.assertFalse((empty / ".project-governance").exists())

    # --- the PROB-0025 reproduction, end to end ------------------------------------------------

    def test_new_from_a_subdirectory_continues_the_real_id_space(self):
        result = self.run_tool("new", "problem", "Registered from a subdirectory", cwd=self.deep)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PROB-0002", result.stdout, "the stray ID space is back")
        problems = json.loads((self.gov / "problems.json").read_text(encoding="utf-8"))
        self.assertEqual([item["id"] for item in problems], ["PROB-0001", "PROB-0002"])
        self.assertFalse(
            (self.deep / ".project-governance").exists(),
            "a parallel ledger was created under the subdirectory",
        )
        self.assertIn("[LEDGER]", result.stderr, "the operator is not told which ledger was written")

    def test_status_and_validate_from_a_subdirectory_read_the_real_ledger(self):
        # Two open items sharing an ID only exist in the root ledger. A parallel
        # tree would report `Problems: 0` and `[VALID]`, so this is what proves
        # the read went to the resolved root - `status` prints counts, not IDs.
        self.write_json("problems.json", [
            {"id": "PROB-0001", "title": "Real item", "status": "open"},
            {"id": "PROB-0001", "title": "Duplicated item", "status": "open"},
        ])
        status = self.run_tool("status", cwd=self.deep)
        self.assertEqual(status.returncode, 0, status.stdout + status.stderr)
        self.assertIn("Problems: 2", status.stdout, "status read a ledger other than the root's")
        self.assertNotIn("framework", status.stdout, "status reported the subdirectory as the location")
        self.assertIn("[LEDGER]", status.stderr)
        validate = self.run_tool("validate", cwd=self.deep)
        self.assertEqual(validate.returncode, 1, validate.stdout + validate.stderr)
        self.assertIn("[DUPLICATE ID] PROB-0001", validate.stdout, "validate read a parallel ledger")
        self.assertFalse((self.deep / ".project-governance").exists())

    def test_done_from_a_subdirectory_closes_the_real_item(self):
        result = self.run_tool("done", "PROB-0001", cwd=self.deep)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        problems = json.loads((self.gov / "problems.json").read_text(encoding="utf-8"))
        self.assertEqual(problems, [])
        self.assertIn("done_and_deleted", (self.gov / "event_log.jsonl").read_text(encoding="utf-8"))

    # --- the PROB-0026 refusal: init is only for a repository without a ledger ------------------

    def test_init_refuses_on_top_of_an_existing_ledger(self):
        agents = self.root / "AGENTS.md"
        agents.write_text("# Project Governance Rules\nSENTINEL-OWNER-POLICY\n", encoding="utf-8")
        prompt = self.gov / "SYSTEM_PROMPT.md"
        prompt.write_text("# Owner prompt\nSENTINEL-OWNER-POLICY\n", encoding="utf-8")
        with self.assertRaises(pgb.GovernanceError) as refused:
            pgb.init_project(self.root)
        self.assertIn("already exists", str(refused.exception))
        self.assertIn("SENTINEL-OWNER-POLICY", agents.read_text(encoding="utf-8"))
        self.assertIn("SENTINEL-OWNER-POLICY", prompt.read_text(encoding="utf-8"))

    def test_cli_init_from_a_subdirectory_refuses_and_starts_nothing(self):
        result = self.run_tool("init", cwd=self.deep)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("[ERROR]", result.stderr)
        self.assertFalse((self.deep / ".project-governance").exists())
        self.assertTrue((self.gov / "problems.json").exists())

    def test_init_creates_the_first_ledger_of_a_fresh_repository(self):
        fresh = temp_root() / "newrepo"
        fresh.mkdir()
        self.assertEqual(pgb.init_project(fresh), 0)
        self.assertTrue((fresh / ".project-governance" / "problems.json").exists())
        self.assertTrue((fresh / "AGENTS.md").exists())
        # And the second run of the same command is the refusal, not a silent re-write.
        with self.assertRaises(pgb.GovernanceError):
            pgb.init_project(fresh)

    def test_init_keeps_owner_files_when_it_has_no_ledger_to_refuse(self):
        # A tree can hold owner-written rules and only an empty `.project-governance/`: no ledger to
        # refuse on, so `init` proceeds - and must still not replace the files with its template.
        fresh = temp_root() / "partial"
        (fresh / ".project-governance").mkdir(parents=True)
        (fresh / ".project-governance" / "SYSTEM_PROMPT.md").write_text(
            "# Owner prompt\nSENTINEL-OWNER-POLICY\n", encoding="utf-8")
        (fresh / "AGENTS.md").write_text(
            "# Project Governance Rules\nSENTINEL-OWNER-POLICY\n", encoding="utf-8")
        result = pgb.init_project(fresh)
        self.assertEqual(result, 0)
        self.assertTrue((fresh / ".project-governance" / "problems.json").exists())
        self.assertIn("SENTINEL-OWNER-POLICY", (fresh / "AGENTS.md").read_text(encoding="utf-8"))
        self.assertIn("SENTINEL-OWNER-POLICY",
                      (fresh / ".project-governance" / "SYSTEM_PROMPT.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
