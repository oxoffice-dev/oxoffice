---
date: 2026-11-21
status: superseded
scope: ci
---

# Lighter CI on pull requests

## Context

Every push to a pull request ran eight jobs: lint, WebAssembly, build and
test on three platforms, and the scaling tests on three platforms, each
building its own wheel. On the night of 2026-11-21 about twenty pull requests were
open, many stacked on one another and pushed again after each review: 17 runs
waited in the queue for 3 running, and merges waited on the queue more than
on the work. The repository is public, so this costs time, not money.

## Decision

- **A ready pull request into `main`** keeps build and test on Linux, macOS
  and Windows (decided on 2026-11-20, so a platform-only failure shows before
  the merge), and times the QUICK scaling sets **on Linux only**. macOS
  runners are the noisiest for timings, and `main` still times every call on
  the three platforms after the merge (now once a night, see
  [nightly scaling](2026-11-21-nightly-scaling-on-macos-and-windows.md)).
- **A draft, or a pull request whose base is another branch**, runs lint,
  WebAssembly and the Linux build and tests only. The full run needs both a
  base of `main` and a ready pull request, on `opened`, `synchronize`,
  `reopened` or `ready_for_review` (the last one now a trigger).
- **A change of base to `main` runs nothing**: GitHub's move of a stacked
  pull request when its base merges, or a change by hand, is an `edited`
  event, left out because the concurrency group would let every title or
  body edit cancel a real run. Such a pull request can be merged cleanly with
  only its old Linux run, so the merge step itself checks the platforms:
  `gh pr checks` must list `Build and test` for macOS and Windows, otherwise
  push a new commit, or close and reopen, first (loop step 6 of
  `docs/DEVELOPMENT.md`). A documentation-only pull request runs no build
  and has nothing to check.
- `workflow_dispatch` runs everything, like `main` (`main` no longer does
  since [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md);
  `workflow_dispatch` still runs everything).
- `main` is unchanged: every job on the three platforms, every scaling call.
  (Superseded in part by [nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md):
  a push to `main` now times the scaling calls on Linux only; and by
  [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md):
  a push to `main` now builds and tests on Linux only.)

Per push: 8 jobs become 6 for a ready pull request into `main`, 3 for a
draft or a stacked one.

## Outcome

- One `ci.yml` change: an `if` and a computed matrix on `build` and
  `scaling`, and the new trigger. No required check names a removed job.
- The [CI and release spec](2026-11-20-ci-and-release.md) still describes the
  first setup; this spec supersedes its rule on pull request platforms.

## Links

- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
- [CI and release from scratch](2026-11-20-ci-and-release.md)
- Superseded by [one CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md).
