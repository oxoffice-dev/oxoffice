---
date: 2026-11-20
status: accepted
scope: release
---

# CI and release from scratch

## Context

We start CI and release from scratch with one version for the whole
project. We want two plain workflows, inspired by kaya: one that tests, one
that releases.

## Decision

**`ci.yml`** runs on every pull request and every push to `main`:

- format, clippy with warnings denied, cargo-deny (advisories, licences,
  sources)
- Rust tests on the workspace, without the Python crates, and the WASM check
- the wheel built with maturin, installed, smoke-tested, then the Python tests
- pull requests run on Linux only, `main` runs the same jobs on Linux, macOS
  and Windows

**`pr-title.yml`** checks that a pull request title is a conventional commit,
since squash merges turn titles into the commits of `main`.

**`release.yml`** is started by hand with a version (`0.1.0`, no `v`):

1. `scripts/set_version.py` writes the version into the workspace,
   `cargo update --workspace` refreshes the lock file, git-cliff regenerates
   `CHANGELOG.md`, and the workflow commits `chore(release): vX.Y.Z`, tags
   `vX.Y.Z` and pushes both.
2. It builds the `oxoffice` binary for `x86_64` and `aarch64` Linux (glibc
   2.28 through zig), `aarch64` and `x86_64` macOS, and `x86_64` Windows; the
   abi3 wheels for the same platforms (manylinux 2.28 on Linux); and the
   source distribution.
3. It publishes a GitHub release named `oxoffice vX.Y.Z` with git-cliff notes
   for that version, every artifact, and `SHA256SUMS`. 0.x releases are
   marked as pre-releases.

`dry_run` builds everything from `main` and publishes nothing.

### Release assets

The contract with oxoffice-skills and any installer:

| Asset | Name |
| --- | --- |
| CLI, Unix | `oxoffice-<version>-<rust target>.tar.gz`, holding `oxoffice-<version>-<target>/oxoffice` |
| CLI, Windows | `oxoffice-<version>-x86_64-pc-windows-msvc.zip`, holding `oxoffice.exe` |
| Wheels | maturin's names, `oxoffice-<version>-cp310-abi3-<platform>.whl` |
| Source | `oxoffice-<version>.tar.gz` |
| Checksums | `SHA256SUMS`, one `sha256sum` line per asset |

The wheel also installs the `oxoffice` command, so a Python install gives
both the API and the CLI.

### Not now

PyPI, crates.io and conda-forge publication. They come as extra jobs of
`release.yml` when the API settles, each with its own spec.

## Plan

1. `ci.yml`, `pr-title.yml`, `cliff.toml` (done).
2. `release.yml` and `scripts/set_version.py` (done), then a `dry_run`
   once the repository is on GitHub.
3. v0.1.0.

## Learnings

- If `main` gets branch protection that forbids direct pushes, the release
  commit needs a GitHub App token or a release pull request. Not needed yet.
