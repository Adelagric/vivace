//! Spike M0 — chiffrer le plafond de gain sur fetch + extraction.
//! Jetable : pas de bin proxies, pas d'installed.*, pas d'autoload, pas d'auth.
//! Usage : spike <fixture-dir> [--offline]   (extrait dans <fixture-dir>/vendor-spike)

use sha1::{Digest, Sha1};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone)]
struct Dist {
    name: String,
    url: String,
}

fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("COMPOSER_CACHE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").expect("HOME");
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Caches/composer")
    } else {
        PathBuf::from(home).join(".cache/composer")
    }
}

fn cache_path(cache: &Path, d: &Dist) -> PathBuf {
    let mut h = Sha1::new();
    h.update(d.url.as_bytes());
    let sha = hex::encode(h.finalize());
    cache.join("files").join(&d.name).join(format!("{sha}.zip"))
}

/// Extraction avec strip du dossier racine unique des dists GitHub.
fn extract(zip_bytes: &[u8], dest: &Path) -> anyhow::Result<()> {
    let mut ar = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    std::fs::create_dir_all(dest)?;
    for i in 0..ar.len() {
        let mut f = ar.by_index(i)?;
        let Some(raw) = f.enclosed_name() else {
            continue;
        }; // zip-slip: rejeté par enclosed_name
        let stripped: PathBuf = raw.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(stripped);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf)?;
            std::fs::write(&out, buf)?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let fixture = PathBuf::from(args.next().expect("usage: spike <fixture-dir> [--offline]"));
    let offline = args.next().as_deref() == Some("--offline");

    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("composer.lock"))?)?;
    let mut dists = vec![];
    for key in ["packages", "packages-dev"] {
        for p in lock[key].as_array().into_iter().flatten() {
            let name = p["name"].as_str().unwrap().to_string();
            match p["dist"]["url"].as_str() {
                Some(url) => dists.push(Dist {
                    name,
                    url: url.to_string(),
                }),
                None => eprintln!("skip (pas de dist): {name}"),
            }
        }
    }
    let total = dists.len();
    let cache = cache_dir();
    let vendor = fixture.join("vendor-spike");
    let _ = std::fs::remove_dir_all(&vendor);

    let t0 = Instant::now();
    let client = reqwest::Client::builder()
        .user_agent("vivace-spike/0.1")
        .build()?;
    let mut set = tokio::task::JoinSet::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
    for d in dists {
        let (client, cache, vendor, sem) =
            (client.clone(), cache.clone(), vendor.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let cp = cache_path(&cache, &d);
            let bytes = if cp.exists() {
                std::fs::read(&cp)?
            } else if offline {
                anyhow::bail!("cache miss en mode offline: {}", d.name)
            } else {
                let b = client
                    .get(&d.url)
                    .send()
                    .await?
                    .error_for_status()?
                    .bytes()
                    .await?
                    .to_vec();
                std::fs::create_dir_all(cp.parent().unwrap())?;
                let tmp = cp.with_extension("tmp-spike");
                std::fs::write(&tmp, &b)?;
                std::fs::rename(&tmp, &cp)?;
                b
            };
            let dest = vendor.join(&d.name);
            tokio::task::spawn_blocking(move || extract(&bytes, &dest)).await??;
            anyhow::Ok(())
        });
    }
    let mut errs = 0;
    while let Some(r) = set.join_next().await {
        if let Err(e) = r? {
            eprintln!("ERR {e:#}");
            errs += 1;
        }
    }
    println!(
        "spike: {} paquets extraits ({} erreurs) en {:.3}s",
        total - errs,
        errs,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}
