//! Détection de la plateforme locale (PHP, extensions) et vérification que le
//! lock est installable dessus — l'équivalent pratique de l'étape « lock
//! installability » de Composer, sans solveur : contraintes `platform`/
//! `platform-dev` du lock + `require` php/ext-* de chaque paquet verrouillé.
//!
//! La détection shell-out UNE fois vers `php -r` et met le résultat en cache
//! (clé : chemin canonique + mtime + taille du binaire php) — indispensable au
//! budget no-op < 50 ms, le démarrage de PHP coûtant ~30-60 ms à lui seul.

use crate::constraint;
use crate::error::{Error, Result};
use crate::lock::Lock;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Platform {
    pub php_version: String,
    pub is_64bit: bool,
    /// nom d'extension (minuscule) → version (phpversion(ext), sinon version PHP).
    pub extensions: BTreeMap<String, String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CachedPlatform {
    php_path: String,
    mtime_unix: i64,
    size: u64,
    platform: Platform,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlatformFailure {
    /// "php", "ext-mbstring", …
    pub requirement: String,
    pub constraint: String,
    /// Paquet demandeur (None = section platform du lock).
    pub required_by: Option<String>,
    pub reason: FailureReason,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FailureReason {
    Missing,
    Mismatch { installed: String },
    Unsupported,
}

pub fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("VIVACE_CACHE_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("vivace");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Caches/vivace")
    } else {
        PathBuf::from(home).join(".cache/vivace")
    }
}

const DETECT_SNIPPET: &str = r#"
$exts = [];
foreach (get_loaded_extensions() as $e) {
    $exts[strtolower($e)] = phpversion($e) ?: PHP_VERSION;
}
echo json_encode([
    "php_version" => PHP_VERSION,
    "is_64bit" => PHP_INT_SIZE === 8,
    "extensions" => $exts,
]);
"#;

impl Platform {
    /// Détecte via `php` (surchargable par $VIVACE_PHP), avec cache disque.
    pub fn detect() -> Result<Option<Platform>> {
        let php = std::env::var("VIVACE_PHP").unwrap_or_else(|_| "php".to_owned());
        let Some((path, mtime_unix, size)) = binary_identity(&php) else {
            return Ok(None); // pas de php : l'appelant décide (reqs présentes → erreur)
        };

        let cache_file = cache_dir().join("platform.json");
        if let Ok(bytes) = std::fs::read(&cache_file) {
            if let Ok(cached) = serde_json::from_slice::<CachedPlatform>(&bytes) {
                if cached.php_path == path && cached.mtime_unix == mtime_unix && cached.size == size
                {
                    return Ok(Some(cached.platform));
                }
            }
        }

        let out = Command::new(&php)
            .args(["-d", "error_reporting=0", "-r", DETECT_SNIPPET])
            .output()
            .map_err(|source| Error::ReadFile {
                path: PathBuf::from(&php),
                source,
            })?;
        if !out.status.success() {
            return Ok(None);
        }
        let platform: Platform =
            serde_json::from_slice(&out.stdout).map_err(|source| Error::Json {
                context: "détection plateforme php".to_owned(),
                source,
            })?;

        let cached = CachedPlatform {
            php_path: path,
            mtime_unix,
            size,
            platform: platform.clone(),
        };
        if std::fs::create_dir_all(cache_dir()).is_ok() {
            if let Ok(json) = serde_json::to_vec(&cached) {
                let tmp = cache_file.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &cache_file);
                }
            }
        }
        Ok(Some(platform))
    }

    /// Applique `config.platform` du composer.json racine (surcharge des
    /// versions détectées ; `false` désactive une entrée).
    pub fn apply_overrides(&mut self, root_manifest: &Value) {
        let Some(overrides) = root_manifest
            .get("config")
            .and_then(|c| c.get("platform"))
            .and_then(Value::as_object)
        else {
            return;
        };
        for (name, v) in overrides {
            let name = name.to_ascii_lowercase();
            match (name.as_str(), v) {
                ("php", Value::String(s)) => self.php_version = s.clone(),
                (n, Value::String(s)) => {
                    if let Some(ext) = n.strip_prefix("ext-") {
                        self.extensions.insert(ext.to_owned(), s.clone());
                    }
                }
                (n, Value::Bool(false)) => {
                    if let Some(ext) = n.strip_prefix("ext-") {
                        self.extensions.remove(ext);
                    }
                }
                _ => {}
            }
        }
    }

    fn version_of(&self, requirement: &str) -> Option<&str> {
        match requirement {
            "php" => Some(&self.php_version),
            "php-64bit" => self.is_64bit.then_some(self.php_version.as_str()),
            r => r
                .strip_prefix("ext-")
                .and_then(|e| self.extensions.get(&e.to_ascii_lowercase()))
                .map(String::as_str),
        }
    }
}

fn binary_identity(php: &str) -> Option<(String, i64, u64)> {
    let path = if php.contains('/') {
        PathBuf::from(php)
    } else {
        let out = Command::new("which").arg(php).output().ok()?;
        if !out.status.success() {
            return None;
        }
        PathBuf::from(String::from_utf8(out.stdout).ok()?.trim())
    };
    let canonical = std::fs::canonicalize(&path).ok()?;
    let meta = std::fs::metadata(&canonical).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((canonical.to_string_lossy().into_owned(), mtime, meta.len()))
}

