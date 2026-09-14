//! Port de `Composer\Config\JsonConfigSource` pour composer.json (pas
//! auth.json) : chaque édition passe par `JsonManipulator` sur le texte du
//! fichier ; si le manipulateur renonce (`false`), Composer relit le
//! fichier, applique la même modification au tableau décodé et réécrit
//! tout avec `JsonFile::write` (indentation détectée, tableaux vides
//! rendus `{}` pour les clés qui sont des objets dans le schéma).
//!
//! Non porté : la validation `LAX_SCHEMA` après chaque écriture. Composer
//! restaure le fichier et échoue si le résultat viole le schéma, ce qui
//! revient à refuser d'éditer un manifeste déjà invalide (`"name"` en
//! majuscules, contrainte qui n'est pas une chaîne…) ; ici l'édition a
//! lieu. Un manifeste valide donne le même résultat des deux côtés.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use vivace_core::phpjson::{empty_stdclass, php_json_encode_with, FLAGS_JSONFILE};

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

/// Le composer.json d'un projet, édité en place.
pub struct JsonConfigSource {
    path: PathBuf,
}

/// Édition à tenter par le manipulateur, et son équivalent sur le tableau
/// décodé pour le repli.
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

    /// `removeLink` : retire `$name` de la section, puis la section si
    /// elle est vide.
    pub fn remove_link(&self, link_type: &str, name: &str) -> Result<()> {
        self.manipulate(Edit::RemoveSubNode(link_type, name))?;
        self.manipulate(Edit::RemoveMainKeyIfEmpty(link_type))
    }

    /// `addLink` (le tri est décidé par l'appelant, comme RequireCommand
    /// lit `config.sort-packages`).
    pub fn add_link(
        &self,
        link_type: &str,
        name: &str,
        constraint: &str,
        sort: bool,
    ) -> Result<()> {
        self.manipulate(Edit::AddLink(link_type, name, constraint, sort))
    }

    /// `removeConfigSetting` pour une clé de `config` (`allow-plugins`,
    /// `allow-plugins.vendor/name`).
    pub fn remove_config_setting(&self, name: &str) -> Result<()> {
        self.manipulate(Edit::RemoveConfigSetting(name))
    }

    /// `manipulateJson` : manipulateur d'abord, repli sur la réécriture
    /// complète.
    fn manipulate(&self, edit: Edit<'_>) -> Result<()> {
        // `$this->file->exists()` : sinon Composer part d'un squelette.
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
        // Repli : `$this->file->read()` puis la modification sur le tableau.
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
                    // `0 === count($config[$type])` : un tableau vide (ou
                    // absent — count(null) est une TypeError en PHP 8, que
                    // le manipulateur a déjà écartée en rendant `true`).
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
                    // Ni auth ni `policy.` : `unset($config['config'][$key])`
                    // avec la clé telle quelle (`allow-plugins.x` ne retire
                    // donc rien).
                    if let Some(Value::Object(cfg)) = root.get_mut("config") {
                        cfg.shift_remove(name);
                    }
                }
                Edit::AddLink(t, n, c, _) => {
                    // `$config[$type][$name] = $value` : la section devient
                    // un tableau si elle n'en est pas un.
                    let replacement = match root.get(t) {
                        Some(Value::Object(_)) => None,
                        // Une liste est un tableau à clés entières.
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

/// Les tableaux vides que `JsonFile::write` doit rendre `{}` (objets dans
/// le schéma) : `config.*` sensibles, `autoload.psr-*`, sections racines.
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

/// `JsonFile::encode` avec une indentation autre que 4 espaces : chaque
/// début de ligne de 4n espaces devient n indentations.
pub fn reindent(text: &str, indent: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        if spaces >= 4 {
            // `#^ {4,}#m` → `str_repeat($indent, (int) (strlen / 4))` : le
            // reste de la division disparaît.
            out.push_str(&indent.repeat(spaces / 4));
            out.push_str(&line[spaces..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Chemin du manifeste d'un projet (`Factory::getComposerFile`).
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
