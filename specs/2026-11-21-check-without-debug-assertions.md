---
date: 2026-11-21
status: shipped
scope: ci
---

# A check without debug assertions on every pull request

## Context

Some code exists only with debug assertions: 98 `cfg(debug_assertions)` in
the workspace, mostly the checks that compare every edit with a copy of the
whole document. On a pull request, clippy, the WebAssembly check (the dev
profile) and the build job (the `ci` profile) all compile with debug
assertions. The only build without them is the scaling job's wheel
(`--profile scaling`), which runs only for ready pull requests into `main`
and compiles only the wheel: no test target, no WebAssembly crate, only the
wheel's features. A draft or a stacked pull request, like the splits of the
refactoring, compiles nothing without debug assertions; a mistake there
first shows on `main` or in the nightly run.

The case that found it: a helper under `#[cfg(debug_assertions)]`, moved and
re-exported without the cfg, breaks every build without debug assertions
while clippy and the tests pass.

## Decision

- The lint job, which runs on every pull request and on `main`, runs `cargo
  check --locked --workspace --all-targets --all-features --profile scaling`
  after clippy: every crate and target on Linux, all features on, without
  debug assertions. `cargo check` generates no code for the crates it
  checks, but under a profile of its own it compiles and runs the build
  scripts and proc macros of the dependencies again (ring's C included).
  It and cargo-deny run even when an earlier step of the job failed, so one
  push shows every failure.
- The lint job's rust-cache gets a key of its own (`scaling-check`): `main`
  saves a cache only when its key misses, so the new key makes it save the
  artifacts of this check once.
- Errors only for now: without debug assertions one function of
  `crates/oxoffice-docx/src/document.rs` is dead code (a warning). Once its
  owners gate it, the step becomes `cargo clippy --profile scaling ... -- -D
  warnings`, which denies warnings in the workspace crates only.

Not here: building or testing without debug assertions on pull requests
other than the scaling job's; code behind a feature turned off (only the
WebAssembly crates build it, in the dev profile) or for Windows only. The
nightly run does all of it on three platforms. A `cargo check --profile
scaling` of the WebAssembly crates could close the feature part later.

## Plan

One pull request. Checked locally with 8 jobs, so slower on a 4-vCPU
runner: the check takes 34 s on a cold target and 19 s when every workspace
crate changed; a helper under
`#[cfg(debug_assertions)]` re-exported without the cfg passes `cargo check`
with debug assertions and fails this one (E0432). The step's time on the
runner goes in the pull request, cold (its own runs: the new cache key
starts empty) and warm (once `main` has saved it). After the merge, the
`v0-rust-scaling-check-lint` entry `main` saves must hold the scaling
artifacts; if `main`'s first run with the key failed before the check, the
entry is deleted and a dispatch on `main` saves it again.

## Amendment 2026-11-21: clippy, warnings denied

#286 gated the last item that was dead without debug assertions, so the step
became `cargo clippy --locked --workspace --all-targets --all-features
--profile scaling -- -D warnings`, as the Decision planned: it fails on
errors and on warnings, in the workspace crates only, and it replaces the
`cargo check` (clippy type-checks too). `CLAUDE.md` and
`docs/DEVELOPMENT.md` list the same command (the maintainer approved the
`CLAUDE.md` line). Measured locally after the check's artifacts were
there: 39 seconds, no warning.

## Links

- [A faster build job](2026-11-21-faster-build-job.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
