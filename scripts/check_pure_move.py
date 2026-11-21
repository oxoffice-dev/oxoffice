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


def normalise(tokens: list[tuple[str, str]], new_file: bool = False) -> list[tuple[str, str]]:
    """Drop restricted visibility, trailing commas (not the one of a 1-tuple) and, in a new file, `//!` docs."""
    out, stack, i = [], [], 0
    while i < len(tokens):
        kind, text = tokens[i]
        if kind == "comment":
            if not (new_file and text.startswith("//!")):
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
        elif kind == "code" and text in "([{":
            stack.append([0, text != "(" or argument_list(out)])
            out.append((kind, text))
        elif kind == "code" and text in ")]}":
            commas, list_like = stack.pop() if stack else (0, True)
            if out and out[-1] == ("code", ",") and (list_like or commas >= 2):
                out.pop()
            out.append((kind, text))
        else:
            if kind == "code" and text == "," and stack:
                stack[-1][0] += 1
            out.append((kind, text))
        i += 1
    return out


KEYWORDS = {"return", "in", "match", "if", "while", "let", "mut", "else", "break", "yield", "as", "move", "ref"}


def argument_list(out) -> bool:
    """Whether a `(` opening now follows a callee or a signature name, where a trailing comma is only style."""
    code = [t for k, t in out if k == "code"]
    if not code:
        return False
    prev = code[-1]
    if prev == ">":
        return len(code) < 2 or code[-2] not in ("-", "=")
    return prev in (")", "]") or (IDENT.fullmatch(prev) is not None and prev not in KEYWORDS)


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


NAMED_ITEM = ("fn", "struct", "enum", "type", "const", "static", "trait", "union", "mod", "macro")
RESERVED = {"crate", "self", "super", "std", "core", "alloc"}


def visibility(tokens) -> str:
    """`pub`, `pub(crate)`, `pub(super)`... or `` for the item whose tokens start at its first code token."""
    code = [t for k, t in tokens if k == "code"]
    i = 0
    while i < len(code) and code[i] == "#":  # attributes
        i = skip_group([("code", t) for t in code], i + 1)
    if i < len(code) and code[i] == "pub":
        if i + 1 < len(code) and code[i + 1] == "(":
            j = code.index(")", i)
            return "pub(" + "".join(code[i + 2 : j]) + ")"
        return "pub"
    return ""


def item_name(tokens, kind):
    """The name an item declares, if it declares one."""
    if kind not in NAMED_ITEM:
        return None
    code = [t for k, t in tokens if k == "code"]
    if kind == "macro":
        return None
    for n, t in enumerate(code):
        if t == kind and n + 1 < len(code) and IDENT.fullmatch(code[n + 1]):
            return code[n + 1]
    return None


class File:
    """One side of one file: its units, its named top-level items and its wiring lines."""

    def __init__(self, path: str, text: str, children: set[str], new_file: bool):
        self.path, self.units, self.names, self.wiring, self.errors = path, Counter(), {}, [], []
        tokens = lex(text)
        self.idents = Counter(
            t for n, (k, t) in enumerate(tokens) if k == "code" and IDENT.fullmatch(t) and (n == 0 or tokens[n - 1][1] != ".")
        )
        for start, body, kind, end in items(tokens, 0, len(tokens)):
            raw = tokens[start:end]
            code = [t for k, t in raw if k == "code"]
            if kind == "impl":
                brace = next(k for k in range(body, end) if tokens[k] == ("code", "{"))
                header = tuple(normalise([t for t in tokens[start:brace] if t[0] != "comment"]))
                comments = tuple(normalise([t for t in tokens[start:body] if t[0] == "comment"], new_file))
                if comments:
                    self.units[("impl comments", header, comments)] += 1
                inner = items(tokens, brace + 1, end - 1)
                if not inner:
                    self.units[("impl empty", header)] += 1
                for s_, _, _, e_ in inner:
                    self.units[("impl item", header, tuple(normalise(tokens[s_:e_], new_file)))] += 1
                continue
            if new_file and code == ["use", "super", ":", ":", "*", ";"]:
                continue
            if kind == "mod" and code[:1] == ["mod"] and len(code) == 3 and code[1] in children:
                continue
            if kind == "use":
                use_at = code.index("use")
                if code[use_at + 1] in children:
                    if code[0] == "#" or "as" in code:
                        self.errors.append(f"{path}: wiring with an attribute or a rename: {' '.join(code)}")
                    self.wiring.append((code[: use_at], code[use_at + 1], [t for t in code[use_at + 2 :] if IDENT.fullmatch(t)]))
                    continue
            name = item_name(raw, kind)
            if name is not None:
                self.names[name] = visibility(tokens[body:end])
            self.units[(kind, tuple(normalise(raw, new_file)))] += 1


