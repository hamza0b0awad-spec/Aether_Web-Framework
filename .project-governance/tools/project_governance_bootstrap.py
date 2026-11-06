#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Project Governance Bootstrap v4.1 - Fully English Clean Edition
- All console output, file contents, comments, and instructions in English.
- `init` creates the first ledger of a repository: SYSTEM_PROMPT.md, AGENTS.md, OWNER_VISION.md and
  PROJECT_MAP.md. It refuses when a ledger already exists at or above the target, and it never
  replaces a governance file that already has content.
- Other commands locate that ledger by walking up from the target directory, and refuse when no
  repository around them has one.
- Supports external paths via --root. The search walks up, so `init` for a nested directory inside an
  existing repository is refused along with the rest; initialise that project outside this tree.
"""
import json, re, sys, shutil, os
from pathlib import Path
from datetime import datetime, timezone

TOOL_VERSION = "4.1-clean-en"
GOV_DIR = ".project-governance"
TOOLS_DIR = "tools"
FILES = {
    "requirements": "requirements.json",
    "problems": "problems.json",
    "proposals": "proposals.json",
    "criticisms": "criticisms.json",
    "project_map": "PROJECT_MAP.md",
    "owner_vision": "OWNER_VISION.md",
    "working_context": "WORKING_CONTEXT.md",
    "system_prompt": "SYSTEM_PROMPT.md",
    "event_log": "event_log.jsonl",
}
EXCLUDED_DIRS = {".git",".hg",".svn",".idea",".venv","venv","env","node_modules","target","dist","build","out","__pycache__",".mypy_cache",".project-governance","bin","obj",".next",".turbo"}
EXCLUDED_NAMES = {".env",".env.local","credentials.json","secrets.json","id_rsa"}

def now(): return datetime.now(timezone.utc).isoformat()[:19]+"Z"
def norm(s): return re.sub(r'\s+', ' ', s.strip().lower())

class GovernanceError(Exception):
    """A governance file cannot be trusted: refuse the command instead of guessing."""

LEDGER_MARKERS = ("problems.json", "requirements.json", "proposals.json", "criticisms.json",
                  "event_log.jsonl")


def locate_ledger(start):
    """The nearest directory at or above `start` that already holds a governance ledger.

    The ledger is identified by its data files, not by the directory name alone, so an empty
    `.project-governance/` left behind by an interrupted run does not shadow the real one above it.
    """
    current = Path(start).resolve()
    for candidate in (current, *current.parents):
        gov = candidate / GOV_DIR
        if gov.is_dir() and any((gov / name).exists() for name in LEDGER_MARKERS):
            return candidate
    return None


def resolve_root(start):
    """The root a governance command must act on: found by walking up, never invented.

    Resolving the root from the current directory alone let `new` run from a subdirectory create a
    second `.project-governance/` with its own empty ledger, and the first ID it allocated
    (`PROB-0001`) was one the real ledger had used and closed years earlier. The root `validate`
    reads would never have seen that item, so nothing failed. Measurement: 2026-10-25, PROB-0025.
    """
    found = locate_ledger(start)
    if found is None:
        raise GovernanceError(
            f"no governance ledger in {Path(start).resolve()} or any parent directory. These "
            f"commands act on the ledger that holds the ID history, so they will not start a new "
            f"one here: run them inside a project that has `{GOV_DIR}/`, pass --root, or use "
            f"`init` to create the first ledger of a repository."
        )
    return found

def load_json_list(root, key):
    p = root / GOV_DIR / FILES[key]
    if not p.exists(): return []
    # An unparsable list used to be read as [], which silently restarted the
    # numbering at 0001 and then overwrote the file with the new items.
    try:
        data = json.loads(p.read_text(encoding='utf-8'))
    except (OSError, ValueError) as exc:
        raise GovernanceError(f"{p} cannot be read as JSON: {exc}") from exc
    if not isinstance(data, list):
        raise GovernanceError(f"{p} must contain a JSON list, found {type(data).__name__}")
    # The file shape is checked above; these are the fields every reader uses,
    # so an element that lacks them is refused before `.upper()` or `norm()`
    # meets it.
    for number, item in enumerate(data, 1):
        if not isinstance(item, dict):
            raise GovernanceError(f"{p} item {number} is not a JSON object")
        if not isinstance(item.get('id'), str) or not isinstance(item.get('title'), str):
            raise GovernanceError(f"{p} item {number} needs a string \"id\" and \"title\"")
        if 'description' in item and not isinstance(item['description'], str):
            raise GovernanceError(f"{p} item {number} has a non-string \"description\"")
    return data

def save_json_list(root, key, data):
    p = root / GOV_DIR / FILES[key]
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(data, ensure_ascii=False, indent=2)+"\n", encoding='utf-8')

def log(root, action, data):
    p = root / GOV_DIR / FILES["event_log"]
    p.parent.mkdir(parents=True, exist_ok=True)
    rec={"time":now(),"action":action,**data}
    # A write failure is reported like a read failure: the CLI turns it into
    # `[ERROR] …` and exit 2 instead of a traceback, and callers can roll back.
    try:
        with p.open("a",encoding="utf-8") as f: f.write(json.dumps(rec,ensure_ascii=False)+"\n")
    except OSError as exc:
        raise GovernanceError(f"{p} cannot be written: {exc}") from exc

def _ledger_records(root):
    """Yield the event log's records, naming every line it cannot read.

    Both the high-water mark and the reuse check depend on the whole ledger, so
    a line that does not parse is reported on stderr with its line number and
    skipped: the rest of the ledger still counts, and nothing is silently
    treated as absent. A log that cannot be read at all is refused, because an
    empty ledger would restart the numbering.
    """
    p = root / GOV_DIR / FILES["event_log"]
    if not p.exists(): return
    try:
        text = p.read_text(encoding='utf-8')
    except (OSError, UnicodeDecodeError) as exc:
        # `read_text` decodes, so invalid UTF-8 fails with a ValueError, not
        # an OSError: both are refusals, not tracebacks.
        raise GovernanceError(f"{p} cannot be read: {exc}") from exc
    for number, line in enumerate(text.splitlines(), 1):
        line=line.strip()
        if not line: continue
        try: rec=json.loads(line)
        except ValueError:
            print(f"[WARN] {FILES['event_log']} line {number} is not valid JSON; "
                  f"any ID it recorded cannot be considered", file=sys.stderr)
            continue
        if not isinstance(rec, dict):
            print(f"[WARN] {FILES['event_log']} line {number} is not a JSON object; "
                  f"any ID it recorded cannot be considered", file=sys.stderr)
            continue
        yield rec

def ledger_ids(root, prefix):
    """Every ID this prefix has ever had, including closed and deleted ones.

    `done_item` removes an item from its list, so the list alone is not a
    high-water mark: closing the newest item would hand its number out again.
    The event log is append-only and keeps every allocation.

    Corrupt lines are a declared limit, not an exception: each one is named on
    stderr and skipped, the valid lines around it still count, and a record
    whose `id` is not a string contributes nothing. Skipping means an ID that
    exists only on a corrupt line is invisible here; `validate` reports the
    same lines, so the gap is visible rather than silent.
    """
    found=set()
    for rec in _ledger_records(root):
        value=rec.get('id')
        if isinstance(value, str) and value.upper().startswith(prefix.upper()+"-"):
            found.add(value.upper())
    return found

ALLOCATION_ACTIONS = {"new_requirement","new_problem","new_proposal","new_criticism"}

def ledger_allocations(root):
    """How many times each ID was handed out: {ID: [(time, title), ...]}.

    Only allocation records count: `done_and_deleted` and `promoted` carry an
    ID without handing out a new one, so counting them would make every closed
    item look like a reuse. Same malformed-line limit as `ledger_ids`.
    """
    found={}
    for rec in _ledger_records(root):
        action=rec.get('action'); value=rec.get('id')
        if action in ALLOCATION_ACTIONS and isinstance(value, str):
            found.setdefault(value.upper(), []).append((rec.get('time',''), rec.get('title','')))
    return found

def next_id(items, prefix, extra_ids=()):
    nums=[]
    for it in items:
        m=re.search(r'(\d+)', it.get('id',''))
        if m: nums.append(int(m.group(1)))
    for i in extra_ids:
        m=re.search(r'(\d+)', i)
        if m: nums.append(int(m.group(1)))
    return f"{prefix}-{max(nums)+1:04d}" if nums else f"{prefix}-0001"

# ===== PROJECT MAP - Smart inspect =====
EXECUTABLE_EXTS = {".rs",".py",".ts",".tsx",".js",".jsx",".go",".java",".kt",".cs",".cpp",".c",".h",".hpp",".swift",".php",".rb",".dart",".sh"}
EXECUTABLE_NAMES = {"cargo.toml","package.json","tauri.conf.json","pyproject.toml","go.mod","pom.xml","build.gradle","makefile"}

def walk_project(root, max_entries=500):
    files=[]
    for cur, dirs, fnames in os.walk(root, topdown=True):
        cp=Path(cur)
        dirs[:]=[d for d in dirs if d not in EXCLUDED_DIRS and not d.startswith(".") or d in {".vscode",".github",".clinerules"}]
        for fn in sorted(fnames):
            p=cp/fn
            if fn in EXCLUDED_NAMES: continue
            suf=p.suffix.lower()
            is_executable = suf in EXECUTABLE_EXTS or fn.lower() in EXECUTABLE_NAMES
            if not is_executable: continue
            if p.stat().st_size>800_000: continue
            try: rel=p.relative_to(root).as_posix()
            except: rel=p.as_posix()
            if rel.startswith(".project-governance/"): continue
            files.append(p)
            if len(files)>=max_entries: return files, True
    return files, False

def smart_describe_file(path, root):
    name=path.name.lower()
    suf=path.suffix.lower()
    try: parent=path.parent.relative_to(root).as_posix()
    except: parent="root"
    
    content = ""
    try: content = path.read_text(encoding='utf-8', errors='ignore')[:3000]
    except: content = ""
    
    if name=="cargo.toml": return "Rust Config", "Defines dependencies and package parameters"
    if name=="package.json": return "Node Config", "Dependencies, scripts, and package settings"
    if name=="main.rs": return "Main Entry Point", "Application bootstrap and runtime initialization"
    
    fns = re.findall(r'fn\s+(\w+)', content)
    comps = re.findall(r'(?:function|const|class)\s+(\w+)', content)
    
    if suf in {".rs"}: return f"Rust Code | {parent}", f"Key functions: {', '.join(fns[:4])}" if fns else "Source executable"
    elif suf in {".ts", ".tsx"}: return f"TypeScript | {parent}", f"Exports: {', '.join(comps[:3])}" if comps else "UI / logic file"
    elif suf == ".py": return f"Python Script | {parent}", "Python script or module"
    else: return f"Executable {suf} | {parent}", "Source code file"

def detect_stack(root, files):
    names = {p.name.lower() for p in files}
    suffixes = {p.suffix.lower() for p in files}
    stack = []
    if "cargo.toml" in names or ".rs" in suffixes: stack.append("Rust")
    if "package.json" in names or suffixes & {".js", ".jsx", ".ts", ".tsx"}: stack.append("JavaScript/TypeScript")
    if "pyproject.toml" in names or "requirements.txt" in names or ".py" in suffixes: stack.append("Python")
    if "tauri.conf.json" in names: stack.append("Tauri")
    if "go.mod" in names or ".go" in suffixes: stack.append("Go")
    return list(dict.fromkeys(stack)) or ["Undefined Stack"]

def generate_project_map(root):
    files, truncated = walk_project(root)
    stack = detect_stack(root, files)
    lines=[]
    lines.append("# PROJECT MAP - Current Repository Evidence Map")
    lines.append(f"_Generated: {now()} by {TOOL_VERSION}_")
    lines.append(f"_Root Path: {root}_")
    lines.append("_Regenerate with: `python -B .project-governance/tools/project_governance_bootstrap.py map`_")
    lines.append("")
    lines.append("## 🧰 Detected Stack")
    for s in stack: lines.append(f"- {s}")
    lines.append("")
    lines.append("## ⚡ Source & Config Files")
    lines.append(f"Total Files Scanned: {len(files)} executable/config files "
                 "(markdown, data files, files over 800 KB and `.project-governance/` excluded)")
    lines.append("")
    lines.append("| Path | Summary | Description |")
    lines.append("|---|---|---|")
    
    for p in sorted(files, key=lambda x: (len(x.parts), x.as_posix())):
        try: rel=p.relative_to(root).as_posix()
        except: rel=p.name
        short, detail = smart_describe_file(p, root)
        lines.append(f"| `{rel}` | {short} | {detail} |")
        
    if truncated:
        lines.append("\n> Scan limited to 500 executable files.")
        
    lines.append("\n## 📂 Governance Directory Structure")
    lines.append("| File | Role |")
    lines.append("|---|---|")
    lines.append("| requirements.json | Active tasks - deleted upon completion |")
    lines.append("| problems.json | Active bugs or obstacles |")
    lines.append("| proposals.json | Proposed additions requiring approval |")
    lines.append("| criticisms.json | Architecture or code quality concerns |")
    lines.append("| PROJECT_MAP.md | Generated repository file index |")
    lines.append("| OWNER_VISION.md | Read-only owner intent and constraints |")
    lines.append("| SYSTEM_PROMPT.md | Project-independent AI instructions |")
    
    return "\n".join(lines)

def install_self_copy(root):
    source=Path(__file__).resolve()
    target_dir=root / GOV_DIR / TOOLS_DIR
    target_dir.mkdir(parents=True, exist_ok=True)
    target=target_dir / "project_governance_bootstrap.py"
    try:
        if source==target.resolve(): return "already in place"
    except: pass
    shutil.copy2(source, target)
    return f"installed to {target}"

def init_project(root):
    existing = locate_ledger(root)
    if existing is not None:
        # Measured 2026-10-25 on a scratch copy of this repository: running `init` where a ledger
        # already lives exits 0 and prints `[SUCCESS]`, while replacing `AGENTS.md`,
        # `SYSTEM_PROMPT.md` and `PROJECT_MAP.md` with the blank template. The first two hold the
        # owner's rules in a real project, and `PROJECT_MAP.md` is what `map` regenerates, so `init`
        # is only ever correct for a repository that does not have a ledger yet.
        raise GovernanceError(
            f"a governance ledger already exists at {existing / GOV_DIR}; `init` creates the first "
            f"ledger of a repository and would replace its owner-written files with the template. "
            f"Use `status`, `new`, `done`, `promote` or `validate` on it, and `map` to refresh the "
            f"generated index."
        )
    g=root / GOV_DIR
    g.mkdir(exist_ok=True)
    copy_msg=install_self_copy(root)
    
    # 1. OWNER_VISION.md
    ov=g / FILES["owner_vision"]
    if not ov.exists():
        ov.write_text("""# Owner Vision (owner-controlled)
