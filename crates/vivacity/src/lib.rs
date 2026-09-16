//! vivacity: a fast, Composer-compatible installer.
//! stdout: nothing for now (reserved for machine-readable output); all
//! narrative goes to stderr. Exit codes: 0 ok, 1 runtime error,
//! 2 usage (clap), 3 out of scope with no fallback possible, 4 platform.
//!
//! The `vivacity` binary is just a call to [`run`]: another program can
//! embed the commands as they are (`vivacity::run(["vivacity", "install", …])`)
//! and get the same exit code.

mod require;

use anyhow::Context as _;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "vivacity",
    version,
    about = "Fast, Composer-compatible installer"
)]
enum Cli {
    /// Install dependencies from composer.lock (drop-in `composer install`).
    Install(InstallArgs),
    /// Regenerate the autoloader (drop-in `composer dump-autoload`).
    #[command(name = "dump-autoload", alias = "dumpautoload")]
    DumpAutoload(DumpArgs),
    /// Resolve dependencies and write composer.lock (drop-in `composer update`).
    #[command(alias = "upgrade")]
    Update(UpdateArgs),
    /// Remove packages from composer.json, then update (drop-in `composer remove`).
    #[command(alias = "rm")]
    Remove(RemoveArgs),
    /// Add packages to composer.json, then update (drop-in `composer require`).
    #[command(alias = "r")]
    Require(RequireArgs),
}

#[derive(clap::Args, Debug)]
struct RequireArgs {
    /// Packages to require: `vendor/name`, `vendor/name:^1.0`, `vendor/name ^1.0`.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Add to require-dev.
    #[arg(long)]
    dev: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    prefer_source: bool,
    #[arg(long)]
    prefer_dist: bool,
    #[arg(long)]
    prefer_install: Option<String>,
    /// Exact constraint (the version found) instead of `^x.y`.
    #[arg(long)]
    fixed: bool,
    #[arg(long)]
    no_suggest: bool,
    #[arg(long)]
    no_progress: bool,
    /// Do not update dependencies (implies --no-install).
    #[arg(long)]
    no_update: bool,
    /// Write the lock file without installing.
    #[arg(long)]
    no_install: bool,
    #[arg(long)]
    no_audit: bool,
    #[arg(long, value_name = "FORMAT")]
    audit_format: Option<String>,
    #[arg(long)]
    no_blocking: bool,
    #[arg(long)]
    no_security_blocking: bool,
    /// Run the update with --no-dev.
    #[arg(long)]
    update_no_dev: bool,
    /// Also update dependencies, except those required by the root (`-w`).
    #[arg(short = 'w', long)]
    update_with_dependencies: bool,
    /// Alias for --update-with-dependencies.
    #[arg(long)]
    with_dependencies: bool,
    /// Also update dependencies, root requirements included (`-W`).
    #[arg(short = 'W', long)]
    update_with_all_dependencies: bool,
    /// Alias for --update-with-all-dependencies.
    #[arg(long)]
    with_all_dependencies: bool,
    #[arg(short = 'm', long)]
    minimal_changes: bool,
    #[arg(long)]
    prefer_stable: bool,
    #[arg(long)]
    prefer_lowest: bool,
    /// Sort the packages of the section (also `config.sort-packages`).
    #[arg(long)]
    sort_packages: bool,
    #[arg(short = 'o', long)]
    optimize_autoloader: bool,
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    #[arg(long)]
    apcu_autoloader: bool,
    #[arg(long, value_name = "PREFIX")]
    apcu_autoloader_prefix: Option<String>,
    #[arg(long)]
    ignore_platform_reqs: bool,
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    #[arg(short = 'n', long)]
    no_interaction: bool,
    #[arg(short = 'q', long)]
    quiet: bool,
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,
    #[arg(long)]
    no_scripts: bool,
    #[arg(long)]
    no_plugins: bool,
    /// Do not generate the autoloader.
    #[arg(long)]
    no_autoloader: bool,
    #[arg(long)]
    no_fallback: bool,
    #[arg(long)]
    offline: bool,
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct RemoveArgs {
    /// Packages to remove; `vendor/*` patterns accepted.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Remove from require-dev.
    #[arg(long)]
    dev: bool,
    /// Do not update dependencies (implies --no-install).
    #[arg(long)]
    no_update: bool,
    /// Write the lock file without installing.
    #[arg(long)]
    no_install: bool,
    /// Accepted for compatibility: vivacity does not audit (yet).
    #[arg(long)]
    no_audit: bool,
    /// Run the update with --no-dev.
    #[arg(long)]
    update_no_dev: bool,
    /// Deprecated in Composer (default behaviour).
    #[arg(short = 'w', long)]
    update_with_dependencies: bool,
    /// Also update dependencies that are root requirements (`-W`).
    #[arg(short = 'W', long)]
    update_with_all_dependencies: bool,
    /// Alias for --update-with-all-dependencies.
    #[arg(long)]
    with_all_dependencies: bool,
    /// Only update the listed packages.
    #[arg(long)]
    no_update_with_dependencies: bool,
    /// Remove every locked package that nothing requires.
    #[arg(long)]
    unused: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(short = 'm', long)]
    minimal_changes: bool,
    /// Accepted for compatibility: vivacity is never interactive, does not
    /// audit and applies no blocking policy.
    #[arg(short = 'n', long)]
    no_interaction: bool,
    #[arg(long)]
    no_blocking: bool,
    #[arg(long)]
    no_security_blocking: bool,
    #[arg(long, value_name = "FORMAT")]
    audit_format: Option<String>,
    #[arg(long)]
    apcu_autoloader: bool,
    #[arg(long, value_name = "PREFIX")]
    apcu_autoloader_prefix: Option<String>,
    #[arg(short = 'q', long)]
    quiet: bool,
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,
    #[arg(long)]
    no_progress: bool,
    #[arg(long)]
    no_scripts: bool,
    #[arg(long)]
    no_plugins: bool,
    /// Do not generate the autoloader.
    #[arg(long)]
    no_autoloader: bool,
    #[arg(short = 'o', long)]
    optimize_autoloader: bool,
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    #[arg(long)]
    ignore_platform_reqs: bool,
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    #[arg(long)]
    no_fallback: bool,
    #[arg(long)]
    offline: bool,
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct UpdateArgs {
    /// Packages to update (the others stay locked); `vendor/*` patterns accepted.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Also update their dependencies, except those required by the root (`-w`).
    #[arg(short = 'w', long)]
    with_dependencies: bool,
    /// Also update their dependencies, root requirements included (`-W`).
    #[arg(short = 'W', long)]
    with_all_dependencies: bool,
    /// Write the lock file without installing.
    #[arg(long)]
    no_install: bool,
    /// Outputs the operations but will not execute anything.
    #[arg(long)]
    dry_run: bool,
    /// Do not install require-dev packages (they are still resolved).
    #[arg(long)]
    no_dev: bool,
    /// Do not generate the autoloader.
    #[arg(long)]
    no_autoloader: bool,
    #[arg(short = 'o', long)]
    optimize_autoloader: bool,
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    /// Accepted for compatibility: vivacity never runs scripts.
    #[arg(long)]
    no_scripts: bool,
    #[arg(long)]
    no_plugins: bool,
    /// Accepted for compatibility: vivacity does not audit (yet).
    #[arg(long)]
    no_audit: bool,
    /// Disable blocking policies (security advisories, malware).
    #[arg(long)]
    no_blocking: bool,
    #[arg(long)]
    no_security_blocking: bool,
    #[arg(long)]
    prefer_stable: bool,
    #[arg(long)]
    prefer_lowest: bool,
    #[arg(long)]
    ignore_platform_reqs: bool,
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    #[arg(long)]
    no_fallback: bool,
    #[arg(long)]
    offline: bool,
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
    #[arg(skip)]
    spawn_fallback: bool,
}

#[derive(clap::Args, Debug)]
struct DumpArgs {
    /// Do not include require-dev packages in the autoloader.
    #[arg(long)]
    no_dev: bool,
    /// Optimized classmap: every PSR directory is scanned.
    #[arg(short = 'o', long)]
    optimize: bool,
    /// Authoritative classmap (implies -o).
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    #[arg(long)]
    ignore_platform_reqs: bool,
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    /// Like Composer: no plugin, not even emulated ones (composer/installers).
    #[arg(long)]
    no_plugins: bool,
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct InstallArgs {
    /// Do not install require-dev packages.
    #[arg(long)]
    no_dev: bool,
    /// Do not generate the autoloader.
    #[arg(long)]
    no_autoloader: bool,
    /// Optimized classmap (`-o`).
    #[arg(short = 'o', long)]
    optimize_autoloader: bool,
    /// Authoritative classmap (`-a`, implies -o).
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    /// Accepted for compatibility: vivacity never runs scripts.
    #[arg(long)]
    no_scripts: bool,
    /// Like Composer: no plugin, not even emulated ones (composer/installers);
    /// everything installs into vendor/.
    #[arg(long)]
    no_plugins: bool,
    /// Ignore all platform requirements.
    #[arg(long)]
    ignore_platform_reqs: bool,
    /// Ignore one specific platform requirement (repeatable, `ext-*` patterns).
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    /// Never delegate to composer (fail explicitly when out of scope).
    #[arg(long)]
    no_fallback: bool,
    /// Only use local caches (no network access).
    #[arg(long)]
    offline: bool,
    /// Disable blocking policies (the lock's malware list).
    #[arg(long)]
    no_blocking: bool,
    #[arg(long)]
    no_security_blocking: bool,
    /// Rejected, as in Composer (`composer update --no-install`).
    #[arg(long)]
    no_install: bool,
    /// Checks (policies, scope, platform) and prints the operations
    /// without writing anything.
    #[arg(long)]
    dry_run: bool,
    /// Project directory (default: current directory).
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
    /// Internal: run `composer install` as a subprocess instead of
    /// replacing the process (when the caller still has work to do after).
    /// Off Unix there is no `exec()`: the subprocess is the only mode, and
    /// this field is never read (hence the `allow` — callers still set it
    /// so there is a single construction flow).
    #[arg(skip)]
    #[cfg_attr(not(unix), allow(dead_code))]
    spawn_fallback: bool,
    /// Internal: install following a resolution (`doInstall` with
    /// `alreadySolved`); the lock pool has already been filtered.
    #[arg(skip)]
    after_update: bool,
    /// Internal: the lock of a dry-run update, never written — the install
    /// phase reads it instead of composer.lock (`Locker::setLockData`
    /// keeps the data in memory; `mockLocalRepositories`).
    #[arg(skip)]
    virtual_lock: Option<serde_json::Value>,
    /// Internal: the manifest as the dry run's patched root package sees
    /// it (`require`/`require-dev` of `RequireCommand`/`RemoveCommand`
    /// applied in memory).
    #[arg(skip)]
    virtual_manifest: Option<serde_json::Value>,
}

/// `VIVACITY_TRACE=1`: duration of each phase on stderr (perf diagnostics).
fn trace(label: &str, since: std::time::Instant) {
    if std::env::var_os("VIVACITY_TRACE").is_some() {
        eprintln!(
            "trace: {label:<22} {:>7.1} ms",
            since.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// Run a vivacity command line (`args[0]` is the program name, as in
/// `std::env::args()`), runtime errors included: the return value is the
/// binary's exit code. A usage error (clap) prints the help or the message
/// and returns 2; `--help`/`--version` return 0.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            // `e.exit()` writes the help to stdout, the error to stderr.
            let _ = e.print();
            return e.exit_code();
        }
    };
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    }
}

fn dispatch(cli: Cli) -> anyhow::Result<i32> {
    match cli {
        Cli::Install(args) => run_install(&args),
        Cli::DumpAutoload(args) => run_dump(&args),
        Cli::Update(args) => run_update(&args),
        Cli::Remove(args) => run_remove(&args),
        Cli::Require(args) => require::run_require(&args),
    }
}

/// Absolute project root: every relative path written into vendor/ (bin
/// proxies, install-path) derives from it and must not depend on the
/// current directory.
fn project_dir(working_dir: Option<&std::path::Path>) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot determine the current directory")?;
    Ok(match working_dir {
        Some(d) if d.is_absolute() => d.to_path_buf(),
        Some(d) => cwd.join(d),
        None => cwd,
    })
}

