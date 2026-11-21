---
date: 2026-11-21
status: shipped
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
  the three platforms after the merge.
- **A draft, or a pull request whose base is another branch**, runs lint,
  WebAssembly and the Linux build and tests only. The full run needs both a
  base of `main` and a ready pull request, on a push, a reopen or
  `ready_for_review` (now a trigger).
- **A stacked pull request moved onto `main`** when its base merges gets no
  run from the move itself (an `edited` event, left out so that title and
  body edits do not run or cancel CI). Its rebase onto `main`, which squash
  merges make necessary anyway, is pushed before the merge, and the merge
  waits for that run on the three platforms.
- `workflow_dispatch` runs everything, like `main`.
- `main` is unchanged: every job on the three platforms, every scaling call.

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
