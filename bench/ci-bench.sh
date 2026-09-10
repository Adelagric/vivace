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
    c=$(jq -r ".results[] | select(.command==\"composer-$sc\") | (.median*1000|round)" "$WORK/$fx.json")
    v=$(jq -r ".results[] | select(.command==\"vivace-$sc\") | (.median*1000|round)" "$WORK/$fx.json")
    gain=$(awk -v c="$c" -v v="$v" 'BEGIN{ if (v>0) printf "%.1f×", c/v; else print "?" }')
    out+="| $fx | $sc | ${c} ms | ${v} ms | $gain |\n"
  done
done
printf "$out"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && { echo "## vivace vs Composer ($(uname -s) $(uname -m), $(nproc 2>/dev/null || sysctl -n hw.ncpu) cœurs)"; printf "$out"; } >> "$GITHUB_STEP_SUMMARY" || true
