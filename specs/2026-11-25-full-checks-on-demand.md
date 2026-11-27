---
date: 2026-11-25
status: shipped
scope: ci
---

# Full checks on demand

## Context

The full checks (debug assertions on the three platforms, the whole render
corpus, every producer's copy, every scaling call, the release-wheel stack)
ran on a schedule, `nightly.yml`, cron 02:23 UTC. The cron rarely fired at
that time: the scheduled runs of 2026-11-24 and 2026-11-25 started at 12:37
and 11:27 UTC, about ten hours late, in the middle of the working day, beside
the CI of pull requests. A run takes about 90 minutes (the full test jobs:
1h07 to 1h30).

The schedule did not cover the case that matters most. #650 took
`Run.add_picture` out of `GROWING` (the calls whose cost grows with the
body), but the call still grows. The CI of pull requests did not catch it,
because its QUICK set does not time that call. v0.6.0 was released from a
later commit without a full checks run on it: the release does not require
one, and the last green run was older than #650. The next scheduled run
found the regression after the release had shipped (#653).

The workflow's display name was already "Full checks"; the file, the issue
label (`ci-nightly`) and the docs still said nightly.

## Decision

- No schedule. The full checks run on demand: `workflow_dispatch` on any
  branch or tag, and `workflow_call` from another workflow.
- When to run them: on the commit to release (docs/RELEASE.md, unchanged:
  still a habit, not a gate), and on a pull request whose change the CI of
  pull requests does not cover (a call that leaves or joins `GROWING`, the
  corpus walks, a snapshot, the render).
- The issue report stays, opt-in: a dispatch on `main` with `report=true`
  opens or comments one issue, closed by the next green reporting run.
- Renamed: `nightly.yml` -> `full-checks.yml`, label `ci-nightly` ->
  `ci-full-checks` (renamed on GitHub, so the issues that already carry it
  keep it), issue title "Full checks failing".
- Not done: a gate in the release workflow, a pull request label that
  starts the full checks. Both stay possible through `workflow_call`.

Trade-off: a regression the CI of pull requests misses now waits for the
next manual run, at the latest the next release, and may sit among more
commits than one day's.

## Plan

One pull request: the workflow file renamed, its `schedule` trigger
removed, the report job keyed on `inputs.report` alone; every mention in
`ci.yml`, `release.yml`, the test helpers, `docs/DEVELOPMENT.md` and
`docs/RELEASE.md`. Checked with `actionlint`; a renamed workflow can only
be dispatched once it is on `main`, so the first run is the one after the
merge, on `main` with `report=true`. The dated specs keep their wording;
the CI spec of 2026-11-22 gets a line that links here.

After the merge, in this order: merge #654 (the fix of #653), rename the
label on GitHub (`gh label edit ci-nightly --name ci-full-checks
--description "The full checks failed on main"`), so that #653 and the
closed issues keep it, then dispatch `full-checks.yml` on `main` with
`report=true`. A failing run before the rename would create the new label
and a second issue; a green one would leave #653 open.

## Amendment 2026-11-27: Linux only, sharded, a fixed budget

The run before 0.8.0 took about two and a half hours (the full Rust tests
2 h on macOS, 1 h 35 on Linux, the Python tests up to 1 h after them on
each platform). The maintainer asked for a drastic cut and no more growth.

- The Rust and Python full tests run on Linux only: the render is the same
  bytes on every platform (ci.yml already skips it on macOS and Windows) and
  the workflow walks are platform-free; ci.yml runs every test on the three.
- The Rust tests run as three shards side by side (render and writings,
  compare, the rest), split by libtest name filters; the Python tests run in
  their own job, beside them.
- Scaling runs on Linux and Windows (Windows stands for the other
  platforms); the macOS runners, the scarcest, run nothing here.
- Budget: about an hour, the longest job. A set added takes the place of
  another or runs a sample (docs/DEVELOPMENT.md, CI).

## Links

- [One CI for pull requests and main](2026-11-22-one-ci-for-pull-requests-and-main.md)
- #650, #653, #654
