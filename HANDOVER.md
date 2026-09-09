# HANDOVER

État par jalon, commandes de validation, risques connus, pièges. Mis à jour en continu.

## Validation (toutes plateformes de dev)

```bash
fixtures/make.sh                         # une fois : crée + qualifie laravel/symfony/sylius (php+composer requis)
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo test                               # inclut les tests oracle (php + composer dans le PATH)
cargo build --release
harness/diff-vendor.sh                   # parité vendor/ vs Composer sur les 3 fixtures
bench/profile.sh ; bench/spike-vs-composer.sh   # M0, longs
```

Les tests intégration `tests/oracle_*.rs` et `tests/fixtures_*.rs` copient le
binaire `composer` du PATH vers `$TMPDIR/vivace-oracle-composer.phar` et
appellent ses classes via `php -r`. Sans php/composer ils ÉCHOUENT avec un
message explicite (jamais de skip silencieux).

## Jalons

| jalon | état | preuve |
|---|---|---|
| M0 fixtures + profil + spike | terminé | bench/M0-profil.md |
| M1 manifestes, content-hash, scope, platform | terminé | oracle golden + différentiels + proptest, 0 divergence |
| M2 fetch + store + clone + état + proxies + CLI | terminé (`--no-autoloader`) | harness/diff-vendor.sh : 0 diff × 3 fixtures ; bench/M2-install.md |
| M3 autoload (normal, -o, -a) | à faire | — |
| M4 harness formel (normalisations, boot, CI Linux) | à faire (script shell en place) | — |
| M5 benchmarks publiés | partiel (M0/M2 mesurés) | — |
| M6 sortie publique | à faire | — |

## Ce qui N'EST PAS couvert / testé (honnêtement)

- **Autoload** : `vivace install` sans `--no-autoloader` refuse explicitement (M3).
- **Linux** : jamais exécuté. Le clone y passera par hardlinks (clone.rs) —
  chemin de code compilé mais non exercé ; perf non mesurée.
- **Réseau réel** : le fetch (reqwest, auth, retries, écriture cache) n'a été
  exercé qu'en cache chaud (`--offline`). Un test réseau contrôlé (miroir
  local) est prévu en M4/M5. L'auth `gitlab-token`/`gitlab-oauth` n'est pas
  implémentée (github-oauth, http-basic, bearer seulement).
- **Version du root package** : pas de détection VCS (Composer devine
  `dev-<branche>` + sha depuis git) → `installed.php` diverge sur `root` dans un
  checkout git. À porter (VersionGuesser) ou à normaliser dans le harness.
- **`extra.runtime` personnalisé** : routé en fallback, pas émulé.
- **Windows** : hors scope v1 (proxies .bat non générés).
- **Concurrence** : deux installs simultanés sur le même vendor/ ne sont pas
  protégés (comme Composer) ; le store, lui, est sûr (temp+rename).
- **Drift** : `InstalledVersions.php` (assets/) et le template symfony/runtime
  sont épinglés sur Composer 2.10.3 / symfony-demo actuel ; pas encore de test
  automatique de drift contre une version plus récente.

## Pièges

- `serde_json` DOIT garder `preserve_order` + `float_roundtrip` (content-hash).
- Le pattern classmap de Composer (M3) exige pcre2 (possessifs, lookbehind,
  octets non-UTF-8) — décision plan r1/F4.
- Sylius boot : `php -d memory_limit=1G` (128 Mo brew insuffisants).
- `symfony/demo` n'existe pas sur Packagist : `symfony/symfony-demo`.
- Les fixtures (`fixtures/work/`) sont gitignorées : `fixtures/make.sh` d'abord.
