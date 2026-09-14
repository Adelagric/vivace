//! vivace — installeur Composer-compatible rapide.
//! stdout : rien pour l'instant (réservé aux sorties machine) ; tout le
//! narratif va sur stderr. Codes retour : 0 ok, 1 erreur d'exécution,
//! 2 usage (clap), 3 hors-scope sans fallback possible, 4 plateforme.
//!
//! Le binaire `vivace` n'est qu'un appel à [`run`] : un autre programme
//! peut embarquer les commandes telles quelles (`vivace::run(["vivace",
//! "install", …])`) et obtenir le même code retour.

mod require;

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
    /// Retire des paquets de composer.json, puis met à jour (drop-in `composer remove`).
    #[command(alias = "rm")]
    Remove(RemoveArgs),
    /// Ajoute des paquets à composer.json, puis met à jour (drop-in `composer require`).
    #[command(alias = "r")]
    Require(RequireArgs),
}

#[derive(clap::Args, Debug)]
struct RequireArgs {
    /// Paquets à requérir : `vendor/name`, `vendor/name:^1.0`, `vendor/name ^1.0`.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Ajouter à require-dev.
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
    /// Contrainte exacte (la version trouvée) au lieu de `^x.y`.
    #[arg(long)]
    fixed: bool,
    #[arg(long)]
    no_suggest: bool,
    #[arg(long)]
    no_progress: bool,
    /// Ne pas mettre à jour les dépendances (implique --no-install).
    #[arg(long)]
    no_update: bool,
    /// Écrire le lock sans installer.
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
    /// Mise à jour avec --no-dev.
    #[arg(long)]
    update_no_dev: bool,
    /// Mettre aussi à jour les dépendances, sauf celles requises par la racine (`-w`).
    #[arg(short = 'w', long)]
    update_with_dependencies: bool,
    /// Alias de --update-with-dependencies.
    #[arg(long)]
    with_dependencies: bool,
    /// Mettre aussi à jour les dépendances, exigences racine comprises (`-W`).
    #[arg(short = 'W', long)]
    update_with_all_dependencies: bool,
    /// Alias de --update-with-all-dependencies.
    #[arg(long)]
    with_all_dependencies: bool,
    #[arg(short = 'm', long)]
    minimal_changes: bool,
    #[arg(long)]
    prefer_stable: bool,
    #[arg(long)]
    prefer_lowest: bool,
    /// Trier les paquets de la section (aussi `config.sort-packages`).
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
    /// Ne pas générer l'autoloader.
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
    /// Paquets à retirer ; motifs `vendor/*` acceptés.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Retirer de require-dev.
    #[arg(long)]
    dev: bool,
    /// Ne pas mettre à jour les dépendances (implique --no-install).
    #[arg(long)]
    no_update: bool,
    /// Écrire le lock sans installer.
    #[arg(long)]
    no_install: bool,
    /// Accepté pour compatibilité : vivace n'audite pas (encore).
    #[arg(long)]
    no_audit: bool,
    /// Mise à jour avec --no-dev.
    #[arg(long)]
    update_no_dev: bool,
    /// Déprécié chez Composer (comportement par défaut).
    #[arg(short = 'w', long)]
    update_with_dependencies: bool,
    /// Mettre aussi à jour les dépendances qui sont des exigences racine (`-W`).
    #[arg(short = 'W', long)]
    update_with_all_dependencies: bool,
    /// Alias de --update-with-all-dependencies.
    #[arg(long)]
    with_all_dependencies: bool,
    /// Ne mettre à jour que les paquets listés.
    #[arg(long)]
    no_update_with_dependencies: bool,
    /// Retirer tous les paquets verrouillés que rien ne requiert.
    #[arg(long)]
    unused: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(short = 'm', long)]
    minimal_changes: bool,
    /// Acceptés pour compatibilité : vivace n'est jamais interactif, n'audite
    /// pas et n'applique pas de politique de blocage.
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
    /// Ne pas générer l'autoloader.
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
    /// Paquets à mettre à jour (les autres restent verrouillés) ; motifs `vendor/*` acceptés.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Mettre aussi à jour leurs dépendances, sauf celles requises par la racine (`-w`).
    #[arg(short = 'w', long)]
    with_dependencies: bool,
    /// Mettre aussi à jour leurs dépendances, exigences racine comprises (`-W`).
    #[arg(short = 'W', long)]
    with_all_dependencies: bool,
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
    /// Désactiver les politiques de blocage (avis de sécurité, malware).
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
    /// Désactiver les politiques de blocage (liste malware du lock).
    #[arg(long)]
    no_blocking: bool,
    #[arg(long)]
    no_security_blocking: bool,
    /// Refusé, comme chez Composer (`composer update --no-install`).
    #[arg(long)]
    no_install: bool,
    /// Vérifications (politiques, portée, plateforme) sans rien écrire ;
    /// Composer affiche en plus les opérations.
    #[arg(long)]
    dry_run: bool,
    /// Répertoire du projet (défaut : répertoire courant).
    #[arg(long, value_name = "DIR")]
    working_dir: Option<PathBuf>,
    /// Interne : lancer `composer install` en sous-processus au lieu de
    /// remplacer le processus (quand l'appelant a encore du travail après).
    #[arg(skip)]
    spawn_fallback: bool,
    /// Interne : installation qui suit une résolution (`doInstall` avec
    /// `alreadySolved`) — le pool du lock a déjà été filtré.
    #[arg(skip)]
    after_update: bool,
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

