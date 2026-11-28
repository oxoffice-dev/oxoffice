---
date: 2026-11-28
status: accepted
scope: tests
---

# A slimmer test suite: the same lines, less to wait for

## Context

The maintainer finds the suite too big: about 7 minutes of Rust tests and 7 of Python on a CI runner
([PR #730](https://github.com/oxoffice-dev/oxoffice/pull/730) shards them). The cut of
[2026-11-22](2026-11-22-a-smaller-test-suite.md) kept the docx tests that run 99 % of the lines; 1,100 tests have
been added since, as the rule "a bug gets its test then" asks. This round cuts again, in this order: tests that cost
time and run nothing another test does, walks that repeat a walk, then the synthetic variants. It keeps the corpus
round trips, the render walks, the known failures, the Excel-pinned tables (numfmt, xlsx) and every test a spec,
the docs or the corpus manifest names.

The Python tests are cut in their own pull request (`crates/oxoffice-py/tests`); this one is the Rust tests.

## Measure

`scripts/test_cover/` (all run from the repository root, each with a `--help` text in its docstring):

- `rust_time.py`: every test binary CI runs, with libtest's per-test times; `rust_cpu.py`: each test alone in its
  own process, wall and CPU seconds. CPU is what a 4-core runner waits for: the equivalent-writings walk runs 137 CPU
  seconds on 10 s of wall here.
- `measure.py`: each test alone under `-C instrument-coverage`; its library lines as a bit set (test code, the
  binding crates and the two test-support crates left out). `report.py` reads it with the CPU times: lines only a test
  runs, and the greedy cover that keeps a share of the lines for the least CPU.
- `grid_sample.py`: the sample of the block operation grid (below).
- `cov_suite.py`: one instrumented run of the whole suite, per crate; `--compare before after` prints the coverage
  of each crate and the lines lost, by file with `--lost`. This is the check of a cut.

Before (a2b76f10, `scaling` profile, 32 cores, other sessions running): 1,989 tests; 650 s of test time summed
(libtest, shared caches counted by each test that waits for them), 1,554 CPU seconds with each test alone in a
process (711 once the render walks, which share one render, count once); 48 s of wall time here.

## Decision

1. **Tests that run nothing another test does** (`report.py --target 1.0`): 588 of 1,915 measured tests run no line
   the others do not. In the docx and pptx binaries: 381 tests, less the ones a spec, the docs or the corpus manifest
   names, the `should_panic` ones (the known failures), the `corpus`, `render` and `equivalent_writings` modules, and the
   nested `embedded::signed` modules, whose helpers the prune cannot see (a feature-gated test uses them). The
   other binaries (CLI, xlsx, numfmt, pdf, core, opc, the unit tests of `src/`) are left whole: the CLI tests pin
   argument forms and JSON keys a line count cannot see, xlsx and numfmt are pinned to Excel, the rest cost nothing.
   `scripts/test_cover/prune.py` deleted the tests and the helpers only they used.
2. **One corpus walk, not two**: `comments::every_corpus_document_lists_its_story_ranges` opened every Word file of
   the corpus to call `story_ranges`; the round trip walk (`corpus::every_document_saves_every_part_unchanged`) opens
   the same files and now calls it too.
3. **Pull requests run a sample of two spec walks, the full checks run all of them** (`OXOFFICE_WORKFLOWS=full`, the
   switch the workflow walks already use; the pull request CI and main's push CI do not set it, the full checks do):
   - the equivalent-writings walk of Word documents leaves out the small thesis. The producer shapes now hold what
     only the thesis walked there: a table of contents field whose result holds a hyperlink with a bookmark mark and a
     page reference, and a hyperlink beside a page field;
   - the block operation grid of `block_locations` runs 107 of its 419 cells (70 -> 13 CPU seconds). `SAMPLE` is
     written by `scripts/test_cover/grid_sample.py --write`: a greedy cover of every shape, every operation, every
     (operation, refusal message) pair of the grid (the messages of `shapes()` are what a line count cannot see) and
     every library line the grid runs. The test fails when `SAMPLE` names a cell the grid does not have, or when a
     shape, an operation or a (operation, message) pair has no sampled cell, so a new shape or refusal asks for a rerun.
4. **What the line cover could not judge, and was put back.** A first version of this cut went by lines alone; the
   review of the pull request found assertions no remaining test made. Restored, as the old tests or as rows of
   one table: the counts of `docProps/app.xml` of a pptx after an add, an add at, a duplicate and an import, and
   after notes are set, cleared and created; the refusal of a date format Word cannot show; the atomicity of a refused
   comment (no `CommentText` style left, bytes unchanged, on three routes) and the "it occurs N times" message; the
   text box insets in the PDF path; the spellings of the source in a refused save (`./`, `sub/../`, hard link on every
   platform, symbolic link on Unix); the mode a replaced file keeps; the full precision of chart values; the removal
   of a chart and of a shared chart; the PDF fields of headers, text boxes, empty and wrapped controls; the first
   page header and footer and the inheritance of headers; the slide metadata of pptx. They were found by searching
   the strings and the numbers a deleted test asserted, and the calls it made, in the tests that remain (the scripts
   of the pull request do not do this; `git show <base>:<file>` has the old text).
   The cover also cannot judge a test whose only unique lines are under `cfg(windows)`, `cfg(macos)` or
   `cfg(debug_assertions)`: it is measured on Linux in the `scaling` profile. Such a test is kept.
5. Not cut: the equivalent-writings walk of decks, the corpus round trip, the render walks (kept whole), the CLI, xlsx
   and numfmt tests (decision of the maintainer), the unit tests of `src/`.

## Results

| | before | after |
|---|---|---|
| tests (Rust, the CI command) | 1,989 | 1,674 |
| docx / pptx integration tests | 1,166 / 373 | 947 / 249 |
| CPU seconds, each test alone (render walks counted once) | 711 | 465 (-35 %) |
| test code against the base of the cut | | -9.2k lines (+1.2k, -10.4k) |
| library line coverage (same base) | 69.42 % (280,768 of 404,464) | 69.42 % (280,760 of 404,464) |

The coverage is the union of every test binary of the CI command, measured on the base and on this branch. 8 lines
are lost: 5 of an error path of the comparison of stories ("owner shell changed", which no test asserts and one
input of the equivalent-writings walk used to reach), 2 of a match arm of the comparison of controls, 1 of the HTML
export. The 28 lines that dropping the small thesis from pull requests lost came back with the producer shapes, but four
(a map closure in `field_complex.rs` and the three above). CPU times vary by 10 to 15 % from one run to the
next on this shared 32-core machine; the restored tests are in the after column. Line coverage counts what runs, not
what is asserted: the restored tests of item 4 are what a review found; a test the search did not name may remain
unasserted.

## Plan

One pull request: the scripts, the cut tests, this spec. The next ones: the Python tests (their own pull request), and
the walks above if the maintainer decides on their inputs.

## Links

- [A smaller test suite](2026-11-22-a-smaller-test-suite.md)
- [One CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md)
