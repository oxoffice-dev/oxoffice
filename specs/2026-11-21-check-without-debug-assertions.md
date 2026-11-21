---
date: 2026-11-21
status: shipped
scope: ci
---

# A check without debug assertions on every pull request

## Context

Some code exists only with debug assertions: 98 `cfg(debug_assertions)` in
the workspace, mostly the checks that compare every edit with a copy of the
whole document. On a pull request, clippy, the WebAssembly check and the
build job all compile with debug assertions (the `ci` profile turns them
on). The only build without them is the scaling job's wheel
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
  after clippy: every crate, test and feature, without debug assertions.
  `cargo check` builds no code, so it costs the type checking only.
- The lint job's rust-cache gets a key of its own (`scaling-check`): `main`
  saves a cache only when its key misses, so the new key makes it save the
  artifacts of this check once.
- Errors only for now: without debug assertions one function of
  `crates/oxoffice-docx/src/document.rs` is dead code (a warning). Once its
  owners gate it, the step becomes `cargo clippy --profile scaling ... -- -D
  warnings`, which denies warnings in the workspace crates only.

Not here: building or testing without debug assertions on pull requests
other than the scaling job's; the nightly run does both on three platforms.

## Plan

One pull request. Checked locally: the check takes 34 s on a cold target and
19 s when every workspace crate changed (8 jobs); a helper under
`#[cfg(debug_assertions)]` re-exported without the cfg passes `cargo check`
with debug assertions and fails this one (E0432). The step's time on the
runner goes in the pull request.

## Links

- [A faster build job](2026-11-21-faster-build-job.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#ci)
