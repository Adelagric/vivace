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
