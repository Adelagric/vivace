//! La transaction d'installation : diff (lock ↔ état installé), fetch parallèle
//! vers le store, clone store→vendor, proxies bin, fichiers d'état, stub
//! runtime. Idempotente (relancée après interruption, elle converge) : l'état
//! de référence est `installed.json` + la présence des répertoires, et chaque
//! paquet est posé par clone dans un vendor/<name> préalablement supprimé.

use crate::error::{Error, Result};
use crate::fetch::{Fetcher, Provenance};
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
    /// Parallélisme des téléchargements/extractions.
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
}

/// Identité installée d'un paquet : version + référence de dist.
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
    project_dir: &Path,
    lock: &Lock,
    root_manifest: &Value,
    store: Arc<Store>,
    fetcher: Arc<Fetcher>,
    opts: &InstallOptions,
) -> Result<InstallReport> {
    let vendor = project_dir.join("vendor");
    std::fs::create_dir_all(&vendor).map_err(Error::io(&vendor))?;

    let mut report = InstallReport::default();
    let wanted: Vec<&LockPackage> = lock.wanted_packages(opts.with_dev).collect();
    let wanted_names: std::collections::BTreeSet<&str> = wanted.iter().map(|p| p.name()).collect();
    let previous = installed_identities(&vendor);

    // Suppressions : présents avant, plus voulus.
    for name in previous.keys() {
        if !wanted_names.contains(name.as_str()) {
            let dir = vendor.join(name);
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(Error::io(&dir))?;
                prune_empty_parent(&vendor, name);
            }
            report.removed += 1;
        }
    }

    // À poser : identité changée, ou répertoire absent.
    let mut to_install: Vec<&LockPackage> = Vec::new();
    for p in &wanted {
        if p.is_metapackage() {
            continue;
        }
        let unchanged = previous.get(p.name()) == Some(&identity(p))
            && vendor.join(p.install_subpath()).is_dir();
        if unchanged {
            report.unchanged += 1;
        } else {
            to_install.push(p);
        }
    }

    // Fetch + extraction vers le store, en parallèle borné.
    let sem = Arc::new(tokio::sync::Semaphore::new(opts.jobs.max(1)));
    let mut tasks = tokio::task::JoinSet::new();
    for p in &to_install {
        if store.contains(p.name(), p.version(), p.dist_reference()) {
            report.store_hits += 1;
            continue;
        }
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
        let offline = opts.offline;
        tasks.spawn(async move {
            let _permit = sem.acquire().await.map_err(|_| Error::Http {
                url: url.clone(),
                message: "semaphore fermé".to_owned(),
            })?;
            let (bytes, provenance) = fetcher
                .dist_bytes(&name, &url, shasum.as_deref(), offline)
                .await?;
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
            Ok::<Provenance, Error>(provenance)
        });
    }
    while let Some(joined) = tasks.join_next().await {
        let provenance = joined.map_err(|e| Error::Http {
            url: "join".to_owned(),
            message: e.to_string(),
        })??;
        match provenance {
            Provenance::Cache => report.from_cache += 1,
            Provenance::Network => report.from_network += 1,
        }
    }

    // Pose : suppression de l'ancienne version puis clone depuis le store.
    for p in &to_install {
        // On repart toujours de vendor/<name> vide (target-dir compris).
        let pkg_root = vendor.join(p.name());
        if pkg_root.exists() {
            std::fs::remove_dir_all(&pkg_root).map_err(Error::io(&pkg_root))?;
        }
        let dest = vendor.join(p.install_subpath());
        let src = store.entry_path(p.name(), p.version(), p.dist_reference());
        crate::clone::clone_tree(&src, &dest)?;
        report.installed += 1;
    }

    // Proxies bin : reconstruits pour tous les paquets voulus, puis purge des
    // proxies orphelins (paquets retirés).
    for p in &wanted {
        let bins = p.bins();
        if !bins.is_empty() {
            crate::binproxy::install_binaries(&vendor, &p.install_subpath(), &bins)?;
        }
    }
    prune_orphan_bin_proxies(&vendor, &wanted)?;

    // Fichiers d'état + stub runtime.
    let root = RootPackage::detect(root_manifest, project_dir, opts.with_dev);
    crate::state::write_state_files(
        &vendor.join("composer"),
        lock,
        &root,
        root_manifest,
        opts.with_dev,
    )?;
    if wanted.iter().any(|p| p.name() == "symfony/runtime") {
        crate::runtime_stub::write_stub(&vendor)?;
    }

    Ok(report)
}

/// vendor/a/b supprimé → retire aussi vendor/a s'il est vide.
fn prune_empty_parent(vendor: &Path, name: &str) {
    if let Some((vendor_ns, _)) = name.split_once('/') {
        let parent = vendor.join(vendor_ns);
        if std::fs::read_dir(&parent)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&parent);
        }
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

/// Chemin utilitaire : vendor/composer du projet.
pub fn vendor_composer_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("vendor/composer")
}