This file is controlled by the project owner. The AI may read it but MUST NOT edit, rewrite, reorganize, or append to it.

## Project identity
- Project name: [Specify Name]
- Domain: [Specify Domain]

## Project purpose
[Describe core requirements and target goals for this project]

## Priorities
1. Correctness
2. Performance
3. Simplicity

## Constraints
- Do not introduce unnecessary dependencies or over-engineer.
""", encoding='utf-8')

    for key in ["requirements","problems","proposals","criticisms"]:
        p=g / FILES[key]
        if not p.exists(): save_json_list(root, key, [])

    # 2. PROJECT_MAP.md
    pm=g / FILES["project_map"]
    pm.write_text(generate_project_map(root), encoding='utf-8')

    # 3. WORKING_CONTEXT.md
    wc=g / FILES["working_context"]
    if not wc.exists():
        wc.write_text(f"# Working Context\n\n## Current Goal\nInitialize project development.\n\n## Last Update\n{now()}\n", encoding='utf-8')

    # 4. SYSTEM_PROMPT.md (Instructions for AI) - a template only for a tree that has no rules yet.
    sp=g / FILES["system_prompt"]
    if sp.exists():
        print(f"[KEPT] {sp} already exists; `init` does not overwrite governance instructions.")
    else:
        sp.write_text("""# Generic Project Governance Agent

