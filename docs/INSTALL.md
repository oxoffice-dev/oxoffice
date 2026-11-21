# Install

oxoffice ships the same build through several channels. Pick the one that fits
how you work. Every channel carries the `oxoffice` command, and the Python
channels also carry the `oxoffice` package (`oxoffice.docx`, `oxoffice.pptx`).

| Channel | Gives you | Best for |
| --- | --- | --- |
| [One-line installer](#one-line-installer) | the `oxoffice` command | a shell, no Python needed |
| [PyPI](#pypi) | the Python package and the command | Python projects, `uv`, `pip` |
| [conda (prefix.dev)](#conda) | the Python package and the command | pixi and conda environments |
| [GitHub release](#github-release) | every file, by hand | offline or pinned installs |
| [Rust](#rust) | the crates | Rust projects |

Linux (x86_64 and aarch64, glibc 2.28 or later), macOS (arm64 and x86_64) and
Windows (x86_64). Python 3.10 or later.

## One-line installer

Linux and macOS:

```bash
curl -fsSL https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.ps1 | iex"
```

The installer downloads the archive for your platform from the latest release,
checks it against `SHA256SUMS` (and its build provenance when the GitHub CLI is
signed in), puts `oxoffice` in `~/.oxoffice/bin` and adds that folder to your
`PATH`. Set `OXOFFICE_VERSION=0.1.0` to pin a version, `OXOFFICE_HOME` to
install elsewhere, `OXOFFICE_NO_PATH_UPDATE=1` to leave your shell files alone.

## PyPI

```bash
pip install oxoffice           # in a virtual environment
uv add oxoffice                # in a uv project
uv tool install oxoffice       # only the `oxoffice` command, isolated
```

```python
from oxoffice.docx import Document
from oxoffice.pptx import Presentation
```

The wheels are published with PyPI attestations, visible on each file's page.

## conda

The packages live in the `hadim/oxoffice` channel on prefix.dev and use
conda-forge for Python:

```bash
pixi global install -c https://prefix.dev/hadim/oxoffice -c conda-forge oxoffice   # the command
pixi add --channel https://prefix.dev/hadim/oxoffice oxoffice                      # in a pixi project
conda install -c https://repo.prefix.dev/hadim/oxoffice -c conda-forge oxoffice    # in a conda environment
```

Each package is published with an attestation through prefix.dev's trusted
publishing.

## GitHub release

Every [release](https://github.com/oxoffice-dev/oxoffice/releases) carries:

- `oxoffice-<version>-<target>.tar.gz` (`.zip` on Windows): the `oxoffice` binary
- `oxoffice-<version>-cp310-abi3-<platform>.whl`: the Python wheels
- `oxoffice-<version>.tar.gz`: the source distribution
- `install.sh`, `install.ps1`: the one-line installers
- `oxoffice-<version>-limitations.md`: the known limitations of that version
  (the installed build lists them too: `oxoffice limitations`)
- `oxoffice-<version>.spdx.json`: the SBOM
- `SHA256SUMS` and `oxoffice-<version>.provenance.sigstore.json`

Check what you downloaded before using it:

```bash
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify oxoffice-<version>-<target>.tar.gz --repo oxoffice-dev/oxoffice
```

The attestation proves the file was built by this repository's release
workflow from the tagged commit, on a GitHub-hosted runner.

## Rust

Until the crates are on crates.io, depend on the repository:

```toml
[dependencies]
oxoffice-docx = { git = "https://github.com/oxoffice-dev/oxoffice", tag = "v0.1.0" }
oxoffice-pptx = { git = "https://github.com/oxoffice-dev/oxoffice", tag = "v0.1.0" }
```

## From source

```bash
git clone https://github.com/oxoffice-dev/oxoffice && cd oxoffice
cargo build --release -p oxoffice-cli                  # target/release/oxoffice
pip install maturin && maturin build --release -m crates/oxoffice-py/Cargo.toml
```
