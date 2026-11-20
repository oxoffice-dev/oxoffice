---
date: 2026-11-21
status: accepted
scope: tests
---

# Corpus at scale and behaviour tests by area

## Context

Steps 3 and 4 of [the test suite plan](2026-11-20-test-suite-from-scratch.md).
The corpus pilot (#1) holds two source files (a report and a deck) saved by
Word, PowerPoint, LibreOffice and Google: 11 files, 0.6 MB, few features. The
facade crates have no behaviour tests, only the corpus round trip and the
save refusal (#10), each in a binary of its own.

## Decision

### Corpus

- **Families.** One source per kind of document a user has, each mixing the
  features of the plan:

  | docx | carries |
  | --- | --- |
  | report (pilot) | headings, lists, styled and merged tables, picture, comment, sections |
  | contract | multi-level legal numbering, bookmarks and REF fields, tracked changes by two authors, comments with replies, footnotes, PAGE / NUMPAGES footer |
  | thesis | title page, roman then arabic page numbers, TOC and SEQ fields, footnotes and endnotes, equations, captions, odd and even headers, landscape section |
  | letter | letterhead picture, DATE and MERGEFIELD fields, hyperlinks, tab stops |
  | cv | borderless layout table, nested table, hyperlinks, small caps, leader tabs |
  | form | content controls (text, date, list, checkbox), legacy form fields, form protection |
  | minutes | right-to-left Arabic and Hebrew, Chinese and Japanese, mixed-script runs |
  | manual | nested tables, vertical and horizontal merges, repeated header rows, two-column section, chart, internal hyperlinks |
  | paper | display and inline equations (fractions, matrices, sums), two-column body after a one-column abstract, endnotes |
  | newsletter | floating pictures with wrap, text boxes, columns, shapes |
  | long-report | about 150 pages of the report pattern, for edits on long files |

  | pptx | carries |
  | --- | --- |
  | deck (pilot) | layouts, placeholders, bullet levels, notes, table, picture, text box |
  | pitch | bar, line and pie charts, merged table cells, transitions, sections |
  | training | groups, auto shapes, connectors, hyperlinks, hidden slide, slide numbers and footers, comments |
  | multilingual | right-to-left and CJK text |
  | long-deck | 60 slides of the deck pattern |

- **Authoring.** python-docx and python-pptx write the structure; a small
  module of OOXML helpers (`generate/ooxml.py`) adds what they lack (notes,
  fields, bookmarks, revisions, content controls, equations, floating
  drawings, docx charts, transitions, comments of decks). The producers then
  rewrite this XML their own way, which is the point. oxoffice does not author
  the corpus it is tested on.
- **Producers.** Every source goes through LibreOffice (headless, on any
  machine), Google Docs and Slides (an rclone remote), and Word and
  PowerPoint for Mac (AppleScript driven by `produce.py`, run on a Mac; every
  document is closed after its save, the apps quit even on error).
- **Manifest from the files.** `generate/manifest.py` rewrites
  `manifest.toml` from the files on disk: the producer versions recorded by
  `author.py` and `produce.py`, the size, and the features found in each
  file's XML (a producer that drops content controls shows it). A run on the
  Mac only needs a commit. A file that fails the round trip carries
  `known_failure = <issue>`, and the test fails when it passes again.
- **Deterministic sources.** Fixed dates and seeds, zip entries with fixed
  timestamps: rerunning `author.py` leaves git quiet.
- **Budget.** About 16 families times 4 or 5 producers, about 70 files.
  Under 1 MB per file and 20 MB in total, checked by a test.

### Behaviour tests

- **One integration binary per facade crate**: `crates/oxoffice-docx/tests/docx/`
  (`cargo test -p oxoffice-docx --test docx`) and
  `crates/oxoffice-pptx/tests/pptx/`. `main.rs` declares one module per
  area, `support.rs` holds the shared builders. The corpus round trip and the
  save refusal move in as modules, so each crate links one test binary.
- **docx areas**: text and runs, paragraphs and styles, tables, lists,
  sections and headers, notes, comments, revisions and compare, fields and
  TOC. **pptx areas**: slides and layouts, shapes and text, tables, charts,
  pictures, notes and comments.
- **What a test checks**: a behaviour a user relies on, through the public
  facade, read back through the facade after a save and a reopen. Part XML
  only when the facade has no reader for it. Cases on Word- and Google-saved
  corpus files where the producer matters.
- **Cases are mined from the upstream tests** (rdocx `eb980e94`), rewritten,
  never copied; names describe the behaviour.
- **What breaks** becomes an issue, and its test stays as a strict known
  failure, `#[should_panic(expected = "...")]` with the issue in a comment:
  the fix makes the test fail until the attribute goes, like the strict xfails
  of the scaling tests. A limitation users can hit and work around also gets
  its entry in `limitations.json`.

### Not now

- Behaviour tests for pictures, charts, content controls, conversions (HTML,
  Markdown, PDF) on the docx side, and rendering on the pptx side: next round.
- A corpus subset on pull requests: only if the full round trip passes about a
  minute on CI.

## Plan

1. This spec.
2. `test(corpus)`: generator, OOXML helpers, manifest generator, LibreOffice
   files, the AppleScript routes, a manifest and budget check. Google files
   when the rclone remote is set up.
3. `test(corpus)`: Word and PowerPoint files, from a run on the Mac.
4. `test(docx)`: the docx binary, one module per area.
5. `test(pptx)`: the pptx binary, one module per area.
6. Outcome in [the test suite spec](2026-11-20-test-suite-from-scratch.md).

## Links

- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md)
- #1 corpus pilot, #10 save refusal, #11 scaling
