"""Generate docs/LIMITATIONS.md from crates/oxoffice-cli/limitations.json.

    python3 scripts/limitations_doc.py           # write the page
    python3 scripts/limitations_doc.py --check   # exit 1 when the page is out of date (CI)
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates/oxoffice-cli/limitations.json"
PAGE = ROOT / "docs/LIMITATIONS.md"
ISSUES = "https://github.com/oxoffice-dev/oxoffice/issues"


def render() -> str:
    entries = json.loads(SOURCE.read_text(encoding="utf-8"))
    lines = [
        "# Known limitations",
        "",
        "What this version of oxoffice cannot do yet, and what to do instead. Generated from",
        "[`crates/oxoffice-cli/limitations.json`](../crates/oxoffice-cli/limitations.json) by",
        "`scripts/limitations_doc.py`. The installed build gives the same list:",
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
    page = render()
    if "--check" in sys.argv:
        if not PAGE.exists() or PAGE.read_text(encoding="utf-8") != page:
            sys.exit("docs/LIMITATIONS.md is out of date: run python3 scripts/limitations_doc.py")
        return
    PAGE.write_text(page, encoding="utf-8")


if __name__ == "__main__":
    main()