fn run_install(args: &InstallArgs) -> anyhow::Result<i32> {
    if args.no_install {
        eprintln!("Invalid option \"--no-install\". Use \"composer update --no-install\" instead if you are trying to update the composer.lock file.");
        return Ok(1);
    }
    let t0 = std::time::Instant::now();
    let project = project_dir(args.working_dir.as_deref())?;
    let manifest_path = project.join("composer.json");
    let lock_path = project.join("composer.lock");

    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let manifest: serde_json::Value = match &args.virtual_manifest {
        Some(v) => v.clone(),
        None => serde_json::from_str(&manifest_text).context("invalid composer.json")?,
    };
    if args.virtual_lock.is_none() && !lock_path.is_file() {
        anyhow::bail!(
            "no composer.lock in {} — vivacity does not resolve dependencies yet, \
             run `composer update` first",
            project.display()
        );
    }
    // A dry-run update installs from its unwritten lock.
    let lock_text: String = match &args.virtual_lock {
        Some(v) => {
            vivacity_core::phpjson::php_json_encode_with(v, vivacity_core::phpjson::FLAGS_JSONFILE)?
        }
        None => std::fs::read_to_string(&lock_path)
            .with_context(|| format!("cannot read {}", lock_path.display()))?,
    };
    let lock = vivacity_core::lock::Lock::parse(&lock_text)?;
    trace("read manifests", t0);

    let with_dev = !args.no_dev && std::env::var("COMPOSER_NO_DEV").as_deref() != Ok("1");
    let config_lock = config_lock_enabled(&manifest_text);
    // `Installer::doInstall`: the headline, then the platform verification
    // notice (the lock is solved against the platform when not coming
    // straight from an update), then the freshness warning.
    if config_lock {
        eprintln!(
            "Installing dependencies from lock file{}",
            if with_dev {
                " (including require-dev)"
            } else {
                ""
            }
        );
    }
    if !args.after_update {
        eprintln!("Verifying lock file contents can be installed on current platform.");
        // Lock freshness: same behaviour as Composer, a warning.
        if let (Ok(actual), Some(expected)) = (
            vivacity_core::content_hash::content_hash(&manifest_text),
            lock.content_hash.as_deref(),
        ) {
            if actual != expected {
                eprintln!(
                    "Warning: The lock file is not up to date with the latest changes in composer.json. \
                     You may be getting outdated dependencies. It is recommended that you run `composer update` or `composer update <package name>`."
                );
            }
        }
    }

    // `Installer::doInstall` runs the lock pool through the list filter in
    // install scope: a flagged locked version (malware list) is not
    // installed.
    if !args.after_update {
        let http = http_transport(&project, args.offline)?;
        let cache_repo_dir = vivacity_core::fetch::composer_cache_dir().join("repo");
        let (problems, warnings) = vivacity_resolver::session::install_policy_problems(
            &project,
            vivacity_core::fetch::composer_home().as_deref(),
            Some(http),
            Some(&cache_repo_dir),
            with_dev,
            args.no_blocking || args.no_security_blocking,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        for w in &warnings {
            eprintln!("{w}");
        }
        if !problems.is_empty() {
            // `Installer::doInstall`: the headline, then
            // `SolverProblemsException::getPrettyString` (problems
            // deduplicated and numbered, each ending with a newline).
            eprintln!("Your lock file does not contain a compatible set of packages. Please run composer update.");
            let mut text = String::from("\n");
            let mut seen: Vec<&String> = Vec::new();
            for p in &problems {
                if seen.contains(&p) {
                    continue;
                }
                seen.push(p);
                text.push_str(&format!("  Problem {}\n    {p}\n", seen.len()));
            }
            eprintln!("{text}");
            return Ok(2);
        }
        trace("policy", t0);
    }

    // Platform.
    let mut ignored = args.ignore_platform_req.clone();
    if args.ignore_platform_reqs {
        ignored.push("*".to_owned());
    }
    if ignored.iter().all(|p| p != "*") {
        match vivacity_core::platform::Platform::detect()? {
            Some(mut platform) => {
                platform.apply_overrides(&manifest);
                let failures = vivacity_core::platform::check(&lock, &platform, with_dev, &ignored);
                if !failures.is_empty() {
                    eprintln!("Your lock file cannot be installed on this platform:");
                    for f in &failures {
                        let by = f
                            .required_by
                            .as_deref()
                            .map(|p| format!(" (required by {p})"))
                            .unwrap_or_default();
                        eprintln!(
                            "  - {} {}{}: {:?}",
                            f.requirement, f.constraint, by, f.reason
                        );
                    }
                    eprintln!(
                        "Use --ignore-platform-req=<req> or --ignore-platform-reqs to bypass."
                    );
                    return Ok(4);
                }
            }
            None => {
                if !lock.platform.is_empty() || (with_dev && !lock.platform_dev.is_empty()) {
                    eprintln!(
                        "Warning: php not found, platform requirements were not checked \
                         (--ignore-platform-reqs silences this warning)"
                    );
                }
            }
        }
    }

    trace("platform check", t0);
    // `LocalRepoTransaction` (installed.json against the lock): the
    // `Package operations` summary, and in a dry run the operation lines
    // in transaction order, then the abandoned warnings of `Installer::run`.
    let lock_value: serde_json::Value =
        serde_json::from_str(&lock_text).context("invalid composer.lock")?;
    let mut arena: Vec<vivacity_resolver::package::Package> = Vec::new();
    // The local repository: installed.json purged of the packages whose
    // install path is gone (`Factory::purgePackages`).
    let as_lock = serde_json::json!({"packages": installed_packages(&project), "packages-dev": []});
    let present = vivacity_resolver::repository::locked_repository_with(&as_lock, &mut arena, true)
        .map_err(|e| anyhow::anyhow!("installed.json: {}", e.0))?;
    let result =
        vivacity_resolver::repository::locked_repository_with(&lock_value, &mut arena, with_dev)
            .map_err(|e| anyhow::anyhow!("composer.lock: {}", e.0))?;
    // `Locker::getMissingRequirementInfo`: a root requirement the lock does
    // not satisfy (a hand-edited composer.json) stops here with code 4.
    let missing = missing_requirement_info(&manifest, &lock_value, with_dev)?;
    if !missing.is_empty() {
        for line in &missing {
            eprintln!("{line}");
        }
        if !require::truthy(require::config_value(
            &manifest,
            "allow-missing-requirements",
        )) {
            return Ok(4);
        }
    }
    let transaction = vivacity_resolver::transaction::Transaction::new(&arena, &present, &result);
    {
        use vivacity_resolver::transaction::Operation;
        let ops = &transaction.operations;
        let installs = ops
            .iter()
            .filter(|o| matches!(o, Operation::Install(_)))
            .count();
        let updates = ops
            .iter()
            .filter(|o| matches!(o, Operation::Update(..)))
            .count();
        let removals = ops
            .iter()
            .filter(|o| matches!(o, Operation::Uninstall(_)))
            .count();
        if installs + updates + removals == 0 {
            eprintln!("Nothing to install, update or remove");
        } else {
            eprintln!(
                "Package operations: {installs} install{}, {updates} update{}, {removals} removal{}",
                if installs == 1 { "" } else { "s" },
                if updates == 1 { "" } else { "s" },
                if removals == 1 { "" } else { "s" }
            );
        }
    }
    if args.dry_run {
        for op in &transaction.operations {
            if let Some(line) = op.show(&arena, false) {
                eprintln!("  - {line}");
            }
        }
        // `Installer::run` after `doInstall` (an update prints its own
        // report after this phase).
        if !args.after_update {
            for line in abandoned_warnings(&lock_value) {
                eprintln!("{line}");
            }
            print_funding(&project);
        }
        return Ok(0);
    }

    // Out of scope: exec composer as fallback (default) or fail explicitly.
    let scope =
        vivacity_core::scope::analyze(&project, &lock, &manifest, with_dev, !args.no_plugins);
    if !scope.is_native_ok() {
        return fallback_or_fail(args, &project, &scope);
    }
    let Some(layout) = scope.layout.as_ref() else {
        anyhow::bail!("internal: scope is native but no layout was resolved");
    };
    if let Some(tag) = &layout.installers_tag {
        eprintln!("Note: composer/installers {tag} emulated natively (custom install paths)");
    }
    if vivacity_core::runtime_stub::has_custom_runtime_options(&manifest) {
        let scope = vivacity_core::scope::ScopeReport {
            issues: vec![vivacity_core::scope::ScopeIssue::UnknownPlugin(
                "symfony/runtime with custom extra.runtime options".to_owned(),
            )],
            skipped_plugins: vec![],
            layout: None,
        };
        return fallback_or_fail(args, &project, &scope);
    }
    for plugin in &scope.skipped_plugins {
        eprintln!(
            "Note: plugin {plugin} installed as a plain library (vivacity never runs plugins)"
        );
    }
    trace("scope", t0);

    // The operation lines of a real install, in transaction order, with the
    // downloader's appendix (`getInstallOperationAppendix`: `: Extracting
    // archive`, `: Symlinking from …`, `: Mirroring from …`, `: Source
    // already present`) computed on the state before the transaction, like
    // Composer does right before each operation. They are printed once the
    // transaction has succeeded: the placements run in parallel, and a
    // failure is reported alone.
    let operation_lines = operation_lines(&project, &arena, &transaction.operations, layout)?;

    // Transaction.
    let store = Arc::new(vivacity_core::store::Store::default_location());
    let auth = vivacity_core::fetch::Auth::load(&project);
    let fetcher = Arc::new(vivacity_core::fetch::Fetcher::new(
        vivacity_core::fetch::composer_cache_dir(),
        auth,
    )?);
    let opts = vivacity_core::installer::InstallOptions {
        with_dev,
        offline: args.offline,
        ..Default::default()
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    let report = match runtime.block_on(vivacity_core::installer::install(
        &project, &lock, &manifest, layout, store, fetcher, &opts,
    )) {
        Ok(r) => r,
        // Emulation refusal detected before any write: vendor/ is intact.
        Err(vivacity_core::Error::Unsupported(msg)) => {
            let scope = vivacity_core::scope::ScopeReport {
                issues: vec![vivacity_core::scope::ScopeIssue::Layout(msg)],
                skipped_plugins: vec![],
                layout: None,
            };
            return fallback_or_fail(args, &project, &scope);
        }
        Err(vivacity_core::Error::Refused(msg)) => {
            eprintln!("{msg}");
            return Ok(1);
        }
        Err(e) => return Err(e.into()),
    };
    trace("install transaction", t0);
    for line in &operation_lines {
        eprintln!("{line}");
    }
    let mut autoload_note = String::new();
    if !args.no_autoloader {
        eprintln!("Generating autoload files");
        // `AutoloadGenerator::dump($localRepo)`: the local repository, not
        // the lock (they differ for an unchanged package whose lock entry
        // moved).
        let local = report.local_repository.as_ref().unwrap_or(&lock);
        let report = dump_autoload(
            &project,
            local,
            &manifest,
            layout,
            with_dev,
            args.optimize_autoloader || args.classmap_authoritative,
            args.classmap_authoritative,
            args.ignore_platform_reqs,
            &args.ignore_platform_req,
        )?;
        autoload_note = format!(", autoloader with {} classes", report.classes);
        trace("autoload dump", t0);
    }
    let warmed = if report.store_warmed > 0 {
        format!(", store warmed for {} packages", report.store_warmed)
    } else {
        String::new()
    };
    eprintln!(
        "vivacity: {} installed, {} unchanged, {} removed ({} from store, {} from cache, {} from network){warmed}{autoload_note} in {:.2}s",
        report.installed,
        report.unchanged,
        report.removed,
        report.store_hits,
        report.from_cache,
        report.from_network,
        t0.elapsed().as_secs_f32()
    );
    Ok(0)
}

/// `  - <operation><appendix>` for every operation of a real install.
fn operation_lines(
    project: &std::path::Path,
    arena: &[vivacity_resolver::package::Package],
    operations: &[vivacity_resolver::transaction::Operation],
    layout: &vivacity_core::layout::Layout,
) -> anyhow::Result<Vec<String>> {
    use vivacity_resolver::transaction::Operation;
    let mut lines = Vec::new();
    for op in operations {
        let Some(shown) = op.show(arena, false) else {
            continue;
        };
        let appendix = match *op {
            Operation::Install(p) | Operation::Update(_, p) => {
                let pkg = &arena[p];
                match pkg.dist.as_ref() {
                    Some(d) if d.kind == "path" => {
                        let install_path = layout
                            .abs(&pkg.name)
                            .unwrap_or_else(|| project.join("vendor").join(&pkg.name));
                        match vivacity_core::path_install::install_appendix(
                            project,
                            &install_path,
                            &d.url,
                            pkg.raw.get("transport-options"),
                        ) {
                            Ok(a) => a,
                            Err(vivacity_core::Error::Refused(msg)) => anyhow::bail!("{msg}"),
                            Err(e) => return Err(e.into()),
                        }
                    }
                    Some(_) if pkg.package_type != "metapackage" => {
                        ": Extracting archive".to_owned()
                    }
                    _ => String::new(),
                }
            }
            Operation::Uninstall(p) => {
                let pkg = &arena[p];
                match pkg.dist.as_ref() {
                    Some(d) if d.kind == "path" => {
                        let install_path = layout
                            .abs(&pkg.name)
                            .unwrap_or_else(|| project.join("vendor").join(&pkg.name));
                        if vivacity_core::path_install::is_own_source(
                            project,
                            &install_path.to_string_lossy(),
                            &d.url,
                        ) {
                            format!(", source is still present in {}", install_path.display())
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                }
            }
            _ => String::new(),
        };
        lines.push(format!("  - {shown}{appendix}"));
    }
    Ok(lines)
}

#[allow(clippy::too_many_arguments)]
fn dump_autoload(
    project: &std::path::Path,
    lock: &vivacity_core::lock::Lock,
    manifest: &serde_json::Value,
    layout: &vivacity_core::layout::Layout,
    dev_mode: bool,
    optimize: bool,
    authoritative: bool,
    ignore_all: bool,
    ignored: &[String],
) -> anyhow::Result<vivacity_autoload::DumpReport> {
    let platform_check = match manifest.get("config").and_then(|c| c.get("platform-check")) {
        Some(serde_json::Value::Bool(false)) => vivacity_autoload::PlatformCheckMode::Off,
        Some(serde_json::Value::Bool(true)) => vivacity_autoload::PlatformCheckMode::Full,
        _ => vivacity_autoload::PlatformCheckMode::PhpOnly,
    };
    // Like InstallCommand: the flags OR the composer.json config.
    let cfg_bool = |key: &str| {
        manifest
            .get("config")
            .and_then(|c| c.get(key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    let authoritative = authoritative || cfg_bool("classmap-authoritative");
    let optimize = optimize || authoritative || cfg_bool("optimize-autoloader");
    let opts = vivacity_autoload::DumpOptions {
        dev_mode,
        optimize,
        authoritative,
        platform_check,
        ignore_all_platform_reqs: ignore_all || ignored.iter().any(|p| p == "*"),
        ignored_platform_reqs: ignored.to_vec(),
        suffix: None,
        classmap_cache: if std::env::var_os("VIVACITY_NO_CLASSMAP_CACHE").is_some() {
            None
        } else {
            Some(vivacity_autoload::ClassmapCacheConfig {
                store_root: vivacity_core::platform::cache_dir().join("store"),
                cache_root: vivacity_core::platform::cache_dir(),
            })
        },
    };
    let report = vivacity_autoload::dump(project, lock, manifest, layout, &opts)?;
    for w in &report.warnings {
        eprintln!("{w}");
    }
    Ok(report)
}

fn run_dump(args: &DumpArgs) -> anyhow::Result<i32> {
    let t0 = std::time::Instant::now();
    let project = project_dir(args.working_dir.as_deref())?;
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(project.join("composer.json"))
            .context("cannot read composer.json")?,
    )
    .context("invalid composer.json")?;
    let lock = vivacity_core::lock::Lock::read(&project.join("composer.lock"))?;
    // Dev mode: that of the installed state (installed.json), like Composer.
    let installed_dev = std::fs::read_to_string(project.join("vendor/composer/installed.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("dev").and_then(serde_json::Value::as_bool))
        .unwrap_or(true);
    let dev_mode = !args.no_dev && installed_dev;
    let layout = match vivacity_core::layout::Layout::resolve(
        &project,
        &lock,
        &manifest,
        dev_mode,
        !args.no_plugins,
    ) {
        Ok(l) => l,
        Err(issues) => {
            eprintln!("vivacity: this lock is outside what vivacity handles natively:");
            for i in &issues {
                eprintln!("  - {i}");
            }
            eprintln!("Run `composer dump-autoload` instead.");
            return Ok(3);
        }
    };
    // Composer runs the PRE_AUTOLOAD_DUMP listeners of every installed plugin
    // (drupal/core-composer-scaffold adds classmap entries and writes
    // vendor/drupal/DrupalInstalled.php): a plugin vivacity neither emulates
    // nor knows to be inert makes the dump non-reproducible.
    if !args.no_plugins {
        let issues = vivacity_core::scope::plugin_issues(&lock, installed_dev);
        if !issues.is_empty() {
            eprintln!("vivacity: this lock is outside what vivacity handles natively:");
            for i in &issues {
                eprintln!("  - {i}");
            }
            eprintln!("Run `composer dump-autoload` instead.");
            return Ok(3);
        }
    }
    let report = dump_autoload(
        &project,
        &lock,
        &manifest,
        &layout,
        dev_mode,
        args.optimize || args.classmap_authoritative,
        args.classmap_authoritative,
        args.ignore_platform_reqs,
        &args.ignore_platform_req,
    )?;
    eprintln!(
        "vivacity: autoloader generated ({} classes) in {:.2}s",
        report.classes,
        t0.elapsed().as_secs_f32()
    );
    Ok(0)
}

fn fallback_or_fail(
    args: &InstallArgs,
    project: &std::path::Path,
    scope: &vivacity_core::scope::ScopeReport,
) -> anyhow::Result<i32> {
    eprintln!("vivacity: this lock is outside what vivacity handles natively:");
    for issue in &scope.issues {
        eprintln!("  - {issue}");
    }
    if args.no_fallback {
        eprintln!("--no-fallback given: stopping here (no partial vendor/ was written).");
        return Ok(3);
    }
    let composer = which_composer();
    let Some(composer) = composer else {
        eprintln!(
            "composer not found for the fallback — install Composer or remove the unsupported items."
        );
        return Ok(3);
    };
    eprintln!("vivacity: delegating to `composer install`…");
    let mut cmd = std::process::Command::new(composer);
    cmd.arg("install").current_dir(project);
    if args.no_dev {
        cmd.arg("--no-dev");
    }
    if args.no_autoloader {
        cmd.arg("--no-autoloader");
    }
    if args.optimize_autoloader {
        cmd.arg("--optimize-autoloader");
    }
    if args.classmap_authoritative {
        cmd.arg("--classmap-authoritative");
    }
    if args.no_scripts {
        cmd.arg("--no-scripts");
    }
    if args.no_plugins {
        cmd.arg("--no-plugins");
    }
    if args.ignore_platform_reqs {
        cmd.arg("--ignore-platform-reqs");
    }
    for req in &args.ignore_platform_req {
        cmd.arg(format!("--ignore-platform-req={req}"));
    }
    #[cfg(unix)]
    if !args.spawn_fallback {
        use std::os::unix::process::CommandExt as _;
        let err = cmd.exec();
        return Err(err).context("cannot exec composer");
    }
    let status = cmd.status().context("cannot run composer")?;
    Ok(status.code().unwrap_or(1))
}

fn which_composer() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    // On Windows, composer installs itself as composer.bat/.cmd (a wrapper
    // around the .phar); `Command` can launch a .bat (via cmd.exe).
    let names: &[&str] = if cfg!(windows) {
        &["composer.bat", "composer.cmd", "composer.exe", "composer"]
    } else {
        &["composer"]
    };
    std::env::split_paths(&path)
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|c| c.is_file())
}

/// `composer update`: resolution (exact port of Composer's solver), lock
/// written if its data changes, then `install`.
/// Network transport for remote composer repositories: the vivacity-core
/// Fetcher (Composer auth, retries), made synchronous, with a parallel
/// batch (Composer: curl multi, 12 downloads at a time).
fn http_transport(
    project: &std::path::Path,
    offline: bool,
) -> anyhow::Result<vivacity_resolver::repository::HttpTransports> {
    let runtime =
        Arc::new(tokio::runtime::Runtime::new().context("cannot start the async runtime")?);
    let fetcher = Arc::new(vivacity_core::fetch::Fetcher::new(
        vivacity_core::fetch::composer_cache_dir(),
        vivacity_core::fetch::Auth::load(project),
    )?);
    fn to_fetched(
        r: vivacity_core::fetch::MetadataResponse,
    ) -> vivacity_resolver::repository::Fetched {
        use vivacity_core::fetch::MetadataResponse as M;
        use vivacity_resolver::repository::Fetched as F;
        match r {
            M::NotModified => F::NotModified,
            M::NotFound => F::NotFound,
            M::Body {
                bytes,
                last_modified,
            } => F::Body {
                bytes,
                last_modified,
            },
        }
    }
    let http: vivacity_resolver::repository::HttpFetch = {
        let runtime = runtime.clone();
        let fetcher = fetcher.clone();
        Arc::new(move |url: &str, ims: Option<&str>| {
            if offline {
                return Err(format!("offline: cannot fetch {url}"));
            }
            runtime
                .block_on(fetcher.metadata_fetch(url, ims))
                .map(to_fetched)
                .map_err(|e| e.to_string())
        })
    };
    // Parallel batch (Composer: curl multi, 12 downloads at a time).
    let http_many: vivacity_resolver::repository::HttpFetchMany = {
        let runtime = runtime.clone();
        let fetcher = fetcher.clone();
        Arc::new(move |requests: &[vivacity_resolver::repository::Request]| {
            if offline {
                return requests
                    .iter()
                    .map(|(u, _)| Err(format!("offline: cannot fetch {u}")))
                    .collect();
            }
            let requests: Vec<(String, Option<String>)> = requests.to_vec();
            runtime.block_on(async {
                let sem = Arc::new(tokio::sync::Semaphore::new(12));
                let tasks: Vec<_> = requests
                    .into_iter()
                    .map(|(u, ims)| {
                        let sem = sem.clone();
                        let fetcher = fetcher.clone();
                        async move {
                            let _permit = sem.acquire().await;
                            fetcher
                                .metadata_fetch(&u, ims.as_deref())
                                .await
                                .map(to_fetched)
                                .map_err(|e| e.to_string())
                        }
                    })
                    .collect();
                futures_join_all(tasks).await
            })
        })
    };
    // Form POST (security advisories API).
    let http_post: vivacity_resolver::repository::HttpPost = {
        let runtime = runtime.clone();
        let fetcher = fetcher.clone();
        Arc::new(move |url: &str, body: &str| {
            if offline {
                return Err(format!("offline: cannot fetch {url}"));
            }
            runtime
                .block_on(fetcher.post_form(url, body))
                .map(to_fetched)
                .map_err(|e| e.to_string())
        })
    };
    Ok((http, Some(http_many), Some(http_post)))
}

fn run_update(args: &UpdateArgs) -> anyhow::Result<i32> {
    // Partial update: `update a/b [-w|-W]` (UpdateCommand).
    let env_flag = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0");
    for p in &args.packages {
        if ["lock", "nothing", "mirrors"].contains(&p.as_str()) {
            anyhow::bail!(
                "`vivacity update {p}` (lock file metadata refresh) is not supported yet"
            );
        }
        if p.contains([' ', '=', ':']) {
            anyhow::bail!("temporary constraints (`update {p}`, `--with`) are not supported yet");
        }
    }
    let transitive = if args.with_all_dependencies || env_flag("COMPOSER_WITH_ALL_DEPENDENCIES") {
        vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDeps
    } else if args.with_dependencies || env_flag("COMPOSER_WITH_DEPENDENCIES") {
        vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDepsNoRootRequire
    } else {
        vivacity_resolver::pool::UpdateMode::OnlyListed
    };
    let mut options = if args.packages.is_empty() {
        vivacity_resolver::session::UpdateOptions::default()
    } else {
        vivacity_resolver::session::UpdateOptions::partial(&args.packages, transitive)
    };
    options.no_blocking = args.no_blocking || args.no_security_blocking;
    // BaseCommand: COMPOSER_PREFER_STABLE / COMPOSER_PREFER_LOWEST count as
    // the options.
    let prefer_stable = args.prefer_stable || env_flag("COMPOSER_PREFER_STABLE");
    let prefer_lowest = args.prefer_lowest || env_flag("COMPOSER_PREFER_LOWEST");
    run_update_resolved(args, options, prefer_stable, prefer_lowest)
}

/// Resolution, lock write and install, once the list and the update mode
/// are decided (shared by `update` and `remove`).
fn run_update_resolved(
    args: &UpdateArgs,
    options: vivacity_resolver::session::UpdateOptions,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> anyhow::Result<i32> {
    let resolved = resolve_and_lock(args, options, prefer_stable, prefer_lowest)?;
    if resolved.status != 0 {
        return Ok(resolved.status);
    }
    let project = project_dir(args.working_dir.as_deref())?;
    if !args.no_install {
        let (virtual_lock, virtual_manifest) = if args.dry_run {
            (resolved.lock.clone(), Some(resolved.manifest.clone()))
        } else {
            (None, None)
        };
        let status = install_after_update(args, virtual_lock, virtual_manifest)?;
        if status != 0 {
            return Ok(status);
        }
    }
    print_post_update(&resolved, &project);
    Ok(0)
}

/// The install that follows the lock write (`Installer::run` with
/// `setInstall(true)`).
fn install_after_update(
    args: &UpdateArgs,
    virtual_lock: Option<serde_json::Value>,
    virtual_manifest: Option<serde_json::Value>,
) -> anyhow::Result<i32> {
    run_install(&InstallArgs {
        no_dev: args.no_dev,
        no_autoloader: args.no_autoloader,
        optimize_autoloader: args.optimize_autoloader,
        classmap_authoritative: args.classmap_authoritative,
        no_scripts: args.no_scripts,
        no_plugins: args.no_plugins,
        ignore_platform_reqs: args.ignore_platform_reqs,
        ignore_platform_req: args.ignore_platform_req.clone(),
        no_fallback: args.no_fallback,
        offline: args.offline,
        no_blocking: args.no_blocking,
        no_security_blocking: args.no_security_blocking,
        no_install: false,
        dry_run: args.dry_run,
        working_dir: args.working_dir.clone(),
        spawn_fallback: args.spawn_fallback,
        after_update: true,
        virtual_lock,
        virtual_manifest,
    })
}

/// Resolution and lock write: 0, or 2 on an unsolvable set.
/// The outcome of the resolution: the status (0, or 2 on an unsolvable
/// set) and the lock data, written or not (`config.lock: false` resolves
/// without writing; Composer then keeps a "virtual" lock).
pub(crate) struct Resolved {
    pub(crate) status: i32,
    pub(crate) lock: Option<serde_json::Value>,
    /// What `Installer::run` prints after the install phase (suggestions,
    /// abandoned packages), ready to print; the funding count is read
    /// from installed.json at that time (`print_post_update`).
    pub(crate) post: Vec<String>,
    /// The manifest as the (possibly patched) root package sees it.
    pub(crate) manifest: serde_json::Value,
}

/// Prints the post-update report of `Installer::run` once the install
/// phase (if any) is done: the precomputed lines, then the funding count
/// of the packages installed now — unless `dumpAutoloader` is off (dry
/// run).
pub(crate) fn print_post_update(resolved: &Resolved, project: &std::path::Path) {
    for line in &resolved.post {
        eprintln!("{line}");
    }
    print_funding(project);
}

/// The packages of vendor/composer/installed.json (list form of old
/// Composers accepted), minus those whose install path is gone
/// (`Factory::purgePackages` through `LibraryInstaller::isInstalled`; a
/// metapackage is always installed).
fn installed_packages(project: &std::path::Path) -> Vec<serde_json::Value> {
    let composer_dir = project.join("vendor/composer");
    let Some(installed) = std::fs::read_to_string(composer_dir.join("installed.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return Vec::new();
    };
    let list = installed
        .get("packages")
        .and_then(|p| p.as_array())
        .cloned()
        .or_else(|| installed.as_array().cloned())
        .unwrap_or_default();
    list.into_iter()
        .filter(|p| {
            if p.get("type").and_then(|t| t.as_str()) == Some("metapackage") {
                return true;
            }
            match p.get("install-path").and_then(|v| v.as_str()) {
                Some(rel) => composer_dir.join(rel).exists(),
                None => true,
            }
        })
        .collect()
}

/// `Installer::run`: the funding count of the local repository's packages
/// (never an alias), unless `COMPOSER_FUND` is a number equal to 0. Printed
/// in every mode, dry run included (`$localRepo` is then the mocked
/// repository of the same packages).
fn print_funding(project: &std::path::Path) {
    if let Ok(env) = std::env::var("COMPOSER_FUND") {
        let t = env.trim();
        if let Ok(n) = t.parse::<f64>() {
            if n == 0.0 {
                return;
            }
        }
    }
    let funding = installed_packages(project)
        .iter()
        .filter(|p| {
            p.get("funding")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|f| !f.is_empty())
        })
        .count();
    if funding > 0 {
        eprintln!(
            "{funding} package{} you are using {} looking for funding.\nUse the `composer fund` command to find out more!",
            if funding == 1 { "" } else { "s" },
            if funding == 1 { "is" } else { "are" }
        );
    }
}

fn resolve_and_lock(
    args: &UpdateArgs,
    options: vivacity_resolver::session::UpdateOptions,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> anyhow::Result<Resolved> {
    use vivacity_resolver::platform_filter::PlatformRequirementFilter;
    use vivacity_resolver::session::UpdateSession;
    let t0 = std::time::Instant::now();
    let project = project_dir(args.working_dir.as_deref())?;
    let manifest_path = project.join("composer.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let home = vivacity_core::fetch::composer_home();
    let http = http_transport(&project, args.offline)?;
    let cache_repo_dir = vivacity_core::fetch::composer_cache_dir().join("repo");
    let mut session = UpdateSession::prepare_update(
        &project,
        home.as_deref(),
        true,
        Some(http),
        Some(&cache_repo_dir),
        &options,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    trace("prepare", t0);
    session.prefer_stable = prefer_stable;
    session.prefer_lowest = prefer_lowest;
    session.installer_dev_mode =
        !(args.no_dev || std::env::var("COMPOSER_NO_DEV").is_ok_and(|v| !v.is_empty() && v != "0"));
    // BaseCommand: COMPOSER_IGNORE_PLATFORM_REQS counts as the option,
    // COMPOSER_IGNORE_PLATFORM_REQ (comma-separated list) counts as the
    // list when it is empty.
    let env_flag = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0");
    let ignore_all = args.ignore_platform_reqs || env_flag("COMPOSER_IGNORE_PLATFORM_REQS");
    let mut ignore_list = args.ignore_platform_req.clone();
    if !ignore_all && ignore_list.is_empty() {
        if let Ok(env) = std::env::var("COMPOSER_IGNORE_PLATFORM_REQ") {
            if !env.is_empty() {
                eprintln!("COMPOSER_IGNORE_PLATFORM_REQ is set to ignore {env}. You may experience unexpected errors.");
                ignore_list = env.split(',').map(str::to_owned).collect();
            }
        }
    }
    if ignore_all && env_flag("COMPOSER_IGNORE_PLATFORM_REQS") && !args.ignore_platform_reqs {
        eprintln!("COMPOSER_IGNORE_PLATFORM_REQS is set. You may experience unexpected errors.");
    }
    let filter = if ignore_all {
        PlatformRequirementFilter::IgnoreAll
    } else if !ignore_list.is_empty() {
        PlatformRequirementFilter::from_list(&ignore_list)
    } else {
        PlatformRequirementFilter::IgnoreNothing
    };
    eprintln!("Loading composer repositories with package information");
    eprintln!("Updating dependencies");
    // `Installer::run`: an unsolvable set means exit code 2
    // (`SolverProblemsException`), any other error is an exception.
    let (lock, report) = match session.update(&manifest_text, &filter) {
        Ok(r) => r,
        Err(e) if e.kind == vivacity_resolver::session::SessionErrorKind::Unsolvable => {
            eprintln!("{e}");
            return Ok(Resolved {
                status: 2,
                lock: None,
                post: Vec::new(),
                manifest: serde_json::Value::Null,
            });
        }
        Err(e) => return Err(anyhow::anyhow!("{e}")),
    };
    trace("resolve", t0);

    let lock_path = project.join("composer.lock");
    let mut text = vivacity_core::phpjson::php_json_encode_with(
        &lock,
        vivacity_core::phpjson::FLAGS_JSONFILE,
    )?;
    // `JsonFile::read` remembers the existing lock's indentation, `write`
    // reuses it.
    let old_text = std::fs::read_to_string(&lock_path).ok();
    if let Some(old) = &old_text {
        let indent = vivacity_resolver::json_manipulator::detect_indenting(old)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if indent != "    " {
            text = vivacity_resolver::config_source::reindent(&text, &indent);
        }
    }
    text.push('\n');
    // `JsonFile::write` goes through `filePutContentsIfModified`: the file
    // is only rewritten if its bytes change. (`Locker::setLockData` also
    // compares the decoded data, but `{}` versus `[]` makes that comparison
    // always false for an ordinary lock; the bytes are the observable
    // criterion.)
    let unchanged = old_text.as_deref() == Some(text.as_str());
    // `config.lock: false`: Composer resolves without writing a lock.
    let write_lock = config_lock_enabled(&manifest_text);
    let ops = &report.transaction.transaction.operations;
    if ops.is_empty() {
        eprintln!("Nothing to modify in lock file");
    } else if write_lock {
        let count = |f: &dyn Fn(&vivacity_resolver::transaction::Operation) -> bool| {
            ops.iter().filter(|o| f(o)).count()
        };
        use vivacity_resolver::transaction::Operation;
        let installs = count(&|o| matches!(o, Operation::Install(_)));
        let updates = count(&|o| matches!(o, Operation::Update(..)));
        let removals = count(&|o| matches!(o, Operation::Uninstall(_)));
        eprintln!(
            "Lock file operations: {installs} install{}, {updates} update{}, {removals} removal{}",
            if installs == 1 { "" } else { "s" },
            if updates == 1 { "" } else { "s" },
            if removals == 1 { "" } else { "s" }
        );
        // `  - Locking …` / `Upgrading` / `Removing`, removals first, by name.
        for line in vivacity_resolver::transaction::lock_operation_lines(&session.arena, ops) {
            eprintln!("{line}");
        }
    }
    // `$updatedLock && $this->writeLock && $this->executeOperations`: a dry
    // run neither writes nor says so.
    if write_lock && !args.dry_run {
        eprintln!("Writing lock file");
        if !unchanged {
            std::fs::write(&lock_path, &text)
                .with_context(|| format!("cannot write {}", lock_path.display()))?;
        }
    }
    trace("write lock", t0);
    // `Installer::run` after `doUpdate` (and after the install phase): the
    // suggestions of the newly installed packages (and of the root on a
    // fresh install), the abandoned packages of the new lock.
    let fresh_install = !project.join("vendor/composer/installed.json").is_file();
    let post = post_update_report(&session, ops, &lock, &manifest_text, fresh_install);
    let manifest = patched_manifest(&session, &manifest_text);
    Ok(Resolved {
        status: 0,
        lock: Some(lock),
        post,
        manifest,
    })
}

/// composer.json with the session root's `require`/`require-dev` (the
/// in-memory patch of a dry run applied; identical otherwise).
fn patched_manifest(
    session: &vivacity_resolver::session::UpdateSession,
    manifest_text: &str,
) -> serde_json::Value {
    let mut manifest: serde_json::Value = serde_json::from_str(manifest_text).unwrap_or_default();
    let links_to_object = |links: &vivacity_resolver::package::Links| -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for l in links.iter() {
            map.insert(
                l.target.clone(),
                serde_json::Value::String(l.pretty_constraint.clone()),
            );
        }
        serde_json::Value::Object(map)
    };
    if let Some(obj) = manifest.as_object_mut() {
        for (key, links) in [
            ("require", &session.root.package.requires),
            ("require-dev", &session.root.package.dev_requires),
        ] {
            if links.is_empty() {
                obj.remove(key);
            } else {
                obj.insert(key.to_owned(), links_to_object(links));
            }
        }
    }
    manifest
}

/// The lines of `Installer::run` after a successful update:
/// `SuggestedPackagesReporter::outputMinimalistic` (suggestions of the
/// packages installed by this update, plus the root's own on a fresh
/// install, minus the targets some other package provides), one warning
/// per abandoned package of the new lock. The funding count is added by
/// `print_post_update`.
fn post_update_report(
    session: &vivacity_resolver::session::UpdateSession,
    ops: &[vivacity_resolver::transaction::Operation],
    lock: &serde_json::Value,
    manifest_text: &str,
    fresh_install: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    use vivacity_resolver::transaction::Operation;
    let manifest: serde_json::Value = serde_json::from_str(manifest_text).unwrap_or_default();
    // (source pretty name, target)
    let mut suggestions: Vec<(String, String)> = Vec::new();
    let add_from =
        |suggestions: &mut Vec<(String, String)>, source: &str, raw: &serde_json::Value| {
            if let Some(map) = raw.get("suggest").and_then(serde_json::Value::as_object) {
                for target in map.keys() {
                    suggestions.push((source.to_owned(), target.clone()));
                }
            }
        };
    for op in ops {
        if let Operation::Install(idx) = op {
            let p = &session.arena[*idx];
            add_from(&mut suggestions, &p.pretty_name, &p.raw);
        }
    }
    if fresh_install {
        let root_name = manifest
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("__root__");
        add_from(&mut suggestions, root_name, &manifest);
    }
    if !suggestions.is_empty() {
        // `InstalledRepository([locked repo (dev mode), platform repo, root])`:
        // `getNames()` of each package -> the packages providing that name.
        let mut installed_names: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut add_names = |name: &str, names: Vec<String>| {
            for n in names {
                installed_names.entry(n).or_default().push(name.to_owned());
            }
        };
        let mut lock_sections = vec!["packages"];
        if session.installer_dev_mode {
            lock_sections.push("packages-dev");
        }
        for section in lock_sections {
            for p in lock
                .get(section)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(name) = p.get("name").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let name = name.to_lowercase();
                let mut names = vec![name.clone()];
                for key in ["provide", "replace"] {
                    for target in p
                        .get(key)
                        .and_then(serde_json::Value::as_object)
                        .into_iter()
                        .flat_map(|m| m.keys())
                    {
                        if !names.contains(target) {
                            names.push(target.clone());
                        }
                    }
                }
                add_names(&name, names);
            }
        }
        for &idx in &session.platform {
            let p = &session.arena[idx];
            add_names(&p.name, p.names(true));
        }
        let root = &session.root.package;
        add_names(&root.name, root.names(true));
        let count = suggestions
            .iter()
            .filter(|(source, target)| {
                let source_lower = source.to_lowercase();
                !installed_names
                    .get(target)
                    .is_some_and(|providers| providers.iter().any(|p| *p != source_lower))
            })
            .count();
        if count > 0 {
            out.push(format!(
                "{count} package suggestions were added by new dependencies, use `composer suggest` to see details."
            ));
        }
    }
    out.extend(abandoned_warnings(lock));
    out
}

/// `Installer::run`: one warning per abandoned package of the lock (dev
/// included), in lock order.
fn abandoned_warnings(lock: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for section in ["packages", "packages-dev"] {
        for p in lock
            .get(section)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            // `CompletePackage::isAbandoned` = `(bool) $abandoned`,
            // `getReplacementPackage` = the string, else null.
            let abandoned = match p.get("abandoned") {
                None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false)) => {
                    continue
                }
                Some(serde_json::Value::String(s)) if s.is_empty() || s == "0" => continue,
                Some(serde_json::Value::Number(n)) if n.as_f64() == Some(0.0) => continue,
                Some(serde_json::Value::Array(a)) if a.is_empty() => continue,
                Some(serde_json::Value::Object(o)) if o.is_empty() => continue,
                Some(serde_json::Value::String(replacement)) => Some(replacement.clone()),
                Some(_) => None,
            };
            let replacement = match abandoned {
                Some(r) => format!("Use {r} instead"),
                None => "No replacement was suggested".to_owned(),
            };
            out.push(format!(
                "Package {} is abandoned, you should avoid using it. {replacement}.",
                p.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            ));
        }
    }
    out
}

/// `Config::get('lock')`: project then global config; `"false"` and PHP's
/// falsy values disable it.
fn config_lock_enabled(manifest_text: &str) -> bool {
    let manifest: serde_json::Value = serde_json::from_str(manifest_text).unwrap_or_default();
    match require::config_value(&manifest, "lock") {
        None => true,
        Some(serde_json::Value::String(s)) => s != "false" && !(s.is_empty() || s == "0"),
        Some(serde_json::Value::Bool(b)) => b,
        Some(serde_json::Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Array(a)) => !a.is_empty(),
        Some(serde_json::Value::Object(m)) => !m.is_empty(),
    }
}

/// Minimal `join_all` (no futures dependency): the tasks run concurrently
/// on the runtime, results in order.
async fn futures_join_all<F: std::future::Future + Send + 'static>(tasks: Vec<F>) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let handles: Vec<_> = tasks.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("metadata task panicked"));
    }
    out
}

/// `RemoveCommand::execute`: composer.json edited through
/// `JsonConfigSource`, `allow-plugins` cleaned up, then a partial update
/// (list = removed packages, "with dependencies except root requirements"
/// mode by default), composer.json restored if it fails.
fn run_remove(args: &RemoveArgs) -> anyhow::Result<i32> {
    use vivacity_resolver::config_source::{composer_file, JsonConfigSource};
    let env_flag = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0");
    if args.minimal_changes || env_flag("COMPOSER_MINIMAL_CHANGES") {
        anyhow::bail!("`--minimal-changes` (update-with-minimal-changes) is not supported yet");
    }
    // `--dry-run`: the links are collected instead of removed from the
    // file, and the root package is patched in memory (`$toRemove`).
    let dry_run = args.dry_run;
    let mut to_remove: Vec<(bool, String)> = Vec::new();
    if args.packages.is_empty() && !args.unused {
        eprintln!("Not enough arguments (missing: \"packages\").");
        return Ok(1);
    }
    let project = project_dir(args.working_dir.as_deref())?;
    let file = composer_file(&project);
    // The name Composer displays is the path as `Factory` gives it. An
    // alternate manifest (`COMPOSER=alt.json`, lock `alt.lock`) is not
    // followed by the resolution: rejected rather than resolved against
    // composer.json.
    let file_label = match std::env::var("COMPOSER").ok().map(|f| f.trim().to_owned()) {
        Some(f) if !f.is_empty() && f != "composer.json" && f != "./composer.json" => {
            anyhow::bail!("COMPOSER={f}: an alternate manifest is not supported yet");
        }
        Some(f) if !f.is_empty() => f,
        _ => "./composer.json".to_owned(),
    };
    let manifest_text = std::fs::read_to_string(&file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text)
        .with_context(|| format!("{} does not contain valid JSON", file.display()))?;
    if serde_json::from_str::<serde_json::Value>(&manifest_text)
        .ok()
        .and_then(|m| m.get("config")?.get("update-with-minimal-changes").cloned())
        .is_some_and(|v| v == serde_json::Value::Bool(true))
    {
        anyhow::bail!("config.update-with-minimal-changes is not supported yet");
    }
    let mut packages: Vec<String> = args.packages.iter().map(|p| p.to_lowercase()).collect();

    // `Locker::isLocked`: a readable lock that has a `packages` key.
    let locked_data: Option<serde_json::Value> =
        std::fs::read_to_string(project.join("composer.lock"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .filter(|v: &serde_json::Value| v.get("packages").is_some());
    if args.unused {
        let lock = locked_data.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "A valid composer.lock file is required to run this command with --unused"
            )
        })?;
        packages.extend(unused_locked_packages(&manifest, &lock));
        if packages.is_empty() {
            eprintln!("No unused packages to remove");
            return Ok(0);
        }
    }

    let backup = manifest_text.clone();
    let json = JsonConfigSource::new(&file);
    let (link_type, alt_type) = if args.dev {
        ("require-dev", "require")
    } else {
        ("require", "require-dev")
    };
    if args.update_with_dependencies {
        eprintln!("You are using the deprecated option \"update-with-dependencies\". This is now default behaviour. The --no-update-with-dependencies option can be used to remove a package without its dependencies.");
    }
    // `$composer[$linkType][strtolower($name)] = $name`: the lowercase keys
    // are added to the decoded array (the originals stay, and `array_keys`
    // sees them all).
    let keyed = |section: &str| -> Vec<(String, String)> {
        let mut keys: Vec<(String, String)> = Vec::new();
        if let Some(m) = manifest.get(section).and_then(|v| v.as_object()) {
            for k in m.keys() {
                keys.push((k.clone(), k.clone()));
            }
            for k in m.keys() {
                let lower = k.to_lowercase();
                match keys.iter_mut().find(|(key, _)| *key == lower) {
                    Some(entry) => entry.1 = k.clone(),
                    None => keys.push((lower, k.clone())),
                }
            }
        }
        keys
    };
    let sections = [(link_type, keyed(link_type)), (alt_type, keyed(alt_type))];
    let lookup = |section: &str, key: &str| -> Option<String> {
        sections
            .iter()
            .find(|(s, _)| *s == section)?
            .1
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, name)| name.clone())
    };
    let grep = |section: &str, pattern: &str| -> Vec<String> {
        let re = vivacity_resolver::pool::package_name_regexp(pattern);
        sections
            .iter()
            .find(|(s, _)| *s == section)
            .map(|(_, keys)| {
                keys.iter()
                    .filter(|(k, _)| re.is_match(k.as_bytes()).unwrap_or(false))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    let has_section = |section: &str| manifest.get(section).is_some_and(|v| !v.is_null());
    let dev_key = link_type == "require-dev";
    for package in &packages {
        if let Some(name) = lookup(link_type, package) {
            if dry_run {
                to_remove.push((dev_key, name));
            } else {
                json.remove_link(link_type, &name)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
        } else if let Some(name) = lookup(alt_type, package) {
            eprintln!("{name} could not be found in {link_type} but it is present in {alt_type}");
        } else if has_section(link_type) && !grep(link_type, package).is_empty() {
            for matched in grep(link_type, package) {
                if dry_run {
                    to_remove.push((dev_key, matched));
                } else {
                    json.remove_link(link_type, &matched)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
        } else if has_section(alt_type) && !grep(alt_type, package).is_empty() {
            for matched in grep(alt_type, package) {
                eprintln!(
                    "{matched} could not be found in {link_type} but it is present in {alt_type}"
                );
            }
        } else {
            eprintln!("{package} is not required in your composer.json and has not been removed");
        }
    }
    eprintln!("{file_label} has been updated");
    if args.no_update {
        return Ok(0);
    }

    // `allow-plugins`: the merged config (project then global); entries
    // whose key is a removed package are dropped from composer.json.
    let updated: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    if let (false, Some(serde_json::Value::Object(allow))) = (
        dry_run,
        vivacity_core::layout::merged_allow_plugins(
            updated.get("config").and_then(|c| c.get("allow-plugins")),
            vivacity_core::layout::global_allow_plugins().as_ref(),
        ),
    ) {
        let removed: Vec<&String> = allow.keys().filter(|k| packages.contains(k)).collect();
        if !removed.is_empty() {
            if removed.len() == allow.len() {
                json.remove_config_setting("allow-plugins")
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            } else {
                for plugin in removed {
                    json.remove_config_setting(&format!("allow-plugins.{plugin}"))
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
        }
    }

    // `Request::UPDATE_LISTED_WITH_TRANSITIVE_DEPS_NO_ROOT_REQUIRE` by
    // default; `COMPOSER_WITH_ALL_DEPENDENCIES` counts as the option
    // (BaseCommand), `COMPOSER_WITH_DEPENDENCIES` has no option here.
    let transitive = if args.update_with_all_dependencies
        || args.with_all_dependencies
        || env_flag("COMPOSER_WITH_ALL_DEPENDENCIES")
    {
        vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDeps
    } else if args.no_update_with_dependencies {
        vivacity_resolver::pool::UpdateMode::OnlyListed
    } else {
        vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDepsNoRootRequire
    };
    let mut flags = String::new();
    if transitive == vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDeps {
        flags.push_str(" --with-all-dependencies");
    } else if transitive == vivacity_resolver::pool::UpdateMode::OnlyListed {
        flags.push_str(" --with-dependencies");
    }
    eprintln!("Running composer update {}{flags}", packages.join(" "));
    // `setUpdateAllowList` only if a lock exists.
    let mut options = if locked_data.is_some() {
        vivacity_resolver::session::UpdateOptions::partial(&packages, transitive)
    } else {
        vivacity_resolver::session::UpdateOptions::default()
    };
    options.no_blocking = args.no_blocking || args.no_security_blocking;
    if dry_run {
        options.root_patch = Some(vivacity_resolver::root::RootPatch {
            removals: to_remove,
            ..Default::default()
        });
    }
    let update_args = UpdateArgs {
        packages: Vec::new(),
        with_dependencies: false,
        with_all_dependencies: false,
        no_install: args.no_install,
        dry_run: args.dry_run,
        no_dev: args.update_no_dev || env_flag("COMPOSER_NO_DEV"),
        no_autoloader: args.no_autoloader,
        optimize_autoloader: args.optimize_autoloader,
        classmap_authoritative: args.classmap_authoritative,
        no_scripts: args.no_scripts,
        no_plugins: args.no_plugins,
        no_audit: args.no_audit,
        no_blocking: args.no_blocking,
        no_security_blocking: args.no_security_blocking,
        prefer_stable: false,
        prefer_lowest: false,
        ignore_platform_reqs: args.ignore_platform_reqs,
        ignore_platform_req: args.ignore_platform_req.clone(),
        no_fallback: args.no_fallback,
        offline: args.offline,
        working_dir: args.working_dir.clone(),
        // The rest of `remove` (restore, local repository check) must run
        // even if the install is handed over to Composer.
        spawn_fallback: true,
    };
    // `remove` has no prefer-stable/lowest option: only the manifest
    // counts.
    // An exception (transport, invalid manifest) propagates without a
    // restore; only a non-zero status from `Installer::run` restores.
    let status = run_update_resolved(&update_args, options, false, false)?;
    if status != 0 {
        eprintln!("\nRemoval failed, reverting {file_label} to its original content.");
        std::fs::write(&file, &backup)
            .with_context(|| format!("cannot restore {}", file.display()))?;
    }
    // Is the package still in the local repository? That repository is
    // vendor/composer/installed.json minus the packages whose install path
    // no longer exists (`Factory::purgePackages` via
    // `LibraryInstaller::isInstalled`); a metapackage always counts. Not
    // checked on a dry run.
    if dry_run {
        return Ok(status);
    }
    for package in &packages {
        if locally_installed(&project, package) {
            eprintln!("Removal failed, {package} is still present, it may be required by another package. See `composer why {package}`.");
            return Ok(2);
        }
    }
    Ok(status)
}

/// `$composer->getRepositoryManager()->getLocalRepository()->findPackages($name)`
/// is non-empty.
fn locally_installed(project: &std::path::Path, name: &str) -> bool {
    installed_packages(project)
        .iter()
        .any(|p| p.get("name").and_then(|n| n.as_str()) == Some(name))
}

/// `Locker::getMissingRequirementInfo`: the root requirements the lock
/// does not satisfy — `require` against the non-dev lock, `require-dev`
/// against the whole lock — with replacers and providers counting, and
/// the three fixed lines when anything is missing.
fn missing_requirement_info(
    manifest: &serde_json::Value,
    lock: &serde_json::Value,
    include_dev: bool,
) -> anyhow::Result<Vec<String>> {
    use vivacity_resolver::constraint::{parse_constraints, Constraint, Op};
    use vivacity_resolver::package::{Origin, Package};
    let mut out = Vec::new();
    let mut missing = false;
    let root_name = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("__root__");
    let mut sets: Vec<(bool, &str, &str)> = vec![(false, "require", "Required")];
    if include_dev {
        sets.push((true, "require-dev", "Required (in require-dev)"));
    }
    for (with_dev, key, description) in sets {
        let mut arena: Vec<Package> = Vec::new();
        let repo =
            vivacity_resolver::repository::locked_repository_with(lock, &mut arena, with_dev)
                .map_err(|e| anyhow::anyhow!("composer.lock: {}", e.0))?;
        let Some(links) = manifest.get(key).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for (target, pretty) in links {
            let target = target.to_lowercase();
            let Some(pretty) = pretty.as_str() else {
                continue;
            };
            if vivacity_resolver::platform::is_platform_package(&target) || pretty == "self.version"
            {
                continue;
            }
            let Ok(parsed) = parse_constraints(pretty) else {
                continue;
            };
            // `findPackagesWithReplacersAndProviders($target, $constraint)`
            // over [lock repo, root repo]: a package of that name, or a
            // replacer/provider whose link matches the constraint.
            let matches = |idx: usize, constraint: Option<&Constraint>| -> bool {
                let p = &arena[idx];
                if p.name == target {
                    return constraint
                        .is_none_or(|c| c.matches(&Constraint::new(Op::Eq, p.version.clone())));
                }
                p.replaces.iter().chain(p.provides.iter()).any(|l| {
                    l.target == target && constraint.is_none_or(|c| c.matches(&l.constraint))
                })
            };
            let root_matches = |constraint: Option<&Constraint>| -> bool {
                root_name.to_lowercase() == target && constraint.is_none()
            };
            if repo.iter().any(|&i| matches(i, Some(&parsed.constraint)))
                || root_matches(Some(&parsed.constraint))
            {
                continue;
            }
            let any: Option<usize> = repo.iter().copied().find(|&i| matches(i, None));
            let line = match any {
                Some(idx) => {
                    let p = &arena[idx];
                    let mut description_text = p.pretty_version.clone();
                    if p.name != target {
                        for (links, text) in [
                            (&p.replaces, "replaced as {} by {}"),
                            (&p.provides, "provided as {} by {}"),
                        ] {
                            if let Some(l) = links.iter().find(|l| l.target == target) {
                                description_text =
                                    text.replacen("{}", &l.pretty_constraint, 1).replacen(
                                        "{}",
                                        &format!("{} {}", p.pretty_name, p.pretty_version),
                                        1,
                                    );
                                break;
                            }
                        }
                    }
                    format!("- {description} package \"{target}\" is in the lock file as \"{description_text}\" but that does not satisfy your constraint \"{pretty}\".")
                }
                None => {
                    format!("- {description} package \"{target}\" is not present in the lock file.")
                }
            };
            out.push(line);
            missing = true;
        }
        let _ = Origin::Locked;
    }
    if missing {
        out.push("This usually happens when composer files are incorrectly merged or the composer.json file is manually edited.".to_owned());
        out.push("Read more about correctly resolving merge conflicts https://getcomposer.org/doc/articles/resolving-merge-conflicts.md".to_owned());
        out.push("and prefer using the \"require\" command over editing the composer.json file directly https://getcomposer.org/doc/03-cli.md#require-r".to_owned());
    }
    Ok(out)
}

/// `remove --unused`: the lock's packages (non-dev) required neither by
/// the root nor by any package reachable from it, in lock order.
fn unused_locked_packages(manifest: &serde_json::Value, lock: &serde_json::Value) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut required: BTreeSet<String> = BTreeSet::new();
    for section in ["require", "require-dev"] {
        if let Some(m) = manifest.get(section).and_then(|v| v.as_object()) {
            required.extend(m.keys().map(|k| k.to_lowercase()));
        }
    }
    let mut locked: Vec<&serde_json::Value> = lock
        .get("packages")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let names = |p: &serde_json::Value| -> Vec<String> {
        let mut out = vec![p
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_lowercase()];
        for key in ["replace", "provide"] {
            if let Some(m) = p.get(key).and_then(|v| v.as_object()) {
                out.extend(m.keys().map(|k| k.to_lowercase()));
            }
        }
        out
    };
    loop {
        let mut found = false;
        let mut i = 0;
        while i < locked.len() {
            let p = locked[i];
            if names(p).iter().any(|n| required.contains(n)) {
                if let Some(m) = p.get("require").and_then(|v| v.as_object()) {
                    required.extend(m.keys().map(|k| k.to_lowercase()));
                }
                found = true;
                locked.remove(i);
            } else {
                i += 1;
            }
        }
        if !found {
            break;
        }
    }
    locked
        .iter()
        .map(|p| {
            p.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_lowercase()
        })
        .collect()
}
