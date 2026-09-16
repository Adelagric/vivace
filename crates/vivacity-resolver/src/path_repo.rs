//! `Composer\Repository\PathRepository` (docs/reference/PathRepository.php):
//! every directory matched by the `url` glob that holds a `composer.json`
//! becomes a package with a `path` dist. The dist reference is
//! `sha1(json . serialize(options))` (the raw file bytes and the repository
//! options with `relative` auto-appended), replaced by the HEAD commit when
//! the directory has a `.git` of its own and `reference` is `auto`, null
//! with `reference: none`. The version comes, in this order, from
//! `options.versions[name]`, the package's `version`, `COMPOSER_ROOT_VERSION`
//! (when the directory and the project share the same HEAD), the git guess
//! (a feature branch adds the branch AND its parent as two packages), else
//! `dev-main`.

use crate::loader;
use crate::package::{Origin, Package};
use crate::repository::RepoError;
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::path::Path;
use vivacity_core::glob;
use vivacity_core::root_version;

/// A `path` repository: its packages are in the arena (`members`).
pub struct PathRepository {
    /// Arena indices, in `addPackage` order (an alias right before its base).
    pub members: Vec<usize>,
    /// The `url` as written in the configuration.
    pub url_as_written: String,
}

impl PathRepository {
    /// `getRepoName`.
    pub fn repo_name(&self) -> String {
        format!("path repo ({})", self.url_as_written)
    }
}

/// `Filesystem::isAbsolutePath` as written in Composer: a leading `/`, a
/// `:` as second character (any drive-letter form), or a leading `\\\\`.
fn php_is_absolute_path(path: &str) -> bool {
    path.starts_with('/') || path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\")
}

