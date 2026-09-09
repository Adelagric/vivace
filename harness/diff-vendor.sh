#!/usr/bin/env bash
# Harness différentiel M2/M4 : pour chaque fixture, compare le vendor/ produit
# par `composer install --no-plugins --no-scripts [--no-autoloader]` et celui
# produit par `vivace install [--no-autoloader]` sur des copies nues.
#
# Écarts tolérés (documentés) :
#   - vendor/autoload_runtime.php : généré par vivace (émulation symfony/runtime),
#     absent d'un install Composer sans plugins — c'est la feature r2 ;
#   - "No such file or directory" : `diff -r` ne sait pas suivre un symlink
#     pendant (identique des deux côtés, vérifié via readlink).
#
# Usage : harness/diff-vendor.sh [--with-autoloader] [fixture...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}"
AUTOLOAD_FLAG="--no-autoloader"
if [ "${1:-}" = "--with-autoloader" ]; then AUTOLOAD_FLAG=""; shift; fi
FIXTURES=("$@"); [ ${#FIXTURES[@]} -eq 0 ] && FIXTURES=(laravel symfony sylius)

[ -x "$VIVACE" ] || { echo "binaire absent : cargo build --release"; exit 1; }
mkdir -p "$WORK"
status=0
for fx in "${FIXTURES[@]}"; do
  src="$ROOT/fixtures/work/$fx"
  ref="$WORK/ref-$fx"; viv="$WORK/viv-$fx"
  rm -rf "$ref" "$viv"; mkdir -p "$ref" "$viv"
  # Projet complet (sans vendor/node_modules) : les règles d'autoload de la
  # racine (classmap src/Kernel.php, psr-4 app/…) doivent exister.
  (cd "$src" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var -cf - .) | (cd "$ref" && tar -xf -)
  (cd "$src" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var -cf - .) | (cd "$viv" && tar -xf -)
  (cd "$ref" && composer install --no-interaction --no-plugins --no-scripts $AUTOLOAD_FLAG --quiet)
  (cd "$viv" && "$VIVACE" install $AUTOLOAD_FLAG --offline 2>/dev/null)
  lines=$(diff -r "$ref/vendor" "$viv/vendor" 2>&1 \
    | grep -v 'autoload_runtime.php' \
    | grep -v 'No such file or directory' \
    | wc -l | tr -d ' ' || true)   # grep -v renvoie 1 sur diff vide : c'est le succès
  if [ "$lines" = "0" ]; then
    echo "OK   $fx : vendor/ identique"
  else
    echo "FAIL $fx : $lines lignes de diff"
    diff -r "$ref/vendor" "$viv/vendor" 2>&1 | grep -v autoload_runtime.php | head -20
    status=1
  fi
done
exit $status
