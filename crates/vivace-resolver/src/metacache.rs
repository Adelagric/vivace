//! Cache des métadonnées d'un dépôt `composer`, au format et à l'emplacement
//! de Composer (`Cache` sur `cache-repo-dir/<url assainie>/`) : mêmes noms
//! de fichiers (`packages.json`, `provider-<vendor>~<name>[~dev].json`),
//! même contenu (le JSON ré-encodé avec `last-modified` injecté quand le
//! serveur a donné l'en-tête, le corps brut sinon). Un cache écrit par l'un
//! est lu par l'autre, et vice versa.

use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct MetadataCache {
    root: PathBuf,
}

/// `Url::sanitize` réduit aux identifiants dans l'URL (`user:pass@`) et au
/// paramètre `access_token`.
fn sanitize_url(url: &str) -> String {
    static TOKEN: std::sync::OnceLock<pcre2::bytes::Regex> = std::sync::OnceLock::new();
    static CREDS: std::sync::OnceLock<pcre2::bytes::Regex> = std::sync::OnceLock::new();
    let token = crate::version::regex(&TOKEN, r"([&?]access_token=)[^&]+", false);
    let mut out = String::new();
    let mut last = 0;
    for m in token.find_iter(url.as_bytes()).flatten() {
        out.push_str(&url[last..m.start()]);
        let matched = &url[m.start()..m.end()];
        let eq = matched.find('=').map(|i| i + 1).unwrap_or(matched.len());
        out.push_str(&matched[..eq]);
        out.push_str("***");
        last = m.end();
    }
    out.push_str(&url[last..]);
    let url = out;
    let creds = crate::version::regex(
        &CREDS,
        r"(?:(?P<prefix>[a-z0-9][a-z0-9+.-]*://)|\A)(?P<user>[^:/\s?#]*)(?::(?P<password>[^\s/?#]+))?@",
        true,
    );
    match creds.captures(url.as_bytes()) {
        Ok(Some(caps)) => {
            let whole = caps.get(0).map(|m| (m.start(), m.end())).unwrap_or((0, 0));
            let prefix = caps.get(1).map(|m| &url[m.start()..m.end()]).unwrap_or("");
            let user = caps.get(2).map(|m| &url[m.start()..m.end()]).unwrap_or("");
            let user = sanitize_username(user);
            let replacement = if caps.get(3).is_some_and(|m| m.end() > m.start()) {
                format!("{prefix}{user}:***@")
            } else {
                format!("{prefix}{user}@")
            };
            format!("{}{}{}", &url[..whole.0], replacement, &url[whole.1..])
        }
        _ => url,
    }
}

/// `Url::sanitizeUsername`.
fn sanitize_username(user: &str) -> String {
    const NON_SECRET: &[&str] = &[
        "gitlab-ci-token",
        "x-token-auth",
        "oauth2",
        "x-oauth-basic",
        "git",
        "user",
        "token",
    ];
    static GH: std::sync::OnceLock<pcre2::bytes::Regex> = std::sync::OnceLock::new();
    if NON_SECRET.contains(&user) {
        return user.to_owned();
    }
    let gh = crate::version::regex(&GH, r"^(?:ghp|gho|ghu|ghs|ghr|github_pat)_", false);
    if gh.is_match(user.as_bytes()).unwrap_or(false) || user.len() >= 12 {
        return format!("{}***", &user[..user.len().min(3)]);
    }
    user.to_owned()
}

impl MetadataCache {
    /// `new Cache($io, $config->get('cache-repo-dir').'/'.preg_replace('{[^a-z0-9.]}i', '-', Url::sanitize($url)))`.
    pub fn new(cache_repo_dir: &Path, repo_url: &str) -> MetadataCache {
        let slug: String = sanitize_url(repo_url)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        MetadataCache {
            root: cache_repo_dir.join(slug),
        }
    }

    /// Clé assainie (`Cache` : `[^a-z0-9.$~_]` → `-`).
    fn path(&self, key: &str) -> PathBuf {
        let file: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '$' | '~' | '_') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        self.root.join(file)
    }

    /// `provider-<name avec / → ~>.json`.
    pub fn provider_key(file_name: &str) -> String {
        format!("provider-{}.json", file_name.replace('/', "~"))
    }

    pub fn read(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    /// `Cache::getAge` : âge du fichier en secondes.
    pub fn age(&self, key: &str) -> Option<u64> {
        let modified = std::fs::metadata(self.path(key)).ok()?.modified().ok()?;
        std::time::SystemTime::now()
            .duration_since(modified)
            .ok()
            .map(|d| d.as_secs())
    }

    /// Écriture atomique (temp + rename), comme `Cache::write` ; un échec est
    /// silencieux (Composer continue sans cache).
    pub fn write(&self, key: &str, contents: &[u8]) {
        if std::fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let path = self.path(key);
        let tmp = path.with_extension(format!("json{:010x}.tmp", std::process::id()));
        if std::fs::write(&tmp, contents).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Le contenu à écrire quand le serveur a fourni `Last-Modified` :
    /// `$data['last-modified'] = …` puis `JsonFile::encode` compact — avec
    /// `JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE` pour les fichiers
    /// de paquets (`asyncFetchFile`), avec les flags 0 (slashes et unicode
    /// échappés) pour `packages.json` et les `includes` (`fetchFile`).
    pub fn with_last_modified(data: &Value, last_modified: &str, escaped: bool) -> Option<Vec<u8>> {
        let mut data = data.clone();
        let obj = data.as_object_mut()?;
        obj.insert(
            "last-modified".into(),
            Value::String(last_modified.to_owned()),
        );
        let opts = vivace_core::phpjson::EncodeOptions {
            pretty: false,
            escape_slashes: escaped,
            escape_unicode: escaped,
        };
        vivace_core::phpjson::php_json_encode_with(&data, opts)
            .ok()
            .map(String::into_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_composer() {
        let c = MetadataCache::new(Path::new("/c/repo"), "https://repo.packagist.org");
        assert_eq!(c.root, Path::new("/c/repo/https---repo.packagist.org"));
        assert_eq!(
            c.path(&MetadataCache::provider_key("acme/lib~dev")),
            Path::new("/c/repo/https---repo.packagist.org/provider-acme~lib~dev.json")
        );
        let c = MetadataCache::new(
            Path::new("/c/repo"),
            "https://user:secret@satis.example.org/",
        );
        assert_eq!(
            c.root,
            Path::new("/c/repo/https---user-----satis.example.org-")
        );
        assert_eq!(
            sanitize_url("https://ghp_abcdefghijklmnop@github.com/x"),
            "https://ghp***@github.com/x"
        );
        assert_eq!(
            sanitize_url("https://x.org/p?access_token=abc&y=1"),
            "https://x.org/p?access_token=***&y=1"
        );
        let data = serde_json::json!({"packages": {"a/b": []}, "minified": "composer/2.0"});
        let bytes =
            MetadataCache::with_last_modified(&data, "Sat, 12 Sep 2026 10:00:00 GMT", false)
                .unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"packages":{"a/b":[]},"minified":"composer/2.0","last-modified":"Sat, 12 Sep 2026 10:00:00 GMT"}"#
        );
        let bytes = MetadataCache::with_last_modified(&data, "x", true).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"packages":{"a\/b":[]},"minified":"composer\/2.0","last-modified":"x"}"#
        );
    }
}
