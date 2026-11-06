"""Regression tests for the evidence-durability gate (track 79).

`evidence_check.py` answers one question per document line: does this machine-absolute path name
something a reader who only has the repository can open? The tests here are about the refusals, for
the same reason the citation tests are: a gate whose pattern quietly stopped matching stays green
and reports the defect it was written to catch.

Each case is a one-line document in a two-document tree, so the glob is exercised too. Three shapes
the first version got wrong are pinned here: `http://` parsed as drive `p:` (a following slash is
refused now), a prose `C:` with no separator (not a path), and the POSIX spelling `file:///etc/x`,
whose remainder used to be read as the relative path `etc/x` and excused as "inside the repo".

The repository test at the bottom runs the gate on this repository's own docs and demands a floor on
the number of documents *and* tokens, because zero documents and zero findings are the same output.

    python -B -m unittest discover -s .project-governance/tools/tests
"""
import contextlib
import importlib.util
import io
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

# Importing the tool would otherwise leave a __pycache__ inside tools/.
sys.dont_write_bytecode = True

TOOL = Path(__file__).resolve().parents[1] / "evidence_check.py"
spec = importlib.util.spec_from_file_location("evidence_check", TOOL)
ec = importlib.util.module_from_spec(spec)
sys.modules["evidence_check"] = ec
spec.loader.exec_module(ec)

REPO = Path(__file__).resolve().parents[3]


def file_url(path: Path) -> str:
    """The `file://` spelling of an absolute path on this machine.

    Git-Bash spells drive C: as `/c/`, which is the spelling this repository's links use, so a
    Windows root becomes `file:///c/...` rather than `file:///C:/...`.
    """
    text = path.as_posix()
    if re.match(r"^[A-Za-z]:/", text):
        return "file:///" + text[0].lower() + text[2:]
    return "file://" + text


def link_into(root: Path, doc: str, target: Path) -> None:
    """Rewrite a fixture document to name `target` in the absolute spelling of its own host."""
    (root / doc).write_text(f"See {file_url(target)}.", encoding="utf-8")


class Tree:
    """A temporary root with the two document globs and one file each."""

    def __init__(self, doc: str, other: str | None = None):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "docs").mkdir()
        (self.root / "docs" / "note.md").write_text(doc, encoding="utf-8")
        if other is not None:
            (self.root / "framework" / "docs").mkdir(parents=True)
            (self.root / "framework" / "docs" / "other.md").write_text(other, encoding="utf-8")

    def close(self):
        self._tmp.cleanup()


