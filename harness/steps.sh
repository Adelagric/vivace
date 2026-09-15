#!/usr/bin/env bash
# Harness à étapes : une commande qui édite composer.json puis met à jour le
# lock (`remove`, bientôt `require`) jouée par Composer et par vivacity depuis
# la même copie d'une fixture, sur le même instantané Packagist figé. Les
# deux composer.json, les deux composer.lock et les codes retour doivent
# coïncider.
#
# Usage : harness/steps.sh [fixture...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=lib/registry.sh
. "$ROOT/harness/lib/registry.sh"
VIVACITY="$ROOT/target/release/vivacity"
WORK="${VIVACITY_HARNESS_DIR:-/tmp/vivacity-harness}/steps"
FIXTURES=("$@"); [ ${#FIXTURES[@]} -eq 0 ] && FIXTURES=(laravel symfony sylius rector drupal solver-policies solver-conflict solver-problems)
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
#   @global-sort            config.sort-packages true dans le config.json global
#   @emptyjson              composer.json vide (0 octet)
#   @nojson                 pas de composer.json
#   @notype                 clé `type` retirée du manifeste
#   @lockfalse              config.lock false dans le manifeste
#   @indent2                composer.lock réindenté à 2 espaces
#   @corruptlock            composer.lock illisible
#   @minstab:dev            minimum-stability dev + prefer-stable false
#   @platform:php=7.4.0     config.platform.php dans le manifeste
#   @lockfile:name          copie `name` (dans le répertoire de la fixture) sur composer.lock
#   @jq:expr                applique l'expression jq au manifeste (ex. .config.policy.abandoned.block=true)
#   @registry-jq:expr       applique l'expression jq au packages.json de l'instantané (le cas seulement)
#   @repofilter:json        redéclare le dépôt `snapshot` dans le manifeste avec cette option `filter`
#   @env:NAME=VALUE         variable d'environnement des deux côtés (le cas seulement)
#   @stderr                 compare aussi la sortie d'erreur à partir de la ligne
#                           « Your requirements could not be resolved… » (ou
#                           « Unable to find a compatible set… », « Your lock
#                           file does not contain… ») jusqu'à la fin, à l'octet.
#                           Composer tourne alors sans --quiet (les explications
#                           sont au niveau normal) et avec COMPOSER_TESTS_ARE_RUNNING
#                           (sinon `::error ::…` sur stdout sous GitHub Actions).
STEPS=(
  "laravel|remove laravel/tinker"
  "laravel|remove laravel/tinker @nolock"
  "laravel|remove laravel/tinker @badlock"
  "laravel|remove laravel/tinker @installed:laravel/tinker"
  "laravel|remove laravel/tinker @installed-nodir:laravel/tinker"
  "laravel|remove --unused @drop:laravel/tinker"
  "laravel|remove --unused laravel/pint --dev @drop:laravel/tinker"
  "laravel|remove laravel/pint --dev @require:symfony/console=^99 @stub:symfony/console"
  "laravel|remove laravel/pint --dev @require:symfony/console=^99 @stub:symfony/console @stderr"
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
  "laravel|require symfony/uid"
  "laravel|require symfony/uid --no-update"
  "laravel|require symfony/uid:^7.0"
  "laravel|require symfony/uid ^7.0 --dev"
  "laravel|require symfony/uid --dev --fixed"
  "laravel|require symfony/clock --fixed"
  "laravel|require laravel/pint"
  "laravel|require laravel/pint --dev"
  "laravel|require laravel/tinker:^2.9 -W"
  "laravel|require laravel/tinker"
  "laravel|require symfony/uid @nolock"
  "laravel|require symfony/uid @badlock"
  "laravel|require symfony/uid symfony/process:^7"
  "laravel|require symfony/uid --sort-packages @drop:laravel/tinker"
  "laravel|require symfony/uid --dev @require:symfony/console=^99 @stub:symfony/console"
  "laravel|require symfony/uid @emptyjson"
  "laravel|require symfony/uid @nojson"
  "laravel|require symfony/uid --no-update @nojson"
  "laravel|require symfony/uid --fixed @notype"
  "laravel|require symfony/uid @nolock @lockfalse"
  "laravel|require laravel/pint @lockfalse"
  "laravel|require symfony/uid @indent2"
  "laravel|require symfony/uid @corruptlock"
  "laravel|require ext-json @global-sort"
  "laravel|require ext-nonexistent"
  "laravel|require ext-nonexistent --ignore-platform-req=ext-nonexistent"
  "laravel|require symfony/clock @platform:php=7.4.0"
  "laravel|require symfony/uid symfony/uid:^7.1"
  "laravel|require symfony/uid:^7.1 symfony/uid"
  "laravel|require SYMFONY/UID"
  "rector|require symfony/finder @minstab:dev"
  "solver-policies|update @nolock"
  "solver-policies|update --no-blocking @nolock"
  "solver-policies|update --no-security-blocking @nolock"
  "solver-policies|update @nolock @jq:.config.policy=false"
  "solver-policies|update @nolock @require:acme/only-bad=^1"
  "solver-policies|update @nolock @require:acme/vuln=1.1.0"
  "solver-policies|update @nolock @require:acme/replacer=^2"
  "solver-policies|update @nolock @jq:.config.policy.abandoned.block=true"
  "solver-policies|update @nolock @jq:.config.audit[\"block-abandoned\"]=true"
  "solver-policies|update @nolock @jq:.config.policy.malware.ignore={\"acme/bad\":null}"
  "solver-policies|update @nolock @jq:.config.policy.malware[\"block-scope\"]=\"install\""
  "solver-policies|update @nolock @jq:.config.policy.malware[\"ignore-source\"]=[\"testsource\"]"
  "solver-policies|update @nolock @jq:.config.policy.malware=false"
  "solver-policies|update @nolock @jq:.config.policy.advisories[\"ignore-id\"]=[\"PKSA-test-vuln-0001\"]"
  "solver-policies|update @nolock @jq:.config.policy.advisories.ignore={\"acme/vuln\":\"known\"}"
  "solver-policies|update @nolock @jq:.config.audit.ignore=[\"CVE-2026-0001\"]"
  "solver-policies|update @nolock @jq:.config.audit.ignore=[\"acme/vuln\"]"
  "solver-policies|update @nolock @jq:.config.audit[\"block-insecure\"]=false"
  "solver-policies|update @nolock @jq:.config.policy.advisories[\"ignore-severity\"]=[\"high\"]"
  "solver-policies|update acme/vuln @lockfile:composer.lock.malware"
  "solver-policies|update acme/lib @lockfile:composer.lock.malware @jq:.config.policy.malware[\"block-scope\"]=\"update\""
  "solver-policies|update acme/lib @lockfile:composer.lock.malware @jq:.config.policy.malware[\"block-scope\"]=\"install\""
  "solver-policies|update acme/bad"
  "solver-policies|require acme/replacer"
  "solver-policies|remove acme/abandoned"
  "solver-policies|install @lockfile:composer.lock.malware"
  "solver-policies|install --no-blocking @lockfile:composer.lock.malware"
  "solver-policies|install @lockfile:composer.lock.malware @jq:.config.policy.malware[\"block-scope\"]=\"update\""
  "solver-policies|install @lockfile:composer.lock.malware @jq:.config.policy.malware.ignore={\"acme/bad\":{\"constraint\":\"1.1.0\"}}"
  "solver-policies|install"
  "solver-policies|install @lockfile:composer.lock.malware @jq:.repositories={\"dead\":{\"type\":\"composer\",\"url\":\"https://127.0.0.1:1\"}}"
  "solver-policies|install @lockfile:composer.lock.malware @jq:.config.policy.malware.ignore={\"acme/bad\":[]}"
  "solver-policies|install @lockfile:composer.lock.malware @jq:.config.policy.malware[\"block-scope\"]=\"install\""
  "solver-policies|install @lockfile:composer.lock.malware @repofilter:{\"malware\":\"x\"}"
  "solver-policies|install @lockfile:composer.lock.malware @repofilter:{\"malware\":false}"
  "solver-policies|install --no-install @lockfile:composer.lock.malware"
  "solver-policies|update @nolock @registry-jq:.filter.metadata=false"
  "solver-policies|update @nolock @require:acme/partial=^1"
  "solver-policies|update @nolock @require:acme/partial=^1 @jq:.config.policy.advisories.ignore={\"acme/partial\":null}"
  "solver-policies|update @nolock @require:acme/partial=^1 @jq:.config.policy.advisories[\"ignore-id\"]=[\"PKSA-test-partial-0001\"]"
  "solver-policies|update @nolock @jq:.config.policy.advisories.ignore={\"acme/vuln\":[]}"
  "solver-policies|update @nolock @jq:.config.audit[\"ignore-severity\"]={\"high\":{\"apply\":\"audit\"}}"
  "solver-policies|update @nolock @jq:.config.audit[\"ignore-severity\"]={\"high\":{\"apply\":\"block\"}}"
  "solver-policies|update @nolock @jq:.config.policy.foo=true"
  "solver-policies|update @nolock @jq:.config.policy.foo=false"
  "solver-policies|update @nolock @env:COMPOSER_AUDIT_ABANDONED=foo"
  "solver-policies|update @nolock @env:COMPOSER_POLICY=0"
  "solver-policies|update @nolock @jq:.config.policy=false @env:COMPOSER_POLICY=1"
  "solver-policies|update @nolock @env:COMPOSER_POLICY_ADVISORIES_BLOCK=0"
  "solver-policies|update @nolock @env:COMPOSER_POLICY_MALWARE_BLOCK=off"
  "solver-policies|update @nolock @env:COMPOSER_NO_BLOCKING=yes"
  "solver-policies|update @nolock @jq:.config.policy[\"ignore-unreachable\"]=false"
  "solver-policies|update @nolock @require:acme/replacer=^2 @jq:.require[\"acme/vuln\"]=\"1.1.0\""
  "rector|require nette/utils @minstab:dev"
  "symfony|require symfony/yaml"
  "symfony|require symfony/string --dev"
  "symfony|require twig/intl-extra"
  "sylius|require symfony/uid"
  "sylius|require symfony/uid -w"
  "rector|require phpstan/phpdoc-parser --dev"
  "rector|require nette/utils"
  "rector|require symfony/finder"
  "drupal|require drupal/core-project-message"
  "drupal|require composer/installers"
  "solver-conflict|update @nolock @stderr"
  "solver-conflict|update --no-dev @nolock @stderr"
  "solver-conflict|require psr/log:^1.0 @nolock @stderr"
  "rector|update @require:phpstan/phpstan=^99 @stderr"
  "rector|update phpstan/phpstan @require:phpstan/phpstan=^99 @stderr"
  "rector|require phpstan/phpstan:^99 @stderr"
  "solver-problems|update @nolock @stderr @require:acme/missing-dep=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/unstable=^1.0 @jq:.[\"minimum-stability\"]=\"stable\""
  "solver-problems|update @nolock @stderr @require:acme/unstable=^1.0@beta @jq:.[\"minimum-stability\"]=\"stable\""
  "solver-problems|update @nolock @stderr @require:acme/conflict-a=^1.0 @require:acme/conflict-b=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/replaces-a=^1.0 @require:acme/replaces-b=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/replaces-a=^1.0 @require:acme/shared=^1.0 @require:acme/replaces-b=*"
  "solver-problems|update @nolock @stderr @require:acme/aliased=1.x-dev"
  "solver-problems|update @nolock @stderr @jq:.require[\"acme/aliased\"]=\"dev-main\\u0020as\\u00201.0.0\""
  "solver-problems|update @nolock @stderr @require:acme/many=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/many=~1.3.0"
  "solver-problems|update @nolock @stderr @require:acme/nope=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/nope=1.2.3"
  "solver-problems|update @nolock @stderr @require:acme/nope=dev-main"
  "solver-problems|update @nolock @stderr @require:php=^99"
  "solver-problems|update @nolock @stderr @require:ext-nope=*"
  "solver-problems|update @nolock @stderr @require:ext-nope=* @require:acme/provides-ext=^1.0"
  "solver-problems|update @nolock @stderr @require:ext-nope=* @registry-jq:.[\"providers-api\"]=(.[\"metadata-url\"]|sub(\"p2/%package%.json\";\"providers/%package%.json\"))"
  "solver-problems|update @nolock @stderr @require:lib-nope=^2.0 @require:acme/provides-lib=^1.0"
  "solver-problems|update @nolock @stderr @require:lib-icu=^999"
  "solver-problems|update @nolock @stderr @require:acme/needs-php=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/needs-ext=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/needs-ext=^1.0 @jq:.config.platform[\"ext-nope\"]=false"
  "solver-problems|update @nolock @stderr @require:acme/needs-php=^1.0 @platform:php=7.4.0"
  "solver-problems|update --ignore-platform-req=ext-nope @nolock @stderr @require:ext-nope=* @require:acme/needs-php=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/top=^1.0"
  "solver-problems|update @nolock @stderr @require:acme/top=^1.0 @require:acme/leaf=^1.0"
  "solver-problems|update acme/removed @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.0"
  "solver-problems|update acme/removed @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.0 @registry-jq:.[\"available-package-patterns\"]=[\"acme/a*\",\"acme/c*\",\"acme/l*\",\"acme/m*\",\"acme/n*\",\"acme/o*\",\"acme/p*\",\"acme/s*\",\"acme/t*\",\"acme/u*\"]"
  "solver-problems|update acme/other @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.1"
  "solver-problems|update acme/other @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.1 @registry-jq:.[\"available-package-patterns\"]=[\"acme/a*\",\"acme/c*\",\"acme/l*\",\"acme/m*\",\"acme/n*\",\"acme/o*\",\"acme/p*\",\"acme/s*\",\"acme/t*\",\"acme/u*\"]"
  "solver-problems|update acme/other @stderr @jq:.[\"minimum-stability\"]=\"stable\""
  "solver-problems|update acme/other @stderr @require:acme/missing-dep=^1.0"
  "solver-problems|update acme/other -W @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.1"
  "solver-problems|update @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.1"
  "solver-problems|update @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.1 @registry-jq:.[\"available-package-patterns\"]=[\"acme/a*\",\"acme/c*\",\"acme/l*\",\"acme/m*\",\"acme/n*\",\"acme/o*\",\"acme/p*\",\"acme/s*\",\"acme/t*\",\"acme/u*\"]"
  "solver-problems|update @stderr @lockfile:composer.lock.removed @require:acme/removed=^1.0 @require:acme/missing-dep=^1.0"
  "solver-problems|update --no-dev @nolock @stderr @require:acme/nope=^1.0"
  "solver-problems|require acme/missing-dep @stderr"
  "solver-problems|remove acme/other @stderr @require:acme/missing-dep=^1.0"
  "solver-policies|update @nolock @stderr @require:acme/only-bad=^1"
  "solver-policies|update @nolock @stderr @require:acme/vuln=1.1.0"
  "solver-policies|update @nolock @stderr @jq:.config.policy.abandoned.block=true"
  "solver-policies|update acme/vuln @stderr @lockfile:composer.lock.malware"
  "solver-policies|install @stderr @lockfile:composer.lock.malware"
)
[ -x "$VIVACITY" ] || { echo "binaire absent : cargo build --release"; exit 1; }
mkdir -p "$WORK"
status=0
for fx in "${FIXTURES[@]}"; do
  archive="$ROOT/fixtures/registry/$fx.tar.gz"
  [ -f "$archive" ] || { echo "SKIP $fx : pas d'instantané (tools/snapshot-packagist.sh $fx)"; continue; }
  reg="$WORK/registry-$fx"; rm -rf "$reg"; mkdir -p "$reg"
  tar -C "$reg" -xzf "$archive"
  # packages.json avec l'URL absolue (Composer résout un metadata-url
  # relatif contre la racine du système de fichiers) et les politiques de
  # blocage déclarées comme sur Packagist.
  write_snapshot_packages_json "$reg"
  home="$WORK/home-$fx"; rm -rf "$home"; mkdir -p "$home"
  printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}}\n' "$reg" > "$home/config.json"
  root_version=""; [ "$fx" = "rector" ] && root_version="dev-main"
  n=0
  for spec in "${STEPS[@]}"; do
    [ "${spec%%|*}" = "$fx" ] || continue
    n=$((n + 1))
    read -r -a sargs <<< "${spec#*|}"
    preps=(); stubs=(); envs=(); registry_edited=0; compare_stderr=0
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
        @global-sort) printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}, "config": {"sort-packages": true}}\n' "$reg" > "$home/config.json" ;;
        @registry-jq:*) jq "${prep#@registry-jq:}" "$reg/packages.json" > "$reg/p.tmp" && mv "$reg/p.tmp" "$reg/packages.json"; registry_edited=1 ;;
        @env:*) kv="${prep#@env:}"; export "${kv%%=*}=${kv#*=}"; envs+=("${kv%%=*}") ;;
        @stderr) compare_stderr=1 ;;
      esac
    done
    for side in ref viv; do
      d="$WORK/$side-$fx-$n"; rm -rf "$d"; mkdir -p "$d"
      cp "$ROOT/fixtures/projects/$fx/composer.json" "$d/"
      [ -f "$ROOT/fixtures/projects/$fx/composer.lock" ] && cp "$ROOT/fixtures/projects/$fx/composer.lock" "$d/"
      for prep in "${preps[@]+"${preps[@]}"}"; do
        case "$prep" in
          @stub:*|@global-allow:*|@global-sort|@registry-jq:*|@env:*|@stderr) ;;
          @repofilter:*) jq --arg u "file://$reg" --argjson f "${prep#@repofilter:}" '.repositories.snapshot = {"type": "composer", "url": $u, "filter": $f}' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @emptyjson) : > "$d/composer.json" ;;
          @nojson) rm -f "$d/composer.json" "$d/composer.lock" ;;
          @notype) jq 'del(.type)' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @lockfalse) jq '.config.lock = false' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @indent2) jq --indent 2 . "$d/composer.lock" > "$d/l.tmp" && mv "$d/l.tmp" "$d/composer.lock" ;;
          @corruptlock) printf '{"packages": [' > "$d/composer.lock" ;;
          @minstab:*) jq --arg s "${prep#@minstab:}" '.["minimum-stability"] = $s | .["prefer-stable"] = false' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @platform:*) kv="${prep#@platform:}"; jq --arg p "${kv%%=*}" --arg v "${kv#*=}" '.config.platform[$p] = $v' "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
          @lockfile:*) cp "$ROOT/fixtures/projects/$fx/${prep#@lockfile:}" "$d/composer.lock" ;;
          @jq:*) jq "${prep#@jq:}" "$d/composer.json" > "$d/c.tmp" && mv "$d/c.tmp" "$d/composer.json" ;;
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
    # `install` : ni --no-audit (il n'audite que sur --audit) ni --no-install
    # (refusé) ; --dry-run vérifie le lock (politiques, plateforme) sans
    # rien télécharger.
    extra=(--no-install --no-audit); [ "${sargs[0]}" = "install" ] && extra=(--dry-run)
    quiet=(--quiet); [ "$compare_stderr" = 1 ] && quiet=(--no-ansi)
    (cd "$WORK/ref-$fx-$n" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" COMPOSER_TESTS_ARE_RUNNING=1 \
      composer "${sargs[@]}" "${extra[@]}" --no-scripts --no-plugins --no-interaction "${quiet[@]}" >"$WORK/$fx-$n.composer.log" 2>"$WORK/$fx-$n.composer.err") || ref_code=$?
    viv_code=0
    (cd "$WORK/viv-$fx-$n" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" \
      "$VIVACITY" "${sargs[@]}" "${extra[0]}" >"$WORK/$fx-$n.vivacity.log" 2>"$WORK/$fx-$n.vivacity.err") || viv_code=$?
    # Les métadonnées remplacées par @stub sont rendues à l'instantané, le
    # packages.json et l'environnement aussi.
    for f in "${stubs[@]+"${stubs[@]}"}"; do rm -f "$f"; [ -f "$f.orig" ] && mv "$f.orig" "$f"; done
    [ "$registry_edited" = 0 ] || write_snapshot_packages_json "$reg"
    for e in "${envs[@]+"${envs[@]}"}"; do unset "$e"; done
    label="$fx ${sargs[*]}"; [ ${#preps[@]} -gt 0 ] && label="$label (${preps[*]})"
    if [ "$ref_code" != "$viv_code" ]; then
      echo "FAIL $label : code retour composer=$ref_code vivacity=$viv_code"
      tail -3 "$WORK/$fx-$n.composer.err" "$WORK/$fx-$n.vivacity.err"; status=1; continue
    fi
    ok=1
    if [ "$compare_stderr" = 1 ]; then
      # De la ligne d'ancrage à la fin ; les lignes de progression avant
      # (« Loading composer repositories… ») ne sont pas comparées.
      anchor='^(Your requirements could not be resolved|Unable to find a compatible set|Your lock file does not contain)'
      for side in composer vivacity; do
        sed -E -n "/$anchor/,\$p" "$WORK/$fx-$n.$side.err" > "$WORK/$fx-$n.$side.tail"
      done
      if ! [ -s "$WORK/$fx-$n.composer.tail" ]; then
        echo "FAIL $label : pas de ligne d'ancrage dans la sortie de Composer (cas mal choisi)"; ok=0
      elif ! grep -q '^    - ' "$WORK/$fx-$n.composer.tail"; then
        echo "FAIL $label : oracle aveugle — Composer n'a écrit aucune raison (\`    - …\`)"; ok=0
      elif ! diff -q "$WORK/$fx-$n.composer.tail" "$WORK/$fx-$n.vivacity.tail" >/dev/null 2>&1; then
        echo "FAIL $label : les explications diffèrent"
        diff "$WORK/$fx-$n.composer.tail" "$WORK/$fx-$n.vivacity.tail" | head -30 || true; ok=0
      fi
    fi
    for f in composer.json composer.lock; do
      # Absent des deux côtés (manifeste jamais créé, lock jamais écrit) : égal.
      [ -e "$WORK/ref-$fx-$n/$f" ] || [ -e "$WORK/viv-$fx-$n/$f" ] || continue
      if ! diff -q "$WORK/ref-$fx-$n/$f" "$WORK/viv-$fx-$n/$f" >/dev/null 2>&1; then
        echo "FAIL $label : $f diffère"
        diff "$WORK/ref-$fx-$n/$f" "$WORK/viv-$fx-$n/$f" | head -20 || true; ok=0
      fi
    done
    if [ "$ok" = 1 ]; then
      changed=""
      cmp -s "$ROOT/fixtures/projects/$fx/composer.json" "$WORK/ref-$fx-$n/composer.json" || changed="json"
      [ -f "$WORK/ref-$fx-$n/composer.lock" ] && { cmp -s "$ROOT/fixtures/projects/$fx/composer.lock" "$WORK/ref-$fx-$n/composer.lock" || changed="$changed lock"; } || true
      msg="identiques"; [ "$compare_stderr" = 1 ] && msg="identiques, explications comprises"
      echo "OK   $label : $msg (code $ref_code, modifié : ${changed:-rien})"
    else
      status=1
    fi
  done
done
exit $status
