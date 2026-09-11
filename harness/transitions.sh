#!/usr/bin/env bash
# Transitions qu'un plugin émulé rend non reproductibles : vivace doit rendre
# la main à Composer AVANT de toucher au disque, avec un message nominatif.
#
# Cas : montée de version de drupal/core-composer-scaffold. Composer exécute
# alors l'ancien Handler (chargé depuis vendor/) avec le nouveau Plugin — un
# mode mixte que vivace refuse d'imiter quand les deux sources diffèrent.
# Préparation : vendor posé par Composer avec le plugin en 11.4.1 (source
# différente de 11.4.6), puis lock remis à 11.4.6.
#
# Usage : harness/transitions.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VIVACE="$ROOT/target/release/vivace"
WORK="${VIVACE_HARNESS_DIR:-/tmp/vivace-harness}/transitions"
[ -x "$VIVACE" ] || { echo "binaire absent : cargo build --release"; exit 1; }
src="$ROOT/fixtures/work/drupal"
dir="$WORK/drupal-upgrade"
rm -rf "$dir"; mkdir -p "$dir"
(cd "$src" && tar --exclude=./vendor --exclude=./web --exclude=./recipes -cf - .) | (cd "$dir" && tar -xf -)
cd "$dir"
# État « milieu de montée de version » : vendor posé (11.4.6), puis la copie
# installée du plugin remplacée par la source 11.4.1 et installed.json aligné —
# drupal/core exige la version exacte du scaffold, un lock mixte n'existe pas,
# c'est vendor/ qui est en retard sur le lock pendant l'upgrade du cœur.
composer install --no-interaction --no-scripts --quiet
old_json=$(curl -fsSL https://repo.packagist.org/p2/drupal/core-composer-scaffold.json)
old_url=$(python3 - "$old_json" <<'EOF'
import json, sys
cur = {}
for p in json.loads(sys.argv[1])["packages"]["drupal/core-composer-scaffold"]:
    cur = {**cur, **p}
    if cur["version"] == "11.4.1":
        print(cur["dist"]["url"]); break
EOF
)
curl -fsSL -o old.zip "$old_url"
rm -rf vendor/drupal/core-composer-scaffold old && mkdir old && (cd old && unzip -q ../old.zip) && mv old/* vendor/drupal/core-composer-scaffold && rm -rf old old.zip
python3 - <<'EOF'
import json
p = "vendor/composer/installed.json"; d = json.load(open(p))
for pkg in d["packages"]:
    if pkg["name"] == "drupal/core-composer-scaffold":
        pkg["version"] = "11.4.1"; pkg["version_normalized"] = "11.4.1.0"
json.dump(d, open(p, "w"), indent=4)
EOF
before=$( (find web vendor -type d | sort; find web vendor -type f -exec shasum -a 256 {} +) | sort | shasum -a 256)
if "$VIVACE" install --no-fallback --offline >"$WORK/upgrade.log" 2>&1; then
  echo "FAIL upgrade : vivace a installé au lieu de rendre la main"; exit 1
fi
if ! grep -q 'is being upgraded from 11.4.1 to 11.4.6 with a different source' "$WORK/upgrade.log"; then
  echo "FAIL upgrade : message inattendu :"; tail -5 "$WORK/upgrade.log"; exit 1
fi
after=$( (find web vendor -type d | sort; find web vendor -type f -exec shasum -a 256 {} +) | sort | shasum -a 256)
[ "$before" = "$after" ] || { echo "FAIL upgrade : le disque a été modifié avant le refus"; exit 1; }
echo "OK   drupal : montée de version du scaffold refusée sans toucher au disque"
