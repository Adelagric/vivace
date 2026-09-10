#!/usr/bin/env bash
# Benchmarks reproductibles vivace vs Composer sur les fixtures (hyperfine).
# Produit un tableau Markdown sur stdout (et dans $GITHUB_STEP_SUMMARY en CI).
# Scénarios, caches chauds : no-op ; warm (vendor supprimé) ; dump-autoload -o.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_BENCH_DIR:-/tmp/vivace-bench}"
RUNS="${BENCH_RUNS:-10}"
C="composer --no-interaction --no-plugins --no-scripts"
mkdir -p "$WORK"
out="| fixture | scénario | Composer | vivace | gain |\n|---|---|---|---|---|\n"
median_ms() { jq -r ".results[] | select(.command|test(\"$2\")) | (.median*1000|round)" "$1" | head -1; }
for fx in laravel symfony sylius; do
  d="$WORK/$fx"; rm -rf "$d"; mkdir -p "$d"
  (cd "$ROOT/fixtures/work/$fx" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var -cf - .) | (cd "$d" && tar -xf -)
  cd "$d"
  $C install --quiet; "$VIVACE" install --offline 2>/dev/null   # chauffe caches, store, classmap
  hyperfine -N --warmup 1 --min-runs "$RUNS" --export-json "$WORK/$fx.json" \
    --prepare true -n "composer-noop" "composer install --no-interaction --no-plugins --no-scripts" \
    --prepare true -n "vivace-noop" "$VIVACE install --offline" \
    --prepare "rm -rf vendor" -n "composer-warm" "composer install --no-interaction --no-plugins --no-scripts" \
    --prepare "rm -rf vendor" -n "vivace-warm" "$VIVACE install --offline" \
    --prepare true -n "composer-dump-o" "composer dump-autoload -o --no-plugins --no-scripts --quiet" \
    --prepare true -n "vivace-dump-o" "$VIVACE dump-autoload -o" >/dev/null 2>&1
  for sc in noop warm dump-o; do
    c=$(median_ms "$WORK/$fx.json" "^composer-$sc\$|composer-$sc "); v=$(median_ms "$WORK/$fx.json" "vivace-$sc")
    c=$(jq -r ".results[] | select(.command|startswith(\"composer\")) | select(.command|test(\"$( [ $sc = noop ] && echo 'install --no-interaction --no-plugins --no-scripts$' || ([ $sc = warm ] && echo 'install --no-interaction --no-plugins --no-scripts$' || echo 'dump-autoload') )\")) | (.median*1000|round)" "$WORK/$fx.json" | sed -n "$([ $sc = warm ] && echo 2 || echo 1)p")
    v=$(jq -r ".results[] | select(.command|startswith(\"$VIVACE\")) | select(.command|test(\"$([ $sc = dump-o ] && echo 'dump-autoload' || echo 'install --offline$')\")) | (.median*1000|round)" "$WORK/$fx.json" | sed -n "$([ $sc = warm ] && echo 2 || echo 1)p")
    gain=$(awk -v c="$c" -v v="$v" 'BEGIN{ if (v>0) printf "%.1f×", c/v; else print "?" }')
    out+="| $fx | $sc | ${c} ms | ${v} ms | $gain |\n"
  done
done
printf "$out"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && { echo "## vivace vs Composer ($(uname -s) $(uname -m), $(nproc 2>/dev/null || sysctl -n hw.ncpu) cœurs)"; printf "$out"; } >> "$GITHUB_STEP_SUMMARY" || true
