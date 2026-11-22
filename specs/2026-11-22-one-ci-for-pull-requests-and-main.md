---
date: 2026-11-22
status: accepted
scope: ci
---

# One CI for pull requests and main, the full sets at night

## Context

Measured on main run 44957008425 and pull request runs 44954965939,
44947896375 and 44946565585 (2026-11-21):

- A pull request waited 20 to 24 minutes, bounded by `Build and test` on
  macOS and Windows (542 to 1427 s each: Rust tests 300 to 760 s, Python
  tests 175 to 475 s).
- `Scaling (ubuntu-latest)` ran 4 minutes on a pull request, 53 % of it the
  untimed `test_every_timed_call_runs_once` that `Build and test` already
  runs, and 25 minutes on each push to main (every call).
- `Skill examples` ran 18 minutes on each push to main only: 952 s for the 64
  blocks, on the `ci` wheel, against the skills repository's default branch.
- Pull requests, pushes to main, drafts, stacked pull requests, the nightly
  run and a dispatch each ran a different set: hard to tell what had run.

The cost is the debug assertions: a debug build checks every edit against a
copy of the whole document. Locally, at 4 threads like a runner, the docx
Rust tests take 151 s with them and 78 s without, the Python suite 137 s and
38 s.

## Decision

`ci.yml` runs one set of jobs on every pull request, drafts and stacked ones
included, and on every push to main, three platforms on both (about 10
minutes, bounded by the slowest platform):

| Job | What |
| --- | --- |
| Format, clippy, licences, WebAssembly | as before, with the WebAssembly check and the pptx tests without `render` folded in |
| Test (3 OS) | wheel, smoke, Rust integration tests, Python tests, all without debug assertions (`--profile scaling`); render subset on Linux, off elsewhere; workflow walks on one copy |
| Debug checks (Linux) | the Python suite on a wheel with debug assertions (`--profile ci`) |
| Timings (Linux) | the QUICK scaling sets, without the untimed run-once test the test jobs already run |

`nightly.yml` ("Full checks", 02:23 UTC, `workflow_dispatch`,
`workflow_call`) runs what the CI leaves out: the Rust and Python tests
under debug assertions on the three platforms with `OXOFFICE_RENDER=full`
and `OXOFFICE_WORKFLOWS=full`; every scaling call on the three platforms.
A failure opens or comments one issue labelled `ci-nightly`; the next green
run closes it. Its jobs restore main's caches (shared keys) and save none:
the debug test builds start from scratch on macOS and Windows, which nobody
waits for.

Every push, drafts and stacked pull requests included, now runs a macOS job.
The night of 2026-11-21 queued on the 5 macOS jobs the organization could
run at once; the Enterprise limits raised since (20 and more in parallel,
no job waiting all morning) make one rule for every run worth it. A draft
marked ready runs nothing more: it ran the same jobs.

The skill examples leave this repository (user decision, 2026-11-22): they
test what the skills document, the released package installed the ways the
skills say, so they belong to oxoffice-skills, with a CI of its own on its
pushes, its pull requests and a schedule. Until that CI exists the scripts
stay in `tests/skills/`, runnable by hand; `run_examples.py --jobs N` runs
the blocks N at a time for it: a block starts once every earlier block that
names one of its files is done; a missed link almost always fails a block,
so any failure, or an input file changed by a block, runs the skill again in
order in a fresh folder, and only that verdict counts.

Not here: release.yml (its own pull request), shards of the Rust tests,
cargo-nextest, mold (small gains, more moving parts). The profiles keep
their names: `ci` with debug assertions, `scaling` without.

## Plan

1. This pull request: ci.yml, nightly.yml, run_examples.py `--jobs`, docs.
   Checked by its own CI run (times in the body), a nightly dispatch on the
   branch, and mutation proofs: a QUICK call made linear fails the timings;
   a block calling a missing method fails the examples; every link removed
   from the runner still gives the in-order verdict.
2. release.yml: build, a short check of each package, publish.
3. oxoffice-skills: its CI, then `tests/skills/` leaves this repository.

## Links

- Supersedes [lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md),
  [nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md),
  [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md).
- [A faster build job](2026-11-21-faster-build-job.md),
  [a check without debug assertions](2026-11-21-check-without-debug-assertions.md).
