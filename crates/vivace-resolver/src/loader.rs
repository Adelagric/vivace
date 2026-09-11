//! Port de `Composer\Package\Loader\ArrayLoader` (docs/reference/resolver/
//! ArrayLoader.php) restreint aux champs du modèle, et de
//! `MetadataMinifier::expand`. Une entrée JSON (version p2 ou entrée de
//! lock) devient un `Package`, plus son alias de branche le cas échéant.

use crate::constraint::parse_constraints;
use crate::package::{Link, LinkType, Links, Origin, Package, SourceRef};
use crate::version::{
    normalize, normalize_branch, parse_numeric_alias_prefix, parse_stability, VersionError,
    DEFAULT_BRANCH_ALIAS,
};
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct LoadError(pub String);

/// `MetadataMinifier::expand`.
pub fn expand_minified(versions: &[Value]) -> Vec<Value> {
    let mut expanded: Vec<Value> = Vec::new();
    let mut current: Option<Map<String, Value>> = None;
    for v in versions {
        let data = v.as_object().cloned().unwrap_or_default();
        // `if (!$expandedVersion)` : un premier élément vide compte pour rien.
        if current.as_ref().is_some_and(Map::is_empty) {
            current = None;
        }
        match &mut current {
            None => {
                current = Some(data.clone());
                expanded.push(Value::Object(data));
            }
            Some(cur) => {
                for (k, val) in data {
                    if val == Value::String("__unset".into()) {
                        cur.shift_remove(&k);
                    } else {
                        cur.insert(k, val);
                    }
                }
                expanded.push(Value::Object(cur.clone()));
            }
        }
    }
    expanded
}

/// `ArrayLoader::getBranchAlias($config)` → alias normalisé.
pub fn branch_alias(config: &Map<String, Value>) -> Option<String> {
    let version = config.get("version")?.as_str()?;
    if !version.starts_with("dev-") && !version.ends_with("-dev") {
        return None;
    }
    if let Some(map) = config
        .get("extra")
        .and_then(|e| e.get("branch-alias"))
        .and_then(Value::as_object)
    {
        for (source, target) in map {
            let Some(target) = target.as_str() else {
                continue;
            };
            if !target.ends_with("-dev") {
                continue;
            }
            let validated = if target == DEFAULT_BRANCH_ALIAS {
                target.to_owned()
            } else {
                normalize_branch(&target[..target.len() - 4])
            };
            if !validated.ends_with("-dev") {
                continue;
            }
            if version.to_lowercase() != source.to_lowercase() {
                continue;
            }
            if let (Some(sp), Some(tp)) = (
                parse_numeric_alias_prefix(source),
                parse_numeric_alias_prefix(target),
            ) {
                if !tp.to_lowercase().starts_with(&sp.to_lowercase()) {
                    continue;
                }
            }
            return Some(validated);
        }
    }
    if config.get("default-branch") == Some(&Value::Bool(true)) {
        let v = version.strip_prefix('v').unwrap_or(version);
        if parse_numeric_alias_prefix(v).is_none() {
            return Some(DEFAULT_BRANCH_ALIAS.to_owned());
        }
    }
    None
}

/// `preg_replace('{(\.9{7})+}', '.x', $alias)`.
pub fn pretty_alias(normalized: &str) -> String {
    const X: &str = ".9999999";
    let mut out = String::new();
    let mut rest = normalized;
    while let Some(i) = rest.find(X) {
        out.push_str(&rest[..i]);
        out.push_str(".x");
        rest = &rest[i + X.len()..];
        while let Some(r) = rest.strip_prefix(X) {
            rest = r;
        }
    }
    out.push_str(rest);
    out
}

