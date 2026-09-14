# Changelog

All notable changes to vivacity (named vivace up to 0.5.0). The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [SemVer](https://semver.org/) — the CLI surface and the
byte-identical-output promise are the public API.

## [0.6.0] — 2026-09-14

### Changed
- **Renamed to vivacity.** Another Rust reimplementation of Composer,
  `svandragt/vivace`, predates this project by four days under the same
  name with the same byte-identical promise; sharing the name helps no one.
  Everything follows: the binary is `vivacity`, the crates are `vivacity`,
  `vivacity-core`, `vivacity-resolver` and `vivacity-autoload`, the library
  entry point is `vivacity::run`, the environment variables are
  `VIVACITY_*` (`VIVACE_*` is no longer read), the cache lives under
  `~/.cache/vivacity` / `~/Library/Caches/vivacity` (a `vivace` cache is
  simply left behind), the action is `Adelagric/vivacity`, the repository
  redirects from its old name. The `vivace*` crates are yanked on
  crates.io; nothing else changes in behaviour.
- All module docs, comments, error messages and `--help` texts are in
  English.

### Removed
- **`drupal/core-composer-scaffold` emulation.** It was a port of the
  plugin's source, which is GPL-2.0-or-later — a derivative work that
  cannot be distributed under MIT/Apache-2.0 with the rest of the crates
  and binaries (see NOTICE.md). The plugin is now handled like any other
  unknown plugin: `install` delegates to `composer install` before
  touching `vendor/`, and `dump-autoload` exits with code 3 while the
  plugin is locked and allowed, because Composer would run its
  `pre-autoload-dump` listener. The vendored source under
  `docs/reference/drupal-scaffold/` is removed with it. Releases 0.3.0 to
  0.5.0 are yanked on crates.io for the same reason.

## [0.5.0] — 2026-09-14

### Added
- **`vivace require`**: a port of `RequireCommand` — `vendor/name`,
  `vendor/name:^1.0`, `vendor/name ^1.0`, `--dev`, `--fixed`,
  `--no-update`, `--no-install`, `-w`/`-W`, `--sort-packages`,
  `--prefer-stable`/`--prefer-lowest`, `--update-no-dev`, the platform
  filters; a package given without constraint gets the version
  `VersionSelector` picks, the same partial update runs, then the
  constraint is rewritten from the locked version and the lock's
  `content-hash` and `stability-flags` are updated in place
  (`Locker::updateHash`); `composer.json` and `composer.lock` are restored
  (or deleted when just created) when the resolution fails. Not
  supported: `--dry-run`, `--minimal-changes`, the interactive prompts,
  the "Did you mean" search, installing from a virtual lock
  (`config.lock: false` without `--no-install`).
- **`vivace remove`**: a port of `RemoveCommand` — `composer.json` is
  edited in place (names matched case-insensitively and by `vendor/*`
  patterns, `--dev`, a package found in the other section is reported and
  left alone as in non-interactive Composer), `allow-plugins` entries of
  removed plugins are dropped, then the same partial update as Composer
  runs (`-W`, `--no-update-with-dependencies`, `--no-update`,
  `--no-install`, `--update-no-dev`, `--unused`), `composer.json` is
  restored when it fails, and exit code 2 is returned when the package is
  still installed. Not supported: `--dry-run`, `--minimal-changes`,
  `COMPOSER=other.json`.
- **Partial updates**: `vivace update vendor/name [-w|-W]` (patterns,
  `COMPOSER_WITH_DEPENDENCIES`/`COMPOSER_WITH_ALL_DEPENDENCIES`), with
  Composer's warnings; `update lock|nothing|mirrors` and temporary
  constraints are refused.
- **Dependency policies** (Composer 2.10): the pool filters that remove
  versions covered by a security advisory (`policy.advisories`, default
  on), versions on a filter list — Packagist's malware list
  (`policy.malware`, default on, `block-scope`) — and abandoned packages
  (`policy.abandoned`, default off), with the legacy `config.audit`
  keys, the global/project merge, `COMPOSER_POLICY`,
  `COMPOSER_POLICY_*_BLOCK`, `COMPOSER_NO_BLOCKING` and `--no-blocking`;
  `ignore`/`ignore-id`/`ignore-severity`/`ignore-source` rules, the
  `ignore-unreachable` behaviour, and the security-advisories API call
  when a rule needs complete advisories. A locked version flagged by a
  list is a resolution problem (exit 2) as in Composer, and `install`
  refuses a lock that pins one, with Composer's message. Until now vivace
  behaved as if `--no-blocking` were always set. `install --dry-run` runs
  the checks without writing anything; `install --no-install` is refused
  as in Composer.
- `update` keeps a metadata cache in Composer's own `cache-repo-dir`, in
  Composer's layout and byte format, revalidated with `If-Modified-Since`;
  a cache written by Composer is read by vivace and vice versa. When the
  network fails and the cache has a dated copy, the copy is used with a
  warning, like Composer's degraded mode. Metadata files of a pool batch
  are fetched in parallel (12 at a time), as `loadAsyncPackages` does.
- Ports checked against the phar on their own: `JsonManipulator` (12 102
  editing scenarios over 794 real manifests plus 725 synthetic layouts)
  and `VersionSelector` (902 names over the five snapshots, platform and
  stability variants).
- The `vivace` crate is a library with a thin binary: `vivace::run(args)`
  runs any command in-process and returns the exit code, so another
  program can embed the commands instead of shelling out. Crate metadata
  is ready for crates.io.
- `update`, `require` and `remove` honour `COMPOSER_IGNORE_PLATFORM_REQS`
  and `COMPOSER_IGNORE_PLATFORM_REQ`, as `BaseCommand` does.
- Verification: `harness/steps.sh` plays `require`, `remove`, `update`
  and `install` cases through Composer and vivace on the frozen snapshots
  and compares `composer.json`, `composer.lock` and the exit code (128
  cases, in CI); the resolver oracle and the harnesses now run with the
  policies on (5 fixtures, pools reduced by 2-23 %, locks unchanged) plus
  the synthetic `solver-policies` fixture.

### Changed
- `update` exits with code 2 when the requirements cannot be resolved,
  Composer's `ERROR_DEPENDENCY_RESOLUTION_FAILED`, instead of 1.
- `update` and `require` keep the indentation of an existing
  `composer.lock` when rewriting it, as `JsonFile::write` does.
- `install` now reads the repositories' `packages.json` (and, for the
  malware list, Packagist's summary) before installing, as Composer 2.10
  does; unreachable repositories are ignored with a warning by default.

### Fixed
- The JSON encoder now escapes U+2028 and U+2029 as `json_encode` does
  without `JSON_UNESCAPED_LINE_TERMINATORS`; a package description
  containing either would have produced a `composer.lock` differing from
  Composer's by those two characters.
- `config.allow-plugins` rules from the project now take precedence over
  the global ones in the order Composer merges them; a global `*` rule
  could win over a project rule for the same package.

## [0.4.0] — 2026-09-12

### Added
- **`vivace update`**: dependency resolution by a line-by-line port of
  Composer 2.10.3's resolver — semver (`composer/semver`), `ComposerRepository`
  (Packagist v2 protocol, `~dev` files, inline packages, `available-packages`),
  `PoolBuilder`, `PoolOptimizer`, `RuleSetGenerator`, the CDCL `Solver` with
  its learning and backtracking, `DefaultPolicy`, `Transaction` /
  `LockTransaction`, `extractDevPackages` (the second solve that splits
  `packages-dev`) and `Locker::setLockData` (`ArrayDumper`, content-hash,
  root aliases, platform requirements). The lock it writes is byte-identical
  to Composer's on the five application fixtures and four solver cases,
  from frozen Packagist snapshots; remote `composer` repositories over HTTPS
  are supported. `--no-install`, `--no-dev`, `--prefer-stable`,
  `--prefer-lowest`, `--ignore-platform-reqs`, `--ignore-platform-req`.
  Plain (Satis-style) `composer` repositories without `metadata-url`,
  repository `mirrors` and `options` (written as `dist.mirrors`,
  `source.mirrors`, `transport-options`) are handled like Composer does;
  solved packages go through the same security validation as
  Composer's `ValidatingArrayLoader::validatePackage`.
  Not yet: `require`/`remove`, partial updates, `--with`, `vcs`/`path`
  repositories, Composer's problem messages on an unsolvable set.
- Oracles for the port: `tools/oracle-pool.php` (pool, and with `--solve`
  the solver's decisions read by reflection), frozen snapshots in
  `fixtures/registry/`, `harness/update.sh`.

## [0.3.0] — 2026-09-11

### Added
- **`drupal/core-composer-scaffold` emulated natively**: a stock
  `drupal/recommended-project` installs with vivace alone — scaffolded web
  root files, `web/autoload.php` / `autoload_runtime.php`, `.gitignore`
  management, and the plugin's autoload-time additions (classmap entries,
  `vendor/drupal/DrupalInstalled.php` with its xxh3 hash). Plugin versions
  are recognised by the fingerprint of their source (116 of the 120
  releases from 10.3.0 to 12.0.0-alpha1). Checked against the real plugin on
  15 synthetic projects (whole trees compared) and on the whole Drupal fixture.
- `drupal/core-project-message` and `drupal/core-recipe-unpack` classified
  as inert at install time (installed as plain libraries).
- Frozen fixture `drupal`; `harness/transitions.sh` (a plugin upgrade in
  progress is handed to Composer before anything is written).

### Fixed
- `installed.json` no longer carries `installation-source` for
  metapackages (Composer omits it).
- `include_paths.php` (PEAR `include-path`) is written in `installed.json`
  order, the order `composer dump-autoload` produces; `composer install`'s
  own order depends on asynchronous extraction and varies between runs.

## [0.2.0] — 2026-09-10

### Added
- **`composer/installers` emulated natively**: packages placed outside
  `vendor/` (WordPress plugins/themes, Drupal modules, …) when the plugin is
  locked at 2.0.0–2.3.0, allowed by `config.allow-plugins` and its framework
  only uses the location table (58 of 96 frameworks). `installer-paths` (by
  package, `type:`, `vendor:`), `installer-name` and `installer-disable` are
  supported. Everything else falls back to Composer with a message naming
  the reason. Checked against the real plugin on 665 cases and on a whole
  WordPress project in CI.
- **GitHub Action** at the repository root: `uses: Adelagric/vivace@v0.2.0`
  installs the matching release and adds it to `PATH` (with store/dist
  caching).
- **Drift job** (`drift.yml`): weekly run against the latest Composer and the
  snapshot, diffing the vendored reference files first; opens a `drift`
  issue on failure. CI itself is now pinned to Composer 2.10.3.
- `harness/removal.sh`: packages dropped from the lock disappear like with
  Composer, inside and outside `vendor/`.
- Frozen fixtures `rector` (dev-main packages with `default-branch`,
  extension-installer plugins, `platform-check: false`) and `wordpress`.

### Fixed
- `installed.php` listed no `aliases` for locked packages on a default
  branch (`9999999-dev`) or with `extra.branch-alias`; the root package's
  alias pretty version now matches Composer too.
- Archive extraction stripped the first path component unconditionally: a
  zip whose top level is not exactly one directory lost its root files. It
  now follows `ArchiveDownloader`'s single-directory rule.
- `vendor/bin` proxies and `install-path` values are computed with an exact
  port of `Filesystem::findShortestPath` instead of an approximation.
- `install.sh` authenticates the release lookup with `GITHUB_TOKEN` when
  present (anonymous API rate limit on CI runners).

## [0.1.1] — 2026-09-10

### Changed
- CLI and installer messages in English.

## [0.1.0] — 2026-09-10

First public release: `vivace install` from `composer.lock`, byte-identical
`vendor/` (packages, bin proxies, `installed.json`/`installed.php`, full
autoloader) on Laravel, the Symfony demo and Sylius; content-addressed store
with clone/hardlink; per-store-entry classmap cache; fallback to Composer for
anything outside scope.
