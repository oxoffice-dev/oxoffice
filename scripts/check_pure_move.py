"""Check that a change to Rust files only moves code between them.

    python3 scripts/check_pure_move.py [BASE [HEAD]]   # default: the merge base with origin/main, and HEAD

Every `.rs` file the range adds, deletes or changes is read whole at BASE and at HEAD and cut into units:

- each top-level item, with its attributes and the comments before it, as one token sequence;
- each item of an `impl` block, paired with that block's header and attributes, so methods may be regrouped into new
  blocks of the same header but not moved to another type or out of a `cfg`.

BASE and HEAD must hold the same units, in any order and any file. Within a unit, order counts. The only edits a split
may make are set aside:

- restricted visibility (`pub(crate)`, `pub(super)`, `pub(in ...)`), so an item or field may be widened;
- whitespace and trailing commas, so rustfmt may reflow a line;
- `//!` module docs;
- the wiring of a new child module `x` (a file the range adds next to its parent `y.rs`, at `y/x.rs`): `mod x;` and
  `use x::...;` in the parent, at any visibility, and `use super::*;` at the top of the child.

Every other `use` and `mod` declaration is a unit like the others. Exit 1 and print what differs otherwise.
"""

import re
import subprocess
import sys
from collections import Counter
from pathlib import PurePosixPath

IDENT = re.compile(r"[A-Za-z0-9_]+")
RAW_STRING = re.compile(r'b?r(#*)"')
CHAR = re.compile(r"(\\(x..|u\{[0-9a-fA-F]+\}|.)|[^\\'])'")
QUALIFIERS = ("unsafe", "async", "extern", "default")
ENDS_AT_SEMICOLON = ("const", "static", "type", "use", "let")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


def lex(text: str) -> list[tuple[str, str]]:
    """Split Rust source into (kind, text) tokens: comment, literal or code."""
    tokens, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
        elif text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            tokens.append(("comment", text[i:end]))
            i = end
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            tokens.append(("comment", text[i:j]))
            i = j
        elif (m := RAW_STRING.match(text, i)) is not None:
            close = '"' + m.group(1)
            end = text.index(close, m.end()) + len(close)
            tokens.append(("literal", text[i:end]))
            i = end
        elif c == '"' or text.startswith('b"', i):
            j = i + (2 if c == "b" else 1)
            while text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            tokens.append(("literal", text[i : j + 1]))
            i = j + 1
        elif c == "'" or text.startswith("b'", i):
            j = i + (2 if c == "b" else 1)
            if (m := CHAR.match(text, j)) is not None:
                tokens.append(("literal", text[i : m.end()]))
                i = m.end()
            elif (m := IDENT.match(text, j)) is not None:  # a lifetime or a label
                tokens.append(("code", "'" + m.group(0)))
                i = m.end()
            else:
                tokens.append(("code", c))
                i += 1
        elif text.startswith("r#", i) and IDENT.match(text, i + 2):
            m = IDENT.match(text, i + 2)
            tokens.append(("code", text[i : m.end()]))
            i = m.end()
        elif (m := IDENT.match(text, i)) is not None:
            tokens.append(("code", m.group(0)))
            i = m.end()
        else:
            tokens.append(("code", c))
            i += 1
    return tokens


def normalise(tokens: list[tuple[str, str]]) -> list[tuple[str, str]]:
    """Drop restricted visibility, trailing commas and `//!` docs; collapse whitespace inside comments."""
    out, i = [], 0
    while i < len(tokens):
        kind, text = tokens[i]
        if kind == "comment":
            if not text.startswith("//!"):
                out.append((kind, " ".join(text.split())))
        elif (
            text == "pub"
            and i + 2 < len(tokens)
            and tokens[i + 1] == ("code", "(")
            and tokens[i + 2][1] in ("crate", "super", "self", "in")
        ):
            depth, i = 0, i + 1
            while True:
                depth += tokens[i] == ("code", "(")
                depth -= tokens[i] == ("code", ")")
                if depth == 0:
                    break
                i += 1
        elif text == "," and kind == "code" and next_code(tokens, i + 1) in (")", "]", "}"):
            pass
        else:
            out.append((kind, text))
        i += 1
    return out


def next_code(tokens, i):
    while i < len(tokens) and tokens[i][0] != "code":
        i += 1
    return tokens[i][1] if i < len(tokens) else None


def skip_group(tokens, i):
    """Index after the bracket group that opens at i."""
    depth = 0
    while True:
        if tokens[i][0] == "code":
            depth += tokens[i][1] in "([{"
            depth -= tokens[i][1] in ")]}"
            if depth == 0:
                return i + 1
        i += 1


