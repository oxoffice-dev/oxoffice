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

**`ci.yml`** runs on every pull request and every push to `main`, in three
parallel jobs:

- `lint`: format, clippy with warnings denied, cargo-deny (advisories,
  licences, sources)
- `wasm`: the WebAssembly check
- `build`: one build of the wheel with the `ci` profile, then the smoke
  test, the Rust integration tests and the Python tests. The wheel carries
  both the Python package and the `oxoffice` command, so one build serves
  the Python and the CLI checks.
- pull requests run on Linux only, `main` runs the same jobs on Linux, macOS
  and Windows (both changed since: pull requests, see
  [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md);
  `main` on Linux since 2026-11-21, see
  [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md))

The `ci` cargo profile has no LTO, debug assertions on, the workspace crates
unoptimised and the dependencies at opt-level 1. The `release` profile (fat
LTO, one codegen unit) is only used by the release workflow: building the
wheel with it took more than 10 minutes on a cold runner, against 6 minutes
with `ci` at opt-level 1, and about 30 % less again with unoptimised
workspace crates. rust-cache also keeps the workspace crates, so a commit
that does not touch a crate does not rebuild it, and a change that only
touches documentation builds nothing. (It did not: a fresh checkout makes
the sources newer than the cached artifacts, and every workspace crate
compiled on every run. Dropped on 2026-11-21, see
[a faster build job](2026-11-21-faster-build-job.md).)

What remains is dominated by three very large crates compiled one after the
other: `oxoffice-docx` (59 s on a fast laptop), `oxoffice-docx-oxml` (45 s)
and `oxoffice-pptx` (33 s). Splitting them is part of the code inspection on
the roadmap, and is the lever that will shorten CI further.

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

4. It repackages each platform's wheel as a conda package
   (`conda/recipe.yaml`) and publishes it with `rattler-build publish`, which
   builds it, runs the recipe's tests and uploads it to the `hadim/oxoffice`
   channel on prefix.dev through a trusted publisher (workflow `release.yml`,
   environment `conda`), with an attestation.

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

5. It publishes the wheels and the sdist to PyPI through a trusted publisher
   (workflow `release.yml`, environment `pypi`), with PyPI attestations.

No token is stored anywhere: PyPI and prefix.dev trust the workflow through
OIDC. The install channels are documented in `docs/INSTALL.md`.

### Not now

crates.io and conda-forge. They come when the API settles, each with its own
spec.

## Plan

1. `ci.yml`, `pr-title.yml`, `cliff.toml` (done).
2. `release.yml` and `scripts/set_version.py` (done), then a `dry_run`
   once the repository is on GitHub.
3. v0.1.0.

## Learnings

- Pull requests were rebase merged for a few hours on 2026-11-20, then went
  back to squash merges: rebase merges land unsigned on `main` (GitHub
  rewrites the commits and does not sign them) and make every commit of a
  branch a changelog line that would need its own check. Squash keeps one
  signed commit per pull request, named by the checked title. The title check
  now runs only when a pull request is opened, renamed or marked ready, since
  running it on every push was noise.
- Pull requests run on Linux, macOS and Windows too since 2026-11-20, not on
  Linux only: the repository is public, so the runners cost nothing, the jobs
  run in parallel and Windows (about 10 minutes) bounds the wait, and a
  Windows-only failure would otherwise first show on a red `main`.

- If `main` gets branch protection that forbids direct pushes, the release
  commit needs a GitHub App token or a release pull request. Not needed yet.
