//! Port de `Composer\Repository\PlatformRepository` : les paquets de
//! plateforme (composer*, php*, ext-*, lib-*, hhvm) dans l'ordre exact de
//! Composer, avec les surcharges `config.platform`. Le sondage du PHP
//! courant est fait par assets/platform-probe.php (transcription de
//! `initialize()`, versions brutes) ; la normalisation et ses replis sont
//! rejoués ici avec le port exact de VersionParser.

use crate::constraint::{Constraint, Op};
use crate::package::{Link, LinkType, Links, Origin, Package};
use crate::version::{group, normalize, regex};
use pcre2::bytes::Regex;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::OnceLock;

/// Version de Composer émulée et ses API (Composer::VERSION,
/// PluginInterface::PLUGIN_API_VERSION, Composer::RUNTIME_API_VERSION).
pub const COMPOSER_VERSION: &str = "2.10.3";
pub const PLUGIN_API_VERSION: &str = "2.9.0";
pub const RUNTIME_API_VERSION: &str = "2.2.2";

const PROBE: &str = include_str!("../assets/platform-probe.php");

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PlatformError(pub String);

/// `PlatformRepository::PLATFORM_PACKAGE_REGEX`.
pub fn is_platform_package(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &RE,
        r"^(?:php(?:-64bit|-ipv6|-zts|-debug)?|hhvm|(?:ext|lib)-[a-z0-9](?:[_.-]?[a-z0-9]+)*|composer(?:-(?:plugin|runtime)-api)?)\z",
        true,
    );
    re.is_match(name.as_bytes()).unwrap_or(false)
}

/// Une entrée brute du sondage.
#[derive(Debug, Clone, serde::Deserialize)]
struct Probed {
    kind: String,
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    replaces: Vec<String>,
    #[serde(default)]
    provides: Vec<String>,
}

