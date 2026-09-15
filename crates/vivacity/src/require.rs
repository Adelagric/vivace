//! `vivacity require`: port of `RequireCommand` (docs/reference/
//! RequireCommand.php) and of the part of `PackageDiscoveryTrait` it
//! reaches outside interactive mode: `vendor/name[:constraint]` arguments,
//! constraint guessed by the `VersionSelector`, composer.json edited by
//! the `JsonManipulator`, partial update, then
//! `updateRequirementsAfterResolution` (final constraint taken from the
//! lock, `Locker::updateHash`), files restored if the resolution fails.

use anyhow::Context as _;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use vivacity_resolver::json_manipulator::JsonManipulator;
use vivacity_resolver::platform_filter::PlatformRequirementFilter;
use vivacity_resolver::session::{UpdateOptions, UpdateSession};
use vivacity_resolver::version::{looks_too_strict, parse_name_version_pairs};
use vivacity_resolver::version_selector::{
    find_best_candidate, find_recommended_require_version, platform_constraints,
};

use crate::{
    http_transport, install_after_update, project_dir, resolve_and_lock, RequireArgs, UpdateArgs,
};

/// What a run manipulates: paths, backups, creation state.
struct Files {
    file_label: String,
    json: PathBuf,
    lock: PathBuf,
    newly_created: bool,
    composer_backup: String,
    lock_backup: Option<String>,
}

impl Files {
    /// `revertComposerFile`.
    fn revert(&self) -> anyhow::Result<()> {
        if self.newly_created {
            eprintln!("\nInstallation failed, deleting {}.", self.file_label);
            let _ = std::fs::remove_file(&self.json);
            if self.lock.exists() {
                let _ = std::fs::remove_file(&self.lock);
            }
        } else {
            let msg = if self.lock_backup.is_some() {
                format!(" and {} to their ", lock_label(&self.file_label))
            } else {
                " to its ".to_owned()
            };
            eprintln!(
                "\nInstallation failed, reverting {}{msg}original content.",
                self.file_label
            );
            std::fs::write(&self.json, &self.composer_backup)
                .with_context(|| format!("cannot restore {}", self.json.display()))?;
            if let Some(lock) = &self.lock_backup {
                std::fs::write(&self.lock, lock)
                    .with_context(|| format!("cannot restore {}", self.lock.display()))?;
            }
        }
        Ok(())
    }
}

/// `Factory::getLockFile`: `composer.json` to `composer.lock`.
fn lock_label(file: &str) -> String {
    match file.strip_suffix(".json") {
        Some(stem) => format!("{stem}.lock"),
        None => format!("{file}.lock"),
    }
}

