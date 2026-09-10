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
