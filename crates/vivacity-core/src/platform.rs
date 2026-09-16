//! Detection of the local platform (PHP, extensions) and check that the lock
//! is installable on it, the practical equivalent of Composer's "lock
//! installability" step, without a solver: `platform`/`platform-dev`
//! constraints of the lock + php/ext-* `require` of each locked package.
//!
//! Detection shells out ONCE to `php -r` and caches the result (key:
//! canonical path + mtime + size of the php binary), which is essential to
//! the no-op budget of < 50 ms, since PHP startup alone costs ~30-60 ms.

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
    /// extension name (lowercase) -> version (phpversion(ext), else the PHP version).
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
    /// "php", "ext-mbstring", ...
    pub requirement: String,
    pub constraint: String,
    /// Requesting package (None = the lock's platform section).
    pub required_by: Option<String>,
    pub reason: FailureReason,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FailureReason {
    Missing,
    Mismatch { installed: String },
    Unsupported,
}

/// Whether parallel file I/O pays on this machine: yes where I/O latency
/// dominates (Linux: ext4/WSL2 measured 2x on a wiped vendor/ and on a
/// cold classmap scan), no where the page cache is the bottleneck (APFS:
/// parallel reads 1.3-4x slower, DECISIONS.md M5). `VIVACITY_PARALLEL_IO`
/// (`0`/`1`) overrides the default.
pub fn parallel_io() -> bool {
    match std::env::var("VIVACITY_PARALLEL_IO") {
        Ok(v) => v != "0" && !v.is_empty(),
        Err(_) => cfg!(target_os = "linux"),
    }
}

pub fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("VIVACITY_CACHE_DIR") {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        if let Ok(l) = std::env::var("LOCALAPPDATA") {
            if !l.is_empty() {
                return PathBuf::from(l).join("vivacity");
            }
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("vivacity");
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_owned());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Caches/vivacity")
    } else {
        PathBuf::from(home).join(".cache/vivacity")
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
    /// Detects through `php` (overridable with $VIVACITY_PHP), with a disk cache.
    pub fn detect() -> Result<Option<Platform>> {
        let php = std::env::var("VIVACITY_PHP").unwrap_or_else(|_| "php".to_owned());
        let Some((path, mtime_unix, size)) = binary_identity(&php) else {
            return Ok(None); // no php: the caller decides (requirements present -> error)
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
                context: "php platform detection".to_owned(),
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

    /// Applies `config.platform` from the root composer.json (overrides the
    /// detected versions; `false` disables an entry).
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
    let path = if php.contains('/') || php.contains('\\') {
        PathBuf::from(php)
    } else {
        // `which` does not exist on Windows; `where` prints one line per
        // match, the first being the one the shell would launch.
        let finder = if cfg!(windows) { "where" } else { "which" };
        let out = Command::new(finder).arg(php).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8(out.stdout).ok()?;
        PathBuf::from(stdout.lines().next()?.trim())
    };
    let canonical = crate::pathutil::canonicalize(&path).ok()?;
    let meta = std::fs::metadata(&canonical).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((canonical.to_string_lossy().into_owned(), mtime, meta.len()))
}

/// Checks the lock against the platform. `ignored`: names to ignore, trailing
/// `*` accepted (`ext-*`), or the special list `["*"]` to ignore everything.
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
                // A platform package never has a `/` (otherwise it is a vendor
                // such as php-http/*). composer-plugin-api / composer-runtime-api
                // are excluded: satisfied by construction on the vivacity side.
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
        // lib-*: not detected in v1, so only its presence in the lock's
        // platform section concerns us, and we report it as Unsupported.
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
