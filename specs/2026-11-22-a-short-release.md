---
date: 2026-11-22
status: accepted
scope: release
---

# A short release: build, check each package, publish

## Context

The 0.2.0 run (44950236376) took about 48 minutes with every job green:
builds 7 to 20 minutes (fat LTO, no cache), then `smoke` waited for every
wheel and took 10 to 25 minutes per platform, 85 % of it the bisection that
measures the stack over the whole 68-file corpus; the release job under a
minute, PyPI and prefix.dev about 3. A `conda-check` job per platform ran
beside the smoke on the same wheel.

The tests and the stack measure guard the code, not the packaging: the full
checks run them every night on the three platforms
([one CI](2026-11-22-one-ci-for-pull-requests-and-main.md)).

## Decision

- The `Version` job also requires a green run of the full checks
  (`nightly.yml`) on the dispatched commit (one API call), and says how to
  get one: no test suite runs in the release.
- One `Check` job per wheel replaces `smoke` and `conda-check`, on the same
  runners: install the wheel, the version it reports, then four corpus files
  (Word, Google Docs, LibreOffice, PowerPoint) opened, edited, saved, reopened
  and rendered on the main thread (`wheel_smoke.py --files`, a few seconds),
  then the conda package built and tested into a local channel.
- No stack measure in the release (user decision, 2026-11-22): the nightly
  run builds a release-profile Linux wheel and runs `wheel_smoke.py
  --measure` on the whole corpus, with the 1 MiB budget.
- Unchanged: the builds, the sdist, the release job (commit, tag, assets,
  atomic push), PyPI, prefix.dev.

Expected: about 25 minutes (slowest build, then a few minutes of checks),
from 48. Not here: thin LTO and a cache for the release builds, which need a
measured comparison first; artifacts built in advance (the version is
compiled in, and the user keeps the version commit after the builds).

## Plan

One pull request: release.yml, `wheel_smoke.py --files`, the nightly stack
job, docs/RELEASE.md. Checked by the next release; the light check and the
stack job run locally before the push, and the nightly job by a dispatch.

## Links

- [Release wheels run on the main thread before publishing](2026-11-21-release-wheel-smoke-test.md)
- [The release pushes main and the tag after every build is green](2026-11-21-release-pushes-after-the-builds.md)
- Measured on release run 44950236376 (0.2.0).
