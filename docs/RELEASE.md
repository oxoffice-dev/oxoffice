# Release

One manual workflow, no manual tags, no hand-edited changelog.

## Cutting a release

1. Make sure `main` is green.
2. Run a dry run first: Actions, **Release**, **Run workflow**, the version,
   **dry_run** ticked. It builds `main` as it is and runs every check but the
   one that the wheels carry the new version, and publishes nothing.
3. Run it again without **dry_run**. The workflow sets the version,
   regenerates `CHANGELOG.md` with git-cliff, commits `chore(release):
   vX.Y.Z`, tags `vX.Y.Z`, builds, checks each wheel on its platform with
   `scripts/wheel_smoke.py` and publishes the GitHub release, then PyPI and
   prefix.dev.

The smoke check opens, edits, saves and renders the corpus on the main thread,
then measures the smallest thread stack the same work needs and fails above
its budget, 1 MiB (`STACK_BUDGET_KIB`): the main thread of most Windows
programs that embed Python. Each job's summary shows the measure.

### When a run fails after the tag

The commit and the tag are pushed before the builds, so a failed build or
check leaves them on `main` with nothing published. Fix the cause on `main`,
then either release the next patch version, or remove the tag
(`git push --delete origin vX.Y.Z`), revert the `chore(release)` commit and
run the same version again. A failure in PyPI or prefix.dev after the GitHub
release is retried with **Re-run failed jobs**: both skip files already
uploaded.

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
