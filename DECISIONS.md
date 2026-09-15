# DECISIONS

Choix de conception non triviaux, datés, avec alternatives. Compléter en continu.

## 2026-09-09 — Architecture store + clone (plan r3)

Le spike M0 (bench/M0-profil.md) montre que réécrire l'extraction en Rust ne gagne
presque rien (1,01× sur sylius) : le goulot warm est l'I/O metadata (40k fichiers).
Décision : extraire une fois par (paquet, version, ref) dans un store local, puis
cloner vers vendor/ — `clonefile(2)` APFS (0,73 s vs 5,8 s mesuré), reflink
btrfs/xfs, fallback hardlink puis copie. Alternative écartée : extraction directe
optimisée (plafond démontré trop bas).

## 2026-09-09 — Émulation native d'une allowlist de plugins (plan r2)

`--no-plugins` casse le boot de tout Symfony moderne : `vendor/autoload_runtime.php`
est généré par le plugin `symfony/runtime` (stub ~30 lignes, déterministe, options
`extra.runtime`). v1 émule nativement ce stub (test de drift contre le plugin réel) ;
tout autre plugin → fallback `exec composer`. Alternative écartée : exécuter les
plugins PHP (réintroduit le runtime Composer, annule le gain).

## 2026-09-09 — Oracle différentiel comme gate de parité

L'oracle de compatibilité est le Composer installé (phar 2.10.3 copié en .phar,
classes invoquées via `php -r`). Golden bloquant : content-hash des 3 fixtures.
Sources canoniques épinglées depuis le phar dans docs/reference/ (Locker.php,
JsonFile.php) — jamais de port de mémoire.

## 2026-09-09 — serde_json avec `preserve_order` + `float_roundtrip`

- `preserve_order` : le content-hash de Composer ne ksorte que le premier niveau ;
  l'ordre d'insertion des clés imbriquées doit survivre au parsing.
- `float_roundtrip` : trouvé par property test (cas figé en régression dans
  oracle_content_hash.rs) — sans elle, serde_json parse les doubles à 1 ULP près
  et le hash diverge. Coût parsing accepté (les manifestes sont petits).

## 2026-09-09 — Formatage des doubles : port empirique de PHP

`json_encode` (serialize_precision=-1) : chiffres shortest-round-trip, notation
fixe ssi point décimal ∈ (-4, 17], sinon exponentielle `d[.ddd|.0]e±X`, pas de
`.0` final en fixe, `-0` pour le zéro négatif, int > PHP_INT_MAX décodé float.
Bornes relevées empiriquement sur PHP 8.5.10 (table dans l'historique de session,
vérifiée en continu par le proptest différentiel). On réutilise les chiffres de
`format!("{:e}")` (shortest std) plutôt qu'une dépendance ryu directe.

## 2026-09-09 — Dépendances (workspace, épinglées `=X.Y.Z`)