You are an engineering assistant working in the repository that contains `.project-governance/`.
This system tracks tasks without relying on external assumptions. Never assume past project context, stacks, or features without explicit repository evidence.

## Governance Files
- `OWNER_VISION.md`: Read-only source of owner intent.
- `PROJECT_MAP.md`: Generated repository index.
- `requirements.json`: Approved active work items.
- `problems.json`: Confirmed blockers or bugs.
- `proposals.json`: Optional improvements requiring approval.
- `criticisms.json`: Concerns regarding design, security, or quality.

## Mandatory Workflow
1. Check status: `python3 .project-governance/tools/project_governance_bootstrap.py status`
2. Inspect `OWNER_VISION.md` and active tasks in `requirements.json`.
3. Implement active requirements one by one.
4. When a requirement is completed and verified, mark it done: `python3 .project-governance/tools/project_governance_bootstrap.py done REQ-XXXX`
5. Automatically add user feature requests or bugs to governance using `new requirement` or `new problem`.
6. There is one ledger per repository: the tool walks up from the current directory to find it and
   prints `[LEDGER] using <root>` when that walk changes the target, so commands from a subdirectory
   write the real ledger instead of minting a parallel ID space. `init` is only for a repository with
   no ledger yet - it refuses where one exists and never replaces an existing `AGENTS.md` or
   `SYSTEM_PROMPT.md` with this template.
