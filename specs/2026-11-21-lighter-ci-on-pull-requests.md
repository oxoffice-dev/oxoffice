---
date: 2026-11-21
status: shipped
scope: ci
---

# Lighter CI on pull requests

## Context

Every push to a pull request ran nine jobs: lint, WebAssembly, build and test
on three platforms, and the scaling tests on three platforms, each building
its own wheel. On the night of 2026-11-21 about twenty pull requests were
open, many stacked on one another and pushed again after each review: 17 runs
waited in the queue for 3 running, and merges waited on the queue more than
on the work. The repository is public, so this costs time, not money.

## Decision

- **A ready pull request into `main`** keeps build and test on Linux, macOS
  and Windows (decided on 2026-11-20, so a platform-only failure shows before
  the merge), and times the QUICK scaling sets **on Linux only**. macOS
  runners are the noisiest for timings, and `main` still times every call on
  the three platforms after the merge.
- **A draft, or a pull request stacked on another branch**, runs lint,
  WebAssembly and the Linux build and tests only. It gets the full run when
  it is marked ready (`ready_for_review` is now a trigger) or rebased onto
  `main`, which is the head that gets merged.
- `main` is unchanged: every job on the three platforms, every scaling call.

Per push: 9 jobs become 7 for a ready pull request into `main`, 3 for a
draft or a stacked one.

## Outcome

- One `ci.yml` change: an `if` and a computed matrix on `build` and
  `scaling`, and the new trigger. No required check names a removed job.
- The [CI and release spec](2026-11-20-ci-and-release.md) still describes the
  first setup; this spec supersedes its rule on pull request platforms.

## Links

- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
- [CI and release from scratch](2026-11-20-ci-and-release.md)