/// Vérifie le lock contre la plateforme. `ignored` : noms à ignorer, `*` final
/// accepté (`ext-*`), ou la liste spéciale `["*"]` pour tout ignorer.
pub fn check(
    lock: &Lock,
    platform: &Platform,
    with_dev: bool,
    ignored: &[String],
) -> Vec<PlatformFailure> {
    let mut failures = Vec::new();

    let mut reqs: Vec<(String, String, Option<String>)> = Vec::new();
    for (name, cons) in &lock.platform {
        reqs.push((name.clone(), cons.clone(), None));
    }
    if with_dev {
        for (name, cons) in &lock.platform_dev {
            reqs.push((name.clone(), cons.clone(), None));
        }
    }
    for p in lock.wanted_packages(with_dev) {
        if let Some(require) = p.raw.get("require").and_then(Value::as_object) {
            for (name, cons) in require {
                let lname = name.to_ascii_lowercase();
                // Un platform package n'a jamais de `/` (sinon c'est un vendor
                // comme php-http/*). composer-plugin-api / composer-runtime-api
                // sont exclus : satisfaits par construction côté vivace.
                let is_platform = !lname.contains('/')
                    && (lname == "php"
                        || lname.starts_with("php-")
                        || lname.starts_with("ext-")
                        || lname.starts_with("lib-"));
                if is_platform {
                    if let Some(c) = cons.as_str() {
                        reqs.push((lname, c.to_owned(), Some(p.name().to_owned())));
                    }
                }
            }
        }
    }

    for (requirement, cons, required_by) in reqs {
        if is_ignored(&requirement, ignored) {
            continue;
        }
        // lib-* : non détecté en v1 → seule la présence dans la section
        // platform du lock nous concerne, et on la signale comme Unsupported.
        if requirement.starts_with("lib-") {
            failures.push(PlatformFailure {
                requirement,
                constraint: cons,
                required_by,
                reason: FailureReason::Unsupported,
            });
            continue;
        }
        let Some(installed) = platform.version_of(&requirement) else {
            failures.push(PlatformFailure {
                requirement,
                constraint: cons,
                required_by,
                reason: FailureReason::Missing,
            });
            continue;
        };
        match constraint::satisfies(installed, &cons) {
            Ok(true) => {}
            Ok(false) => failures.push(PlatformFailure {
                requirement,
                constraint: cons,
                required_by,
                reason: FailureReason::Mismatch {
                    installed: installed.to_owned(),
                },
            }),
            Err(_) => failures.push(PlatformFailure {
                requirement,
                constraint: cons,
                required_by,
                reason: FailureReason::Unsupported,
            }),
        }
    }
    failures
}

fn is_ignored(requirement: &str, ignored: &[String]) -> bool {
    ignored.iter().any(|pat| {
        let pat = pat.strip_suffix('+').unwrap_or(pat);
        pat == "*"
            || pat == requirement
            || pat
                .strip_suffix('*')
                .is_some_and(|prefix| requirement.starts_with(prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn platform() -> Platform {
        Platform {
            php_version: "8.2.5".to_owned(),
            is_64bit: true,
            extensions: [("mbstring", "8.2.5"), ("intl", "8.2.5")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        }
    }

    fn lock(platform: Value, packages: Value) -> Lock {
        Lock::parse(&json!({ "packages": packages, "platform": platform }).to_string())
            .expect("lock")
    }

    #[test]
    fn satisfied_lock_passes() {
        let l = lock(
            json!({"php": ">=8.1", "ext-mbstring": "*"}),
            json!([{"name":"a/b","version":"1.0","require":{"php":"^8.0","ext-intl":"*"}}]),
        );
        assert!(check(&l, &platform(), true, &[]).is_empty());
    }

    #[test]
    fn reports_mismatch_missing_and_unsupported() {
        let l = lock(
            json!({"php": ">=8.3", "ext-gd": "*", "lib-icu": ">=70"}),
            json!([]),
        );
        let f = check(&l, &platform(), true, &[]);
        assert_eq!(f.len(), 3);
        assert_eq!(
            f[0].reason,
            FailureReason::Mismatch {
                installed: "8.2.5".to_owned()
            }
        );
        assert_eq!(f[1].reason, FailureReason::Missing);
        assert_eq!(f[2].reason, FailureReason::Unsupported);
    }

    #[test]
    fn package_requirements_are_checked_and_attributed() {
        let l = lock(
            json!({}),
            json!([{"name":"a/b","version":"1.0","require":{"php":">=8.3","some/dep":"^1.0"}}]),
        );
        let f = check(&l, &platform(), true, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].required_by.as_deref(), Some("a/b"));
        assert_eq!(f[0].requirement, "php");
    }

    #[test]
    fn ignore_patterns_work() {
        let l = lock(json!({"php": ">=9.0", "ext-gd": "*"}), json!([]));
        let all = check(&l, &platform(), true, &[]);
        assert_eq!(all.len(), 2);
        assert!(check(&l, &platform(), true, &["*".to_owned()]).is_empty());
        assert_eq!(check(&l, &platform(), true, &["ext-*".to_owned()]).len(), 1);
        assert_eq!(check(&l, &platform(), true, &["php".to_owned()]).len(), 1);
        assert_eq!(
            check(&l, &platform(), true, &["ext-gd+".to_owned()]).len(),
            1
        );
    }

    #[test]
    fn overrides_apply() {
        let mut p = platform();
        p.apply_overrides(&json!({"config": {"platform": {"php": "8.3.0", "ext-gd": "8.3.0", "ext-intl": false}}}));
        assert_eq!(p.php_version, "8.3.0");
        assert!(p.extensions.contains_key("gd"));
        assert!(!p.extensions.contains_key("intl"));
    }
}