/// `(string)` PHP d'un scalaire JSON.
fn php_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "1".to_owned(),
        Value::Bool(false) | Value::Null => String::new(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// `source`/`dist` de `configureObject` : forme imposée, référence castée en
/// chaîne (`null` reste absent).
fn source_ref(name: &str, key: &str, v: Option<&Value>) -> Result<Option<SourceRef>, LoadError> {
    // `isset($config['source'])` : absent ou null → rien.
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let o = v.as_object();
    let has = |k: &str| o.is_some_and(|o| o.get(k).is_some_and(|x| !x.is_null()));
    let ok = if key == "source" {
        has("type") && has("url") && has("reference")
    } else {
        has("type") && has("url")
    };
    if !ok {
        let shape = if key == "source" {
            "{\"type\": ..., \"url\": ..., \"reference\": ...}"
        } else {
            "{\"type\": ..., \"url\": ..., \"reference\": ..., \"shasum\": ...}"
        };
        return Err(LoadError(format!(
            "Package {name}'s {key} key should be specified as {shape},\n{v} given."
        )));
    }
    let o = o.expect("object checked above");
    Ok(Some(SourceRef {
        kind: php_string(&o["type"]),
        url: php_string(&o["url"]),
        reference: o.get("reference").filter(|r| !r.is_null()).map(php_string),
    }))
}

/// `ArrayLoader::parseLinks` (`load`, un paquet) ou
/// `configureCachedLinks` (`loadPackages`, lot d'un dépôt) pour un type de
/// lien. En lot, un lien vers le paquet lui-même est ignoré, et une
/// contrainte non textuelle est une erreur ; à l'unité, elle est ignorée.
fn parse_links(
    source: &str,
    source_version: &str,
    kind: LinkType,
    map: Option<&Value>,
    batch: bool,
) -> Result<Links, LoadError> {
    let mut out = Links::default();
    let Some(obj) = map.and_then(Value::as_object) else {
        return Ok(out);
    };
    for (target, constraint) in obj {
        let target = target.to_lowercase();
        if batch && target == source {
            continue;
        }
        let Some(pretty) = constraint.as_str() else {
            if batch {
                return Err(LoadError(format!(
                    "Link constraint in {source} {} > {target} should be a string, got {}",
                    kind.description(),
                    match constraint {
                        Value::Null => "null".to_owned(),
                        Value::Bool(_) => "bool".to_owned(),
                        Value::Number(n) =>
                            if n.is_f64() {
                                "float".to_owned()
                            } else {
                                "int".to_owned()
                            },
                        Value::Array(_) | Value::Object(_) => "array".to_owned(),
                        Value::String(_) => unreachable!(),
                    }
                )));
            }
            continue;
        };
        let text = if pretty == "self.version" {
            source_version
        } else {
            pretty
        };
        let parsed = parse_constraints(text).map_err(|e| {
            LoadError(format!(
                "Link constraint in {source} {} > {target} should be a valid version constraint, got \"{text}\": {}",
                kind.description(),
                e.0
            ))
        })?;
        out.insert(Link::new(source, &target, parsed.constraint, pretty, kind));
    }
    Ok(out)
}

/// `ArrayLoader::load($config)` (`batch` = false) ou une itération de
/// `ArrayLoader::loadPackages` (`batch` = true) : le paquet, et son alias
/// éventuel (à ajouter juste avant lui, comme `ArrayRepository::addPackage`).
pub fn load(
    config: &Value,
    origin: Origin,
    batch: bool,
) -> Result<(Package, Option<(String, String)>), LoadError> {
    let obj = config
        .as_object()
        .ok_or_else(|| LoadError("package config is not an object".into()))?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| LoadError(format!("Unknown package has no name defined ({config}).")))?;
    let pretty_version = match obj.get("version") {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_))) => php_string(v),
        _ => return Err(LoadError(format!("Package {name} has no version defined."))),
    };
    let version = match obj.get("version_normalized").and_then(Value::as_str) {
        Some(v) if v == DEFAULT_BRANCH_ALIAS => {
            normalize(&pretty_version, None).map_err(|e: VersionError| {
                LoadError(format!(
                    "Failed to normalize version for package \"{name}\": {}",
                    e.0
                ))
            })?
        }
        Some(v) => v.to_owned(),
        None => normalize(&pretty_version, None).map_err(|e: VersionError| {
            LoadError(format!(
                "Failed to normalize version for package \"{name}\": {}",
                e.0
            ))
        })?,
    };
    let mut p = Package::new(name, &version, &pretty_version, origin);
    p.package_type = obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_lowercase)
        .unwrap_or_else(|| "library".to_owned());
    p.is_default_branch = obj.get("default-branch") == Some(&Value::Bool(true));
    p.source = source_ref(name, "source", obj.get("source"))?;
    p.dist = source_ref(name, "dist", obj.get("dist"))?;
    p.stability = parse_stability(&version);
    let src = p.name.clone();
    p.requires = parse_links(
        &src,
        &pretty_version,
        LinkType::Require,
        obj.get("require"),
        batch,
    )?;
    p.conflicts = parse_links(
        &src,
        &pretty_version,
        LinkType::Conflict,
        obj.get("conflict"),
        batch,
    )?;
    p.provides = parse_links(
        &src,
        &pretty_version,
        LinkType::Provide,
        obj.get("provide"),
        batch,
    )?;
    p.replaces = parse_links(
        &src,
        &pretty_version,
        LinkType::Replace,
        obj.get("replace"),
        batch,
    )?;
    p.dev_requires = parse_links(
        &src,
        &pretty_version,
        LinkType::DevRequire,
        obj.get("require-dev"),
        batch,
    )?;
    p.raw = config.clone();
    let alias = branch_alias(obj).map(|normalized| {
        let pretty = pretty_alias(&normalized);
        (normalized, pretty)
    });
    Ok((p, alias))
}

