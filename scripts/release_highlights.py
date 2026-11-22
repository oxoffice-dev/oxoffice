#!/usr/bin/env python3
"""Keep each release's highlights at the top of its changelog section.

The release workflow takes the highlights as an input (a few Markdown bullets
written for a reader in a hurry). git-cliff writes the rest of the changelog
from the commits and rewrites the whole file at every release, so each
release's highlights are stored in `changelog/highlights/X.Y.Z.md` and put
back, under a `### ✨ Highlights` heading right after `## [X.Y.Z]`, every time:

    python3 scripts/release_highlights.py store changelog/highlights   # HIGHLIGHTS, VERSION -> X.Y.Z.md
    python3 scripts/release_highlights.py apply changelog/highlights CHANGELOG.md
    python3 scripts/release_highlights.py --check                      # the self test the lint job runs

The text comes from the environment (HIGHLIGHTS, VERSION), never from the
command line, so that a workflow input reaches the script without passing
through a shell. A value typed in the Actions form has no line breaks: a
literal `\\n` stands for one.
"""
import os
import re
import sys
import tempfile

HEADING = "### ✨ Highlights"
VERSION_FILE = re.compile(r"^(\d+\.\d+\.\d+)\.md$")


def normalize(text: str) -> str:
    """The highlights as Markdown lines, `\\n` read as a line break when the value has none."""
    if "\n" not in text:
        text = text.replace("\\n", "\n")
    return "\n".join(line.rstrip() for line in text.strip().splitlines())


def insert(document: str, version: str, highlights: str) -> str:
    """The document with the highlights after the `## [version]` heading."""
    body = normalize(highlights)
    if not body:
        raise ValueError(f"the highlights of {version} are empty")
    match = re.search(rf"^## \[v?{re.escape(version)}\].*$", document, re.MULTILINE)
    if match is None:
        raise ValueError(f"no '## [{version}]' heading to put the highlights under")
    if HEADING in document[match.end():].split("\n## ", 1)[0]:
        raise ValueError(f"{version} already has its highlights")
    return document[: match.end()] + f"\n\n{HEADING}\n\n{body}" + document[match.end():]


def stored(directory: str) -> dict:
    """Version -> highlights of every file in the directory."""
    found = {}
    for name in sorted(os.listdir(directory)) if os.path.isdir(directory) else []:
        match = VERSION_FILE.match(name)
        if match:
            with open(os.path.join(directory, name), encoding="utf-8") as handle:
                found[match.group(1)] = handle.read()
    return found


def apply(document: str, highlights: dict) -> str:
    """The document with every stored release's highlights under its own heading.

    A release the document has no section for is left out: the release notes
    hold only the new version, the changelog every one.
    """
    for version, text in highlights.items():
        if re.search(rf"^## \[v?{re.escape(version)}\]", document, re.MULTILINE):
            document = insert(document, version, text)
    return document


def store(directory: str, version: str, highlights: str) -> str:
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"'{version}' is not X.Y.Z")
    body = normalize(highlights)
    if not body:
        raise ValueError(f"the highlights of {version} are empty")
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, f"{version}.md")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(body + "\n")
    return path


def check() -> None:
    changelog = (
        "# Changelog\n\n## [0.4.0] - 2026-12-01\n\n### 🐛 Bug fixes\n\n- fix 4\n\n"
        "## [0.3.0] - 2026-11-22\n\n### 🐛 Bug fixes\n\n- fix 3\n\n## [0.2.0] - 2026-11-21\n\n- old\n"
    )
    with tempfile.TemporaryDirectory() as directory:
        store(directory, "0.3.0", "- three a\\n- three b")
        store(directory, "0.4.0", "- four\n")
        out = apply(changelog, stored(directory))
    four, three, two = out.index("## [0.4.0]"), out.index("## [0.3.0]"), out.index("## [0.2.0]")
    assert four < out.index("- four") < out.index("- fix 4") < three, out
    assert three < out.index("- three a\n- three b") < out.index("- fix 3") < two, out
    assert out.count(HEADING) == 2, out
    notes = "## [0.4.0] - 2026-12-01\n\n### 🐛 Bug fixes\n"
    assert apply(notes, {"0.3.0": "- three", "0.4.0": "- four"}).startswith(
        f"## [0.4.0] - 2026-12-01\n\n{HEADING}\n\n- four\n\n### 🐛"
    )
    refused = (
        lambda: insert(changelog, "0.4.0", " \n "),
        lambda: insert(changelog, "0.5.0", "- x"),
        lambda: insert(out, "0.4.0", "- again"),
        lambda: store(tempfile.gettempdir(), "0.4", "- x"),
    )
    for call in refused:
        try:
            call()
        except ValueError:
            continue
        raise AssertionError("accepted what it must refuse")


def main(args: list) -> int:
    try:
        if args == ["--check"]:
            check()
        elif len(args) == 2 and args[0] == "store":
            print(store(args[1], os.environ.get("VERSION", ""), os.environ.get("HIGHLIGHTS", "")))
        elif len(args) == 3 and args[0] == "apply":
            with open(args[2], encoding="utf-8") as handle:
                document = handle.read()
            document = apply(document, stored(args[1]))
            with open(args[2], "w", encoding="utf-8") as handle:
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
