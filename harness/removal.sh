#!/usr/bin/env bash
# Suppression de paquets : après un install complet, des paquets disparaissent
# du lock (un dans vendor/, un posé hors vendor/ par composer/installers) et
# le second install doit laisser le même projet que Composer — répertoires
# retirés, parents vides nettoyés, installed.json/installed.php et autoload
# réécrits. Chaque côté part de son propre premier install.
#
# Usage : harness/removal.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}/removal"
[ -x "$VIVACE" ] || { echo "binaire absent : cargo build --release"; exit 1; }

# fixture → paquets retirés (feuilles, pour que le lock reste cohérent)
removed_for() {
  case "$1" in
    wordpress) echo "wpackagist-plugin/hello-dolly monolog/monolog" ;;
    laravel)   echo "fakerphp/faker" ;;
  esac
}

drop_packages() { # project-dir names... — retirés du lock ET du require de
  # composer.json (Composer refuse un lock où manque un paquet requis).
  local dir="$1"; shift
  local lock_filter='.' json_filter='.'
  for n in "$@"; do
    lock_filter="$lock_filter | .packages |= map(select(.name != \"$n\")) | .[\"packages-dev\"] |= map(select(.name != \"$n\"))"
    json_filter="$json_filter | del(.require[\"$n\"]) | del(.[\"require-dev\"][\"$n\"])"
  done
  jq "$lock_filter" "$dir/composer.lock" > "$dir/composer.lock.tmp" && mv "$dir/composer.lock.tmp" "$dir/composer.lock"
  jq "$json_filter" "$dir/composer.json" > "$dir/composer.json.tmp" && mv "$dir/composer.json.tmp" "$dir/composer.json"
}

status=0
for fx in wordpress laravel; do
  src="$ROOT/fixtures/work/$fx"
  ref="$WORK/ref-$fx"; viv="$WORK/viv-$fx"
  rm -rf "$ref" "$viv"; mkdir -p "$ref" "$viv"
  for d in "$ref" "$viv"; do
    (cd "$src" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var -cf - .) | (cd "$d" && tar -xf -)
    (cd "$d" && git init -q -b main && git add -A >/dev/null && \
      GIT_AUTHOR_NAME=vivace GIT_AUTHOR_EMAIL=v@v GIT_AUTHOR_DATE="2026-09-10T00:00:00Z" \
      GIT_COMMITTER_NAME=vivace GIT_COMMITTER_EMAIL=v@v GIT_COMMITTER_DATE="2026-09-10T00:00:00Z" \
      git commit -q -m fixture)
  done
  if jq -e '.config["allow-plugins"]["composer/installers"] == true' "$src/composer.json" >/dev/null 2>&1; then
    plugin_flag=""
  else
    plugin_flag="--no-plugins"
  fi
  # Premier install de chaque côté.
  (cd "$ref" && composer install --no-interaction $plugin_flag --no-scripts --quiet)
  (cd "$viv" && "$VIVACE" install --offline 2>"$WORK/$fx.1.log") || { echo "FAIL $fx : premier install"; tail -5 "$WORK/$fx.1.log"; status=1; continue; }
  # Le lock perd des paquets, puis second install (Composer avertit que le
  # lock n'est plus à jour : c'est attendu des deux côtés).
  # shellcheck disable=SC2046
  drop_packages "$ref" $(removed_for "$fx")
  cp "$ref/composer.lock" "$viv/composer.lock"
  cp "$ref/composer.json" "$viv/composer.json"
  (cd "$ref" && composer install --no-interaction $plugin_flag --no-scripts --quiet 2>/dev/null)
  (cd "$viv" && "$VIVACE" install --offline 2>"$WORK/$fx.2.log") || { echo "FAIL $fx : second install"; tail -5 "$WORK/$fx.2.log"; status=1; continue; }
  lines=$(diff -r --exclude=.git "$ref" "$viv" 2>&1 \
    | grep -v 'autoload_runtime.php' \
    | grep -v 'No such file or directory' \
    | wc -l | tr -d ' ' || true)
  if [ "$lines" = "0" ]; then
    echo "OK   $fx : projet identique après suppression de $(removed_for "$fx")"
  else
    echo "FAIL $fx : $lines lignes de diff après suppression"
    diff -r --exclude=.git "$ref" "$viv" 2>&1 | grep -v autoload_runtime.php | head -20
    status=1
  fi
done
exit $status
