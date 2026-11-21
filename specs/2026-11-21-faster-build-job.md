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
  no other run reads (a pull request is pushed once). The Actions cache was
  at 10.4 GB of its 10 and evicting `main`'s entries. `cache-workspace-crates`
  saved no compile: a fresh checkout makes the sources newer than the cached
  workspace artifacts.

## Decision

- **Python tests in parallel** in the build job: `pytest -n auto`
  (pytest-xdist, pinned with its hashes in `.github/requirements/ci.txt`).
  Every test that writes uses `tmp_path` or `tmp_path_factory`, which give
  each worker its own folder. The scaling job stays sequential: its timings
  need a quiet runner.
- **Caches saved by runs on `main` only** (`save-if`), in every job, and no
  `cache-workspace-crates`. Pull requests restore `main`'s caches. Since a
  push to `main` builds on Linux only, the macOS and Windows caches are
  refreshed by the nightly run; after a runner image change, pull requests
  build those platforms cold until the next night, or until a
  `workflow_dispatch` on `main` refreshes them.
- **One deployment target on macOS**: the build job sets
  `MACOSX_DEPLOYMENT_TARGET=11.0`, the value maturin uses, for the whole job.
  The wheel keeps its `macosx_11_0` tag.

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

## Links

- [Nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md)
- [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
