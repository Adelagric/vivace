#!/usr/bin/env bash
# Harness différentiel M2/M4 : pour chaque fixture, compare le vendor/ produit
# par `composer install --no-plugins --no-scripts [--no-autoloader]` et celui
# produit par `vivace install [--no-autoloader]` sur des copies nues.
#
# Écarts tolérés (documentés) :
#   - vendor/autoload_runtime.php : généré par vivace (émulation symfony/runtime),
#     absent d'un install Composer sans plugins — c'est la feature r2 ;
#   - "No such file or directory" : `diff -r` ne sait pas suivre un symlink
#     pendant (identique des deux côtés, vérifié via readlink) ;
#   - vendor/composer/include_paths.php (paquets PEAR à `include-path`) :
#     Composer l'écrit dans l'ordre d'achèvement des extractions asynchrones —
#     trois `composer install` successifs donnent trois ordres (vérifié le
#     2026-09-11 sur la fixture drupal) ; `composer dump-autoload` le réécrit
#     dans l'ordre d'installed.json, que vivace produit. Comparé trié.
#
# Usage : harness/diff-vendor.sh [--with-autoloader] [fixture...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}"
AUTOLOAD_FLAG="--no-autoloader"
if [ "${1:-}" = "--with-autoloader" ]; then AUTOLOAD_FLAG=""; shift; fi
FIXTURES=("$@"); [ ${#FIXTURES[@]} -eq 0 ] && FIXTURES=(laravel symfony sylius rector wordpress drupal)

[ -x "$VIVACE" ] || { echo "binaire absent : cargo build --release"; exit 1; }
mkdir -p "$WORK"
status=0
for fx in "${FIXTURES[@]}"; do
  src="$ROOT/fixtures/work/$fx"
  ref="$WORK/ref-$fx"; viv="$WORK/viv-$fx"
  rm -rf "$ref" "$viv"; mkdir -p "$ref" "$viv"
  # Projet complet sans les sorties d'un install (vendor/, node_modules, var,
  # et ce que les plugins émulés produisent : web/, wp-content/, recipes/, le
  # .editorconfig/.gitattributes scaffoldés) — sinon les fichiers copiés puis
  # committés par le harness seraient « trackés et inchangés » des deux côtés
  # et le scaffold n'aurait rien à prouver. Les règles d'autoload de la racine
  # (src/Kernel.php, app/…) restent présentes.
  (cd "$src" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var --exclude=./web --exclude=./wp-content --exclude=./recipes --exclude=./.editorconfig --exclude=./.gitattributes -cf - .) | (cd "$ref" && tar -xf -)
  (cd "$src" && tar --exclude=./vendor --exclude=./node_modules --exclude=./var --exclude=./web --exclude=./wp-content --exclude=./recipes --exclude=./.editorconfig --exclude=./.gitattributes -cf - .) | (cd "$viv" && tar -xf -)
  # Dépôt git identique des deux côtés (même arbre, même auteur/date → même SHA) :
  # Composer devine la version racine depuis git, vivace doit faire pareil.
  for d in "$ref" "$viv"; do
    (cd "$d" && git init -q -b main && git add -A >/dev/null && \
      GIT_AUTHOR_NAME=vivace GIT_AUTHOR_EMAIL=v@v GIT_AUTHOR_DATE="2026-09-10T00:00:00Z" \
      GIT_COMMITTER_NAME=vivace GIT_COMMITTER_EMAIL=v@v GIT_COMMITTER_DATE="2026-09-10T00:00:00Z" \
      git commit -q -m fixture)
  done
  # Fixture à plugin de layout émulé (composer/installers autorisé) : la
  # référence tourne AVEC le plugin et la comparaison couvre le projet entier,
  # puisque des paquets vivent hors vendor/.
  if jq -e '.config["allow-plugins"]["composer/installers"] == true' "$src/composer.json" >/dev/null 2>&1; then
    plugin_flag=""; scope_ref="$ref"; scope_viv="$viv"; what="projet"
  else
    plugin_flag="--no-plugins"; scope_ref="$ref/vendor"; scope_viv="$viv/vendor"; what="vendor/"
  fi
  (cd "$ref" && composer install --no-interaction $plugin_flag --no-scripts $AUTOLOAD_FLAG --quiet)
  if ! (cd "$viv" && "$VIVACE" install $AUTOLOAD_FLAG --offline 2>"$WORK/$fx.vivace.log"); then
    echo "FAIL $fx : vivace install a échoué :"; tail -20 "$WORK/$fx.vivace.log"; status=1; continue
  fi
  ip_ref="$ref/vendor/composer/include_paths.php"; ip_viv="$viv/vendor/composer/include_paths.php"
  if [ -f "$ip_ref" ] && [ -f "$ip_viv" ] && ! diff -q <(sort "$ip_ref") <(sort "$ip_viv") >/dev/null; then
    echo "FAIL $fx : include_paths.php diffère même trié"; diff <(sort "$ip_ref") <(sort "$ip_viv") | head; status=1; continue
  fi
  lines=$(diff -r --exclude=.git --exclude=include_paths.php "$scope_ref" "$scope_viv" 2>&1 \
    | grep -v 'vendor/autoload_runtime.php\|vendor: autoload_runtime.php' \
    | grep -v 'No such file or directory' \
    | wc -l | tr -d ' ' || true)   # grep -v renvoie 1 sur diff vide : c'est le succès
  if [ "$lines" = "0" ]; then
    echo "OK   $fx : $what identique"
  else
    echo "FAIL $fx : $lines lignes de diff"
    diff -r --exclude=.git --exclude=include_paths.php "$scope_ref" "$scope_viv" 2>&1 | grep -v 'vendor/autoload_runtime.php\|vendor: autoload_runtime.php' | head -20
    status=1
  fi
done
exit $status
