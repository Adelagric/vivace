//! The install transaction: diff (lock vs installed state), parallel fetch
//! into the store, store-to-vendor clone, bin proxies, state files, runtime
//! stub. Idempotent (rerun after an interruption, it converges): the reference
//! state is `installed.json` + the presence of the directories, and each
//! package is laid out by cloning into a previously removed vendor/<name>.

use crate::error::{Error, Result};
use crate::fetch::{Fetcher, Provenance};
use crate::layout::Layout;
use crate::lock::{Lock, LockPackage};
use crate::state::RootPackage;
use crate::store::Store;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct InstallOptions {
    pub with_dev: bool,
    pub offline: bool,
    /// Download/extraction parallelism.
    pub jobs: usize,
    /// drupal/core-composer-scaffold locked and allowed (scope): check its
    /// source and plan the scaffold before any write.
    pub scaffold: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        InstallOptions {
            with_dev: true,
            offline: false,
            jobs: 16,
            scaffold: false,
        }
    }
}

/// What the CLI applies after the transaction and the autoloader when the
/// Drupal scaffold is emulated.
#[derive(Debug, Clone)]
pub struct ScaffoldOutcome {
    pub profile: crate::scaffold::Profile,
    pub plan: crate::scaffold::Plan,
    /// Canonical project root (the plugin's physical `getcwd()`).
    pub root: PathBuf,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub from_cache: usize,
    pub from_network: usize,
    pub store_hits: usize,
    /// Unchanged packages extracted into the store (pre-existing vendor).
    pub store_warmed: usize,
    /// Drupal scaffold plan, to apply after the autoloader.
    pub scaffold: Option<ScaffoldOutcome>,
}

/// Installed identity of a package: version + dist reference.
fn identity(p: &LockPackage) -> (String, String) {
    (
        p.version().to_owned(),
        p.dist_reference().unwrap_or("").to_owned(),
    )
}

fn installed_identities(vendor: &Path) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    let path = vendor.join("composer/installed.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    for p in v["packages"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().unwrap_or_default();
        let version = p["version"].as_str().unwrap_or_default();
        let reference = p["dist"]["reference"].as_str().unwrap_or_default();
        out.insert(name.to_owned(), (version.to_owned(), reference.to_owned()));
    }
    out
}

