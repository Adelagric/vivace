# vivacity

[![ci](https://github.com/Adelagric/vivacity/actions/workflows/ci.yml/badge.svg)](https://github.com/Adelagric/vivacity/actions/workflows/ci.yml)

`composer install` and `composer update`, reimplemented in Rust. Given the
same `composer.json` and `composer.lock`, it writes the same `vendor/` and
the same lock file as Composer 2.10.3, byte for byte. No PHP is run.
(Named `vivace` up to 0.5.0; see the changelog for why it moved.)

```
composer install       →  vivacity install
composer update        →  vivacity update        (also `update vendor/name [-w|-W]`)
composer require       →  vivacity require
composer remove        →  vivacity remove
composer dump-autoload →  vivacity dump-autoload
```

Flags: `--no-dev`, `-o`, `-a`, `--no-autoloader`, `--no-install`,
`--prefer-stable`, `--prefer-lowest`, `--ignore-platform-reqs`,
`--ignore-platform-req`, `--working-dir`. `config.optimize-autoloader`,
`config.classmap-authoritative`, `config.platform`, `config.allow-plugins`
and `config.lock` are read from `composer.json` the way Composer reads them.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Adelagric/vivacity/main/install.sh | sh
```

Linux x86_64/arm64, macOS arm64/x86_64, Windows x86_64 (Git Bash); the
script checks the sha256. On Windows from PowerShell:

```powershell
irm https://raw.githubusercontent.com/Adelagric/vivacity/main/install.ps1 | iex
```

Also `cargo install vivacity` (crates.io), `cargo binstall vivacity`, or
`cargo install --path crates/vivacity`.

GitHub Actions:

```yaml
- uses: Adelagric/vivacity@v0.8.0
- run: vivacity install
```

## How it is checked

`harness/diff-vendor.sh` runs `composer install` and `vivacity install` on six
projects (Laravel, the Symfony demo, Sylius, rector-src, a WordPress site
using `composer/installers`, a Drupal `recommended-project`) and `diff -r`s
the results — the whole project tree for the last two, since their files
land outside `vendor/`. `harness/update.sh` does the same for
`composer update --no-install` and `vivacity update --no-install` against
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
rescanning class maps, not by PHP. vivacity extracts each package once into a
content-addressed store, clones it into `vendor/` (`clonefile` on APFS,
hardlinks elsewhere), and caches the class map per store entry. Measured
numbers and the scripts that produce them are in [bench/](bench/); the
short version is that a no-op install takes tens of milliseconds, a warm
reinstall of Sylius under a second, and `update --no-install` on Sylius
about a third of Composer's time. Cold network installs are not faster.

## The resolver

`vivacity update` uses Composer's own algorithm, ported function by function:
`PoolBuilder`, `PoolOptimizer`, `RuleSetGenerator`, the CDCL `Solver`,
`DefaultPolicy`, `LockTransaction`, `Locker`. Any other algorithm would pick
different versions on ambiguous inputs and produce a lock nobody can compare
with Composer's. The port is checked on frozen snapshots by comparing the
candidate pool, then the solver's complete decision sequence, with what
Composer computes on the same data (`tools/oracle-pool.php`).

When no set satisfies the requirements, vivacity prints Composer's
explanation — the numbered problems with their reasons, the hints
(`Potential causes`, the php.ini list for a missing extension, the `-W`
advice) — word for word: `Problem::getPrettyString`, `Rule::getPrettyString`
and `SolverProblemsException` are ported, and `harness/steps.sh` compares
that text byte for byte with Composer's on 50 unsolvable cases (a
synthetic registry built by `fixtures/make-solver-problems.py` reaches every
reason branch the CLI can produce, plus real-registry cases).

What the commands print is Composer's text too: the `Lock file
operations` summary and its `  - Locking …` / `Upgrading` / `Removing`
lines, the suggestions count, the abandoned-package warnings, the funding
line, `install`'s `Package operations`, and `--dry-run` on `update`,
`require` and `remove` (resolution and listing, nothing written — the
install phase lists its operations from the unwritten lock). The stderr
of every `steps.sh` case is compared byte for byte from the first of
those lines to the end (187 cases).

Repositories: `composer` type only, Packagist v2 protocol and plain
`packages.json` files, local or over HTTPS. Not yet: `--with`,
`vcs`/`path` repositories, `--minimal-changes`, the audit (`--no-audit`
is the compared behaviour), the `--verbose` form of the explanations.

Composer 2.10's dependency policies are applied the same way: versions
covered by a security advisory or flagged on Packagist's malware list are
removed from the pool before resolution (`config.policy`, the legacy
`config.audit` keys, `COMPOSER_POLICY*`, `--no-blocking`), and `install`
refuses a lock that pins a flagged version. Advisories and list entries
come from the same metadata files Composer reads, so the frozen snapshots
carry them and the parity checks run with the policies on. Custom policy
lists with sources and a repository `filter.api-url` are not supported
(refused when reached).

`vivacity require` and `vivacity remove` edit `composer.json` the way Composer
does — a port of `JsonManipulator`, which rewrites only the affected keys
and keeps the file's layout — pick the same `^x.y` constraint (a port of
`VersionSelector`), then run the same partial update, and `require`
rewrites the constraint and the lock's `content-hash` from the resolved
version the way `RequireCommand` does. `harness/steps.sh` plays the same
commands through both tools on the frozen snapshots and compares
`composer.json`, `composer.lock` and the exit code. Not ported: the
interactive prompts (vivacity behaves like `--no-interaction`) and the
Packagist search behind "Did you mean …".

## Plugins and scripts

Scripts are never run. Two plugins are emulated and checked against the
real ones: `symfony/runtime` and `composer/installers` (versions
2.0.0–2.3.0, frameworks that only use the plugin's path table — WordPress
and Drupal included). A short list of plugins that do nothing at install
time (`symfony/flex`, `php-http/discovery`, `phpstan/extension-installer`,
…) is installed as plain libraries. `drupal/core-composer-scaffold` is not
emulated (its source is GPL-2.0-or-later, see NOTICE.md): a project that
uses it goes through the Composer fallback below.

Anything else — other plugins, `composer/installers` cases with custom
naming, source-only packages, a plugin upgrade in progress — is detected
before `vendor/` is touched, and vivacity execs the real `composer install`
instead (`--no-fallback` to make it fail). Post-install scripts such as
Laravel's `package:discover` are yours to run.

Not supported: `gitlab-token` auth, root version detection from
hg/svn/fossil.

## Embedding

The `vivacity` crate is a library with a thin binary on top: another program
can run any vivacity command in-process and get the binary's exit code.

```rust
// Cargo.toml: vivacity = "0.5"
let code = vivacity::run(["vivacity", "install", "--working-dir", "/srv/app"]);
```

The lower layers are separate crates (`vivacity-core`: manifests, lock,
store, installers; `vivacity-resolver`: the resolver port; `vivacity-autoload`:
the autoloader generator).

Embedding brings in vivacity's HTTP stack (reqwest over rustls). The default
feature `rustls-tls-ring` bundles the ring crypto provider — right for the
standalone binary, wrong for a host that already links another rustls
provider: Cargo unions features across the whole tree, so both providers end
up compiled in and `rustls::CryptoProvider::from_crate_features()` returns
`None`, which panics every bare `ClientConfig::builder()` in the process at
runtime. Such a host selects the provider-agnostic backend instead and
installs its own process-wide default provider before the first request:

```toml
vivacity = { version = "0.8", default-features = false, features = ["rustls-tls-no-provider"] }
```

## Development

```bash
fixtures/make.sh             # once; needs php and composer
cargo test                   # unit tests and oracles against the Composer phar
harness/diff-vendor.sh --with-autoloader
harness/update.sh
harness/steps.sh
harness/removal.sh
harness/transitions.sh
harness/boot.sh
harness/drift-reference.sh   # docs/reference/ vs the installed Composer
harness/linux.sh             # everything in a Linux container
```

Design notes: [DECISIONS.md](DECISIONS.md). Plans: `docs/plans/`.

## License

MIT or Apache-2.0, at your option. The ports are derived from Composer
and its libraries (MIT, © Nils Adermann, Jordi Boggiano, and the Composer
project) and from `composer/installers` (MIT, © Kyle Robinson Young);
the reference sources are vendored in `docs/reference/` with their
license texts — see [NOTICE.md](NOTICE.md).
