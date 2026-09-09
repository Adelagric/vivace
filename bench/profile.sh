#!/usr/bin/env bash
# M0 — profil de décomposition du temps de `composer install` par fixture.
# Scénarios :
#   bootstrap : composer --version                       (coût fixe du runtime)
#   noop      : install avec vendor déjà présent          (scénario idempotence)
#   warm      : install cache chaud, vendor supprimé      (scénario CI avec cache)
#   warm-noAL : idem sans dump d'autoload                 (warm - warm-noAL = coût autoload)
#   dump-o    : dump-autoload -o seul, vendor présent     (coût classmap optimisée)
#   cold      : install cache vide (3 runs seulement, tape le réseau réel)
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$DIR/bench/results"
mkdir -p "$OUT"
FLAGS="--no-interaction --no-plugins --no-scripts"

for fx in laravel symfony sylius; do
  FX="$DIR/fixtures/work/$fx"
  [ -d "$FX" ] || { echo "fixture $fx absente, lancer fixtures/make.sh"; exit 1; }
  cd "$FX"
  echo "==== $fx ===="
  composer install $FLAGS --quiet   # état de départ propre + cache chaud garanti

  hyperfine --warmup 1 --min-runs 10 --export-json "$OUT/$fx.json" \
    --prepare true -n bootstrap "composer --version" \
    --prepare true -n noop      "composer install $FLAGS" \
    --prepare "rm -rf vendor" -n warm      "composer install $FLAGS" \
    --prepare "rm -rf vendor" -n warm-noAL "composer install $FLAGS --no-autoloader" \
    --prepare true -n dump-o    "composer dump-autoload -o --quiet"

  COLD_CACHE="$(mktemp -d)"
  hyperfine --min-runs 3 --export-json "$OUT/$fx-cold.json" \
    -n cold --prepare "rm -rf vendor" \
    "COMPOSER_CACHE_DIR=$COLD_CACHE/\$RANDOM composer install $FLAGS"
  rm -rf "$COLD_CACHE"

  jq -r '.results[] | "\(.command)\t\(.median)"' "$OUT/$fx.json" || true
done
echo "Résultats JSON dans $OUT/"
