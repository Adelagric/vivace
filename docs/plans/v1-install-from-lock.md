# vivace — plan v1 : `install` depuis composer.lock

Statut : **révision 8** — dépôt public (github.com/Adelagric/vivace), CI GitHub Actions verte sur Linux x86_64 et macOS (2026-09-10) ; restent la release v0.1.0 (binaires) et l'annonce, toutes deux sur feu vert utilisateur. Historique : M0→M5 + M4 terminés localement (Linux en conteneur : chaîne complète verte, copie et hardlinks ; réseau réel via rustls ; fixtures figées ; CI écrite, non exécutée à distance). Reste M6 : dépôt public, premier run CI, benchmarks consolidés, annonce (feu vert utilisateur). Historique : M0→M5 terminés (M5 : détection parallèle + cache de classmap par entrée de store ; no-op 23-54 ms, warm 9,9-13,7×, dump -o 10-35× — bench/M5-perf.md). Reste M4 (harness formel, CI Linux) et M6 (sortie publique). Historique : M0→M3 terminés : `vivace install` complet (autoload normal, -o, -a, --no-dev) produit un vendor/ identique à Composer sur les 3 fixtures (harness --with-autoloader), les apps bootent sans Composer. r5 = découvertes M3 : noms de classes en octets bruts, config optimize-autoloader honorée, chemin classmap absent = erreur. Suite : M4 harness formel/CI Linux, M5 benchmarks, M6 sortie. Historique : M0, M1, M2 terminés (voir HANDOVER.md, bench/M2-install.md : parité vendor/ 0 diff × 3 fixtures, no-op 11-18 ms, warm 9,5-15×). r4 = découvertes M2 : `target-dir` legacy, chemins `./x` du namespace composer/*, replace/provide du root. Suite : M3 autoload. M0 terminé (mesures dans `bench/M0-profil.md`). r1 = intégration méta-analyse (`[F#]`) ; r2 = allowlist de plugins émulés ; r3 = **architecture store + clonefile** (le spike a montré que l'extraction naïve ne gagne rien : 1,0-1,8×, goulot = I/O metadata ; le clone par paquet mesure 8× sur la pose des fichiers).

## Cadrage

