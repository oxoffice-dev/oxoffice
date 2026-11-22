"""Find and cut whole items (a function with its attributes and doc comments, a `use`, a `const`...) in Rust source,
with a lexer that knows comments, strings, raw strings and char literals, so that braces inside them do not count."""

import re

# An attribute or a comment that belongs to the item below it; a section rule (`// ---- Name ----`) belongs to the
# section, not to its first item.
ATTR_OR_DOC = re.compile(r"^\s*(#\[|//(?!\s*-{3}))")


def code_mask(text):
    """A copy of `text` where every character inside a comment, a string or a char literal is replaced by a space
    (newlines kept), so that brace and semicolon matching can run on the rest."""
    out, i, n = list(text), 0, len(text)

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif (m := re.match(r'b?r(#*)"', text[i:i + 300])) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            close = '"' + m.group(1)
            j = text.find(close, i + m.end())
            j = n if j < 0 else j + len(close)
            blank(i, j)
            i = j
        elif c == '"' or (c == "b" and text.startswith('b"', i) and (i == 0 or not text[i - 1].isalnum())):
            j = i + (2 if c == "b" else 1)
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i, j + 1)
            i = j + 1
        elif c == "'":
            m = re.match(r"'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'", text[i:i + 12])
            if m:
                blank(i, i + m.end())
                i += m.end()
            else:
                i += 1          # a lifetime
        else:
            i += 1
    return "".join(out)


def item_end(mask, start):
    """The offset just past the item that starts at `start`: its closing brace, or its semicolon when it has no
    body (a `use`, a `const`, a unit struct)."""
    depth, i = 0, start
    while i < len(mask):
        c = mask[i]
        if c in "{([":
            depth += 1
        elif c in "})]":
            depth -= 1
            if depth == 0 and c == "}":
                return i + 1
        elif c == ";" and depth == 0:
            return i + 1
        i += 1
    raise ValueError("unterminated item")


def item_span(text, mask, line_start):
    """(start, end) offsets of the item whose keyword is on the line starting at `line_start`, its attributes and the
    comments right above it (no blank line between) included, and the newline after it."""
    lines_before = text[:line_start].split("\n")[:-1]
    start = line_start
    while lines_before:
        if ATTR_OR_DOC.match(lines_before[-1]):
            start -= len(lines_before.pop()) + 1
            continue
        # the last lines of an attribute written over several lines: back to its `#[`
        k = len(lines_before) - 1
        while k >= 0 and lines_before[k].strip() and not lines_before[k].lstrip().startswith("#[") \
                and not lines_before[k].rstrip().endswith((";", "}", "{")):
            k -= 1
        if k >= 0 and lines_before[k].lstrip().startswith("#[") and lines_before[-1].rstrip().endswith("]"):
            while len(lines_before) > k:
                start -= len(lines_before.pop()) + 1
            continue
        break
    end = item_end(mask, line_start)
    if text[end:end + 1] == "\n":
        end += 1
    return start, end


def cut(text, spans):
    """Remove the spans; where a cut leaves two blank lines or more, keep one."""
    for a, b in sorted(spans, reverse=True):
        before = len(text[:a]) - len(text[:a].rstrip("\n"))
        after = len(text[b:]) - len(text[b:].lstrip("\n"))
        if a and b < len(text):
            keep = "\n" * min(before + after, 2)
        else:
            keep = "\n" if a else ""      # the last item of a file: the file still ends with a newline
        text = text[:a - before] + keep + text[b + after:]
    return text


def test_functions(text):
    """{name: (start, end)} of every #[test] function of a file at the top level."""
    mask = code_mask(text)
    found = {}
    for m in re.finditer(r"^(pub\s+)?fn\s+([A-Za-z0-9_]+)", mask, re.M):
        start, end = item_span(text, mask, m.start())
        if re.search(r"^\s*#\[test\]", text[start:m.start()], re.M):
            found[m.group(2)] = (start, end)
    return found
