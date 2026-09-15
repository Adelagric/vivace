# Windows support

Status of the `feat/windows-support` branch (rebased onto v0.6.0, the
vivacity rename): an exploratory spike on a Windows 11 host (MSVC toolchain,
`x86_64-pc-windows-msvc`), then a hardening pass on the same host with a real
Windows PHP 8.3 and Composer 2.10. Honest inventory: what compiles, what
runs, what was verified, what is deferred.

## What works

- **The workspace compiles on Windows unmodified since before this branch** —
  every Unix-ism was already `#[cfg(unix)]`-gated. The problem was never
  compilation: the Windows side of each gate was a silent no-op (symlinks
  silently dropped, no bin proxies usable from cmd, wrong cache dirs, verbatim
  `\\?\` paths leaking into generated files). This branch writes the missing
  half.
- **`vivacity install` runs on Windows** and produces a working `vendor/`:
  packages, `vendor/bin` proxies (unixy + `.bat`), `installed.json`,
  `installed.php`, and the full autoloader. Verified with a fixture of
  `psr/log` + `monolog/monolog` + `nikic/php-parser`.
- **Byte parity on that fixture, against native Windows Composer 2.10.3**
  (PHP 8.3): the `vendor/` tree produced by vivacity on Windows is
  byte-identical — sha256 of all 429 files — to the one produced by
  Composer 2.10.3 running natively on the same host, both for the plain
  install and for `dump-autoload -o` (404-class classmap; the original
  spike compared against Composer 2.10.2 in WSL). This is one fixture, not
  an oracle suite — see "What is deferred".
- **`vendor/bin/*.bat` proxies execute.** `php-parse.bat` from a
  vivacity-built `vendor/` (nikic/php-parser v5.9.0) runs under a real
  Windows PHP 8.3 from cmd, forwards its arguments and exit code, and
  produces the tool's real output. Pinned by
  `fixtures_binproxy::bat_proxy_executes_with_a_real_php`
  (ignored by default — needs a `php` on PATH; the Windows CI leg runs it
  with `--ignored`).
- **Long paths (>260 chars) work end-to-end**, with `LongPathsEnabled=0`
  (no OS opt-in): `vivacity install` under a 307-character project root
  succeeds, the generated classmap contains classic-form (non-verbatim)
  paths, and PHP 8.3 boots the autoloader from there
  (`require vendor/autoload.php` + class instantiation). Rust ≥ 1.58
  converts to `\\?\` in its own syscalls; PHP ≥ 7.1 does the same on its
  side, and PHP `realpath()` returns the *classic* long form — which is why
  `pathutil::canonicalize` strips the verbatim prefix even beyond `MAX_PATH`
  (pinned by `canonicalize_strips_verbatim_beyond_max_path`).
- **The Composer fallback delegates on Windows.** A lock outside vivacity's
  native scope (a source-only package) finds `composer.bat` on PATH, spawns
  the real Composer 2.10, and propagates its exit code; that run also
  confirmed Composer's cache at `%LOCALAPPDATA%/Composer`, the same location
  `fetch.rs` ports.
- **Unit tests**: all `--lib` tests of the four crates pass on Windows
  (112 as of this branch), and `cargo clippy --workspace --all-targets`
  with `-D warnings` is clean.
- **A Windows CI leg** (`.github/workflows/windows.yml`, windows-latest):
  fmt (which also proves the LF checkout), clippy `-D warnings`, release
  build, all `--lib` tests, the `.bat` proxy tests (bin-compat rule, caller
  semantics, skip-existing, Composer bytes + real execution under
  setup-php), and a `--version` smoke run.
- **The Windows parity oracle** (`windows.yml`, `parity` job): the full
  six-project `harness/diff-vendor.sh` — without autoloader, then with
  autoloader cold + warm — against a pinned native Windows Composer 2.10.3
  (setup-php, PHP 8.4), 0 diff required, under Git Bash on windows-latest.
  This is the same oracle `ci.yml` runs on ubuntu/macos, and the merge bar
  this branch was missing. The only harness change it needed was resolving
  the binary as `target/release/vivacity.exe` (guarded by `uname` so WSL's
  binfmt interop doesn't pick the `.exe`).

## What the port changed

- `pathutil::canonicalize` — `std::fs::canonicalize` minus the Windows
  verbatim prefix (`\\?\C:\x` → `C:\x`), *including* beyond `MAX_PATH`: the
  classic long form is what PHP `realpath()` returns and both Rust and PHP
  re-verbatim internally (see "Long paths" above). The verbatim form is kept
  only when a re-stat cannot find the classic form (reserved names, trailing
  dot/space — shapes classic Win32 would mangle). `normalize_path` turned
  `\\?\C:\x` into `//?/C:/x`, which neither Win32 nor generated autoload
  files understand. Used at every canonicalize site in core/autoload.
- `pathutil::is_absolute_path` — port of Composer's
  `Filesystem::isAbsolutePath` (`/…`, `\…`, `C:/…`, stream wrappers). Every
  `starts_with('/')` absoluteness test missed drive-letter paths; the visible
  symptom was `$baseDir . '/C:/…/InstalledVersions.php'` in
  `autoload_classmap.php`.
- `clone.rs` — store→vendor symlink entries try a real Windows symlink
  (`symlink_file`/`symlink_dir`) and **fall back to copying the resolved
  content** when symlink creation is not permitted (it requires Developer
  Mode or `SeCreateSymbolicLinkPrivilege`). Previously the entry was silently
  dropped. A dangling link fails loudly.
- `extract.rs` — zip entries that are symlinks (unix mode `S_IFLNK`) become
  plain files whose content is the link target, after the same hostility
  checks. This matches what PHP's `ZipArchive` — i.e. what Composer itself —
  produces on Windows.
- `binproxy.rs` — `vendor/bin/<name>.bat` proxies follow **`bin-compat`**,
  exactly like `BinaryInstaller::installBinaries` (2.10.3): the value comes
  from `COMPOSER_BIN_COMPAT` else `config.bin-compat` (default `auto`), and
  the `.bat` is written **iff it resolves to `full`** — `"full"` anywhere,
  or `"auto"` on Windows or WSL (`Platform::isWindowsSubsystemForLinux`:
  `/proc/version` contains "microsoft", not in a container). A plain
  Linux/macOS install writes **no** `.bat`, matching what Composer produces
  there. (An earlier revision of this branch wrote the `.bat` on every
  platform on the theory that Composer's proxy mode does — that was wrong,
  and made `vendor/bin` on Linux/macOS *diverge* from Composer; caught by
  review with `harness/diff-vendor.sh`.) In full mode the `.bat` calls the
  neighbouring unixy proxy (`php "%~dp0/<name>"`) for a plain `php` caller;
  `determineBinaryCaller` is ported exactly — `call` for a `.bat` or `.exe`
  target (NOT `.cmd`), and a shebang keeps everything after the last `/`,
  arguments included (`#!/usr/bin/env php -dfoo` → caller `php -dfoo`), any
  non-`php` caller targeting the real binary via `findShortestPath`. An
  existing `<name>.bat` is **skipped**, not overwritten
  (`installFullBinaries`). Orphan `.bat` proxies are pruned with their unixy
  siblings; under a non-full bin-compat a leftover `.bat` is purged too.
  Verified byte-identical to Composer 2.10.3 on native Windows (full mode)
  **and** on Linux (no `.bat`, 0-diff `harness/diff-vendor.sh`), and
  executing under a real Windows PHP 8.3 (see "What works").
- `fetch.rs` — Composer-compatible Windows locations: home
  `%APPDATA%/Composer`, cache `%LOCALAPPDATA%/Composer`
  (`COMPOSER_HOME`/`COMPOSER_CACHE_DIR` still win).
- `platform.rs` — vivacity cache at `%LOCALAPPDATA%/vivacity`; PHP detection
  uses `where` instead of `which` and understands backslashed explicit paths.
- `.gitattributes` — `* text=auto eol=lf`. The generated files are compared
  to Composer byte-for-byte and Composer emits LF everywhere, including on
  Windows; with the default `core.autocrlf=true` the `include_str!` assets
  and the `docs/reference` sources checked out as CRLF, which corrupted the
  generated output. **A pre-existing Windows clone must be re-checked-out**
  (`git rm --cached -r . && git reset --hard`) to pick this up.

## What is deferred / not verified

- **The PHP-driven oracle tests (`tests/oracle_*.rs`) on Windows.** The
  `parity` job covers the diff-vendor suite (see "What works"), but the
  Rust oracle tests that spawn a PHP interpreter are exercised on Windows
  only insofar as `cargo test --lib` reaches them; the remaining harness
  scripts (`update.sh`, `steps.sh`, `removal.sh`, `transitions.sh`,
  `boot.sh`, `drift-reference.sh`) still run on ubuntu/macos only.
- **Symlink fallback semantics.** The copy fallback in `clone.rs` means a
  package whose dist contains symlinks materializes differently than on
  Unix (copies instead of links) when the host lacks the symlink privilege;
  an entry is never dropped silently, and a dangling link fails loudly
  (reviewed; the clone unit test exercises the privileged path on hosts
  that allow symlinks — the *unprivileged* fallback branch has no forced
  test). Composer-on-Windows has the same general shape (ZipArchive never
  creates links), but the exact interaction with `path`-repository symlink
  options (`options.symlink`) is unimplemented territory on this branch, as
  it already was on Unix.
- **Long paths, the remainder.** vivacity's own I/O, the generated files, and
  PHP consuming them are verified >260 (see above). Two known limits:
  `.bat` proxies under a >260 `vendor/bin` do **not** run — cmd.exe has no
  long-path support and fails loudly ("The system cannot find the path
  specified"; verified at 332 chars; Composer's `.bat` has the same limit) —
  and external tools spawned on those paths were not audited (`git` needs
  `core.longpaths`; some shells and editors still choke).
- **`vivacity update` / resolver on Windows** was compile-checked and
  unit-tested but not exercised against the network beyond what `install`
  uses (the fixture's metadata + dist downloads worked).
- **Case-insensitivity.** NTFS is case-insensitive; nothing was audited for
  packages whose paths differ only by case, or for `preg` path matching
  (`exclude-from-classmap`) against mixed-case user input.
- **Composer fallback, the remainder.** `install` delegation was exercised
  end-to-end (see above); the `require`/`remove` delegation paths share the
  same spawn plumbing but were not run against a real Windows Composer.
- **`.bat` special cases.** Execution is verified for the common shape
  (PHP target with shebang → `php` caller). The phpunit hack and a
  `.bat`/`.exe`-target package (`call` caller, `findShortestPath` over
  backslashed paths) have byte-level unit coverage but no execution test.

## What a mergeable port still needs

1. ~~The full oracle harness (diff-vendor, six projects) against a pinned
   Windows Composer on the Windows CI leg~~ — done: the `parity` job in
   `windows.yml` (see "What works").
2. `.bat` execution tests for the special cases: phpunit and a
   `.bat`-target package.
3. A decision on symlink-carrying dists and `path` repositories (copy vs
   junction vs require-privilege), with a forced test of the unprivileged
   fallback.
4. A case-insensitivity audit.
5. Re-checkout guidance for existing clones (the `.gitattributes` change).
