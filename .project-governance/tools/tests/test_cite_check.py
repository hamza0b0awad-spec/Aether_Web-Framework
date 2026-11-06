"""Regression tests for the docs-citation checker (track 80, PROB-0032).

`cite_check.py` is a gate, so the tests here are about its refusals rather than its happy path: a
gate that stays green because it stopped looking is the defect it was written to catch. Each test
builds a four-file tree, points a document at it, and asserts the exact finding.

The cases come from the drift track 80 actually measured on this repository - a coordinate edited
by one line, an edit that moves a span's end while its start stays put, a symbol renamed out from
under a span, a citation added without a pin, a pin left behind after the citation moved, a
basename that matches two files - plus the two forms the corpus uses, `file.rs:12` and a bare `:12`
inherited from the file named earlier on the same line.

Three properties are not incidental. A multi-line span pins both ends, so an edit *inside* the span
fails even though the first line never moved; the first version pinned one line inside the span and
let exactly that through. A pin whose shape disagrees with the span (one regex for a range, or a
pair for a single line) is refused rather than half-checked. And the whole repository must pass,
asserted with a floor on the citation count so the test cannot pass because the glob found nothing.

    python -B -m unittest discover -s .project-governance/tools/tests
"""
import contextlib
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

# Importing the tool would otherwise leave a __pycache__ inside tools/.
sys.dont_write_bytecode = True

TOOL = Path(__file__).resolve().parents[1] / "cite_check.py"
spec = importlib.util.spec_from_file_location("cite_check", TOOL)
cc = importlib.util.module_from_spec(spec)
sys.modules["cite_check"] = cc
spec.loader.exec_module(cc)

REPO = Path(__file__).resolve().parents[3]

SOURCE = """\
fn first() {}

fn second() {
    let charge = 1;
}

fn third() {
    let charge = 2;
}
"""

# The same file with one line inserted above `fn third`, which moves that function from 7 to 8.
SHIFTED = "fn first() {}\n\n\nfn second() {\n    let charge = 1;\n}\n\nfn third() {\n    let charge = 2;\n}\n"


class Tree:
    """A temporary root with a source file, a second file of the same name elsewhere, and a doc."""

    def __init__(self, doc: str, source: str = SOURCE, duplicate: bool = False,
                 product: bool = False):
        # `product` places the source where the names pass looks for the framework's own API. A file
        # outside it is a governance tool's or the site's, and contributes only its stem.
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.src_dir = "framework/src/deep" if product else "src/deep"
        (self.root / self.src_dir).mkdir(parents=True)
        (self.root / "docs").mkdir()
        (self.root / self.src_dir / "app.rs").write_text(source, encoding="utf-8")
        if duplicate:
            (self.root / "src" / "other").mkdir()
            (self.root / "src" / "other" / "app.rs").write_text(source, encoding="utf-8")
        (self.root / "docs" / "note.md").write_text(doc, encoding="utf-8")

    def close(self):
        self._tmp.cleanup()