""", encoding='utf-8')

    # 5. AGENTS.md - owner-authored once written; never replaced by a re-init.
    agents=root / "AGENTS.md"
    if agents.exists():
        print(f"[KEPT] {agents} already exists; `init` does not overwrite the owner's rules.")
    else:
        agents.write_text("""# Project Governance Rules
Before starting work, read `.project-governance/SYSTEM_PROMPT.md`, `OWNER_VISION.md`, and `PROJECT_MAP.md`.
Use the helper script `.project-governance/tools/project_governance_bootstrap.py` to keep tasks and project maps updated.
""", encoding='utf-8')

    print(f"[SUCCESS] Governance initialized at: {g}")
    print(f"   {copy_msg}")
    return 0

def add_item(root, kind, title, desc="", exclude_id=None):
    map_prefix={"requirement":"REQ","problem":"PROB","proposal":"PROP","criticism":"CRIT"}
    prefix=map_prefix.get(kind, "REQ")
    file_key=kind+"s"
    if kind=="criticism": file_key="criticisms"
    elif kind=="requirement": file_key="requirements"
    elif kind=="problem": file_key="problems"
    elif kind=="proposal": file_key="proposals"
    
    items=load_json_list(root, file_key)
    all_items=[]
    for fk in ["requirements","problems","proposals","criticisms"]: all_items.extend(load_json_list(root, fk))
    
    nt=norm(title)
    for it in all_items:
        # A promotion under an unchanged title must not match the item being
        # promoted: `exclude_id` names the one entry that may hold that title.
        if exclude_id is not None and it['id'].upper()==exclude_id.upper(): continue
        if norm(it['title'])==nt:
            print(f"[DUPLICATE ERROR] {it['id']} already exists: {it['title']}")
            return 2
            
    new_id=next_id(items, prefix, ledger_ids(root, prefix))
    item={"id":new_id, "title":title.strip(), "description":desc.strip(), "created_at":now(), "status":"open", "type":kind}
    items.append(item)
    save_json_list(root, file_key, items)
    log(root, f"new_{kind}", {"id":new_id, "title":title})
    print(f"[ADDED] {new_id} [{kind}] - {title}")
    return 0

def done_item(root, item_id):
    item_id=item_id.upper()
    for fk in ["requirements","problems","proposals","criticisms"]:
        items=load_json_list(root, fk)
        found=None; rest=[]
        for it in items:
            if it['id'].upper()==item_id: found=it
            else: rest.append(it)
        if found:
            save_json_list(root, fk, rest)
            try:
                log(root, "done_and_deleted", {"id":found['id'], "from":fk})
            except GovernanceError:
                # The item was removed before its record: put the original list
                # back, so a refused write does not close it without a ledger
                # entry.
                save_json_list(root, fk, items)
                raise
            print(f"[COMPLETED & REMOVED] {found['id']} from {fk}")
            return 0
    print(f"[NOT FOUND] Item: {item_id}")
    return 2

def promote_item(root, old_id, new_title):
    old_id=old_id.upper()
    # Read everything the promotion will touch before the first write: a file
    # that cannot be read must refuse the command, not surface after the source
    # has already been deleted.
    for fk in ["requirements","problems","proposals","criticisms"]:
        load_json_list(root, fk)
    for fk in ["criticisms","problems","proposals"]:
        items=load_json_list(root, fk)
        found=None
        for it in items:
            if it['id'].upper()==old_id: found=it
        if found is None: continue
        before={it['id'] for it in load_json_list(root, "requirements")}
        try:
            rc=add_item(root, "requirement", new_title, found.get('description',''), exclude_id=found['id'])
        except GovernanceError:
            # `add_item` may have written the requirement before the ledger
            # refused it: restore the requirement list so nothing is lost.
            save_json_list(root, "requirements",
                           [it for it in load_json_list(root, "requirements") if it['id'] in before])
            raise
        if rc != 0: return rc
        save_json_list(root, fk, [it for it in items if it is not found])
        log(root, "promoted", {"from":found['id'], "to":new_title})
        print(f"[PROMOTED] {found['id']} converted to requirement.")
        return 0
    print(f"[NOT FOUND] ID: {old_id}")
    return 2

def status(root):
    print("\n=== Governance Status ===")
    g=root / GOV_DIR
    reqs=load_json_list(root, "requirements")
    probs=load_json_list(root, "problems")
    props=load_json_list(root, "proposals")
    crits=load_json_list(root, "criticisms")
    print(f"Location: {g}/")
    print(f"Counts -> Requirements: {len(reqs)} | Problems: {len(probs)} | Proposals: {len(props)} | Criticisms: {len(crits)}")
    if reqs:
        print("\n[ACTIVE REQUIREMENTS]:")
        for it in reqs[:5]: print(f"   - {it['id']}: {it['title']}")
    else:
        print("\n[INFO] No active requirements.")
    return 0

def map_cmd(root):
    g=root / GOV_DIR
    content=generate_project_map(root)
    pm=g / FILES["project_map"]
    pm.write_text(content, encoding='utf-8')
    log(root, "map_refreshed", {})
    print("[SUCCESS] PROJECT_MAP.md updated successfully.")
    return 0

def validate(root):
    all_items=[]
    for k in ["requirements","problems","proposals","criticisms"]: all_items.extend(load_json_list(root, k))
    titles={}; ids={}; ok=True
    for it in all_items:
        nt=norm(it['title'])
        if nt in titles:
            print(f"[DUPLICATE TITLE] {it['id']} and {titles[nt]}")
            ok=False
        titles[nt]=it['id']
        if it['id'] in ids:
            print(f"[DUPLICATE ID] {it['id']}")
            ok=False
        ids[it['id']]=1
    # The open lists cannot show an ID that was closed and handed out again,
    # because closing deletes it; only the append-only log can.
    open_ids={i.upper() for i in ids}
    allocations=ledger_allocations(root)
    for item_id, uses in sorted(allocations.items()):
        if len(uses)<2: continue
        ok=False
        print(f"[RECYCLED ID] {item_id} was allocated {len(uses)} times: "
              + "; ".join(f"{t} {title!r}" for t,title in uses))
        if item_id in open_ids:
            print(f"[OPEN VS HISTORY] {item_id} is open now under an ID that earlier items already used")
    for item_id in sorted(open_ids-set(allocations)):
        # An item that never passed through `add_item` has no allocation record,
        # so once it is gone the high-water mark forgets its number entirely.
        ok=False
        print(f"[UNLOGGED ID] {item_id} is open with no allocation record in the ledger")
    if ok: print(f"[VALID] Clean state - {len(all_items)} total items.")
    return 0 if ok else 1

def main():
    import argparse
    p=argparse.ArgumentParser(description="Clean English Governance Tool")
    p.add_argument("--root", default=".", help="Target project root path")
    sub=p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("init", help="Initialize clean governance structure")
    a=sub.add_parser("new", help="Add new item")
    a.add_argument("kind", choices=["requirement","problem","proposal","criticism"])
    a.add_argument("title")
    a.add_argument("--desc", default="")
    d=sub.add_parser("done", help="Complete and remove item")
    d.add_argument("id")
    pr=sub.add_parser("promote", help="Promote item to requirement")
    pr.add_argument("old_id")
    pr.add_argument("new_title")
    sub.add_parser("status", help="Show status summary")
    sub.add_parser("validate", help="Check integrity")
    sub.add_parser("map", help="Refresh repository map")
    args=p.parse_args()
    
    root=Path(args.root).resolve()
    try:
        if args.cmd=="init": return init_project(root)
        # Every other command acts on the ledger that holds the ID history, wherever in this
        # repository it lives - see `resolve_root`.
        ledger_root=resolve_root(root)
        if ledger_root!=root:
            print(f"[LEDGER] using {ledger_root} (resolved by walking up from {root})", file=sys.stderr)
        root=ledger_root
        if args.cmd=="new": return add_item(root, args.kind, args.title, args.desc)
        if args.cmd=="done": return done_item(root, args.id)
        if args.cmd=="promote": return promote_item(root, args.old_id, args.new_title)
        if args.cmd=="status": return status(root)
        if args.cmd=="validate": return validate(root)
        if args.cmd=="map": return map_cmd(root)
    except GovernanceError as exc:
        print(f"[ERROR] {exc}", file=sys.stderr)
        return 2

if __name__=="__main__":
    sys.exit(main())