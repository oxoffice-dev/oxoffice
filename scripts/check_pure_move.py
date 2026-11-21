"""Check that a change to Rust files only moves code between them.

    python3 scripts/check_pure_move.py [BASE [HEAD]]   # default: the merge base with origin/main, and HEAD

Every `.rs` file the range adds, deletes or changes is read whole at BASE and at HEAD and cut into units:

- each top-level item, with its attributes and doc comments, as one token sequence;
- each item of an `impl` block, paired with that block's header and attributes, so methods may be regrouped into new
  blocks of the same header but not moved to another type or out of a `cfg`;
- each loose comment (`//`, `/* */`, `//!`), which may end up before another item.

BASE and HEAD must hold the same units, in any order and any file; within a unit, order counts. Set aside:

- `pub(super)` in added files, so a moved item or field may be widened to it (from a child, the reach a private
  item of the parent had);
- whitespace, and trailing commas outside one-element tuples, so rustfmt may reflow a line;
- `//!` docs and `use super::*;` in added files;
- `mod x;` in a parent, read from its code, for each added file `x.rs` it declares.

The `use x::...` lines of a parent are checked, not set aside: no attribute, no rename, only names `x` defines, each
at the visibility it had before. A public path no longer re-exported fails; a crate path is reported. A moved item
still named in its parent or a new sibling without an explicit import fails, unless every item there that names it
also binds it locally (a parameter, a local, a field) and never calls it, which is only reported.

Added files may not hold a `macro_rules!` or a `super::`/`self::` path, whose meaning depends on where they sit.
When a file that is already a child is split again, an item or field that is already `pub(super)` and crosses the
cut fails the check (it shows as a changed unit); such splits are checked by hand, or with this script extended then.
Without such items, they are checked like a first split.

Not covered, and left to the compiler and the review: name resolution in files the range does not change; code moved
into a file that already existed, or out of a module other than the parent; a moved private method whose name a trait
method shares; loose comments moving inside an item; and children gated by `#[cfg]`, which are refused and checked by
hand. Exit 1 and print what differs otherwise.
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
        elif text.startswith(("&&", "||"), i):  # `a && b` is not `a & &b`
            tokens.append(("code", text[i : i + 2]))
            i += 2
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
            new_file
            and text == "pub"
            and i + 2 < len(tokens)
            and tokens[i + 1] == ("code", "(")
            and tokens[i + 2][1] == "super"
            and tokens[i + 3] == ("code", ")")
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


def attached(tokens):
    """Split an item's tokens into the item with its doc comments, and its loose comments (`//`, `/* */`, `//!`).

    A doc comment documents its item and stays with it; a loose comment may end up before another item."""
    item, loose = [], []
    for kind, text in tokens:
        doc = text.startswith(("///", "/**")) and not text.startswith(("////", "/***"))
        (loose if kind == "comment" and not doc else item).append((kind, text))
    return item, loose


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
        self.path, self.units, self.names, self.kinds, self.wiring, self.errors = path, Counter(), {}, {}, [], []
        tokens = lex(text)
        code = [t for k, t in tokens if k == "code"]
        # names used outside a field or method access (`a.b`), but `..b` is a use of `b`
        self.idents = Counter(
            t for n, t in enumerate(code)
            if IDENT.fullmatch(t) and not (n > 0 and code[n - 1] == "." and (n < 2 or code[n - 2] != "."))
        )
        # names used by an item that does not also bind them (as a local, a parameter or a field): such a use
        # can only reach an item, so a moved item still named that way must be imported
        self.unbound = Counter()
        for start, _, kind, end in items(tokens, 0, len(tokens)):
            if kind == "impl":
                brace = next(k for k in range(start, end) if tokens[k] == ("code", "{"))
                parts = [(s_, e_) for s_, _, _, e_ in items(tokens, brace + 1, end - 1)]
            else:
                parts = [(start, end)]
            for s_, e_ in parts:
                part = [t for k, t in tokens[s_:e_] if k == "code"]
                # a closure parameter follows the `|` that opens the closure, not the one that closes it
                opens = ("(", ",", "=", "move", "{", ";", "=>", "return", "||")
                binds = {part[n + 1] for n in range(len(part) - 1)
                         if IDENT.fullmatch(part[n + 1])
                         and (part[n] in ("let", "mut", "for")
                              or (part[n] == "|" and (n == 0 or part[n - 1] in opens)))}
                binds |= {part[n] for n in range(len(part) - 2)
                          if IDENT.fullmatch(part[n]) and part[n + 1] == ":" and part[n + 2] != ":"}
                used = {t for n, t in enumerate(part)
                        if IDENT.fullmatch(t) and not (n > 0 and part[n - 1] == "." and (n < 2 or part[n - 2] != "."))}
                # a name called like a function reaches an item, whatever the item also binds
                calls = {t for n, t in enumerate(part[:-1])
                         if IDENT.fullmatch(t) and not (n > 0 and part[n - 1] in (".", "fn"))
                         and (part[n + 1] == "(" or part[n + 1 : n + 4] == [":", ":", "<"])}  # `f(` or `f::<T>(`
                self.unbound.update((used - binds) | calls)
        for start, body, kind, end in items(tokens, 0, len(tokens)):
            raw, loose = attached(tokens[start:end])
            for comment in normalise(loose, new_file):
                self.units[("comment", comment)] += 1
            code = [t for k, t in raw if k == "code"]
            if kind == "impl":
                brace = next(k for k in range(body, end) if tokens[k] == ("code", "{"))
                header_tokens, _ = attached(tokens[start:brace])
                header = tuple(normalise([t for t in header_tokens if t[0] != "comment"]))
                docs = tuple(normalise([t for t in header_tokens if t[0] == "comment"], new_file))
                if docs:
                    self.units[("impl docs", header, docs)] += 1
                inner = items(tokens, brace + 1, end - 1)
                if not inner:
                    self.units[("impl empty", header)] += 1
                for s_, _, _, e_ in inner:
                    item_tokens, item_loose = attached(tokens[s_:e_])
                    for comment in normalise(item_loose, new_file):
                        self.units[("comment", comment)] += 1
                    self.units[("impl item", header, tuple(normalise(item_tokens, new_file)))] += 1
                continue
            if new_file and code == ["use", "super", ":", ":", "*", ";"]:
                continue
            if kind == "mod" and code[:1] == ["mod"] and len(code) == 3 and code[1] in children:
                continue
            if new_file and kind == "macro" and code[:1] == ["macro_rules"]:
                self.errors.append(f"{path}: a macro_rules! moved here, where its textual scope differs")
            if new_file and any(code[n] in ("super", "self") and code[n + 1 : n + 3] == [":", ":"]
                                for n in range(len(code) - 2)):
                self.errors.append(f"{path}: moved code with a relative path (super:: or self::): {' '.join(code)[:120]}")
            if kind == "use":
                use_at = code.index("use")
                if code[use_at + 1] in children:
                    if code[0] == "#" or "as" in code or "*" in code:
                        self.errors.append(f"{path}: wiring with an attribute, a rename or a glob: {' '.join(code)}")
                    self.wiring.append((code[: use_at], code[use_at + 1], [t for t in code[use_at + 2 :] if IDENT.fullmatch(t)]))
                    continue
            name = item_name(raw, kind)
            if name is not None:
                self.names[name] = visibility(tokens[body:end])
                self.kinds[name] = kind
            self.units[(kind, tuple(normalise(raw, new_file)))] += 1


def declared_modules(text: str) -> set[str]:
    """The `mod x;` declarations of a file, read from its code tokens (comments and literals cannot fake one)."""
    tokens = lex(text)
    return {
        name
        for start, body, kind, end in items(tokens, 0, len(tokens))
        if kind == "mod"
        for code in [[t for k, t in tokens[body:end] if k == "code"]]
        if len(code) == 3 and code[0] == "mod" and code[2] == ";"
        for name in [code[1]]
    }


def child_path_of(parent: str, child: str) -> str:
    """The file of module `child` declared in `parent`."""
    p = PurePosixPath(parent)
    folder = p.parent if p.name in ("lib.rs", "main.rs", "mod.rs") else p.with_suffix("")
    return str(folder / f"{child}.rs")


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
        if not path.endswith(".rs"):
            continue
        child = PurePosixPath(path)
        child = child.with_name(child.name[:-3] + ".rs") if child.suffix != ".rs" else child
        parent = str(child.parent) + ".rs"
        if read(head, parent) is None:  # a module of a crate root or of a mod.rs
            parent = next((str(child.parent / root) for root in ("lib.rs", "main.rs", "mod.rs")
                           if read(head, str(child.parent / root)) is not None), parent)
        declared = declared_modules(read(head, parent) or "")
        if not IDENT.fullmatch(child.name[:-3]) or child.name[:-3] in RESERVED or child.name[:-3] not in declared:
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
                child_file = after.get(child_path_of(parent, child))
                if child_file is None or name not in child_file.names:
                    errors.append(f"{parent}: re-exports {child}::{name}, which {child}.rs does not define")
                    continue
                was = home_before.get((parent, name))
                want = {"pub": "pub", "pub(crate)": "pub(crate)"}.get(was or "", "")
                if was is not None and vis != want:
                    errors.append(f"{parent}: {name} was `{was or 'private'}` here, re-exported as `{vis or 'private'}`")
                imported[name] = child
        for child in names:
            child_file = after[child_path_of(parent, child)]
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
            child_path = child_path_of(parent, child)
            for name in after[child_path].names:
                if name in imported or (parent, name) not in home_before:
                    continue
                users = [p for p in [parent] + [child_path_of(parent, c) for c in names]
                         if p != child_path and name not in after[p].names and after[p].idents[name]]
                for user in users:
                    message = (f"{child_path}: {name} moved here but is still named in {user} without an explicit "
                               "import: check what it resolves to there")
                    if after[user].unbound[name]:
                        errors.append(message)
                    else:
                        notes.append(message + " (every item there that names it also binds it locally)")

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
    if unit[0] == "comment":
        tokens = [tokens]
    text = " ".join(t for _, t in tokens)
    return f"[{unit[0]}] " + (text[:160] + " ..." if len(text) > 160 else text)


if __name__ == "__main__":
    sys.exit(main())
