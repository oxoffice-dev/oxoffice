"""Render the known limitations page from crates/oxoffice-cli/limitations/, one <id>.json per entry.

    python3 scripts/limitations_doc.py                          # print the page (lint runs this)
    python3 scripts/limitations_doc.py -o docs/LIMITATIONS.md   # write it, for a local look (git ignores it)
    python3 scripts/limitations_doc.py --ref v0.2.0 -o page.md  # the release asset, linking the entries at its tag

The page is not committed: the release attaches it, built for its tag. `oxoffice limitations` gives each build its
own list.
"""

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates/oxoffice-cli/limitations"
REPOSITORY = "https://github.com/oxoffice-dev/oxoffice"
ISSUES = f"{REPOSITORY}/issues"


def render(ref: str) -> str:
    # The files and the order of the build script: file name order, hidden files skipped.
    files = sorted((p for p in SOURCE.glob("*.json") if not p.name.startswith(".")), key=lambda p: p.name)
    entries = [json.loads(p.read_text(encoding="utf-8")) for p in files]
    lines = [
        "# Known limitations",
        "",
        f"What oxoffice {ref} cannot do yet, and what to do instead. Generated from",
        f"[`crates/oxoffice-cli/limitations/`]({REPOSITORY}/tree/{ref}/crates/oxoffice-cli/limitations) by",
        "`scripts/limitations_doc.py`. The installed build gives its own list:",
        "",
        "```bash",
        "oxoffice limitations [docx|pptx] [--json]",
        "python -c \"import oxoffice; print(oxoffice.limitations('docx'))\"",
        "```",
        "",
    ]
    for fmt in ("docx", "pptx", "xlsx", "all"):
        selected = [e for e in entries if e["format"] == fmt]
        if not selected:
            continue
        lines += [f"## {fmt}", "", "| Area | Limitation | Workaround | Issue |", "| --- | --- | --- | --- |"]
        for e in selected:
            cells = [e["area"], f"{e['title']}: {e['symptom']}", e["workaround"], f"[#{e['issue']}]({ISSUES}/{e['issue']})"]
            lines.append("| " + " | ".join(c.replace("|", "\\|") for c in cells) + " |")
        lines.append("")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ref", default="main", help="the tag or branch the page describes and links to")
    parser.add_argument("-o", "--output", type=Path, help="write the page there instead of printing it")
    args = parser.parse_args()
    page = render(args.ref)
    if args.output:
        args.output.write_text(page, encoding="utf-8")
    else:
        print(page, end="")


if __name__ == "__main__":
    main()