pub async fn install(
    _project_dir: &Path,
    lock: &Lock,
    root_manifest: &Value,
    layout: &Layout,
    store: Arc<Store>,
    fetcher: Arc<Fetcher>,
    opts: &InstallOptions,
) -> Result<InstallReport> {
    // Absolute root (the layout's): the relative paths of the proxies and of
    // the state files must not depend on a relative --working-dir.
    let project_dir = layout.root();
    let vendor = project_dir.join("vendor");
    std::fs::create_dir_all(&vendor).map_err(Error::io(&vendor))?;

    let mut report = InstallReport::default();
    let wanted: Vec<&LockPackage> = lock.wanted_packages(opts.with_dev).collect();
    let wanted_names: std::collections::BTreeSet<&str> = wanted.iter().map(|p| p.name()).collect();
    let previous = installed_identities(&vendor);

    // To lay out: changed identity, or missing directory. Unchanged packages
    // whose store entry is missing (vendor/ laid out by Composer before vivace)
    // are extracted into the store without being re-cloned: the classmap
    // cache applies from the next run on.
    let mut to_install: Vec<&LockPackage> = Vec::new();
    let mut to_warm: Vec<&LockPackage> = Vec::new();
    for p in &wanted {
        if p.is_metapackage() {
            continue;
        }
        let unchanged = previous.get(p.name()) == Some(&identity(p))
            && layout.abs(p.name()).is_some_and(|d| d.is_dir());
        if unchanged {
            report.unchanged += 1;
            if !store.contains(p.name(), p.version(), p.dist_reference()) {
                to_warm.push(p);
            }
        } else {
            to_install.push(p);
        }
    }

    // Fetch + extraction into the store, with bounded parallelism. Packages to
    // "warm" only use the local cache (never the network) and their failure
    // is silent: it is an optimisation, not an obligation.
    let sem = Arc::new(tokio::sync::Semaphore::new(opts.jobs.max(1)));
    let mut tasks = tokio::task::JoinSet::new();
    let warm_names: std::collections::BTreeSet<&str> = to_warm.iter().map(|p| p.name()).collect();
    for p in to_install.iter().chain(to_warm.iter()) {
        if store.contains(p.name(), p.version(), p.dist_reference()) {
            report.store_hits += 1;
            continue;
        }
        let warm_only =
            warm_names.contains(p.name()) && !to_install.iter().any(|q| q.name() == p.name());
        let (name, version) = (p.name().to_owned(), p.version().to_owned());
        let dist_ref = p.dist_reference().map(str::to_owned);
        let url = p
            .dist_url()
            .ok_or_else(|| Error::Http {
                url: name.clone(),
                message:
                    "paquet sans dist url (le détecteur de scope aurait dû router en fallback)"
                        .to_owned(),
            })?
            .to_owned();
        let shasum = p.dist_shasum().map(str::to_owned);
        let (store, fetcher, sem) = (store.clone(), fetcher.clone(), sem.clone());
        let offline = opts.offline || warm_only;
        tasks.spawn(async move {
            let _permit = sem.acquire().await.map_err(|_| Error::Http {
                url: url.clone(),
                message: "semaphore fermé".to_owned(),
            })?;
            let fetched = fetcher
                .dist_bytes(&name, &url, shasum.as_deref(), offline)
                .await;
            let (bytes, provenance) = match fetched {
                Ok(v) => v,
                // Warming: zip missing from the cache, do not insist.
                Err(_) if warm_only => return Ok::<Option<Provenance>, Error>(None),
                Err(e) => return Err(e),
            };
            let store_name = name.clone();
            let version2 = version.clone();
            let dist_ref2 = dist_ref.clone();
            tokio::task::spawn_blocking(move || {
                store.ensure(&store_name, &version2, dist_ref2.as_deref(), &bytes)
            })
            .await
            .map_err(|e| Error::Http {
                url: name.clone(),
                message: format!("tâche d'extraction interrompue: {e}"),
            })??;
            Ok::<Option<Provenance>, Error>(Some(provenance))
        });
    }
    while let Some(joined) = tasks.join_next().await {
        let provenance = joined.map_err(|e| Error::Http {
            url: "join".to_owned(),
            message: e.to_string(),
        })??;
        match provenance {
            Some(Provenance::Cache) => report.from_cache += 1,
            Some(Provenance::Network) => report.from_network += 1,
            None => {}
        }
    }
    report.store_warmed = to_warm.len();

    // Drupal scaffold: plugin source checked (lock AND installed copy), plan
    // computed on the current on-disk state, before any removal, so that a
    // refusal leaves vendor/ intact and hands over to Composer.
    if opts.scaffold {
        report.scaffold = Some(plan_scaffold(
            lock,
            root_manifest,
            layout,
            &store,
            &wanted,
            &previous,
            opts.with_dev,
        )?);
    }

    // Removals: present before, no longer wanted, at the path validated by the
    // layout (old install-path = recomputed path, like LibraryInstaller).
    for name in previous.keys() {
        if !wanted_names.contains(name.as_str()) {
            report.removed += 1;
        }
    }
    for (_, dir) in layout.removals() {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(Error::io(&dir))?;
            prune_empty_parent(project_dir, &dir);
        }
    }

    // Layout: remove the old version, then clone from the store.
    for p in &to_install {
        let (Some(pkg_root), Some(dest)) = (layout.package_root(p.name()), layout.abs(p.name()))
        else {
            continue;
        };
        // Always start again from an empty package root (target-dir included).
        if pkg_root.exists() {
            std::fs::remove_dir_all(&pkg_root).map_err(Error::io(&pkg_root))?;
        }
        let src = store.entry_path(p.name(), p.version(), p.dist_reference());
        crate::clone::clone_tree(&src, &dest)?;
        report.installed += 1;
    }

    // Bin proxies: rebuilt for every wanted package, then purge of the
    // orphaned proxies (removed packages).
    for p in &wanted {
        let bins = p.bins();
        if let (false, Some(dir)) = (bins.is_empty(), layout.abs(p.name())) {
            crate::binproxy::install_binaries(&vendor, &dir, &bins)?;
        }
    }
    prune_orphan_bin_proxies(&vendor, &wanted)?;

    // State files + runtime stub.
    let root = RootPackage::detect(root_manifest, project_dir, opts.with_dev);
    crate::state::write_state_files(
        &vendor.join("composer"),
        lock,
        &root,
        root_manifest,
        opts.with_dev,
        layout,
    )?;
    if wanted.iter().any(|p| p.name() == "symfony/runtime") {
        crate::runtime_stub::write_stub(&vendor)?;
    }

    Ok(report)
}

