#!/usr/bin/env bash
# Empreinte de la source d'un plugin Composer extrait : sha256 de la
# concaténation « <chemin relatif>\n<contenu> » de chaque *.php hors tests/,
# dans l'ordre trié des chemins. Deux dists à source identique (même code,
# metadata différentes) ont la même empreinte : c'est le critère qui décide
# si vivace émule cette version du plugin.
set -euo pipefail
dir="${1:?usage: plugin-fingerprint.sh <package dir>}"
cd "$dir"
find . -name '*.php' -not -path './tests/*' -not -path './Tests/*' | LC_ALL=C sort | while read -r f; do
  printf '%s\n' "${f#./}"; cat "$f"
done | shasum -a 256 | cut -d' ' -f1
