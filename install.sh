#!/usr/bin/env sh
# Installs the vivacity binary from the latest GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/Adelagric/vivacity/main/install.sh | sh
# Variables: VIVACITY_VERSION (default: latest), VIVACITY_INSTALL_DIR (default: ~/.local/bin),
#            GITHUB_TOKEN (optional: authenticates the release lookup — CI runners share
#            the anonymous API rate limit of 60 requests/hour per IP)
set -eu
REPO="Adelagric/vivacity"
DIR="${VIVACITY_INSTALL_DIR:-$HOME/.local/bin}"
auth=""
if [ -n "${GITHUB_TOKEN:-}" ]; then
  auth="Authorization: Bearer $GITHUB_TOKEN"
fi
os=$(uname -s); arch=$(uname -m)
case "$os" in
  Linux)  os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "vivacity: unsupported OS: $os" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "vivacity: unsupported architecture: $arch" >&2; exit 1 ;;
esac
target="$arch-$os"
if [ -n "${VIVACITY_VERSION:-}" ]; then
  tag="$VIVACITY_VERSION"
else
  tag=$(curl -fsSL ${auth:+-H "$auth"} "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$tag" ] || { echo "vivacity: could not determine the latest release" >&2; exit 1; }
fi
name="vivacity-$tag-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$name"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
echo "vivacity: downloading $name" >&2
curl -fsSL -o "$tmp/$name" "$url"
curl -fsSL -o "$tmp/$name.sha256" "$url.sha256"
(cd "$tmp" && { command -v sha256sum >/dev/null && sha256sum -c --quiet "$name.sha256" || shasum -a 256 -c --quiet "$name.sha256"; })
mkdir -p "$DIR"
tar -xzf "$tmp/$name" -C "$tmp" vivacity
install -m 0755 "$tmp/vivacity" "$DIR/vivacity"
echo "vivacity: installed to $DIR/vivacity ($tag)" >&2
case ":$PATH:" in *":$DIR:"*) ;; *) echo "vivacity: add $DIR to your PATH" >&2 ;; esac
