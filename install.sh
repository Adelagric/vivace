#!/usr/bin/env sh
# Installs the vivacity binary from the latest GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/Adelagric/vivacity/main/install.sh | sh
# Linux (x86_64, aarch64), macOS (arm64, x86_64), Windows x86_64 under Git
# Bash / MSYS2 (for PowerShell, see install.ps1).
# Variables: VIVACITY_VERSION (default: latest), VIVACITY_INSTALL_DIR
#            (default: ~/.local/bin; on Windows %LOCALAPPDATA%\Programs\vivacity),
#            VIVACITY_DOWNLOAD_BASE (default: the GitHub release downloads —
#            a directory or URL holding the assets, used by the CI self-test).
set -eu
REPO="Adelagric/vivacity"
os=$(uname -s); arch=$(uname -m)
exe=""
case "$os" in
  Linux)  os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  MINGW*|MSYS*|CYGWIN*) os="pc-windows-msvc"; exe=".exe" ;;
  *) echo "vivacity: unsupported OS: $os" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "vivacity: unsupported architecture: $arch" >&2; exit 1 ;;
esac
target="$arch-$os"
case "$target" in
  aarch64-pc-windows-msvc) echo "vivacity: no Windows ARM build yet (x86_64 only); build from source with cargo" >&2; exit 1 ;;
esac
if [ -n "${VIVACITY_INSTALL_DIR:-}" ]; then
  DIR="$VIVACITY_INSTALL_DIR"
elif [ -n "$exe" ] && [ -n "${LOCALAPPDATA:-}" ]; then
  DIR="$LOCALAPPDATA/Programs/vivacity"
else
  DIR="$HOME/.local/bin"
fi
if [ -n "${VIVACITY_VERSION:-}" ]; then
  tag="$VIVACITY_VERSION"
else
  # The `releases/latest` redirect carries the tag in its Location header —
  # no API call, no rate limit.
  tag=$(curl -fsSI -o /dev/null -w '%{redirect_url}' "https://github.com/$REPO/releases/latest" | sed 's|.*/tag/||')
  [ -n "$tag" ] || { echo "vivacity: could not determine the latest release" >&2; exit 1; }
fi
name="vivacity-$tag-$target.tar.gz"
base="${VIVACITY_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download/$tag}"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
echo "vivacity: downloading $name" >&2
case "$base" in
  http://*|https://*|file://*)
    curl -fsSL -o "$tmp/$name" "$base/$name"
    curl -fsSL -o "$tmp/$name.sha256" "$base/$name.sha256" ;;
  *)
    cp "$base/$name" "$base/$name.sha256" "$tmp/" ;;
esac
(cd "$tmp" && { command -v sha256sum >/dev/null 2>&1 && sha256sum -c --quiet "$name.sha256" || shasum -a 256 -c --quiet "$name.sha256"; })
mkdir -p "$DIR"
tar -xzf "$tmp/$name" -C "$tmp" "vivacity$exe"
install -m 0755 "$tmp/vivacity$exe" "$DIR/vivacity$exe"
echo "vivacity: installed to $DIR/vivacity$exe ($tag)" >&2
case ":$PATH:" in
  *":$DIR:"*) ;;
  *)
    shown="$DIR"
    if [ -n "$exe" ] && command -v cygpath >/dev/null 2>&1; then shown=$(cygpath -w "$DIR"); fi
    echo "vivacity: add $shown to your PATH" >&2 ;;
esac