class CiteCheckTest(unittest.TestCase):
    def audit(self, doc, pins, **kwargs):
        exceptions = kwargs.pop("exceptions", None)
        tree = Tree(doc, **kwargs)
        self.addCleanup(tree.close)
        docs = [tree.root / "docs" / "note.md"]
        return cc.check(
            tree.root,
            docs=docs,
            pins=pins,
            exceptions={} if exceptions is None else exceptions,
        )

    def messages(self, findings):
        return "\n".join(f"{f.where}: {f.message} :: {f.detail}" for f in findings)

    def test_full_path_citation_holds_both_ends(self):
        citations, findings = self.audit(
            "See `src/deep/app.rs:7-9`.",
            {("src/deep/app.rs", 7, 9): (r"fn third\(\) \{", r"^\}$")},
        )
        self.assertEqual(findings, [])
        self.assertEqual(len(citations), 1)

    def test_short_basename_resolves_to_the_single_file(self):
        citations, findings = self.audit(
            "See `app.rs:8`.", {("src/deep/app.rs", 8, 8): r"let charge = 2;"}
        )
        self.assertEqual(findings, [])
        self.assertEqual(citations[0].file_token, "app.rs")

    def test_bare_coordinate_inherits_the_file_on_its_line(self):
        citations, findings = self.audit(
            "The bound (`src/deep/app.rs:7`، `:8`) covers both kinds.",
            {
                ("src/deep/app.rs", 7, 7): r"fn third",
                ("src/deep/app.rs", 8, 8): r"let charge = 2;",
            },
        )
        self.assertEqual(findings, [])
        self.assertEqual([c.file_token for c in citations], ["src/deep/app.rs", "src/deep/app.rs"])
        self.assertEqual([c.start for c in citations], [7, 8])

    def test_bare_coordinate_without_a_file_is_refused(self):
        citations, findings = self.audit("Only `:9` here.", {})
        self.assertEqual(citations, [])
        self.assertIn("names no file on its line", self.messages(findings))

    def test_en_dash_range_is_read_as_a_range(self):
        citations, findings = self.audit(
            "See `src/deep/app.rs:7–9`.",
            {("src/deep/app.rs", 7, 9): (r"fn third\(\) \{", r"^\}$")},
        )
        self.assertEqual(findings, [])
        self.assertEqual((citations[0].start, citations[0].end), (7, 9))

    def test_first_line_moved_fails_with_the_shifted_coordinate(self):
        # A one-line insertion above the function moves `fn third` from 7 to 8; the cited span
        # still exists and still ends on a `}`, so only the pinned first line can catch this.
        _, findings = self.audit(
            "See `src/deep/app.rs:7-9`.",
            {("src/deep/app.rs", 7, 9): (r"fn third\(\) \{", r"^\}$")},
            source=SHIFTED,
        )
        text = self.messages(findings)
        self.assertIn("the span's first line moved", text)
        self.assertIn("pinned at src/deep/app.rs:7; the pattern now matches only at src/deep/app.rs:8", text)
        self.assertIn("re-derive the citation", text)

    def test_edit_inside_the_span_moves_its_end(self):
        # `let total = 3;` inserted *inside* `fn third`: line 7 is untouched, so an anchor-only
        # check is green while the citation's last line is now the initializer and the closing
        # brace has moved to 10. This is the "re-derive both ends" rule.
        source = SOURCE.replace(
            "fn third() {\n    let charge = 2;", "fn third() {\n    let total = 3;\n    let charge = 2;"
        )
        _, findings = self.audit(
            "See `src/deep/app.rs:7-9`.",
            {("src/deep/app.rs", 7, 9): (r"fn third\(\) \{", r"^\}$")},
            source=source,
        )
        text = self.messages(findings)
        self.assertIn("the span's last line moved", text)
        # The last line of a function is `}`, which also closes `fn second`; the report must list
        # both survivors and say the pattern is not unique rather than claim one location.
        self.assertIn("the pattern now matches at src/deep/app.rs:5, 10", text)
        self.assertIn("not unique", text)
        self.assertIn("re-derive both ends", text)

    def test_missing_first_line_names_nowhere_it_went(self):
        _, findings = self.audit(
            "See `src/deep/app.rs:4`.", {("src/deep/app.rs", 4, 4): r"let total = 1;"}
        )
        text = self.messages(findings)
        self.assertIn("the span's first line no longer holds what the pin records", text)
        self.assertIn("matches nowhere in this file", text)

    def test_renamed_symbol_is_reported_with_the_surviving_line(self):
        # The span still exists; the symbol it pinned at its end was replaced by a same-shaped one
        # that also occurs later in the file, so the report can name the line it survives on.
        _, findings = self.audit(
            "See `src/deep/app.rs:4-5`.",
            {("src/deep/app.rs", 4, 5): (r"let charge = 1;", r"let charge = 2;")},
        )
        text = self.messages(findings)
        self.assertIn("the span's last line moved", text)
        self.assertIn("matches only at src/deep/app.rs:8", text)

    def test_pin_shape_must_match_the_span(self):
        _, findings = self.audit(
            "See `src/deep/app.rs:7-9`.", {("src/deep/app.rs", 7, 9): r"fn third"}
        )
        self.assertIn("does not match the span's shape", self.messages(findings))
        _, findings = self.audit(
            "See `src/deep/app.rs:8`.",
            {("src/deep/app.rs", 8, 8): (r"let charge = 2;", r"^\}$")},
        )
        self.assertIn("does not match the span's shape", self.messages(findings))

    def test_citation_without_a_pin_is_refused(self):
        _, findings = self.audit("See `src/deep/app.rs:8`.", {})
        self.assertIn("this span is not pinned", self.messages(findings))

    def test_pin_without_a_citation_is_refused(self):
        _, findings = self.audit(
            "See `src/deep/app.rs:8`.",
            {
                ("src/deep/app.rs", 8, 8): r"let charge = 2;",
                ("src/deep/app.rs", 1, 1): r"fn first",
            },
        )
        self.assertIn("stale pin", self.messages(findings))
        self.assertEqual(sum("stale pin" in f.message for f in findings), 1)

    def test_out_of_bounds_span_is_refused(self):
        _, findings = self.audit("See `src/deep/app.rs:99`.", {})
        self.assertIn("is outside the file", self.messages(findings))

    def test_ambiguous_basename_is_refused(self):
        _, findings = self.audit("See `app.rs:8`.", {}, duplicate=True)
        self.assertIn("is ambiguous", self.messages(findings))

    def test_missing_file_is_refused(self):
        _, findings = self.audit("See `src/nope.rs:1`.", {})
        self.assertIn("no file named 'nope.rs'", self.messages(findings))

    def test_main_exits_nonzero_and_reports(self):
        tree = Tree("See `app.rs:8`.", duplicate=True)
        self.addCleanup(tree.close)
        out = io.StringIO()
        # PINS and NAME_EXCEPTIONS are this repository's tables, keyed by repository-relative
        # paths; a --root fixture has to set both aside or it reports its own absence.
        with mock.patch.object(cc, "PINS", {}), mock.patch.object(cc, "NAME_EXCEPTIONS", {}):
            with contextlib.redirect_stdout(out):
                status = cc.main(["--root", str(tree.root)])
        self.assertEqual(status, 1)
        self.assertIn("ambiguous", out.getvalue())

    def test_main_is_quiet_on_a_clean_tree(self):
        tree = Tree("See `app.rs:7`.", duplicate=False)
        self.addCleanup(tree.close)
        pins = {("src/deep/app.rs", 7, 7): r"fn third"}
        out = io.StringIO()
        with mock.patch.object(cc, "PINS", pins), mock.patch.object(cc, "NAME_EXCEPTIONS", {}):
            with contextlib.redirect_stdout(out):
                status = cc.main(["--root", str(tree.root)])
        self.assertEqual(status, 0, out.getvalue())
        self.assertIn("0 finding(s)", out.getvalue())