def main() -> int:
    head = sys.argv[2] if len(sys.argv) > 2 else "HEAD"
    base = sys.argv[1] if len(sys.argv) > 1 else git("merge-base", "origin/main", head).strip()
    changed = git("diff", "--name-only", "--no-renames", base, head).split()
    paths = [p for p in changed if p.endswith(".rs")]
    others = [p for p in changed if not p.endswith(".rs")]
    added = set(git("diff", "--name-only", "--no-renames", "--diff-filter=A", base, head).split())

    def read(revision, path):
        try:
            return git("show", f"{revision}:{path}")
        except subprocess.CalledProcessError:
            return None

    # a new child is an added y/x.rs whose parent y.rs declares `mod x;` at HEAD
    children: dict[str, set[str]] = {}
    errors = []
    for path in sorted(added):
        child = PurePosixPath(path)
        if child.suffix != ".rs":
            continue
        parent = str(child.parent) + ".rs"
        text = read(head, parent) or ""
        if child.stem in RESERVED or not re.search(rf"^mod {child.stem};$", text, re.M):
            errors.append(f"{path}: added but not declared as `mod {child.stem};` in {parent}")
            continue
        children.setdefault(parent, set()).add(child.stem)

    before = {p: File(p, t, set(), False) for p in paths if (t := read(base, p)) is not None}
    after = {p: File(p, t, children.get(p, set()), p in added) for p in paths if (t := read(head, p)) is not None}
    for f in after.values():
        errors += f.errors

    units_before = sum((f.units for f in before.values()), Counter())
    units_after = sum((f.units for f in after.values()), Counter())
    shown = 0
    for label, diff in (("only at BASE", units_before - units_after), ("only at HEAD", units_after - units_before)):
        for unit, count in diff.items():
            if shown < 30:
                print(f"{label}: {count} x {describe(unit)}")
            shown += 1

    # where every named item lived at BASE, and where it lives at HEAD
    home_before = {(p, n): v for p, f in before.items() for n, v in f.names.items()}
    notes = []
    for parent, names in children.items():
        f = after[parent]
        imported = {}
        for vis_tokens, child, listed in f.wiring:
            vis = "".join(vis_tokens)
            for name in listed:
                child_file = after.get(str(PurePosixPath(parent).with_suffix("")) + f"/{child}.rs")
                if child_file is None or name not in child_file.names:
                    errors.append(f"{parent}: re-exports {child}::{name}, which {child}.rs does not define")
                    continue
                was = home_before.get((parent, name))
                want = {"pub": "pub", "pub(crate)": "pub(crate)"}.get(was or "", "")
                if was is not None and vis != want:
                    errors.append(f"{parent}: {name} was `{was or 'private'}` here, re-exported as `{vis or 'private'}`")
                imported[name] = child
        for child in names:
            child_file = after[str(PurePosixPath(parent).with_suffix("")) + f"/{child}.rs"]
            for name in child_file.names:
                was = home_before.get((parent, name))
                if name in imported or was is None:
                    continue
                if was == "pub":
                    errors.append(f"{parent}: public path {name} is no longer re-exported")
                elif was == "pub(crate)":
                    notes.append(f"{parent}: crate path {name} is no longer re-exported (nothing in the crate names it)")
        # a moved item still named in the parent or a sibling must come through the parent's explicit import
        for child in names:
            child_path = str(PurePosixPath(parent).with_suffix("")) + f"/{child}.rs"
            for name in after[child_path].names:
                if name in imported or (parent, name) not in home_before:
                    continue
                users = [p for p in [parent] + [str(PurePosixPath(parent).with_suffix("")) + f"/{c}.rs" for c in names]
                         if p != child_path and after[p].idents[name]]
                if users:
                    errors.append(f"{child_path}: {name} moved here but is still named in {', '.join(users)} "
                                  "without an explicit import: check what it resolves to there")

    for e in errors:
        print(f"error: {e}")
    for n in notes:
        print(f"note: {n}")
    if others:
        print(f"not checked (not Rust): {', '.join(others)}")
    ok = shown == 0 and not errors
    print(f"{len(paths)} Rust files: {'only moves' if ok else 'NOT only moves'}")
    return 0 if ok else 1


def describe(unit) -> str:
    tokens = unit[-1] if unit[0] != "impl empty" else unit[1]
    text = " ".join(t for _, t in tokens)
    return f"[{unit[0]}] " + (text[:160] + " ..." if len(text) > 160 else text)


if __name__ == "__main__":
    sys.exit(main())
