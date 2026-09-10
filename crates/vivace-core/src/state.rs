//! Génération des fichiers d'état de vendor/composer/ :
//! - `installed.json` : entrées du lock re-dumpées dans l'ordre de clés
//!   canonique d'ArrayDumper (docs/reference/ArrayDumper.php), enrichies de
//!   `version_normalized`, `installation-source` et `install-path`, triées par
//!   (nom, version), au format JsonFile (pretty 4 espaces, slashes/unicode non
//!   échappés) ;
//! - `installed.php` : port de FilesystemRepository::generateInstalledVersions
//!   + dumpToPhpCode (paquets réels, virtuels replaced/provided, racine) ;
//! - `InstalledVersions.php` : copie vendorée du fichier de Composer 2.10.3
//!   (c'est un fichier COPIÉ par Composer, pas généré — test de drift dédié).

use crate::error::{Error, Result};
use crate::lock::{Lock, LockPackage};
use crate::phpjson::{php_json_encode_with, FLAGS_JSONFILE};
use crate::version::normalize_pretty;
use serde_json::{Map, Value};

pub const INSTALLED_VERSIONS_PHP: &str = include_str!("../assets/InstalledVersions.php");

/// Ordre canonique des clés d'une entrée de paquet (ArrayDumper::dump, puis
/// install-path apposé par FilesystemRepository).
const ENTRY_KEY_ORDER: [&str; 33] = [
    "name",
    "version",
    "version_normalized",
    "target-dir",
    "source",
    "dist",
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
    "time",
    "default-branch",
    "bin",
    "type",
    "extra",
    "installation-source",
    "autoload",
    "autoload-dev",
    "notification-url",
    "include-path",
    "php-ext",
    "archive",
    "scripts",
    "license",
    "authors",
    "description",
    "homepage",
    "keywords",
    "repositories",
    "support",
    "funding",
];

/// Le paquet racine du projet (composer.json), pour installed.php.
#[derive(Debug, Clone)]
pub struct RootPackage {
    pub name: String,
    pub pretty_version: String,
    pub version: String,
    pub reference: Option<String>,
    pub package_type: String,
    pub dev: bool,
    /// Alias de branche (`extra.branch-alias`) : version jolie de l'alias.
    pub aliases: Vec<String>,
}

impl RootPackage {
    /// Comme RootPackageLoader : `version` du composer.json, sinon
    /// COMPOSER_ROOT_VERSION, sinon devinée depuis git, sinon
    /// `1.0.0+no-version-set` (voir root_version.rs).
    pub fn detect(manifest: &Value, project_dir: &std::path::Path, dev: bool) -> RootPackage {
        let name = manifest
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("__root__")
            .to_owned();
        let package_type = manifest
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("library")
            .to_owned();
        let rv = crate::root_version::detect(manifest, project_dir);
        let aliases = crate::root_version::branch_alias(manifest, &rv)
            .map(|(_, pretty)| vec![pretty])
            .unwrap_or_default();
        RootPackage {
            name,
            pretty_version: rv.pretty_version,
            version: rv.version,
            reference: rv.reference,
            package_type,
            dev,
            aliases,
        }
    }

    /// Sans détection VCS ni environnement (tests, cas sans projet sur disque).
    pub fn from_manifest(manifest: &Value, dev: bool) -> RootPackage {
        let mut r = RootPackage::detect(
            manifest,
            std::path::Path::new("/nonexistent-vivace-root"),
            dev,
        );
        if manifest.get("version").is_none() && std::env::var("COMPOSER_ROOT_VERSION").is_err() {
            r.pretty_version = crate::root_version::DEFAULT_PRETTY_VERSION.to_owned();
            r.version = "1.0.0.0".to_owned();
            r.reference = None;
            r.aliases = Vec::new();
        }
        r
    }
}

/// Chemin d'installation relatif à vendor/composer — findShortestPath de
/// Composer : les paquets du namespace `composer/*` vivent à côté des fichiers
/// d'état, donc `./pcre` plutôt que `../composer/pcre`.
fn relative_install_path(p: &LockPackage) -> String {
    let sub = p.install_subpath();
    match sub.strip_prefix("composer/") {
        Some(rest) => format!("./{rest}"),
        None => format!("../{sub}"),
    }
}

fn entry_install_path(p: &LockPackage) -> Value {
    if p.is_metapackage() {
        Value::Null
    } else {
        Value::String(relative_install_path(p))
    }
}

