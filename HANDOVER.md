# HANDOVER

État par jalon, commandes de validation, risques connus, pièges. Mis à jour en continu.

## Validation (toutes plateformes de dev)

```bash
fixtures/make.sh                         # une fois : crée + qualifie les 6 fixtures (php+composer requis)
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo test                               # inclut les tests oracle (php + composer dans le PATH)
cargo build --release
harness/diff-vendor.sh [--with-autoloader]   # parité vs Composer sur les 6 fixtures (projet entier pour wordpress et drupal)
harness/removal.sh                       # paquets retirés du lock : même projet que Composer après
harness/transitions.sh                   # montée de version d'un plugin émulé → main rendue à Composer, disque intact
harness/update.sh                        # (résolveur, R0) locks identiques sur l'instantané Packagist figé — rouge tant que `vivace update` n'existe pas
tools/snapshot-packagist.sh <fixture>    # (re)capture un instantané Packagist + lock de référence
harness/boot.sh                          # les 6 fixtures démarrent sur un vendor 100 % vivace
harness/drift-reference.sh [phar]        # docs/reference/ == fichiers du phar (2.10.3 ou autre)
php tools/gen-installers-table.php /tmp/composer.phar [src] [tag]   # régénère assets/installers/<tag>.json
harness/linux.sh                         # toute la chaîne dans un conteneur Linux (Docker)
bench/profile.sh ; bench/spike-vs-composer.sh   # M0, longs
```

Les tests intégration `tests/oracle_*.rs` et `tests/fixtures_*.rs` copient le
binaire `composer` du PATH vers `$TMPDIR/vivace-oracle-composer.phar` et
appellent ses classes via `php -r`. Sans php/composer ils ÉCHOUENT avec un
message explicite (jamais de skip silencieux).

## Jalons

