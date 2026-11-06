#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Can a reader who has only this repository still check the evidence a document cites?

Track 79 measured the committed documents against the directories this project actually keeps
evidence in. The governance notes cite scratch paths freely - 71 distinct paths, 120 mentions - but
each of those lines states the reading it was taken from, which is what makes the note survive the
directory. The user-facing set is different - `docs/`, `framework/docs/`, and since track 109 both
READMEs: a path there reads as "this is where the evidence is", and every one of them pointed at a
directory the owner deletes.

The rule this file enforces:

    A machine-absolute path named in a user-facing document must resolve inside the repository.

Portable notation is exempt by construction, because it does not name one machine: `~`, `$TMPDIR`,
`%TEMP%` resolve per-reader and stay true. A drive letter (`C:/...`, `C:\\...`), a `file://` URL or a
UNC path names a layout that existed on the author's disk; if it does not land in the repository, the
number beside it has no durable source and the document has to carry the reading, the command and the
date instead - which is what `notes/2026-10-26-remediation-roadmap.md` §8 asks for.

The rule fails closed. A future document that legitimately wants to name, say, `C:/Program Files`
will produce a finding and the author must either move the fact into portable notation or add an
exemption here with a reason. That is deliberate: a gate that only looks for the paths this track
happened to find is the gate that stays green when the next one appears.

Run it from anywhere:

    python -B .project-governance/tools/evidence_check.py
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import NamedTuple

REPO_ROOT = Path(__file__).resolve().parents[2]
# Track 109 added both READMEs. The root one is the repository's first sentence and states the
# suite totals and the crate count, so it holds exactly the kind of claim this gate exists to keep
# checkable; keeping the corpus identical to `cite_check.DOC_GLOBS` is what stops a document joining
# one gate and quietly never reaching the other (a test asserts the two lists match).
DOC_GLOBS = ("docs/**/*.md", "framework/docs/**/*.md", "README.md", "framework/README.md")

# `C:/x`, `C:\x`, `file:///c/x`, and `\\server\share`. A following character must be a separator so
# `C:` in prose ("status C:") does not match, and the letter must not itself follow a word or be
# followed by another slash, or every `http://` becomes drive `p:`. `%TEMP%`, `$TMPDIR` and `~` are
# not matched at all. The trailing class runs to the end of the path so the finding names the file.
_REST = r"[^ )`\"',\]]*"
ABSOLUTE = re.compile(
    rf"(?:(?<![A-Za-z])[A-Za-z]:[\\/](?!/){_REST}|file:///{_REST}|\\\\[^\\\s]+\\[^\\\s]+{_REST})"
)


class Finding(NamedTuple):
    where: str
    message: str
    detail: str


def strip_token(raw: str) -> str:
    """The path part of a token: drop a URL fragment and trailing punctuation a sentence added."""
    token = raw.split("#", 1)[0]
    return token.rstrip(".,;:)`\"'")


def as_path(token: str) -> Path | None:
    """The local path a token names, or None when it is not a machine-absolute path we read."""
    if token.startswith("file:///"):
        rest = token[len("file:///") :]
        # Git-Bash spells drive C: as /c/, and the markdown links in this repo use that spelling.
        if re.match(r"^[A-Za-z]:[\\/]", rest):
            rest = rest[0].upper() + rest[1:]
        elif re.match(r"^[A-Za-z]/", rest):
            rest = rest[0].upper() + ":" + rest[1:]
        else:
            # `file:///etc/x` is the POSIX spelling: the third slash is the root, not a drive.
            # Reading the remainder as relative would excuse every such link as "inside the repo".
            rest = "/" + rest
        return Path(rest)
    return Path(token)


def citations_in(doc: Path) -> list[tuple[int, str]]:
    hits: list[tuple[int, str]] = []
    for lineno, line in enumerate(doc.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        for m in ABSOLUTE.finditer(line):
            hits.append((lineno, strip_token(m.group(0))))
    return hits


def check(root: Path = REPO_ROOT, docs: list[Path] | None = None) -> tuple[int, list[Finding]]:
    """Scan every user-facing document; returns (documents read, findings)."""
    if docs is None:
        docs = sorted({p for pattern in DOC_GLOBS for p in root.glob(pattern) if p.is_file()})
    findings: list[Finding] = []
    for doc in docs:
        for lineno, token in citations_in(doc):
            path = as_path(token)
            if path is None or path.is_relative_to(root):
                continue
            findings.append(
                Finding(
                    f"{doc.relative_to(root).as_posix()}:{lineno}",
                    "this path is outside the repository, so it is not evidence a reader can check",
                    f"{token} resolves to {path}; commit the artifact, cite it repository-relative, "
                    "or state the reading with the command and the date instead",
                )
            )
    return len(docs), findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=str(REPO_ROOT), help="repository root to audit")
    args = parser.parse_args(argv)
    root = Path(args.root).resolve()
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    scanned, findings = check(root)
    for f in findings:
        print(f"FAIL  {f.where}")
        print(f"      {f.message}")
        print(f"      {f.detail}")
    print(f"{scanned} documents scanned, {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
