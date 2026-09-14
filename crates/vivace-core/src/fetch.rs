//! Dist downloads, interoperable with Composer's cache: same layout
//! (`<cache>/files/<vendor>/<pkg>/<sha1-of-url>.zip`), both read AND fed, so
//! a cache warmed by one serves the other. Minimal v1 auth: `github-oauth`,
//! `http-basic`, `bearer` (project auth.json, COMPOSER_AUTH, then the
//! COMPOSER_HOME auth.json). The lock's shasum, when present, is checked on
//! download AND when reading back from the cache (meta-analysis F7: a shared
//! cache is read back with suspicion).

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
    /// Merges (from lowest to highest priority): the COMPOSER_HOME auth.json,
    /// the COMPOSER_AUTH variable, the project auth.json.
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

    /// Value of the Authorization header for this host, if any.
    pub fn authorization_for(&self, host: &str) -> Option<String> {
        let host = host.to_ascii_lowercase();
        // GitHub dists go through api.github.com / codeload.github.com
        // but the token is stored under github.com.
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

/// `Factory::useXdg`: true as soon as any `XDG_*` environment variable exists.
fn use_xdg() -> bool {
    std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("XDG_"))
}

fn user_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h.trim_end_matches('/')))
}

/// `Factory::getHomeDir` (docs/reference/Factory.php): COMPOSER_HOME, else
/// the first existing directory among `$XDG_CONFIG_HOME/composer` (if XDG is
/// in use) and `~/.composer`, else the first candidate.
pub fn composer_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("COMPOSER_HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    let user = user_dir()?;
    let mut dirs: Vec<PathBuf> = Vec::new();
    if use_xdg() {
        let xdg = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| user.join(".config"));
        dirs.push(xdg.join("composer"));
    }
    dirs.push(user.join(".composer"));
    dirs.iter()
        .find(|d| d.is_dir())
        .cloned()
        .or_else(|| dirs.first().cloned())
}

/// `Factory::getCacheDir`: COMPOSER_CACHE_DIR; else `$COMPOSER_HOME/cache`
/// if COMPOSER_HOME is set; Darwin -> `~/Library/Caches/composer`;
/// `~/.composer/cache` if it exists; XDG -> `$XDG_CACHE_HOME/composer`;
/// else `<home>/cache`.
pub fn composer_cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("COMPOSER_CACHE_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(h) = std::env::var("COMPOSER_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join("cache");
        }
    }
    let user = user_dir().unwrap_or_else(|| PathBuf::from("."));
    let home = composer_home().unwrap_or_else(|| user.join(".composer"));
    if cfg!(target_os = "macos") {
        return user.join("Library/Caches/composer");
    }
    if home == user.join(".composer") && home.join("cache").is_dir() {
        return home.join("cache");
    }
    if use_xdg() {
        let xdg = std::env::var("XDG_CACHE_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| user.join(".cache"));
        return xdg.join("composer");
    }
    home.join("cache")
}

/// Cache path of a dist, identical to Composer's: sha1 of the FULL URL
/// (deliberate in Composer: prevents cross-repository poisoning), key
/// sanitised to `[a-z0-9._/-]`.
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

/// Response of a conditional metadata GET.
#[derive(Debug, Clone)]
pub enum MetadataResponse {
    NotModified,
    NotFound,
    Body {
        bytes: Vec<u8>,
        last_modified: Option<String>,
    },
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

    /// Bytes of the dist: cache first (shasum re-checked), network otherwise
    /// (3 attempts, backoff), cache fed through temp+rename.
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
                    // Corrupted/poisoned cache entry: throw it away.
                    let _ = std::fs::remove_file(&cache_path);
                }
                _ => return Ok((bytes, Provenance::Cache)),
            }
        }
        if offline {
            return Err(Error::Http {
                url: url.to_owned(),
                message: format!("missing cache for {name} in offline mode"),
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
            message: format!("failed after 3 attempts: {last_err}"),
        })
    }

    /// Metadata GET (packages.json, p2 files): `Ok(None)` on 404 (unknown
    /// package, tolerated by Composer), error otherwise; 3 attempts on
    /// transport errors.
    pub async fn metadata_bytes(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.metadata_fetch(url, None).await? {
            MetadataResponse::Body { bytes, .. } => Ok(Some(bytes)),
            MetadataResponse::NotFound | MetadataResponse::NotModified => Ok(None),
        }
    }

    /// `application/x-www-form-urlencoded` POST (the security advisories
    /// API: `packages[]=...`), 10 s timeout like Composer, a single attempt;
    /// 404 -> `NotFound`.
    pub async fn post_form(&self, url: &str, body: &str) -> Result<MetadataResponse> {
        let mut req = self
            .client
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .timeout(std::time::Duration::from_secs(10))
            .body(body.to_owned());
        if let Ok(parsed) = reqwest::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                if let Some(authz) = self.auth.authorization_for(host) {
                    req = req.header(reqwest::header::AUTHORIZATION, authz);
                }
            }
        }
        let resp = req.send().await.map_err(|e| Error::Http {
            url: url.to_owned(),
            message: e.to_string(),
        })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(MetadataResponse::NotFound);
        }
        let resp = resp.error_for_status().map_err(|e| Error::Http {
            url: url.to_owned(),
            message: e.to_string(),
        })?;
        let bytes = resp.bytes().await.map_err(|e| Error::Http {
            url: url.to_owned(),
            message: e.to_string(),
        })?;
        Ok(MetadataResponse::Body {
            bytes: bytes.to_vec(),
            last_modified: None,
        })
    }

    /// Conditional metadata GET: `If-Modified-Since` when the cache has a
    /// date, 304 -> `NotModified`, 404 -> `NotFound`, else the body and the
    /// `Last-Modified` header; 3 attempts on transport errors.
    pub async fn metadata_fetch(
        &self,
        url: &str,
        if_modified_since: Option<&str>,
    ) -> Result<MetadataResponse> {
        let mut last_err = String::new();
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(250 * (1 << attempt))).await;
            }
            let mut req = self.client.get(url);
            if let Ok(parsed) = reqwest::Url::parse(url) {
                if let Some(host) = parsed.host_str() {
                    if let Some(authz) = self.auth.authorization_for(host) {
                        req = req.header(reqwest::header::AUTHORIZATION, authz);
                    }
                }
            }
            if let Some(ims) = if_modified_since {
                req = req.header(reqwest::header::IF_MODIFIED_SINCE, ims);
            }
            match req.send().await {
                Ok(resp) => {
                    if resp.status() == reqwest::StatusCode::NOT_FOUND {
                        return Ok(MetadataResponse::NotFound);
                    }
                    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
                        return Ok(MetadataResponse::NotModified);
                    }
                    let last_modified = resp
                        .headers()
                        .get(reqwest::header::LAST_MODIFIED)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    match resp.error_for_status() {
                        Ok(resp) => match resp.bytes().await {
                            Ok(b) => {
                                return Ok(MetadataResponse::Body {
                                    bytes: b.to_vec(),
                                    last_modified,
                                })
                            }
                            Err(e) => last_err = e.to_string(),
                        },
                        Err(e) => {
                            // 4xx other than 404 are not retried.
                            if e.status().is_some_and(|s| s.is_client_error()) {
                                return Err(Error::Http {
                                    url: url.to_owned(),
                                    message: e.to_string(),
                                });
                            }
                            last_err = e.to_string();
                        }
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        Err(Error::Http {
            url: url.to_owned(),
            message: format!("failed after 3 attempts: {last_err}"),
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
        // Checked against Composer's real cache in M0: the key is
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
            "github dists go through codeload"
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