| jalon | état | preuve |
|---|---|---|
| M0 fixtures + profil + spike | terminé | bench/M0-profil.md |
| M1 manifestes, content-hash, scope, platform | terminé | oracle golden + différentiels + proptest, 0 divergence |
| M2 fetch + store + clone + état + proxies + CLI | terminé (`--no-autoloader`) | harness/diff-vendor.sh : 0 diff × 3 fixtures ; bench/M2-install.md |
| M3 autoload (normal, -o, -a, --no-dev) | terminé | harness --with-autoloader : 0 diff × 3 fixtures ; oracle classmap ~50k fichiers ; bench/M3-autoload.md |
| M4 harness (parité + boot), Linux en conteneur, CI GitHub Actions | terminé : chaîne verte dans le conteneur Linux arm64 (copie ET hardlinks) ET sur GitHub Actions ubuntu-latest x86_64 + macos-latest (run #2, 2026-09-10) ; réseau réel exercé | bench/M4-linux.md, harness/*.sh, .github/workflows/ci.yml |
| M5 perf classmap | terminé (détection parallèle + cache par entrée de store) ; benchmarks publiables à consolider en M6 | bench/M5-perf.md ; harness 0 diff cache froid/chaud |
| M6 sortie publique | terminé : v0.1.0/v0.1.1 publiées, annonce r/PHP | .github/workflows/release.yml |
| v0.2 composer/installers natif, drift, action | publié (v0.2.0, 2026-09-11) | tests/oracle_installers.rs (665 cas), fixture wordpress (projet entier 0 diff), harness/removal.sh, drift.yml, action.yml + action-test.yml |
| v0.3 drupal/core-composer-scaffold natif | publié (v0.3.0, 2026-09-11) | tests/oracle_scaffold.rs (15 cas, arbres entiers), fixture drupal (projet entier 0 diff, boot `vendor/bin/dr`), harness/transitions.sh |

| v0.4 résolveur (option A : port du solveur) | R0 fait : instantanés + harness update.sh (Composer reproduit le lock de référence sur 5 fixtures) ; R1 métadonnées, R2 solveur, R3 `update` à faire | docs/plans/v0.4-resolver.md, fixtures/registry/, harness/update.sh |

## Ce qui N'EST PAS couvert / testé (honnêtement)

- **Cache de classmap** : suppose vendor/ immuable entre deux installs (un fichier édité à la main n'est pas rescanné) ; `VIVACE_NO_CLASSMAP_CACHE=1` pour désactiver. Sur un vendor/ posé par Composer, le premier `vivace install` chauffe le store depuis le cache zip (≈1 s sur Laravel) ; les suivants profitent du cache (65 ms). `VIVACE_TRACE=1` affiche les phases (temps cumulés).
- **Autoload, cas non exercés par les fixtures** : `target-dir` avec psr-0 racine (targetDirLoader non porté), `include-path`, apcu, `exclude-from-classmap` avec globs `**` (porté, non vérifié par diff), chemins `.phar`.

- **Linux** : exercé en conteneur arm64 (php:8.4) et sur runner GitHub x86_64 (ubuntu-latest) — gates, tests, parité, boot ; copie et hardlinks exercés en conteneur. Perf Linux mesurée en runs uniques seulement (pas d'hyperfine sur runner).
- **Réseau réel** : exercé une fois (109 zips GitHub via rustls, caches vides, 3,46 s, app boote) ; retries/backoff et auth jamais exercés en conditions réelles. `gitlab-token`/`gitlab-oauth` non implémentées (github-oauth, http-basic, bearer seulement). rustls n'utilise pas le magasin de CA système (`SSL_CERT_FILE` ignoré).
- **Version du root package** : portée (VersionGuesser git : branche, HEAD détaché, tag exact, branche de feature → parente, branch-alias ; COMPOSER_ROOT_VERSION). Le harness commite un dépôt git identique des deux côtés : parité vérifiée sur `main`. Non exercés par le harness : branches de feature, HEAD détaché, alias de la racine (tests unitaires seulement) ; hg/fossil/svn non portés (défaut `1.0.0+no-version-set`).
- **Alias des paquets verrouillés** : port de `ArrayLoader::getBranchAlias` (`branch-alias` + `default-branch`), oracle de 31 cas contre le phar ; exercé par le harness via la fixture rector (`dev-main` + `default-branch`). Non exercé par diff : `extra.branch-alias` sur un paquet verrouillé en dev (oracle seulement).
- **`extra.runtime` personnalisé** : routé en fallback, pas émulé.
- **Windows** : hors scope v1 (proxies .bat non générés).
- **Concurrence** : deux installs simultanés sur le même vendor/ ne sont pas
  protégés (comme Composer) ; le store, lui, est sûr (temp+rename).
- **Drift** : `ci.yml` épinglé sur Composer 2.10.3 ; `drift.yml` (hebdo +
  manuel) teste `composer:v2` et `snapshot` en deux étages (jumeaux de
  docs/reference/, puis tests + harness) et ouvre une issue `drift`. Le
  template symfony/runtime n'a pas de jumeau vendoré (pas dans le phar) :
  son drift n'est vu que par le boot de la fixture symfony.
- **composer/installers** : émulé pour les tags 2.0.0…2.3.0 et les 58
  frameworks « table seule » ; les 38 à logique custom (agl, akaunting,
  asgard, bitrix, cakephp, cockpit, croogo, dokuwiki, ee2, ee3, fork, grav,
  hurad, lms, majima, mantisbt, matomo, mautic, maya, mediawiki, microweber,
  october, ontowiki, oxid, piwik, plentymarkets, processwire, pxcms, radphp,
  roundcube, shopware, silverstripe, sitedirect, sydes, tao, tastyigniter,
  winter, yawik) → fallback nominatif. Exercé par diff : WordPress (plugins,
  mu-plugin, thème, `installer-paths` par type et par nom, `bin` hors
  vendor/, suppression, `--no-plugins`, `--working-dir` relatif à la main) ;
  par oracle seulement : les autres frameworks,
  `installer-disable`, `installer-name`, `vendor:`. Non couvert : un
  `installer-paths` ciblant vendor/ (refusé), `allow-plugins` global lu mais
  jamais exercé en CI, un lock 1.x (refusé), un projet dont le vendor a été
  posé par un installers de version différente (le plan de suppression
  compare l'ancien install-path au chemin recalculé : désaccord → fallback) ;
  plugin présent d'un seul côté entre installed.json et le lock → fallback
  (test unitaire, pas de fixture). Non porté : `realpath()` de BinaryInstaller
  sur un vendor/ symlinké avec un `bin` hors vendor/ (Composer écrirait un
  chemin absolu) ; `installer-name` contenant `{` refusé plutôt qu'imité.
- **drupal/core-composer-scaffold** : émulé pour 116 des 120 versions
  (10.3.0 → 12.0.0-alpha1 ; 11.3.0–11.3.3 refusées : hash non trié). Exercé
  par diff : la fixture drupal (file-mapping de drupal/core, locations
  `web/`, autoload de référence, DrupalInstalled.php, .gitignore sur un dépôt
  ignorant vendor/) ; par oracle : surcharges, append/prepend/default,
  overwrite false, allowed-packages récursifs, locations personnalisées,
  fichiers trackés, gitignore forcé, profils 11.3.16 et 11.2.14. Non
  couvert : `symlink: true` (refusé), git absent du PATH (traité comme
  « ni ignoré ni tracké »), un `.gitignore` global (`core.excludesFile`)
  différent entre deux machines, `[web-root]` symlinké (accepté si la
  location déclarée se résout), perms des fichiers scaffoldés (contenu et
  présence comparés, pas les modes). Les hooks `pre/post-drupal-scaffold-cmd`
  du composer.json racine ne sont pas exécutés (comme tout script).
- **Extraction** : strip du dossier racine seulement s'il est unique
  (règle ArchiveDownloader) ; un zip avec un `.DS_Store` de premier niveau
  et rien d'autre à côté du dossier est traité comme mono-dossier, comme Composer.

## Pièges

- `serde_json` DOIT garder `preserve_order` + `float_roundtrip` (content-hash).
- Le pattern classmap de Composer exige pcre2 (possessifs, lookbehind,
  octets non-UTF-8) — décision plan r1/F4. Les noms de classes sont des `Vec<u8>`.
- `harness/diff-vendor.sh` copie le projet complet (les règles d'autoload de
  la racine — `src/Kernel.php` chez Sylius — doivent exister).
- Sylius boot : `php -d memory_limit=1G` (128 Mo brew insuffisants) ; une résolution FRAÎCHE de sylius-standard ne boote pas sur PHP 8.5 (Doctrine ORM / lazy objects) — CI épinglée en PHP 8.4.
- `symfony/demo` n'existe pas sur Packagist : `symfony/symfony-demo`.
- Les fixtures (`fixtures/work/`) sont gitignorées : `fixtures/make.sh` d'abord — il copie les squelettes FIGÉS de `fixtures/projects/` (locks committés) ; ne pas re-résoudre.
- Cache Composer : chemins canoniques portés de `Factory` (Linux sans XDG → `~/.composer/cache`) ; `harness/linux.sh` persiste `/root/.composer` dans un volume.
