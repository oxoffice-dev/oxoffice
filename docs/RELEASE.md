# Release

One manual workflow, no manual tags, no hand-edited changelog.

## Cutting a release

1. Make sure `main` is green, and that a run building, testing and timing
   every scaling call on macOS and Windows is green on the commit you release
   (a push to `main` runs on Linux only): the last nightly run
   (`gh run list -w CI -e schedule -L 1 --json headSha,conclusion`) if it ran
   on that commit, otherwise `gh workflow run ci.yml --ref main` and wait for
   it.
2. Actions, **Release**, **Run workflow** from `main`, the version (or
   `gh workflow run release.yml -f version=X.Y.Z`). Merging into `main`
   meanwhile is fine. Every build job checks out the commit the run was
   dispatched on and sets the version in its own workspace
   (`scripts/set_version.py`, then `cargo update --workspace`): the binaries,
   the wheels and the sdist all come from that one commit. Each wheel runs
   on its platform (`scripts/wheel_smoke.py`), and each conda package is
   built and tested into a local channel.
3. Only once all of that is green, the last job sets the version again on
   the dispatched commit, regenerates `CHANGELOG.md` with git-cliff, commits
   `chore(release): vX.Y.Z` on top of that commit and tags it: the tag holds
   exactly the tree every job built, plus the version and the changelog. It
   builds the remaining assets and the release notes, then pushes `main` and
   the tag in one atomic push (both land or neither does), and publishes the
   GitHub release, then PyPI and prefix.dev.

If `main` did not move during the run, it is fast-forwarded to the version
commit. If pull requests merged meanwhile, they are not in the release:
`main` gets a merge commit, `chore(release): merge vX.Y.Z into main`, whose
parents are the current `main` and the version commit, so the tag stays on
`main`'s history and git-cliff puts those pull requests in the next release.
A conflict in that merge is resolved by the workflow: `CHANGELOG.md` takes
the release's, `Cargo.toml` and `Cargo.lock` take `main`'s, and the version is
set again on them (a crate added during the run gets it too).

The smoke check opens, edits, saves and renders the corpus on the main thread,
then measures the smallest thread stack the same work needs and fails above
its budget, 1 MiB (`STACK_BUDGET_KIB`): the main thread of most Windows
programs that embed Python. Each job's summary shows the measure.

### When a run fails

Nothing reaches `main` or the tags before every build, smoke check, conda
check and asset is done: the push is the last step but the GitHub release.

- **A build, a check or an asset fails**: nothing was pushed. A transient
  failure is retried with **Re-run failed jobs**. Otherwise fix the cause on
  `main` and run the same version again.
- **The push is refused five times** (`main` kept moving, or a rule refused
  it): nothing was pushed. **Re-run failed jobs**.
- **The GitHub release fails** after the push: **Re-run failed jobs**. The
  last job finds the tag on the version commit it made and goes on; it stops
  if the tag is on any other commit. A new run of the same version stops at
  once, since the tag exists. If it keeps failing, delete the tag
  (`git push --delete origin vX.Y.Z`), fix the cause on `main` and run the
  same version again; the version commit already merged into `main` stays.
- **PyPI or prefix.dev fails** after the GitHub release: **Re-run failed
  jobs**: both skip files already uploaded.

## Versions

SemVer, one version for every crate and the wheel. While we are in 0.x, a
breaking change bumps the minor version and anything else the patch version.
No prereleases yet: the workflow only takes `X.Y.Z`. A prerelease would need
its PEP 440 and conda spellings (`0.2.0rc1`, no `-`) and a GitHub release
that is not marked latest, which the installers would otherwise pick.
`scripts/set_version.py` is the only thing that writes the version.

## What a release carries

| Asset | Contents |
| --- | --- |
| `oxoffice-X.Y.Z-<target>.tar.gz` / `.zip` | the `oxoffice` binary for Linux (x86_64, aarch64, glibc 2.28+), macOS (arm64, x86_64), Windows (x86_64) |
| `oxoffice-X.Y.Z-cp310-abi3-*.whl` | the Python package with the `oxoffice` command, Python 3.10+ |
| `oxoffice-X.Y.Z.tar.gz` | the source distribution |
| `install.sh`, `install.ps1` | the one-line installers of the CLI |
| `oxoffice-X.Y.Z-limitations.md` | the known limitations of this release, as `oxoffice limitations` lists them |
| `oxoffice-X.Y.Z.spdx.json` | the SBOM |
| `SHA256SUMS` | checksums of every asset |
| `oxoffice-X.Y.Z.provenance.sigstore.json` | the signed build provenance of every asset |

Check a downloaded file with `sha256sum -c SHA256SUMS --ignore-missing` and
`gh attestation verify <file> --repo oxoffice-dev/oxoffice`.

The same wheels and the sdist go to PyPI, and the wheels are repackaged for
conda and uploaded to the `hadim/oxoffice` channel on prefix.dev, both through
trusted publishers (environments `pypi` and `conda`) with attestations.
crates.io and conda-forge are not published yet. How users install each
channel is in [INSTALL.md](INSTALL.md).

## Changelog

`CHANGELOG.md` is generated from the conventional commits of `main` by
git-cliff (`cliff.toml`). Commit titles are the changelog, and pull requests
are squash merged under their title, which is why pull request titles are
checked. Preview it locally with
`uvx git-cliff --unreleased`.
