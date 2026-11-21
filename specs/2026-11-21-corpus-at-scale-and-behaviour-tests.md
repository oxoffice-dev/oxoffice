---
date: 2026-11-21
status: shipped
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

## Outcome

### What shipped

- #17: one integration binary per facade crate (`--test docx`, `--test pptx`).
- #34: 9 docx modules, 322 tests read back after a save and a reopen, on
  synthetic documents and the four corpus reports; 31 strict known failures
  pointing at #26 to #33 and #53, plus the compare limitations #3 and #4.
- #46: 6 pptx modules, 208 tests, 16 strict known failures pointing at #38 to
  #44.
- #59: 16 families (11 docx, 5 pptx) authored and saved by LibreOffice, 36
  files and 2 MB with the pilot's Word, PowerPoint and Google copies. The
  round trip checks every part byte for byte, on parallel threads; 8 files
  are known failures that rewrite `word/document.xml` (#57).
- 13 entries in `limitations.json` for the bugs users can work around.
- By the next morning, the fixes made overnight by other sessions had turned
  22 of the 47 known failures into passing tests (18 docx, 4 pptx): each fix
  removed its `should_panic`, and its limitation entry, when it had one, went
  or was narrowed to the cases left.
- Follow-up, open when this was written: #140 (from #59's last review) adds
  manifest features for the form's signature and locked controls, and reads
  16:9 by its ratio, before the Mac run.
- Not done here: Word, PowerPoint and Google copies of the new families. The
  machine that built them has neither Office nor an rclone remote; the scripts
  are ready and #62 holds the commands.
- #62, done the next morning on a Mac with Office 16.115 and Google Docs and
  Slides: 28 more files (10 docx by Word, 4 pptx by PowerPoint, 10 docx and 4
  pptx by Google), 64 files and 4.6 MB in all. No repair dialog in Word or
  PowerPoint. Every new file round-trips byte for byte with no known failure,
  and the behaviour modules that read the corpus pass on them.
- What the producers change, from the manifest and the files: Google turns
  the form's content controls and legacy fields, and the letter's DATE and
  MERGEFIELD fields, into plain text (the thesis TOC control survives), charts
  into pictures and every endnote into a footnote; it drops the thesis's roman
  page numbers, the contract's resolved comment and the deck sections; in
  decks it blanks comment authors and resets comment dates to the export time.
  Word for Mac keeps every authored feature but the contract's comment
  threads: the replies come back as top-level comments (the sources declare
  Word 2010 compatibility mode, a likely cause: #182).
- Google embeds whole faces it licenses: Cambria Math (Monotype, licensed to
  Google) in the paper and the thesis, 900 KB each. `produce.py` now drops
  every embedded face whose licence is not the SIL Open Font License from
  Google Docs exports (Slides exports embed none), with its relationship and
  font-table entry; the OFL Noto faces stay.

### Learnings

- **The independent reviews of #34, #46 and #59 found tests that passed
  whatever the facade did**: header parts counted from the stories still referenced, a
  replacement whose result was never read, a stale frame pinned as correct, a
  table compared with the parser's own output, a known failure built on counts
  nobody had checked against the file. Fixing them turned up four more bugs
  (#26, #28, #53, and the LibreOffice compare case of #32). A test reads what
  it checks from where it can fail: the saved package or the stored bytes,
  not the model that wrote them.
- **Byte-for-byte is cheap and strong on untouched files**: it found #57,
  which the "every part is still there" check missed. After an edit, modelled
  parts are written again, so the comparison there is canonical (attribute
  order, empty elements, where a namespace is declared).
- **Real producer files find what synthetic ones miss**: a Word saved in
  French breaks every lookup of a built-in style by its id (#27); LibreOffice's
  empty run properties block comparison (#32); Google's table frame ignores
  its grid (#44).
- **A strict known failure needs a message only its own assertion makes**, so
  that a fix flips it and an unrelated panic does not; and a known failure of
  the corpus names the parts it may rewrite, nothing more.
- Debug-build cost, measured locally: the docx binary runs in about 15 s, the
  pptx one in 7 s, the corpus round trip in 12 s on parallel threads (one long
  report alone takes 12 s). On CI (#59's run) the docx binary takes about 57 s
  on Linux and 82 s on Windows, the pptx one 28 s and 39 s, and the corpus
  round trip about 22 s and 30 s: under the minute the decision set for a
  corpus subset, so none yet.
- **The pilot files are frozen**: the behaviour tests pin facts of every
  producer's copy of the report and the deck, so their sources match the
  pilot's part for part (its chart PNG is a committed file: PNG compression
  differs between machines) and `produce.py` saves them again only when named.
- No OOXML validator could be installed here; Word and PowerPoint on the Mac
  stay the judges of the authored XML (#62).

## Links

- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md)
- #1 corpus pilot, #10 save refusal, #11 scaling
- #17, #34, #46, #59, #140; issues #26 to #33, #38 to #44, #53, #57, #62
