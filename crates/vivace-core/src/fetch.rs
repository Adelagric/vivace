//! Téléchargement des dists, interopérable avec le cache de Composer :
//! même layout (`<cache>/files/<vendor>/<pkg>/<sha1-de-l-url>.zip`), lu ET
//! alimenté, donc un cache chauffé par l'un sert à l'autre. Auth minimale
//! v1 : `github-oauth`, `http-basic`, `bearer` (auth.json du projet,
//! COMPOSER_AUTH, puis auth.json de COMPOSER_HOME). Le shasum du lock, quand
//! il existe, est vérifié au téléchargement ET à la relecture du cache
//! (méta-analyse F7 : un cache partagé se relit avec méfiance).

use crate::error::{Error, Result};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct Auth {
    pub github_oauth: BTreeMap<String, String>,
    pub http_basic: BTreeMap<String, (String, String)>,
    pub bearer: BTreeMap<String, String>,
}

impl Auth {
    /// Fusionne (du moins prioritaire au plus prioritaire) : auth.json de
    /// COMPOSER_HOME, variable COMPOSER_AUTH, auth.json du projet.
    pub fn load(project_dir: &Path) -> Auth {
        let mut auth = Auth::default();
        if let Some(home) = composer_home() {
            auth.merge_json_file(&home.join("auth.json"));
        }
        if let Ok(env) = std::env::var("COMPOSER_AUTH") {
            if let Ok(v) = serde_json::from_str::<Value>(&env) {
                auth.merge_value(&v);
            }
        }
        auth.merge_json_file(&project_dir.join("auth.json"));
        auth
    }

    fn merge_json_file(&mut self, path: &Path) {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                self.merge_value(&v);
            }
        }
    }

    fn merge_value(&mut self, v: &Value) {
        if let Some(map) = v.get("github-oauth").and_then(Value::as_object) {
            for (host, tok) in map {
                if let Some(t) = tok.as_str() {
                    self.github_oauth
                        .insert(host.to_ascii_lowercase(), t.to_owned());
                }
            }
        }
        if let Some(map) = v.get("bearer").and_then(Value::as_object) {
            for (host, tok) in map {
                if let Some(t) = tok.as_str() {
                    self.bearer.insert(host.to_ascii_lowercase(), t.to_owned());
                }
            }
        }
        if let Some(map) = v.get("http-basic").and_then(Value::as_object) {
            for (host, creds) in map {
                if let (Some(u), Some(p)) = (
                    creds.get("username").and_then(Value::as_str),
                    creds.get("password").and_then(Value::as_str),
                ) {
                    self.http_basic
                        .insert(host.to_ascii_lowercase(), (u.to_owned(), p.to_owned()));
                }
            }
        }
    }

    /// Valeur de l'en-tête Authorization pour cet hôte, le cas échéant.
    pub fn authorization_for(&self, host: &str) -> Option<String> {
        let host = host.to_ascii_lowercase();
        // Les dists GitHub passent par api.github.com / codeload.github.com
        // mais le token est rangé sous github.com.
        if host == "github.com" || host.ends_with(".github.com") {
            if let Some(t) = self.github_oauth.get("github.com") {
                return Some(format!("token {t}"));
            }
        }
        if let Some(t) = self.github_oauth.get(&host) {
            return Some(format!("token {t}"));
        }
        if let Some(t) = self.bearer.get(&host) {
            return Some(format!("Bearer {t}"));
        }
        if let Some((u, p)) = self.http_basic.get(&host) {
            use base64::Engine as _;
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
            return Some(format!("Basic {encoded}"));
        }
        None
    }
}

fn composer_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("COMPOSER_HOME") {
        return Some(PathBuf::from(h));
    }
    let home = std::env::var("HOME").ok()?;
    if cfg!(target_os = "macos") {
        Some(PathBuf::from(home).join(".composer"))
    } else if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(xdg).join("composer"))
    } else {
        Some(PathBuf::from(home).join(".config/composer"))
    }
}

pub fn composer_cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("COMPOSER_CACHE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Caches/composer")
    } else if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg).join("composer")
    } else {
        PathBuf::from(home).join(".cache/composer")
    }
}