/// Lance le sondage sur le PHP courant (`VIVACE_PHP` ou `php`).
pub fn probe() -> Result<Vec<Value>, PlatformError> {
    let php = std::env::var("VIVACE_PHP").unwrap_or_else(|_| "php".to_owned());
    let dir = tempfile::tempdir().map_err(|e| PlatformError(e.to_string()))?;
    let script = dir.path().join("platform-probe.php");
    std::fs::write(&script, PROBE).map_err(|e| PlatformError(e.to_string()))?;
    let out = Command::new(&php)
        .arg(&script)
        .output()
        .map_err(|e| PlatformError(format!("cannot run {php}: {e}")))?;
    if !out.status.success() {
        return Err(PlatformError(format!(
            "platform probe failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| PlatformError(format!("platform probe output: {e}")))
}

struct Override {
    name: String,
    version: Option<String>,
}

/// La liste ordonnée des paquets de plateforme (`getPackages()`), `probed`
/// étant la sortie du sondage et `overrides` `config.platform`.
pub fn platform_packages(
    probed: &[Value],
    overrides_cfg: &Map<String, Value>,
) -> Result<Vec<Package>, PlatformError> {
    // Ordre d'insertion de `config.platform` (tableau PHP), pas trié.
    let mut overrides: Vec<(String, Override)> = Vec::new();
    for (name, version) in overrides_cfg {
        let v = match version {
            Value::String(s) => Some(s.clone()),
            Value::Bool(false) => None,
            other => {
                return Err(PlatformError(format!(
                    "config.platform.{name} should be a string or false, but got {other}"
                )))
            }
        };
        if name == "php" && v.is_none() {
            return Err(PlatformError(
                "config.platform.php cannot be set to false as you cannot disable php entirely."
                    .into(),
            ));
        }
        let key = name.to_lowercase();
        let o = Override {
            name: name.clone(),
            version: v,
        };
        match overrides.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = o,
            None => overrides.push((key, o)),
        }
    }
    let override_of = |name: &str| overrides.iter().find(|(k, _)| k == name).map(|(_, o)| o);

    let mut packages: Vec<Package> = Vec::new();
    let mut libraries: BTreeMap<String, bool> = BTreeMap::new();

    // addOverriddenPackage
    let add_overridden = |packages: &mut Vec<Package>,
                          o: &Override,
                          name: Option<&str>|
     -> Result<(), PlatformError> {
        let Some(pretty) = &o.version else {
            return Ok(());
        };
        let version = normalize(pretty, None).map_err(|e| PlatformError(e.0))?;
        let mut p = Package::new(name.unwrap_or(&o.name), &version, pretty, Origin::Platform);
        p.raw = serde_json::json!({"description": "Package overridden via config.platform", "extra": {"config.platform": true}});
        packages.push(p);
        Ok(())
    };
    for (_, o) in &overrides {
        if !is_platform_package(&o.name) {
            return Err(PlatformError(format!(
                "Invalid platform package name in config.platform: {}",
                o.name
            )));
        }
        if o.version.is_some() {
            add_overridden(&mut packages, o, None)?;
        }
    }
    // addPackage
    let add = |packages: &mut Vec<Package>, p: Package| -> Result<(), PlatformError> {
        if let Some(o) = override_of(&p.name) {
            if o.version.is_none() {
                return Ok(()); // désactivé
            }
            return Ok(()); // déjà ajouté par la surcharge
        }
        if let Some(php) = override_of("php") {
            if p.name.starts_with("php-") {
                return add_overridden(packages, php, Some(&p.pretty_name));
            }
        }
        packages.push(p);
        Ok(())
    };
    let simple = |name: &str, pretty: &str, description: &str| -> Result<Package, PlatformError> {
        let version = normalize(pretty, None).map_err(|e| PlatformError(e.0))?;
        let mut p = Package::new(name, &version, pretty, Origin::Platform);
        p.raw = serde_json::json!({"description": description});
        Ok(p)
    };
    add(
        &mut packages,
        simple("composer", COMPOSER_VERSION, "Composer package")?,
    )?;
    add(
        &mut packages,
        simple(
            "composer-plugin-api",
            PLUGIN_API_VERSION,
            "The Composer Plugin API",
        )?,
    )?;
    add(
        &mut packages,
        simple(
            "composer-runtime-api",
            RUNTIME_API_VERSION,
            "The Composer Runtime API",
        )?,
    )?;

    let entries: Vec<Probed> = probed
        .iter()
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| PlatformError(format!("probe entry: {e}")))
        })
        .collect::<Result<_, _>>()?;

    static PHP_FALLBACK: OnceLock<Regex> = OnceLock::new();
    static EXT_FALLBACK: OnceLock<Regex> = OnceLock::new();
    for e in &entries {
        match e.kind.as_str() {
            "php" => {
                let (version, pretty) = match normalize(&e.version, None) {
                    Ok(v) => (v, e.version.clone()),
                    Err(_) => {
                        let re = regex(&PHP_FALLBACK, r"^([^~+-]+).*$", false);
                        let pretty = match re.captures(e.version.as_bytes()) {
                            Ok(Some(caps)) => group(&caps, 1).to_owned(),
                            _ => e.version.clone(),
                        };
                        let v = normalize(&pretty, None).map_err(|x| PlatformError(x.0))?;
                        (v, pretty)
                    }
                };
                let mut p = Package::new(&e.name, &version, &pretty, Origin::Platform);
                p.raw = serde_json::json!({"description": e.description});
                add(&mut packages, p)?;
            }
            "ext" => {
                let mut extra_description = String::new();
                let (version, pretty) = match normalize(&e.version, None) {
                    Ok(v) => (v, e.version.clone()),
                    Err(_) => {
                        extra_description = format!(" (actual version: {})", e.version);
                        let re = regex(&EXT_FALLBACK, r"^(\d+\.\d+\.\d+(?:\.\d+)?)", false);
                        let pretty = match re.captures(e.version.as_bytes()) {
                            Ok(Some(caps)) => group(&caps, 1).to_owned(),
                            _ => "0".to_owned(),
                        };
                        let v = normalize(&pretty, None).map_err(|x| PlatformError(x.0))?;
                        (v, pretty)
                    }
                };
                let package_name = format!("ext-{}", e.name.to_lowercase().replace(' ', "-"));
                let mut p = Package::new(&package_name, &version, &pretty, Origin::Platform);
                p.package_type = "php-ext".to_owned();
                p.raw = serde_json::json!({"description": format!("The {} PHP extension{extra_description}", e.name)});
                if e.name == "uuid" {
                    p.replaces.insert(Link::new(
                        "ext-uuid",
                        "lib-uuid",
                        Constraint::new(Op::Eq, version.clone()),
                        &pretty,
                        LinkType::Replace,
                    ));
                }
                add(&mut packages, p)?;
            }
            "lib" => {
                let Ok(version) = normalize(&e.version, None) else {
                    continue;
                };
                let lib_name = format!("lib-{}", e.name);
                if !is_platform_package(&lib_name) || libraries.contains_key(&lib_name) {
                    continue;
                }
                libraries.insert(lib_name.clone(), true);
                let description = e
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("The {} library", e.name));
                let mut p = Package::new(&lib_name, &version, &e.version, Origin::Platform);
                p.raw = serde_json::json!({"description": description});
                let mut replaces = Links::default();
                for r in &e.replaces {
                    let r = r.to_lowercase();
                    // Clé PHP = nom nu, cible = `lib-<nom>` (addLibrary).
                    replaces.insert(Link {
                        key: Some(r.clone()),
                        source: lib_name.clone(),
                        target: format!("lib-{r}"),
                        constraint: Constraint::new(Op::Eq, version.clone()),
                        pretty_constraint: e.version.clone(),
                        kind: LinkType::Replace,
                    });
                }
                let mut provides = Links::default();
                for pr in &e.provides {
                    let pr = pr.to_lowercase();
                    provides.insert(Link {
                        key: Some(pr.clone()),
                        source: lib_name.clone(),
                        target: format!("lib-{pr}"),
                        constraint: Constraint::new(Op::Eq, version.clone()),
                        pretty_constraint: e.version.clone(),
                        kind: LinkType::Provide,
                    });
                }
                p.replaces = replaces;
                p.provides = provides;
                add(&mut packages, p)?;
            }
            _ => {}
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names() {
        assert!(is_platform_package("php"));
        assert!(is_platform_package("ext-mbstring"));
        assert!(is_platform_package("lib-icu-cldr"));
        assert!(is_platform_package("composer-plugin-api"));
        assert!(!is_platform_package("php-http/discovery"));
        assert!(!is_platform_package("ext-"));
    }

    #[test]
    fn builds_from_a_probe_with_overrides() {
        let probed = vec![
            serde_json::json!({"kind": "php", "name": "php", "version": "8.5.10", "description": "The PHP interpreter"}),
            serde_json::json!({"kind": "php", "name": "php-64bit", "version": "8.5.10", "description": "x"}),
            serde_json::json!({"kind": "ext", "name": "Zend OPcache", "version": "8.5.10"}),
            serde_json::json!({"kind": "ext", "name": "weird", "version": "not a version"}),
            serde_json::json!({"kind": "lib", "name": "libsodium", "version": "1.0.20", "replaces": [], "provides": []}),
            serde_json::json!({"kind": "lib", "name": "libsodium", "version": "1.0.20", "replaces": [], "provides": []}),
            serde_json::json!({"kind": "lib", "name": "libxml", "version": "2.13.4", "description": "libxml library version", "replaces": [], "provides": ["dom-libxml"]}),
        ];
        let mut overrides = Map::new();
        overrides.insert("php".into(), Value::String("8.2.0".into()));
        overrides.insert("ext-weird".into(), Value::Bool(false));
        let pk = platform_packages(&probed, &overrides).unwrap();
        let names: Vec<&str> = pk.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "php",
                "composer",
                "composer-plugin-api",
                "composer-runtime-api",
                "php-64bit",
                "ext-zend-opcache",
                "lib-libsodium",
                "lib-libxml"
            ]
        );
        assert_eq!(pk[0].version, "8.2.0.0");
        assert_eq!(
            pk[4].version, "8.2.0.0",
            "php-64bit prend la version surchargée de php"
        );
        // Clé PHP sans `lib-`, cible avec (addLibrary).
        let link = pk[7].provides.get("dom-libxml").unwrap();
        assert_eq!(link.target, "lib-dom-libxml");
        assert_eq!(link.constraint.to_string(), "== 2.13.4.0");
        assert!(pk[7].provides.get("lib-dom-libxml").is_none());
        // Ordre d'insertion des surcharges, pas alphabétique.
        let mut overrides = Map::new();
        overrides.insert("php".into(), Value::String("8.2.0".into()));
        overrides.insert("ext-mbstring".into(), Value::String("1.0".into()));
        overrides.insert("Ext-Json".into(), Value::String("2.0".into()));
        let pk = platform_packages(&probed, &overrides).unwrap();
        let names: Vec<&str> = pk.iter().take(3).map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["php", "ext-mbstring", "ext-json"]);
    }
}