/// Exécute une ligne de commande vivace (`args[0]` est le nom du
/// programme, comme `std::env::args()`), erreurs d'exécution comprises :
/// le code retour est celui du binaire. Une erreur d'usage (clap) affiche
/// l'aide ou le message et rend 2 ; `--help`/`--version` rendent 0.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            // `e.exit()` écrit l'aide sur stdout, l'erreur sur stderr.
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

    // `Installer::doInstall` passe le pool du lock par le filtre de listes
    // en portée install : une version verrouillée signalée (liste malware)
    // n'est pas installée.
    if !args.after_update {
        let http = http_transport(&project, args.offline)?;
        let cache_repo_dir = vivace_core::fetch::composer_cache_dir().join("repo");
        let (problems, warnings) = vivace_resolver::session::install_policy_problems(
            &project,
            vivace_core::fetch::composer_home().as_deref(),
            Some(http),
            Some(&cache_repo_dir),
            with_dev,
            args.no_blocking || args.no_security_blocking,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        for w in &warnings {
            eprintln!("Warning: {w}");
        }
        if !problems.is_empty() {
            eprintln!("Your lock file does not contain a compatible set of packages. Please run composer update.");
            for (i, p) in problems.iter().enumerate() {
                eprintln!("\n  Problem {}\n    {p}", i + 1);
            }
            return Ok(2);
        }
        trace("policy", t0);
    }

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
    if args.dry_run {
        eprintln!("Installing dependencies from lock file (dry run)");
        return Ok(0);
    }

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
    std::env::split_paths(&path)
        .map(|d| d.join("composer"))
        .find(|c| c.is_file())
}