/// `config.<key>` from the project, then from COMPOSER_HOME/config.json.
pub fn config_value(manifest: &Value, key: &str) -> Option<Value> {
    if let Some(v) = manifest.get("config").and_then(|c| c.get(key)) {
        return Some(v.clone());
    }
    let home = vivacity_core::fetch::composer_home()?;
    let text = std::fs::read_to_string(home.join("config.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("config")?.get(key).cloned()
}

fn truthy(v: Option<Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !(s.is_empty() || s == "0"),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
        _ => false,
    }
}

/// `updateFile`: `updateFileCleanly` (manipulator), otherwise a full
/// rewrite of the decoded manifest.
fn update_file(
    path: &Path,
    new: &[(String, String)],
    require_key: &str,
    remove_key: &str,
    sort_packages: bool,
) -> anyhow::Result<()> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut manipulator = JsonManipulator::new(&contents).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut clean = true;
    for (package, constraint) in new {
        if !manipulator
            .add_link(require_key, package, constraint, sort_packages)
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            clean = false;
            break;
        }
        if !manipulator
            .remove_sub_node(remove_key, package)
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            clean = false;
            break;
        }
    }
    if clean {
        manipulator
            .remove_main_key_if_empty(remove_key)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::write(path, manipulator.contents())
            .with_context(|| format!("cannot write {}", path.display()))?;
        return Ok(());
    }
    // `$this->json->read()` then `write`: everything is re-encoded, without
    // JsonConfigSource's `stdClass` fixups.
    let mut definition: Value = serde_json::from_str(&contents)
        .with_context(|| format!("{} does not contain valid JSON", path.display()))?;
    let indent = vivacity_resolver::json_manipulator::detect_indenting(&contents)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Value::Object(root) = &mut definition {
        for (package, constraint) in new {
            if !matches!(root.get(require_key), Some(Value::Object(_))) {
                root.insert(require_key.to_owned(), Value::Object(Map::new()));
            }
            if let Some(Value::Object(section)) = root.get_mut(require_key) {
                section.insert(package.clone(), Value::String(constraint.clone()));
            }
            if let Some(Value::Object(section)) = root.get_mut(remove_key) {
                section.shift_remove(package);
                if section.is_empty() {
                    root.shift_remove(remove_key);
                }
            }
        }
    }
    let mut text = vivacity_core::phpjson::php_json_encode_with(
        &definition,
        vivacity_core::phpjson::FLAGS_JSONFILE,
    )?;
    if indent != "    " {
        text = vivacity_resolver::config_source::reindent(&text, &indent);
    }
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// `getPlatformExceptionDetails`.
fn platform_exception_details(
    session: &UpdateSession,
    candidate: usize,
    platform_overrides: &Map<String, Value>,
) -> String {
    use vivacity_resolver::constraint::{Constraint, Op};
    let pkg = &session.arena[candidate];
    let mut details = Vec::new();
    for link in pkg.requires.iter() {
        if !vivacity_resolver::platform::is_platform_package(&link.target) {
            continue;
        }
        let platform_pkg = session
            .platform
            .iter()
            .map(|&i| &session.arena[i])
            .find(|p| p.name == link.target);
        let Some(platform_pkg) = platform_pkg else {
            // `isPlatformPackageDisabled` (config.platform keys in
            // lowercase).
            let disabled = platform_overrides
                .iter()
                .any(|(k, v)| k.to_lowercase() == link.target && v == &Value::Bool(false));
            if disabled {
                details.push(format!(
                    "{} {} requires {} {} but it is disabled by your platform config. Enable it again with \"composer config platform.{} --unset\".",
                    pkg.pretty_name, pkg.pretty_version, link.target, link.pretty_constraint, link.target
                ));
            } else {
                details.push(format!(
                    "{} {} requires {} {} but it is not present.",
                    pkg.pretty_name, pkg.pretty_version, link.target, link.pretty_constraint
                ));
            }
            continue;
        };
        if !link
            .constraint
            .matches(&Constraint::new(Op::Eq, &platform_pkg.version))
        {
            let mut version = platform_pkg.pretty_version.clone();
            // `isset($platformExtra['config.platform'])`: the override's
            // description ("Package overridden via config.platform, actual:
            // …") follows the version.
            if platform_pkg
                .raw
                .get("extra")
                .and_then(|e| e.get("config.platform"))
                .is_some_and(|v| !v.is_null())
            {
                let description = platform_pkg
                    .raw
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                version.push_str(&format!(" ({description})"));
            }
            details.push(format!(
                "{} {} requires {} {} which does not match your installed version {version}.",
                pkg.pretty_name, pkg.pretty_version, link.target, link.pretty_constraint
            ));
        }
    }
    if details.is_empty() {
        return String::new();
    }
    format!(":\n  - {}", details.join("\n  - "))
}

