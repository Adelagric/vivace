#!/usr/bin/env bash
# Harness du résolveur : `composer update --no-install` et `vivace update
# --no-install` sur le même instantané Packagist figé (fixtures/registry),
# composer.json intact — le lock produit doit être identique à l'octet, et
# identique au lock de référence capturé avec l'instantané (déterminisme de
# Composer lui-même, vérifié à chaque run).
#
# Le dépôt local est injecté par la configuration globale de Composer
# (COMPOSER_HOME/config.json : repositories + packagist.org: false), que les
# deux outils doivent honorer.
#
# Usage : harness/update.sh [fixture...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}/update"
FIXTURES=("$@"); [ ${#FIXTURES[@]} -eq 0 ] && FIXTURES=(laravel symfony sylius rector drupal)
[ -x "$VIVACE" ] || { echo "binaire absent : cargo build --release"; exit 1; }
mkdir -p "$WORK"
status=0
for fx in "${FIXTURES[@]}"; do
  archive="$ROOT/fixtures/registry/$fx.tar.gz"
  [ -f "$archive" ] || { echo "SKIP $fx : pas d'instantané (tools/snapshot-packagist.sh $fx)"; continue; }
  reg="$WORK/registry-$fx"; rm -rf "$reg"; mkdir -p "$reg"
  tar -C "$reg" -xzf "$archive"
  # packages.json avec l'URL absolue : Composer résout un metadata-url relatif
  # contre la racine du système de fichiers, pas contre le dépôt.
  printf '{"packages": [], "notify-batch": "https://packagist.org/downloads/", "metadata-url": "file://%s/p2/%%package%%.json"}\n' "$reg" > "$reg/packages.json"
  home="$WORK/home-$fx"; rm -rf "$home"; mkdir -p "$home"
  printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}}\n' "$reg" > "$home/config.json"
  root_version=""; [ "$fx" = "rector" ] && root_version="dev-main"
  for side in ref viv; do
    d="$WORK/$side-$fx"; rm -rf "$d"; mkdir -p "$d"
    cp "$ROOT/fixtures/projects/$fx/composer.json" "$d/"
    [ -f "$ROOT/fixtures/projects/$fx/composer.lock" ] && cp "$ROOT/fixtures/projects/$fx/composer.lock" "$d/"
  done
  if ! (cd "$WORK/ref-$fx" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" \
        composer update --no-install --no-scripts --no-plugins --no-interaction --no-audit --quiet 2>"$WORK/$fx.composer.log"); then
    echo "FAIL $fx : composer update a échoué :"; tail -5 "$WORK/$fx.composer.log"; status=1; continue
  fi
  if ! diff -q "$WORK/ref-$fx/composer.lock" "$reg/composer.lock.expected" >/dev/null; then
    echo "FAIL $fx : composer update sur l'instantané ≠ lock de référence (Composer non déterministe ?)"
    diff "$WORK/ref-$fx/composer.lock" "$reg/composer.lock.expected" | head -10; status=1; continue
  fi
  if ! (cd "$WORK/viv-$fx" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" \
        "$VIVACE" update --no-install 2>"$WORK/$fx.vivace.log"); then
    echo "FAIL $fx : vivace update a échoué :"; tail -5 "$WORK/$fx.vivace.log"; status=1; continue
  fi
  if diff -q "$WORK/ref-$fx/composer.lock" "$WORK/viv-$fx/composer.lock" >/dev/null; then
    echo "OK   $fx : composer.lock identique ($(jq '.packages | length' "$WORK/viv-$fx/composer.lock") paquets)"
  else
    echo "FAIL $fx : composer.lock diffère"
    diff "$WORK/ref-$fx/composer.lock" "$WORK/viv-$fx/composer.lock" | head -20; status=1
  fi
done
exit $status