class EvidenceCheckTest(unittest.TestCase):
    def tree(self, doc, other=None):
        tree = Tree(doc, other)
        self.addCleanup(tree.close)
        return tree

    def audit(self, doc, other=None):
        tree = self.tree(doc, other)
        scanned, findings = ec.check(tree.root)
        return scanned, findings

    def messages(self, findings):
        return "\n".join(f"{f.where}: {f.message} :: {f.detail}" for f in findings)

    def test_drive_path_outside_the_repository_is_refused(self):
        _, findings = self.audit("Raw log: `C:/tmp/run1.txt` beside the table.")
        self.assertEqual(len(findings), 1, self.messages(findings))
        self.assertEqual(findings[0].where, "docs/note.md:1")
        self.assertIn("C:/tmp/run1.txt", findings[0].detail)
        self.assertIn("outside the repository", findings[0].message)
        self.assertIn("commit the artifact", findings[0].detail)

    def test_backslash_drive_path_is_refused(self):
        _, findings = self.audit(r"Copied from `D:\maps\raw.csv`.")
        self.assertEqual(len(findings), 1, self.messages(findings))
        self.assertIn(r"D:\maps\raw.csv", findings[0].detail)

    def test_unc_path_is_refused(self):
        _, findings = self.audit(r"Sampled from `\\buildhost\share\run.txt`.")
        self.assertEqual(len(findings), 1, self.messages(findings))
        self.assertIn("buildhost", findings[0].detail)

    def test_file_url_outside_the_repository_is_refused(self):
        _, findings = self.audit("Log: `file:///c/tmp/run5.txt`.")
        self.assertEqual(len(findings), 1, self.messages(findings))
        self.assertIn("file:///c/tmp/run5.txt", findings[0].detail)

    def test_posix_file_url_is_read_as_absolute_not_relative(self):
        # `file:///etc/run.txt` used to be stripped to the relative path `etc/run.txt`, which is
        # never outside the root, so this shape passed the gate while naming a machine path. The
        # finding alone cannot prove the fix - its detail echoes the token, so a `"/etc/run.txt" in
        # detail` assertion passes under the old code too - hence the parsed-path assertion.
        self.assertEqual(ec.as_path("file:///etc/run.txt"), Path("/etc/run.txt"))
        _, findings = self.audit("Log: `file:///etc/run.txt`.")
        self.assertEqual(len(findings), 1, self.messages(findings))

    def test_every_spelling_of_one_path_folds_to_it(self):
        # The three Windows spellings must name one file, and the fold is compared as text rather
        # than by `Path` equality: Windows path comparison is case-insensitive, so a lower-case
        # drive would pass an equality check that the report cannot rely on.
        for spelling in ("C:/tmp/x.txt", "file:///c/tmp/x.txt", "file:///c:/tmp/x.txt"):
            self.assertEqual(ec.as_path(spelling).as_posix(), "C:/tmp/x.txt", spelling)

    def test_link_into_the_repository_passes(self):
        tree = self.tree("pending")
        link_into(tree.root, "docs/note.md", tree.root / "framework" / "docs" / "other.md")
        _, findings = ec.check(tree.root)
        self.assertEqual(findings, [], self.messages(findings))
        self.assertEqual(len(ec.citations_in(tree.root / "docs" / "note.md")), 1)

    def test_git_bash_spelling_of_a_repository_path_passes(self):
        # The markdown links in `PRODUCTION_READINESS_REPORT.md` are spelled `file:///c/Users/...`;
        # `as_path` has to fold that back to `C:/Users/...` for the in-repository test to hold.
        if sys.platform != "win32":
            self.skipTest("the Git-Bash drive spelling names a Windows layout")
        tree = self.tree("pending")
        text = (tree.root / "docs" / "note.md").as_posix()
        spelled = "file:///" + text[0].lower() + text[2:]
        self.assertTrue(spelled.startswith("file:///c/"), spelled)
        (tree.root / "docs" / "note.md").write_text(f"See {spelled}.", encoding="utf-8")
        _, findings = ec.check(tree.root)
        self.assertEqual(findings, [], self.messages(findings))
        self.assertEqual(len(ec.citations_in(tree.root / "docs" / "note.md")), 1)

    def test_portable_notation_names_no_machine(self):
        _, findings = self.audit("Logs land in `%TEMP%/run.txt`, `$TMPDIR/run.txt`, `~/run.txt`.")
        self.assertEqual(findings, [])
        self.assertEqual(ec.citations_in(Path(self.tree("").root) / "docs" / "note.md"), [])

    def test_relative_path_and_urls_are_not_machine_paths(self):
        doc = "See `framework/docs/perf/x.txt`, https://example.com/run.txt, http://0.0.0.0:3000/x."
        tree = self.tree(doc)
        _, findings = ec.check(tree.root)
        self.assertEqual(findings, [], self.messages(findings))
        self.assertEqual(ec.citations_in(tree.root / "docs" / "note.md"), [])

    def test_prose_drive_letter_is_not_a_path(self):
        _, findings = self.audit("The status C: column is blank; `C:` alone names no file.")
        self.assertEqual(findings, [])

    def test_both_document_globs_are_read(self):
        scanned, findings = self.audit("Clean.", other="Raw log: `C:/tmp/other.txt`.")
        self.assertEqual(scanned, 2)
        self.assertEqual([f.where for f in findings], ["framework/docs/other.md:1"])

    def test_main_exits_one_and_names_the_document(self):
        tree = self.tree("Raw log: `C:/tmp/run1.txt`.")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = ec.main(["--root", str(tree.root)])
        self.assertEqual(status, 1)
        self.assertIn("FAIL  docs/note.md:1", out.getvalue())
        self.assertIn("1 finding(s)", out.getvalue())

    def test_main_is_clean_when_the_evidence_is_inside(self):
        tree = self.tree("pending")
        link_into(tree.root, "docs/note.md", tree.root / "docs" / "note.md")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = ec.main(["--root", str(tree.root)])
        self.assertEqual(status, 0, out.getvalue())
        self.assertIn("1 documents scanned, 0 finding(s)", out.getvalue())


class RepositoryTest(unittest.TestCase):
    """The gate's own subject: this repository's documents against this repository's layout."""

    def test_every_absolute_path_in_the_docs_resolves_inside_the_repository(self):
        scanned, findings = ec.check(REPO)
        self.assertGreaterEqual(scanned, 30, "the document glob found almost nothing")
        self.assertEqual(findings, [], "\n".join(f"{f.where}: {f.message}" for f in findings))

    def test_the_docs_carry_no_machine_absolute_path(self):
        # Track 104 made this gate's corpus empty on purpose: the last four tokens were the
        # `file:///c/Users/...` links in `PRODUCTION_READINESS_REPORT.md`, and an absolute path
        # resolves on exactly one machine. The old test required at least one such token to keep
        # the pattern exercised; the fixtures above now prove the detector, and this test asserts
        # the stronger invariant -- no document names a path on the machine that wrote it. A token
        # that happens to resolve inside the repository is still a defect here, which is why this
        # checks the tokens rather than the resolver's findings.
        docs = sorted({p for pattern in ec.DOC_GLOBS for p in REPO.glob(pattern) if p.is_file()})
        tokens = [tok for doc in docs for _, tok in ec.citations_in(doc)]
        self.assertEqual(tokens, [], f"machine-absolute path in the docs: {tokens}")

    def test_both_document_gates_read_the_same_corpus(self):
        # Track 109: a document that joins one gate's set and not the other's gets half a review, and
        # the green line a reader sees comes from the gate that never opened it. Both READMEs joined
        # both sets; this is what keeps the two lists from drifting apart again.
        spec_cc = importlib.util.spec_from_file_location(
            "cite_check_for_corpus", Path(__file__).resolve().parents[1] / "cite_check.py"
        )
        cc = importlib.util.module_from_spec(spec_cc)
        spec_cc.loader.exec_module(cc)
        self.assertEqual(ec.DOC_GLOBS, cc.DOC_GLOBS, "the two doc gates read different corpora")
        for readme in ("README.md", "framework/README.md"):
            self.assertIn(readme, ec.DOC_GLOBS, f"{readme} is outside the checked set")


if __name__ == "__main__":
    unittest.main()
