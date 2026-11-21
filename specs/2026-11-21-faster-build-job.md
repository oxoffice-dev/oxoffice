---
date: 2026-11-21
status: shipped
scope: ci
---

# A faster build job

## Context

Profile of `Build and test (macos-latest)` on 32 pull request runs of
2026-11-21 (04:54 to 08:21 UTC): 15.3 minutes at the median [8.4 to 22.9],
whatever the pull request touches.

- Python tests: 8.6 minutes, one test after the other. About three quarters
  of it is one parametrised test, `test_every_timed_call_runs_once`: about
  224 docx calls made once on the build with debug assertions.
- Rust integration tests: 3.7 minutes, of which 1.4 compiling. `cargo test
  --workspace` builds the workspace crates again: they get their default
  features and the dev-dependencies' features, which the wheel build does
  not, and on macOS maturin sets `MACOSX_DEPLOYMENT_TARGET=11.0`, which
  `cargo test` did not, so crates that track it rebuilt too.
- rust-cache: 72 % of the macOS jobs restored a cache by its restore key and
  saved a new one of about 506 MB into the pull request's own scope, which
  only that pull request's later runs read: about 2.25 GB per new pull
  request over its four jobs. The Actions cache was at 10.4 GB of its 10 and
  evicting `main`'s entries. `cache-workspace-crates` saved no compile: a
  fresh checkout makes the sources newer than the cached workspace
  artifacts.

## Decision

- **Python tests in parallel** in the build job: `pytest -n auto`
  (pytest-xdist, pinned with its hashes in `.github/requirements/ci.txt`).
  Every test that writes uses `tmp_path` or `tmp_path_factory`, which give
  each worker its own folder. The scaling job stays sequential: its timings
  need a quiet runner.
- **Caches saved by runs on `main` only** (`save-if`), in every job, also
  when a test fails after the compile (`cache-on-failure`, on `main` only),
  and no `cache-workspace-crates`. Pull requests restore `main`'s caches.
  Since a push to `main` builds on Linux only, the macOS and Windows caches
  are refreshed by the nightly run; after a runner image change, pull
  requests build those platforms cold until the next night, or until a
  `workflow_dispatch` on `main` refreshes them.
- **One deployment target on macOS**: the build job sets
  `MACOSX_DEPLOYMENT_TARGET=11.0`, the value maturin uses, for the whole job.
  The wheel keeps its `macosx_11_0` tag.

Expected, per pull request run on macOS: Python tests 8.6 to about 3.5
minutes (three workers; locally on Linux, four workers took the suite from
325 to 97 seconds); about 0.3 minutes of cache upload less per job and no
2.25 GB per new pull request; for the deployment target, one crate less to
compile on a warm cache (the workspace crates still compile twice, their
features differ), more on a cold one, where the third-party crates both
builds share compile once.

Nothing is dropped: every test runs where it ran. Not here, and for later:

- the same features for the wheel build and `cargo test`, which would save
  the rest of the second compile but must not change what the wheel ships;
- running `test_every_timed_call_runs_once` only on Linux in pull requests
  (about 2 macOS minutes more once parallel): a coverage trade-off for the
  maintainer.

## Plan

One pull request. Checked locally by the whole Python suite with
`-n auto` on a `ci` build; then on its own run, against the medians above:
the macOS job, its Python tests step, the compile of its Rust tests step,
the wheel's tag, and that no cache is saved.

## Outcome

Shipped in #216. Its own run (44888491973), beside the medians of the
profile this spec started from (32 pull request runs created from 04:54
to 08:21 UTC; the Linux, Windows and upload figures come from that profile
too) and beside #207's run (44887139694), 17 minutes earlier with the old
configuration and the same exact cache keys, on another branch (31 more
Rust tests). One run each, and runner speed varies a lot between runs.

| Build and test | profile median | #207, old | #216, new |
| --- | --- | --- | --- |
| macOS job | 15.3 min | 16.6 min | 9.7 min |
| macOS Python tests | 8.6 min | 8.6 min | 3.0 min |
| macOS Rust tests | 3.7 min | 5.2 min | 4.3 min |
| Linux job | 13.3 min | 10.5 min | 11.1 min |
| Windows job | 14.8 min | 11.5 min | 10.0 min |

- The parallel Python tests are the gain: the same tests passed (333 and 2
  skipped on macOS and Linux, 332 and 3 skipped on Windows), in about a
  third of the time on macOS and a bit over half on Windows. Linux varied
  with its runner (#216's ran `docx/main` in 158 s, #207's in 92 s, on the
  same cache).
- No cache was saved (`save-if: false`). Both runs restored an exact key,
  where the old configuration did not upload either; the 0.33 minutes of
  the profile were runs that restored by the restore key.
- 930 Rust tests passed; the wheel kept its `macosx_11_0_arm64` tag.
- The deployment target did not do what it was for. The Rust tests step
  compiled 85 seconds, as on #207: the 20 workspace crates again, including
  `oxoffice-media`, which has no dependency and no feature, so on macOS
  something other than features and this variable keeps the two builds
  apart, not found yet. It also compiled `ring`, which no warm macOS run had
  compiled before: its build script likely tracks the variable, and the
  cache `main` saved holds a `ring` built without it; `main` saves again only
  when the key changes. Aligning the features of the whole graph,
  dev-dependencies included (they reach every workspace crate through
  `thiserror-impl`), remains the lever on Linux and Windows; for macOS, the
  cause above comes first. The variable goes if a run on a cache `main`
  saved with it shows no gain either.

## Links

- [Nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md)
- [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
