# M6 — Benchmarks Linux sur runner GitHub (2026-09-10)

`bench/ci-bench.sh` exécuté par `.github/workflows/bench.yml` sur ubuntu-latest
(x86_64, 4 vCPU, ext4), PHP 8.4, Composer 2.10.3, hyperfine 10 runs, caches chauds
(zips Composer, store et cache de classmap vivace). Run 34503342839.

| fixture | scénario | Composer | vivace | gain |
|---|---|---|---|---|
| laravel | noop | 995 ms | 355 ms | 2.8× |
| laravel | warm | 1700 ms | 138 ms | 12.3× |
| laravel | dump-o | 873 ms | 72 ms | 12.1× |
| symfony | noop | 481 ms | 64 ms | 7.5× |
| symfony | warm | 1525 ms | 115 ms | 13.3× |
| symfony | dump-o | 1017 ms | 91 ms | 11.2× |
| sylius | noop | 508 ms | 81 ms | 6.3× |
| sylius | warm | 3522 ms | 659 ms | 5.3× |
| sylius | dump-o | 2147 ms | 243 ms | 8.8× |

Lecture : comme anticipé, Composer est plus rapide sur ext4 que sur APFS
(rafales de petits fichiers), donc les ratios Linux sont plus modestes que
sur macOS (warm 5-13× contre 10-15×). Le no-op Laravel à 355 ms — plus lent
que son propre warm — est une anomalie à expliquer (voir le suivi ci-dessous),
pas un chiffre à présenter tel quel.

## Suivi : le no-op Laravel à 355 ms — cause trouvée

Trace de phases (`VIVACE_TRACE=1`) dans le conteneur Linux : no-op = 47 ms,
warm = 200 ms — pas d'anomalie. Sur le runner, l'ordre du script était la clé :
le `vendor/` de départ avait été posé par **Composer**, et le premier
`vivace install` a tout trouvé « inchangé » sans rien extraire → **store vide**
→ pas d'entrée de store → pas de cache de classmap → scan complet à chaque
no-op (355 ms sur 4 vCPU). Le premier warm remplit le store et tout redevient
rapide (138 ms, puis `dump -o` 72 ms).

C'est le scénario réel de toute adoption sur un projet existant. Correctif :
un paquet inchangé dont l'entrée de store manque est extrait dans le store
depuis le cache zip (sans être re-cloné, jamais via le réseau, échec
silencieux) — le run suivant profite du cache. Mesuré à nouveau sur le runner
après correctif : voir la ligne « laravel noop » du prochain tableau.