/// `findBestVersionAndNameForPackage` outside interactive mode: the
/// canonical name and the constraint (`--fixed`: the pretty version), or
/// the error Composer raises.
fn find_best_version_and_name(
    session: &mut UpdateSession,
    name: &str,
    filter: &PlatformRequirementFilter,
    preferred_stability: &str,
    fixed: bool,
    platform_overrides: &Map<String, Value>,
) -> anyhow::Result<(String, String)> {
    let platform = platform_constraints(&session.platform, &session.arena);
    let best = |session: &mut UpdateSession,
                ignore_stability: bool,
                filter: &PlatformRequirementFilter,
                warn: bool|
     -> anyhow::Result<Option<usize>> {
        let candidates = session
            .find_packages_for_require(name, ignore_stability)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut warnings = Vec::new();
        let found = find_best_candidate(
            &candidates,
            &session.arena,
            preferred_stability,
            filter,
            &platform,
            &mut warnings,
        );
        if warn {
            for w in warnings.iter().filter(|w| !w.verbose) {
                eprintln!("{}", w.message);
            }
        }
        Ok(found)
    };
    let effective_minimum_stability = session.root.minimum_stability.clone();
    if let Some(idx) = best(session, false, filter, true)? {
        let pkg = &session.arena[idx];
        let constraint = if fixed {
            pkg.pretty_version.clone()
        } else {
            find_recommended_require_version(pkg, &session.arena, &session.php_version)
        };
        return Ok((pkg.pretty_name.clone(), constraint));
    }
    if filter.is_ignored(name) {
        return Ok((name.to_owned(), "*".to_owned()));
    }
    // `$repoSet->getProviders($name)`: Packagist's providers API is not
    // ported; no provider.
    let ignore_all = PlatformRequirementFilter::IgnoreAll;
    if !matches!(filter, PlatformRequirementFilter::IgnoreAll) {
        if let Some(candidate) = best(session, false, &ignore_all, false)? {
            anyhow::bail!(
                "Package {name} has requirements incompatible with your PHP version, PHP extensions and Composer version{}",
                platform_exception_details(session, candidate, platform_overrides)
            );
        }
    }
    if best(session, true, filter, false)?.is_some() {
        anyhow::bail!(
            "Could not find a version of package {name} matching your minimum-stability ({effective_minimum_stability}). Require it with an explicit version constraint allowing its desired stability."
        );
    }
    if !matches!(filter, PlatformRequirementFilter::IgnoreAll) {
        if let Some(candidate) = best(session, true, &ignore_all, false)? {
            let additional = if best(session, false, &ignore_all, false)?.is_none() {
                format!(
                    "\n\nAdditionally, the package was only found with a stability of \"{}\" while your minimum stability is \"{effective_minimum_stability}\".",
                    session.arena[candidate].stability
                )
            } else {
                String::new()
            };
            anyhow::bail!(
                "Could not find package {name} in any version matching your PHP version, PHP extensions and Composer version{}{additional}",
                platform_exception_details(session, candidate, platform_overrides)
            );
        }
    }
    // `findSimilar` (Packagist search): not ported.
    anyhow::bail!(
        "Could not find a matching version of package {name}. Check the package spelling, your version constraint and that the package is available in a stability which matches your minimum-stability ({effective_minimum_stability})."
    )
}

