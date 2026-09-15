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
}

impl Default for InstallOptions {
    fn default() -> Self {
        InstallOptions {
            with_dev: true,
            offline: false,
            jobs: 16,
        }
    }
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
    // whose store entry is missing (vendor/ laid out by Composer before vivacity)
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
                    "package without a dist url (the scope detector should have routed to the fallback)"
                        .to_owned(),
            })?
            .to_owned();
        let shasum = p.dist_shasum().map(str::to_owned);
        let (store, fetcher, sem) = (store.clone(), fetcher.clone(), sem.clone());
        let offline = opts.offline || warm_only;
        tasks.spawn(async move {
            let _permit = sem.acquire().await.map_err(|_| Error::Http {
                url: url.clone(),
                message: "semaphore closed".to_owned(),
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
                message: format!("extraction task interrupted: {e}"),
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
    // Packages land in disjoint directories → fan-out where parallel I/O
    // pays (Linux: sylius vendor/ wiped 3.75 s -> 1.68 s on ext4/WSL2);
    // sequential where it does not (APFS clonefile is metadata-bound and
    // contends: 607 ms -> 635 ms on an M4 Max). Each package stays atomic
    // (remove-before-clone); only the inter-package order changes, which
    // affects nothing but mtimes. `VIVACITY_PARALLEL_IO=0|1` overrides.
    let place = |p: &&LockPackage| -> Result<bool> {
        let (Some(pkg_root), Some(dest)) = (layout.package_root(p.name()), layout.abs(p.name()))
        else {
            return Ok(false);
        };
        // Always start again from an empty package root (target-dir included).
        if pkg_root.exists() {
            std::fs::remove_dir_all(&pkg_root).map_err(Error::io(&pkg_root))?;
        }
        let src = store.entry_path(p.name(), p.version(), p.dist_reference());
        crate::clone::clone_tree(&src, &dest)?;
        Ok(true)
    };
    let placed: Vec<bool> = if crate::platform::parallel_io() {
        use rayon::prelude::*;
        to_install.par_iter().map(place).collect::<Result<_>>()?
    } else {
        to_install.iter().map(place).collect::<Result<_>>()?
    };
    let installed: usize = placed.into_iter().filter(|placed| *placed).count();
    report.installed += installed;

    // Bin proxies: rebuilt for the packages actually (re)placed
    // (`BinaryInstaller::installBinaries` on install/update), and — like
    // `Installer::run`'s `ensureBinariesPresence` over every installed
    // package — written for an unchanged package only where the proxy is
    // MISSING (`installBinaries(..., warnOnOverwrite: false)` skips an
    // existing one): a wiped `vendor/bin` comes back on a no-op install,
    // and a no-op install otherwise touches nothing. The purge of orphaned
    // proxies (removed packages) runs on `wanted`. The `.bat` follows the
    // resolved bin-compat (`full`, or `auto` on Windows/WSL), like
    // Composer's BinaryInstaller — a plain Linux/macOS install writes no
    // `.bat`.
    let bin_compat = crate::binproxy::resolve_bin_compat(root_manifest)?;
    let placed: std::collections::HashSet<&str> = to_install.iter().map(|p| p.name()).collect();
    for p in &wanted {
        let bins = p.bins();
        if bins.is_empty() {
            continue;
        }
        let Some(dir) = layout.abs(p.name()) else {
            continue;
        };
        let missing = || {
            bins.iter().any(|b| {
                let b = b.trim_start_matches("./");
                let link_name = b.rsplit_once('/').map(|(_, f)| f).unwrap_or(b);
                dir.join(b).exists() && !vendor.join("bin").join(link_name).exists()
            })
        };
        if placed.contains(p.name()) || missing() {
            crate::binproxy::install_binaries(&vendor, &dir, &bins, bin_compat)?;
        }
    }
    prune_orphan_bin_proxies(&vendor, &wanted, bin_compat)?;

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

fn prune_orphan_bin_proxies(
    vendor: &Path,
    wanted: &[&LockPackage],
    bin_compat: crate::binproxy::BinCompat,
) -> Result<()> {
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
        // A `.bat` is the Windows proxy of an expected bin — kept only when
        // the resolved bin-compat writes `.bat` proxies at all (otherwise a
        // leftover from a previous full-mode install, purged, converging on
        // what Composer produces on a bare checkout) — or the proxy of a
        // removed package (purged), or a user-placed file.
        let keep = expected.contains(&file_name)
            || (bin_compat == crate::binproxy::BinCompat::Full
                && file_name
                    .strip_suffix(".bat")
                    .is_some_and(|stem| expected.contains(stem)));
        if !keep {
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
