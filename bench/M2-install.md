# M2 — `vivace install --no-autoloader` : parité et mesures (2026-09-09)

Machine : Mac Studio M4 Max, macOS/APFS. PHP 8.5.10, Composer 2.10.3, hyperfine 1.20.
Binaire : `cargo build --release`. Cache zip Composer chaud, store vivace chaud.

## Parité (harness/diff-vendor.sh)

`diff -r` complet entre le vendor/ de `composer install --no-plugins --no-scripts
--no-autoloader` et celui de `vivace install --no-autoloader`, sur copies nues :

| fixture | paquets | fichiers | diff |
|---|---|---|---|
| laravel | 109 | ~10k | **0 ligne** |
| symfony-demo | 153 | ~15k | **0 ligne** (+ `autoload_runtime.php`, feature r2) |
| sylius | 276 | 40 572 | **0 ligne** (+ `autoload_runtime.php`) |

Fichiers d'état (`installed.json`, `installed.php`, `InstalledVersions.php`) et
tous les proxies `vendor/bin` : octet-identiques. Les trois apps **bootent**
sur le vendor vivace après `composer dump-autoload --no-scripts` (Laravel
13.31.0, Symfony 8.1.0, Sylius/Symfony 7.4.18).

Bugs de fidélité attrapés par le différentiel pendant M2 (tous corrigés) :
chemins raccourcis `./pcre` pour le namespace `composer/*` ; `replace`/`provide`
du composer.json racine absents d'installed.php ; `target-dir` (legacy PSR-0,
`payum/core` → `vendor/payum/core/Payum/Core`) ignoré ; strip du dossier racine
transformant un `..` interne en évasion (zip-slip) — trouvé par test unitaire.

## Mesures (médianes hyperfine, `-N`)

| scénario | laravel | symfony | sylius |
|---|---|---|---|
| Composer no-op (M0) | 1 049 ms | 577 ms | 592 ms |
| **vivace no-op** | **11 ms** | **14 ms** | **18 ms** |
| gain | 95× | 41× | 33× |
| Composer warm `--no-autoloader` (M0) | 2 348 ms | 2 235 ms | 5 739 ms |
| **vivace warm, store chaud** | **158 ms** | **181 ms** | **606 ms** |
| gain | 14,9× | 12,3× | 9,5× |
| vivace premier install (store froid, zips en cache) | 890 ms | 920 ms | 3 210 ms |
| gain | 2,6× | 2,4× | 1,8× |

Objectifs M0 (bench/M0-profil.md) : no-op < 50 ms → **tenu ×3 à ×5** ; warm
≥ 3× (≥ 5× visé sur sylius) → **tenu, 9,5-15×** — sans autoload encore
(M3 ajoutera le dump `-o`, qui coûte 0,9-1,8 s à Composer).

Non mesuré / non promis : cold réseau (dépend de Packagist) ; Linux (clone en
hardlinks au lieu de clonefile — à mesurer sur un runner ext4 en M4/CI).
