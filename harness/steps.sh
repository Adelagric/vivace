#!/usr/bin/env bash
# Harness à étapes : une commande qui édite composer.json puis met à jour le
# lock (`remove`, bientôt `require`) jouée par Composer et par vivace depuis
# la même copie d'une fixture, sur le même instantané Packagist figé. Les
# deux composer.json, les deux composer.lock et les codes retour doivent
# coïncider.
#
# Usage : harness/steps.sh [fixture...]
set -euo pipefail
export COMPOSER_NO_BLOCKING=1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}/steps"
FIXTURES=("$@"); [ ${#FIXTURES[@]} -eq 0 ] && FIXTURES=(laravel symfony sylius rector drupal)
# "fixture|arguments" : la commande et ses arguments. Les mots finaux en
# `@…` préparent la copie avant l'étape (et ne sont pas passés) :
#   @nolock                 pas de composer.lock
#   @badlock                composer.lock sans clé `packages` (isLocked faux)
#   @drop:a/b               retire a/b de require (jq)
#   @require:a/b=^9         pose la contrainte dans require (jq)
#   @stub:a/b               fichier de métadonnées vide pour a/b dans l'instantané
#   @installed:a/b          vendor/composer/installed.json avec a/b et son répertoire
#   @installed-nodir:a/b    idem sans le répertoire (purgé par Composer)
#   @global-allow:a/b       config.allow-plugins {a/b: true} dans le config.json global
STEPS=(
  "laravel|remove laravel/tinker"
  "laravel|remove laravel/tinker @nolock"
  "laravel|remove laravel/tinker @badlock"
  "laravel|remove laravel/tinker @installed:laravel/tinker"
  "laravel|remove laravel/tinker @installed-nodir:laravel/tinker"
  "laravel|remove --unused @drop:laravel/tinker"
  "laravel|remove --unused laravel/pint --dev @drop:laravel/tinker"
  "laravel|remove laravel/pint --dev @require:symfony/console=^99 @stub:symfony/console"
  "laravel|remove laravel/tinker -W"
  "laravel|remove laravel/tinker --no-update-with-dependencies"
  "laravel|remove laravel/tinker --no-update"
  "laravel|remove Laravel/Tinker"
  "laravel|remove laravel/pint --dev"
  "laravel|remove laravel/pint"
  "laravel|remove laravel/*"
  "laravel|remove laravel/* --dev"
  "laravel|remove phpunit/phpunit --dev -W"
  "laravel|remove nonexistent/package"
  "laravel|remove --unused"
  "symfony|remove symfony/console"
  "symfony|remove symfony/flex"
  "symfony|remove symfony/runtime --no-update"
  "symfony|remove phpstan/* --dev"
  "symfony|remove symfony/yaml symfony/string -W"
  "symfony|remove twig/* --no-update-with-dependencies"
  "symfony|remove --unused"
  "sylius|remove sylius/paypal-plugin"
  "sylius|remove symfony/flex symfony/runtime"
  "sylius|remove phpstan/extension-installer --dev"
  "rector|remove rector/extension-installer phpstan/extension-installer"
  "rector|remove rector/extension-installer phpstan/extension-installer @global-allow:other/plugin"
  "rector|remove symfony/process --dev --no-update-with-dependencies"
  "drupal|remove drupal/core-project-message"
)
[ -x "$VIVACE" ] || { echo "binaire absent : cargo build --release"; exit 1; }
mkdir -p "$WORK"
status=0
for fx in "${FIXTURES[@]}"; do
  archive="$ROOT/fixtures/registry/$fx.tar.gz"
  [ -f "$archive" ] || { echo "SKIP $fx : pas d'instantané (tools/snapshot-packagist.sh $fx)"; continue; }
  reg="$WORK/registry-$fx"; rm -rf "$reg"; mkdir -p "$reg"
  tar -C "$reg" -xzf "$archive"
  printf '{"packages": [], "notify-batch": "https://packagist.org/downloads/", "metadata-url": "file://%s/p2/%%package%%.json"}\n' "$reg" > "$reg/packages.json"
  home="$WORK/home-$fx"; rm -rf "$home"; mkdir -p "$home"
  printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}}\n' "$reg" > "$home/config.json"
  root_version=""; [ "$fx" = "rector" ] && root_version="dev-main"
  n=0
  for spec in "${STEPS[@]}"; do
    [ "${spec%%|*}" = "$fx" ] || continue
    n=$((n + 1))
    read -r -a sargs <<< "${spec#*|}"
    preps=(); stubs=()
    while [ ${#sargs[@]} -gt 0 ]; do
      last=$(( ${#sargs[@]} - 1 ))
      case "${sargs[$last]}" in
        @*) preps+=("${sargs[$last]}"); unset "sargs[$last]" ;;
        *) break ;;
      esac
    done
    printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}}\n' "$reg" > "$home/config.json"
    # Préparations partagées (instantané, config globale) : une fois.
    for prep in "${preps[@]+"${preps[@]}"}"; do
      case "$prep" in
        @stub:*) p="${prep#@stub:}"; mkdir -p "$reg/p2/${p%%/*}"
          for f in "$reg/p2/$p.json" "$reg/p2/$p~dev.json"; do
            [ -f "$f" ] && mv "$f" "$f.orig"; printf '{"packages": {"%s": []}}' "$p" > "$f"; stubs+=("$f")
          done ;;
        @global-allow:*) printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}, "config": {"allow-plugins": {"%s": true}}}\n' "$reg" "${prep#@global-allow:}" > "$home/config.json" ;;
      esac
    done
    for side in ref viv; do
      d="$WORK/$side-$fx-$n"; rm -rf "$d"; mkdir -p "$d"
      cp "$ROOT/fixtures/projects/$fx/composer.json" "$ROOT/fixtures/projects/$fx/composer.lock" "$d/"
      for prep in "${preps[@]+"${preps[@]}"}"; do
        case "$prep" in
          @stub:*|@global-allow:*) ;;
          @nolock) rm -f "$d/composer.lock" ;;
          @badlock) printf '{"_readme": [], "content-hash": "x"}\n' > "$d/composer.lock" ;;
          @drop:*) jq --arg p "${prep#@drop:}" 'del(.require[$p])' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @require:*) kv="${prep#@require:}"; jq --arg p "${kv%%=*}" --arg c "${kv#*=}" '.require[$p] = $c' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @installed:*|@installed-nodir:*) p="${prep#*:}"; mkdir -p "$d/vendor/composer"
            printf '{"packages": [{"name": "%s", "version": "1.0.0", "version_normalized": "1.0.0.0", "type": "library", "install-path": "../%s"}], "dev": true, "dev-package-names": []}\n' "$p" "$p" > "$d/vendor/composer/installed.json"
            [ "${prep%%:*}" = "@installed-nodir" ] || mkdir -p "$d/vendor/$p" ;;
          *) echo "prep inconnu : $prep"; exit 1 ;;
        esac
      done
    done
    ref_code=0
    (cd "$WORK/ref-$fx-$n" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" \
      composer "${sargs[@]}" --no-install --no-scripts --no-plugins --no-interaction --no-audit --quiet >"$WORK/$fx-$n.composer.log" 2>&1) || ref_code=$?
    viv_code=0
    (cd "$WORK/viv-$fx-$n" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" \
      "$VIVACE" "${sargs[@]}" --no-install >"$WORK/$fx-$n.vivace.log" 2>&1) || viv_code=$?
    # Les métadonnées remplacées par @stub sont rendues à l'instantané.
    for f in "${stubs[@]+"${stubs[@]}"}"; do rm -f "$f"; [ -f "$f.orig" ] && mv "$f.orig" "$f"; done
    label="$fx ${sargs[*]}"; [ ${#preps[@]} -gt 0 ] && label="$label (${preps[*]})"
    if [ "$ref_code" != "$viv_code" ]; then
      echo "FAIL $label : code retour composer=$ref_code vivace=$viv_code"
      tail -3 "$WORK/$fx-$n.composer.log" "$WORK/$fx-$n.vivace.log"; status=1; continue
    fi
    ok=1
    for f in composer.json composer.lock; do
      if ! diff -q "$WORK/ref-$fx-$n/$f" "$WORK/viv-$fx-$n/$f" >/dev/null; then
        echo "FAIL $label : $f diffère"
        diff "$WORK/ref-$fx-$n/$f" "$WORK/viv-$fx-$n/$f" | head -20; ok=0
      fi
    done
    if [ "$ok" = 1 ]; then
      changed=""
      cmp -s "$ROOT/fixtures/projects/$fx/composer.json" "$WORK/ref-$fx-$n/composer.json" || changed="json"
      [ -f "$WORK/ref-$fx-$n/composer.lock" ] && { cmp -s "$ROOT/fixtures/projects/$fx/composer.lock" "$WORK/ref-$fx-$n/composer.lock" || changed="$changed lock"; } || true
      echo "OK   $label : identiques (code $ref_code, modifié : ${changed:-rien})"
    else
      status=1
    fi
  done
done
exit $status
