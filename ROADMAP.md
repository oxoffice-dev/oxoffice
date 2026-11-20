# Roadmap

Where oxoffice is heading, in rough order. Each item gets a spec in `specs/`
before work starts, and may become tickets later. Nothing here is a promise
of dates.

## How the library is meant to be used

- **Rust first.** Every capability exists in the Rust API first.
- **The CLI covers everything that makes sense from a shell**: inspect,
  convert, render, find and replace, fill a template, compare, accept or
  reject changes.
- **Python covers the whole Rust API**, so that when the CLI is not enough
  you switch to Python and keep going, without changing tools.
- Rust, CLI and Python go together. None is preferred over the others, and a
  feature is done when it reaches all three where it makes sense.
- Word and PowerPoint stand as equals, and spreadsheets will join them.
- **python-docx and python-pptx are a starting point, not a contract.**
  Their names make the API familiar, but we do not keep parity with them:
  the API differs wherever that makes it safer, simpler or faster.

## 1. Bootstrap

Fork, rename, a new test suite, CI, release, the skills repository. See
[the fork spec](specs/2026-11-20-friendly-fork-of-rdocx.md).

## 1b. What the skills need

The skills become documentation and examples only. What their helper scripts
did moves into the library (atomic save that refuses its input, counted
replacements, comments on a text, every story with its part and style, text
spans, recursive shape walk), and known limitations become readable with
`oxoffice limitations`. See
[the helpers spec](specs/2026-11-20-skill-helpers-in-the-library.md) and
[the limitations spec](specs/2026-11-20-known-limitations-in-the-cli.md).

## 2. Code inspection and refactoring

Read the code with fresh eyes before building on it.

- Ten source files are above 10,000 lines. `oxoffice-docx`'s `document.rs`
  is 55,000 lines even without its tests, `oxoffice-pptx`'s `lib.rs` 34,000,
  `text.rs` in the Word model 31,000. Decide, file by file, whether a split
  along real responsibilities makes them easier to work on, or whether the
  size is harmless. A split is not a goal in itself.
- Look for duplicated logic between Word and PowerPoint that belongs in a
  shared crate, and for test-only instrumentation left in production code
  (counters, debug hooks) now that the tests are gone.

## 3. Performance at the root

Edits on documents saved by Word or Google Docs cost the whole body: about
130 ms for `Run.text` on a 22-page report and 0.8 s on a 66-page one, where
python-docx needs a fraction of a millisecond. We want to fix the shared
cause rather than each call.

- The cause is on the Rust side, in how an edit is checked and published
  (the "owned publication" machinery: staged save comparisons, owner
  inventories, whole-body proofs), not in Python itself.
- Goal: an edit costs its own element, as in python-docx. The full-document
  check moves to save time, or to debug builds and tests.
- The scaling and budget tests of the new suite are the gate.

## 4. Fillable PDF forms

Export Word form controls to an AcroForm PDF: text fields, checkboxes,
lists and dates with their current values, appearance streams, and an empty
signature field ready to be signed in Acrobat Reader
([rdocx#381](https://github.com/tensorbee/rdocx/issues/381)). Signing itself
stays out.

## 5. Bibliography

Finish citations and the bibliography: the remaining source types and
styles, citation fields and their rendered results.

## 6. Writing revisions and comments

Author tracked changes, not only read, accept and reject them: insertions,
deletions, property changes and moves, with caller-controlled author and
date. Complete comments with replies, resolution and modern comment
metadata. Comparison output as real revisions.

## 7. Spreadsheets

`oxoffice.xlsx`, with the same Rust, CLI and Python shape.

- First, check whether an existing crate (umya-spreadsheet, calamine,
  rust_xlsxwriter) can serve as a base, or whether we build on `oxoffice-opc`.
- First cut: read values, types, shared strings, styles, formulas and their
  cached results. Edit values, formulas, styles, rows, columns, sheets,
  widths and heights, frozen panes and filters. Preserve everything else
  (comments, threaded comments, validation, conditional formats, defined
  names). Mark formulas for recalculation on open rather than evaluate them.
- Then: a bounded formula evaluator, rendering to PDF.

## Not planned

From the upstream roadmap we leave out, unless a real need shows up: the
watermark and shadow qualification work, SmartArt and VML authoring, web
extensions and task panes, executable attachments, strict-conformance
qualification, the external benchmark campaign, and on the spreadsheet
side pivots, the Data Model, Power Query and Office Scripts.
