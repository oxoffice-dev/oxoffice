"""Replace the account behind the comments of a workbook made by hand, and check that nothing of it is left.

    python3 tests/corpus/generate/neutral_xlsx.py <file.xlsx> [name ...]

Excel and Google Sheets write the signed-in account into the persons part of a
workbook with threaded comments (display name, user id), and its name into the
legacy comments (the author of a note), the threaded comments and the core
properties (`dc:creator`, `cp:lastModifiedBy`). The file keeps `Corpus Author`
(`Corpus Author 2`, ... when there are several people) and
`corpus.author@example.com` in every place where a name of the persons part, the
creator or the last editor of the core properties, or one of the extra `name`s
given, appears. The provider of a person (`providerId`: `google-sheets`, `AD`,
`None`...) is not the account and stays. Nothing else changes.

The result is read again and refused, the file left as it was, when it still
holds an account name, an email other than `corpus.author@example.com`, a path
other than `/Users/user/`, or a note author that is not `Corpus Author`,
`Reviewer` (the author of the notes of the `review` family), a `tc=` thread
placeholder or empty. A pattern that should match and does not is an error too.

Standard library only, Python 3.11 or later.
"""

import re
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape, unescape

CORPUS_AUTHOR = "Corpus Author"
CORPUS_EMAIL = "corpus.author@example.com"
HOME = "/Users/user/"
KNOWN_AUTHORS = {"Reviewer"}  # the notes of the review family, written by XlsxWriter

CORE = "docProps/core.xml"
PERSONS = re.compile(r"xl/persons/[^/]+\.xml")
COMMENTS = re.compile(r"xl/comments[^/]*\.xml")
NAMED = re.compile(r"xl/(?:comments[^/]*\.xml|threadedComments/[^/]+\.xml)")
TEXT_PART = re.compile(r".*\.(?:xml|rels|vml)")
CREATOR = re.compile(r"<dc:creator(?:/>|>([^<]*)</dc:creator>)")
LAST_EDITOR = re.compile(r"<cp:lastModifiedBy(?:/>|>([^<]*)</cp:lastModifiedBy>)")
PERSON = re.compile(r"<(?:\w+:)?person\b[^>]*>")
NEUTRAL_NAME = re.compile(rf"{CORPUS_AUTHOR}(?: \d+)?")
NEUTRAL_EMAIL = re.compile(r"corpus\.author\d*@example\.com")
THREAD_AUTHOR = re.compile(r"tc=\{[0-9A-Fa-f-]+\}")
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+")
PATH = re.compile(r"(?<![\w.])(?:/Users/|/home/|[A-Za-z]:\\Users\\)[^\"'<>\s&]*")
QUOTE = {'"': "&quot;"}
UNQUOTE = {"&quot;": '"'}


class NotNeutral(Exception):
    """A workbook that is not neutral, or that could not be made so."""


def read(path: Path) -> tuple[list[zipfile.ZipInfo], dict[str, bytes]]:
    with zipfile.ZipFile(path) as package:
        infos = package.infolist()
        return infos, {info.filename: package.read(info) for info in infos}


def write(path: Path, infos: list[zipfile.ZipInfo], parts: dict[str, bytes]) -> None:
    """The package again: its entries in their order, each with its own metadata."""
    with zipfile.ZipFile(path, "w") as package:
        for info in infos:
            if info.filename in parts:
                package.writestr(info, parts[info.filename])


def replaced(text: str, marker: str, pattern: re.Pattern, replacement, where: str) -> str:
    """`text` with every match of `pattern` replaced. An error when `marker` appears more often than the pattern
    matches: a spelling of the element the pattern does not know."""
    edited, count = pattern.subn(replacement, text)
    if count != text.count(marker):
        raise NotNeutral(f"{where}: `{marker}` appears {text.count(marker)} times, the pattern matches {count}")
    return edited


def real_name(name: str) -> bool:
    return bool(name) and not NEUTRAL_NAME.fullmatch(name) and name not in KNOWN_AUTHORS


def account_names(parts: dict[str, bytes]) -> list[str]:
    """The names of the persons part and of the core properties that are not neutral yet."""
    found = []
    for part, data in parts.items():
        if PERSONS.fullmatch(part):
            found += re.findall(r'displayName="([^"]*)"', data.decode("utf-8"))
        elif part == CORE:
            text = data.decode("utf-8")
            found += [name for pattern in (CREATOR, LAST_EDITOR) for name in pattern.findall(text)]
    return [name for name in (unescape(name, UNQUOTE) for name in found) if real_name(name)]