class SnapshotTest(unittest.TestCase):
    """Track 92: a dated record is exempt only by a reviewed, visible two-part declaration."""

    MARKER = "<!-- cite-check: snapshot 2026-10-18 -->\n"
    LISTED = {"docs/note.md": "2026-10-18"}

    def audit(self, doc, snapshots, pins=None, source=SOURCE):
        tree = Tree(doc, source=source)
        self.addCleanup(tree.close)
        docs = [tree.root / "docs" / "note.md"]
        with mock.patch.object(cc, "SNAPSHOTS", snapshots):
            return cc.check(tree.root, docs=docs, pins=pins or {}, exceptions={})

    def messages(self, findings):
        return "\n".join(f"{f.where}: {f.message} :: {f.detail}" for f in findings)

    def test_a_listed_snapshot_is_exempt_from_coordinate_checks(self):
        # The citation is outside the file and unpinned; a live document would fail twice.
        citations, findings = self.audit(self.MARKER + "See `src/deep/app.rs:99`.\n", self.LISTED)
        self.assertEqual(findings, [])
        self.assertEqual(citations, [])

    def test_a_listed_snapshot_without_its_marker_is_refused(self):
        _, findings = self.audit("See `src/deep/app.rs:8`.\n", self.LISTED)
        self.assertIn("carries no cite-check marker", self.messages(findings))

    def test_a_snapshot_marker_whose_date_disagrees_is_refused(self):
        doc = self.MARKER.replace("2026-10-18", "2026-10-17") + "See `src/deep/app.rs:8`.\n"
        _, findings = self.audit(doc, self.LISTED)
        self.assertIn("disagrees with SNAPSHOTS", self.messages(findings))

    def test_an_unlisted_document_cannot_exempt_itself(self):
        # The marker is reported, and the citations are still checked: a document may not opt out.
        _, findings = self.audit(self.MARKER + "See `src/deep/app.rs:99`.\n", {})
        text = self.messages(findings)
        self.assertIn("may not declare itself a snapshot", text)
        self.assertIn("is outside the file", text)

    def test_a_bom_before_the_marker_still_marks_the_snapshot(self):
        # `CONSOLIDATED_FINAL_REPORT.md` starts with a BOM; the marker sits after it.
        citations, findings = self.audit("\ufeff" + self.MARKER + "See `src/deep/app.rs:99`.\n", self.LISTED)
        self.assertEqual(findings, [])
        self.assertEqual(citations, [])