/// `ArrayLoader::loadPackages` (`batch`) ou `load()` répété, suivi de
/// l'ajout à un dépôt : `load()` rend
/// l'AliasPackage quand il y en a un, et le dépôt ajoute l'alias PUIS le
/// paquet aliasé (`ArrayRepository::addPackage`, `loadAsyncPackages`). L'ordre
/// rendu est donc [alias, base] ; l'arène, elle, garde base avant alias.
pub fn load_packages(
    configs: &[Value],
    origin: Origin,
    arena: &mut Vec<Package>,
    batch: bool,
) -> Result<Vec<usize>, LoadError> {
    let mut out = Vec::new();
    for c in configs {
        let (p, alias) = load(c, origin, batch)?;
        let idx = arena.len();
        arena.push(p);
        if let Some((normalized, pretty)) = alias {
            let a = arena[idx].alias(idx, &normalized, &pretty);
            let aidx = arena.len();
            arena.push(a);
            out.push(aidx);
        }
        out.push(idx);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expands_minified_metadata() {
        let v = vec![
            json!({"name": "a/b", "version": "1.1.0", "require": {"php": ">=8"}, "time": "t1"}),
            json!({"version": "1.0.0", "time": "__unset"}),
        ];
        let e = expand_minified(&v);
        assert_eq!(e[1]["version"], "1.0.0");
        assert_eq!(e[1]["require"]["php"], ">=8");
        assert!(e[1].get("time").is_none());
        assert_eq!(e[0]["time"], "t1");
    }

    #[test]
    fn loads_package_with_alias_and_self_version() {
        let cfg = json!({"name": "Acme/Lib", "version": "dev-main", "default-branch": true,
            "replace": {"acme/old": "self.version"}, "require": {"php": "^8.1"}});
        let mut arena = Vec::new();
        let ids = load_packages(&[cfg], Origin::Repository(0), &mut arena, false).unwrap();
        assert_eq!(
            ids,
            vec![1, 0],
            "alias d'abord, comme ArrayRepository::addPackage"
        );
        let p = &arena[0];
        assert_eq!(p.name, "acme/lib");
        assert_eq!(p.version, "dev-main");
        assert_eq!(
            p.replaces.get("acme/old").unwrap().constraint.to_string(),
            "== dev-main"
        );
        let a = &arena[1];
        assert_eq!(a.version, "9999999-dev");
        assert_eq!(a.pretty_version, "9999999-dev");
        assert_eq!(a.alias_of, Some(0));
        // replace self.version : lien d'origine + lien alias ajouté.
        assert_eq!(a.replaces.0.len(), 2);
        assert_eq!(a.replaces.0[1].constraint.to_string(), "== 9999999-dev");
        assert_eq!(a.replaces.0[1].pretty_constraint, "dev-main");
        // En lot (dépôt composer), un lien vers soi-même est ignoré
        // (`configureCachedLinks`) ; à l'unité (lock, racine) il reste.
        let cfg = json!({"name": "acme/lib", "version": "1.0.0", "replace": {"acme/lib": "self.version", "acme/old": "1.0"}});
        let mut arena = Vec::new();
        load_packages(
            std::slice::from_ref(&cfg),
            Origin::Repository(0),
            &mut arena,
            true,
        )
        .unwrap();
        assert_eq!(
            arena[0]
                .replaces
                .0
                .iter()
                .map(|l| l.target.as_str())
                .collect::<Vec<_>>(),
            vec!["acme/old"]
        );
        load_packages(&[cfg], Origin::Locked, &mut arena, false).unwrap();
        assert_eq!(arena[1].replaces.len(), 2);
        assert_eq!(pretty_alias("1.9999999.9999999.9999999-dev"), "1.x-dev");
        assert_eq!(pretty_alias("2.0.9999999.9999999-dev"), "2.0.x-dev");
    }
}
