"""Replace the account behind the comments of a workbook made by hand.

    python3 tests/corpus/generate/neutral_xlsx.py <file.xlsx> [name ...]

Excel and Google Sheets write the signed-in account into the persons part of a
workbook with threaded comments (display name, user id, provider) and its name
into the comments, the threaded comments and the core properties. The file
keeps `Corpus Author` (`Corpus Author 2`, ... when there are several persons)
and `corpus.author@example.com`, in every place the display names of the
persons part, and the extra `name`s given, appear. Nothing else changes.

Standard library only, Python 3.11 or later.
"""

import re
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape, unescape

PERSONS = re.compile(r"xl/persons/[^/]+\.xml")
NAMED = re.compile(r"xl/(comments[^/]*\.xml|threadedComments/[^/]+\.xml)|docProps/core\.xml")
QUOTE = {'"': "&quot;"}


def attribute(name: str, value: str) -> str:
    return f'{name}="{escape(value, QUOTE)}"'


def neutral(path: Path, extra: list[str]) -> list[str]:
    """Rewrite `path` in place; return the names that were replaced."""
    with zipfile.ZipFile(path) as package:
        infos = package.infolist()
        parts = {info.filename: package.read(info) for info in infos}
    people = [
        unescape(name, {"&quot;": '"'})
        for part in parts
        if PERSONS.fullmatch(part)
        for name in re.findall(r'displayName="([^"]*)"', parts[part].decode("utf-8"))
    ]
    names = list(dict.fromkeys(people + extra))
    new = {name: "Corpus Author" if len(names) == 1 else f"Corpus Author {i}" for i, name in enumerate(names, 1)}
    emails = {name: "corpus.author@example.com" if len(names) == 1 else f"corpus.author{i}@example.com"
              for i, name in enumerate(names, 1)}
    for part in parts:
        if PERSONS.fullmatch(part):
            xml = parts[part].decode("utf-8")

            def person(match: re.Match) -> str:
                tag = match.group(0)
                name = unescape(re.search(r'displayName="([^"]*)"', tag).group(1), {"&quot;": '"'})
                tag = re.sub(r'displayName="[^"]*"', attribute("displayName", new[name]), tag)
                tag = re.sub(r'userId="[^"]*"', attribute("userId", emails[name]), tag)
                return re.sub(r'providerId="[^"]*"', attribute("providerId", "None"), tag)

            parts[part] = re.sub(r"<(?:\w+:)?person\b[^>]*>", person, xml).encode("utf-8")
        elif NAMED.fullmatch(part):
            xml = parts[part].decode("utf-8")
            for name in names:
                for written in {escape(name), escape(name, QUOTE)}:  # as text, and as an attribute value
                    xml = xml.replace(written, new[name])
            parts[part] = xml.encode("utf-8")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for info in infos:
            package.writestr(info, parts[info.filename])
    return names


def main() -> None:
    if not sys.argv[1:]:
        sys.exit(__doc__)
    names = neutral(Path(sys.argv[1]), sys.argv[2:])
    print(f"replaced {len(names)} name(s)" if names else "no person found: nothing replaced")


if __name__ == "__main__":
    main()
