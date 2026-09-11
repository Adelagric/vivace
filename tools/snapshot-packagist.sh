#!/usr/bin/env bash
# Instantané figé des métadonnées Packagist d'une fixture, pour que
# `composer update` et `vivace update` résolvent exactement les mêmes
# données (Packagist bouge d'une minute à l'autre).
#
# Méthode : un `composer update --no-install` avec un cache Composer vierge ;
# le cache contient alors toutes les réponses p2 chargées par PoolBuilder
# (`provider-<vendor>~<name>[~dev].json`). Elles sont renommées au format
# d'un dépôt `composer` statique (`p2/<vendor>/<name>[~dev].json`) et
# archivées dans fixtures/registry/<fixture>.tar.gz. Le `packages.json` est
# généré à l'usage (harness/update.sh) avec l'URL absolue `file://` du
# répertoire déballé — Composer résout `metadata-url` relatif contre la
# racine du système de fichiers, pas contre le dépôt.
#
# Seul repo.packagist.org (protocole v2) est capturé : wpackagist (v1,
# provider-includes) est hors périmètre du résolveur v0.4.
#
# Usage : tools/snapshot-packagist.sh <fixture> [<fixture>...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
for fx in "$@"; do
  src="$ROOT/fixtures/projects/$fx"
  [ -f "$src/composer.json" ] || { echo "fixture $fx introuvable"; exit 1; }
  work="$(mktemp -d)"
  cp "$src/composer.json" "$work/"
  # Le lock de la fixture sert de point de départ (comme chez l'utilisateur) :
  # PoolBuilder charge aussi les paquets verrouillés.
  [ -f "$src/composer.lock" ] && cp "$src/composer.lock" "$work/"
  # Version racine : rector-src remplace rector/rector par `self.version` et
  # ses extensions exigent >= 2.0 — hors dépôt git, la racine vaut
  # 1.0.0+no-version-set et la résolution échoue. Même variable des deux
  # côtés du harness.
  root_version=""
  [ "$fx" = "rector" ] && root_version="dev-main"
  ( cd "$work" && COMPOSER_CACHE_DIR="$work/cache" COMPOSER_ROOT_VERSION="$root_version" \
      composer update --no-install --no-scripts --no-plugins --no-interaction --no-audit --quiet )
  cache="$work/cache/repo/https---repo.packagist.org"
  out="$work/registry"
  mkdir -p "$out/p2"
  n=0
  for f in "$cache"/provider-*.json; do
    base="$(basename "$f" .json)"; base="${base#provider-}"
    vendor="${base%%~*}"; rest="${base#*~}"
    mkdir -p "$out/p2/$vendor"
    cp "$f" "$out/p2/$vendor/$rest.json"
    n=$((n + 1))
  done
  # Paquets virtuels (fournis par d'autres, 404 sur Packagist — ex.
  # php-http/client-implementation) : Composer tolère le 404 en HTTP mais pas
  # un fichier absent en file://. On rejoue l'instantané jusqu'à ce que
  # Composer n'échoue plus, en écrivant un stub « aucune version » pour
  # chaque nom manquant, et on note ces noms dans SNAPSHOT.
  printf '{"packages": [], "notify-batch": "https://packagist.org/downloads/", "metadata-url": "file://%s/p2/%%package%%.json"}\n' "$out" > "$out/packages.json"
  home="$work/home"; mkdir -p "$home"
  printf '{"repositories": {"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}}\n' "$out" > "$home/config.json"
  replay="$work/replay"; mkdir -p "$replay"; cp "$src/composer.json" "$replay/"
  [ -f "$src/composer.lock" ] && cp "$src/composer.lock" "$replay/"
  virtual=()
  for _ in 1 2 3 4 5 6 7 8; do
    if (cd "$replay" && COMPOSER_HOME="$home" COMPOSER_CACHE_DIR="$home/cache" COMPOSER_ROOT_VERSION="$root_version" \
          composer update --no-install --no-scripts --no-plugins --no-interaction --no-audit --quiet 2>"$work/replay.log"); then
      break
    fi
    missing=$(tr -d '\n ' < "$work/replay.log" | grep -o "file://$out/p2/[^\"]*\.json" | sed "s|file://$out/p2/||; s|\.json$||" | head -1 || true)
    [ -n "$missing" ] || { echo "replay failed for $fx:"; tail -5 "$work/replay.log"; exit 1; }
    mkdir -p "$out/p2/$(dirname "$missing")"
    printf '{"packages": {"%s": []}, "minified": "composer/2.0"}\n' "$missing" > "$out/p2/$missing.json"
    virtual+=("$missing")
    rm -rf "$home/cache"
  done
  diff -q "$replay/composer.lock" "$work/composer.lock" >/dev/null || { echo "replay lock differs for $fx"; exit 1; }
  rm -f "$out/packages.json"
  # Lock de référence : résolu contre Packagist au moment de la capture, avec
  # le composer.json intact (le harness injecte le dépôt local par la config
  # globale, pas dans le manifeste : le content-hash fait partie de la parité).
  cp "$work/composer.lock" "$out/composer.lock.expected"
  date -u +%Y-%m-%dT%H:%M:%SZ > "$out/SNAPSHOT"
  echo "composer $(composer --version --no-ansi 2>/dev/null | sed -n 's/^Composer version \([^ ]*\).*/\1/p')" >> "$out/SNAPSHOT"
  [ ${#virtual[@]} -gt 0 ] && printf 'virtual %s\n' "${virtual[@]}" >> "$out/SNAPSHOT"
  mkdir -p "$ROOT/fixtures/registry"
  tar -C "$out" -czf "$ROOT/fixtures/registry/$fx.tar.gz" .
  echo "$fx : $n fichiers p2, ${#virtual[@]} virtuels, $(du -h "$ROOT/fixtures/registry/$fx.tar.gz" | cut -f1) — lock de référence : $(jq '.packages | length' "$work/composer.lock") paquets"
  rm -rf "$work"
done
