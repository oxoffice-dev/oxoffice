---
date: 2026-11-21
status: superseded
scope: ci
---

# Nightly scaling on macOS and Windows

## Context

Every push to `main` ran the full scaling set on Linux, macOS and Windows.
On macOS and Windows that job takes 27 to 31 minutes. GitHub runs at most
5 macOS jobs at a time for the organization, and on the night of
2026-11-21 about 60 merges each queued one of these jobs behind the pull
request builds: a CI run waited about 40 minutes in the queue for 13 minutes
of work.

## Decision

- A push to `main` times every call on **Linux** only.
- A nightly run (02:23 UTC, `schedule`) and `workflow_dispatch` time every
  call on the three platforms. A growth that shows on one platform only is
  caught there, at most a day later, and fixed like any red `main`.
- Pull requests are unchanged: the QUICK set on Linux for a ready pull
  request into `main`.

Not here: the merge queue and a single pending run of `main`, which come
with their own spec (the second:
[main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md)).

## Outcome

- One `ci.yml` change: a `schedule` trigger, the scaling matrix chosen by
  event, and a concurrency key per event and commit so the nightly run never
  waits behind or is cancelled by a push run of the same commit. The nightly
  run runs every job on the three platforms, a daily health check of `main`.
- A red nightly run counts as a red `main`, and a release waits for a green
  one (`docs/DEVELOPMENT.md`, `docs/RELEASE.md`).
- Supersedes in part [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md)
  and [A test suite from scratch](2026-11-20-test-suite-from-scratch.md):
  `main` no longer times every scaling call on the three platforms on each
  push, and the nightly job that spec postponed exists now.

## Links

- [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md)
- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
- Superseded by [one CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md).
