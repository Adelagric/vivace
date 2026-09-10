# M5 — Performance du scan de classmap (2026-09-10)

Machine : Mac Studio M4 Max (14 cœurs), macOS/APFS. PHP 8.5.10, Composer 2.10.3,
hyperfine 1.20 (`-N`, warmup). Binaire release, store/cache chauds, projets complets.

## Décomposition mesurée avant d'optimiser (vendor Sylius, 28 679 fichiers PHP, 133 Mo)

| étape | temps |
|---|---|
| parcours (walkdir, trié, symlinks suivis) | 160-440 ms |
| lecture séquentielle — cache de pages **froid** | 3 700 ms |
| lecture séquentielle — cache chaud | 380-420 ms |
| lecture **parallèle** 14 threads, cache chaud | 1 200-1 800 ms (**plus lente**) |
| `realpath` par fichier | 260 ms |
| `find_classes` séquentiel | 925 ms |
| `find_classes` sur 14 threads | 196 ms |

Enseignements : (1) le « 3,5 s plus lent que Composer » de M3 était un premier
accès froid — à chaud, vivace était déjà à 1,33 s contre 1,60 s ; (2) sur APFS la
lecture parallèle est contre-productive (contention noyau) : lecture séquentielle,
parallélisme réservé au CPU ; (3) `realpath` ne se justifie que sous un symlink.

## Optimisations

1. **Détection parallèle** (threads scoped std, un `ClassFinder` par thread —
   pcre2 n'est pas partageable), résultats fusionnés dans l'ordre de parcours
   (« le premier gagne » préservé). `realpath` calculé seulement sous symlink.
   → Sylius `dump-autoload -o` : 1,33 s → 0,70 s.
2. **Cache de classmap par entrée de store.** Une entrée est immuable par
   (paquet, version, ref) : les classes brutes par fichier (ordre de parcours,
   base64) sont mises en cache sous `~/Library/Caches/vivace/classmap/<sha1>.json`,
   clé = entrée + sous-répertoire + version du format. Exclusions, dédoublonnage,
   filtre PSR et ambiguïtés sont rejoués à l'identique ; un arbre contenant un
   symlink n'est pas caché. Seuls les répertoires du projet racine se rescannent.
   Parité vérifiée à cache froid ET chaud (harness --with-autoloader, 0 diff).
   Désactivable : `VIVACE_NO_CLASSMAP_CACHE=1`.

## Mesures finales (médianes)

| scénario | laravel (-o par config) | symfony | sylius |
|---|---|---|---|
| Composer no-op (M0) | 1 049 ms | 577 ms | 592 ms |
| vivace no-op — M3 / après parallélisme / **avec cache** | 449 / 233 / **54 ms** | 80 / – / **23 ms** | 87 / – / **31 ms** |
| gain final | 19× | 25× | 19× |
| Composer warm (M0, autoload inclus) | 2 695 ms | 2 405 ms | 6 007 ms |
| **vivace warm, store + cache chauds** | **196 ms** | **182 ms** | **609 ms** |
| gain | 13,7× | 13,2× | 9,9× |
| Composer `dump-autoload -o` (mesuré à chaud) | 1 591 ms | ~940 ms | 1 601 ms |
| **vivace `dump-autoload -o`** | **46 ms** | **60 ms** | **151 ms** |
| gain | 35× | ~16× | 10,6× |

Objectif M0 « no-op < 50 ms » : tenu sur Symfony/Sylius, **54 ms sur Laravel**
(rescan du projet racine `app/`+`tests/` + lecture des caches JSON) — à 4 ms
de l'objectif, noté tel quel. Le cache pèse 1,5 Mo pour 175 entrées (3 fixtures).

## Contrat et limites

- Le cache suppose `vendor/` **immuable** entre deux installs (même contrat que
  le store : pnpm-like). Un fichier édité à la main dans vendor/ ne sera pas
  rescanné tant que (paquet, version, ref) ne change pas ; Composer, lui,
  rescanne toujours. Escape hatch : `VIVACE_NO_CLASSMAP_CACHE=1` ou vider
  `~/Library/Caches/vivace/classmap`.
- Non mesuré : Linux, disques lents (la lecture froide domine alors).
