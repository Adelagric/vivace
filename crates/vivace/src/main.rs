//! vivace — installeur Composer-compatible rapide.
//! stdout : rien pour l'instant (réservé aux sorties machine) ; tout le
//! narratif va sur stderr. Codes retour : 0 ok, 1 erreur d'exécution,
//! 2 usage (clap), 3 hors-scope sans fallback possible, 4 plateforme.

use anyhow::Context as _;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "vivace",
    version,
    about = "Fast, Composer-compatible installer"
)]
enum Cli {
    /// Installe les dépendances depuis composer.lock (drop-in `composer install`).
    Install(InstallArgs),
    /// Régénère l'autoloader (drop-in `composer dump-autoload`).
    #[command(name = "dump-autoload", alias = "dumpautoload")]
    DumpAutoload(DumpArgs),
    /// Résout les dépendances et écrit composer.lock (drop-in `composer update`).
    #[command(alias = "upgrade")]
    Update(UpdateArgs),
}

#[derive(clap::Args, Debug)]
struct UpdateArgs {
    /// Écrire le lock sans installer.
    #[arg(long)]
    no_install: bool,
    /// Ne pas installer les paquets de require-dev (ils sont quand même résolus).
    #[arg(long)]
    no_dev: bool,
    /// Ne pas générer l'autoloader.
    #[arg(long)]
    no_autoloader: bool,
    #[arg(short = 'o', long)]
    optimize_autoloader: bool,
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    /// Accepté pour compatibilité : vivace n'exécute jamais les scripts.
    #[arg(long)]
    no_scripts: bool,
    #[arg(long)]
    no_plugins: bool,
    /// Accepté pour compatibilité : vivace n'audite pas (encore).
    #[arg(long)]
    no_audit: bool,
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
}

