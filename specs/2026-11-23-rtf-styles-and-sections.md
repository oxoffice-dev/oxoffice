---
date: 2026-11-23
status: accepted
scope: docx
---

# RTF keeps paragraph styles and section layout

## Context

`Document::to_rtf_bytes` wrote no `{\stylesheet}` and no section layout
(#555). Headings came into Word as Normal paragraphs with direct formatting
only, and a two-column document opened as one column (word/paper.docx: 3
pages instead of 2). Both were reported as dropped, but nothing in the RTF
let Word or LibreOffice rebuild them. The reader skipped `{\stylesheet}` and
dropped `\paperw`, the margins and `\sectd`.

Each run was also written as `{\plain ...}` with its direct formatting only:
`\plain` resets a run to RTF's defaults, so a Normal run lost even its
document-default size (RTF's default is 12 pt) and a heading run its bold,
size and colour.

## Decision

Write and read RTF the way Word does.

- **Styles (PR A).** The writer lists the paragraph styles the body uses in
  `{\stylesheet}`: the default paragraph style first as `\s0`, each style after
  the styles it is based on, with its formatting resolved through its chain,
  `\sbasedonN`, `\snextN` and its name. Each paragraph names its style
  (`\sN`) and restates the style's formatting with its own over it, and each
  run restates its paragraph style's, its character style's and its own run
  formatting, as Word writes them. A theme font is written as the font the
  theme names. Keep with next, keep lines together, page break before and
  widow control (`\keepn`, `\keep`, `\pagebb`, `\widctlpar`) are written and
  read, since the heading styles carry them.
- The reader builds a document style from each paragraph style of the
  stylesheet: `\s0` is the default paragraph style whatever its name, one
  with the name of an existing style (`heading 1`...) replaces that style's
  formatting, another is added with an id made from its name; every style
  exists before any is based on or followed by another. A paragraph takes
  the style of its `\sN` (`\s0` when it names none), and what it and its runs
  restate from that style stays with the style instead of becoming direct
  formatting. As in Word, a value a styled paragraph or run does not state is
  RTF's default (left, no indent, not bold, auto colour...), so a value that
  turns a style's setting off comes back as direct formatting.
- A run's font is the one the most specific level names (defaults, paragraph
  style, character style, run), its theme font first within a level.
- Character, table and section styles of the stylesheet are not read; a
  character style is written as direct formatting on its runs.
- **Sections (PR B).** The writer writes each section's page size,
  orientation, margins, columns (`\cols`, `\colsx`) and break kind after
  `\sectd`, and `\sect` at each section's end; the reader reads them back,
  document-level `\paperw`... included.

Not done: table styles and conditional formatting, character styles as
styles, the stylesheet entries of styles no paragraph uses (Word lists every
style).

## Plan

1. PR A: styles, with tests for the stylesheet the writer emits, a round trip
   that keeps the styles and leaves no direct formatting equal to them, a
   Word-like stylesheet read back, and the headings of word/paper.docx
   through RTF.
2. PR B: sections, with a round trip of a two-column section and of page
   size and margins.
3. Word check (Mac): our RTF of word/paper.docx opened in Word reads its
   headings as Heading 1, its body in two columns, on as many pages as Word
   gives the docx.

## Links

- Issue #555.