class NamesPassTest(unittest.TestCase):
    """Track 109: a document may cite a real line and still name an API that does not exist."""

    SOURCE = """\
pub struct Keeper {
    pub count: u64,
}

impl Keeper {
    pub fn bump(&mut self) {
        self.count += 1;
    }
}

fn helper_fn() {}
"""

    def audit(self, doc, pins, **kwargs):
        exceptions = kwargs.pop("exceptions", {})
        tree = Tree(doc, source=kwargs.pop("source", self.SOURCE), product=True, **kwargs)
        self.addCleanup(tree.close)
        docs = [tree.root / "docs" / "note.md"]
        return cc.check(tree.root, docs=docs, pins=pins, exceptions=exceptions)

    def messages(self, findings):
        return "\n".join(f"{f.where}: {f.message} :: {f.detail}" for f in findings)

    def test_a_name_the_tree_does_not_carry_is_reported(self):
        _, findings = self.audit("The loop calls `helper_fnx` each pass.", {})
        self.assertIn("no name in this tree is `helper_fnx`", self.messages(findings))

    def test_the_same_name_declared_in_the_tree_is_not_reported(self):
        # The pair of the test above: without this, the first test passes for the wrong reason.
        _, findings = self.audit("The loop calls `helper_fn` each pass.", {})
        self.assertEqual(findings, [])

    def test_a_member_of_a_workspace_type_is_checked_by_name(self):
        _, findings = self.audit("Call `Keeper::bump`.", {})
        self.assertEqual(findings, [])
        _, findings = self.audit("Call `Keeper::raise`.", {})
        self.assertIn("`Keeper::raise`, which this tree does not declare", self.messages(findings))

    def test_renaming_the_declaration_makes_the_claim_a_phantom(self):
        # The document does not move, only the source under it. A gate that read the document alone
        # would stay green, so the same span is judged against both trees.
        _, findings = self.audit("Call `Keeper::bump`.", {})
        self.assertEqual(findings, [])
        renamed = self.SOURCE.replace("pub fn bump", "pub fn lift")
        _, findings = self.audit("Call `Keeper::bump`.", {}, source=renamed)
        self.assertIn("`Keeper::bump`, which this tree does not declare", self.messages(findings))

    def test_a_member_attributed_to_another_crate_is_not_judged(self):
        # `spawn` exists nowhere in the fixture; the document says tokio owns it, and the gate has
        # no evidence about tokio. Reporting it would make the gate a list of other crates' APIs.
        _, findings = self.audit("`tokio::spawn` starts the task.", {})
        self.assertEqual(findings, [])
        # ...but only inside the span that names the crate. A bare name is still this document's
        # claim about this tree, so a lint belongs in NAME_EXCEPTIONS with its reason, not in a
        # silent skip - which is exactly what `clippy::type_complexity` costs in the live set.
        _, findings = self.audit("`tokio::spawn` and its `unused_local_variable` friend.", {})
        self.assertIn("no name in this tree is `unused_local_variable`", self.messages(findings))

    def test_a_span_with_a_slash_is_a_path_not_a_name(self):
        # Paths are the citation pass's and evidence_check's subject; `fuzz_targets` is a directory.
        _, findings = self.audit("See `fuzz/fuzz_targets/http_parse_fuzz.rs`.", {})
        self.assertEqual(findings, [])

    def test_a_name_inside_a_fenced_block_is_the_documents_own_example(self):
        doc = "Example:\n\n```rust\nlet x = not_a_real_thing();\n```\n"
        _, findings = self.audit(doc, {})
        self.assertEqual(findings, [])

    def test_an_exception_shelters_its_document_only(self):
        exceptions = {("docs/note.md", "helper_fnx"): "reviewed"}
        _, findings = self.audit("Calls `helper_fnx`.", {}, exceptions=exceptions)
        self.assertEqual(findings, [])
        _, findings = self.audit("Calls `helper_fnx`.", {}, exceptions={("other.md", "helper_fnx"): "x"})
        self.assertIn("no name in this tree is `helper_fnx`", self.messages(findings))

    def test_an_exception_no_longer_cited_is_reported_as_stale(self):
        _, findings = self.audit(
            "Nothing here.", {}, exceptions={("docs/note.md", "gone_name"): "reviewed"}
        )
        self.assertIn("stale name exception", self.messages(findings))

    def test_the_gates_own_prose_is_not_evidence_of_an_api(self):
        # A tool that reads its own writing as a source would shelter any name it mentions. Both
        # prose channels are pinned: the docstring, and the comment - which is how this gate first
        # sheltered `with_json`, by listing it in a comment above its own exception table. The
        # tool's declarations still count, because a document may name `cite_check` legitimately.
        tree = Tree("The response carries `error_idx` and `error_cmt`.", source=self.SOURCE,
                    product=True)
        self.addCleanup(tree.close)
        (tree.root / ".project-governance" / "tools").mkdir(parents=True)
        (tree.root / ".project-governance" / "tools" / "note.py").write_text(
            '"""The response carries `error_idx`, which does not exist."""\n'
            "# `error_cmt` is the same shape, written as a comment.\n"
            "def error_declared() {}\n",
            encoding="utf-8",
        )
        _, findings = cc.check(
            tree.root, docs=[tree.root / "docs" / "note.md"], pins={}, exceptions={}
        )
        text = self.messages(findings)
        self.assertIn("no name in this tree is `error_idx`", text)
        self.assertIn("no name in this tree is `error_cmt`", text)
        self.assertNotIn("error_declared", text)


