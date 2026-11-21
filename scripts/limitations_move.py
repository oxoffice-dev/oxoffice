"""Move a branch's changes to the old crates/oxoffice-cli/limitations.json into crates/oxoffice-cli/limitations/.

The limitations moved to one <id>.json file per entry. A branch that still edits limitations.json gets a
modify/delete conflict when it rebases on main; this replays what the conflicting commit did to the old file,
entry by entry, as files, then drops the old file:

    # during `git rebase origin/main`, at the conflict:
    python3 scripts/limitations_move.py REBASE_HEAD~1 REBASE_HEAD
    GIT_EDITOR=true git rebase --continue

    # during `git merge origin/main`, at the conflict: what the branch did since it left main
    python3 scripts/limitations_move.py "$(git merge-base HEAD MERGE_HEAD)" HEAD

An entry the commit added or changed is written to its file, an entry it removed loses its file. The files it
touches and the removal of limitations.json are staged. It warns when main no longer has an entry the commit
changed or removed: check those by hand. A one-shot tool, removed once no open branch needs it.
"""

import json
import subprocess
import sys
from pathlib import Path

OLD = "crates/oxoffice-cli/limitations.json"
DIR = Path("crates/oxoffice-cli/limitations")


def entries_at(rev: str) -> dict[str, dict]:
    shown = subprocess.run(["git", "show", f"{rev}:{OLD}"], capture_output=True, text=True, encoding="utf-8")
    if shown.returncode != 0:
        return {}
    return {entry["id"]: entry for entry in json.loads(shown.stdout)}


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    before, after = entries_at(sys.argv[1]), entries_at(sys.argv[2])
    root = Path(subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip())
    touched = []
    for id_ in sorted(before.keys() | after.keys()):
        if before.get(id_) == after.get(id_):
            continue
        path = root / DIR / f"{id_}.json"
        if id_ in before and not path.exists():
            print(f"warning: main no longer has {id_}, which this commit changed or removed: check it", file=sys.stderr)
        if id_ in after:
            path.write_text(json.dumps(after[id_], indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
            print(f"{'changed' if id_ in before else 'added'}: {path.relative_to(root)}")
        elif path.exists():
            path.unlink()
            print(f"removed: {path.relative_to(root)}")
        else:
            continue
        touched.append(str(DIR / f"{id_}.json"))
    if touched:
        subprocess.run(["git", "add", "-A", "--", *touched], cwd=root, check=True)
    subprocess.run(["git", "rm", "-q", "-f", "--ignore-unmatch", "--", OLD], cwd=root, check=True)


if __name__ == "__main__":
    main()
