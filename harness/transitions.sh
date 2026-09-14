#!/usr/bin/env bash
# Plugins that vivace does not emulate must hand over to Composer BEFORE
# touching the disk, naming the plugin. Case: the Drupal recommended-project,
# whose lock carries drupal/core-composer-scaffold (not emulated: its source
# is GPL-2.0-or-later, see NOTICE.md). `vivace install --no-fallback` must
# refuse with exit code 3 and leave the tree untouched; the fallback path
# itself is proven by `harness/diff-vendor.sh drupal` (whole project
# identical to Composer's).
#
# Usage: harness/transitions.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}/transitions"
[ -x "$VIVACE" ] || { echo "binary missing: cargo build --release"; exit 1; }
src="$ROOT/fixtures/work/drupal"
dir="$WORK/drupal-scaffold"
rm -rf "$dir"; mkdir -p "$dir"
(cd "$src" && tar --exclude=./vendor --exclude=./web --exclude=./recipes -cf - .) | (cd "$dir" && tar -xf -)
cd "$dir"
before=$( (find . -type d | sort; find . -type f -exec shasum -a 256 {} +) | sort | shasum -a 256)
code=0
"$VIVACE" install --no-fallback --offline >"$WORK/scaffold.log" 2>&1 || code=$?
if [ "$code" != 3 ]; then
  echo "FAIL drupal: expected exit 3 (out of scope, no fallback), got $code:"; tail -5 "$WORK/scaffold.log"; exit 1
fi
if ! grep -q 'drupal/core-composer-scaffold' "$WORK/scaffold.log"; then
  echo "FAIL drupal: the refusal does not name the plugin:"; tail -5 "$WORK/scaffold.log"; exit 1
fi
after=$( (find . -type d | sort; find . -type f -exec shasum -a 256 {} +) | sort | shasum -a 256)
[ "$before" = "$after" ] || { echo "FAIL drupal: the tree was modified before the refusal"; exit 1; }
echo "OK   drupal: core-composer-scaffold handed over to Composer without touching the disk"