class ProposalTest(unittest.TestCase):
    """Track 109: a design document names what it proposes; the exemption is a reviewed pair."""

    MARKER = "<!-- cite-check: proposal 2026-10-18 -->\n"
    LISTED = {"docs/note.md": "2026-10-18"}

    def audit(self, doc, proposals, pins=None):
        tree = Tree(doc, source=NamesPassTest.SOURCE)
        self.addCleanup(tree.close)
        docs = [tree.root / "docs" / "note.md"]
        with mock.patch.object(cc, "PROPOSALS", proposals):
            return cc.check(tree.root, docs=docs, pins=pins or {}, exceptions={})

    def messages(self, findings):
        return "\n".join(f.message for f in findings)

    def test_a_listed_proposal_is_exempt_from_names_but_not_from_citations(self):
        doc = self.MARKER + "It will keep `arrival_seq`.\nSee `src/deep/app.rs:99`.\n"
        _, findings = self.audit(doc, self.LISTED)
        self.assertNotIn("arrival_seq", self.messages(findings))
        self.assertIn("is outside the file", self.messages(findings))

    def test_a_listed_proposal_without_its_marker_is_reported(self):
        _, findings = self.audit("It will keep `arrival_seq`.\n", self.LISTED)
        self.assertIn("carries no cite-check marker", self.messages(findings))

    def test_a_document_cannot_exempt_itself(self):
        _, findings = self.audit(
            self.MARKER + "It will keep `arrival_seq`.\n", {}
        )
        self.assertIn("may not declare itself a proposal", self.messages(findings))

    def test_a_marker_that_disagrees_with_the_list_is_reported(self):
        _, findings = self.audit(self.MARKER, {"docs/note.md": "2026-09-01"})
        self.assertIn("disagrees with PROPOSALS", self.messages(findings))