/// installed.json complet (texte, avec le newline final de JsonFile::write).
pub fn installed_json(lock: &Lock, with_dev: bool) -> Result<String> {
    let mut entries: Vec<&LockPackage> = lock.wanted_packages(with_dev).collect();
    entries.sort_by(|a, b| a.name().cmp(b.name()).then(a.version().cmp(b.version())));

    let mut packages = Vec::new();
    for p in &entries {
        let mut entry = Map::new();
        let mut src = p.raw.clone();
        src.insert(
            "version_normalized".to_owned(),
            Value::String(normalize_pretty(p.version()).unwrap_or_else(|_| p.version().to_owned())),
        );
        src.insert(
            "installation-source".to_owned(),
            Value::String("dist".to_owned()),
        );
        for key in ENTRY_KEY_ORDER {
            if let Some(v) = src.remove(key) {
                entry.insert(key.to_owned(), v);
            }
        }
        // Clés hors liste (rares) : après, dans leur ordre d'origine.
        for (k, v) in src {
            entry.insert(k, v);
        }
        entry.insert("install-path".to_owned(), entry_install_path(p));
        packages.push(Value::Object(entry));
    }

    let mut dev_names: Vec<Value> = lock
        .packages_dev
        .iter()
        .map(|p| Value::String(p.name().to_ascii_lowercase()))
        .collect();
    dev_names.sort_by(|a, b| a.as_str().cmp(&b.as_str()));

    let mut doc = Map::new();
    doc.insert("packages".to_owned(), Value::Array(packages));
    doc.insert("dev".to_owned(), Value::Bool(with_dev));
    doc.insert(
        "dev-package-names".to_owned(),
        Value::Array(if with_dev { dev_names } else { Vec::new() }),
    );
    let mut text = php_json_encode_with(&Value::Object(doc), FLAGS_JSONFILE)?;
    text.push('\n');
    Ok(text)
}

/// Une entrée du tableau `versions` d'installed.php.
#[derive(Debug, Default)]
struct VersionEntry {
    pretty_version: Option<String>,
    version: Option<String>,
    reference: Option<Option<String>>,
    package_type: Option<String>,
    install_path: Option<Option<String>>, // None = pas encore posé ; Some(None) = null
    dev_requirement: Option<bool>,
    aliases: Vec<String>,
    replaced: Vec<String>,
    provided: Vec<String>,
}

/// `PlatformRepository::isPlatformPackage` (php, hhvm, ext-*, lib-*,
/// composer, composer-plugin-api, composer-runtime-api).
fn is_platform_package(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "php"
        || n == "hhvm"
        || n == "composer"
        || n == "composer-plugin-api"
        || n == "composer-runtime-api"
        || matches!(
            n.as_str(),
            "php-64bit" | "php-ipv6" | "php-zts" | "php-debug"
        )
        || (n.starts_with("ext-") && !n.contains('/'))
        || (n.starts_with("lib-") && !n.contains('/'))
}

