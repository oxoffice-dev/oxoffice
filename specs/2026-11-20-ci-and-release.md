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
   for that version, every artifact, the two installers, an SPDX SBOM,
   `SHA256SUMS`, and a build provenance attestation signed through Sigstore
   that covers every file listed in `SHA256SUMS`. The release is marked
   latest, so `releases/latest/download/...` always points at it.

`dry_run` builds everything from `main` and publishes nothing.

**Everything is pinned.** Every action is referenced by its full commit SHA
with the release tag as a comment, Python tools come from requirement files
compiled with hashes (`.github/requirements/*.txt`, installed with
`--require-hashes`), maturin and git-cliff run at fixed versions inside their
actions, and the Rust toolchain is pinned by `rust-toolchain.toml`.
Dependabot proposes the updates once a month.

### Release assets

The contract with oxoffice-skills and any installer:

| Asset | Name |
| --- | --- |
| CLI, Unix | `oxoffice-<version>-<rust target>.tar.gz`, holding `oxoffice-<version>-<target>/oxoffice` |
| CLI, Windows | `oxoffice-<version>-x86_64-pc-windows-msvc.zip`, holding `oxoffice.exe` |
| Wheels | maturin's names, `oxoffice-<version>-cp310-abi3-<platform>.whl` |
| Source | `oxoffice-<version>.tar.gz` |
| Installers | `install.sh` (Linux, macOS) and `install.ps1` (Windows) |
| SBOM | `oxoffice-<version>.spdx.json`, the dependencies of the release |
| Checksums | `SHA256SUMS`, one `sha256sum` line per asset |
| Provenance | `oxoffice-<version>.provenance.sigstore.json`, the signed SLSA provenance bundle, also stored in GitHub's attestation registry |

### Proving where a file comes from

Every asset can be checked two ways:

- `sha256sum -c SHA256SUMS --ignore-missing`: the file is the one the release
  lists.
- `gh attestation verify <file> --repo oxoffice-dev/oxoffice`: the file was
  built by `release.yml` in this repository, from the tagged commit, on a
  GitHub-hosted runner. This is what rules out a tampered asset: a file
  uploaded by hand, or built anywhere else, has no valid attestation.

The installers check the checksum always, and the attestation when the
GitHub CLI is installed and signed in.

### Installing the CLI

Like pixi, one line per platform, always from the latest release (or
`OXOFFICE_VERSION`):

```bash
curl -fsSL https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.sh | sh
```

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.ps1 | iex"
```

They install `oxoffice` into `~/.oxoffice/bin` and add it to the `PATH`.

The wheel also installs the `oxoffice` command, so a Python install gives
both the API and the CLI.

### Not now

PyPI, crates.io and conda-forge publication. They come as extra jobs of
`release.yml` when the API settles, each with its own spec. PyPI will use a
trusted publisher bound to `release.yml` and a `pypi` environment, so no
token is stored anywhere.

## Plan

1. `ci.yml`, `pr-title.yml`, `cliff.toml` (done).
2. `release.yml` and `scripts/set_version.py` (done), then a `dry_run`
   once the repository is on GitHub.
3. v0.1.0.

## Learnings

- If `main` gets branch protection that forbids direct pushes, the release
  commit needs a GitHub App token or a release pull request. Not needed yet.