**Problème.** `composer install` est lent (bootstrap PHP, génération d'autoload, extraction) ; en CI c'est souvent l'étape la plus lente d'un projet PHP. Aucun port Rust sérieux n'existe (septembre 2026 : quatre embryons < 10 stars, aucun leader). Le précédent uv/pip prouve la demande et le playbook.

**Proposition.** Un binaire Rust unique, `vivace`, substituable à Composer pour `install`, interopérable au niveau des *données* : `composer.json`, `composer.lock`, API/CDN Packagist, structure `vendor/`, cache Composer.

**Principe d'adoption clé `[F5]` : jamais d'échec sec.** Quand un lock contient un cas hors scope (plugins modifiant le layout, `installer-paths`, source-only…), vivace **délègue à `composer install` réel** (`exec`), par défaut (opt-out `--no-fallback` pour les benchmarks). Substituable sans audit préalable, comme uv face à pip.

**Principe de sûreté `[F3]` : fail-fast, pas de vendor silencieusement faux.** Sans Composer disponible pour le fallback, un lock hors scope → **erreur explicite** listant les causes (jamais un simple warning), sauf acquittement par flag. Un warning n'est acceptable que pour ce qui ne change pas le contenu du vendor (ex. hash désynchronisé — comportement Composer, acté comme spec `[F10]`).

**Contraintes.**
- Un `vendor/` produit par vivace doit être fonctionnellement indistinguable d'un `vendor/` Composer (autoload, `installed.php`/`InstalledVersions.php` compris).
- Le chemin rapide n'exécute jamais de PHP. `--scripts` `[F1]` : les événements sont délégués à `composer run-script <event>` (Composer requis pour ce flag — documenté) ; les callbacks statiques (`Vendor\Class::method`) ne sont **pas** exécutables hors runtime Composer, donc jamais émulés.
- Cibles v1 : macOS + Linux. Windows différé.

**Non-objectifs v1.** Résolution (`update`/`require`), exécution des plugins, custom installers, install depuis `source`, `create-project`, Windows. Hors scope ≠ hors service : couvert par le fallback.

**Critères de succès observables.**
1. Sur 3 fixtures réelles **qualifiées à l'oracle** `[F2]` (chacune doit démarrer après `composer install --no-plugins --no-scripts` — Laravel frais, Symfony demo, un gros projet type api-platform/monorepo Symfony ; PAS Magento, écosystème plugins-dépendant), `vivace install` sur checkout nu passe le harness différentiel et l'app démarre (tests du framework verts).
2. Benchmark honnête vs Composer 2 (hyperfine, froid/chaud, réseau contrôlé, p50 ≥ 10 runs). **Objectifs fixés par les mesures M0** (`bench/M0-profil.md`) : no-op < 50 ms (> 10×) ; warm store chaud avec `-o` ≥ 3× (≥ 5× visé sur sylius) ; premier install ≥ parité ; cold mesuré sans promesse.
3. Idempotence (2e run = no-op rapide) et robustesse à l'interruption (kill -9 → re-run répare).

## Décisions techniques

| Sujet | Choix | Raison |
|---|---|---|
| CLI | `clap`, `install` avec les flags principaux de Composer ; promesse limitée à « mêmes flags, code retour 0/≠0 » `[F10]` | personne ne parse la sortie texte de Composer |
| Plugins émulés (allowlist) | **r2** : `--no-plugins` casse le boot de tout Symfony moderne — `vendor/autoload_runtime.php` est généré par le plugin `symfony/runtime` (stub déterministe ~30 lignes, options `extra.runtime`). v1 émule nativement une allowlist courte de plugins ubiquitaires et déterministes, en commençant par `symfony/runtime` (test de drift contre le plugin réel, comme InstalledVersions). Les autres plugins → fallback | sans ça, tout l'écosystème Symfony partirait en fallback et vivace ne servirait qu'à Laravel |
| HTTP | `reqwest` + `tokio`, HTTP/2 | fan-out massif, pattern uv/pnpm |
| Extraction | crate `zip` sur `spawn_blocking` ; **sanitisation zip-slip** (entrées `../`, symlinks absolus) + tests archives hostiles `[F7]` | sécurité de base d'un package manager |
| **Store (r3)** | extraction **une fois** par (paquet, version, dist-ref) dans un store local adressé par contenu (`~/.cache/vivace/store/`), puis **clone par paquet vers vendor/** : `clonefile(2)` sur macOS/APFS, reflink `FICLONE` sur btrfs/xfs, fallback hardlink (ext4/runners CI) puis copie ; mesuré 5,8 s → 0,73 s sur sylius | c'est le différenciateur structurel — Composer ré-extrait à chaque install et ne peut pas adopter ce modèle sans casser son architecture |
| Intégrité | `shasum` du lock vérifié quand présent, **au téléchargement et à la relecture du cache** `[F7]` | le cache partagé avec Composer double l'exposition |
| Classmap | pattern PCRE de `composer/class-map-generator` réutilisé **à l'octet près via crate `pcre2` mode bytes** `[F4]` (le pattern utilise possessifs + lookbehind + octets non-UTF-8, indisponibles dans `regex`) ; migration éventuelle vers `regex::bytes` seulement si prouvée équivalente par le différentiel | fidélité d'abord, perf ensuite |
| `InstalledVersions.php` | **vendoré depuis un tag Composer épinglé** + test de drift qui diffe contre le Composer de l'oracle `[F8]` ; gestion des `aliases` du lock dans `installed.php` `[F8]` | fichier copié, pas généré — il évolue avec Composer |
| Content-hash | ré-encodage JSON identique à `JsonFile::encode(..., 0)`, `ksort` **premier niveau seulement** → `serde_json` avec `preserve_order` `[F10]` ; mismatch → warning (= comportement Composer) | parité exacte, risque dégonflé |
| Écritures | temp + rename **partout** : packages, fichiers d'état, cache `[F9]` ; non-garantie sur installs concurrents documentée (comme Composer) | crash-safety, interop cache |
| Cache | layout Composer (`COMPOSER_CACHE_DIR`, `files/<vendor>/<pkg>/<sha1-url>.zip`) | cache partagé bidirectionnel |
| Licence | double MIT / Apache-2.0 | standard Rust |
| Versioning | hors v1 ; sous-ensemble minimal pour le platform-check, testé contre `composer/semver` | `semver-php` (crate) non éprouvé |

## Architecture (workspace cargo, dégonflée `[F10]`)

```
vivace/
├─ crates/
│  ├─ vivace/            # binaire : clap, orchestration, détection hors-scope → fallback exec composer
│  ├─ vivace-core/       # manifestes (json/lock, content-hash), platform-check, fetch (cache/auth/
│  │                      #   retries/redirects), install (transaction, extraction, bin proxies,
│  │                      #   installed.*) — frontières internes en modules, pas en crates
│  └─ vivace-autoload/   # dump normal / -o / -a, pcre2 classmap, autoload_*.php, suffixe, platform_check.php
├─ harness/              # harness différentiel (outil de dev, pas un crate publié)
└─ docs/plans/
```

## Jalons

**M0 — Référence, fixtures, plafond de gain `[F2][F6]`.**
1. `brew install php composer` ; 3 fixtures qualifiées à l'oracle (`composer install --no-plugins --no-scripts` → app démarre), script `fixtures/make.sh`.
2. **Profil de décomposition** du temps de `composer install` par fixture (réseau / unzip / autoload / bootstrap) — Composer 2 télécharge déjà en parallèle, le gain est à chercher ailleurs.
3. **Spike borné (≤ 1 jour) fetch+unzip en Rust** → chiffre le plafond (Amdahl) et fixe l'objectif du critère n°2. Si le profil montre que le vrai gisement est l'autoload `-o`, ré-ordonner M2/M3 en conséquence.

**M1 — Manifestes.** Parsing lock/json, content-hash (différentiel vs oracle PHP sur N variantes), platform-check minimal, **détecteur hors-scope** (plugins affectant le layout, installer-paths, scripts-callbacks…) qui pilote le fallback `[F3][F5]`. Livrable : `vivace check` interne vert sur les fixtures.

**M2 — Fetch + store + install (r3).** Téléchargements parallèles (cache zip Composer, auth, shasum), extraction sanitisée **vers le store** (une fois par paquet@ref), **clone store→vendor** (clonefile/reflink/hardlink/copie selon FS, détection au premier run), layout vendor/, bin proxies, metapackages, `installed.json`/`installed.php` (aliases compris)/`InstalledVersions.php` vendoré, stub `symfony/runtime` (r2), transaction diff + temp/rename. Livrable : `vivace install --no-autoloader` vert au harness partiel + no-op < 50 ms.

**M3 — Autoload.** Classmap pcre2 (différentiel vs `class-map-generator` via oracle, y compris sur tout le vendor des fixtures), génération des fichiers `vendor/composer/*`, modes normal/`-o`/`-a`, tri topologique. Livrable : autoload complet vert au harness.

**M4 — Harness différentiel (juge de paix).** Diff de deux vendor/ (arborescence, contenu normalisé — suffixe autoloader, champs volatils), puis test comportemental (boot + suite de tests du framework sur le vendor vivace). CI macOS + Linux.

**M5 — Benchmarks.** hyperfine : froid/chaud, avec/sans vendor, `-o` ; Packagist réel + miroir local pour isoler CPU du réseau ; comparaison à l'objectif fixé en M0. Scripts publiés.

**M6 — Sortie publique.** README (benchmarks reproductibles, limites explicites, comportement fallback), CI, `cargo dist`, annonce r/PHP + HN — **après feu vert utilisateur** (règle : validation des posts publics).

## Tests & vérification

- Différentiel systématique, l'oracle est Composer (piloté depuis les tests Rust) : content-hash, classmap, fichiers générés, vendor complet, **drift `InstalledVersions.php`** `[F8]`.
- Property tests PhpFileCleaner (heredoc, strings imbriquées, non-UTF-8) ; **archives hostiles** (zip-slip, symlinks, zip bomb raisonnable) `[F7]`.
- Cas limites : install interrompu, lock désynchronisé (warning), 404 Packagist, disque plein, vendor lecture seule, auth manquante, **fallback exercé de bout en bout** `[F5]`, deux installs simultanés (documenté non garanti) `[F9]`.
- CI : matrix macOS/Linux avec PHP + Composer pour l'oracle.

## Risques résiduels

| Risque | Mitigation |
|---|---|
| Plafond de gain décevant sur `install` pur (le gros gain d'uv vient de la résolution, exclue de v1) | mesuré dès M0 (spike) ; si < seuil intéressant, les données disent où frapper (autoload `-o` ? bootstrap ?) et alimentent la décision v2-résolveur — décision utilisateur |
| Dérive de `class-map-generator` / `InstalledVersions.php` au fil des releases Composer | tests de drift épinglés sur tag + bump volontaire |
| Un des 4 embryons concurrents décolle | scope serré + fallback = time-to-usable court ; le harness différentiel est le différenciateur crédibilité |
| `pcre2` = dépendance C | isolée dans vivace-autoload ; plan de sortie `regex::bytes` si le différentiel le valide |

## Journal des révisions

- **r7 (2026-09-10, fin M4)** : Linux exercé en conteneur avant toute CI distante (deux bugs de portabilité trouvés : openssl-sys → rustls ; cache Composer Linux → port de Factory::getCacheDir) ; fixtures figées dans fixtures/projects (résolution fraîche de Sylius cassée en amont) ; harness/boot.sh ; CI GitHub Actions macOS+Linux.

- **r6 (2026-09-10, fin M5)** : mesure avant optimisation (lecture parallèle contre-productive sur APFS, « lenteur » M3 = cache froid) ; détection parallèle + realpath sous symlink seulement ; cache de classmap par entrée de store (contrat vendor immuable, escape hatch). Parité maintenue cache froid/chaud.

- **r5 (2026-09-10, fin M3)** : générateur d'autoload complet (port d'AutoloadGenerator + class-map-generator, pcre2), parité vendor/ 0 diff × 3 fixtures avec autoloader, en modes normal/-o/-a/--no-dev ; classmap validée sur ~50k fichiers contre l'oracle ; trois découvertes absorbées (octets bruts, config optimize, chemin classmap absent).

- **r4 (2026-09-09, fin M2)** : `vivace install --no-autoloader` fonctionnel, vendor/ identique à Composer sur les 3 fixtures (diff -r), apps bootent. Trois comportements Composer non planifiés reproduits (target-dir, chemins composer/*, replace/provide racine). Objectifs chiffrés M0 tenus avec marge (no-op 33-95×, warm 9,5-15×).

- **r3 (2026-09-09, fin M0)** : mesures faites (profil + spike + plancher clonefile). L'extraction naïve en Rust ne suffit pas (1,01× sur sylius) ; adoption de l'architecture **store adressé par contenu + clone par paquet** (8× mesuré sur la pose des fichiers) ; objectifs chiffrés du critère n°2 fixés ; M2 devient « fetch + store + clone ».
- **r2 (2026-09-09, M0)** : la qualification des fixtures a révélé que `--no-plugins` casse le boot Symfony (`autoload_runtime.php` généré par le plugin `symfony/runtime`) → ajout de l'émulation native d'une allowlist de plugins déterministes ; critère de qualification des fixtures mis à jour en conséquence (make.sh encode le contrat v1).

- **r1 (2026-09-09)** : intégration des 10 failles de la méta-analyse adversariale — fallback exec composer par défaut (F5), fail-fast sur hors-scope sans fallback (F3), `--scripts` via `composer run-script` (F1), fixture Magento remplacée + qualification oracle (F2), pcre2 pour la classmap (F4), spike de plafond en M0 + objectif chiffré (F6), sécurité extraction/cache (F7), InstalledVersions vendoré + drift test + aliases (F8), temp+rename généralisé (F9), CLI/architecture dégonflées + hash-mismatch=warning (F10).
- r0 : plan initial.
