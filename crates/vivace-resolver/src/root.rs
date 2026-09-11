//! Le paquet racine tel que `RootPackageLoader::load` le construit
//! (docs/reference/RootPackageLoader.php) : version (composer.json,
//! COMPOSER_ROOT_VERSION, git, sinon `1.0.0+no-version-set`), liens,
//! `minimum-stability`, `prefer-stable`, et les trois extractions depuis les
//! contraintes des requires : alias (`X as Y`), drapeaux de stabilité (`@dev`,
//! branches), références (`#sha`).

use crate::constraint::parse_constraints;
use crate::loader;
use crate::package::{Links, Origin, Package};
use crate::version::{
    self, group, normalize, regex, stability_rank, VersionError, STABILITIES_REGEX,
};
use pcre2::bytes::Regex;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

pub const DEFAULT_PRETTY_VERSION: &str = "1.0.0+no-version-set";

/// `RootPackage::getAliases()` : un alias déclaré dans un require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootAlias {
    pub package: String,
    pub version: String,
    pub alias: String,
    pub alias_normalized: String,
}

#[derive(Debug, Clone)]
pub struct RootPackage {
    pub package: Package,
    /// `RootAliasPackage` : (alias normalisé, alias joli) si `extra.branch-alias` s'applique.
    pub branch_alias: Option<(String, String)>,
    pub minimum_stability: String,
    pub prefer_stable: bool,
    /// name → rang de stabilité (`BasePackage::STABILITIES`).
    pub stability_flags: BTreeMap<String, i32>,
    pub aliases: Vec<RootAlias>,
    /// name → référence git.
    pub references: BTreeMap<String, String>,
    /// `config.platform`.
    pub platform_overrides: Map<String, Value>,
    pub manifest: Value,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RootError(pub String);

impl RootPackage {
    /// Charge composer.json (déjà parsé) avec la version racine devinée par
    /// vivace-core (même règles que RootPackageLoader + VersionGuesser).
    pub fn load(manifest: &Value, project_dir: &Path) -> Result<RootPackage, RootError> {
        let mut config = manifest
            .as_object()
            .cloned()
            .ok_or_else(|| RootError("composer.json is not an object".into()))?;
        if !config.contains_key("name") {
            config.insert("name".into(), Value::String("__root__".into()));
        }
        let mut auto_versioned = false;
        if !config.contains_key("version") {
            let rv = vivace_core::root_version::detect(manifest, project_dir);
            if rv.pretty_version == vivace_core::root_version::DEFAULT_PRETTY_VERSION {
                config.insert("version".into(), Value::String("1.0.0".into()));
                auto_versioned = true;
            } else {
                config.insert("version".into(), Value::String(rv.pretty_version.clone()));
                config.insert(
                    "version_normalized".into(),
                    Value::String(rv.version.clone()),
                );
                if let Some(commit) = rv.reference {
                    let r = serde_json::json!({"type": "", "url": "", "reference": commit});
                    config.insert("source".into(), r.clone());
                    config.insert("dist".into(), r);
                }
            }
        }
        let value = Value::Object(config.clone());
        let (mut package, alias) =
            loader::load(&value, Origin::Root, false).map_err(|e| RootError(e.0))?;
        if auto_versioned {
            package.pretty_version = DEFAULT_PRETTY_VERSION.to_owned();
        }
        let minimum_stability = match config.get("minimum-stability").and_then(Value::as_str) {
            Some(s) => normalize_stability(s)?,
            None => "stable".to_owned(),
        };
        let mut aliases = Vec::new();
        let mut stability_flags = BTreeMap::new();
        let mut references = BTreeMap::new();
        for links in [&package.requires, &package.dev_requires] {
            let map: Vec<(String, String)> = links
                .iter()
                // `$link->getConstraint()->getPrettyString()` : pour
                // `self.version`, la version de la racine.
                .map(|l| {
                    let pretty = if l.pretty_constraint == "self.version" {
                        // Version jolie au moment du parseLinks (avant
                        // `setPrettyVersion('1.0.0+no-version-set')`).
                        if auto_versioned {
                            "1.0.0".to_owned()
                        } else {
                            package.pretty_version.clone()
                        }
                    } else {
                        l.pretty_constraint.clone()
                    };
                    (l.target.clone(), pretty)
                })
                .collect();
            extract_aliases(&map, &mut aliases)?;
            extract_stability_flags(&map, &minimum_stability, &mut stability_flags);
            extract_references(&map, &mut references);
            if map.iter().any(|(n, _)| *n == package.name) {
                return Err(RootError(format!(
                    "Root package '{}' cannot require itself in its composer.json\nDid you accidentally name your root package after an external package?",
                    package.pretty_name
                )));
            }
        }
        let prefer_stable = config
            .get("prefer-stable")
            .map(|v| match v {
                Value::Bool(b) => *b,
                Value::Number(n) => n.as_f64() != Some(0.0),
                Value::String(s) => !(s.is_empty() || s == "0"),
                Value::Null => false,
                _ => true,
            })
            .unwrap_or(false);
        let platform_overrides = config
            .get("config")
            .and_then(|c| c.get("platform"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        Ok(RootPackage {
            package,
            branch_alias: alias,
            minimum_stability,
            prefer_stable,
            stability_flags,
            aliases,
            references,
            platform_overrides,
            manifest: manifest.clone(),
        })
    }

    /// Liens `require` + `require-dev` (`array_merge` : les dev-requires
    /// écrasent une cible en double, à sa position).
    pub fn all_requires(&self) -> Links {
        let mut out = self.package.requires.clone();
        for l in self.package.dev_requires.iter() {
            out.insert(l.clone());
        }
        out
    }
}

/// `VersionParser::normalizeStability`.
pub fn normalize_stability(s: &str) -> Result<String, RootError> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "stable" | "beta" | "alpha" | "dev" => Ok(lower),
        "rc" => Ok("RC".to_owned()),
        _ => Err(RootError(format!(
            "Invalid stability string \"{s}\", expected one of stable, RC, beta, alpha or dev"
        ))),
    }
}

/// `RootPackageLoader::extractAliases`.
fn extract_aliases(
    requires: &[(String, String)],
    aliases: &mut Vec<RootAlias>,
) -> Result<(), RootError> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &RE,
        r"(?:^|\| *|, *)([^,\s#|]+)(?:#[^ ]+)? +as +([^,\s|]+)(?:$| *\|| *,)",
        false,
    );
    for (name, req) in requires {
        if let Ok(Some(caps)) = re.captures(req.as_bytes()) {
            let v = group(&caps, 1);
            let a = group(&caps, 2);
            aliases.push(RootAlias {
                package: name.to_lowercase(),
                version: normalize(v, Some(req)).map_err(|e: VersionError| RootError(e.0))?,
                alias: a.to_owned(),
                alias_normalized: normalize(a, Some(req))
                    .map_err(|e: VersionError| RootError(e.0))?,
            });
        } else if req.contains(" as ") {
            return Err(RootError(format!(
                "Invalid alias definition in \"{name}\": \"{req}\". Aliases should be in the form \"exact-version as other-exact-version\"."
            )));
        }
    }
    Ok(())
}

