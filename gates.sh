#!/usr/bin/env bash
# gates.sh — run every gate this repository has, and exit with the truth.
#
#   bash gates.sh          # run all eight, print one line each, exit 0 or 1
#
# The gates, in the order a reviewer wants them:
#   fmt       cargo fmt --all -- --check
#   clippy    cargo clippy --workspace --all-targets -- -D warnings
#   tests     cargo test --workspace
#   cite      every docs/ path:line checked against the code it names, and every API name those
#             documents assert checked against the names the tree actually declares
#   evidence  every docs/ number checked against a source a reader can open
#   counts    every suite, citation, corpus and workspace figure in the live documents checked
#             against the three logs and the manifest this run just read; it runs after `tests`,
#             `cite` and `evidence` so all three are this run's
#   govtests  the governance tool's own unit tests (after `cite`, so the count gate's own
#             repository test compares the documents with the citation log this run wrote)
#   validate  the ledger validator
#
# `validate` exits 1 by design: four findings are permanent history (two
# recycled PROB IDs, one recycled REQ ID, and REQ-0001 open under a reused ID).
# Silencing them would hide a real hazard, so this script does not silence them.
# It instead pins exactly those four and fails if a fifth appears, if one of the
# four disappears (that would mean the ledger history was rewritten), or if the
# validator exits with anything but 1. That pin is the same set the
# `test_recycling_ratchet` unit test guards; the two must agree.
#
# Logs: target/gates/*.log. Nothing is deleted; a FAIL names its log. Two runs share those
# files, and a run that outlives its terminal writes them concurrently: measured 2026-11-05,
# an interrupted run spliced a second doctest tail into tests.log and made `counts` report 22
# findings against a tree that had none. A second run is refused instead.

set -u

cd "$(dirname "$0")" || { echo "FAIL  env       cannot cd to the repository root"; exit 2; }

LOG_DIR="target/gates"
mkdir -p "$LOG_DIR"

# One run at a time: the logs below are written by name, not per-run, so a second writer can
# only corrupt the first. `mkdir` is the atomic test-and-set this shell has on both platforms.
LOCK_DIR="$LOG_DIR/.lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  holder="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  if [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null; then
    echo "FAIL  env       another gates.sh run holds $LOCK_DIR (pid $holder)"
    exit 2
  fi
  # A crash leaves the directory behind; the pid is gone, so the lock is stale.
  rm -rf "$LOCK_DIR"
  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    echo "FAIL  env       cannot acquire $LOCK_DIR"
    exit 2
  fi
fi
echo $$ >"$LOCK_DIR/pid"
trap 'rm -rf "$LOCK_DIR"' EXIT
trap 'exit 130' INT TERM

if command -v python >/dev/null 2>&1; then
  PY=python
elif command -v python3 >/dev/null 2>&1; then
  PY=python3
else
  echo "FAIL  env       no python or python3 on PATH"
  exit 2
fi

FAILED=0

report() { # name status log
  if [ "$2" -eq 0 ]; then
    printf 'PASS  %-9s\n' "$1"
  else
    printf 'FAIL  %-9s (%s)\n' "$1" "$3"
    FAILED=1
  fi
}

( cd framework && cargo fmt --all -- --check ) >"$LOG_DIR/fmt.log" 2>&1
report fmt $? "$LOG_DIR/fmt.log"

( cd framework && cargo clippy --workspace --all-targets -- -D warnings ) >"$LOG_DIR/clippy.log" 2>&1
report clippy $? "$LOG_DIR/clippy.log"

( cd framework && cargo test --workspace ) >"$LOG_DIR/tests.log" 2>&1
report tests $? "$LOG_DIR/tests.log"

"$PY" -B .project-governance/tools/cite_check.py >"$LOG_DIR/cite.log" 2>&1
report cite $? "$LOG_DIR/cite.log"

"$PY" -B .project-governance/tools/evidence_check.py >"$LOG_DIR/evidence.log" 2>&1
report evidence $? "$LOG_DIR/evidence.log"

"$PY" -B .project-governance/tools/count_check.py >"$LOG_DIR/counts.log" 2>&1
report counts $? "$LOG_DIR/counts.log"

"$PY" -B -m unittest discover -s .project-governance/tools/tests >"$LOG_DIR/govtests.log" 2>&1
report govtests $? "$LOG_DIR/govtests.log"

"$PY" -B .project-governance/tools/project_governance_bootstrap.py validate \
  >"$LOG_DIR/validate.log" 2>"$LOG_DIR/validate.err"
validate_status=$?

P1='[RECYCLED ID] PROB-0007 was allocated 2 times:'
P2='[RECYCLED ID] PROB-0017 was allocated 2 times:'
P3='[RECYCLED ID] REQ-0001 was allocated 3 times:'
P4='[OPEN VS HISTORY] REQ-0001 is open now under an ID that earlier items already used'

pinned_ok=1
for p in "$P1" "$P2" "$P3" "$P4"; do
  [ "$(grep -cF -- "$p" "$LOG_DIR/validate.log")" -eq 1 ] || pinned_ok=0
done
unexpected=$(grep -E '^\[' "$LOG_DIR/validate.log" \
  | grep -vF -e "$P1" -e "$P2" -e "$P3" -e "$P4" | wc -l | tr -d ' ')

if [ "$validate_status" -eq 1 ] && [ "$pinned_ok" -eq 1 ] && [ "$unexpected" -eq 0 ]; then
  printf 'PASS  %-9s (exit 1, the four pinned findings only)\n' validate
  sed 's/^/        /' "$LOG_DIR/validate.log"
else
  printf 'FAIL  %-9s (exit %s, %s unexpected finding(s), %s; %s)\n' \
    validate "$validate_status" "$unexpected" \
    "$([ "$pinned_ok" -eq 1 ] && echo 'pinned set intact' || echo 'pinned set changed')" \
    "$LOG_DIR/validate.log"
  FAILED=1
fi

exit "$FAILED"