/// Directory holding the source of a wanted package: the store entry if it
/// exists, else its current install path.
fn source_dir(store: &Store, layout: &Layout, p: &LockPackage) -> Option<PathBuf> {
    if store.contains(p.name(), p.version(), p.dist_reference()) {
        Some(store.entry_path(p.name(), p.version(), p.dist_reference()))
    } else {
        layout.abs(p.name()).filter(|d| d.is_dir())
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_scaffold(
    lock: &Lock,
    root_manifest: &Value,
    layout: &Layout,
    store: &Store,
    wanted: &[&LockPackage],
    previous: &BTreeMap<String, (String, String)>,
    with_dev: bool,
) -> Result<ScaffoldOutcome> {
    use crate::scaffold::{self, PLUGIN};
    let unsupported = |m: String| Error::Unsupported(m);
    let plugin = wanted
        .iter()
        .find(|p| p.name() == PLUGIN)
        .ok_or_else(|| unsupported(format!("{PLUGIN}: not in the lock")))?;
    let plugin_dir = source_dir(store, layout, plugin).ok_or_else(|| {
        unsupported(format!(
            "{PLUGIN}: source not available (offline and absent from the store)"
        ))
    })?;
    let fp = scaffold::fingerprint(&plugin_dir).map_err(Error::io(&plugin_dir))?;
    let profile = scaffold::profile_for(&fp).ok_or_else(|| {
        unsupported(format!(
            "{PLUGIN} {}: this plugin source is not emulated (fingerprint {}…)",
            plugin.version(),
            &fp[..12]
        ))
    })?;
    // Different installed version: Composer would run the old Handler with
    // the new Plugin, reproducible only if the sources are identical.
    if let Some((prev_version, _)) = previous.get(PLUGIN) {
        if prev_version != plugin.version() {
            let installed = layout.abs(PLUGIN).filter(|d| d.is_dir());
            let same = match installed {
                Some(dir) => scaffold::fingerprint(&dir).map_err(Error::io(&dir))? == fp,
                None => true,
            };
            if !same {
                return Err(unsupported(format!(
                    "{PLUGIN} is being upgraded from {prev_version} to {} with a different source: let Composer handle this transition",
                    plugin.version()
                )));
            }
        }
    }
    let root = std::fs::canonicalize(layout.root()).map_err(Error::io(layout.root()))?;
    // Metapackages stay visible (findPackage finds them and recurses into
    // their allowed-packages); their install path is empty in Composer, hence
    // an unreachable `/...` source if they declared a mapping.
    let packages: Vec<scaffold::ScaffoldPackage> = wanted
        .iter()
        .filter_map(|p| {
            let dir = if p.is_metapackage() {
                PathBuf::from("/")
            } else {
                source_dir(store, layout, p)?
            };
            Some(scaffold::ScaffoldPackage {
                name: p.name().to_owned(),
                dir,
                extra: p.raw.get("extra").cloned(),
            })
        })
        .collect();
    let root_name = root_manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("__root__");
    let plan = scaffold::plan(
        profile,
        &root,
        root_name,
        root_manifest.get("extra"),
        &packages,
    )
    .map_err(unsupported)?;
    let _ = (lock, with_dev);
    Ok(ScaffoldOutcome {
        profile,
        plan,
        root,
    })
}

/// `LibraryInstaller::uninstall`: the parent directory of the removed package
/// (vendor/<ns>, web/app/plugins...) is removed if empty, never the project
/// root.
fn prune_empty_parent(project_dir: &Path, removed: &Path) {
    let Some(parent) = removed.parent() else {
        return;
    };
    if parent == project_dir {
        return;
    }
    if std::fs::read_dir(parent)
        .map(|mut d| d.next().is_none())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(parent);
    }
}

fn prune_orphan_bin_proxies(vendor: &Path, wanted: &[&LockPackage]) -> Result<()> {
    let bin_dir = vendor.join("bin");
    let Ok(entries) = std::fs::read_dir(&bin_dir) else {
        return Ok(());
    };
    let mut expected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for p in wanted {
        for bin in p.bins() {
            let bin = bin.trim_start_matches("./");
            let link = bin.rsplit_once('/').map(|(_, f)| f).unwrap_or(bin);
            expected.insert(link.to_owned());
        }
    }
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !expected.contains(&file_name) && !file_name.ends_with(".bat") {
            let p = entry.path();
            std::fs::remove_file(&p).map_err(Error::io(&p))?;
        }
    }
    Ok(())
}

/// Utility path: the project's vendor/composer.
pub fn vendor_composer_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("vendor/composer")
}
