#!/usr/bin/env sh
# Installe le binaire vivace depuis la dernière release GitHub.
#   curl -fsSL https://raw.githubusercontent.com/Adelagric/vivace/main/install.sh | sh
# Variables : VIVACE_VERSION (défaut : latest), VIVACE_INSTALL_DIR (défaut : ~/.local/bin)
set -eu
REPO="Adelagric/vivace"
DIR="${VIVACE_INSTALL_DIR:-$HOME/.local/bin}"
os=$(uname -s); arch=$(uname -m)
case "$os" in
  Linux)  os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "vivace: système non supporté: $os" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "vivace: architecture non supportée: $arch" >&2; exit 1 ;;
esac
target="$arch-$os"
if [ -n "${VIVACE_VERSION:-}" ]; then
  tag="$VIVACE_VERSION"
else
  tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$tag" ] || { echo "vivace: impossible de déterminer la dernière version" >&2; exit 1; }
fi
name="vivace-$tag-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$name"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
echo "vivace: téléchargement de $name" >&2
curl -fsSL -o "$tmp/$name" "$url"
curl -fsSL -o "$tmp/$name.sha256" "$url.sha256"
(cd "$tmp" && { command -v sha256sum >/dev/null && sha256sum -c --quiet "$name.sha256" || shasum -a 256 -c --quiet "$name.sha256"; })
mkdir -p "$DIR"
tar -xzf "$tmp/$name" -C "$tmp" vivace
install -m 0755 "$tmp/vivace" "$DIR/vivace"
echo "vivace: installé dans $DIR/vivace ($tag)" >&2
case ":$PATH:" in *":$DIR:"*) ;; *) echo "vivace: ajoutez $DIR à votre PATH" >&2 ;; esac
