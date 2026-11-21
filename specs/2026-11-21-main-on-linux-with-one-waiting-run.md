---
date: 2026-11-21
status: shipped
scope: ci
---

# main builds on Linux, with one run waiting

## Context

Every push to `main` built and tested on Linux, macOS and Windows, and each
commit of `main` had a run of its own that nothing cancelled. On the night
of 2026-11-21 about 60 merges each queued a macOS and a Windows build behind
the pull request builds (macOS took at most 5 jobs at a time), and the runs
of `main` queued behind each other: the last green `main` run waited about
3 hours for macOS. The pull request had already built and tested the same
code on the three platforms before it merged, and the nightly run
([nightly scaling](2026-11-21-nightly-scaling-on-macos-and-windows.md))
replays every job there once a day.

The maintainer decided on the morning of 2026-11-21: a push to `main` runs
on Linux only, and `main` keeps one run waiting.

## Decision

- **A push to `main` builds and tests on Linux only**, and times every
  scaling call on Linux, as the nightly spec already does. A ready pull
  request into `main` keeps its three platforms; the nightly run and
  `workflow_dispatch` keep every job on the three platforms.
- **One run of `main` runs, one waits.** Pushes to `main` share the
  concurrency group `ci-main` with `cancel-in-progress: false`: the run in
  progress always finishes, and a newer push replaces the waiting run
  (GitHub cancels a pending run when a newer one joins its group). A burst
  of merges builds the first commit and the last one; the last one carries
  every change between them.
- The nightly run and `workflow_dispatch` keep a group per event and commit,
  so they never wait behind a push run, never replace it, and are never
  replaced by it.

What this gives up, and why it is enough:

- A commit of `main` whose run was replaced has no run of its own. A failure
  shows on the next run of `main`; to find the commit, run
  `workflow_dispatch` on a branch at each candidate (as for a red nightly
  run).
- A failure that only a macOS or Windows build of the merged tree shows
  (two pull requests green apart, broken together) shows in the nightly
  run, at most a day later, and counts as a red `main`.
- The three platforms of the pull request are checked by the merge step
  only (loop step 6 of `docs/DEVELOPMENT.md`): `main` requires no status
  check, so a merge with a red or queued macOS or Windows build, a stacked
  pull request merged with its Linux run only, or a direct push, is first
  built on those platforms by the nightly run. The push run of `main` used
  to catch it on the merged commit itself. A required check needs a job
  that every pull request reports, documentation-only ones included (they
  skip the build); such a summary job is planned with the merge queue,
  postponed.
- A re-run of an older run of `main` keeps its `push` event and joins
  `ci-main`, replacing the waiting run of a newer commit: re-run only the
  newest, and dispatch on a branch for an older commit.
- rust-cache entries of `main` for macOS and Windows, which pull requests
  restore, are refreshed by the nightly run instead of each push.

Not here: the merge queue, postponed until a day is measured with the
nightly run, the larger runner limits and reviews before push.

## Outcome

- One `ci.yml` change: the concurrency key of a push to `main` is `ci-main`,
  and the build matrix of a push is Linux only (any other event not listed
  as Linux-only builds the three platforms, as before).
- `docs/DEVELOPMENT.md` (CI) and `docs/RELEASE.md` (step 1: the
  three-platform run on the released commit is the nightly run or a
  dispatch) say so. Supersedes in part
  [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md),
  [A test suite from scratch](2026-11-20-test-suite-from-scratch.md) and
  [CI and release from scratch](2026-11-20-ci-and-release.md), where `main`
  ran every job on the three platforms.

## Links

- [Nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md)
- [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
