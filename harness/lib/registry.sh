# Fonctions partagées par les harness et le script d'instantané : le
# packages.json d'un instantané Packagist rejoué en `file://`.
#
# Il déclare ce que Packagist déclare pour que les politiques de blocage de
# Composer (avis de sécurité, liste malware) s'appliquent au rejeu : les avis
# partiels sont dans les fichiers p2 (`security-advisories`), la liste
# malware vient du résumé `summary.json` (capturé avec l'instantané, vide
# sinon) puis des fichiers p2 (`filter`). `api-url` nul exige
# `available-package-patterns`.

# write_snapshot_packages_json <registry dir>
write_snapshot_packages_json() {
  local reg="$1"
  [ -f "$reg/summary.json" ] || printf '{"filter": {"malware": {}}}\n' > "$reg/summary.json"
  printf '{"packages": [], "notify-batch": "https://packagist.org/downloads/", "metadata-url": "file://%s/p2/%%package%%.json", "available-package-patterns": ["*"], "security-advisories": {"metadata": true, "api-url": null}, "filter": {"metadata": true, "lists": {"malware": {"enabled": true}}, "summary-url": "file://%s/summary.json"}}\n' "$reg" "$reg" > "$reg/packages.json"
}

# snapshot_repositories_json <registry dir>
# Le bloc `repositories` de la configuration globale : l'instantané rejoué
# en file:// et Packagist désactivé — ou seulement Packagist désactivé quand
# l'instantané n'a aucune métadonnée (fixture servie par des dépôts `path` :
# un dépôt composer vide ferait échouer chaque nom en « could not be
# downloaded »).
snapshot_repositories_json() {
  local reg="$1"
  if [ -n "$(find "$reg/p2" -type f 2>/dev/null | head -1)" ]; then
    printf '{"snapshot": {"type": "composer", "url": "file://%s"}, "packagist.org": false}' "$reg"
  else
    printf '{"packagist.org": false}'
  fi
}
