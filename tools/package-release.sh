#!/usr/bin/env bash
# Packages a built vivacity binary as a release asset, the same way on every
# platform (release.yml runs it under bash — Git Bash on windows-latest):
#   vivacity-<tag>-<target>.tar.gz         (vivacity[.exe], README, licences)
#   vivacity-<tag>-<target>.tar.gz.sha256  (`<hex>  <name>`, as sha256sum -c reads it)
# then re-reads the archive: hash check, extraction, `--version`.
#
# Usage: tools/package-release.sh <tag> <target> [binary] [out-dir]
#   binary  default: target/<target>/release/vivacity[.exe], else target/release/…
#   out-dir default: .
set -euo pipefail
tag="$1"; target="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exe=""; case "$target" in *-windows-*) exe=".exe" ;; esac
bin="${3:-}"
if [ -z "$bin" ]; then
  bin="$ROOT/target/$target/release/vivacity$exe"
  [ -f "$bin" ] || bin="$ROOT/target/release/vivacity$exe"
fi
out="${4:-.}"; mkdir -p "$out"; out="$(cd "$out" && pwd)"
[ -f "$bin" ] || { echo "package-release: binary not found: $bin" >&2; exit 1; }
name="vivacity-$tag-$target.tar.gz"
dist="$(mktemp -d)"; trap 'rm -rf "$dist"' EXIT
mkdir "$dist/pkg"
cp "$bin" "$dist/pkg/vivacity$exe"
chmod 0755 "$dist/pkg/vivacity$exe"
cp "$ROOT/README.md" "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/NOTICE.md" "$dist/pkg/"
# Explicit member list: no `./` prefix, so `tar -xzf … vivacity` works with
# GNU tar and bsdtar alike (install.sh, tar.exe on Windows).
tar -C "$dist/pkg" -czf "$out/$name" "vivacity$exe" README.md NOTICE.md LICENSE-MIT LICENSE-APACHE
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$out" && sha256sum "$name" > "$name.sha256")
else
  (cd "$out" && shasum -a 256 "$name" > "$name.sha256")
fi
# Self-check: the asset must round-trip exactly as install.sh consumes it.
mkdir "$dist/check"
(cd "$out" && { command -v sha256sum >/dev/null 2>&1 && sha256sum -c --quiet "$name.sha256" || shasum -a 256 -c --quiet "$name.sha256"; })
tar -xzf "$out/$name" -C "$dist/check" "vivacity$exe"
"$dist/check/vivacity$exe" --version >/dev/null
echo "package-release: $out/$name ($(wc -c < "$out/$name" | tr -d ' ') bytes) and .sha256 written and verified" >&2
