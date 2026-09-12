# vivace

[![ci](https://github.com/Adelagric/vivace/actions/workflows/ci.yml/badge.svg)](https://github.com/Adelagric/vivace/actions/workflows/ci.yml)

`composer install` and `composer update`, reimplemented in Rust. Given the
same `composer.json` and `composer.lock`, it writes the same `vendor/` and
the same lock file as Composer 2.10.3, byte for byte. No PHP is run.

```
composer install       →  vivace install
composer update        →  vivace update
composer dump-autoload →  vivace dump-autoload
```

Flags: `--no-dev`, `-o`, `-a`, `--no-autoloader`, `--no-install`,
`--prefer-stable`, `--prefer-lowest`, `--ignore-platform-reqs`,
`--ignore-platform-req`, `--working-dir`. `config.optimize-autoloader`,
`config.classmap-authoritative`, `config.platform`, `config.allow-plugins`
and `config.lock` are read from `composer.json` the way Composer reads them.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Adelagric/vivace/main/install.sh | sh
```

Linux x86_64/arm64, macOS arm64/x86_64; the script checks the sha256.
Also `cargo binstall vivace`, or `cargo install --path crates/vivace`.

GitHub Actions:

```yaml
- uses: Adelagric/vivace@v0.4.0
- run: vivace install
```

## How it is checked

`harness/diff-vendor.sh` runs `composer install` and `vivace install` on six
projects (Laravel, the Symfony demo, Sylius, rector-src, a WordPress site
using `composer/installers`, a Drupal `recommended-project`) and `diff -r`s
the results — the whole project tree for the last two, since their files
land outside `vendor/`. `harness/update.sh` does the same for
`composer update --no-install` and `vivace update --no-install` against
frozen Packagist snapshots, comparing the lock files. Both must report no
difference; CI runs them on Linux and macOS on every push.

Underneath, each generated file and each step of the resolver is a port of
the corresponding Composer function; the source files ported are vendored
in `docs/reference/` and a CI step re-diffs them against the phar, so an
upstream change fails the build instead of drifting silently. Ports with
tricky semantics (class detection, JSON encoding, version constraints,
the solver's decision sequence) also have tests that call the real Composer
phar on the same inputs. `tests/`, `harness/`, `tools/oracle-*.php` and
`fixtures/` are all in the repo; the fixtures are downloaded by
`fixtures/make.sh`.

What is not covered is listed in [HANDOVER.md](HANDOVER.md).

## Speed

Warm `install` is bound by writing tens of thousands of small files and
rescanning class maps, not by PHP. vivace extracts each package once into a
content-addressed store, clones it into `vendor/` (`clonefile` on APFS,
hardlinks elsewhere), and caches the class map per store entry. Measured
numbers and the scripts that produce them are in [bench/](bench/); the
short version is that a no-op install takes tens of milliseconds, a warm
reinstall of Sylius under a second, and `update --no-install` on Sylius
about a third of Composer's time. Cold network installs are not faster.

## The resolver

`vivace update` uses Composer's own algorithm, ported function by function:
`PoolBuilder`, `PoolOptimizer`, `RuleSetGenerator`, the CDCL `Solver`,
`DefaultPolicy`, `LockTransaction`, `Locker`. Any other algorithm would pick
different versions on ambiguous inputs and produce a lock nobody can compare
with Composer's. The port is checked on frozen snapshots by comparing the
candidate pool, then the solver's complete decision sequence, with what
Composer computes on the same data (`tools/oracle-pool.php`).

Repositories: `composer` type only, Packagist v2 protocol and plain
`packages.json` files, local or over HTTPS. Not yet: `require`, `remove`,
partial updates (`composer update vendor/name`), `--with`, `vcs`/`path`
repositories, Composer's explanation when a set is unsolvable.

## Plugins and scripts

Scripts are never run. Three plugins are emulated and checked against the
real ones: `symfony/runtime`, `composer/installers` (versions 2.0.0–2.3.0,
frameworks that only use the plugin's path table — WordPress and Drupal
included) and `drupal/core-composer-scaffold`. A short list of plugins that
do nothing at install time (`symfony/flex`, `php-http/discovery`,
`phpstan/extension-installer`, …) is installed as plain libraries.

Anything else — other plugins, `composer/installers` cases with custom
naming, source-only packages, a plugin upgrade in progress — is detected
before `vendor/` is touched, and vivace execs the real `composer install`
instead (`--no-fallback` to make it fail). Post-install scripts such as
Laravel's `package:discover` are yours to run.

Not supported: Windows, `gitlab-token` auth, root version detection from
hg/svn/fossil.

## Development

```bash
fixtures/make.sh             # once; needs php and composer
cargo test                   # unit tests and oracles against the Composer phar
harness/diff-vendor.sh --with-autoloader
harness/update.sh
harness/removal.sh
harness/transitions.sh
harness/boot.sh
harness/drift-reference.sh   # docs/reference/ vs the installed Composer
harness/linux.sh             # everything in a Linux container
```

Design notes: [DECISIONS.md](DECISIONS.md). Plans: `docs/plans/`.

## License

MIT or Apache-2.0, at your option.
