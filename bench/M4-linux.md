# M4 — Linux, réseau réel, CI (2026-09-10)

## Linux (conteneur `harness/linux.sh` : php:8.4-cli-bookworm, arm64, overlayfs)

Chaîne complète exécutée dans le conteneur, code monté en lecture seule :

| étape | résultat |
|---|---|
| `cargo fmt --check`, `clippy -D warnings`, `build --release` | OK (après passage de reqwest à **rustls** : le premier build Linux échouait sur `openssl-sys`) |
| `fixtures/make.sh` (squelettes figés `fixtures/projects/`, locks committés) | 3/3 qualifiées |
| `cargo test` (unitaires + oracles PHP + proptest) | 14 suites OK |
| `harness/diff-vendor.sh` | 0 diff × 3 |
| `harness/diff-vendor.sh --with-autoloader`, cache froid puis chaud | 0 diff × 3, deux fois |
| `harness/boot.sh` | 3/3 bootent |

Découvertes Linux, toutes hors logique métier mais réelles :
- **Cache Composer** : sur Linux sans variable `XDG_*`, Composer utilise
  `~/.composer/cache`, pas `~/.cache/composer`. `Factory::getHomeDir` /
  `getCacheDir` sont maintenant portés à l'identique (docs/reference/Factory.php).
- **TLS** : rustls remplace OpenSSL (aucune dépendance système).
- **Stratégie de clone** : avec le store sur un autre système de fichiers que
  `vendor/` (volume Docker vs overlay), c'est le **repli copie** qui a été exercé
  (inodes distincts). Le chemin **hardlink** a été exercé séparément, store et
  vendor sur le même FS : inode identique, `nlink=2`, l'app boote.
- **Fixtures** : une résolution fraîche de `sylius-standard` ne boote plus
  (Doctrine ORM vs Symfony 8, incompatibilité amont du jour) → les squelettes
  et locks sont figés dans `fixtures/projects/` (5 Mo, MIT), `make.sh` ne
  résout plus rien.

### Chronos Linux indicatifs (conteneur arm64, mesures uniques, Laravel)

| scénario | vivace | Composer |
|---|---|---|
| premier install (zips en cache, store froid) | 0,35 s | — |
| no-op | 0,04 s | — |
| warm, vendor supprimé, store chaud (hardlinks) | 0,10 s | 0,87 s |

Composer est nettement plus rapide dans ce conteneur que sur macOS (0,87 s vs
2,7 s) — overlayfs/ext4 gèrent mieux les rafales de petits fichiers qu'APFS ;
les ratios macOS ne se transposent donc pas tels quels. À mesurer proprement
(hyperfine) sur un runner CI en M6.

## Réseau réel (macOS, rustls, caches vides)

`vivace install` sur Laravel avec `COMPOSER_CACHE_DIR` et `VIVACE_CACHE_DIR`
vierges : 109 zips téléchargés depuis GitHub, install complet (autoload compris)
en **3,46 s** (une seule mesure) ; Composer à froid en M0 : 8 056 ms
(médiane de 3). L'app boote ; le cache zip est alimenté (109 fichiers).
Non comparé au harness (référence disponible générée sans `app/`).

## CI

`.github/workflows/ci.yml` (ubuntu-latest + macos-latest, PHP 8.4 via
setup-php, extensions incluant exif/gd/intl) rejoue exactement la chaîne du
conteneur. **Exécutée à distance le 2026-09-10** (run #2, après un premier
échec dû aux répertoires vides non versionnés des squelettes → `.gitkeep`) :
verte sur ubuntu-latest (x86_64, 5 min 48) et macos-latest (11 min 51).
