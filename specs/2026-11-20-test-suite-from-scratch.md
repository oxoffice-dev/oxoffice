---
date: 2026-11-20
status: accepted
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

## Decided with the review

- The corpus is committed in the repository, within its budget. No Git LFS,
  no download step.
- Producers: Word and PowerPoint on a Mac, Google Docs and Slides, and
  LibreOffice, after a pilot of one file per producer.
- The recipes of oxoffice-skills, which has no tests of its own, run on every
  push to `main` against the wheel and the CLI just built. Not on pull
  requests.
- No nightly job. `main` runs the full set. A nightly job comes only if
  `main` gets too long. (It came on 2026-11-21 for scaling on macOS and
  Windows: [nightly scaling](2026-11-21-nightly-scaling-on-macos-and-windows.md).)

## Outcome

### Scaling (plan step 1, #11)

- Ported from rdocx-skills with the same protocol and thresholds:
  `crates/oxoffice-py/tests/test_{docx,pptx}_scaling.py`, 259 docx and 248
  pptx calls, plus the check that every mutating call of the stubs is timed.
  Since #22, a call outside the band it is expected in (under 2.5 times, or
  1.5 to 6 times for a call that still grows) is measured again, a growing
  call included: one load spike on `main` read as a fix and failed the run.
- **Timings need a build without debug assertions.** In the `ci` profile the
  debug checks compare every edit with a copy of the whole document, so every
  call grows: 148 docx and 122 pptx calls went over 2.5 times, and pptx
  Presentation calls cost 0.3 to 0.9 s at 5 slides, 3.5 times that at 20. In a
  release build the same pptx calls cost under 1 ms and are flat. A
  `scaling` cargo profile (the `ci` profile without debug assertions) gives
  the release picture at the `ci` build cost, so the scaling tests run in a
  `scaling` CI job of their own, beside `build`.
- **Measured on that build** (Linux, two serial runs, 2026-11-20): every pptx
  call is flat. 81 docx calls grow with the body, 2.0 to 4.0 times from 25 to
  100 paragraphs, none faster than linear: they are strict xfails tied to #6
  (`GROWING`), and a fix shows as an XPASS. The old accepted list is gone.
  No pptx issue was opened, the growth was the debug checks.
- `OXOFFICE_SCALING=quick` (31 calls, pull requests) and `full` (main), both
  on Linux, macOS and Windows. Local wall times on a fast Linux machine: quick
  13 s, full 3 min 44 s (428 passed, 81 xfailed). CI runners are slower, and
  macOS and Windows were first measured by the CI runs of this pull request.
- **Since 2026-11-21** the docx bodies carry the identifiers Word writes on
  every paragraph, run and row, and the docx test compares N and 16N
  paragraphs, so that a linear cost shows over the floor: 150 docx calls grow.
  See [the edit cost spec](2026-11-21-edit-cost-at-the-root.md).

### Corpus at scale (plan step 3, #59)

- 16 families authored and saved by LibreOffice: 36 files in 2 MB with the
  pilot's Word, PowerPoint and Google copies. The Word, PowerPoint and Google
  copies of the new families followed (#62): 64 files in 4.6 MB.
- The round trip checks every part byte for byte. Details and learnings in
  [2026-11-21-corpus-at-scale-and-behaviour-tests.md](2026-11-21-corpus-at-scale-and-behaviour-tests.md).

### Behaviour tests (plan step 4, #34 and #46)

- docx: text and runs, paragraphs and styles, tables, lists, sections and
  headers, notes, comments, revisions and compare, fields and TOC. pptx:
  slides and layouts, shapes and text, tables, charts, pictures, notes and
  comments. 530 behaviour tests added to the two binaries, 47 strict known
  failures, each tied to its issue; by the next morning the fixes of the night
  had turned 22 of them into passing tests.
- Pictures, charts, content controls and conversions on the docx side, and
  rendering on the pptx side, are the next round.
### Skill examples (from the review)

- `tests/skills/` runs the examples of oxoffice-skills (`references/examples.md`
  of each skill, the recipes renamed) in the `skill-examples` job, on main
  only: it waits for `build`, takes the Linux wheel that `build` made and
  tested (the `ci` profile), and reads the skills' default branch, so a
  change on either side shows on the next push to main.
- Inputs are built at each run and come out the same every time: the
  synthetic report and deck, Pillow's bundled CC0 font and a drawn picture
  (python-docx, python-pptx and Pillow are already in `ci.txt`).
- Three checks: every block runs (64 blocks), what they wrote is what they
  describe (`check_outputs.py`), and every function, keyword, attribute,
  command and flag the skills cite exists in the build (`check_names.py`,
  which fails on the first list of problems).
- Measured locally on 0.1.0 (Linux): 8 min 41 s on a `ci` build, 2 min 56 s on
  a `scaling` build. The `ci` build was kept: it is the wheel the other jobs
  test, and its debug assertions check the examples' edits too. On a runner
  the examples take about 18 minutes; the job has a 40 minute timeout. The
  checks read paragraphs through `docx text --json`: walking
  `Document.paragraphs` on that build took over 6 minutes.
