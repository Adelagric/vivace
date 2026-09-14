//! Port of `Composer\Config\JsonConfigSource` for composer.json (not
//! auth.json): every edit goes through `JsonManipulator` on the file text;
//! if the manipulator gives up (`false`), Composer re-reads the file,
//! applies the same modification to the decoded array and rewrites
//! everything with `JsonFile::write` (detected indentation, empty arrays
//! rendered as `{}` for keys that are objects in the schema).
//!
//! Not ported: the `LAX_SCHEMA` validation after each write. Composer
//! restores the file and fails if the result violates the schema, which
//! amounts to refusing to edit an already invalid manifest (uppercase
//! `"name"`, non-string constraint...); here the edit goes through. A valid
//! manifest gives the same result on both sides.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use vivacity_core::phpjson::{empty_stdclass, php_json_encode_with, FLAGS_JSONFILE};

use crate::json_manipulator::{detect_indenting, JsonManipulator, ManipulatorError};

#[derive(Debug, thiserror::Error)]
pub enum ConfigSourceError {
    #[error("{0}")]
    Manipulator(#[from] ManipulatorError),
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    Encode(String),
}

type Result<T> = std::result::Result<T, ConfigSourceError>;

/// A project's composer.json, edited in place.
pub struct JsonConfigSource {
    path: PathBuf,
}

/// Edit to attempt with the manipulator, and its equivalent on the decoded
/// array for the fallback.
enum Edit<'a> {
    RemoveSubNode(&'a str, &'a str),
    RemoveMainKeyIfEmpty(&'a str),
    RemoveConfigSetting(&'a str),
    AddLink(&'a str, &'a str, &'a str, bool),
}

impl JsonConfigSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `removeLink`: removes `$name` from the section, then the section if
    /// it is empty.
    pub fn remove_link(&self, link_type: &str, name: &str) -> Result<()> {
        self.manipulate(Edit::RemoveSubNode(link_type, name))?;
        self.manipulate(Edit::RemoveMainKeyIfEmpty(link_type))
    }

    /// `addLink` (sorting is decided by the caller, as RequireCommand reads
    /// `config.sort-packages`).
    pub fn add_link(
        &self,
        link_type: &str,
        name: &str,
        constraint: &str,
        sort: bool,
    ) -> Result<()> {
        self.manipulate(Edit::AddLink(link_type, name, constraint, sort))
    }

    /// `removeConfigSetting` for a `config` key (`allow-plugins`,
    /// `allow-plugins.vendor/name`).
    pub fn remove_config_setting(&self, name: &str) -> Result<()> {
        self.manipulate(Edit::RemoveConfigSetting(name))
    }

    /// `manipulateJson`: manipulator first, fallback to the full rewrite.
    fn manipulate(&self, edit: Edit<'_>) -> Result<()> {
        // `$this->file->exists()`: otherwise Composer starts from a skeleton.
        let contents = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                "{\n    \"config\": {\n    }\n}\n".to_owned()
            }
            Err(source) => {
                return Err(ConfigSourceError::Read {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let mut manipulator = JsonManipulator::new(&contents)?;
        let done = match &edit {
            Edit::RemoveSubNode(main, name) => manipulator.remove_sub_node(main, name)?,
            Edit::RemoveMainKeyIfEmpty(key) => manipulator.remove_main_key_if_empty(key)?,
            Edit::RemoveConfigSetting(name) => manipulator.remove_config_setting(name)?,
            Edit::AddLink(t, n, c, sort) => manipulator.add_link(t, n, c, *sort)?,
        };
        if done {
            return self.write_text(&manipulator.contents());
        }
        // Fallback: `$this->file->read()` then the modification on the array.
        let mut config: Value = serde_json::from_str(&contents).map_err(|e| {
            ConfigSourceError::Encode(format!("cannot decode {}: {e}", self.path.display()))
        })?;
        let indent = detect_indenting(&contents)?;
        if let Value::Object(root) = &mut config {
            match edit {
                Edit::RemoveSubNode(main, name) => {
                    if let Some(Value::Object(section)) = root.get_mut(main) {
                        section.shift_remove(name);
                    }
                }
                Edit::RemoveMainKeyIfEmpty(key) => {
                    // `0 === count($config[$type])`: an empty array (or an
                    // absent one; count(null) is a TypeError in PHP 8, which
                    // the manipulator already ruled out by returning `true`).
                    let empty = match root.get(key) {
                        Some(Value::Object(m)) => m.is_empty(),
                        Some(Value::Array(a)) => a.is_empty(),
                        _ => false,
                    };
                    if empty {
                        root.shift_remove(key);
                    }
                }
                Edit::RemoveConfigSetting(name) => {
                    // Neither auth nor `policy.`: `unset($config['config'][$key])`
                    // with the key as is (so `allow-plugins.x` removes
                    // nothing).
                    if let Some(Value::Object(cfg)) = root.get_mut("config") {
                        cfg.shift_remove(name);
                    }
                }
                Edit::AddLink(t, n, c, _) => {
                    // `$config[$type][$name] = $value`: the section becomes
                    // an array if it is not one.
                    let replacement = match root.get(t) {
                        Some(Value::Object(_)) => None,
                        // A list is an array with integer keys.
                        Some(Value::Array(a)) => Some(Value::Object(
                            a.iter()
                                .enumerate()
                                .map(|(i, v)| (i.to_string(), v.clone()))
                                .collect(),
                        )),
                        Some(Value::Null) | None => Some(Value::Object(Map::new())),
                        Some(other) => {
                            return Err(ConfigSourceError::Encode(format!(
                                "Cannot use a scalar value as an array ({t}: {other})"
                            )))
                        }
                    };
                    if let Some(r) = replacement {
                        root.insert(t.to_owned(), r);
                    }
                    if let Some(Value::Object(section)) = root.get_mut(t) {
                        section.insert(n.to_owned(), Value::String(c.to_owned()));
                    }
                }
            }
            fixup_empty_objects(root);
        }
        let mut text = php_json_encode_with(&config, FLAGS_JSONFILE)
            .map_err(|e| ConfigSourceError::Encode(e.to_string()))?;
        if indent != "    " {
            text = reindent(&text, &indent);
        }
        text.push('\n');
        // `filePutContentsIfModified`.
        if std::fs::read_to_string(&self.path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        self.write_text(&text)
    }

    fn write_text(&self, text: &str) -> Result<()> {
        std::fs::write(&self.path, text).map_err(|source| ConfigSourceError::Write {
            path: self.path.clone(),
            source,
        })
    }
}

/// The empty arrays `JsonFile::write` must render as `{}` (objects in the
/// schema): relevant `config.*`, `autoload.psr-*`, root sections.
fn fixup_empty_objects(root: &mut Map<String, Value>) {
    let is_empty_array = |v: &Value| match v {
        Value::Object(m) => m.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    };
    if let Some(Value::Object(cfg)) = root.get_mut("config") {
        if let Some(Value::Object(policy)) = cfg.get_mut("policy") {
            for (_, v) in policy.iter_mut() {
                if is_empty_array(v) {
                    *v = empty_stdclass();
                }
            }
            if policy.is_empty() {
                cfg.insert("policy".into(), empty_stdclass());
            }
        }
        for prop in [
            "platform",
            "http-basic",
            "bearer",
            "gitlab-token",
            "gitlab-oauth",
            "github-oauth",
            "custom-headers",
            "forgejo-token",
            "preferred-install",
        ] {
            if cfg.get(prop).is_some_and(is_empty_array) {
                cfg.insert(prop.into(), empty_stdclass());
            }
        }
    }
    for section in ["autoload", "autoload-dev"] {
        if let Some(Value::Object(a)) = root.get_mut(section) {
            for prop in ["psr-0", "psr-4"] {
                if a.get(prop).is_some_and(is_empty_array) {
                    a.insert(prop.into(), empty_stdclass());
                }
            }
        }
    }
    for prop in [
        "require",
        "require-dev",
        "conflict",
        "provide",
        "replace",
        "suggest",
        "config",
        "autoload",
        "autoload-dev",
        "scripts",
        "scripts-descriptions",
        "scripts-aliases",
        "support",
    ] {
        if root.get(prop).is_some_and(is_empty_array) {
            root.insert(prop.into(), empty_stdclass());
        }
    }
}

/// `JsonFile::encode` with an indentation other than 4 spaces: every line
/// start of 4n spaces becomes n indents.
pub fn reindent(text: &str, indent: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        if spaces >= 4 {
            // `#^ {4,}#m` -> `str_repeat($indent, (int) (strlen / 4))`: the
            // remainder of the division is dropped.
            out.push_str(&indent.repeat(spaces / 4));
            out.push_str(&line[spaces..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Path of a project's manifest (`Factory::getComposerFile`).
pub fn composer_file(project: &Path) -> PathBuf {
    match std::env::var("COMPOSER") {
        Ok(f) if !f.trim().is_empty() => project.join(f.trim()),
        _ => project.join("composer.json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindent_replaces_leading_groups_of_four() {
        assert_eq!(
            reindent("{\n    \"a\": {\n        \"b\": 1\n    }\n}", "\t"),
            "{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}"
        );
        assert_eq!(reindent("{\n     \"a\": 1\n}", "  "), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn remove_link_edits_in_place_and_drops_empty_section() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("composer.json");
        std::fs::write(
            &path,
            "{\n  \"name\": \"a/b\",\n  \"require\": {\n    \"c/d\": \"^1\"\n  }\n}\n",
        )
        .expect("write");
        let src = JsonConfigSource::new(&path);
        src.remove_link("require", "c/d").expect("remove");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\n  \"name\": \"a/b\"\n}\n"
        );
    }
}
