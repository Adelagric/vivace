//! Generation of the vendor/composer/ state files:
//! - `installed.json`: lock entries re-dumped in ArrayDumper's canonical key
//!   order (docs/reference/ArrayDumper.php), enriched with
//!   `version_normalized`, `installation-source` and `install-path`, sorted by
//!   (name, version), in JsonFile format (pretty 4 spaces, slashes/unicode
//!   unescaped);
//! - `installed.php`: port of FilesystemRepository::generateInstalledVersions
//!   + dumpToPhpCode (real packages, replaced/provided virtual ones, root);
//! - `InstalledVersions.php`: vendored copy of the Composer 2.10.3 file (it
//!   is a file COPIED by Composer, not generated; dedicated drift test).

use crate::error::{Error, Result};
use crate::layout::Layout;
use crate::lock::{Lock, LockPackage};
use crate::pathutil::php_str;
use crate::phpjson::{php_json_encode_with, FLAGS_JSONFILE};
use crate::version::normalize_pretty;
use serde_json::{Map, Value};

pub const INSTALLED_VERSIONS_PHP: &str = include_str!("../assets/InstalledVersions.php");

/// Canonical key order of a package entry (ArrayDumper::dump, then
/// install-path appended by FilesystemRepository).
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

/// The project's root package (composer.json), for installed.php.
#[derive(Debug, Clone)]
pub struct RootPackage {
    pub name: String,
    pub pretty_version: String,
    pub version: String,
    pub reference: Option<String>,
    pub package_type: String,
    pub dev: bool,
    /// Branch alias (`extra.branch-alias`): pretty version of the alias.
    pub aliases: Vec<String>,
    /// The same alias, normalised (`RootAliasPackage::getVersion()`).
    pub alias_normalized: Option<String>,
}

impl RootPackage {
    /// Like RootPackageLoader: `version` from composer.json, else
    /// COMPOSER_ROOT_VERSION, else guessed from git, else
    /// `1.0.0+no-version-set` (see root_version.rs).
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
        let alias = crate::root_version::branch_alias(manifest, &rv);
        RootPackage {
            name,
            pretty_version: rv.pretty_version,
            version: rv.version,
            reference: rv.reference,
            package_type,
            dev,
            aliases: alias.iter().map(|(_, pretty)| pretty.clone()).collect(),
            alias_normalized: alias.map(|(n, _)| n),
        }
    }

    /// Without VCS or environment detection (tests, cases with no project on disk).
    pub fn from_manifest(manifest: &Value, dev: bool) -> RootPackage {
        let mut r = RootPackage::detect(
            manifest,
            std::path::Path::new("/nonexistent-vivacity-root"),
            dev,
        );
        if manifest.get("version").is_none() && std::env::var("COMPOSER_ROOT_VERSION").is_err() {
            r.pretty_version = crate::root_version::DEFAULT_PRETTY_VERSION.to_owned();
            r.version = "1.0.0.0".to_owned();
            r.reference = None;
            r.aliases = Vec::new();
            r.alias_normalized = None;
        }
        r
    }
}

/// `install_path` of installed.php (dumpToPhpCode): `__DIR__ . '/<rel>'`,
/// or the string exported as is if Composer found no relative path
/// (absolute).
fn install_path_code(install_path: &str) -> String {
    if crate::pathutil::is_absolute_path(install_path) {
        php_str(install_path)
    } else {
        format!("__DIR__ . {}", php_str(&format!("/{install_path}")))
    }
}

