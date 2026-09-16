#!/usr/bin/env bash
# Harness des dépôts `path` : la fixture path-repos est matérialisée des deux
# côtés (fixtures/path-repos.sh : dépôts git imbriqués, liens, modes, dépôt
# du projet sur `develop`), puis la même séquence est jouée par Composer et
# par vivacity — install (liens symboliques), install à vide, édition des
# composer.json de deux paquets puis update (« Source already present » sur
# un paquet symlinké, re-miroir sur un paquet en miroir), remove, require de
# la branche parente ; sur des copies neuves, install en miroir
# (COMPOSER_MIRROR_PATH_REPOS=1) puis install sans l'option (rien à faire :
# la stratégie ne fait pas partie de l'identité).
#
# À chaque étape : codes retour, stderr de la première ligne d'en-tête à la
# fin (les lignes `vivacity: …` de résumé retirées), composer.json et
# composer.lock, vendor/ comparé par `diff -r --no-dereference` ET par un
# inventaire `stat` (modes, cibles des liens) — `diff -r` ne voit ni les
# modes ni un `.git` copié à tort, et le miroir est précisément la règle qui
# les décide. Les sources des paquets doivent rester intactes.
#
# Usage : harness/path-repos.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/fixture.sh
. "$ROOT/harness/lib/fixture.sh"
VIVACITY="$ROOT/target/release/vivacity"
WORK="${VIVACITY_HARNESS_DIR:-/tmp/vivacity-harness}/path-repos"
[ -x "$VIVACITY" ] || { echo "binaire absent : cargo build --release"; exit 1; }
mkdir -p "$WORK"
harness_git_env "$WORK"
home="$WORK/home"; rm -rf "$home"; mkdir -p "$home"
printf '{"repositories": {"packagist.org": false}}\n' > "$home/config.json"
export COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_TESTS_ARE_RUNNING=1
status=0
anchor='^(Installing dependencies from lock file|Lock file operations|Nothing to modify in lock file|Nothing to install, update or remove|Your requirements could not be resolved)'

# Modes et cibles des liens (un lien absolu est rapporté relativement au
# projet, physique ou non).
inventory() { # vendor dir, project dir
  local real; real="$(cd "$2" && pwd -P)"
  (cd "$1" && find . -print | LC_ALL=C sort | while IFS= read -r f; do
    if [ -L "$f" ]; then echo "L $f -> $(readlink "$f" | sed -e "s|^$real/|<project>/|" -e "s|^$2/|<project>/|")"
    elif [ "$(uname -s)" = Darwin ]; then echo "$(stat -f '%Sp' "$f") $f"
    else echo "$(stat -c '%A' "$f") $f"; fi
  done)
}

mtime() { if [ "$(uname -s)" = Darwin ]; then stat -f '%m' "$1"; else stat -c '%Y' "$1"; fi; }

# Un fichier d'un paquet en miroir porte le mtime (à la seconde) de sa
# source (Symfony `copy` : `touch($target, filemtime($origin))`) — un
# invariant par côté, puisque les deux copies de la fixture n'ont pas le
# même mtime de source.
check_mirror_mtimes() { # project dir
  local proj="$1" name url pkgdir f src ok=1
  while IFS=$'\t' read -r name url; do
    pkgdir="$proj/vendor/$name"
    [ -d "$pkgdir" ] && [ ! -L "$pkgdir" ] || continue
    while IFS= read -r f; do
      src="$proj/$url/${f#"$pkgdir/"}"
      [ "$(mtime "$f")" = "$(mtime "$src")" ] || { echo "  mtime $f ≠ $src"; ok=0; }
    done < <(find "$pkgdir" -type f)
  done < <(jq -r '.packages[] | select(.dist.type == "path") | [.name, .dist.url] | @tsv' "$proj/vendor/composer/installed.json")
  [ "$ok" = 1 ]
}