/// `__construct` + `initialize`: `def` is the repository configuration,
/// `project_dir` PHP's cwd, `origin` the repository's index in the set.
pub fn open(
    def: &Value,
    project_dir: &Path,
    origin: Origin,
    arena: &mut Vec<Package>,
) -> Result<PathRepository, RepoError> {
    let url_as_written = match def.get("url") {
        Some(Value::String(s)) => s.clone(),
        _ => {
            return Err(RepoError::data(
                "You must specify the `url` configuration for the path repository",
            ))
        }
    };
    let url = glob::expand_path(&url_as_written);
    let mut options: Map<String, Value> = match def.get("options") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    // `!isset($this->options['relative'])`: absent or null.
    if options.get("relative").is_none_or(Value::is_null) {
        options.insert("relative".into(), Value::Bool(!php_is_absolute_path(&url)));
    }
    let options = Value::Object(options);
    let serialized_options = vivacity_core::phpserialize::serialize(&options)
        .map_err(|e| RepoError::data(format!("path repository options: {e}")))?;

    let url_matches: Vec<String> = glob::glob_dirs(&url, project_dir)
        .into_iter()
        .map(|m| {
            // `str_replace(DIRECTORY_SEPARATOR, '/', …)`: a no-op on Unix.
            let m = if cfg!(windows) {
                m.replace('\\', "/")
            } else {
                m
            };
            m.trim_end_matches('/').to_owned()
        })
        .collect();
    if url_matches.is_empty() {
        let has_magic = |s: &str| s.contains(['*', '{', '}']);
        if has_magic(&url) {
            let mut walk = url.clone();
            while has_magic(&walk) {
                walk = glob::php_dirname(&walk);
            }
            if project_dir.join(&walk).is_dir() {
                return Ok(PathRepository {
                    members: Vec::new(),
                    url_as_written,
                });
            }
        }
        return Err(RepoError::data(format!(
            "The `url` supplied for the path ({url}) repository does not exist"
        )));
    }

    // `$this->options['reference'] ?? 'auto'`, compared with `===`: a
    // non-string value matches neither `none` nor `config`/`auto` (no
    // reference at all).
    let reference_mode = match options.get("reference") {
        None | Some(Value::Null) => "auto".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => String::new(),
    };
    let mut members: Vec<usize> = Vec::new();
    for matched in url_matches {
        let dir = project_dir.join(&matched);
        // `$path = realpath($url) . '/'`: the messages name the real path.
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        let composer_file = dir.join("composer.json");
        if !composer_file.exists() {
            continue;
        }
        let json = std::fs::read(&composer_file)
            .map_err(|e| RepoError::data(format!("{}: {e}", composer_file.display())))?;
        let mut package: Map<String, Value> = match serde_json::from_slice::<Value>(&json) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                return Err(RepoError::data(format!(
                    "\"{}\" does not contain valid JSON",
                    composer_file.display()
                )))
            }
            Err(e) => {
                return Err(RepoError::data(format!(
                    "\"{}\" does not contain valid JSON\n{e}",
                    composer_file.display()
                )))
            }
        };
        let mut dist = Map::new();
        dist.insert("type".into(), Value::String("path".into()));
        dist.insert("url".into(), Value::String(matched.clone()));
        match reference_mode.as_str() {
            "none" => {
                dist.insert("reference".into(), Value::Null);
            }
            "config" | "auto" => {
                let mut h = Sha1::new();
                h.update(&json);
                h.update(serialized_options.as_bytes());
                dist.insert(
                    "reference".into(),
                    Value::String(format!("{:x}", h.finalize())),
                );
            }
            _ => {}
        }
        package.insert("dist".into(), Value::Object(dist));

        // `array_intersect_key($this->options, ['symlink' => true, 'relative' => true])`
        let mut transport = Map::new();
        for (k, v) in options.as_object().into_iter().flatten() {
            if k == "symlink" || k == "relative" {
                transport.insert(k.clone(), v.clone());
            }
        }
        package.insert("transport-options".into(), Value::Object(transport));

        let name = package
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(name) = &name {
            if let Some(v) = options
                .get("versions")
                .and_then(|v| v.get(name))
                .filter(|v| !v.is_null())
            {
                package.insert("version".into(), v.clone());
            }
        }
        let has_version = |p: &Map<String, Value>| p.get("version").is_some_and(|v| !v.is_null());

        if !has_version(&package) {
            // `($rootVersion = Platform::getEnv('COMPOSER_ROOT_VERSION'))`:
            // empty and `0` are falsy (`root_version_from_env` says so).
            if let Some(env_version) = root_version::root_version_from_env() {
                let head = |d: &Path| root_version::git(d, &["rev-parse", "HEAD"]);
                if let (Some(a), Some(b)) = (head(&dir), head(project_dir)) {
                    if a == b {
                        package.insert("version".into(), Value::String(env_version));
                    }
                }
            }
        }

        if reference_mode == "auto" && dir.join(".git").is_dir() {
            if let Some(out) = root_version::git(
                &dir,
                &[
                    "rev-list",
                    "-n1",
                    "--format=%H",
                    "HEAD",
                    "--no-show-signature",
                ],
            ) {
                // `parseRevListOutput`: the `commit <sha>` header dropped.
                let sha = out
                    .lines()
                    .filter(|l| !l.starts_with("commit "))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(d) = package.get_mut("dist").and_then(Value::as_object_mut) {
                    d.insert("reference".into(), Value::String(sha.trim().to_owned()));
                }
            }
        }

        let mut feature: Option<String> = None;
        if !has_version(&package) {
            match root_version::guess_version(&Value::Object(package.clone()), &dir) {
                Some(g) if !g.pretty_version.is_empty() => {
                    feature = g.feature_pretty_version.filter(|f| !f.is_empty());
                    package.insert("version".into(), Value::String(g.pretty_version));
                }
                _ => {
                    package.insert("version".into(), Value::String("dev-main".into()));
                }
            }
        }
        let add = |config: Value, arena: &mut Vec<Package>, members: &mut Vec<usize>| {
            let ids = loader::load_packages(&[config], origin, arena, false).map_err(|e| {
                RepoError::data(format!(
                    "Failed loading the package in {}: {}",
                    composer_file.display(),
                    e.0
                ))
            })?;
            members.extend(ids);
            Ok::<(), RepoError>(())
        };
        if let Some(feature_version) = feature {
            let mut feature_package = package.clone();
            feature_package.insert("version".into(), Value::String(feature_version));
            add(Value::Object(feature_package), arena, &mut members)?;
        }
        add(Value::Object(package), arena, &mut members)?;
    }
    Ok(PathRepository {
        members,
        url_as_written,
    })
}
