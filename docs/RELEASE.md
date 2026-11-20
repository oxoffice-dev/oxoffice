# Release

One manual workflow, no manual tags, no hand-edited changelog.

## Cutting a release

1. Make sure `main` is green.
2. Actions, **Release**, **Run workflow**, give the version (`0.2.0`,
   without `v`). Tick **dry_run** to build everything without publishing.
3. The workflow sets the version, regenerates `CHANGELOG.md` with git-cliff,
   commits `chore(release): vX.Y.Z`, tags `vX.Y.Z`, builds and publishes the
   GitHub release.

## Versions

SemVer, one version for every crate and the wheel. While we are in 0.x, a
breaking change bumps the minor version and anything else the patch version.
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

The same wheels are repackaged for conda and uploaded to the `hadim/oxoffice`
channel on prefix.dev. PyPI, crates.io and conda-forge are not published yet.

## Changelog

`CHANGELOG.md` is generated from the conventional commits of `main` by
git-cliff (`cliff.toml`). Commit titles are the changelog, which is why pull
request titles are checked. Preview it locally with
`uvx git-cliff --unreleased`.