/// Full installed.json (text, with JsonFile::write's trailing newline).
pub fn installed_json(lock: &Lock, with_dev: bool, layout: &Layout) -> Result<String> {
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
        // ArrayDumper: the key only exists if an installation source was
        // chosen, never for a metapackage (nothing is installed).
        if !p.is_metapackage() {
            src.insert(
                "installation-source".to_owned(),
                Value::String("dist".to_owned()),
            );
        }
        for key in ENTRY_KEY_ORDER {
            if let Some(v) = src.remove(key) {
                entry.insert(key.to_owned(), v);
            }
        }
        // Keys outside the list (rare): afterwards, in their original order.
        for (k, v) in src {
            entry.insert(k, v);
        }
        entry.insert(
            "install-path".to_owned(),
            layout
                .install_path(p.name())
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
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

/// An entry of the `versions` array of installed.php.
#[derive(Debug, Default)]
struct VersionEntry {
    pretty_version: Option<String>,
    version: Option<String>,
    reference: Option<Option<String>>,
    package_type: Option<String>,
    install_path: Option<Option<String>>, // None = not set yet; Some(None) = null
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

/// Full installed.php (port of generateInstalledVersions + dumpToPhpCode).
pub fn installed_php(
    lock: &Lock,
    root: &RootPackage,
    root_manifest: &Value,
    with_dev: bool,
    layout: &Layout,
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
        entry.install_path = Some(
            layout
                .install_path(p.name())
                .map(|ip| install_path_code(&ip)),
        );
        entry.dev_requirement = Some(is_dev);
        // Branch package: Composer loads an AliasPackage (branch-alias or
        // default-branch) and installed.php lists its pretty version.
        let default_branch = p
            .raw
            .get("default-branch")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some((_, pretty)) =
            crate::root_version::branch_alias_of(p.version(), p.raw.get("extra"), default_branch)
        {
            entry.aliases.push(pretty);
        }
    }

    // Virtual packages: replace then provide (same rules as Composer).
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

    // replace/provide of the root composer.json (e.g. replaced polyfills).
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

    // The root is part of versions.
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

    // Rendered in dumpToPhpCode format (4 spaces per level, var_export of
    // scalars, install_path as a __DIR__ expression).
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

pub fn write_state_files(
    vendor_composer: &std::path::Path,
    lock: &Lock,
    root: &RootPackage,
    root_manifest: &Value,
    with_dev: bool,
    layout: &Layout,
) -> Result<()> {
    std::fs::create_dir_all(vendor_composer).map_err(Error::io(vendor_composer))?;
    let writes = [
        ("installed.json", installed_json(lock, with_dev, layout)?),
        (
            "installed.php",
            installed_php(lock, root, root_manifest, with_dev, layout)?,
        ),
        ("InstalledVersions.php", INSTALLED_VERSIONS_PHP.to_owned()),
    ];
    for (file, content) in writes {
        let path = vendor_composer.join(file);
        // Deterministic content: skip the write (and the mtime bump) when
        // the file is already byte-identical — safe for parity.
        if std::fs::read(&path).is_ok_and(|existing| existing == content.as_bytes()) {
            continue;
        }
        let tmp = vendor_composer.join(format!(".{file}.vivacity-tmp"));
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
        let lock = sample_lock();
        let layout = Layout::vendor_only(std::path::Path::new("/proj"), &lock, true);
        let text = installed_json(&lock, true, &layout).expect("json");
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
            "global sort by name"
        );
        assert_eq!(v["packages"][0]["version_normalized"], "1.2.0.0");
        assert_eq!(v["packages"][0]["installation-source"], "dist");
        assert_eq!(v["packages"][0]["install-path"], "../a/lib");
        assert_eq!(v["packages"][1]["install-path"], Value::Null, "metapackage");
        assert_eq!(v["dev"], true);
        assert_eq!(v["dev-package-names"][0], "d/tool");
        // Key order: version_normalized right after version.
        let entry_text = text.split("\"a/lib\"").nth(1).expect("entry");
        let vn = entry_text.find("version_normalized").expect("vn");
        let dist = entry_text.find("\"dist\"").expect("dist");
        assert!(vn < dist);

        let lock = sample_lock();
        let layout = Layout::vendor_only(std::path::Path::new("/proj"), &lock, false);
        let no_dev = installed_json(&lock, false, &layout).expect("json");
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
            alias_normalized: None,
        };
        let lock = sample_lock();
        let layout = Layout::vendor_only(std::path::Path::new("/proj"), &lock, true);
        let text = installed_php(&lock, &root, &json!({}), true, &layout).expect("php");
        assert!(text.starts_with("<?php return array(\n"));
        assert!(text.contains("'acme/app' => array("));
        assert!(text.contains("'a/lib-compat' => array("));
        assert!(
            text.contains("0 => 'v1.2.0',"),
            "self.version resolved: {text}"
        );
        assert!(text.contains("'psr/log-implementation' => array("));
        assert!(
            !text.contains("'php' => array("),
            "platform targets are excluded"
        );
        assert!(text.contains("'install_path' => __DIR__ . '/../a/lib',"));
        assert!(
            text.contains("'install_path' => null,"),
            "metapackage without a path"
        );
        assert!(text.contains("'dev_requirement' => true,"));
    }

    #[test]
    fn php_str_escapes() {
        assert_eq!(php_str("a'b\\c"), r"'a\'b\\c'");
    }
}
