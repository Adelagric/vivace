//! Lecture de composer.lock. Vue typée minimale par-dessus le JSON brut :
//! `installed.json`/`installed.php` devront resservir les entrées **à
//! l'identique**, donc chaque paquet garde sa valeur brute (`raw`) et n'expose
//! en champs typés que ce dont l'installeur a besoin.

use crate::error::{Error, Result};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug)]
pub struct Lock {
    pub content_hash: Option<String>,
    pub packages: Vec<LockPackage>,
    pub packages_dev: Vec<LockPackage>,
    /// Contraintes plateforme du projet (php, ext-*, lib-*) → constraint.
    pub platform: Vec<(String, String)>,
    pub platform_dev: Vec<(String, String)>,
    pub plugin_api_version: Option<String>,
    pub aliases: Vec<Value>,
}

#[derive(Debug)]
pub struct LockPackage {
    pub raw: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistKind {
    Zip,
    Other,
    Missing,
}

impl LockPackage {
    fn str_field(&self, key: &str) -> Option<&str> {
        self.raw.get(key).and_then(Value::as_str)
    }

    pub fn name(&self) -> &str {
        self.str_field("name").unwrap_or("")
    }

    pub fn version(&self) -> &str {
        self.str_field("version").unwrap_or("")
    }

    /// Type de paquet, défaut Composer : "library".
    pub fn package_type(&self) -> &str {
        self.str_field("type").unwrap_or("library")
    }

    pub fn dist_url(&self) -> Option<&str> {
        self.raw.get("dist")?.get("url")?.as_str()
    }

    pub fn dist_reference(&self) -> Option<&str> {
        self.raw.get("dist")?.get("reference")?.as_str()
    }

    /// shasum sha1 de la dist si non vide (souvent vide sur Packagist).
    pub fn dist_shasum(&self) -> Option<&str> {
        self.raw
            .get("dist")?
            .get("shasum")?
            .as_str()
            .filter(|s| !s.is_empty())
    }

    pub fn dist_kind(&self) -> DistKind {
        match self
            .raw
            .get("dist")
            .and_then(|d| d.get("type"))
            .and_then(Value::as_str)
        {
            Some("zip") => DistKind::Zip,
            Some(_) => DistKind::Other,
            None => DistKind::Missing,
        }
    }

    pub fn is_metapackage(&self) -> bool {
        self.package_type() == "metapackage"
    }

    pub fn bins(&self) -> Vec<&str> {
        self.raw
            .get("bin")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }
}

fn parse_packages(v: Option<&Value>) -> Vec<LockPackage> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_object())
                .map(|m| LockPackage { raw: m.clone() })
                .collect()
        })
        .unwrap_or_default()
}

/// `platform` est `{}` ou `{"php": ">=8.2", "ext-mbstring": "*"}` — et, quirk
/// d'encodage PHP, parfois `[]` (array vide) quand il n'y a rien.
fn parse_platform(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

impl Lock {
    pub fn parse(text: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(text).map_err(|source| Error::Json {
            context: "composer.lock".to_owned(),
            source,
        })?;
        Ok(Lock {
            content_hash: v
                .get("content-hash")
                .and_then(Value::as_str)
                .map(str::to_owned),
            packages: parse_packages(v.get("packages")),
            packages_dev: parse_packages(v.get("packages-dev")),
            platform: parse_platform(v.get("platform")),
            platform_dev: parse_platform(v.get("platform-dev")),
            plugin_api_version: v
                .get("plugin-api-version")
                .and_then(Value::as_str)
                .map(str::to_owned),
            aliases: v
                .get("aliases")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        })
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Paquets à installer selon --no-dev.
    pub fn wanted_packages(&self, with_dev: bool) -> impl Iterator<Item = &LockPackage> {
        self.packages
            .iter()
            .chain(
                self.packages_dev
                    .iter()
                    .take(if with_dev { usize::MAX } else { 0 }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_lock() {
        let lock = Lock::parse(
            r#"{"content-hash":"abc","packages":[{"name":"a/b","version":"1.0.0",
                "dist":{"type":"zip","url":"https://x/y.zip","reference":"deadbeef","shasum":""},
                "type":"library","bin":["bin/tool"]}],
               "packages-dev":[],"platform":{"php":">=8.1"},"platform-dev":[]}"#,
        )
        .expect("parse");
        assert_eq!(lock.content_hash.as_deref(), Some("abc"));
        let p = &lock.packages[0];
        assert_eq!(p.name(), "a/b");
        assert_eq!(p.dist_kind(), DistKind::Zip);
        assert_eq!(p.dist_shasum(), None); // vide → None
        assert_eq!(p.bins(), vec!["bin/tool"]);
        assert_eq!(lock.platform, vec![("php".to_owned(), ">=8.1".to_owned())]);
        assert_eq!(lock.wanted_packages(false).count(), 1);
    }

    #[test]
    fn empty_platform_as_array() {
        let lock = Lock::parse(r#"{"packages":[],"platform":[]}"#).expect("parse");
        assert!(lock.platform.is_empty());
    }
}
