"""Set the one oxoffice version everywhere it is written.

Usage: python3 scripts/set_version.py 0.2.0

Updates `[workspace.package] version` and the `version` of every internal
crate in `[workspace.dependencies]` (the entries with a `path`). Crates and
the Python package read their version from the workspace, so nothing else
needs to change. Prints the files it changed, nothing when the version is
already set.
"""

import re
import sys
from pathlib import Path

SEMVER = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")


def set_version(text: str, version: str) -> str:
    out = []
    section = None
    for line in text.splitlines(keepends=True):
        header = re.match(r"^\[([^\]]+)\]\s*$", line)
        if header:
            section = header.group(1)
        elif section == "workspace.package" and re.match(r"^version\s*=", line):
            line = f'version = "{version}"\n'
        elif section == "workspace.dependencies" and "path = " in line:
            line = re.sub(r'version = "[^"]*"', f'version = "{version}"', line)
        out.append(line)
    return "".join(out)


def main() -> None:
    if len(sys.argv) != 2 or not SEMVER.match(sys.argv[1]):
        sys.exit("usage: set_version.py X.Y.Z")
    path = Path(__file__).resolve().parent.parent / "Cargo.toml"
    text = path.read_text(encoding="utf-8")
    new = set_version(text, sys.argv[1])
    if new != text:
        path.write_text(new, encoding="utf-8")
        print(path)


if __name__ == "__main__":
    main()