/// `preg_split` des contraintes en morceaux « et » à travers les « ou ».
fn split_constraints(req: &str) -> Vec<String> {
    static OR: OnceLock<Regex> = OnceLock::new();
    static AND: OnceLock<Regex> = OnceLock::new();
    let or = regex(&OR, r"\s*\|\|?\s*", false);
    let and = regex(
        &AND,
        r"(?<!^|as|[=>< ,]) *(?<!-)[, ](?!-) *(?!,|as|$)",
        false,
    );
    let mut out = Vec::new();
    for part in split(or, req.trim()) {
        out.extend(split(and, &part));
    }
    out
}

fn split(re: &Regex, subject: &str) -> Vec<String> {
    let bytes = subject.as_bytes();
    let mut out = Vec::new();
    let mut last = 0;
    for m in re.find_iter(bytes).flatten() {
        if m.start() == m.end() && m.start() == last && last == bytes.len() {
            break;
        }
        out.push(subject[last..m.start()].to_owned());
        last = m.end();
    }
    out.push(subject[last..].to_owned());
    out
}

/// `RootPackageLoader::extractStabilityFlags`.
pub fn extract_stability_flags(
    requires: &[(String, String)],
    minimum_stability: &str,
    flags: &mut BTreeMap<String, i32>,
) {
    static AT: OnceLock<Regex> = OnceLock::new();
    static AS: OnceLock<Regex> = OnceLock::new();
    static PLAIN: OnceLock<Regex> = OnceLock::new();
    let at = regex(&AT, &format!("^[^@]*?@({STABILITIES_REGEX})$"), true);
    let as_re = regex(&AS, r"^([^,\s@]+) as .+$", false);
    let plain = regex(&PLAIN, r"^[^,\s@]+$", false);
    let minimum = stability_rank(minimum_stability);
    for (req_name, req) in requires {
        let constraints = split_constraints(req);
        let mut matched = false;
        for c in &constraints {
            if let Ok(Some(caps)) = at.captures(c.as_bytes()) {
                let name = req_name.to_lowercase();
                let Ok(stab) = normalize_stability(group(&caps, 1)) else {
                    continue;
                };
                let rank = stability_rank(&stab);
                if flags.get(&name).is_some_and(|f| *f > rank) {
                    continue;
                }
                flags.insert(name, rank);
                matched = true;
            }
        }
        if matched {
            continue;
        }
        for c in &constraints {
            let stripped = match as_re.captures(c.as_bytes()) {
                Ok(Some(caps)) => group(&caps, 1).to_owned(),
                _ => c.clone(),
            };
            if plain.is_match(stripped.as_bytes()).unwrap_or(false) {
                let stability = version::parse_stability(&stripped);
                if stability != "stable" {
                    let name = req_name.to_lowercase();
                    let rank = stability_rank(stability);
                    if flags.get(&name).is_some_and(|f| *f > rank) || minimum > rank {
                        continue;
                    }
                    flags.insert(name, rank);
                }
            }
        }
    }
}