#[derive(clap::Args, Debug)]
struct DumpArgs {
    /// Ne pas inclure les paquets de require-dev dans l'autoloader.
    #[arg(long)]
    no_dev: bool,
    /// Classmap optimisée : tous les répertoires PSR sont scannés.
    #[arg(short = 'o', long)]
    optimize: bool,
    /// Classmap autoritaire (implique -o).
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    #[arg(long)]
    ignore_platform_reqs: bool,
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    /// Comme Composer : aucun plugin, même émulé (composer/installers).
    #[arg(long)]
    no_plugins: bool,
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct InstallArgs {
    /// Ne pas installer les paquets de require-dev.
    #[arg(long)]
    no_dev: bool,
    /// Ne pas générer l'autoloader.
    #[arg(long)]
    no_autoloader: bool,
    /// Classmap optimisée (`-o`).
    #[arg(short = 'o', long)]
    optimize_autoloader: bool,
    /// Classmap autoritaire (`-a`, implique -o).
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    /// Accepté pour compatibilité : vivace n'exécute jamais les scripts.
    #[arg(long)]
    no_scripts: bool,
    /// Comme Composer : aucun plugin, même émulé (composer/installers) — tout
    /// s'installe dans vendor/.
    #[arg(long)]
    no_plugins: bool,
    /// Ignorer toutes les exigences de plateforme.
    #[arg(long)]
    ignore_platform_reqs: bool,
    /// Ignorer une exigence de plateforme précise (répétable, motifs `ext-*`).
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    /// Ne jamais déléguer à composer (échoue explicitement hors scope).
    #[arg(long)]
    no_fallback: bool,
    /// N'utiliser que les caches locaux (aucun accès réseau).
    #[arg(long)]
    offline: bool,
    /// Répertoire du projet (défaut : répertoire courant).
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
}

/// `VIVACE_TRACE=1` : durée de chaque phase sur stderr (diagnostic perf).
fn trace(label: &str, since: std::time::Instant) {
    if std::env::var_os("VIVACE_TRACE").is_some() {
        eprintln!(
            "trace: {label:<22} {:>7.1} ms",
            since.elapsed().as_secs_f64() * 1000.0
        );
    }
}

fn main() -> anyhow::Result<()> {
    let code = match Cli::parse() {
        Cli::Install(args) => run_install(&args)?,
        Cli::DumpAutoload(args) => run_dump(&args)?,
        Cli::Update(args) => run_update(&args)?,
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Racine du projet, absolue : tous les chemins relatifs écrits dans vendor/
/// (proxies bin, install-path) en découlent et ne doivent pas dépendre du
/// répertoire courant.
fn project_dir(working_dir: Option<&std::path::Path>) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot determine the current directory")?;
    Ok(match working_dir {
        Some(d) if d.is_absolute() => d.to_path_buf(),
        Some(d) => cwd.join(d),
        None => cwd,
    })
}

/// `Plugin::preAutoloadDump` du scaffold Drupal : écrit
/// vendor/drupal/DrupalInstalled.php et rend les entrées de classmap à
/// ajouter à la racine. Rien sans scaffold émulé.
fn scaffold_pre_dump(
    project: &std::path::Path,
    profile: Option<vivace_core::scaffold::Profile>,
    lock: &vivace_core::lock::Lock,
    manifest: &serde_json::Value,
    dev_mode: bool,
) -> anyhow::Result<Vec<String>> {
    let Some(profile) = profile else {
        return Ok(Vec::new());
    };
    let root = vivace_core::state::RootPackage::detect(manifest, project, dev_mode);
    let packages = vivace_core::scaffold::hash_packages(lock, dev_mode);
    let Some(pre) = vivace_core::scaffold::pre_autoload_dump(
        profile,
        "vendor",
        &packages,
        &vivace_core::scaffold::root_hash_package(&root),
    ) else {
        return Ok(Vec::new());
    };
    let dir = project.join("vendor/drupal");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    std::fs::write(dir.join("DrupalInstalled.php"), &pre.drupal_installed)
        .context("cannot write vendor/drupal/DrupalInstalled.php")?;
    Ok(pre.root_classmap)
}

/// Profil du scaffold Drupal pour `dump-autoload` : plugin verrouillé,
/// autorisé, et copie installée à empreinte connue.
fn scaffold_profile_installed(
    layout: &vivace_core::layout::Layout,
    lock: &vivace_core::lock::Lock,
    manifest: &serde_json::Value,
    dev_mode: bool,
    plugins_enabled: bool,
) -> anyhow::Result<Option<vivace_core::scaffold::Profile>> {
    use vivace_core::scaffold::PLUGIN;
    if !plugins_enabled || !lock.wanted_packages(dev_mode).any(|p| p.name() == PLUGIN) {
        return Ok(None);
    }
    match vivace_core::layout::plugin_allowed(manifest, PLUGIN) {
        vivace_core::layout::PluginVerdict::Allowed => {}
        vivace_core::layout::PluginVerdict::Blocked => return Ok(None),
        vivace_core::layout::PluginVerdict::Unlisted => anyhow::bail!(
            "{PLUGIN} is a plugin not covered by config.allow-plugins (Composer would refuse to run it)"
        ),
    }
    let Some(dir) = layout.abs(PLUGIN).filter(|d| d.is_dir()) else {
        return Ok(None);
    };
    let fp = vivace_core::scaffold::fingerprint(&dir)?;
    match vivace_core::scaffold::profile_for(&fp) {
        Some(p) => Ok(Some(p)),
        None => anyhow::bail!("{PLUGIN}: the installed plugin source is not emulated (fingerprint {}…); run `composer dump-autoload`", &fp[..12]),
    }
}

fn run_install(args: &InstallArgs) -> anyhow::Result<i32> {
    let t0 = std::time::Instant::now();
    let project = project_dir(args.working_dir.as_deref())?;
    let manifest_path = project.join("composer.json");
    let lock_path = project.join("composer.lock");

    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_text).context("invalid composer.json")?;
    if !lock_path.is_file() {
        anyhow::bail!(
            "no composer.lock in {} — vivace does not resolve dependencies yet, \
             run `composer update` first",
            project.display()
        );
    }
    let lock = vivace_core::lock::Lock::read(&lock_path)?;
    trace("read manifests", t0);

    // Fraîcheur du lock : même comportement que Composer, un avertissement.
    if let (Ok(actual), Some(expected)) = (
        vivace_core::content_hash::content_hash(&manifest_text),
        lock.content_hash.as_deref(),
    ) {
        if actual != expected {
            eprintln!(
                "Warning: The lock file is not up to date with the latest changes in composer.json. \
                 You may be getting outdated dependencies. It is recommended that you run `composer update` or `composer update <package name>`."
            );
        }
    }

    let with_dev = !args.no_dev && std::env::var("COMPOSER_NO_DEV").as_deref() != Ok("1");

    // Hors-scope → fallback exec composer (par défaut) ou erreur explicite.
    let scope = vivace_core::scope::analyze(&project, &lock, &manifest, with_dev, !args.no_plugins);
    if !scope.is_native_ok() {
        return fallback_or_fail(args, &project, &scope);
    }
    let Some(layout) = scope.layout.as_ref() else {
        anyhow::bail!("internal: scope is native but no layout was resolved");
    };
    if let Some(tag) = &layout.installers_tag {
        eprintln!("Note: composer/installers {tag} emulated natively (custom install paths)");
    }
    if vivace_core::runtime_stub::has_custom_runtime_options(&manifest) {
        let scope = vivace_core::scope::ScopeReport {
            issues: vec![vivace_core::scope::ScopeIssue::UnknownPlugin(
                "symfony/runtime with custom extra.runtime options".to_owned(),
            )],
            skipped_plugins: vec![],
            layout: None,
            scaffold: false,
        };
        return fallback_or_fail(args, &project, &scope);
    }
    for plugin in &scope.skipped_plugins {
        eprintln!("Note: plugin {plugin} installed as a plain library (vivace never runs plugins)");
    }
    trace("scope", t0);

    // Plateforme.
    let mut ignored = args.ignore_platform_req.clone();
    if args.ignore_platform_reqs {
        ignored.push("*".to_owned());
    }
    if ignored.iter().all(|p| p != "*") {
        match vivace_core::platform::Platform::detect()? {
            Some(mut platform) => {
                platform.apply_overrides(&manifest);
                let failures = vivace_core::platform::check(&lock, &platform, with_dev, &ignored);
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

    // Transaction.
    let store = Arc::new(vivace_core::store::Store::default_location());
    let auth = vivace_core::fetch::Auth::load(&project);
    let fetcher = Arc::new(vivace_core::fetch::Fetcher::new(
        vivace_core::fetch::composer_cache_dir(),
        auth,
    )?);
    let opts = vivace_core::installer::InstallOptions {
        with_dev,
        offline: args.offline,
        scaffold: scope.scaffold,
        ..Default::default()
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    let report = match runtime.block_on(vivace_core::installer::install(
        &project, &lock, &manifest, layout, store, fetcher, &opts,
    )) {
        Ok(r) => r,
        // Refus d'émulation détecté avant toute écriture : vendor/ est intact.
        Err(vivace_core::Error::Unsupported(msg)) => {
            let scope = vivace_core::scope::ScopeReport {
                issues: vec![vivace_core::scope::ScopeIssue::Layout(msg)],
                skipped_plugins: vec![],
                layout: None,
                scaffold: false,
            };
            return fallback_or_fail(args, &project, &scope);
        }
        Err(e) => return Err(e.into()),
    };
    if report.scaffold.is_some() {
        eprintln!("Note: drupal/core-composer-scaffold emulated natively (scaffold files, autoload references)");
    }

    trace("install transaction", t0);
    let mut autoload_note = String::new();
    if !args.no_autoloader {
        let extra = scaffold_pre_dump(
            &project,
            report.scaffold.as_ref().map(|s| s.profile),
            &lock,
            &manifest,
            with_dev,
        )?;
        let report = dump_autoload(
            &project,
            &lock,
            &manifest,
            layout,
            extra,
            with_dev,
            args.optimize_autoloader || args.classmap_authoritative,
            args.classmap_authoritative,
            args.ignore_platform_reqs,
            &args.ignore_platform_req,
        )?;
        autoload_note = format!(", autoloader with {} classes", report.classes);
        trace("autoload dump", t0);
    }
    // POST_INSTALL_CMD : le scaffold écrit après l'autoloader, comme le plugin.
    if let Some(sc) = &report.scaffold {
        sc.plan
            .apply()
            .context("cannot apply the Drupal scaffold")?;
        trace("scaffold", t0);
    }

    let warmed = if report.store_warmed > 0 {
        format!(", store warmed for {} packages", report.store_warmed)
    } else {
        String::new()
    };
    eprintln!(
        "vivace: {} installed, {} unchanged, {} removed ({} from store, {} from cache, {} from network){warmed}{autoload_note} in {:.2}s",
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

#[allow(clippy::too_many_arguments)]
fn dump_autoload(
    project: &std::path::Path,
    lock: &vivace_core::lock::Lock,
    manifest: &serde_json::Value,
    layout: &vivace_core::layout::Layout,
    extra_root_classmap: Vec<String>,
    dev_mode: bool,
    optimize: bool,
    authoritative: bool,
    ignore_all: bool,
    ignored: &[String],
) -> anyhow::Result<vivace_autoload::DumpReport> {
    let platform_check = match manifest.get("config").and_then(|c| c.get("platform-check")) {
        Some(serde_json::Value::Bool(false)) => vivace_autoload::PlatformCheckMode::Off,
        Some(serde_json::Value::Bool(true)) => vivace_autoload::PlatformCheckMode::Full,
        _ => vivace_autoload::PlatformCheckMode::PhpOnly,
    };
    // Comme InstallCommand : les flags OU la config du composer.json.
    let cfg_bool = |key: &str| {
        manifest
            .get("config")
            .and_then(|c| c.get(key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    let authoritative = authoritative || cfg_bool("classmap-authoritative");
    let optimize = optimize || authoritative || cfg_bool("optimize-autoloader");
    let opts = vivace_autoload::DumpOptions {
        dev_mode,
        optimize,
        authoritative,
        platform_check,
        ignore_all_platform_reqs: ignore_all || ignored.iter().any(|p| p == "*"),
        ignored_platform_reqs: ignored.to_vec(),
        suffix: None,
        classmap_cache: if std::env::var_os("VIVACE_NO_CLASSMAP_CACHE").is_some() {
            None
        } else {
            Some(vivace_autoload::ClassmapCacheConfig {
                store_root: vivace_core::platform::cache_dir().join("store"),
                cache_root: vivace_core::platform::cache_dir(),
            })
        },
        extra_root_classmap,
    };
    let report = vivace_autoload::dump(project, lock, manifest, layout, &opts)?;
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
    let lock = vivace_core::lock::Lock::read(&project.join("composer.lock"))?;
    // Mode dev : celui de l'état installé (installed.json), comme Composer.
    let installed_dev = std::fs::read_to_string(project.join("vendor/composer/installed.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("dev").and_then(serde_json::Value::as_bool))
        .unwrap_or(true);
    let dev_mode = !args.no_dev && installed_dev;
    let layout = match vivace_core::layout::Layout::resolve(
        &project,
        &lock,
        &manifest,
        dev_mode,
        !args.no_plugins,
    ) {
        Ok(l) => l,
        Err(issues) => {
            eprintln!("vivace: this lock is outside what vivace handles natively:");
            for i in &issues {
                eprintln!("  - {i}");
            }
            eprintln!("Run `composer dump-autoload` instead.");
            return Ok(3);
        }
    };
    // preAutoloadDump lit le dépôt local (installed.json, dev compris si le
    // vendor a été posé avec) : l'état installé, pas le mode de ce dump.
    let profile =
        scaffold_profile_installed(&layout, &lock, &manifest, installed_dev, !args.no_plugins)?;
    let extra = scaffold_pre_dump(&project, profile, &lock, &manifest, installed_dev)?;
    let report = dump_autoload(
        &project,
        &lock,
        &manifest,
        &layout,
        extra,
        dev_mode,
        args.optimize || args.classmap_authoritative,
        args.classmap_authoritative,
        args.ignore_platform_reqs,
        &args.ignore_platform_req,
    )?;
    eprintln!(
        "vivace: autoloader generated ({} classes) in {:.2}s",
        report.classes,
        t0.elapsed().as_secs_f32()
    );
    Ok(0)
}

fn fallback_or_fail(
    args: &InstallArgs,
    project: &std::path::Path,
    scope: &vivace_core::scope::ScopeReport,
) -> anyhow::Result<i32> {
    eprintln!("vivace: this lock is outside what vivace handles natively:");
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
    eprintln!("vivace: delegating to `composer install`…");
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
    {
        use std::os::unix::process::CommandExt as _;
        let err = cmd.exec();
        Err(err).context("cannot exec composer")
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("cannot run composer")?;
        Ok(status.code().unwrap_or(1))
    }
}

fn which_composer() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("composer"))
        .find(|c| c.is_file())
}

/// `composer update` : résolution (port exact du solveur de Composer),
/// écriture du lock si ses données changent, puis `install`.
fn run_update(args: &UpdateArgs) -> anyhow::Result<i32> {
    use vivace_resolver::platform_filter::PlatformRequirementFilter;
    use vivace_resolver::session::UpdateSession;
    let t0 = std::time::Instant::now();
    let project = project_dir(args.working_dir.as_deref())?;
    let manifest_path = project.join("composer.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let home = vivace_core::fetch::composer_home();
    // Transport réseau des dépôts composer distants : le Fetcher de
    // vivace-core (auth de Composer, retries), rendu synchrone.
    let runtime =
        Arc::new(tokio::runtime::Runtime::new().context("cannot start the async runtime")?);
    let fetcher = Arc::new(vivace_core::fetch::Fetcher::new(
        vivace_core::fetch::composer_cache_dir(),
        vivace_core::fetch::Auth::load(&project),
    )?);
    let offline = args.offline;
    let http: vivace_resolver::repository::HttpFetch = {
        let runtime = runtime.clone();
        Arc::new(move |url: &str| {
            if offline {
                return Err(format!("offline: cannot fetch {url}"));
            }
            runtime
                .block_on(fetcher.metadata_bytes(url))
                .map_err(|e| e.to_string())
        })
    };
    let mut session = UpdateSession::prepare_with(&project, home.as_deref(), true, Some(http))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    trace("prepare", t0);
    session.prefer_stable = args.prefer_stable;
    session.prefer_lowest = args.prefer_lowest;
    let filter = if args.ignore_platform_reqs {
        PlatformRequirementFilter::IgnoreAll
    } else if !args.ignore_platform_req.is_empty() {
        PlatformRequirementFilter::from_list(&args.ignore_platform_req)
    } else {
        PlatformRequirementFilter::IgnoreNothing
    };
    eprintln!("Loading composer repositories with package information");
    eprintln!("Updating dependencies");
    let (lock, report) = session
        .update(&manifest_text, &filter)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    trace("resolve", t0);

    let lock_path = project.join("composer.lock");
    let mut text =
        vivace_core::phpjson::php_json_encode_with(&lock, vivace_core::phpjson::FLAGS_JSONFILE)?;
    text.push('\n');
    // `Locker::setLockData` : réécrit seulement si les données changent
    // (comparées après un même ré-encodage, comme `$lock !== getLockData()`).
    let unchanged = std::fs::read_to_string(&lock_path)
        .ok()
        .and_then(|old| serde_json::from_str::<serde_json::Value>(&old).ok())
        .and_then(|old| {
            vivace_core::phpjson::php_json_encode_with(&old, vivace_core::phpjson::FLAGS_JSONFILE)
                .ok()
        })
        .is_some_and(|old| old + "\n" == text);
    if report.transaction.transaction.operations.is_empty() {
        eprintln!("Nothing to modify in lock file");
    } else {
        let ops = &report.transaction.transaction.operations;
        let count = |f: &dyn Fn(&vivace_resolver::transaction::Operation) -> bool| {
            ops.iter().filter(|o| f(o)).count()
        };
        use vivace_resolver::transaction::Operation;
        let installs = count(&|o| matches!(o, Operation::Install(_)));
        let updates = count(&|o| matches!(o, Operation::Update(..)));
        let removals = count(&|o| matches!(o, Operation::Uninstall(_)));
        eprintln!(
            "Lock file operations: {installs} install{}, {updates} update{}, {removals} removal{}",
            if installs == 1 { "" } else { "s" },
            if updates == 1 { "" } else { "s" },
            if removals == 1 { "" } else { "s" }
        );
    }
    if !unchanged {
        eprintln!("Writing lock file");
        std::fs::write(&lock_path, &text)
            .with_context(|| format!("cannot write {}", lock_path.display()))?;
    }
    trace("write lock", t0);
    if args.no_install {
        return Ok(0);
    }
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
        working_dir: args.working_dir.clone(),
    })
}
