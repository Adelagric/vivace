# M3 — Autoload : parité et mesures (2026-09-10)

Machine : Mac Studio M4 Max, macOS/APFS. PHP 8.5.10, Composer 2.10.3, hyperfine 1.20.
Binaire release, store et cache chauds, projets complets (harness --with-autoloader).

## Parité

`diff -r` complet, `composer install --no-plugins --no-scripts` vs `vivace install` :

| scénario | laravel | symfony-demo | sylius |
|---|---|---|---|
| install (config du projet : Laravel = `optimize-autoloader: true`) | **0** | **0** | **0** |
| `-a` (classmap autoritaire) | **0** | — | — |
| `--no-dev` | — | **0** | — |
| `--no-dev -o` (16 611 classes) | — | — | **0** |

Fichiers couverts : `autoload.php`, `autoload_real.php`, `autoload_static.php`,
`autoload_{psr4,namespaces,classmap,files}.php`, `platform_check.php`,
`ClassLoader.php`, `LICENSE` — octet pour octet. Les trois apps bootent sur
l'autoload vivace sans jamais appeler Composer.

Détection de classes : `PhpFileParser::findClasses` (phar) sur ~50 000 fichiers
des vendors laravel+sylius → **0 divergence** (noms comparés en base64, octets
bruts compris).

Bugs de fidélité attrapés par le différentiel pendant M3 : `config.optimize-autoloader`
ignoré (Laravel l'active par défaut) ; nom de classe non-UTF-8 (`symfony/cache`)
remplacé par U+FFFD ; chemin `classmap` absent ignoré là où Composer échoue.

## Mesures (médianes hyperfine `-N`)

| scénario | laravel (6 849 cl., -o par config) | symfony (1 306 cl.) | sylius (1 278 cl.) |
|---|---|---|---|
| Composer no-op (M0, autoload inclus) | 1 049 ms | 577 ms | 592 ms |
| **vivace no-op** | **449 ms** | **80 ms** | **87 ms** |
| gain | 2,3× | 7,2× | 6,8× |
| Composer warm (M0, autoload inclus) | 2 695 ms | 2 405 ms | 6 007 ms |
| **vivace warm, store chaud** | **1 492 ms** | **371 ms** | **807 ms** |
| gain | 1,8× | 6,5× | 7,4× |
| Composer `dump-autoload -o` (M0) | 1 591 ms | 940 ms* | 1 797 ms* |
| **vivace dump-autoload** | **437 ms** (-o) | **72 ms** (normal) | **76 ms** (normal) |

\* Composer mesuré en `-o` en M0, vivace ici en mode normal (config du projet) :
non comparables directement.

## Lecture honnête

- **Le scan de classmap optimisé est le point faible.** Sur Laravel, il coûte
  ~430 ms à chaque install (Composer rescanne aussi, ~1,6 s) : le no-op
  retombe à 449 ms, au-dessus de l'objectif < 50 ms fixé en M0 — cet objectif
  n'est tenu (11-18 ms) que sans régénération d'autoload optimisé.
  Sur Sylius `--no-dev -o` (16 611 classes), notre scan prend ~3,5 s là où
  Composer met ~1,8 s en `-o` : **plus lent que Composer**. Causes : scan
  mono-thread, un `realpath` par fichier, pcre2 sur chaque fichier.
- **Réponse prévue (M5)** : (1) cache de classmap par entrée du store — une
  entrée est immuable par (paquet, version, ref), donc son scan l'est aussi ;
  seuls les répertoires du projet racine se rescannent ; (2) scan parallèle
  (rayon) ; (3) `realpath` du répertoire de base une fois, pas par fichier.
  Attendu : no-op Laravel sous 50 ms, `-o` Sylius bien sous Composer.
- Non mesuré : cold réseau, Linux (hardlinks).
