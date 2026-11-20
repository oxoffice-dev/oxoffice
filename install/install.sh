#!/bin/sh
# Install the oxoffice command line.
#
#   curl -fsSL https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.sh | sh
#
# Environment:
#   OXOFFICE_VERSION          version to install, such as 0.1.0 (default: the latest release)
#   OXOFFICE_HOME             install root (default: ~/.oxoffice), the binary goes to $OXOFFICE_HOME/bin
#   OXOFFICE_NO_PATH_UPDATE   set to any value to leave shell startup files alone
#
# The archive is checked against the release's SHA256SUMS before anything is
# installed. When the GitHub CLI is available and signed in, the archive's
# build provenance is verified too (gh attestation verify).

set -eu

REPO="oxoffice-dev/oxoffice"
HOME_DIR="${OXOFFICE_HOME:-$HOME/.oxoffice}"
BIN_DIR="$HOME_DIR/bin"

say() { printf '%s\n' "oxoffice: $*"; }
die() { printf '%s\n' "oxoffice: error: $*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "this installer needs '$1'"; }
need curl
need tar
need uname

case "$(uname -s)" in
  Linux) os=unknown-linux-gnu ;;
  Darwin) os=apple-darwin ;;
  *) die "unsupported system $(uname -s), on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac
target="$arch-$os"

version="${OXOFFICE_VERSION:-}"
if [ -z "$version" ]; then
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")
  version="${latest##*/v}"
  [ -n "$version" ] && [ "$version" != "$latest" ] || die "could not find the latest release"
fi
version="${version#v}"

archive="oxoffice-$version-$target.tar.gz"
base="https://github.com/$REPO/releases/download/v$version"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading oxoffice $version for $target"
curl -fsSL "$base/$archive" -o "$tmp/$archive" || die "no build $archive in release v$version"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "release v$version has no SHA256SUMS"

expected=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "$archive is not listed in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
else
  actual=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
fi
[ "$actual" = "$expected" ] || die "checksum mismatch for $archive, nothing installed"
say "checksum verified"

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  if gh attestation verify "$tmp/$archive" --repo "$REPO" >/dev/null 2>&1; then
    say "build provenance verified"
  else
    die "build provenance of $archive could not be verified, nothing installed"
  fi
else
  say "tip: with the GitHub CLI signed in, this installer also verifies the build provenance"
fi

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$BIN_DIR"
mv "$tmp/oxoffice-$version-$target/oxoffice" "$BIN_DIR/oxoffice"
chmod +x "$BIN_DIR/oxoffice"
say "installed $("$BIN_DIR/oxoffice" --version) in $BIN_DIR"

case ":$PATH:" in
  *":$BIN_DIR:"*) exit 0 ;;
esac
if [ -n "${OXOFFICE_NO_PATH_UPDATE:-}" ]; then
  say "add $BIN_DIR to your PATH"
  exit 0
fi
line="export PATH=\"$BIN_DIR:\$PATH\""
for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
  if [ -f "$rc" ] && ! grep -Fq "$BIN_DIR" "$rc"; then
    printf '\n# oxoffice\n%s\n' "$line" >>"$rc"
    say "added $BIN_DIR to PATH in $rc"
  fi
done
fish_conf="$HOME/.config/fish/config.fish"
if [ -f "$fish_conf" ] && ! grep -Fq "$BIN_DIR" "$fish_conf"; then
  printf '\n# oxoffice\nfish_add_path "%s"\n' "$BIN_DIR" >>"$fish_conf"
  say "added $BIN_DIR to PATH in $fish_conf"
fi
say "restart your shell, or run: $line"
