# vivace

[![ci](https://github.com/Adelagric/vivace/actions/workflows/ci.yml/badge.svg)](https://github.com/Adelagric/vivace/actions/workflows/ci.yml)

A fast, drop-in replacement for `composer install`, written in Rust.

vivace reads your `composer.json` and `composer.lock`, downloads the same
dists, and produces a `vendor/` that is **byte-for-byte identical** to what
Composer 2 produces — packages, `vendor/bin` proxies, `installed.json`,
`installed.php`, and the full autoloader (`autoload_static.php`,
`platform_check.php`, …). It just does it faster, because it never starts
PHP, extracts every package once into a content-addressed store and clones
it into `vendor/` (APFS `clonefile`, hardlinks elsewhere), and caches class
maps per store entry.

```
composer install       →  vivace install
composer dump-autoload →  vivace dump-autoload
```

Same flags where they matter: `--no-dev`, `-o/--optimize-autoloader`,
`-a/--classmap-authoritative`, `--no-autoloader`, `--ignore-platform-reqs`,
`--ignore-platform-req=…`. `config.optimize-autoloader` and
`config.classmap-authoritative` are honoured like Composer does.

## Numbers

Mac Studio M4 Max, macOS/APFS, PHP 8.5, Composer 2.10.3, hyperfine medians,
warm caches. Full methodology and raw data in [`bench/`](bench/).

| scenario | Laravel (109 pkgs) | Symfony demo (153) | Sylius (276) |
|---|---|---|---|
| `install`, nothing to do | **54 ms** vs 1 049 ms | **23 ms** vs 577 ms | **31 ms** vs 592 ms |
| `install`, `vendor/` deleted, store warm | **196 ms** vs 2 695 ms | **182 ms** vs 2 405 ms | **609 ms** vs 6 007 ms |
| `dump-autoload -o` | **46 ms** vs 1 591 ms | **60 ms** vs ~940 ms | **151 ms** vs 1 601 ms |

On Linux (GitHub `ubuntu-latest`, 4 vCPU, ext4 — where Composer itself is
faster than on APFS), the same script measures 12-16× on no-op installs, 5-9×
on warm installs and 10-13× on `dump-autoload -o`; see [`bench/M6-linux-ci.md`](bench/M6-linux-ci.md)
for the full table, produced by [`bench/ci-bench.sh`](bench/ci-bench.sh) on
every push. First install on a machine (store cold, zips in Composer's cache):
2-3× faster than Composer. Cold network: not benchmarked — that one is up to
Packagist.

## How it stays honest

Every claim above comes from a differential harness, not from unit tests
alone: [`harness/diff-vendor.sh`](harness/diff-vendor.sh) runs `composer install`
and `vivace install` on the same projects and `diff -r`s the two `vendor/`
trees. It passes with **zero differences** on all five fixtures (Laravel, the
Symfony demo, Sylius, rector-src, and a WordPress project laid out by
`composer/installers` — compared as a whole project, since packages live
outside `vendor/`), with and
without the autoloader, in normal, `-o`, `-a` and `--no-dev` modes, with a
cold and a warm classmap cache. Class detection was checked against
Composer's own `PhpFileParser::findClasses` on ~50 000 real PHP files. Every
generated file is a port of the pinned Composer 2.10.3 source — see
[`DECISIONS.md`](DECISIONS.md) for what was decided and why, and
[`HANDOVER.md`](HANDOVER.md) for what is *not* covered yet.

CI is pinned to Composer 2.10.3 so it stays deterministic; a weekly
[drift job](.github/workflows/drift.yml) reruns everything against the latest
stable and the snapshot, diffing the vendored reference files first so a
failure names the exact port that moved.

## composer/installers

Projects that place packages outside `vendor/` through `composer/installers`
(WordPress plugins and themes, Drupal modules, Magento 1, Moodle, …) are
handled natively when the plugin is locked at a version between 2.0.0 and
2.3.0, allowed by
`config.allow-plugins`, and its framework only uses the plugin's location
table (58 of the 96 frameworks, WordPress and Drupal included). The path
logic is a port of the plugin checked against the real plugin on 665 cases
(`installer-paths` by type, name and vendor, `installer-name`,
`installer-disable`, unsupported types → `vendor/`). Frameworks with custom
naming logic (CakePHP, Grav, October/Winter, Shopware, Mautic, Matomo, …) and
anything vivace would have to guess (absolute or out-of-project targets, two
packages on one path) fall back to Composer with a message naming the reason.

## What vivace does not do (v1)

- **Resolve dependencies.** No `update`, no `require`: you need a
  `composer.lock`. (A PubGrub-based resolver is the natural next step.)
- **Run scripts or plugins.** vivace never executes PHP. `symfony/runtime` is
  emulated natively (its `autoload_runtime.php` stub); a short list of plugins
  proven harmless at install time (`symfony/flex`, `php-http/discovery`,
  `phpstan/extension-installer`, …) is installed as plain libraries with a
  notice. Post-install scripts such as Laravel's `package:discover` are yours
  to run afterwards.
- **Anything it isn't sure about.** Other layout-changing plugins
  (`composer-patches`, `installers-extender`, core scaffolders), unknown
  plugins, source-only packages, `composer/installers` cases outside the
  emulated set: vivace detects them *before* touching `vendor/` and `exec`s
  the real `composer install` instead (opt out with `--no-fallback`). You
  never get a silently wrong `vendor/`.
- Windows, `gitlab-token` auth, root package version detection from hg/svn/fossil (git is ported).

## Install

Prebuilt binaries (Linux x86_64/arm64, macOS arm64/x86_64), checksum-verified:

```bash
curl -fsSL https://raw.githubusercontent.com/Adelagric/vivace/main/install.sh | sh
```

Or with `cargo binstall vivace`, or from source (`cargo install --path crates/vivace`).

### In GitHub Actions

```yaml
- uses: Adelagric/vivace@v0.2.0   # installs exactly v0.2.0, sha256-verified download
- run: vivace install
```

The action caches the vivace store and Composer's dist cache between runs
(`cache: false` to opt out) and needs no Composer on the runner as long as
the lock stays inside what vivace handles natively; keep Composer installed
if you want the fallback.

## Development

```bash
fixtures/make.sh        # once: creates and qualifies the five fixture projects (php + composer needed)
cargo test              # unit tests + differential tests against the real Composer phar
harness/diff-vendor.sh --with-autoloader
harness/removal.sh      # packages dropped from the lock disappear like with Composer
harness/boot.sh
harness/drift-reference.sh   # docs/reference/ still matches the installed Composer
harness/linux.sh        # the whole chain in a Linux container (Docker)
```

## License

MIT or Apache-2.0, at your option.
