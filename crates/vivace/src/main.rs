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
    /// Accepté pour compatibilité : vivace n'exécute jamais les plugins.
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
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn run_install(args: &InstallArgs) -> anyhow::Result<i32> {
    let t0 = std::time::Instant::now();
    let project = match &args.working_dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().context("répertoire courant illisible")?,
    };
    let manifest_path = project.join("composer.json");
    let lock_path = project.join("composer.lock");

    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("lecture de {}", manifest_path.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_text).context("composer.json invalide")?;
    if !lock_path.is_file() {
        anyhow::bail!(
            "pas de composer.lock dans {} — la résolution n'est pas couverte par vivace v1, \
             lancer `composer update` d'abord",
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
    let scope = vivace_core::scope::analyze(&lock, &manifest, with_dev);
    if !scope.is_native_ok() {
        return fallback_or_fail(args, &project, &scope);
    }
    if vivace_core::runtime_stub::has_custom_runtime_options(&manifest) {
        let scope = vivace_core::scope::ScopeReport {
            issues: vec![vivace_core::scope::ScopeIssue::UnknownPlugin(
                "symfony/runtime (options extra.runtime personnalisées)".to_owned(),
            )],
            skipped_plugins: vec![],
        };
        return fallback_or_fail(args, &project, &scope);
    }
    for plugin in &scope.skipped_plugins {
        eprintln!("Note: plugin {plugin} installé comme library (non exécuté par vivace)");
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
                    eprintln!("Le lock ne peut pas être installé sur cette plateforme :");
                    for f in &failures {
                        let by = f
                            .required_by
                            .as_deref()
                            .map(|p| format!(" (requis par {p})"))
                            .unwrap_or_default();
                        eprintln!(
                            "  - {} {}{}: {:?}",
                            f.requirement, f.constraint, by, f.reason
                        );
                    }
                    eprintln!(
                        "Contourner avec --ignore-platform-req=<req> ou --ignore-platform-reqs."
                    );
                    return Ok(4);
                }
            }
            None => {
                if !lock.platform.is_empty() || (with_dev && !lock.platform_dev.is_empty()) {
                    eprintln!(
                        "Warning: php introuvable, exigences de plateforme non vérifiées \
                         (--ignore-platform-reqs pour masquer cet avertissement)"
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
        ..Default::default()
    };
    let runtime = tokio::runtime::Runtime::new().context("initialisation tokio")?;
    let report = runtime.block_on(vivace_core::installer::install(
        &project, &lock, &manifest, store, fetcher, &opts,
    ))?;

    trace("install transaction", t0);
    let mut autoload_note = String::new();
    if !args.no_autoloader {
        let report = dump_autoload(
            &project,
            &lock,
            &manifest,
            with_dev,
            args.optimize_autoloader || args.classmap_authoritative,
            args.classmap_authoritative,
            args.ignore_platform_reqs,
            &args.ignore_platform_req,
        )?;
        autoload_note = format!(", autoload {} classes", report.classes);
        trace("autoload dump", t0);
    }

    let warmed = if report.store_warmed > 0 {
        format!(", store chauffé pour {} paquets", report.store_warmed)
    } else {
        String::new()
    };
    eprintln!(
        "vivace: {} installés, {} inchangés, {} retirés ({} du store, {} du cache, {} du réseau){warmed}{autoload_note} en {:.2}s",
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
    };
    let report = vivace_autoload::dump(project, lock, manifest, &opts)?;
    for w in &report.warnings {
        eprintln!("{w}");
    }
    Ok(report)
}

fn run_dump(args: &DumpArgs) -> anyhow::Result<i32> {
    let t0 = std::time::Instant::now();
    let project = match &args.working_dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().context("répertoire courant illisible")?,
    };
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(project.join("composer.json")).context("lecture composer.json")?,
    )
    .context("composer.json invalide")?;
    let lock = vivace_core::lock::Lock::read(&project.join("composer.lock"))?;
    // Mode dev : celui de l'état installé (installed.json), comme Composer.
    let installed_dev = std::fs::read_to_string(project.join("vendor/composer/installed.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("dev").and_then(serde_json::Value::as_bool))
        .unwrap_or(true);
    let dev_mode = !args.no_dev && installed_dev;
    let report = dump_autoload(
        &project,
        &lock,
        &manifest,
        dev_mode,
        args.optimize || args.classmap_authoritative,
        args.classmap_authoritative,
        args.ignore_platform_reqs,
        &args.ignore_platform_req,
    )?;
    eprintln!(
        "vivace: autoload généré ({} classes) en {:.2}s",
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
    eprintln!("vivace: lock hors du scope natif :");
    for issue in &scope.issues {
        eprintln!("  - {issue:?}");
    }
    if args.no_fallback {
        eprintln!("--no-fallback demandé : abandon explicite (aucun vendor/ partiel écrit).");
        return Ok(3);
    }
    let composer = which_composer();
    let Some(composer) = composer else {
        eprintln!(
            "composer introuvable pour le fallback — installer Composer ou retirer les éléments hors scope."
        );
        return Ok(3);
    };
    eprintln!("vivace: délégation à `composer install`…");
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
        Err(err).context("exec composer")
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("lancement de composer")?;
        Ok(status.code().unwrap_or(1))
    }
}

fn which_composer() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("composer"))
        .find(|c| c.is_file())
}