# step <label> <composer args...> — joue la commande des deux côtés dans
# $ref et $viv (variables globales) et compare.
step() {
  local label="$1"; shift
  local ref_code=0 viv_code=0 ok=1
  (cd "$ref" && composer "$@" --no-scripts --no-plugins --no-interaction --no-ansi >"$WORK/$label.composer.log" 2>"$WORK/$label.composer.err") || ref_code=$?
  (cd "$viv" && "$VIVACITY" "$@" >"$WORK/$label.vivacity.log" 2>"$WORK/$label.vivacity.err") || viv_code=$?
  if [ "$ref_code" != "$viv_code" ]; then
    echo "FAIL path-repos $label : code retour composer=$ref_code vivacity=$viv_code"
    tail -3 "$WORK/$label.composer.err" "$WORK/$label.vivacity.err"; status=1; return
  fi
  sed -E -n "/$anchor/,\$p" "$WORK/$label.composer.err" > "$WORK/$label.composer.tail"
  sed -E -n "/$anchor/,\$p" "$WORK/$label.vivacity.err" | grep -v '^vivacity: ' > "$WORK/$label.vivacity.tail" || true
  if ! [ -s "$WORK/$label.composer.tail" ]; then
    echo "FAIL path-repos $label : pas de ligne d'ancrage dans la sortie de Composer"; ok=0
  elif ! diff -q "$WORK/$label.composer.tail" "$WORK/$label.vivacity.tail" >/dev/null; then
    echo "FAIL path-repos $label : la sortie diffère"
    diff "$WORK/$label.composer.tail" "$WORK/$label.vivacity.tail" | head -20 || true; ok=0
  fi
  for f in composer.json composer.lock; do
    if ! diff -q "$ref/$f" "$viv/$f" >/dev/null 2>&1; then
      echo "FAIL path-repos $label : $f diffère"; diff "$ref/$f" "$viv/$f" | head -10 || true; ok=0
    fi
  done
  # Les cibles des liens sont comparées par l'inventaire (un lien absolu
  # contient le nom du côté) : `diff -r` compare tout le reste.
  diff -r --no-dereference "$ref/vendor" "$viv/vendor" 2>&1 | grep -v '^Symbolic links .* differ$' >"$WORK/$label.vendor.diff" || true
  if [ -s "$WORK/$label.vendor.diff" ]; then
    echo "FAIL path-repos $label : vendor/ diffère"; head -20 "$WORK/$label.vendor.diff"; ok=0
  fi
  inventory "$ref/vendor" "$ref" > "$WORK/$label.ref.inv"; inventory "$viv/vendor" "$viv" > "$WORK/$label.viv.inv"
  if ! diff -q "$WORK/$label.ref.inv" "$WORK/$label.viv.inv" >/dev/null; then
    echo "FAIL path-repos $label : l'inventaire de vendor/ (modes, liens) diffère"
    diff "$WORK/$label.ref.inv" "$WORK/$label.viv.inv" | head -20 || true; ok=0
  fi
  for side in "$ref" "$viv"; do
    check_mirror_mtimes "$side" || { echo "FAIL path-repos $label : mtime d'un fichier en miroir ≠ source ($side)"; ok=0; }
  done
  # Les sources ne bougent jamais (un remove délie, ne supprime pas).
  for side in "$ref" "$viv"; do
    for src in packages/alpha packages/delta packages/gamma libs/beta libs/epsilon src-zeta; do
      [ -f "$side/$src/composer.json" ] || { echo "FAIL path-repos $label : source $src touchée ($side)"; ok=0; }
    done
  done
  if [ "$ok" = 1 ]; then
    echo "OK   path-repos $label : identiques (code $ref_code, $(grep -c '^  - ' "$WORK/$label.composer.tail" || true) opérations, $(wc -l < "$WORK/$label.ref.inv" | tr -d ' ') entrées)"
  else
    status=1
  fi
}

edit_package() { # dir…
  for d in "$@"; do
    for side in "$ref" "$viv"; do
      jq '.description = "edited"' "$side/$d/composer.json" > "$side/$d/c.tmp" && mv "$side/$d/c.tmp" "$side/$d/composer.json"
    done
  done
}

ref="$WORK/ref"; viv="$WORK/viv"
stage_project path-repos "$ref"; stage_project path-repos "$viv"
step "install" install
step "install-again" install
edit_package packages/alpha libs/beta
step "update-after-edit" update --no-audit
step "remove-alpha" remove acme/alpha --no-audit
step "require-gamma-main" require acme/gamma:dev-main --no-audit
# Les options du dépôt changent : la référence des paquets aussi, et la
# mise à jour remplace les liens par des miroirs (`symlink: false`), puis
# les miroirs par des liens absolus (`relative: false`).
for side in "$ref" "$viv"; do
  jq '.repositories[0].options = {"symlink": false}' "$side/composer.json" > "$side/c.tmp" && mv "$side/c.tmp" "$side/composer.json"
done
step "update-links-to-mirrors" update --no-audit
for side in "$ref" "$viv"; do
  jq '.repositories[0].options = {"relative": false}' "$side/composer.json" > "$side/c.tmp" && mv "$side/c.tmp" "$side/composer.json"
done
step "update-mirrors-to-absolute-links" update --no-audit
# Un lien supprimé à la main : Composer purge l'entrée d'installed.json et
# réinstalle sans créer vendor/bin.
rm "$ref/vendor/acme/delta" "$viv/vendor/acme/delta"
step "install-after-deleted-link" install
# Copies neuves : stratégie miroir imposée par l'environnement, puis retour
# à l'option par défaut sur un vendor/ déjà en miroir.
stage_project path-repos "$ref"; stage_project path-repos "$viv"
export COMPOSER_MIRROR_PATH_REPOS=1
step "install-mirror" install
unset COMPOSER_MIRROR_PATH_REPOS
step "install-over-mirror" install
exit $status
