"""Delete the #[test] functions of a test binary that are not in its keep list, then the code that only they used.

    python scripts/test_cover/prune.py crates/oxoffice-docx/tests/docx scripts/test_cover/docx-keep.txt \
        --check "cargo check --profile scaling --locked -p oxoffice-docx --test docx --message-format=json"

The keep list has one `module::test` per line (cover.py writes it). After the cut, the fix command (unused imports)
and the check command run again and again: each whole item the check reports as never used is deleted, until none
is; an unused field or variant is printed for a hand.
"""

import argparse
import json
import re
import shlex
import subprocess
from pathlib import Path

import rust_items


def cut_tests(folder, keep):
    removed = 0
    for path in sorted(folder.glob("*.rs")):
        text = path.read_text()
        tests = rust_items.test_functions(text)
        drop = [span for name, span in tests.items() if f"{path.stem}::{name}" not in keep]
        if drop:
            path.write_text(rust_items.cut(text, drop))
            removed += len(drop)
    return removed


ITEM = re.compile(r"^(function|constant|static|struct|enum|type alias|associated function|method|"
                  r"associated functions|methods|associated items|functions|constants|structs) ")


def unused(check):
    """(file, line) of each whole item the check reports as never used; fields and variants are left alone."""
    out = subprocess.run(shlex.split(check), capture_output=True, text=True).stdout
    found, other = set(), []
    for line in out.splitlines():
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        m = msg.get("message") or {}
        if msg.get("reason") != "compiler-message":
            continue
        if m.get("level") == "error":
            raise SystemExit(m.get("rendered"))
        if (m.get("code") or {}).get("code") != "dead_code":
            continue
        if not ITEM.match(m.get("message", "")):
            other.append(m.get("rendered"))
            continue
        kind = re.match(r"(struct|enum) `([A-Za-z0-9_]+)`", m.get("message", ""))
        for span in m.get("spans", []):
            if span.get("is_primary"):
                found.add((span["file_name"], span["line_start"], kind.group(2) if kind else None))
    for rendered in other:
        print("left for a hand:", rendered)
    return sorted(found)


def remove_unused(found):
    by_file, types = {}, {}
    for f, line, name in found:
        by_file.setdefault(f, []).append(line)
        if name:
            types.setdefault(f, []).append(name)
    for f, lines in by_file.items():
        path = Path(f)
        text = path.read_text()
        mask = rust_items.code_mask(text)
        offsets = [0]
        for l in text.split("\n"):
            offsets.append(offsets[-1] + len(l) + 1)
        spans = []
        for line in lines:
            start = offsets[line - 1]
            start += len(text[start:offsets[line]]) - len(text[start:offsets[line]].lstrip())
            spans.append(rust_items.item_span(text, mask, start))
        for name in types.get(f, []):       # the impl blocks of a type that goes
            for m in re.finditer(rf"^impl(<[^>]*>)?\s+([A-Za-z0-9_:<>, ]+\s+for\s+)?{name}\b", mask, re.M):
                spans.append(rust_items.item_span(text, mask, m.start()))
        spans = sorted(set(spans))
        outer = [s for s in spans if not any(o != s and o[0] <= s[0] and s[1] <= o[1] for o in spans)]
        path.write_text(rust_items.cut(text, outer))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folder", type=Path)
    ap.add_argument("keep", type=Path)
    ap.add_argument("--check", required=True, help="a cargo check with --message-format=json")
    ap.add_argument("--fix", help="a cargo fix that removes the unused imports, run before each check")
    args = ap.parse_args()
    keep = {line.strip() for line in args.keep.read_text().splitlines() if line.strip()}
    print(f"{cut_tests(args.folder, keep)} test functions deleted")
    for round_ in range(1, 20):
        if args.fix:
            subprocess.run(shlex.split(args.fix), capture_output=True)      # unused imports
        found = unused(args.check)
        print(f"round {round_}: {len(found)} unused items")
        if not found:
            break
        remove_unused(found)


if __name__ == "__main__":
    main()
