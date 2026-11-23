---
date: 2026-11-23
status: shipped
scope: docx
---

# Chicago marks in the exports and in note policies

## Context

Since the fix of #158, the layout draws Word's Chicago marks (`*`, `†`, `‡`, `§`, then doubled, tripled) for notes and
list levels. Two places did not know them (issue #451):

- The HTML, Markdown and EPUB exports wrote a list level in `chicago` format without its mark: HTML and Markdown as
  plain paragraphs, EPUB not as a list. The PDF and page images draw `*.`, `†.` and so on.
- `NoteNumberFormat` (Rust `set_note_policy`, Python `format=`) has no Chicago value. A document can be read and
  rendered with Chicago note marks, but cannot be given them.

## Decision

- **`NoteNumberFormat::Chicago`**, written `w:numFmt w:val="chicago"`; Python `format="chicago"`. The enum becomes
  `#[non_exhaustive]` in the same change, so the next format is not a breaking change. Both changes break an
  exhaustive `match` on the enum, so the commit carries `!`. The CLI has no note-policy command; nothing changes there.
- **HTML**: a Chicago level is an ordered list (`<ol>`). Each item carries the mark the layout draws as a CSS string
  marker, `style="list-style-type:'†. '"`, because CSS names no Chicago counter. A per-item string needs no style
  sheet, so it also works in a fragment. The other formats keep their CSS `type`.
- **EPUB**: the same per-item marker, built from the level's text and the item's number in the EPUB list's own count.
  A legal level shows decimal numbers, as the layout does. An item also keeps the shading the HTML writer gives it.
  Before, any attribute on the item made the EPUB export fail ("list paragraph projection produced no list item"); a
  shaded list item hit it after #519.
- **Markdown**: a list in Markdown is numbered or bulleted, so a Chicago item is a paragraph led by its mark, escaped
  (`\*. One`), as it reads in the PDF.

**Not done**: `@counter-style` (needs a style sheet, so it would not work in a fragment); the marks of other levels
in a Chicago level's text (EPUB writes only the level's own number, as a Chicago list is read alone); in Markdown, a
Chicago level nested in a numbered list is a paragraph after it, not inside the parent item.

## Plan

One pull request, `feat(docx)!: chicago marks in html, markdown and epub lists, and in note policies`, closing #451.
Tests: a five-item Chicago list in HTML, EPUB (with no marker diagnostic) and Markdown; a shaded EPUB list item; a
Chicago note policy that survives a save (Rust); `format="chicago"` (Python).

## Links

- Issue #451; #158 (the layout's Chicago marks); #519 (shaded list items).
