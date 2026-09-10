#!/usr/bin/env bash
# Chaque fixture, installée par vivace seul (install complet, autoload
# compris), doit démarrer. Aucun appel à Composer ici.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}/boot"
boot_cmd() { # bash 3.2 (macOS) : pas de tableaux associatifs
  case "$1" in
    laravel) echo "php artisan --version" ;;
    symfony) echo "php bin/console --version" ;;
    sylius)  echo "php -d memory_limit=1G bin/console --version" ;;
    rector)  echo "php vendor/bin/phpstan --version" ;;
    # Classe d'un plugin posé hors vendor/ par l'émulation de composer/installers.
    wordpress) echo "php -r require\"vendor/autoload.php\";exit(class_exists(\"Roots\\\\Soil\\\\Options\")?0:1);" ;;
  esac
}
status=0
for fx in laravel symfony sylius rector wordpress; do
  src="$ROOT/fixtures/work/$fx"; dst="$WORK/$fx"
  rm -rf "$dst"; mkdir -p "$dst"
  (cd "$src" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var -cf - .) | (cd "$dst" && tar -xf -)
  if ! (cd "$dst" && "$VIVACE" install --offline 2>"$WORK/$fx.vivace.log"); then
    echo "FAIL $fx : vivace install a échoué :"; tail -20 "$WORK/$fx.vivace.log"; status=1; continue
  fi
  if out=$(cd "$dst" && $(boot_cmd "$fx") 2>&1); then
    echo "OK   $fx : $(echo "$out" | head -1)"
  else
    echo "FAIL $fx : $(echo "$out" | head -3)"; status=1
  fi
done
exit $status