def neutral_parts(parts: dict[str, bytes], extra: list[str]) -> list[str]:
    """Make the account of the parts neutral, in place. Returns the real names that were replaced."""
    names = list(dict.fromkeys(account_names(parts) + [name for name in extra if real_name(name)]))
    alias = {name: CORPUS_AUTHOR if len(names) == 1 else f"{CORPUS_AUTHOR} {number}"
             for number, name in enumerate(names, 1)}

    def person(match: re.Match) -> str:
        tag = match.group(0)
        name = unescape(re.search(r'displayName="([^"]*)"', tag).group(1), UNQUOTE)
        display = alias.get(name, name)
        suffix = display.removeprefix(CORPUS_AUTHOR).strip()
        email = f"corpus.author{suffix}@example.com"
        tag = re.sub(r'displayName="[^"]*"', f'displayName="{escape(display, QUOTE)}"', tag)
        return re.sub(r'userId="(?!corpus\.author\d*@example\.com")[^"]*"', f'userId="{email}"', tag)

    def core(tag: str):
        # The creator and the last editor: an empty one stays empty.
        return lambda match: f"<{tag}>{CORPUS_AUTHOR}</{tag}>" if match.group(1) else match.group(0)

    for part in parts:
        if not (PERSONS.fullmatch(part) or NAMED.fullmatch(part) or part == CORE):
            continue
        text = parts[part].decode("utf-8")
        if PERSONS.fullmatch(part):
            text = replaced(text, "displayName=", PERSON, person, part)
        elif NAMED.fullmatch(part):
            for name in names:
                for written in {escape(name), escape(name, QUOTE)}:  # as text, and as an attribute value
                    text = text.replace(written, alias[name])
        else:
            text = replaced(text, "<dc:creator", CREATOR, core("dc:creator"), part)
            text = replaced(text, "<cp:lastModifiedBy", LAST_EDITOR, core("cp:lastModifiedBy"), part)
        parts[part] = text.encode("utf-8")
    return names


def check(parts: dict[str, bytes], names: list[str]) -> None:
    """Refuse a package that still holds an account name, an email, a path or a note author that is not ours."""
    problems = []
    for part, data in parts.items():
        if not TEXT_PART.fullmatch(part):
            continue
        text = data.decode("utf-8", "replace")
        problems += [f"{part}: the name {name!r}" for name in names if name in text or escape(name) in text]
        problems += [f"{part}: the email {found!r}" for found in EMAIL.findall(text) if not NEUTRAL_EMAIL.fullmatch(found)]
        problems += [f"{part}: the path {found!r}" for found in PATH.findall(text) if found != HOME]
        if COMMENTS.fullmatch(part):
            for author in re.findall(r"<author>([^<]*)</author>", text):
                if author and not (NEUTRAL_NAME.fullmatch(author) or author in KNOWN_AUTHORS or THREAD_AUTHOR.fullmatch(author)):
                    problems.append(f"{part}: the note author {author!r}")
        if PERSONS.fullmatch(part):
            problems += [f"{part}: the person {name!r}" for name in re.findall(r'displayName="([^"]*)"', text)
                         if not NEUTRAL_NAME.fullmatch(name)]
        if part == CORE:
            problems += [f"{part}: {name!r}" for pattern in (CREATOR, LAST_EDITOR) for name in pattern.findall(text)
                         if name and name != CORPUS_AUTHOR]
    if problems:
        raise NotNeutral("the workbook is not neutral:\n  " + "\n  ".join(dict.fromkeys(problems)))


def neutral(path: Path, extra: list[str]) -> list[str]:
    """Make the workbook at `path` neutral and check it: the file is written only when the check passes.
    Returns the names that were replaced."""
    infos, parts = read(path)
    before = dict(parts)
    names = neutral_parts(parts, extra)
    check(parts, names)
    if parts != before:
        write(path, infos, parts)
    return names


def main() -> None:
    if not sys.argv[1:]:
        sys.exit(__doc__)
    try:
        names = neutral(Path(sys.argv[1]), sys.argv[2:])
    except NotNeutral as error:
        sys.exit(str(error))
    print(f"replaced {len(names)} name(s)" if names else "no account name found: nothing replaced")


if __name__ == "__main__":
    main()