/// `composer update` : résolution (port exact du solveur de Composer),
/// écriture du lock si ses données changent, puis `install`.
/// Transport réseau des dépôts composer distants : le Fetcher de
/// vivace-core (auth de Composer, retries), rendu synchrone, avec un lot
/// parallèle (Composer : curl multi, 12 téléchargements à la fois).
fn http_transport(
    project: &std::path::Path,
    offline: bool,
) -> anyhow::Result<vivace_resolver::repository::HttpTransports> {
    let runtime =
        Arc::new(tokio::runtime::Runtime::new().context("cannot start the async runtime")?);
    let fetcher = Arc::new(vivace_core::fetch::Fetcher::new(
        vivace_core::fetch::composer_cache_dir(),
        vivace_core::fetch::Auth::load(project),
    )?);
    fn to_fetched(r: vivace_core::fetch::MetadataResponse) -> vivace_resolver::repository::Fetched {
        use vivace_core::fetch::MetadataResponse as M;
        use vivace_resolver::repository::Fetched as F;
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
    let http: vivace_resolver::repository::HttpFetch = {
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
    // Lot en parallèle (Composer : curl multi, 12 téléchargements à la fois).
    let http_many: vivace_resolver::repository::HttpFetchMany = {
        let runtime = runtime.clone();
        let fetcher = fetcher.clone();
        Arc::new(move |requests: &[vivace_resolver::repository::Request]| {
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
    // POST de formulaire (API des avis de sécurité).
    let http_post: vivace_resolver::repository::HttpPost = {
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
    // Mise à jour partielle : `update a/b [-w|-W]` (UpdateCommand).
    let env_flag = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0");
    for p in &args.packages {
        if ["lock", "nothing", "mirrors"].contains(&p.as_str()) {
            anyhow::bail!("`vivace update {p}` (lock file metadata refresh) is not supported yet");
        }
        if p.contains([' ', '=', ':']) {
            anyhow::bail!("temporary constraints (`update {p}`, `--with`) are not supported yet");
        }
    }
    let transitive = if args.with_all_dependencies || env_flag("COMPOSER_WITH_ALL_DEPENDENCIES") {
        vivace_resolver::pool::UpdateMode::ListedWithTransitiveDeps
    } else if args.with_dependencies || env_flag("COMPOSER_WITH_DEPENDENCIES") {
        vivace_resolver::pool::UpdateMode::ListedWithTransitiveDepsNoRootRequire
    } else {
        vivace_resolver::pool::UpdateMode::OnlyListed
    };
    let mut options = if args.packages.is_empty() {
        vivace_resolver::session::UpdateOptions::default()
    } else {
        vivace_resolver::session::UpdateOptions::partial(&args.packages, transitive)
    };
    options.no_blocking = args.no_blocking || args.no_security_blocking;
    // BaseCommand : COMPOSER_PREFER_STABLE / COMPOSER_PREFER_LOWEST valent
    // les options.
    let prefer_stable = args.prefer_stable || env_flag("COMPOSER_PREFER_STABLE");
    let prefer_lowest = args.prefer_lowest || env_flag("COMPOSER_PREFER_LOWEST");
    run_update_resolved(args, options, prefer_stable, prefer_lowest)
}

/// Résolution, écriture du lock et installation, une fois la liste et le
/// mode de mise à jour décidés (partagé par `update` et `remove`).
fn run_update_resolved(
    args: &UpdateArgs,
    options: vivace_resolver::session::UpdateOptions,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> anyhow::Result<i32> {
    let resolved = resolve_and_lock(args, options, prefer_stable, prefer_lowest)?;
    if resolved.status != 0 || args.no_install {
        return Ok(resolved.status);
    }
    install_after_update(args)
}

/// L'installation qui suit l'écriture du lock (`Installer::run` avec
/// `setInstall(true)`).
fn install_after_update(args: &UpdateArgs) -> anyhow::Result<i32> {
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
        dry_run: false,
        working_dir: args.working_dir.clone(),
        spawn_fallback: args.spawn_fallback,
        after_update: true,
    })
}

/// Résolution et écriture du lock : 0, ou 2 sur un ensemble insoluble.
/// Le résultat de la résolution : le statut (0, ou 2 sur un ensemble
/// insoluble) et les données du lock, écrites ou non (`config.lock: false`
/// résout sans écrire — Composer garde alors un lock « virtuel »).
struct Resolved {
    status: i32,
    lock: Option<serde_json::Value>,
}

fn resolve_and_lock(
    args: &UpdateArgs,
    options: vivace_resolver::session::UpdateOptions,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> anyhow::Result<Resolved> {
    use vivace_resolver::platform_filter::PlatformRequirementFilter;
    use vivace_resolver::session::UpdateSession;
    let t0 = std::time::Instant::now();
    let project = project_dir(args.working_dir.as_deref())?;
    let manifest_path = project.join("composer.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let home = vivace_core::fetch::composer_home();
    let http = http_transport(&project, args.offline)?;
    let cache_repo_dir = vivace_core::fetch::composer_cache_dir().join("repo");
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
    // BaseCommand : COMPOSER_IGNORE_PLATFORM_REQS vaut l'option,
    // COMPOSER_IGNORE_PLATFORM_REQ (liste séparée par des virgules) vaut la
    // liste quand elle est vide.
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
    // `Installer::run` : un ensemble insoluble vaut le code 2
    // (`SolverProblemsException`), toute autre erreur est une exception.
    let (lock, report) = match session.update(&manifest_text, &filter) {
        Ok(r) => r,
        Err(e) if e.kind == vivace_resolver::session::SessionErrorKind::Unsolvable => {
            eprintln!("{e}");
            return Ok(Resolved {
                status: 2,
                lock: None,
            });
        }
        Err(e) => return Err(anyhow::anyhow!("{e}")),
    };
    trace("resolve", t0);

    let lock_path = project.join("composer.lock");
    let mut text =
        vivace_core::phpjson::php_json_encode_with(&lock, vivace_core::phpjson::FLAGS_JSONFILE)?;
    // `JsonFile::read` retient l'indentation du lock existant, `write` la
    // réutilise.
    let old_text = std::fs::read_to_string(&lock_path).ok();
    if let Some(old) = &old_text {
        let indent = vivace_resolver::json_manipulator::detect_indenting(old)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if indent != "    " {
            text = vivace_resolver::config_source::reindent(&text, &indent);
        }
    }
    text.push('\n');
    // `JsonFile::write` passe par `filePutContentsIfModified` : le fichier
    // n'est réécrit que si ses octets changent. (`Locker::setLockData`
    // compare aussi les données décodées, mais `{}` contre `[]` y rend la
    // comparaison toujours fausse pour un lock ordinaire ; les octets sont
    // le critère observable.)
    let unchanged = old_text.as_deref() == Some(text.as_str());
    // `config.lock: false` : Composer résout sans écrire de lock.
    let write_lock = config_lock_enabled(&manifest_text);
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
    if write_lock {
        eprintln!("Writing lock file");
        if !unchanged {
            std::fs::write(&lock_path, &text)
                .with_context(|| format!("cannot write {}", lock_path.display()))?;
        }
    }
    trace("write lock", t0);
    Ok(Resolved {
        status: 0,
        lock: Some(lock),
    })
}

/// `Config::get('lock')` : projet puis config globale, `"false"` et les
/// valeurs fausses de PHP désactivent.
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

/// `join_all` minimal (pas de dépendance futures) : les tâches tournent
/// concurremment sur le runtime, résultats dans l'ordre.
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

/// `RemoveCommand::execute` : édition de composer.json par
/// `JsonConfigSource`, nettoyage d'`allow-plugins`, puis mise à jour
/// partielle (liste = paquets retirés, mode « avec dépendances sauf
/// exigences racine » par défaut), composer.json restauré si elle échoue.
fn run_remove(args: &RemoveArgs) -> anyhow::Result<i32> {
    use vivace_resolver::config_source::{composer_file, JsonConfigSource};
    let env_flag = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0");
    if args.dry_run {
        anyhow::bail!("`remove --dry-run` is not supported yet");
    }
    if args.minimal_changes || env_flag("COMPOSER_MINIMAL_CHANGES") {
        anyhow::bail!("`--minimal-changes` (update-with-minimal-changes) is not supported yet");
    }
    if args.packages.is_empty() && !args.unused {
        eprintln!("Not enough arguments (missing: \"packages\").");
        return Ok(1);
    }
    let project = project_dir(args.working_dir.as_deref())?;
    let file = composer_file(&project);
    // Le nom affiché par Composer est le chemin tel que `Factory` le donne.
    // Un autre manifeste (`COMPOSER=alt.json`, lock `alt.lock`) n'est pas
    // suivi par la résolution : refusé plutôt que résolu sur composer.json.
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

    // `Locker::isLocked` : un lock lisible qui a une clé `packages`.
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
    // `$composer[$linkType][strtolower($name)] = $name` : les clés en
    // minuscules s'ajoutent au tableau décodé (les originales restent, et
    // `array_keys` les voit toutes).
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
        let re = vivace_resolver::pool::package_name_regexp(pattern);
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
    for package in &packages {
        if let Some(name) = lookup(link_type, package) {
            json.remove_link(link_type, &name)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        } else if let Some(name) = lookup(alt_type, package) {
            eprintln!("{name} could not be found in {link_type} but it is present in {alt_type}");
        } else if has_section(link_type) && !grep(link_type, package).is_empty() {
            for matched in grep(link_type, package) {
                json.remove_link(link_type, &matched)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
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

    // `allow-plugins` : la config fusionnée (projet puis globale) ; les
    // entrées dont la clé est un paquet retiré partent du composer.json.
    let updated: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    if let Some(serde_json::Value::Object(allow)) = vivace_core::layout::merged_allow_plugins(
        updated.get("config").and_then(|c| c.get("allow-plugins")),
        vivace_core::layout::global_allow_plugins().as_ref(),
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

    // `Request::UPDATE_LISTED_WITH_TRANSITIVE_DEPS_NO_ROOT_REQUIRE` par
    // défaut ; `COMPOSER_WITH_ALL_DEPENDENCIES` vaut l'option (BaseCommand),
    // `COMPOSER_WITH_DEPENDENCIES` n'a pas d'option ici.
    let transitive = if args.update_with_all_dependencies
        || args.with_all_dependencies
        || env_flag("COMPOSER_WITH_ALL_DEPENDENCIES")
    {
        vivace_resolver::pool::UpdateMode::ListedWithTransitiveDeps
    } else if args.no_update_with_dependencies {
        vivace_resolver::pool::UpdateMode::OnlyListed
    } else {
        vivace_resolver::pool::UpdateMode::ListedWithTransitiveDepsNoRootRequire
    };
    let mut flags = String::new();
    if transitive == vivace_resolver::pool::UpdateMode::ListedWithTransitiveDeps {
        flags.push_str(" --with-all-dependencies");
    } else if transitive == vivace_resolver::pool::UpdateMode::OnlyListed {
        flags.push_str(" --with-dependencies");
    }
    eprintln!("Running composer update {}{flags}", packages.join(" "));
    // `setUpdateAllowList` seulement si un lock existe.
    let mut options = if locked_data.is_some() {
        vivace_resolver::session::UpdateOptions::partial(&packages, transitive)
    } else {
        vivace_resolver::session::UpdateOptions::default()
    };
    options.no_blocking = args.no_blocking || args.no_security_blocking;
    let update_args = UpdateArgs {
        packages: Vec::new(),
        with_dependencies: false,
        with_all_dependencies: false,
        no_install: args.no_install,
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
        // La suite de `remove` (restauration, vérification du dépôt local)
        // doit tourner même si l'installation est rendue à Composer.
        spawn_fallback: true,
    };
    // `remove` n'a pas d'option prefer-stable/lowest : seul le manifeste
    // compte.
    // Une exception (transport, manifeste invalide) remonte sans
    // restauration ; seul un statut non nul d'`Installer::run` restaure.
    let status = run_update_resolved(&update_args, options, false, false)?;
    if status != 0 {
        eprintln!("\nRemoval failed, reverting {file_label} to its original content.");
        std::fs::write(&file, &backup)
            .with_context(|| format!("cannot restore {}", file.display()))?;
    }
    // Le paquet est-il encore dans le dépôt local ? Celui-ci est
    // vendor/composer/installed.json moins les paquets dont le chemin
    // d'installation n'existe plus (`Factory::purgePackages` via
    // `LibraryInstaller::isInstalled`) ; un metapackage compte toujours.
    for package in &packages {
        if locally_installed(&project, package) {
            eprintln!("Removal failed, {package} is still present, it may be required by another package. See `composer why {package}`.");
            return Ok(2);
        }
    }
    Ok(status)
}

/// `$composer->getRepositoryManager()->getLocalRepository()->findPackages($name)`
/// non vide.
fn locally_installed(project: &std::path::Path, name: &str) -> bool {
    let composer_dir = project.join("vendor/composer");
    let Some(installed) = std::fs::read_to_string(composer_dir.join("installed.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return false;
    };
    let list = installed
        .get("packages")
        .and_then(|p| p.as_array())
        .cloned()
        .or_else(|| installed.as_array().cloned())
        .unwrap_or_default();
    list.iter().any(|p| {
        if p.get("name").and_then(|n| n.as_str()) != Some(name) {
            return false;
        }
        if p.get("type").and_then(|t| t.as_str()) == Some("metapackage") {
            return true;
        }
        match p.get("install-path").and_then(|v| v.as_str()) {
            Some(rel) => composer_dir.join(rel).exists(),
            None => true,
        }
    })
}

/// `remove --unused` : les paquets du lock (hors dev) que ni la racine ni
/// un paquet atteint depuis elle ne requièrent, dans l'ordre du lock.
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