anyhow (main uniquement), serde/serde_json (manifestes), tokio+reqwest (fan-out
HTTP/2, pattern uv/pnpm), zip (extraction, pas d'async nécessaire : spawn_blocking),
sha1/sha2 (cache/shasum Composer), md-5 (content-hash), thiserror (erreurs lib),
clap (CLI), proptest (dev, oracle fuzzé). `hex` retiré au profit de format hex
std à venir dans vivace-core (le spike jetable l'utilise encore).

## 2026-09-09 — M2 : fidélité vendor/ prouvée par diff -r, pas par tests unitaires

Le juge de paix de l'installeur est `harness/diff-vendor.sh` : `diff -r` entre
le vendor de Composer et le nôtre sur les 3 fixtures. Trois comportements de
Composer découverts en chemin et reproduits (aucun n'était dans le plan) :
`target-dir` legacy (chemin d'install `vendor/<name>/<target-dir>`), chemins
d'état raccourcis `./x` pour le namespace `composer/*` (findShortestPath depuis
vendor/composer), `replace`/`provide` du composer.json racine dans installed.php.
Alternative écartée : un harness Rust structuré dès M2 — le script shell suffit
et M4 le formalisera (normalisations, boot, CI).

## 2026-09-09 — Store : clé (nom, version, ref12), pas de hash de contenu

L'entrée de store est adressée par (paquet, version, 12 hex de la référence de
dist) — pas par sha256 du zip : Packagist ne fournit pas de shasum, et la
référence git est déjà l'ancrage d'intégrité de Composer. Coût : un zip
re-publié sous la même référence (cas pathologique) réutiliserait l'ancienne
extraction. Accepté et documenté ; `vivace store prune` viendra plus tard.

## 2026-09-09 — Clone : clonefile(2) macOS, hardlinks ailleurs, copie en repli

Sur Linux, le mode hardlink signifie qu'éditer EN PLACE un fichier de vendor/
modifie le store (même inode) — pnpm vit avec la même contrainte. vivace ne
modifie jamais un fichier cloné (il remplace des répertoires entiers). À
documenter pour les utilisateurs qui patchent vendor/ à la main ; reflink
FICLONE par fichier sur btrfs/xfs est une amélioration possible.

## 2026-09-10 — M3 : classmap par port du cleaner + pcre2, pas de tokenizer PHP

Composer détecte les classes par `php_strip_whitespace()` (tokenizer) puis un
cleaner à état + UN pattern PCRE (possessifs, lookbehind, octets `\x7f-\xff`).
vivace porte le cleaner (en ajoutant les commentaires `#` que strip_whitespace
retirait) et exécute le pattern original via `pcre2` (décision F4). Preuve :
`PhpFileParser::findClasses` du phar sur ~50 000 fichiers des fixtures, zéro
divergence (tests/oracle_classmap.rs, noms comparés en base64).
Alternative écartée : mago-syntax (vrai parser) — plus « juste » mais pas
identique à Composer sur les cas tordus, et la fidélité prime.

## 2026-09-10 — Noms de classes en octets bruts

`symfony/cache` déclare une classe nommée d'un octet non-UTF-8 et Composer
l'écrit tel quel. Les noms circulent donc en `Vec<u8>` de bout en bout
(scanner, var_export, fichiers assemblés en octets) — le prix d'une parité
octale, trouvé par le harness sur la fixture symfony.

## 2026-09-10 — Chemin classmap absent = erreur, comme Composer

Une règle `classmap` pointant sur un chemin inexistant fait échouer Composer
(« Could not scan for classes inside … »). vivace faisait de même en silence :
aligné sur l'erreur explicite. Corollaire : le harness copie désormais le projet
complet (sans vendor/node_modules/var), pas seulement composer.json/lock.

## 2026-09-10 — `optimize-autoloader` / `classmap-authoritative` : flags OU config

Comme InstallCommand : `-o`/`-a` ou `config.optimize-autoloader` /
`config.classmap-authoritative` du composer.json (Laravel active le premier
par défaut). Le suffixe des classes d'init est le content-hash du lock
(Composer ≥ 2.2), donc déterministe et identique entre les deux outils.

## 2026-09-10 — M5 : mesurer avant d'optimiser a évité deux fausses pistes

La décomposition (bench/M5-perf.md) a montré que le « scan plus lent que
Composer » était un cache de pages froid, et que paralléliser la LECTURE est
contre-productif sur APFS (3-4× plus lent). Optimisations retenues : détection
parallèle sur le CPU (threads scoped std, pas de dépendance rayon), `realpath`
seulement sous symlink, et surtout un cache de classmap par entrée de store.

## 2026-09-10 — Cache de classmap : classes brutes par fichier, sémantique rejouée

Le cache ne stocke que la sortie de `find_classes` par fichier (ordre de
parcours) ; exclusions, dédoublonnage, filtre PSR et ambiguïtés sont rejoués à
chaque dump. Ainsi le cache ne dépend ni du projet ni des règles d'autoload,
seulement de l'entrée de store (immuable) et du sous-répertoire. Un arbre avec
symlink n'est jamais caché. Contrat assumé : vendor/ immuable entre installs
(documenté ; `VIVACE_NO_CLASSMAP_CACHE=1` pour l'ignorer). Alternative écartée :
mettre en cache la classmap finale par projet — dépendante des exclusions et
des chemins, plus fragile pour un gain identique.

## 2026-09-10 — M4 : harness en shell, Linux d'abord en conteneur, CI ensuite

Le harness reste un script (`harness/diff-vendor.sh` + `harness/boot.sh`) :
`diff -r` sur des copies complètes du projet est le test le plus fidèle qui
existe, et le suffixe d'autoloader étant déterministe (content-hash), aucune
normalisation n'est nécessaire — un harness Rust n'apporterait rien. Linux est
exercé localement dans un conteneur (`harness/linux.sh`, image php:8.4 +
rustup, code monté en lecture seule, volumes pour cargo/target/fixtures/caches)
AVANT toute CI distante, pour ne pas déboguer à l'aveugle sur des runners. Le
workflow GitHub Actions (`.github/workflows/ci.yml`, matrice ubuntu + macos)
rejoue la même chaîne.

## 2026-09-10 — Fixtures créées sans exécuter de PHP

`fixtures/make.sh` fait `create-project --no-scripts --no-plugins --no-install`
puis `install --no-plugins --no-scripts` : aucun code PHP tiers n'est exécuté
sur la machine de dev ni en CI (les scripts post-create de Laravel/Sylius sont
inutiles pour la qualification au boot). Effet de bord observé : une résolution
fraîche de sylius-standard ne boote pas sur PHP 8.5 (Doctrine ORM exige
symfony/var-exporter ou les lazy objects natifs 8.4 — incompatibilité amont) ;
la CI épingle PHP 8.4. `VIVACE_FIXTURES_DIR` permet un répertoire alternatif.

## 2026-09-10 — TLS : rustls, pas OpenSSL

Le premier build Linux a échoué sur `openssl-sys` (reqwest en `native-tls`).
Bascule sur `rustls-tls` avec `default-features = false` : aucune bibliothèque
système requise, même comportement HTTP/2, et un binaire distribuable sans
dépendre de la libssl de l'hôte (utile pour `cargo dist` en M6). Coût : la
racine de confiance est celle embarquée par rustls (webpki-roots via reqwest),
pas le magasin système — acceptable pour Packagist/GitHub ; à noter pour les
dépôts privés à CA interne (`SSL_CERT_FILE` non lu : limitation documentée).

## 2026-09-10 — Version racine devinée depuis git, comme Composer

`installed.php` divergeait sur l'entrée `root` dans tout checkout git (Composer
devine `dev-<branche>` + SHA via VersionGuesser). Port de RootPackageLoader +
guessGitVersion (branche courante, HEAD détaché → tag exact, branche de feature
→ parente la plus proche par `git rev-list`, branch-alias, COMPOSER_ROOT_VERSION),
sortie git forcée en anglais (`LANGUAGE=C`, comme GitUtil::cleanEnv — un git en
français a fait échouer le premier test). Le harness commite désormais un dépôt
git identique (même arbre, auteur et date figés → même SHA) des deux côtés.
Non porté : hg/fossil/svn.

## 2026-09-10 — Alias de branche des paquets verrouillés (fixture rector)

Premier lock extérieur passé au harness (rector-src, généré par `composer
update --no-install`) : `installed.php` divergeait sur les quatre paquets
`rector/rector-*` verrouillés en `dev-main` avec `"default-branch": true` —
Composer leur attache un AliasPackage `9999999-dev` (ArrayLoader::getBranchAlias)
et installed.php liste sa version jolie dans `aliases`. vivace ne calculait
d'alias que pour la racine, et avec une règle partielle (raw `extra.branch-alias`).
Décision : port complet de `getBranchAlias` dans `root_version::branch_alias_of`
(cible `-dev` normalisée par normalizeBranch, source comparée sans casse,
préfixes numériques compatibles, sinon `9999999-dev` si default-branch et
version non numérique ; version jolie = `(\.9{7})+` → `.x`), utilisé pour la
racine ET pour chaque paquet du lock. Oracle `tests/oracle_branch_alias.rs`
(31 configurations contre le phar, 0 divergence). rector devient la 4e fixture
figée (squelette minimal : manifestes, lock, points d'entrée d'autoload) ; il
couvre aussi les deux plugins extension-installer et `platform-check: false`.

## 2026-09-10 — composer/installers émulé nativement (v0.2)

Premier plugin de layout émulé. Le chemin d'un paquet n'est jamais deviné :
`layout::resolve` ne s'active que si le plugin est verrouillé, **autorisé**
par `config.allow-plugins` (port des trois formes de PluginManager : booléen,
motifs `packageNameToRegexp`, fusion avec `$COMPOSER_HOME/config.json` ;
non listé → fallback, car Composer non interactif refuse), et **porté** : une
table par tag 2.0.0…2.3.0 (`assets/installers/<tag>.json`, générées par
`tools/gen-installers-table.php` par réflexion sur la source, jamais
recopiées ; la logique Installer/BaseInstaller est identique sur tout 2.x,
seules les tables bougent). 58 frameworks « table seule » sont émulés
(WordPress, Drupal, Laravel, Magento, Moodle…) ; les 38 qui surchargent
`inflectPackageVars`/`getLocations`/`getInstallPath` (CakePHP, Grav,
October/Winter, Shopware, Mautic, Matomo, MediaWiki, ProcessWire…) restent en
fallback avec un message nominatif. Refus délibérés (→ fallback) : cible
absolue, hors projet, racine du projet, sous vendor/, deux paquets sur la même
cible, cible contenant une autre cible, `installer-paths`/`installer-name`
hors schéma, variable de template inconnue.

Vérité : un oracle de bout en bout (`tests/oracle_installers.rs`) construit
un Composer avec le vrai plugin activé et interroge
`InstallationManager::getInstallPath` (dispatch réel : `supports` faux →
LibraryInstaller → vendor/) puis `findShortestPath` — 665 chemins comparés,
0 divergence ; la fixture `wordpress` (wpackagist + roots/soil + un dépôt
`package` avec `bin`) est comparée **projet entier** contre Composer avec le
plugin actif, et `harness/removal.sh` vérifie qu'un paquet retiré du lock
disparaît au bon endroit, dans et hors vendor/.

État du plugin : Composer le charge depuis installed.json
(`PluginManager::loadInstalledPlugins`) et l'installe en premier dans la
transaction. vivace n'émule que les états où installed.json et le lock
concordent ; un plugin présent d'un seul côté (ajouté à un vendor existant,
retiré, ou en require-dev avec `--no-dev`) alors qu'un paquet a un type que
le plugin prendrait → fallback nominatif (« let Composer handle this
transition »). `--no-plugins` désactive l'émulation comme chez Composer.
Trouvé par la revue indépendante, pas par le harness (qui installe toujours
à partir de zéro).

Conséquences dans le code : `Filesystem::findShortestPath(Code)` et
`normalizePath` sont désormais des ports exacts dans vivace-core (l'ancienne
approximation de vivace-autoload est remplacée) ; les proxies bin calculent
leurs chemins relatifs au lieu de `../` codé en dur ; la suppression d'un
paquet retiré suit la règle de LibraryInstaller (chemin recalculé avec la
config courante, égal à l'ancien install-path, sinon fallback ; répertoire
effacé = `getPackageBasePath`, donc `vendor/<name>` sans le target-dir ;
parent vide supprimé, jamais la racine du projet). La racine du projet est
absolutisée dès la CLI : un `--working-dir` relatif produisait des proxies
bin à chemin absolu (régression trouvée par la revue).

## 2026-09-10 — Extraction : strip du dossier racine seulement s'il est unique

`extract.rs` retirait le premier composant de chaque entrée sans condition —
un zip à racine multiple (fichier + dossier, ou deux dossiers) perdait ses
fichiers de premier niveau en silence. Règle d'`ArchiveDownloader::install`
portée : strip ssi exactement une entrée de premier niveau et que c'est un
répertoire (`.DS_Store` ignoré). Trouvé par la méta-analyse du plan v0.2,
avant qu'un dépôt `package` ne le déclenche.

## 2026-09-10 — Drift en deux étages, CI épinglée

`ci.yml` teste désormais `composer:2.10.3` (la référence vendorée) pour être
déterministe. `drift.yml` (hebdomadaire + manuel) interroge le dernier stable
et le snapshot : étage 1, `harness/drift-reference.sh` diffe chaque fichier
de docs/reference/ avec son jumeau dans le phar (signal nominatif en
secondes) ; étage 2, tests + harness complet. Un échec ouvre ou commente une
issue `drift`. Le même script tourne en fin de CI contre le Composer épinglé
(garde-fou contre une référence modifiée à la main).

## 2026-09-10 — GitHub Action à la racine du dépôt

`action.yml` (composite) plutôt qu'un dépôt `setup-vivace` séparé : versionné
par les tags de release, `uses: Adelagric/vivace@v0.2.0` installe exactement
cette version (défaut `github.action_ref`, jamais « latest » depuis une
action ; un ref qui n'est pas un tag est refusé). `install.sh` authentifie la
requête API avec `GITHUB_TOKEN` quand il existe (limite anonyme partagée sur
les runners). `action-test.yml` teste l'action contre le binaire du commit
courant (`binary:`), puis un vrai `vivace install` réseau de la fixture
Laravel. L'attestation de provenance des binaires est notée pour plus tard :
le README dit « sha256-verified download », pas « verified binary ».

## 2026-09-11 — Doubles : égalités exactes arrondies au chiffre pair (dtoa)

Le property test a tiré `-2124202659384827.2` (double exact `…27.25`, à
mi-chemin entre « .2 » et « .3 ») : PHP (`zend_gcvt` mode 0 = dtoa) arrondit
au chiffre pair, `{:e}` de Rust vers le haut. Correctif dans `phpjson` :
recalcul des mêmes n chiffres par le formatage à précision fixe de Rust
(exact, demi-pair), gardé s'il round-trippe encore. Régression déterministe
figée dans `prop_phpjson_oracle.rs`, 400 cas rejoués sans divergence. Second
piège de formatage trouvé par le même test après `float_roundtrip` : la
parité à l'octet sur les flottants ne se devine pas, elle se fuzze.

## 2026-09-11 — drupal/core-composer-scaffold émulé nativement (v0.3)

Fait vérifié : `--no-scripts` ne coupe que les scripts du composer.json
racine ; les écouteurs des plugins s'exécutent, et le scaffold écrit
web/index.php, .htaccess, sites/default/default.settings.php,
web/autoload.php, autoload_runtime.php — ignorés par git dans un projet
Drupal type. Sans émulation, `vivace install` ne donne pas un Drupal qui
démarre. Port complet dans `scaffold.rs` : paquets autorisés (implicites +
récursifs, racine en dernier), opérations replace/append/skip avec surcharge
entre paquets, `checkUnchanged`, fichiers autoload de référence (sauf si
trackés par git), `.gitignore` (règle exacte : option, sinon dépôt git ET
`vendor` ignoré), et `preAutoloadDump` (entrées de classmap Symfony/PSR +
`vendor/drupal/DrupalInstalled.php`, hash xxh3 des noms uniques triés —
d'où la seule dépendance nouvelle, `xxhash-rust`, égalité vérifiée avec
`hash('xxh3')`).

Version du plugin : **empreinte de la source** (sha256 des PHP hors tests)
plutôt qu'un tag — le cœur est identique de 10.3.0 à 12.0.0-alpha1, seules
trois fonctionnalités s'ajoutent (preAutoloadDump en 11.3.0, hash trié en
11.3.4, autoload_runtime.php en 11.4.0) : 11 empreintes, 120 versions, un
port paramétré par profil. Refusé (fallback) : 11.3.0–11.3.3 (hash dans
l'ordre de la transaction Composer, non reproductible), `symlink: true`,
destination existante qui n'est pas un fichier régulier (Composer
l'effacerait récursivement), source hors du paquet, destination hors du
projet, et une copie installée du plugin dont la source diffère de celle du
lock (Composer exécute alors l'ancien Handler avec le nouveau Plugin — cas
réel de toute montée de version du cœur, testé par harness/transitions.sh).
Le plan est calculé après le fetch et **avant** toute suppression : un
refus laisse vendor/ intact. `core-project-message` et `core-recipe-unpack`
sont BENIGN (événements écoutés cités dans scope.rs).

Vérité : oracle bout en bout `tests/oracle_scaffold.rs` (15 mini-projets,
Composer avec le plugin contre Composer `--no-plugins` + le port, arbres
entiers comparés, trois versions du plugin) ; fixture `drupal` comparée
projet entier ; `harness/removal.sh` et `harness/transitions.sh`.
Non-objectif assumé : `cweagans/composer-patches` (modifie les dists →
clé de store à définir), chantier suivant.

## 2026-09-11 — Deux écarts découverts par la fixture Drupal, hors scaffold

- `installed.json` : `installation-source` n'existe pas pour un metapackage
  (ArrayDumper n'écrit la clé que si une source a été choisie) — première
  fixture avec un metapackage (drupal/core-recommended).
- `include_paths.php` (paquets PEAR à `include-path`) : Composer l'écrit dans
  l'ordre d'achèvement de ses extractions asynchrones — trois
  `composer install` successifs, trois ordres — et `composer dump-autoload`
  le réécrit dans l'ordre d'installed.json. vivace produit ce dernier ; le
  harness compare ce seul fichier trié, avec la raison dans le script.

## 2026-09-11 — Résolveur : port du solveur de Composer, et le juge d'abord (R0)

Décision utilisateur (option A du cadrage `docs/plans/v0.4-resolver.md`) :
porter `Composer\DependencyResolver\*`, `ComposerRepository`/`PoolBuilder`
et `Installer::doUpdate` plutôt qu'adopter PubGrub. Raison : quand plusieurs
solutions existent, le lock dépend de la politique **et** de l'ordre de
décision du solveur ; un autre algorithme donne un lock valide mais
différent, invérifiable — contraire à la promesse et à tout ce qui a été
prouvé jusqu'ici. Coût assumé : 6-7 000 lignes de PHP, 3 à 5 semaines,
livrées en jalons (R0 juge, R1 métadonnées, R2 solveur, R3 `update`, puis
`require`/`remove`/messages).

R0 : Packagist bouge, deux `update` à une heure d'écart ne sont pas
comparables. `tools/snapshot-packagist.sh` capture le cache Composer d'un
`update --no-install` vierge (toutes les réponses p2 chargées par
PoolBuilder), le rejoue en `file://` jusqu'à ce que Composer n'échoue plus
(paquets virtuels 404 → stub « aucune version ») et archive le lock de
référence. `harness/update.sh` injecte le dépôt local par la config globale
(composer.json intact → content-hash comparé) et vérifie d'abord que
Composer lui-même reproduit le lock de référence sur l'instantané (vrai sur
les cinq fixtures), puis compare le lock de vivace. Découverte : Composer
tolère un 404 de métadonnées en HTTP mais pas un fichier absent en
`file://`. wpackagist (protocole v1) hors périmètre.

## 2026-09-14 — Retrait de l'émulation drupal/core-composer-scaffold (licence)

Fait : `scaffold.rs` était un port déclaré, fonction par fonction, de
`drupal/core-composer-scaffold`, et `assets/scaffold/*.tpl` des copies de ses
gabarits. Le plugin est GPL-2.0-or-later ; un port est une œuvre dérivée, que
l'on ne peut pas distribuer sous MIT/Apache-2.0 avec le reste des crates et
des binaires. Options considérées : (a) isoler le port dans un crate GPL
optionnel hors des binaires publiés — mais les binaires sont l'usage
principal et un crate GPL dans le workspace reste un piège pour qui dépend
de `vivacity-core` ; (b) passer tout le projet en GPL — écarte l'intégration
dans des outils MIT (ePHPm) ; (c) réécriture en salle blanche à partir de la
seule spécification observable — coûteuse à prouver, pour 4 fixtures ;
(d) retirer l'émulation. Décision : (d). Le plugin redevient un plugin
inconnu : `install` bascule sur `composer install` avant toute écriture,
`dump-autoload` refuse (Composer exécuterait son `pre-autoload-dump`). La
fixture drupal reste dans le harness et passe par le fallback ; la source
vendorée `docs/reference/drupal-scaffold/` est supprimée avec le port. Les
versions 0.3.0 à 0.5.0 ne sont plus sur crates.io (crates `vivace*`
supprimés le 2026-09-15 — un yank ne libère pas le nom, signalé par
svandragt dans l'issue #3).

## 2026-09-14 — Renommage en vivacity (v0.6)

Fait : `svandragt/vivace` (créé le 2026-09-06, quatre jours avant ce dépôt)
est une réimplémentation de Composer en Rust avec la même promesse d'un
`vendor/` identique à l'octet, binaire `viv`, v0.11.0 et ~330 commits au
2026-09-14 ; deux `composer-rs` existent aussi. Même nom, même pitch, même
semaine : la coexistence n'exposait à rien juridiquement (pas de marque,
noms libres) mais brouillait toute recherche, toute mention et l'antériorité
est à lui. Décision : renommer en **vivacity** (libre sur crates.io pour les
quatre crates, aucun projet Rust de ce nom sur GitHub), tout d'un bloc —
binaire, crates, entrée `vivacity::run`, variables `VIVACITY_*`, dossiers de
cache, action, dépôt (redirection GitHub conservée). Les plans historiques
sous `docs/plans/` et les entrées datées de ce fichier gardent l'ancien
nom. Alternative écartée : garder le nom et compter sur la différence
d'approche (oracles différentiels, port du solveur) — invisible depuis un
nom de crate.

## 2026-09-15 — Explications d'un ensemble insoluble : l'oracle est stderr (v0.7)

Fait : `steps.sh` faisait tourner Composer avec `--quiet`, or `Installer`
écrit l'en-tête au niveau QUIET et les explications au niveau normal — un
oracle « comparer stderr » bâti sur la commande existante n'aurait vu que
l'en-tête (méta-analyse, faille 1). Décision : pour les cas `@stderr`, la
référence tourne sans `--quiet`, stderr capturé à part (sous GitHub Actions
`GithubActionError` écrit `::error ::…` sur stdout, neutralisé par
`COMPOSER_TESTS_ARE_RUNNING`), et le harness refuse un cas dont la sortie de
référence ne contient aucune ligne `    - …` (oracle aveugle). La
comparaison va de la ligne d'ancrage à la fin, à l'octet, rendu `--no-ansi`
(styles `<error>`/`<warning>`/`<info>`/`<href>` retirés, `<https://…>`
conservé). Alternatives écartées : un format « maison » (deux vocabulaires
pour un solveur, rien de vérifiable) ; déléguer les cas insolubles à
`composer update` (le port existe pour ne pas dépendre de PHP, et `require`
restaure les fichiers — un sous-processus ne le coordonne pas).
Conséquence structurelle : `SolveError::Problems` porte désormais les règles
matérialisées (le `RuleSet` ne survit pas au solveur), le `Pool` enregistre
les versions retirées par l'optimiseur (`recordRemovedVersionsForPackage`,
sauté jusqu'ici « parce que seuls les messages s'en servent ») et par les
politiques, et la sonde PHP renvoie les fichiers `.ini`.

## 2026-09-15 — Parité de stderr par défaut, et `--dry-run` par un patch du paquet racine (v0.8)

Fait : en portant `--dry-run`, la lecture de `Installer::doUpdate` a montré
que Composer imprime toujours une ligne par opération de lock (`  - Locking
…`), puis un compte de suggestions, un avertissement par paquet abandonné
et la ligne funding — rien de tout ça n'était imprimé par vivacity, et
aucun harness ne le voyait (seul le lock était comparé). Décision : le
harness `steps.sh` compare stderr **par défaut** (de la première ligne
d'opération ou d'en-tête à la fin ; `@nostderr` pour les cas rendus par
Symfony, `--no-update`, `config.lock: false`, un texte d'erreur réseau),
et la parité de sortie devient une propriété mesurée, pas une intention.
Pour `require`/`remove --dry-run`, Composer patche le paquet racine en
mémoire (`array_merge` des liens, non trié) au lieu d'écrire le fichier ;
réécrire puis restaurer composer.json aurait donné le même fichier mais un
ordre de règles différent (`--sort-packages`), donc potentiellement
d'autres décisions du solveur : `RootPatch` est appliqué au chargement de
la session (méta, faille 7). La phase d'installation d'un dry run lit le
lock *non écrit* de la résolution (`Locker::setLockData` le garde en
mémoire) — sans quoi la liste des opérations montre l'ancien lock (trouvé
par l'oracle `@dry-install`). Reporté en v0.9 : les dépôts `path`
(six sous-ports préalables identifiés par la méta : `serialize()`, un
guesser « comme git », `ArchivableFilesFinder`, glob à accolades,
`findShortestPath` avec `preferRelative`, dépôts git imbriqués dans les
fixtures).

## 2026-09-15 — Parallélisme d'E/S par plateforme (PR #4 de Luther Monson)

Fait : la PR parallélise le scan de classmap (répertoires sur rayon) et la
matérialisation store→vendor, avec des gains mesurés sur ext4/WSL2 (sylius :
`dump-autoload -o` à froid 1,03 s → 0,48 s, `install` vendor effacé 3,75 s →
1,68 s ; ×2). Mesuré ici sur APFS (M4 Max) : le même scan passe de 749 ms à
901 ms (temps système 0,47 s → 8,4 s) — c'est la contention de lecture que
M5 avait mesurée (3-4×) et qui avait fait choisir des lectures séquentielles
et une détection CPU sur threads std, sans rayon. Décision : garder le
parallélisme de la PR **là où il paie**. `vivacity_core::platform::parallel_io()`
vaut vrai sur Linux, faux ailleurs (`VIVACITY_PARALLEL_IO=0|1` pour forcer) ;
il gouverne les répertoires en parallèle du scan et le fan-out de la
matérialisation ; la détection de classes reste parallèle sur le CPU partout.
Résultat : Linux (conteneur, 4 vCPU, overlay) scan à froid 550 → 340 ms,
install vendor effacé 806 → 374 ms ; macOS scan 768 → 697 ms (la détection
sur rayon), install 604 → 623 ms (bruit). rayon est adopté (5 crates, épinglé
`=1.12.0`) : un pool global au lieu d'un jeu de threads scoped par
répertoire, et le même outil pour les deux fan-outs — l'entrée M5 « pas de
dépendance rayon » est amendée par celle-ci. Le reste de la PR est fidèle à
Composer : écriture des fichiers générés seulement si les octets changent
(`filePutContentsIfModified`), reflink FICLONE sur Linux (prévu depuis le
plan r3, jamais implémenté — désactivé pour le run dès le premier
ENOTTY/EOPNOTSUPP/EXDEV/EINVAL), cache de classmap en binaire v2
(auto-invalidation par `CACHE_FORMAT`, fichier tronqué ou octets en trop
→ rescan, compteurs lus jamais pré-alloués). Corrigé en revue : les proxies
de `vendor/bin` — `Installer::run` appelle `ensureBinariesPresence` sur
chaque paquet installé à chaque run, donc un proxy manquant d'un paquet
inchangé est recréé (un `vendor/bin` effacé revient), un proxy existant est
laissé ; `libc::FICLONE` (la constante codée en dur ne compile pas sous musl).
