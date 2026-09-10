#!/usr/bin/env sh
# Installs the vivace binary from the latest GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/Adelagric/vivace/main/install.sh | sh
# Variables: VIVACE_VERSION (default: latest), VIVACE_INSTALL_DIR (default: ~/.local/bin),
#            GITHUB_TOKEN (optional: authenticates the release lookup — CI runners share
#            the anonymous API rate limit of 60 requests/hour per IP)
set -eu
REPO="Adelagric/vivace"
DIR="${VIVACE_INSTALL_DIR:-$HOME/.local/bin}"
auth=""
if [ -n "${GITHUB_TOKEN:-}" ]; then
  auth="Authorization: Bearer $GITHUB_TOKEN"
fi
os=$(uname -s); arch=$(uname -m)
case "$os" in
  Linux)  os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "vivace: unsupported OS: $os" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "vivace: unsupported architecture: $arch" >&2; exit 1 ;;
esac
target="$arch-$os"
if [ -n "${VIVACE_VERSION:-}" ]; then
  tag="$VIVACE_VERSION"
else
  tag=$(curl -fsSL ${auth:+-H "$auth"} "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$tag" ] || { echo "vivace: could not determine the latest release" >&2; exit 1; }
fi
name="vivace-$tag-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$name"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
echo "vivace: downloading $name" >&2
curl -fsSL -o "$tmp/$name" "$url"
curl -fsSL -o "$tmp/$name.sha256" "$url.sha256"
(cd "$tmp" && { command -v sha256sum >/dev/null && sha256sum -c --quiet "$name.sha256" || shasum -a 256 -c --quiet "$name.sha256"; })
mkdir -p "$DIR"
tar -xzf "$tmp/$name" -C "$tmp" vivace
install -m 0755 "$tmp/vivace" "$DIR/vivace"
echo "vivace: installed to $DIR/vivace ($tag)" >&2
case ":$PATH:" in *":$DIR:"*) ;; *) echo "vivace: add $DIR to your PATH" >&2 ;; esac