/// installed.php complet (port de generateInstalledVersions + dumpToPhpCode).
pub fn installed_php(
    lock: &Lock,
    root: &RootPackage,
    root_manifest: &Value,
    with_dev: bool,
) -> Result<String> {
    use std::collections::BTreeMap;

    let mut versions: BTreeMap<String, VersionEntry> = BTreeMap::new();
    let dev_names: std::collections::BTreeSet<&str> = lock
        .packages_dev
        .iter()
        .map(|p| p.raw.get("name").and_then(Value::as_str).unwrap_or(""))
        .collect();

    let packages: Vec<&LockPackage> = lock.wanted_packages(with_dev).collect();
    for p in &packages {
        let name = p.name().to_owned();
        let is_dev = dev_names.contains(name.as_str());
        let reference = p
            .dist_reference()
            .or_else(|| {
                p.raw
                    .get("source")
                    .and_then(|s| s.get("reference"))
                    .and_then(Value::as_str)
            })
            .map(str::to_owned);
        let entry = versions.entry(name.clone()).or_default();
        entry.pretty_version = Some(p.version().to_owned());
        entry.version =
            Some(normalize_pretty(p.version()).unwrap_or_else(|_| p.version().to_owned()));
        entry.reference = Some(reference);
        entry.package_type = Some(p.package_type().to_owned());
        entry.install_path = Some(if p.is_metapackage() {
            None
        } else {
            Some(format!("__DIR__ . '/{}'", relative_install_path(p)))
        });
        entry.dev_requirement = Some(is_dev);
    }

    // Paquets virtuels : replace puis provide (mêmes règles que Composer).
    for p in &packages {
        let is_dev = dev_names.contains(p.name());
        for (kind, is_replace) in [("replace", true), ("provide", false)] {
            if let Some(map) = p.raw.get(kind).and_then(Value::as_object) {
                for (target, constraint) in map {
                    if is_platform_package(target) {
                        continue;
                    }
                    let entry = versions.entry(target.clone()).or_default();
                    match entry.dev_requirement {
                        None => entry.dev_requirement = Some(is_dev),
                        Some(true) if !is_dev => entry.dev_requirement = Some(false),
                        _ => {}
                    }
                    let mut c = constraint.as_str().unwrap_or("*").to_owned();
                    if c == "self.version" {
                        c = p.version().to_owned();
                    }
                    let list = if is_replace {
                        &mut entry.replaced
                    } else {
                        &mut entry.provided
                    };
                    if !list.contains(&c) {
                        list.push(c);
                    }
                }
            }
        }
    }

    // replace/provide du composer.json racine (ex. polyfills remplacés).
    for (kind, is_replace) in [("replace", true), ("provide", false)] {
        if let Some(map) = root_manifest.get(kind).and_then(Value::as_object) {
            for (target, constraint) in map {
                if is_platform_package(target) {
                    continue;
                }
                let entry = versions.entry(target.clone()).or_default();
                entry.dev_requirement.get_or_insert(false);
                if entry.dev_requirement == Some(true) {
                    entry.dev_requirement = Some(false);
                }
                let mut c = constraint.as_str().unwrap_or("*").to_owned();
                if c == "self.version" {
                    c = root.pretty_version.clone();
                }
                let list = if is_replace {
                    &mut entry.replaced
                } else {
                    &mut entry.provided
                };
                if !list.contains(&c) {
                    list.push(c);
                }
            }
        }
    }

    // La racine fait partie de versions.
    {
        let entry = versions.entry(root.name.clone()).or_default();
        entry.pretty_version = Some(root.pretty_version.clone());
        entry.version = Some(root.version.clone());
        entry.reference = Some(root.reference.clone());
        entry.package_type = Some(root.package_type.clone());
        entry.install_path = Some(Some("__DIR__ . '/../../'".to_owned()));
        entry.dev_requirement = Some(false);
        entry.aliases = root.aliases.clone();
    }

    for e in versions.values_mut() {
        e.replaced.sort();
        e.provided.sort();
    }

    // Rendu au format dumpToPhpCode (4 espaces par niveau, var_export des
    // scalaires, install_path en expression __DIR__).
    let mut out = String::from("<?php return array(\n");
    out.push_str("    'root' => array(\n");
    push_kv(&mut out, 2, "name", &php_str(&root.name));
    push_kv(
        &mut out,
        2,
        "pretty_version",
        &php_str(&root.pretty_version),
    );
    push_kv(&mut out, 2, "version", &php_str(&root.version));
    push_kv(
        &mut out,
        2,
        "reference",
        &root
            .reference
            .as_deref()
            .map(php_str)
            .unwrap_or_else(|| "null".to_owned()),
    );
    push_kv(&mut out, 2, "type", &php_str(&root.package_type));
    push_kv(&mut out, 2, "install_path", "__DIR__ . '/../../'");
    if root.aliases.is_empty() {
        push_kv(&mut out, 2, "aliases", "array()");
    } else {
        push_string_list(&mut out, 2, "aliases", &root.aliases);
    }
    push_kv(&mut out, 2, "dev", if root.dev { "true" } else { "false" });
    out.push_str("    ),\n");
    out.push_str("    'versions' => array(\n");
    for (name, e) in &versions {
        out.push_str(&format!("        {} => array(\n", php_str(name)));
        if let Some(v) = &e.pretty_version {
            push_kv(&mut out, 3, "pretty_version", &php_str(v));
        }
        if let Some(v) = &e.version {
            push_kv(&mut out, 3, "version", &php_str(v));
        }
        if let Some(r) = &e.reference {
            push_kv(
                &mut out,
                3,
                "reference",
                &r.as_deref()
                    .map(php_str)
                    .unwrap_or_else(|| "null".to_owned()),
            );
        }
        if let Some(t) = &e.package_type {
            push_kv(&mut out, 3, "type", &php_str(t));
        }
        if let Some(ip) = &e.install_path {
            push_kv(&mut out, 3, "install_path", ip.as_deref().unwrap_or("null"));
        }
        if e.pretty_version.is_some() {
            if e.aliases.is_empty() {
                push_kv(&mut out, 3, "aliases", "array()");
            } else {
                push_string_list(&mut out, 3, "aliases", &e.aliases);
            }
        }
        if let Some(d) = e.dev_requirement {
            push_kv(
                &mut out,
                3,
                "dev_requirement",
                if d { "true" } else { "false" },
            );
        }
        push_string_list(&mut out, 3, "replaced", &e.replaced);
        push_string_list(&mut out, 3, "provided", &e.provided);
        out.push_str("        ),\n");
    }
    out.push_str("    ),\n");
    out.push_str(");\n");
    Ok(out)
}

fn push_kv(out: &mut String, level: usize, key: &str, value: &str) {
    for _ in 0..level {
        out.push_str("    ");
    }
    out.push_str(&format!("{} => {},\n", php_str(key), value));
}

