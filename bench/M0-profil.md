# M0 — Profil de décomposition et plafond de gain (2026-09-09)

Machine : Mac Studio M4 Max, macOS (APFS). PHP 8.5.10, Composer 2.10.3, hyperfine 1.20.
Fixtures qualifiées (boot après `composer install --no-plugins --no-scripts` + stub `symfony/runtime`) :
laravel 76+33 pkgs · symfony-demo 99+54 · sylius 195+81 (40 572 fichiers vendor).

## Décomposition `composer install` (médianes, `--no-plugins --no-scripts`)

| scénario | laravel | symfony | sylius |
|---|---|---|---|
| bootstrap (`composer --version`) | 159 ms | 177 ms | 177 ms |
| no-op (vendor présent) | 1 049 ms | 577 ms | 592 ms |
| warm (cache chaud, vendor supprimé) | 2 695 ms | 2 405 ms | 6 007 ms |
| warm `--no-autoloader` | 2 348 ms | 2 235 ms | 5 739 ms |
| `dump-autoload -o` seul | 1 591 ms | 940 ms | 1 797 ms |
| cold (cache vide, réseau réel, 3 runs) | 8 056 ms | 13 547 ms | 18 547 ms |

## Spike Rust naïf (fetch cache chaud + extraction zip parallèle) vs composer warm-noAL

| fixture | composer | spike | gain |
|---|---|---|---|
| laravel | 1 759 ms | 1 002 ms | 1,76× |
| symfony | 2 096 ms | 1 759 ms | 1,19× |
| sylius | 5 856 ms | 5 804 ms | 1,01× |

**Conclusion négative (précieuse)** : porter l'extraction telle quelle en Rust ne gagne presque rien —
le goulot warm est la création de dizaines de milliers de petits fichiers (I/O metadata), pas PHP.
(Confirme la faille F6 de la méta-analyse.)

## Plancher du modèle store + clonefile (mesuré, sylius)

| opération | temps |
|---|---|
| extraction zip naïve (spike) | 5 804 ms |
| `cp -Rc` (clonefile par fichier, 40 572 appels) | 6 360 ms |
| `clonefile(2)` du répertoire vendor entier (1 appel) | **699 ms** |
| clonefile par paquet (271 appels, cas réel du store) | **728 ms** |

→ La pose des fichiers passe de 5,8 s à 0,73 s (**8×**) si les paquets sont extraits une fois
dans un store adressé par contenu puis clonés par projet (modèle pnpm/uv).
Linux : reflink `FICLONE` (btrfs/xfs), fallback hardlinks (ext4, runners GitHub Actions) puis copie.

## Objectifs chiffrés v1 (critère de succès n°2, fixés par ces mesures)

- **no-op** : < 50 ms (vs 577-1 049 ms → > 10×) — le cas CI le plus fréquent.
- **warm, store chaud, avec autoload -o** : ≥ 3× sur laravel/symfony, ≥ 5× visé sur sylius
  (clone ~0,7 s + autoload Rust + état ; vs 6,0 + 1,8 s Composer).
- **premier install (store froid, cache zip chaud)** : ≥ parité, extraction vers store en tâche de fond des runs suivants.
- **cold réseau** : mesuré, gain non promis (réseau-dépendant) ; HTTP/2 + pipeline extract-pendant-download à bencher en M2.
