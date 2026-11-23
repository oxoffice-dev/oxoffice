#!/usr/bin/env python3
"""Put a release's highlights at the top of its changelog section.

The release workflow takes the highlights as an input (a few Markdown bullets
written for a reader in a hurry). git-cliff prepends the new release's section
to CHANGELOG.md and writes the release notes from the commits; this script puts
the highlights under a `### ✨ Highlights` heading right after `## [X.Y.Z]` in
each, once:

    python3 scripts/release_highlights.py validate          # HIGHLIGHTS, VERSION are usable
    python3 scripts/release_highlights.py insert CHANGELOG.md
    python3 scripts/release_highlights.py --check           # the self test the lint job runs

The text comes from the environment (HIGHLIGHTS, VERSION), never from the
command line, so that a workflow input reaches the script without passing
through a shell. A value typed in the Actions form has no line breaks: a
literal `\\n` stands for one.
"""
import os
import re
import sys

HEADING = "### ✨ Highlights"


def normalize(text: str) -> str:
    """The highlights as Markdown lines, `\\n` read as a line break when the value has none."""
    if "\n" not in text:
        text = text.replace("\\n", "\n")
    return "\n".join(line.rstrip() for line in text.strip().splitlines())


def validate(version: str, highlights: str) -> str:
    """The highlights' Markdown, or an error for a version or highlights the release cannot use."""
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"'{version}' is not X.Y.Z")
    body = normalize(highlights)
    if not body:
        raise ValueError(f"the highlights of {version} are empty")
    return body


def insert(document: str, version: str, highlights: str) -> str:
    """The document with the highlights after the `## [version]` heading."""
    body = validate(version, highlights)
    match = re.search(rf"^## \[v?{re.escape(version)}\].*$", document, re.MULTILINE)
    if match is None:
        raise ValueError(f"no '## [{version}]' heading to put the highlights under")
    if HEADING in document[match.end():].split("\n## ", 1)[0]:
        raise ValueError(f"{version} already has its highlights")
    return document[: match.end()] + f"\n\n{HEADING}\n\n{body}" + document[match.end():]


def check() -> None:
    changelog = (
        "# Changelog\n\n## [0.4.0] - 2026-12-01\n\n### 🐛 Bug fixes\n\n- fix 4\n\n"
        f"## [0.3.0] - 2026-11-22\n\n{HEADING}\n\n- three\n\n### 🐛 Bug fixes\n\n- fix 3\n"
    )
    out = insert(changelog, "0.4.0", "- four a\\n- four b")
    four, three = out.index("## [0.4.0]"), out.index("## [0.3.0]")
    assert four < out.index("- four a\n- four b") < out.index("- fix 4") < three, out
    assert out.count(HEADING) == 2, out
    assert out[three:] == changelog[changelog.index("## [0.3.0]"):], out
    notes = "## [0.4.0] - 2026-12-01\n\n### 🐛 Bug fixes\n"
    assert insert(notes, "0.4.0", "- four\n").startswith(
        f"## [0.4.0] - 2026-12-01\n\n{HEADING}\n\n- four\n\n### 🐛"
    )
    refused = (
        lambda: insert(changelog, "0.4.0", " \n "),
        lambda: insert(changelog, "0.5.0", "- x"),
        lambda: insert(out, "0.4.0", "- again"),
        lambda: validate("0.4", "- x"),
    )
    for call in refused:
        try:
            call()
        except ValueError:
            continue
        raise AssertionError("accepted what it must refuse")


def main(args: list) -> int:
    version = os.environ.get("VERSION", "")
    highlights = os.environ.get("HIGHLIGHTS", "")
    try:
        if args == ["--check"]:
            check()
        elif args == ["validate"]:
            validate(version, highlights)
        elif len(args) == 2 and args[0] == "insert":
            with open(args[1], encoding="utf-8") as handle:
                document = handle.read()
            document = insert(document, version, highlights)
            with open(args[1], "w", encoding="utf-8") as handle:
                handle.write(document)
        else:
            print(__doc__, file=sys.stderr)
            return 2
    except ValueError as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