fn push_string_list(out: &mut String, level: usize, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    for _ in 0..level {
        out.push_str("    ");
    }
    out.push_str(&format!("{} => array(\n", php_str(key)));
    for (i, v) in values.iter().enumerate() {
        for _ in 0..=level {
            out.push_str("    ");
        }
        out.push_str(&format!("{i} => {},\n", php_str(v)));
    }
    for _ in 0..level {
        out.push_str("    ");
    }
    out.push_str("),\n");
}

/// var_export() d'une chaîne PHP : quotes simples, `\` et `'` échappés.
fn php_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

pub fn write_state_files(
    vendor_composer: &std::path::Path,
    lock: &Lock,
    root: &RootPackage,
    root_manifest: &Value,
    with_dev: bool,
) -> Result<()> {
    std::fs::create_dir_all(vendor_composer).map_err(Error::io(vendor_composer))?;
    let writes = [
        ("installed.json", installed_json(lock, with_dev)?),
        (
            "installed.php",
            installed_php(lock, root, root_manifest, with_dev)?,
        ),
        ("InstalledVersions.php", INSTALLED_VERSIONS_PHP.to_owned()),
    ];
    for (file, content) in writes {
        let path = vendor_composer.join(file);
        let tmp = vendor_composer.join(format!(".{file}.vivace-tmp"));
        std::fs::write(&tmp, content).map_err(Error::io(&tmp))?;
        std::fs::rename(&tmp, &path).map_err(Error::io(&path))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_lock() -> Lock {
        Lock::parse(
            &json!({
                "packages": [
                    {"name": "a/lib", "version": "v1.2.0", "type": "library",
                     "dist": {"type": "zip", "url": "https://x/a.zip", "reference": "abcdef1234567890"},
                     "replace": {"a/lib-compat": "self.version", "php": "*"},
                     "provide": {"psr/log-implementation": "1.0"}},
                    {"name": "a/meta", "version": "2.0.0", "type": "metapackage"}
                ],
                "packages-dev": [
                    {"name": "d/tool", "version": "3.1.4", "type": "library",
                     "dist": {"type": "zip", "url": "https://x/d.zip", "reference": "feedfacefeedface"}}
                ]
            })
            .to_string(),
        )
        .expect("lock")
    }

    #[test]
    fn installed_json_shape() {
        let text = installed_json(&sample_lock(), true).expect("json");
        let v: Value = serde_json::from_str(&text).expect("parse");
        let names: Vec<&str> = v["packages"]
            .as_array()
            .expect("arr")
            .iter()
            .map(|p| p["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            vec!["a/lib", "a/meta", "d/tool"],
            "tri global par nom"
        );
        assert_eq!(v["packages"][0]["version_normalized"], "1.2.0.0");
        assert_eq!(v["packages"][0]["installation-source"], "dist");
        assert_eq!(v["packages"][0]["install-path"], "../a/lib");
        assert_eq!(v["packages"][1]["install-path"], Value::Null, "metapackage");
        assert_eq!(v["dev"], true);
        assert_eq!(v["dev-package-names"][0], "d/tool");
        // Ordre des clés : version_normalized juste après version.
        let entry_text = text.split("\"a/lib\"").nth(1).expect("entry");
        let vn = entry_text.find("version_normalized").expect("vn");
        let dist = entry_text.find("\"dist\"").expect("dist");
        assert!(vn < dist);

        let no_dev = installed_json(&sample_lock(), false).expect("json");
        let v: Value = serde_json::from_str(&no_dev).expect("parse");
        assert_eq!(v["packages"].as_array().expect("arr").len(), 2);
        assert_eq!(v["dev"], false);
    }

    #[test]
    fn installed_php_contains_virtual_and_root_entries() {
        let root = RootPackage {
            name: "acme/app".to_owned(),
            pretty_version: "1.0.0+no-version-set".to_owned(),
            version: "1.0.0.0".to_owned(),
            reference: None,
            package_type: "project".to_owned(),
            dev: true,
            aliases: Vec::new(),
        };
        let text = installed_php(&sample_lock(), &root, &json!({}), true).expect("php");
        assert!(text.starts_with("<?php return array(\n"));
        assert!(text.contains("'acme/app' => array("));
        assert!(text.contains("'a/lib-compat' => array("));
        assert!(
            text.contains("0 => 'v1.2.0',"),
            "self.version résolu: {text}"
        );
        assert!(text.contains("'psr/log-implementation' => array("));
        assert!(
            !text.contains("'php' => array("),
            "les cibles plateforme sont exclues"
        );
        assert!(text.contains("'install_path' => __DIR__ . '/../a/lib',"));
        assert!(
            text.contains("'install_path' => null,"),
            "metapackage sans chemin"
        );
        assert!(text.contains("'dev_requirement' => true,"));
    }

    #[test]
    fn php_str_escapes() {
        assert_eq!(php_str("a'b\\c"), r"'a\'b\\c'");
    }
}