/// `RootPackageLoader::extractReferences`.
fn extract_references(requires: &[(String, String)], references: &mut BTreeMap<String, String>) {
    static AS: OnceLock<Regex> = OnceLock::new();
    static REF: OnceLock<Regex> = OnceLock::new();
    let as_re = regex(&AS, r"^([^,\s@]+) as .+$", false);
    let re = regex(&REF, r"^[^,\s@]+?#([a-f0-9]+)$", false);
    for (name, req) in requires {
        let stripped = match as_re.captures(req.as_bytes()) {
            Ok(Some(caps)) => group(&caps, 1).to_owned(),
            _ => req.clone(),
        };
        if let Ok(Some(caps)) = re.captures(stripped.as_bytes()) {
            if version::parse_stability(&stripped) == "dev" {
                references.insert(name.to_lowercase(), group(&caps, 1).to_owned());
            }
        }
    }
}

/// Contrainte d'un require racine (avec `as` : la partie source).
pub fn root_constraint(pretty: &str) -> Result<crate::constraint::Constraint, VersionError> {
    Ok(parse_constraints(pretty)?.constraint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_flags_aliases_references() {
        let reqs = vec![
            ("a/b".to_owned(), "^1.0@beta".to_owned()),
            ("c/d".to_owned(), "dev-main as 1.0.x-dev".to_owned()),
            ("e/f".to_owned(), "dev-main#abcdef".to_owned()),
            ("g/h".to_owned(), "1.x-dev || ^2.0".to_owned()),
            ("i/j".to_owned(), "^1.0".to_owned()),
        ];
        let mut flags = BTreeMap::new();
        extract_stability_flags(&reqs, "stable", &mut flags);
        assert_eq!(flags.get("a/b"), Some(&10));
        assert_eq!(flags.get("c/d"), Some(&20));
        assert_eq!(flags.get("e/f"), Some(&20));
        assert_eq!(flags.get("g/h"), Some(&20));
        assert_eq!(flags.get("i/j"), None);
        let mut aliases = Vec::new();
        extract_aliases(&reqs, &mut aliases).unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].alias_normalized, "1.0.9999999.9999999-dev");
        let mut refs = BTreeMap::new();
        extract_references(&reqs, &mut refs);
        assert_eq!(refs.get("e/f").map(String::as_str), Some("abcdef"));
    }

    #[test]
    fn loads_root_without_git() {
        let m = json!({"name": "acme/app", "require": {"php": "^8.1", "monolog/monolog": "^3"}, "minimum-stability": "RC", "prefer-stable": true});
        let r = RootPackage::load(&m, Path::new("/nonexistent-vivace")).unwrap();
        assert_eq!(r.package.pretty_version, DEFAULT_PRETTY_VERSION);
        assert_eq!(r.package.version, "1.0.0.0");
        assert_eq!(r.minimum_stability, "RC");
        assert!(r.prefer_stable);
        assert_eq!(r.package.requires.len(), 2);
    }
}
