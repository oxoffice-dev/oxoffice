"""Check that a change to Rust files only moves code between them.

    python3 scripts/check_pure_move.py [BASE [HEAD]]   # default: origin/main and HEAD

Every `.rs` file the range adds, deletes or changes is read whole at BASE and at HEAD, and both sides must hold the
same code tokens, comments and literals, once the edits a split is allowed to make are set aside:

- `use` and `mod` declarations, and `//!` module docs;
- restricted visibility (`pub(crate)`, `pub(super)`, `pub(in ...)`), so an item may be widened;
- `impl` headers with their braces, so methods may be regrouped into new `impl` blocks of a type, as long as every
  header HEAD adds already exists at BASE.

Whitespace is ignored, so rustfmt may reflow a line. Exit 1 and print what differs otherwise.
"""

import re
import subprocess
import sys
from collections import Counter

IDENT = re.compile(r"[A-Za-z0-9_]+")


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
        elif (m := re.compile(r'b?r(#*)"').match(text, i)) is not None:
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
            m = re.compile(r"(\\(x..|u\{[0-9a-fA-F]+\}|.)|[^\\'])'").match(text, j)
            if m is not None:
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


def drop_attributes(kept: list[str]) -> None:
    """Pop the outer attributes (`#[...]`) that end `kept`: they belonged to a declaration being dropped."""
    while kept and kept[-1] == "]":
        depth, j = 0, len(kept) - 1
        while j >= 0:
            depth += kept[j] == "]"
            depth -= kept[j] == "["
            if depth == 0:
                break
            j -= 1
        if j < 1 or kept[j - 1] != "#":
            return
        del kept[j - 1 :]


def normalise(tokens: list[tuple[str, str]]) -> tuple[Counter, Counter, list[str]]:
    """The code tokens and the comments and literals of a file, minus what a move may change, and its impl headers."""
    code = [t for k, t in tokens if k == "code"]
    others = Counter(
        " ".join(t.split()) for k, t in tokens if k != "code" and not (k == "comment" and t.startswith("//!"))
    )
    kept, headers, closers, depth = [], [], set(), 0
    i = 0
    while i < len(code):
        t = code[i]
        if t == "pub" and i + 1 < len(code) and code[i + 1] == "(":
            close = code.index(")", i)
            if code[i + 2] in ("crate", "super", "self", "in"):
                i = close + 1
                continue
        if t in ("use", "mod") and (t == "use" or code[i + 2] == ";"):
            if kept and kept[-1] == "pub":
                kept.pop()
            if not kept or kept[-1] in (";", "{", "}", "]"):
                if t == "mod":
                    drop_attributes(kept)
                nested, j = 0, i
                while code[j] != ";" or nested:
                    nested += code[j] == "{"
                    nested -= code[j] == "}"
                    j += 1
                i = j + 1
                continue
        statement_start = not kept or kept[-1] in (";", "{", "}", "]")
        if statement_start and t == "impl":
            angle, j = 0, i + 1
            while code[j] != "{" or angle:
                angle += code[j] == "<"
                angle -= code[j] == ">" and code[j - 1] != "-"
                if code[j] == ";":
                    break
                j += 1
            if code[j] == "{":
                headers.append(" ".join(code[i:j]))
                depth += 1
                closers.add(depth)
                i = j + 1
                continue
        if t == "{":
            depth += 1
        elif t == "}":
            if depth in closers:
                closers.discard(depth)
                depth -= 1
                i += 1
                continue
            depth -= 1
        kept.append(t)
        i += 1
    return Counter(kept), others, headers


def side(revision: str, paths: list[str]) -> tuple[Counter, Counter, list[str]]:
    code, others, headers = Counter(), Counter(), []
    for path in paths:
        try:
            text = git("show", f"{revision}:{path}")
        except subprocess.CalledProcessError:
            continue
        c, o, h = normalise(lex(text))
        code, others, headers = code + c, others + o, headers + h
    return code, others, headers


def report(name: str, before: Counter, after: Counter) -> bool:
    removed, added = before - after, after - before
    for label, items in (("only at BASE", removed), ("only at HEAD", added)):
        for item, count in items.most_common(20):
            print(f"{name} {label}: {count} x {item}")
    return not removed and not added


def main() -> int:
    base = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
    head = sys.argv[2] if len(sys.argv) > 2 else "HEAD"
    paths = [p for p in git("diff", "--name-only", "--no-renames", base, head).split() if p.endswith(".rs")]
    if not paths:
        print("no Rust file changed")
        return 0
    before_code, before_others, before_headers = side(base, paths)
    after_code, after_others, after_headers = side(head, paths)
    ok = report("code", before_code, after_code) & report("comment or literal", before_others, after_others)
    for header in sorted(set(after_headers) - set(before_headers)):
        print(f"new impl header at HEAD: {header}")
        ok = False
    print(f"{len(paths)} files: {'only moves' if ok else 'NOT only moves'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