/// Chemin de cache d'une dist, identique à Composer : sha1 de l'URL COMPLÈTE
/// (délibéré chez Composer : évite l'empoisonnement inter-dépôts), clé
/// assainie sur `[a-z0-9._/-]`.
pub fn dist_cache_path(cache_root: &Path, name: &str, url: &str) -> PathBuf {
    let mut h = Sha1::new();
    h.update(url.as_bytes());
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let sane_name: String = name
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    cache_root
        .join("files")
        .join(sane_name)
        .join(format!("{sha}.zip"))
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Fetcher {
    client: reqwest::Client,
    cache_root: PathBuf,
    auth: Auth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Cache,
    Network,
}

impl Fetcher {
    pub fn new(cache_root: PathBuf, auth: Auth) -> Result<Fetcher> {
        let client = reqwest::Client::builder()
            .user_agent(format!("vivace/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Http {
                url: "client".to_owned(),
                message: e.to_string(),
            })?;
        Ok(Fetcher {
            client,
            cache_root,
            auth,
        })
    }

    /// Octets de la dist : cache d'abord (shasum revérifié), réseau sinon
    /// (3 tentatives, backoff), cache alimenté en temp+rename.
    pub async fn dist_bytes(
        &self,
        name: &str,
        url: &str,
        expected_sha1: Option<&str>,
        offline: bool,
    ) -> Result<(Vec<u8>, Provenance)> {
        let cache_path = dist_cache_path(&self.cache_root, name, url);
        if let Ok(bytes) = std::fs::read(&cache_path) {
            match expected_sha1 {
                Some(exp) if sha1_hex(&bytes) != exp => {
                    // Entrée de cache corrompue/empoisonnée : on la jette.
                    let _ = std::fs::remove_file(&cache_path);
                }
                _ => return Ok((bytes, Provenance::Cache)),
            }
        }
        if offline {
            return Err(Error::Http {
                url: url.to_owned(),
                message: format!("cache manquant pour {name} en mode offline"),
            });
        }

        let mut last_err = String::new();
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
            }
            match self.try_download(url).await {
                Ok(bytes) => {
                    if let Some(exp) = expected_sha1 {
                        let actual = sha1_hex(&bytes);
                        if actual != exp {
                            return Err(Error::ShasumMismatch {
                                name: name.to_owned(),
                                expected: exp.to_owned(),
                                actual,
                            });
                        }
                    }
                    if let Some(parent) = cache_path.parent() {
                        if std::fs::create_dir_all(parent).is_ok() {
                            let tmp = cache_path.with_extension("zip.vivace-tmp");
                            if std::fs::write(&tmp, &bytes).is_ok() {
                                let _ = std::fs::rename(&tmp, &cache_path);
                            }
                        }
                    }
                    return Ok((bytes, Provenance::Network));
                }
                Err(e) => last_err = e,
            }
        }
        Err(Error::Http {
            url: url.to_owned(),
            message: format!("échec après 3 tentatives: {last_err}"),
        })
    }

    async fn try_download(&self, url: &str) -> std::result::Result<Vec<u8>, String> {
        let mut req = self.client.get(url);
        if let Ok(parsed) = reqwest::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                if let Some(authz) = self.auth.authorization_for(host) {
                    req = req.header(reqwest::header::AUTHORIZATION, authz);
                }
            }
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let resp = resp.error_for_status().map_err(|e| e.to_string())?;
        Ok(resp.bytes().await.map_err(|e| e.to_string())?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_layout_matches_composer() {
        // Vérifié contre le cache réel de Composer en M0 : la clé est
        // files/<name>/<sha1(url)>.zip.
        let p = dist_cache_path(
            Path::new("/c"),
            "monolog/monolog",
            "https://api.github.com/repos/Seldaek/monolog/zipball/abc",
        );
        let s = p.to_string_lossy();
        assert!(s.starts_with("/c/files/monolog/monolog/"));
        assert!(s.ends_with(".zip"));
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn auth_header_selection() {
        let mut auth = Auth::default();
        auth.github_oauth
            .insert("github.com".into(), "ghtok".into());
        auth.bearer
            .insert("repo.example.com".into(), "beartok".into());
        auth.http_basic
            .insert("basic.example.com".into(), ("user".into(), "pass".into()));

        assert_eq!(
            auth.authorization_for("github.com").as_deref(),
            Some("token ghtok")
        );
        assert_eq!(
            auth.authorization_for("codeload.github.com").as_deref(),
            Some("token ghtok"),
            "les dists github passent par codeload"
        );
        assert_eq!(
            auth.authorization_for("api.github.com").as_deref(),
            Some("token ghtok")
        );
        assert_eq!(
            auth.authorization_for("repo.example.com").as_deref(),
            Some("Bearer beartok")
        );
        assert_eq!(
            auth.authorization_for("basic.example.com").as_deref(),
            Some("Basic dXNlcjpwYXNz")
        );
        assert_eq!(auth.authorization_for("unknown.example.com"), None);
    }

    #[test]
    fn composer_auth_env_shape_is_parsed() {
        let mut auth = Auth::default();
        auth.merge_value(&serde_json::json!({
            "github-oauth": {"github.com": "t1"},
            "http-basic": {"h": {"username": "u", "password": "p"}},
            "bearer": {"b": "tk"}
        }));
        assert_eq!(auth.github_oauth.len(), 1);
        assert_eq!(auth.http_basic.len(), 1);
        assert_eq!(auth.bearer.len(), 1);
    }
}
