#!/usr/bin/env python3
"""Put the release highlights at the top of a release's changelog section.

The release workflow takes the highlights as an input (a few Markdown bullets
written for a reader in a hurry) and writes them, under a `### ✨ Highlights`
heading, right after the first `## [X.Y.Z]` heading of the file: CHANGELOG.md
and the release notes. git-cliff writes the rest from the commits.

    HIGHLIGHTS="- one\n- two" python3 scripts/release_highlights.py CHANGELOG.md
    python3 scripts/release_highlights.py --check   # the self test the lint job runs

The text comes from the environment, never from the command line, so that a
workflow input reaches the script without passing through a shell. A value
typed in the Actions form has no line breaks: a literal `\\n` stands for one.
"""
import os
import re
import sys

HEADING = "### ✨ Highlights"
VERSION_HEADING = re.compile(r"^## \[[^\]]+\].*$", re.MULTILINE)


def normalize(text: str) -> str:
    """The highlights as Markdown lines, `\\n` read as a line break when the value has none."""
    if "\n" not in text:
        text = text.replace("\\n", "\n")
    lines = [line.rstrip() for line in text.strip().splitlines()]
    return "\n".join(lines)


def insert(document: str, highlights: str) -> str:
    """The document with the highlights after its first version heading."""
    body = normalize(highlights)
    if not body:
        raise ValueError("the highlights are empty")
    match = VERSION_HEADING.search(document)
    if match is None:
        raise ValueError("no '## [X.Y.Z]' heading to put the highlights under")
    if HEADING in document[match.end():].split("\n## ", 1)[0]:
        raise ValueError("this version already has its highlights")
    section = f"\n\n{HEADING}\n\n{body}"
    return document[: match.end()] + section + document[match.end():]


def check() -> None:
    changelog = "# Changelog\n\n## [0.3.0] - 2026-11-22\n\n### 🐛 Bug fixes\n\n- a fix\n\n## [0.2.0] - 2026-11-21\n"
    out = insert(changelog, "- one\\n- two")
    assert out.index(HEADING) < out.index("### 🐛 Bug fixes") < out.index("## [0.2.0]"), out
    assert "\n- one\n- two\n" in out, out
    notes = "## [0.3.0] - 2026-11-22\n\n### 🐛 Bug fixes\n"
    assert insert(notes, "- real\n- lines\n").startswith(
        "## [0.3.0] - 2026-11-22\n\n### ✨ Highlights\n\n- real\n- lines\n"
    )
    for bad, document in (("", changelog), ("- x", "no heading"), ("- x", out)):
        try:
            insert(document, bad)
        except ValueError:
            continue
        raise AssertionError(f"accepted {bad!r} in {document!r}")


def main() -> int:
    if sys.argv[1:] == ["--check"]:
        check()
        return 0
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    path = sys.argv[1]
    with open(path, encoding="utf-8") as handle:
        document = handle.read()
    try:
        document = insert(document, os.environ.get("HIGHLIGHTS", ""))
    except ValueError as error:
        print(f"::error::{path}: {error}", file=sys.stderr)
        return 1
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(document)
    return 0


if __name__ == "__main__":
    sys.exit(main())
