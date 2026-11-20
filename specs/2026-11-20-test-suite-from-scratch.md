---
date: 2026-11-20
status: draft
scope: tests
---

# A test suite from scratch

## Context

The fork starts with no tests (see
[2026-11-20-friendly-fork-of-rdocx.md](2026-11-20-friendly-fork-of-rdocx.md)),
so we can choose what the suite is for. The problems we care most about only
show on documents saved by a real editor (Word and Google Docs add
`w:rsid*`, `w14:paraId`, tab stops and list properties everywhere), and some
only as a cost that grows with the document. Small synthetic documents miss
both.

What we want from a suite, in order:

1. Catch what breaks for a user: wrong output, lost content, a file that Word
   or PowerPoint refuses, an edit that costs the whole document.
2. Stay fast: a pull request gets its answer in about 10 minutes.
3. Read like documentation: a test names a behaviour, never a ticket or a
   sprint.

## Decision

### What we cover

| Layer | Question it answers | Runs on |
| --- | --- | --- |
| Corpus round trip | Does every realistic file open, save unchanged and reopen, with every part preserved? | PR (subset), main (all) |
| Behaviour | Does each feature do what its doc says, through the Rust API? | PR |
| Binding parity | Is every public Rust facade call reachable from Python, with the same result? Is every CLI command wired? | PR |
| Scaling | Does every public mutating call cost about the same at N and 4N? | PR (subset), main (all) |
| Budgets | How many milliseconds does each common edit cost on Word- and Google-saved documents? | main, release |
| Rendering | Are page count, line breaks and a few golden pages stable? | main |
| Validity | Do Word, PowerPoint and LibreOffice open what we write? | LibreOffice on main, Word and PowerPoint on demand on a Mac |

### The corpus

`tests/corpus/` holds documents produced by real editors, each with a line in
a manifest: producer and version, features it carries, licence, size.

- Producers: Word and PowerPoint for Mac (scripted, documents closed and the
  apps quit after each run), Google Docs and Slides (exported as `.docx` and
  `.pptx`), LibreOffice (headless), plus oxoffice and python-docx as authors.
- Content: realistic reports, letters, contracts, CVs, forms, theses, decks,
  each covering a mix of headings and TOC, lists, tables with merged and
  nested cells, images, footnotes, comments, tracked changes, sections,
  headers and footers, fields, hyperlinks, bookmarks, content controls,
  equations, charts, right-to-left and CJK text. For decks: masters and
  layouts, placeholders, tables, charts, pictures, groups, notes, comments,
  transitions.
- We generate them ourselves, so they are ours to commit. Public documents
  only when their licence allows redistribution.
- Budget: about 20 MB in total and 1 MB per file, so a clone and a CI run
  stay small.

### How

- Rust behaviour tests: one integration binary per facade crate
  (`crates/oxoffice-docx/tests/`, `crates/oxoffice-pptx/tests/`), one module
  per feature area, shared builders. One binary keeps link time down.
- Corpus tests in Rust, table-driven from the manifest.
- Python tests with pytest under `crates/oxoffice-py/tests/`, on the built
  wheel: parity and a thin layer of behaviour, not a copy of the Rust tests.
- CLI tests: one per command family, through the built binary.
- Scaling: port the N/4N tests from rdocx-skills (`test_docx_scaling.py`,
  `test_pptx_scaling.py`) with their 1.5/2.5 thresholds, in release builds.
- Unit tests inside a module only where a function is hard to reach from the
  facade (a parser corner, a unit conversion).
- Old upstream tests are mined for cases, never copied as they are.

### When

- Pull request: fmt, clippy, Rust tests, Python tests on one Linux build,
  CLI tests, corpus subset, scaling subset. Target about 10 minutes.
- Push to `main`: the same on Linux, macOS and Windows, full corpus, full
  scaling, rendering checks, LibreOffice validity.
- Release: everything, plus budgets reported in the release notes.

## Plan

1. Port the scaling tests and a corpus smoke test on a handful of files.
   That is the gate for the performance work.
2. Corpus pilot: one Word, one PowerPoint, one Google Docs and one Google
   Slides document, to check that each producer route works end to end.
3. Generate the full corpus within the budget.
4. Behaviour tests by area: text and runs, paragraphs and styles, tables,
   lists, sections and headers, notes, comments, revisions and compare,
   fields and TOC, pictures and drawings, charts, content controls and
   forms, conversion to HTML, Markdown and PDF. Then slides and layouts,
   shapes and text, tables, charts, pictures, notes and comments, rendering.
5. Binding parity checks, then CLI tests.

## Open questions

- Do the skills' recipes (oxoffice-skills has no tests of its own) run here
  on `main` against the built wheel and CLI?
- Is a nightly job worth it, or is `main` enough?