pub fn run_require(args: &RequireArgs) -> anyhow::Result<i32> {
    let env_flag = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0");
    if args.minimal_changes || env_flag("COMPOSER_MINIMAL_CHANGES") {
        anyhow::bail!("`--minimal-changes` (update-with-minimal-changes) is not supported yet");
    }
    if let Ok(f) = std::env::var("COMPOSER") {
        let f = f.trim();
        if !f.is_empty() && f != "composer.json" && f != "./composer.json" {
            anyhow::bail!("COMPOSER={f}: an alternate manifest is not supported yet");
        }
    }
    if args.no_suggest {
        eprintln!("You are using the deprecated option \"--no-suggest\". It has no effect and will break in Composer 3.");
    }
    let project = project_dir(args.working_dir.as_deref())?;
    let json = project.join("composer.json");
    let lock = project.join("composer.lock");
    let file_label = "./composer.json".to_owned();
    let newly_created = !json.exists();
    if newly_created {
        std::fs::write(&json, "{\n}\n")
            .with_context(|| format!("{file_label} could not be created."))?;
    }
    // An empty composer.json: `BaseCommand::initialize` (`tryComposer`)
    // fails on the invalid JSON even before RequireCommand's
    // `filesize === 0` branch.
    let files = Files {
        file_label: file_label.clone(),
        json: json.clone(),
        lock: lock.clone(),
        newly_created,
        composer_backup: std::fs::read_to_string(&json)
            .with_context(|| format!("{file_label} is not readable."))?,
        // `if ($this->lockBackup)`: an empty lock does not count.
        lock_backup: std::fs::read_to_string(&lock)
            .ok()
            .filter(|t| !t.is_empty()),
    };
    let manifest: Value = serde_json::from_str(&files.composer_backup)
        .with_context(|| format!("{file_label} does not contain valid JSON"))?;
    if truthy(config_value(&manifest, "update-with-minimal-changes")) {
        anyhow::bail!("config.update-with-minimal-changes is not supported yet");
    }

    if args.fixed {
        let package_type = match manifest.get("type") {
            Some(Value::String(t)) if !t.is_empty() && t != "0" => t.clone(),
            _ => "library".to_owned(),
        };
        if package_type != "project" && !args.dev {
            eprintln!("The \"--fixed\" option is only allowed for packages with a \"project\" type or for dev dependencies to prevent possible misuses.");
            if manifest.get("type").is_none_or(Value::is_null) {
                eprintln!("If your package is not a library, you can explicitly specify the \"type\" by using \"composer config type project\".");
            }
            return Ok(1);
        }
    }

    // `requireComposer()` + the composite repository [platform, repos]: the
    // resolution session provides both, on the current manifest.
    let home = vivacity_core::fetch::composer_home();
    let http = http_transport(&project, args.offline)?;
    let cache_repo_dir = vivacity_core::fetch::composer_cache_dir().join("repo");
    let session = UpdateSession::prepare_update(
        &project,
        home.as_deref(),
        true,
        Some(http),
        Some(&cache_repo_dir),
        &UpdateOptions::default(),
    );
    let mut session = match session {
        Ok(s) => s,
        Err(e) => {
            if newly_created {
                files.revert()?;
                anyhow::bail!("No composer.json present in the current directory ({file_label}), this may be the cause of the following exception.\n{e}");
            }
            return Err(anyhow::anyhow!("{e}"));
        }
    };
    let preferred_stability = if session.root.prefer_stable {
        "stable".to_owned()
    } else {
        session.root.minimum_stability.clone()
    };
    let ignore_all = args.ignore_platform_reqs || env_flag("COMPOSER_IGNORE_PLATFORM_REQS");
    let mut ignore_list = args.ignore_platform_req.clone();
    if !ignore_all && ignore_list.is_empty() {
        if let Ok(env) = std::env::var("COMPOSER_IGNORE_PLATFORM_REQ") {
            if !env.is_empty() {
                ignore_list = env.split(',').map(str::to_owned).collect();
            }
        }
    }
    let filter = if ignore_all {
        PlatformRequirementFilter::IgnoreAll
    } else if !ignore_list.is_empty() {
        PlatformRequirementFilter::from_list(&ignore_list)
    } else {
        PlatformRequirementFilter::IgnoreNothing
    };
    let platform_overrides = session.config.platform.clone();

    // `determineRequirements`.
    for p in &args.packages {
        if p.to_lowercase() == "as" {
            let msg = format!("Cannot use \"{p}\" as a separate argument. Quote the inline alias as one argument, e.g. \"vendor/package:dev-main as 1.2.x-dev\".");
            if newly_created {
                files.revert()?;
            }
            anyhow::bail!("{msg}");
        }
    }
    let use_best_version_constraint = args.no_update;
    let mut requirements: Vec<(String, String)> = Vec::new();
    for (name, version) in parse_name_version_pairs(&args.packages) {
        if let Some(v) = &version {
            if looks_too_strict(v) {
                eprintln!("The \"{v}\" constraint for \"{name}\" appears too strict and will likely not match what you want. See https://getcomposer.org/constraints");
            }
        }
        match version {
            Some(v) => requirements.push((name, v)),
            None => {
                let guessed = find_best_version_and_name(
                    &mut session,
                    &name,
                    &filter,
                    &preferred_stability,
                    args.fixed,
                    &platform_overrides,
                );
                let (canonical, constraint) = match guessed {
                    Ok(r) => r,
                    Err(e) => {
                        if newly_created {
                            files.revert()?;
                            anyhow::bail!("No composer.json present in the current directory ({file_label}), this may be the cause of the following exception.\n{e}");
                        }
                        return Err(e);
                    }
                };
                if use_best_version_constraint {
                    eprintln!("Using version {constraint} for {canonical}");
                    requirements.push((canonical, constraint));
                } else {
                    requirements.push((canonical, "guess".to_owned()));
                }
            }
        }
    }
    // `formatRequirements`: `name version` to an array (the last one wins).
    let mut formatted: Vec<(String, String)> = Vec::new();
    for (name, version) in requirements {
        match formatted.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = version,
            None => formatted.push((name, version)),
        }
    }
    let mut requirements = formatted;
    let (require_key, remove_key) = if args.dev {
        ("require-dev", "require")
    } else {
        ("require", "require-dev")
    };
    let mut to_guess: Vec<String> = Vec::new();
    for (name, constraint) in requirements.iter_mut() {
        if constraint == "guess" {
            *constraint = "*".to_owned();
            to_guess.push(name.clone());
        }
    }
    for (name, constraint) in &requirements {
        if name.to_lowercase() == session.root.package.name {
            eprintln!("Root package '{name}' cannot require itself in its composer.json");
            return Ok(1);
        }
        if constraint == "self.version" {
            continue;
        }
        if let Err(e) = vivacity_resolver::constraint::parse_constraints(constraint) {
            anyhow::bail!("{e}");
        }
    }
    // `getInconsistentRequireKeys`: warning only (non-interactive).
    let by_key = |section: &str| -> Vec<String> {
        manifest
            .get(section)
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut current: Vec<(String, &str)> = Vec::new();
    for k in by_key("require") {
        current.push((k, "require"));
    }
    for k in by_key("require-dev") {
        match current.iter_mut().find(|(n, _)| *n == k) {
            Some(e) => e.1 = "require-dev",
            None => current.push((k, "require-dev")),
        }
    }
    for (package, key) in &current {
        if requirements.iter().any(|(n, _)| n == package) && *key != require_key {
            eprintln!(
                "{package} is currently present in the {remove_key} key and you ran the command {} the --dev flag, which will move it to the {require_key} key.",
                if args.dev { "with" } else { "without" }
            );
        }
    }
    let sort_packages = args.sort_packages || truthy(config_value(&manifest, "sort-packages"));
    let first_require =
        newly_created || (by_key("require").is_empty() && by_key("require-dev").is_empty());

    // `--dry-run`: the file is not written (a newly created one is deleted
    // after the update); the root package is patched in memory instead.
    if !args.dry_run {
        update_file(&json, &requirements, require_key, remove_key, sort_packages)?;
    }
    eprintln!(
        "{file_label} has been {}",
        if newly_created { "created" } else { "updated" }
    );
    if args.no_update {
        return Ok(0);
    }

    // `doUpdate`.
    let transitive = if args.update_with_all_dependencies
        || args.with_all_dependencies
        || env_flag("COMPOSER_WITH_ALL_DEPENDENCIES")
    {
        Some(vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDeps)
    } else if args.update_with_dependencies
        || args.with_dependencies
        || env_flag("COMPOSER_WITH_DEPENDENCIES")
    {
        Some(vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDepsNoRootRequire)
    } else {
        None
    };
    let flags = match transitive {
        Some(vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDeps) => {
            " --with-all-dependencies"
        }
        Some(_) => " --with-dependencies",
        None => "",
    };
    let names: Vec<String> = requirements.iter().map(|(n, _)| n.clone()).collect();
    eprintln!("Running composer update {}{flags}", names.join(" "));
    // `Locker::isLocked`: a readable lock with a non-null `packages` key;
    // an unreadable lock is a ParsingException in `doUpdate`, hence restore
    // and error.
    let locked = match std::fs::read_to_string(&lock) {
        Ok(t) => match serde_json::from_str::<Value>(&t) {
            Ok(v) => v.get("packages").is_some_and(|p| !p.is_null()),
            Err(e) => {
                files.revert()?;
                anyhow::bail!(
                    "\"{}\" does not contain valid JSON\n{e}",
                    lock_label(&file_label)
                );
            }
        },
        Err(_) => false,
    };
    // Without a written lock (`config.lock: false`), Composer installs and
    // guesses from its virtual lock; vivacity only installs from the file.
    let lock_enabled = crate::config_lock_enabled(&files.composer_backup);
    if !lock_enabled && !args.no_install {
        anyhow::bail!("config.lock is false: `vivacity require` can only install from a written composer.lock (use --no-install)");
    }
    let mut options = if !first_require && locked {
        UpdateOptions::partial(
            &names,
            transitive.unwrap_or(vivacity_resolver::pool::UpdateMode::OnlyListed),
        )
    } else {
        UpdateOptions::default()
    };
    options.no_blocking = args.no_blocking || args.no_security_blocking;
    if args.dry_run {
        options.root_patch = Some(vivacity_resolver::root::RootPatch {
            requirements: requirements.clone(),
            dev: args.dev,
            removals: Vec::new(),
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
        spawn_fallback: true,
    };
    let prefer_stable = args.prefer_stable || env_flag("COMPOSER_PREFER_STABLE");
    let prefer_lowest = args.prefer_lowest || env_flag("COMPOSER_PREFER_LOWEST");
    // An exception before the end of the resolution restores the files;
    // after it (install), it propagates as is.
    let resolved = match resolve_and_lock(&update_args, options, prefer_stable, prefer_lowest) {
        Ok(r) => r,
        Err(e) => {
            files.revert()?;
            return Err(e);
        }
    };
    let status = resolved.status;
    if status != 0 {
        if status == 2 {
            if let Some((name, _)) = parse_name_version_pairs(&args.packages)
                .into_iter()
                .find(|(_, v)| v.is_none())
            {
                eprintln!("You can also try re-running composer require with an explicit version constraint, e.g. \"composer require {name}:*\" to figure out if any version is installable, or \"composer require {name}:^2.1\" if you know which you need.");
            }
        }
        files.revert()?;
        return Ok(status);
    }
    // After the resolution, a failing install restores nothing
    // (`dependencyResolutionCompleted`).
    if !args.no_install {
        let virtual_lock = if args.dry_run {
            resolved.lock.clone()
        } else {
            None
        };
        let status = install_after_update(&update_args, virtual_lock)?;
        if status != 0 {
            return Ok(status);
        }
    }
    crate::print_post_update(&resolved, &project, args.dry_run);
    let status = if !to_guess.is_empty() {
        update_requirements_after_resolution(
            &project,
            &json,
            &lock,
            &manifest,
            &session,
            resolved.lock.as_ref(),
            lock_enabled,
            &to_guess,
            require_key,
            remove_key,
            sort_packages,
            args.fixed,
            args.dry_run,
        )?
    } else {
        0
    };
    // `finally`: a json created for a dry run does not survive it.
    if args.dry_run && newly_created {
        let _ = std::fs::remove_file(&json);
    }
    Ok(status)
}

/// `updateRequirementsAfterResolution`: the final constraint of the
/// guessed packages, from the locked version (or the installed one without
/// a lock), then `Locker::updateHash` with the stability flags.
#[allow(clippy::too_many_arguments)]
fn update_requirements_after_resolution(
    project: &Path,
    json: &Path,
    lock_path: &Path,
    _manifest: &Value,
    session: &UpdateSession,
    resolved_lock: Option<&Value>,
    lock_enabled: bool,
    to_guess: &[String],
    require_key: &str,
    remove_key: &str,
    sort_packages: bool,
    fixed: bool,
    dry_run: bool,
) -> anyhow::Result<i32> {
    use vivacity_resolver::package::Origin;
    // `$locker->isLocked()`: the lock the resolution just produced
    // (written, or virtual with `config.lock: false`).
    let lock: Option<Value> = resolved_lock
        .cloned()
        .filter(|v| v.get("packages").is_some_and(|p| !p.is_null()));
    // `getLockedRepository(true)` or the local repository: the entries, in
    // order, loaded by the ArrayLoader; `findPackage($name, '*')` takes the
    // first one with that name.
    let entries: Vec<Value> = match &lock {
        Some(l) => ["packages", "packages-dev"]
            .iter()
            .flat_map(|k| {
                l.get(k)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
        None => std::fs::read_to_string(project.join("vendor/composer/installed.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| {
                v.get("packages")
                    .and_then(Value::as_array)
                    .cloned()
                    .or_else(|| v.as_array().cloned())
            })
            .unwrap_or_default(),
    };
    let mut requirements: Vec<(String, String)> = Vec::new();
    let lower_names: Vec<String> = to_guess.iter().map(|n| n.to_lowercase()).collect();
    for (package_name, lower) in to_guess.iter().zip(&lower_names) {
        let Some(entry) = entries
            .iter()
            .find(|e| e.get("name").and_then(Value::as_str) == Some(lower.as_str()))
        else {
            continue;
        };
        let (pkg, _) = vivacity_resolver::loader::load(entry, Origin::Locked, false)
            .map_err(|e| anyhow::anyhow!("{}: {}", package_name, e.0))?;
        let constraint = if fixed {
            pkg.pretty_version.clone()
        } else {
            find_recommended_require_version(&pkg, &session.arena, &session.php_version)
        };
        eprintln!("Using version {constraint} for {package_name}");
        if constraint.starts_with("dev-")
            && !["dev-main", "dev-master", "dev-trunk", "dev-latest"].contains(&constraint.as_str())
        {
            eprintln!("Version {constraint} looks like it may be a feature branch which is unlikely to keep working in the long run and may be in an unstable state");
        }
        requirements.push((package_name.clone(), constraint));
    }
    if dry_run {
        return Ok(0);
    }
    update_file(json, &requirements, require_key, remove_key, sort_packages)?;
    // `$locker->isLocked() && config.lock`: the freshly written lock gets
    // the new content-hash and the flags.
    let lock_now: Option<String> = std::fs::read_to_string(lock_path)
        .ok()
        .filter(|t| serde_json::from_str::<Value>(t).is_ok_and(|v| v.get("packages").is_some()));
    if let (Some(lock_text), true) = (lock_now, lock.is_some() && lock_enabled) {
        let mut flags = std::collections::BTreeMap::new();
        vivacity_resolver::root::extract_stability_flags(
            &requirements,
            &session.root.minimum_stability,
            &mut flags,
        );
        let manifest_text = std::fs::read_to_string(json)
            .with_context(|| format!("cannot read {}", json.display()))?;
        let new_text = locker_update_hash(&lock_text, &manifest_text, &flags)?;
        if new_text != lock_text {
            std::fs::write(lock_path, new_text)
                .with_context(|| format!("cannot write {}", lock_path.display()))?;
        }
    }
    Ok(0)
}

/// `Locker::updateHash` + `fixupJsonDataType`: the lock re-read, its
/// `content-hash` recomputed, the flags added, re-encoded with the
/// detected indentation.
fn locker_update_hash(
    lock_text: &str,
    manifest_text: &str,
    flags: &std::collections::BTreeMap<String, i32>,
) -> anyhow::Result<String> {
    use vivacity_core::phpjson::{empty_stdclass, php_json_encode_with, FLAGS_JSONFILE};
    let mut lock: Value = serde_json::from_str(lock_text).context("composer.lock")?;
    let indent = vivacity_resolver::json_manipulator::detect_indenting(lock_text)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let Value::Object(data) = &mut lock else {
        anyhow::bail!("composer.lock is not an object");
    };
    data.insert(
        "content-hash".into(),
        Value::String(vivacity_core::content_hash::content_hash(manifest_text)?),
    );
    // `$lockData['stability-flags'][$packageName] = $flag`: on a decoded
    // `{}` (empty array) as on an existing object.
    if !flags.is_empty() {
        let mut current = match data.get("stability-flags") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        for (name, flag) in flags {
            current.insert(name.clone(), Value::from(*flag));
        }
        data.insert("stability-flags".into(), Value::Object(current));
    }
    for key in ["stability-flags", "platform", "platform-dev"] {
        let empty = match data.get(key) {
            Some(Value::Object(m)) => m.is_empty(),
            Some(Value::Array(a)) => a.is_empty(),
            _ => false,
        };
        if empty {
            data.insert(key.into(), empty_stdclass());
        }
    }
    // `ksort($lockData['stability-flags'])`.
    if let Some(Value::Object(m)) = data.get("stability-flags") {
        if !m.contains_key(vivacity_core::phpjson::STDCLASS_MARKER) {
            let mut entries: Vec<(String, Value)> = m.clone().into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
            data.insert(
                "stability-flags".into(),
                Value::Object(entries.into_iter().collect()),
            );
        }
    }
    let mut text = php_json_encode_with(&lock, FLAGS_JSONFILE)?;
    if indent != "    " {
        text = vivacity_resolver::config_source::reindent(&text, &indent);
    }
    text.push('\n');
    Ok(text)
}
