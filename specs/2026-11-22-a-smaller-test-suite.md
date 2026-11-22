---
date: 2026-11-22
status: accepted
scope: tests
---

# A smaller test suite: the docx tests that run 99 % of the lines

## Context

The `Test` job took about 9 minutes per platform on a warm cache (Linux: wheel 82 s, Rust 335 s, Python 102 s),
for a target of about 5. Coverage of every test group, measured on 3e70a900 (each Rust binary, the docx binary per
test, each Python file; builds without debug assertions, `-C instrument-coverage`):

- the docx binary: 1470 tests running 180,963 library lines; 605 of them run 99.01 % of those lines;
- the Python suite adds 3,469 library lines (0.9 %) to the 224,838 the Rust tests run: its value is the bindings
  (18,539 lines); 10 Python files run no library line of their own, and 84 binding lines between them;

## Decision (user, 2026-11-22)

- Keep the docx tests of a greedy 99 % cover (lines gained per second): 605 tests, the `render` module kept whole
  (its tests share one render of the corpus). Also kept whatever their lines, for what a line count cannot see:
  the `corpus` module (every part of every corpus file saved byte for byte, the manifest within its budget, the
  known workflow failures naming real walks) and every known failure (`should_panic`, each the reproduction of an
  open issue: #443, #203, #156, #179). 612 tests in all; the 858 others go, with the helpers only they used.
- Delete the 10 Python files with nothing of their own.
- The nightly keeps its full sets on Linux, macOS and Windows alike (user correction: it costs no waiting, and
  renders can differ per platform where a font falls back to a system one, #457).
- Deleted, not moved to the nightly. From now on a bug or a regression that comes back gets its test then, one at a
  time.

What is lost: at most 1,795 library lines the docx tests no longer run, 1,371 of which no other suite runs (61.8 %
-> 61.4 % of the library), listed by file in `scripts/test_cover/docx-keep.txt.lost.txt` (measured on the 605-test
cover; the 7 tests kept on top only run more); the 84 binding lines of the 10 Python files (the `date=` and
`undated=` keywords of the comment calls, `document_protection`). Line coverage counts what runs, not what is
asserted: a deleted test may have checked a result nobody else checks. The cover builds without debug assertions:
code under `#[cfg(debug_assertions)]` is outside it; the nightly debug builds run it on the three platforms.

Not done: folding the render checks into one pass (they already share one render per run: the per-test measure ran
each test in its own process); a cut of the pptx binary and of the remaining Python files.

## Plan

One pull request: `scripts/test_cover/` (cover.py, prune.py, rust_items.py, the keep list and the lost lines),
the cut docx tests, the Python files, docs. `cover.py` reruns the measure; `prune.py` applies a keep
list and removes the code only the deleted tests used.

## Links

- [One CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md)
