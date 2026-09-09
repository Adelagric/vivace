#!/usr/bin/env bash
# M0 — comparaison plafond : spike Rust (fetch cache chaud + extraction) vs
# `composer install --no-autoloader` cache chaud (le périmètre le plus proche).
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$DIR/bench/results"
SPIKE="$DIR/target/release/spike"
mkdir -p "$OUT"
FLAGS="--no-interaction --no-plugins --no-scripts"

for fx in laravel symfony sylius; do
  FX="$DIR/fixtures/work/$fx"
  cd "$FX"
  composer install $FLAGS --quiet   # garantit le cache chaud
  hyperfine --warmup 1 --min-runs 10 --export-json "$OUT/$fx-spike.json" \
    -n "composer-warm-noAL" --prepare "rm -rf vendor"       "composer install $FLAGS --no-autoloader" \
    -n "spike-rust"         --prepare "rm -rf vendor-spike" "$SPIKE . --offline"
done
jq -r '.results[] | "\(.command)\t\(.median)"' "$OUT"/*-spike.json
