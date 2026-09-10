# Contributing to vivace

The most valuable contribution is a `composer.lock` that vivace gets wrong.
The second most valuable is a `composer.lock` that vivace refuses (falls back
to Composer) where you think it shouldn't have to.

## Report a lock that breaks (5 minutes)

1. On a project of yours, in two copies of the same checkout:
   ```bash
   composer install --no-plugins --no-scripts   # copy A
   vivace install                               # copy B
   diff -r A/vendor B/vendor
   ```
2. If there is any difference, or if vivace printed
   `this lock is outside what vivace handles natively`, open an issue with
   the **lock parity** template: `composer.json`, `composer.lock`, vivace's
   output, OS. Private packages: redact URLs/tokens, keep the structure.

That is enough: the differential harness (`harness/diff-vendor.sh`) turns a
lock into a permanent regression test. Your bug becomes everyone's test.

## Bigger pieces, roughly in order of impact

- **Plugins to emulate natively** — every install-time plugin proven harmless
  (or reproduced exactly, like `symfony/runtime`) moves a whole ecosystem off
  the fallback path. See `scope.rs` for the lists and `runtime_stub.rs` for
  the pattern; parity is proven with the harness, never assumed.
- **Windows** — `vendor/bin` `.bat` proxies (`BinaryInstaller::generateWindowsProxyCode`),
  path handling, no clonefile/hardlink assumptions.
- **Auth** — `gitlab-token`/`gitlab-oauth`; custom CAs (`SSL_CERT_FILE`) with rustls.
- **The resolver** — `composer update`/`require`. `pubgrub` exists in Rust;
  `vivace_core::constraint` is already validated against `composer/semver`
  (see `tests/oracle_semver.rs`). The judge is a byte-identical `composer.lock`.

## Ground rules that keep the project honest

- **Composer is the oracle.** Any behaviour is ported from the pinned Composer
  source (`docs/reference/`, 2.10.3) and checked against the real phar
  (`tests/oracle_*.rs`) or against a real `vendor/` (`harness/`). No porting
  from memory, no "close enough".
- **Never run PHP at install time.** Scripts and plugins are out, by design.
- **Measure before optimising** (`bench/`), and publish the losing numbers too.
- Gates before a PR: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (needs php + composer), `harness/diff-vendor.sh --with-autoloader`.
  `harness/linux.sh` runs the whole chain in a Linux container.

`DECISIONS.md` records why things are the way they are; `HANDOVER.md` lists
what is not covered yet. Read both before a large change — and add to them
with it.
