# Contributing to vivacity

The most valuable contribution is a `composer.lock` that vivacity gets wrong.
The second most valuable is a `composer.lock` that vivacity refuses (falls back
to Composer) where you think it shouldn't have to.

## Report a lock that breaks (5 minutes)

1. On a project of yours, in two copies of the same checkout:
   ```bash
   composer install --no-plugins --no-scripts   # copy A
   vivacity install                               # copy B
   diff -r A/vendor B/vendor
   ```
2. If there is any difference, or if vivacity printed
   `this lock is outside what vivacity handles natively`, open an issue with
   the **lock parity** template: `composer.json`, `composer.lock`, vivacity's
   output, OS. Private packages: redact URLs/tokens, keep the structure.

That is enough: the differential harness (`harness/diff-vendor.sh`) turns a
lock into a permanent regression test. Your bug becomes everyone's test.

The same goes for `update`, `require` and `remove`: a `composer.json` (with
or without its lock) on which `composer update --no-install` and
`vivacity update --no-install` write a different `composer.lock`, print a
different explanation of an unsolvable set, or list different operations,
is a case for `harness/steps.sh` — it replays a command through both tools
on a frozen registry snapshot and compares the files, the exit code and
stderr byte for byte.

## Bigger pieces, roughly in order of impact

- **`path` repositories** (v0.9): `PathRepository` (`sha1(json .
  serialize(options))` references, a git-like version guesser adding the
  feature branch and its parent, brace globs) and `PathDownloader`
  (symlink or `ArchivableFilesFinder` mirror). The opening cadrage is the
  meta-analysis in `docs/plans/v0.8-dry-run-path.md`.
- **`vcs` repositories** — `VcsRepository`, the git/GitHub/GitLab drivers,
  source checkouts on install.
- **Plugins to emulate natively** — every install-time plugin proven harmless
  (or reproduced exactly, like `symfony/runtime` and `composer/installers`)
  moves a whole ecosystem off the fallback path. See `scope.rs` for the
  lists and `runtime_stub.rs` for the pattern; parity is proven with the
  harness, never assumed. Ports of GPL-licensed plugins are not accepted
  (NOTICE.md).
- **Auth** — `gitlab-token`/`gitlab-oauth`; custom CAs (`SSL_CERT_FILE`) with rustls.
- **Remaining `update` options** — `--with`, `--minimal-changes`,
  `bump-after-update`, the audit, `--verbose` explanations.

## Ground rules that keep the project honest

- **Composer is the oracle.** Any behaviour is ported from the pinned Composer
  source (`docs/reference/`, 2.10.3) and checked against the real phar
  (`tests/oracle_*.rs`) or against a real `vendor/` (`harness/`). No porting
  from memory, no "close enough".
- **Never run PHP at install time.** Scripts and plugins are out, by design.
- **Measure before optimising** (`bench/`), and publish the losing numbers too.
- Gates before a PR: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (needs php + composer 2.10.3 on PATH),
  `harness/diff-vendor.sh --with-autoloader`, `harness/update.sh`,
  `harness/steps.sh` (stderr is compared on every case unless `@nostderr`;
  a case whose reference output has no operation or reason line is
  reported as blind, not green), `harness/drift-reference.sh` (every
  vendored file under `docs/reference/` identical to the phar).
  `harness/linux.sh` runs the whole chain in a Linux container; the
  Windows parity job runs on `windows-latest` in CI.
- Every ported function has its reference vendored under `docs/reference/`
  with its licence, and a row in NOTICE.md when it comes from a new
  origin. Documentation (README, HANDOVER, DECISIONS, CHANGELOG, the crate
  READMEs, `docs/`) is updated in the same change as the code.

`DECISIONS.md` records why things are the way they are; `HANDOVER.md` lists
what is not covered yet. Read both before a large change — and add to them
with it.
