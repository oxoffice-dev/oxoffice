"""Move a branch's changes to the old crates/oxoffice-cli/limitations.json into crates/oxoffice-cli/limitations/.

The limitations moved to one <id>.json file per entry. A branch that still edits limitations.json gets a
modify/delete conflict when it rebases on main; this replays what the conflicting commit did to the old file,
entry by entry, onto the files main has now, then drops the old file:

    # during `git rebase origin/main`, at the conflict:
    python3 scripts/limitations_move.py REBASE_HEAD~1 REBASE_HEAD
    GIT_EDITOR=true git rebase --continue

    # during `git merge origin/main`, at the conflict: what the branch did since it left main
    python3 scripts/limitations_move.py "$(git merge-base HEAD MERGE_HEAD)" HEAD

An entry is merged field by field, like git merges lines: a field the commit changed takes the commit's value,
a field only main changed keeps main's. An entry the commit added gets its file, an entry it removed loses it.
The files it writes and the removal of limitations.json are staged. When main changed the same field
differently, removed an entry the commit changed, or changed an entry the commit removed, it touches neither
that file nor limitations.json, names the entry and exits 1: resolve it by hand (the commit's version is in
limitations.json), then `git rm crates/oxoffice-cli/limitations.json`. A one-shot tool, removed once no open
branch needs it.
"""

import json
import subprocess
import sys
from pathlib import Path

OLD = "crates/oxoffice-cli/limitations.json"
DIR = Path("crates/oxoffice-cli/limitations")


def git(*args: str, root: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=root, capture_output=True, text=True, encoding="utf-8")


def entries_at(rev: str) -> dict[str, dict] | None:
    shown = git("show", f"{rev}:{OLD}")
    if shown.returncode != 0:
        return None
    return {entry["id"]: entry for entry in json.loads(shown.stdout)}


def merge(base: dict, ours: dict, theirs: dict) -> dict | None:
    """Main's entry (ours) with the commit's changes from base to theirs; None when a field conflicts."""
    merged = dict(ours)
    for key in base.keys() | theirs.keys():
        if base.get(key) == theirs.get(key):
            continue
        if ours.get(key) not in (base.get(key), theirs.get(key)):
            return None
        if key in theirs:
            merged[key] = theirs[key]
        else:
            merged.pop(key, None)
    return merged


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    for rev in sys.argv[1:]:
        if git("rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}").returncode != 0:
            sys.exit(f"error: {rev} is not a commit here (in a merge, use the merge form of the docstring)")
    before, after = entries_at(sys.argv[1]), entries_at(sys.argv[2])
    if after is None:
        # Never what a conflicting commit looks like; reading it as "every entry removed" would empty the directory.
        sys.exit(f"error: {OLD} does not exist at {sys.argv[2]}: give the commit that edits it (see the docstring)")
    before, after = before or {}, after or {}
    root = Path(git("rev-parse", "--show-toplevel").stdout.strip())
    touched, conflicts = [], []
    for id_ in sorted(before.keys() | after.keys()):
        base, theirs = before.get(id_), after.get(id_)
        if base == theirs:
            continue
        path = root / DIR / f"{id_}.json"
        ours = json.loads(path.read_text(encoding="utf-8")) if path.exists() else None
        if theirs is None:
            if ours is None:
                continue
            if ours != base:
                conflicts.append(f"{id_}: the commit removes it, main changed it")
                continue
            path.unlink()
            print(f"removed: {DIR / path.name}")
        elif base is None or ours is None:
            if ours is None and base is not None:
                conflicts.append(f"{id_}: the commit changes it, main removed it")
                continue
            if ours is not None and ours != theirs:
                conflicts.append(f"{id_}: the commit adds it, main has another entry with this id")
                continue
            if ours is None:
                path.write_text(json.dumps(theirs, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
                print(f"added: {DIR / path.name}")
        else:
            merged = merge(base, ours, theirs)
            if merged is None:
                conflicts.append(f"{id_}: the commit and main changed the same field")
                continue
            if merged != ours:
                path.write_text(json.dumps(merged, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
                print(f"changed: {DIR / path.name}")
        touched.append(str(DIR / path.name))
    if touched:
        subprocess.run(["git", "add", "-A", "--", *touched], cwd=root, check=True)
    if conflicts:
        print("\n".join(["not moved, resolve by hand (the commit's version is in " + OLD + "):", *conflicts]), file=sys.stderr)
        sys.exit(1)
    subprocess.run(["git", "rm", "-q", "-f", "--ignore-unmatch", "--", OLD], cwd=root, check=True)


if __name__ == "__main__":
    main()