class RepositoryTest(unittest.TestCase):
    """The gate's own subject: this repository's docs against this repository's code."""

    def test_every_citation_in_the_repository_holds_its_anchor(self):
        citations, findings = cc.check(REPO)
        self.assertGreaterEqual(len(citations), 50, "the docs glob found almost nothing")
        self.assertEqual(findings, [], "\n".join(f"{f.where}: {f.message}" for f in findings))

    def test_every_pin_is_used_by_a_citation(self):
        citations, _ = cc.check(REPO)
        index = cc.basename_index(REPO)
        used = set()
        for c in citations:
            src, _ = cc.resolve_file(c.file_token, REPO, index)
            if src is not None:
                used.add((src.relative_to(REPO).as_posix(), c.start, c.end))
        unused = sorted(p for p in cc.PINS if p not in used)
        self.assertEqual(unused, [], f"pins no citation points at: {unused}")

    def test_the_glob_covers_the_teaching_set(self):
        # Track 92's whole point: the teaching documents are inside the gate, not beside it.
        names = {p.relative_to(REPO).as_posix() for p in cc.documents(REPO)}
        self.assertIn("docs/00_framework_specification_ar.md", names)
        self.assertIn("framework/docs/ERROR_HANDLING_GUIDE_AR.md", names)

    def test_the_readmes_are_inside_the_checked_set(self):
        # Track 109: both READMEs state the framework's loudest claims - suite totals, crate count,
        # `AetherApp::enable_h2c` - and no gate read them. The matrix's `README / docs accuracy` row
        # stayed partial for exactly this reason, so coverage is asserted here, not assumed.
        names = {p.relative_to(REPO).as_posix() for p in cc.documents(REPO)}
        self.assertIn("README.md", names)
        self.assertIn("framework/README.md", names)

    def test_every_listed_snapshot_is_present_and_marked(self):
        # Eight is the baseline since track 104 moved PRODUCTION_READINESS_REPORT.md out of
        # the dated set: it is a live document now, so it is checked rather than exempted.
        # A further shrink still has to be a reviewed edit that lowers this number.
        self.assertGreaterEqual(len(cc.SNAPSHOTS), 8, "the snapshot list shrank")
        for rel, date in cc.SNAPSHOTS.items():
            path = REPO / rel
            self.assertTrue(path.is_file(), rel)
            first = path.read_text(encoding="utf-8-sig", errors="replace").splitlines()[0]
            self.assertEqual(first, f"<!-- cite-check: snapshot {date} -->", rel)

    def test_the_names_pass_resolves_the_whole_live_set(self):
        # The floor is the guard against a rule that stops firing: a broken regex or an index that
        # reads nothing still reports zero findings, so the count is what proves it looked.
        stats: dict[str, int] = {}
        _, findings = cc.check(REPO, stats=stats)
        self.assertEqual(findings, [], "\n".join(f"{f.where}: {f.message}" for f in findings))
        self.assertGreaterEqual(stats["qualified"], 300, f"qualified names dropped: {stats}")
        self.assertGreaterEqual(stats["snake"], 1200, f"bare names dropped: {stats}")
        self.assertGreaterEqual(
            stats["excepted"],
            len(cc.NAME_EXCEPTIONS),
            "an exception shelters nothing: every key must be mentioned by a live document",
        )

    def test_the_listed_proposal_is_present_and_marked(self):
        self.assertGreaterEqual(len(cc.PROPOSALS), 1, "the proposal list shrank to nothing")
        for rel, date in cc.PROPOSALS.items():
            path = REPO / rel
            self.assertTrue(path.is_file(), rel)
            first = path.read_text(encoding="utf-8-sig", errors="replace").splitlines()[0]
            self.assertEqual(first, f"<!-- cite-check: proposal {date} -->", rel)


if __name__ == "__main__":
    unittest.main()
