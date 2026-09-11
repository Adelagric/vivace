# Changelog

All notable changes to vivace. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [SemVer](https://semver.org/) — the CLI surface and the
byte-identical-output promise are the public API.

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
