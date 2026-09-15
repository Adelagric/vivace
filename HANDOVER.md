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
harness/update.sh                        # `composer update` vs `vivacity update` (complet + 7 cas partiels) : locks identiques sur l'instantané Packagist figé
harness/steps.sh                         # `composer require|remove` vs vivacity (75 cas) : composer.json, lock et code retour identiques
tools/snapshot-packagist.sh <fixture>    # (re)capture un instantané Packagist + lock de référence
harness/boot.sh                          # les 6 fixtures démarrent sur un vendor 100 % vivacity
harness/drift-reference.sh [phar]        # docs/reference/ == fichiers du phar (2.10.3 ou autre)
php tools/gen-installers-table.php /tmp/composer.phar [src] [tag]   # régénère assets/installers/<tag>.json
harness/linux.sh                         # toute la chaîne dans un conteneur Linux (Docker)
bench/profile.sh ; bench/spike-vs-composer.sh   # M0, longs
```

Les tests intégration `tests/oracle_*.rs` et `tests/fixtures_*.rs` copient le
binaire `composer` du PATH vers `$TMPDIR/vivacity-oracle-composer.phar` et
appellent ses classes via `php -r`. Sans php/composer ils ÉCHOUENT avec un
message explicite (jamais de skip silencieux).

## Publication sur crates.io (0.5.0 publiée le 2026-09-14)

Les quatre crates s'empaquettent (`cargo package --workspace --no-verify`,
vérifié : 38 à 138 KiB compressés chacun). Procédure, dans l'ordre des
dépendances, gérée par cargo :

```bash
cargo login                       # une fois, jeton du compte crates.io du mainteneur
# bumper la version : [workspace.package].version ET les trois dépendances
# `path = "../vivacity-*", version = "…"` (crates/vivacity*/Cargo.toml)
cargo publish --workspace         # core → autoload → resolver → vivacity
```

Les quatre crates sont sur crates.io en 0.5.0 (`cargo install vivacity` vérifié). Un job de publication
dans release.yml demanderait un secret `CARGO_REGISTRY_TOKEN` (décision du
mainteneur). Une fois publiés, `cargo install vivacity` et l'embarquement
(`vivacity = "0.5"` → `vivacity::run(args)`) ne passent plus par git.

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
| v0.3 drupal/core-composer-scaffold natif | publié (v0.3.0, 2026-09-11), **retiré en 0.6.0** (port d'un code GPL-2.0-or-later, voir Licences) ; la fixture drupal passe désormais par le fallback `composer install` | fixture drupal (projet entier 0 diff via le fallback, boot `vendor/bin/dr`), harness/transitions.sh (scaffold présent → refus sans toucher au disque) |

| v0.4 résolveur (option A : port du solveur) | publié (v0.4.0, 2026-09-12) : `vivacity update` écrit le lock de Composer à l'octet (pool, séquence de décisions du solveur, opérations, lock) ; cache de métadonnées au format de Composer ; mises à jour partielles | docs/plans/v0.4-resolver.md, tests/oracle_pool.rs, harness/update.sh |
| v0.8 `--dry-run` + lignes d'opérations + rapport post-update | prêt à taguer (0.8.0, 2026-09-15) : `steps.sh` compare stderr par défaut (202 cas, 180 avec stderr identique), `@dry-install` ; `RootPatch` pour `require`/`remove --dry-run` ; lock virtuel pour la phase d'installation d'un dry run ; dépôts `path` → v0.9 | docs/plans/v0.8-dry-run-path.md |
| v0.7 Windows livré + explications d'un ensemble insoluble | prêt à taguer (0.7.0, 2026-09-15) : binaire Windows dans la release, `install.sh` Git Bash + `install.ps1`, action sur windows-latest, `bin-compat` global ; `problem.rs` (Problem/Rule/SolverProblemsException), 51 cas d'explications identiques à l'octet dans `harness/steps.sh` | docs/plans/v0.7-windows-problems.md, fixtures/make-solver-problems.py, tools/package-release.sh |
| v0.5 require/remove/politiques | publié (v0.5.0, 2026-09-14, 4 crates sur crates.io) : partielles, `JsonManipulator` (12 102 + 725 scénarios vs phar), `remove`, `VersionSelector` (902 noms), `require`, politiques de blocage (54 cas) ; `harness/steps.sh` 128 cas | docs/plans/v0.5-require-remove.md, tests/oracle_json_manipulator.rs, harness/steps.sh |

## Licences (2026-09-14)

NOTICE.md liste les origines des ports et leurs licences ; les textes MIT de
Composer, composer/semver, class-map-generator, metadata-minifier et
composer/installers sont à côté des sources vendorées. L'émulation de
`drupal/core-composer-scaffold` (port d'un code GPL-2.0-or-later, œuvre
dérivée incompatible avec la distribution MIT/Apache-2.0) a été retirée en
0.6.0 avec sa source vendorée ; les versions 0.3.0 à 0.5.0 qui la
contenaient ne sont plus distribuées sur crates.io (crates `vivace*`
supprimés le 2026-09-15, ce qui libère aussi le nom pour svandragt/vivace).
Rien n'est porté d'un logiciel GPL.

## Ce qui N'EST PAS couvert / testé (honnêtement)

- **Parallélisme d'E/S** : `platform::parallel_io()` (Linux vrai, ailleurs faux, `VIVACITY_PARALLEL_IO=0|1`) gouverne les répertoires en parallèle du scan et le fan-out de la matérialisation ; mesuré sur APFS et ext4 seulement (DECISIONS 2026-09-15) — btrfs/xfs (reflink), NFS, disques lents : non mesurés. Cache de classmap en binaire v2 (`CACHE_FORMAT`), les anciens `*.json` restent orphelins dans `~/.cache/vivacity/classmap/` (jamais lus).
- **Cache de classmap** : suppose vendor/ immuable entre deux installs (un fichier édité à la main n'est pas rescanné) ; `VIVACITY_NO_CLASSMAP_CACHE=1` pour désactiver. Sur un vendor/ posé par Composer, le premier `vivacity install` chauffe le store depuis le cache zip (≈1 s sur Laravel) ; les suivants profitent du cache (65 ms). `VIVACITY_TRACE=1` affiche les phases (temps cumulés).
- **Autoload, cas non exercés par les fixtures** : `target-dir` avec psr-0 racine (targetDirLoader non porté), `include-path`, apcu, `exclude-from-classmap` avec globs `**` (porté, non vérifié par diff), chemins `.phar`.

- **Linux** : exercé en conteneur arm64 (php:8.4) et sur runner GitHub x86_64 (ubuntu-latest) — gates, tests, parité, boot ; copie et hardlinks exercés en conteneur. Perf Linux mesurée en runs uniques seulement (pas d'hyperfine sur runner).
- **Réseau réel** : exercé une fois (109 zips GitHub via rustls, caches vides, 3,46 s, app boote) ; retries/backoff et auth jamais exercés en conditions réelles. `gitlab-token`/`gitlab-oauth` non implémentées (github-oauth, http-basic, bearer seulement). rustls n'utilise pas le magasin de CA système (`SSL_CERT_FILE` ignoré).
- **Version du root package** : portée (VersionGuesser git : branche, HEAD détaché, tag exact, branche de feature → parente, branch-alias ; COMPOSER_ROOT_VERSION). Le harness commite un dépôt git identique des deux côtés : parité vérifiée sur `main`. Non exercés par le harness : branches de feature, HEAD détaché, alias de la racine (tests unitaires seulement) ; hg/fossil/svn non portés (défaut `1.0.0+no-version-set`).
- **Alias des paquets verrouillés** : port de `ArrayLoader::getBranchAlias` (`branch-alias` + `default-branch`), oracle de 31 cas contre le phar ; exercé par le harness via la fixture rector (`dev-main` + `default-branch`). Non exercé par diff : `extra.branch-alias` sur un paquet verrouillé en dev (oracle seulement).
- **`extra.runtime` personnalisé** : routé en fallback, pas émulé.
- **Politiques de blocage** : portées (avis, liste malware, abandonnés) ; non portés : les listes personnalisées avec sources (refusées quand un dépôt les annonce) et un dépôt avec `filter.api-url` (refusé) ; le message d'échec d'`install` sur un lock signalé ne liste que les versions retirées, pas les problèmes des paquets qui en dépendent (Composer rejoue le solveur ; code 2 identique) ; le code retour d'une source injoignable avec `ignore-unreachable: false` est 1 (Composer 100) ; `vivacity install` fait désormais un aller-retour réseau (packages.json des dépôts, résumé des listes) comme Composer, ignoré avec avertissement hors ligne ; `bump-after-update` (config ou option) n'est pas exécuté par `vivacity update` (Sylius l'a dans sa config : Composer réécrit composer.json après l'update, vivacity non) ; `--with`, `update lock/nothing/mirrors`, `--minimal-changes`, les dépôts `vcs`/`path` : non portés.
- **Sortie des commandes** (v0.8) : lignes d'opérations, suggestions, abandonnés, funding, `install` (`Installing dependencies…`, `Verifying…`, `Package operations`) et `--dry-run` portés et comparés à l'octet ; non couverts : la sortie d'une installation réelle (`  - Downloading`/`Installing` avec progression, `Generating autoload files` — vivacity garde sa ligne de résumé), l'audit (`--no-audit` est le comportement comparé), l'échec de plateforme à `install` (Composer passe par le solveur — « Your lock file does not contain a compatible set… » code 2 — vivacity garde son message et le code 4), le filtre des suggestions utilise la plateforme *avec* `config.platform` (Composer lit `platform-overrides` du lock = la même chose). Portés après revue : `getMissingRequirementInfo` (exigence racine absente du lock → code 4, texte identique), la purge d'installed.json (`Factory::purgePackages` : un chemin d'installation disparu = paquet absent, pour la transaction et le funding), la ligne funding en dry run aussi (`COMPOSER_FUND=0` la coupe), la restauration de composer.json par `require` quand la phase d'installation échoue (`revertComposerFile` sur tout statut non nul), le fallback `composer install` jamais lancé par un dry run.
- **Explications d'un ensemble insoluble** (v0.7) : `problem.rs` = port de `Problem`, `Rule::getPrettyString`, `SolverProblemsException` (+ `RepositorySet::findPackages`/`getProviders`, `IniHelper::getAll`, le suivi des versions retirées par l'optimiseur et par les politiques dans `Pool`). Oracle : `harness/steps.sh @stderr` (Composer sans `--quiet`, stderr séparé, `COMPOSER_TESTS_ARE_RUNNING`) sur 51 cas, texte identique à l'octet de la ligne d'ancrage à la fin (dont l'API `providers-api` rejouée en `file://`). Portés mais sans oracle (inatteignables avec la CLI supportée) : les contraintes temporaires (`--with`), un dépôt de priorité inférieure (`computeCheckForLowerPrioRepo`, le harness n'injecte qu'un dépôt), les deux indices `composer-plugin-api[2.0.0]` (texte mort en 2.10), la règle `LEARNED` dans un problème (« Conclusion: … ») — le diamant du corpus ne l'atteint pas, `--verbose` (jamais activé : `-v` n'est pas une option de vivacity), l'échec de l'extraction non-dev (« Unable to find a compatible set… », aucun cas construit ne l'atteint : les paquets résolus restent candidats ; de plus ce chemin formate avec le jeu de dépôts de l'update, pas le jeu [racine, plateforme, résultat] de `extractDevPackages` — `getRepoName` et `findPackages` y diffèreraient). Écarts connus et acceptés : `Link::getPrettyString` d'un lien `self.version` d'un alias (Composer imprime `== x.y.z.z`), `substr` d'une description coupée au milieu d'un caractère multi-octets (remplacé par U+FFFD), `Url::sanitize` absent de `getRepoName`. Hypothèse d'oracle : la liste des `.ini` vient de `php_ini_loaded_file()`/`php_ini_scanned_files()` du PHP sondé (`COMPOSER_ORIGINAL_INIS` respecté) — identique sur la machine du harness par construction. Trouvé en chemin, non corrigé : un nom en majuscules dans `require` (`acme/NOPE`) fait sortir Composer en 1 (`RootPackageLoader` : « should not contain uppercase characters ») là où vivacity résout et répond 2.
- **`require`** : `--dry-run`, `--minimal-changes`, `COMPOSER=autre.json` et `config.lock: false` sans `--no-install` refusés ; le code retour d'une erreur de transport est 1 (Composer : 100) ; le mtime du lock n'est pas restauré après `updateHash` ; toujours non interactif (pas de proposition de `--dev` d'après les mots-clés, pas de déplacement de clé demandé, pas de confirmation des branches de fonctionnalité) ; `getProviders` (API Packagist des fournisseurs) et `findSimilar` (« Did you mean ») non portés : un nom introuvable donne le message final de Composer sans suggestion ; la validation `LAX_SCHEMA` idem `remove`.
- **`remove`** : `--dry-run`, `--minimal-changes` et `COMPOSER=autre.json` refusés ; toujours non interactif (Composer, sur un TTY sans `-n`, propose de retirer un paquet trouvé dans l'autre section — vivacity avertit seulement, comme `-n`) ; la validation `LAX_SCHEMA` après chaque édition n'est pas portée (Composer refuse d'éditer un manifeste déjà invalide, vivacity l'édite) ; le code retour d'une erreur de transport diffère (Composer 100 ou statut HTTP, vivacity 1) ; `--unused` liste chaque paquet une fois là où Composer répète les alias (message seulement) ; `config.vendor-dir` n'est pas lu (vendor/ partout dans vivacity) ; `JsonManipulator` : seuil de backtracking PCRE, `1e999`, substituts UTF-16 isolés, > 512 niveaux (en-tête du module).
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
- **drupal/core-composer-scaffold** : plus émulé depuis 0.6.0 (licence).
  Plugin inconnu → fallback `composer install` avant toute écriture ;
  `dump-autoload` refuse (code 3) tant que le plugin est verrouillé et
  autorisé, puisque Composer exécuterait son `pre-autoload-dump`.
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
