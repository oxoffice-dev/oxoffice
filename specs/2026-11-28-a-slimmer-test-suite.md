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
3. Not cut, and why (CPU seconds of the 4 biggest tests left, the render walks apart): the equivalent-writings walks
   (137 docx, 29 pptx) are the sample of [the spec of 2026-11-25](2026-11-25-equivalent-xml-writings.md); the block
   operation grid of `block_locations` (70) is the spec of the block control locations, ops times shapes; the corpus
   round trip (34) is kept whole. Cutting one of their inputs is a decision on the spec, not on lines: see the
   recommendations of the pull request.

## Results

| | before | after |
|---|---|---|
| tests (Rust, the CI command) | 1,989 | 1,621 |
| docx / pptx integration tests | 1,166 / 373 | 926 / 245 |
| CPU seconds, each test alone (render walks counted once) | 711 | 573 |
| test code | | -10.6k lines (+54) |
| library line coverage | 69.33 % (279,448 of 403,075) | 69.33 % (279,448 of 403,075) |

The coverage is the union of every test binary of the CI command; per crate it does not move (no line lost; the
`--compare` output is in the pull request). Line coverage counts what runs, not what is asserted: a deleted test may
have checked a result nobody else checks, as the decision of 2026-11-22 said. The time is a small part of the cut:
what costs is the walks above, which a line count keeps.

## Plan

One pull request: the scripts, the cut tests, this spec. The next ones: the Python tests (their own pull request), and
the walks above if the maintainer decides on their inputs.

## Links

- [A smaller test suite](2026-11-22-a-smaller-test-suite.md)
- [One CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md)