def items(tokens, lo, hi):
    """Split tokens[lo:hi] into items: (start, body_start, kind, end), end exclusive.

    An item runs from the first token after the previous item (comments included) to its closing `;` or `}`;
    body_start is the index of its first code token that is not an attribute or a comment."""
    out, i = [], lo
    while i < hi:
        start, j = i, i
        while j < hi and (tokens[j][0] == "comment" or tokens[j][1] == "#"):
            if tokens[j][1] == "#":
                j += 1
                if tokens[j][1] == "!":
                    j += 1
                j = skip_group(tokens, j)
            else:
                j += 1
        if j >= hi:  # trailing comments
            out.append((start, j, "tail", hi))
            break
        body = j
        while tokens[j][1] == "pub" or tokens[j][1] in QUALIFIERS or tokens[j][0] == "literal":
            j = skip_group(tokens, j + 1) if tokens[j + 1][1] == "(" and tokens[j][1] == "pub" else j + 1
        kind = tokens[j][1]
        if kind == "const" and tokens[j + 1][1] in ("fn", "unsafe", "async"):
            kind = "fn"
        if j + 1 < hi and tokens[j + 1][1] == "!":
            kind = "macro"
        depth, k = 0, j
        while k < hi:
            kind_k, text = tokens[k]
            if kind_k == "code" and text in "([{":
                depth += 1
            elif kind_k == "code" and text in ")]}":
                depth -= 1
                if depth == 0 and text == "}" and kind not in ENDS_AT_SEMICOLON and next_code(tokens, k + 1) != ";":
                    break
            elif kind_k == "code" and text == ";" and depth == 0:
                break
            k += 1
        out.append((start, body, kind, k + 1))
        i = k + 1
    return out


def units(path: str, text: str, wiring: set[str], new_file: bool) -> Counter:
    """The units of one file, minus the wiring of the new children listed in `wiring`."""
    tokens = lex(text)
    result = Counter()
    for start, body, kind, end in items(tokens, 0, len(tokens)):
        if kind == "impl":
            brace = next(k for k in range(body, end) if tokens[k] == ("code", "{"))
            header = tuple(normalise([t for t in tokens[start:brace] if t[0] != "comment"]))
            comments = tuple(normalise([t for t in tokens[start:body] if t[0] == "comment"]))
            if comments:
                result[("impl comments", header, comments)] += 1
            for s, _, _, e in items(tokens, brace + 1, end - 1):
                result[("impl item", header, tuple(normalise(tokens[s:e])))] += 1
            continue
        unit = tuple(normalise(tokens[start:end]))
        code = [t for k, t in unit if k == "code"]
        if kind == "mod" and len(code) >= 3 and code[-1] == ";" and code[-2] in wiring:
            continue
        if kind == "use":
            path_start = code.index("use") + 1
            if code[path_start] in wiring or (new_file and code[path_start:] == ["super", ":", ":", "*", ";"]):
                continue
        result[(kind, unit)] += 1
    return result


def side(revision: str, paths: list[str], wiring: dict[str, set[str]], added: set[str]) -> Counter:
    total = Counter()
    for path in paths:
        try:
            text = git("show", f"{revision}:{path}")
        except subprocess.CalledProcessError:
            continue
        total += units(path, text, wiring.get(path, set()), path in added)
    return total


def describe(unit) -> str:
    tokens = unit[-1]
    text = " ".join(t for _, t in tokens)
    head = f"[{unit[1][-1][1] if unit[0].startswith('impl') and unit[1] else unit[0]}] "
    return head + (text[:160] + " ..." if len(text) > 160 else text)


def main() -> int:
    head = sys.argv[2] if len(sys.argv) > 2 else "HEAD"
    base = sys.argv[1] if len(sys.argv) > 1 else git("merge-base", "origin/main", head).strip()
    changed = git("diff", "--name-only", "--no-renames", base, head).split()
    paths = [p for p in changed if p.endswith(".rs")]
    others = [p for p in changed if not p.endswith(".rs")]
    added = set(git("diff", "--name-only", "--no-renames", "--diff-filter=A", base, head).split())
    wiring: dict[str, set[str]] = {}
    for path in added:
        child = PurePosixPath(path)
        if child.suffix == ".rs":
            wiring.setdefault(str(child.parent) + ".rs", set()).add(child.stem)
    before = side(base, paths, wiring, added)
    after = side(head, paths, wiring, added)
    ok = True
    for label, diff in (("only at BASE", before - after), ("only at HEAD", after - before)):
        for unit, count in list(diff.items())[:15]:
            print(f"{label}: {count} x {describe(unit)}")
            ok = False
    if others:
        print(f"not checked (not Rust): {', '.join(others)}")
    print(f"{len(paths)} Rust files: {'only moves' if ok else 'NOT only moves'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
