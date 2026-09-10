//! Port de composer/installers (`Installer` + `BaseInstaller`, tags 2.0.0 à
//! 2.3.0 — docs/reference/installers/ pour la source 2.3.0). La logique est
//! identique sur toute la série 2.x ; seules les tables d'emplacements
//! changent, d'où une table par tag dans assets/installers/<tag>.json
//! (générées par tools/gen-installers-table.php, jamais éditées à la main).
//!
//! Ce module ne décide que du **chemin relatif au projet** qu'un paquet
//! recevrait du plugin ; la normalisation, les refus de cibles dangereuses et
//! l'intégration (installed.json, autoload, proxies) sont dans `layout`.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct TableFile {
    tag: String,
    frameworks: Vec<Framework>,
}

/// Un installer de framework : sa clé (`wordpress`), sa classe PHP, `custom`
/// si la classe surcharge une méthode de BaseInstaller (chemins non portés),
/// et sa table type → template (`plugin` → `wp-content/plugins/{$name}/`).
#[derive(Debug, Clone, Deserialize)]
pub struct Framework {
    pub key: String,
    pub class: String,
    pub custom: bool,
    pub locations: BTreeMap<String, String>,
}

/// La table d'un tag donné, clés triées comme `krsort` (décroissant).
#[derive(Debug)]
pub struct Table {
    pub tag: String,
    /// Ordre de `findFrameworkType` : krsort des clés = strcmp décroissant.
    frameworks_desc: Vec<Framework>,
}

const TABLE_SOURCES: &[(&str, &str)] = &[
    ("2.0.0", include_str!("../assets/installers/v2.0.0.json")),
    ("2.0.1", include_str!("../assets/installers/v2.0.1.json")),
    ("2.1.0", include_str!("../assets/installers/v2.1.0.json")),
    ("2.1.1", include_str!("../assets/installers/v2.1.1.json")),
    ("2.2.0", include_str!("../assets/installers/v2.2.0.json")),
    ("2.3.0", include_str!("../assets/installers/v2.3.0.json")),
];

fn tables() -> &'static BTreeMap<&'static str, Table> {
    static TABLES: OnceLock<BTreeMap<&'static str, Table>> = OnceLock::new();
    TABLES.get_or_init(|| {
        TABLE_SOURCES
            .iter()
            .map(|(version, json)| {
                // Les assets sont générés et vérifiés par test : un JSON
                // invalide est un bug de build, pas une erreur d'exécution.
                let file: TableFile =
                    serde_json::from_str(json).unwrap_or_else(|e| panic!("asset {version}: {e}"));
                let mut frameworks = file.frameworks;
                frameworks.sort_by(|a, b| b.key.cmp(&a.key));
                (
                    *version,
                    Table {
                        tag: file.tag,
                        frameworks_desc: frameworks,
                    },
                )
            })
            .collect()
    })
}

/// Versions de composer/installers dont la table est embarquée.
pub fn ported_versions() -> impl Iterator<Item = &'static str> {
    TABLE_SOURCES.iter().map(|(v, _)| *v)
}

/// Table du tag verrouillé (`v2.3.0` ou `2.3.0`), None si non porté (1.x,
/// tag futur, branche dev).
pub fn table_for(locked_version: &str) -> Option<&'static Table> {
    let v = locked_version.strip_prefix('v').unwrap_or(locked_version);
    tables().get(v)
}

impl Table {
    pub fn frameworks(&self) -> impl Iterator<Item = &Framework> {
        self.frameworks_desc.iter()
    }

    pub fn framework(&self, key: &str) -> Option<&Framework> {
        self.frameworks_desc.iter().find(|f| f.key == key)
    }
}

/// Raison pour laquelle vivace ne calcule pas le chemin (→ fallback Composer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// Le framework surcharge BaseInstaller (inflexion personnalisée).
    CustomFramework { key: String, class: String },
    /// `<fw>-<type>` sans emplacement : Composer lève une exception.
    UnknownLocation { package_type: String },
    /// `extra.installer-paths` hors schéma `object<string, string|string[]>`.
    BadInstallerPaths(String),
    /// `extra.installer-name` non chaîne ou contenant `/`, `..` ou `{`.
    BadInstallerName(String),
    /// Variable de template autre que `{$name}`, `{$vendor}`, `{$type}`.
    UnknownTemplateVar(String),
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsupported::CustomFramework { key, class } => write!(
                f,
                "framework `{key}` ({class}) has custom path logic (not emulated yet)"
            ),
            Unsupported::UnknownLocation { package_type } => {
                write!(
                    f,
                    "package type `{package_type}` has no location in composer/installers"
                )
            }
            Unsupported::BadInstallerPaths(why) => {
                write!(f, "extra.installer-paths is malformed: {why}")
            }
            Unsupported::BadInstallerName(why) => {
                write!(f, "extra.installer-name is malformed: {why}")
            }
            Unsupported::UnknownTemplateVar(var) => {
                write!(
                    f,
                    "installer path template uses unknown variable `{{${var}}}`"
                )
            }
        }
    }
}

/// Résultat du dispatch `InstallationManager::getInstaller` : le plugin
/// prend le paquet (chemin relatif au projet, tel que templaté, barre finale
/// comprise) ou le laisse à LibraryInstaller (`vendor/<name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Vendor,
    Custom(String),
}

/// `Installer::removeDisabledInstallers` : clés retirées par
/// `extra.installer-disable` (racine). `true`, `"all"`, `"*"` (ou `1`)
/// désactivent tout (array_intersect compare en chaînes : `true` → `"1"`).
fn disabled_keys(root_extra: Option<&Value>) -> DisabledKeys {
    let Some(v) = root_extra.and_then(|e| e.get("installer-disable")) else {
        return DisabledKeys::None;
    };
    if matches!(v, Value::Bool(false) | Value::Null) {
        return DisabledKeys::None;
    }
    // `is_array` PHP : liste ou objet, itérés sur leurs valeurs.
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.values().collect(),
        other => vec![other],
    };
    let all = items.iter().any(|i| match i {
        Value::Bool(true) => true,
        Value::String(s) => s == "1" || s == "all" || s == "*",
        Value::Number(n) => n.as_i64() == Some(1) || n.as_f64() == Some(1.0),
        _ => false,
    });
    if all {
        return DisabledKeys::All;
    }
    DisabledKeys::Some(
        items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect(),
    )
}

enum DisabledKeys {
    None,
    All,
    Some(Vec<String>),
}

impl DisabledKeys {
    fn contains(&self, key: &str) -> bool {
        match self {
            DisabledKeys::None => false,
            DisabledKeys::All => true,
            DisabledKeys::Some(keys) => keys.iter().any(|k| k == key),
        }
    }
}

/// `Installer::findFrameworkType` : première clé (ordre krsort, décroissant)
/// qui est un préfixe du type.
fn find_framework<'t>(
    table: &'t Table,
    disabled: &DisabledKeys,
    package_type: &str,
) -> Option<&'t Framework> {
    table
        .frameworks_desc
        .iter()
        .filter(|f| !disabled.contains(&f.key))
        .find(|f| package_type.starts_with(&f.key))
}

/// `Installer::supports` : `preg_match('#<fw>-(<loc>|…)#', $type)`, non
/// ancré ; table vide → `(\w+)`.
fn supports(fw: &Framework, package_type: &str) -> bool {
    let prefix = format!("{}-", fw.key);
    let mut start = 0;
    while let Some(i) = package_type[start..].find(&prefix) {
        let rest = &package_type[start + i + prefix.len()..];
        let hit = if fw.locations.is_empty() {
            rest.bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        } else {
            fw.locations.keys().any(|k| rest.starts_with(k.as_str()))
        };
        if hit {
            return true;
        }
        start += i + 1;
    }
    false
}

/// `BaseInstaller::mapCustomInstallPaths` : première clé dont la liste
/// contient le nom, `type:<type>` ou `vendor:<vendor>` (`in_array` non
/// strict : `true` dans la liste matche tout ; les autres non-chaînes rien).
fn custom_install_path<'a>(
    paths: &'a Value,
    name: &str,
    package_type: &str,
    vendor: &str,
) -> Result<Option<&'a str>, Unsupported> {
    let Some(map) = paths.as_object() else {
        return Err(Unsupported::BadInstallerPaths(
            "expected an object of template => package list".into(),
        ));
    };
    let by_type = format!("type:{package_type}");
    let by_vendor = format!("vendor:{vendor}");
    for (template, names) in map {
        let list: Vec<&Value> = match names {
            Value::Array(a) => a.iter().collect(),
            other => vec![other],
        };
        for n in &list {
            match n {
                Value::String(s) => {
                    if s == name || *s == by_type || *s == by_vendor {
                        return Ok(Some(template.as_str()));
                    }
                }
                Value::Bool(true) => return Ok(Some(template.as_str())),
                Value::Bool(false) | Value::Null | Value::Number(_) => {}
                Value::Array(_) | Value::Object(_) => {
                    return Err(Unsupported::BadInstallerPaths(format!(
                        "nested value under `{template}`"
                    )))
                }
            }
        }
    }
    Ok(None)
}

/// `BaseInstaller::templatePath` : `{$var}` (`[A-Za-z0-9_]*`) remplacé par
/// la variable ; variable inconnue → Unsupported (Composer produirait une
/// chaîne vide avec un warning).
fn template_path(template: &str, vars: &BTreeMap<&str, &str>) -> Result<String, Unsupported> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find("{$") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let len = after
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if after[len..].starts_with('}') {
            let var = &after[..len];
            match vars.get(var) {
                Some(v) => out.push_str(v),
                None => return Err(Unsupported::UnknownTemplateVar(var.to_owned())),
            }
            rest = &after[len + 1..];
        } else {
            out.push_str("{$");
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Chemin qu'un paquet reçoit avec composer/installers actif (plugin
/// autorisé, version portée) : `Placement::Vendor` si le plugin ne prend pas
/// ce type (`supports` faux, framework désactivé), sinon le chemin templaté.
pub fn placement(
    table: &Table,
    root_extra: Option<&Value>,
    name: &str,
    package_type: &str,
    package_extra: Option<&Value>,
) -> Result<Placement, Unsupported> {
    let disabled = disabled_keys(root_extra);
    let Some(fw) = find_framework(table, &disabled, package_type) else {
        return Ok(Placement::Vendor);
    };
    if !supports(fw, package_type) {
        return Ok(Placement::Vendor);
    }
    if fw.custom {
        return Err(Unsupported::CustomFramework {
            key: fw.key.clone(),
            class: fw.class.clone(),
        });
    }

    // BaseInstaller::getInstallPath
    let (vendor, short_name) = match name.split_once('/') {
        Some((v, n)) => (v, n.split('/').next().unwrap_or(n)),
        None => ("", name),
    };
    let mut var_name = short_name.to_owned();
    if let Some(v) = package_extra.and_then(|e| e.get("installer-name")) {
        match v {
            Value::String(s) if s.is_empty() || s == "0" => {} // !empty
            Value::String(s) => {
                // `{` : templatePath ferait un str_replace séquentiel sur le
                // nom lui-même — refusé plutôt qu'imité.
                if s.contains('/') || s.contains('{') || s.split('/').any(|c| c == "..") {
                    return Err(Unsupported::BadInstallerName(format!("`{s}`")));
                }
                var_name = s.clone();
            }
            Value::Null | Value::Bool(false) => {}
            other => {
                return Err(Unsupported::BadInstallerName(format!(
                    "not a string: {other}"
                )))
            }
        }
    }
    let vars: BTreeMap<&str, &str> = BTreeMap::from([
        ("name", var_name.as_str()),
        ("vendor", vendor),
        ("type", package_type),
    ]);

    if let Some(paths) = root_extra.and_then(|e| e.get("installer-paths")) {
        // `!empty($extra['installer-paths'])`
        let empty = match paths {
            Value::Null | Value::Bool(false) => true,
            Value::String(s) => s.is_empty() || s == "0",
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Number(n) => n.as_f64() == Some(0.0),
            Value::Bool(true) => false,
        };
        if !empty {
            if let Some(template) = custom_install_path(paths, name, package_type, vendor)? {
                return Ok(Placement::Custom(template_path(template, &vars)?));
            }
        }
    }

    let location_key = &package_type[fw.key.len() + 1..];
    let Some(template) = fw.locations.get(location_key) else {
        return Err(Unsupported::UnknownLocation {
            package_type: package_type.to_owned(),
        });
    };
    Ok(Placement::Custom(template_path(template, &vars)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t() -> &'static Table {
        table_for("v2.3.0").expect("table 2.3.0")
    }

    #[test]
    fn tables_load_and_versions_are_known() {
        for v in ported_versions() {
            let tbl = table_for(v).expect(v);
            assert_eq!(tbl.tag, format!("v{v}"));
            assert!(tbl.framework("wordpress").is_some());
        }
        assert!(table_for("v1.12.0").is_none());
        assert!(table_for("2.4.0").is_none());
        assert!(table_for("dev-main").is_none());
    }

    #[test]
    fn framework_prefix_uses_krsort_order() {
        let d = DisabledKeys::None;
        assert_eq!(
            find_framework(t(), &d, "fuelphp-package").map(|f| f.key.as_str()),
            Some("fuelphp")
        );
        assert_eq!(
            find_framework(t(), &d, "fuel-package").map(|f| f.key.as_str()),
            Some("fuel")
        );
        assert_eq!(
            find_framework(t(), &d, "redaxo5-addon").map(|f| f.key.as_str()),
            Some("redaxo5")
        );
        assert_eq!(
            find_framework(t(), &d, "concretecms-package").map(|f| f.key.as_str()),
            Some("concretecms")
        );
        assert!(find_framework(t(), &d, "library").is_none());
    }

    #[test]
    fn wordpress_default_and_custom_paths() {
        let p = placement(
            t(),
            None,
            "wpackagist-plugin/akismet",
            "wordpress-plugin",
            None,
        );
        assert_eq!(
            p,
            Ok(Placement::Custom("wp-content/plugins/akismet/".into()))
        );

        let root = json!({"installer-paths": {
            "web/app/mu-plugins/{$name}/": ["type:wordpress-muplugin"],
            "web/app/plugins/{$name}/": ["type:wordpress-plugin"],
            "web/app/themes/{$vendor}-{$name}/": ["type:wordpress-theme"],
            "custom/{$name}": ["wpackagist-plugin/hello-dolly"]
        }});
        assert_eq!(
            placement(
                t(),
                Some(&root),
                "wpackagist-plugin/akismet",
                "wordpress-plugin",
                None
            ),
            Ok(Placement::Custom("web/app/plugins/akismet/".into()))
        );
        // Le nom exact prime seulement s'il vient avant dans l'ordre des clés.
        assert_eq!(
            placement(
                t(),
                Some(&root),
                "wpackagist-plugin/hello-dolly",
                "wordpress-plugin",
                None
            ),
            Ok(Placement::Custom("web/app/plugins/hello-dolly/".into()))
        );
        assert_eq!(
            placement(
                t(),
                Some(&root),
                "wpackagist-theme/twentytwentyfour",
                "wordpress-theme",
                None
            ),
            Ok(Placement::Custom(
                "web/app/themes/wpackagist-theme-twentytwentyfour/".into()
            ))
        );
        // Type non pris par le plugin → LibraryInstaller.
        assert_eq!(
            placement(t(), Some(&root), "a/b", "library", None),
            Ok(Placement::Vendor)
        );
        assert_eq!(
            placement(t(), Some(&root), "a/b", "wordpress-core", None),
            Ok(Placement::Vendor)
        );
        // Regex non ancrée : `wordpress-plugin-x` est supporté, mais sans
        // emplacement → exception chez Composer.
        assert_eq!(
            placement(t(), None, "a/b", "wordpress-plugin-x", None),
            Err(Unsupported::UnknownLocation {
                package_type: "wordpress-plugin-x".into()
            })
        );
    }

    #[test]
    fn installer_name_disable_and_custom_frameworks() {
        let extra = json!({"installer-name": "renamed"});
        assert_eq!(
            placement(t(), None, "a/b", "drupal-module", Some(&extra)),
            Ok(Placement::Custom("modules/renamed/".into()))
        );
        let empty = json!({"installer-name": ""});
        assert_eq!(
            placement(t(), None, "a/b", "drupal-module", Some(&empty)),
            Ok(Placement::Custom("modules/b/".into()))
        );
        let root_all = json!({"installer-disable": true});
        assert_eq!(
            placement(t(), Some(&root_all), "a/b", "drupal-module", None),
            Ok(Placement::Vendor)
        );
        let root_some = json!({"installer-disable": ["drupal"]});
        assert_eq!(
            placement(t(), Some(&root_some), "a/b", "drupal-module", None),
            Ok(Placement::Vendor)
        );
        assert_eq!(
            placement(t(), Some(&root_some), "a/b", "wordpress-plugin", None),
            Ok(Placement::Custom("wp-content/plugins/b/".into()))
        );
        assert!(matches!(
            placement(t(), None, "a/b", "cakephp-plugin", None),
            Err(Unsupported::CustomFramework { .. })
        ));
        assert_eq!(
            placement(
                t(),
                Some(&json!({"installer-paths": {"x/{$nope}": ["a/b"]}})),
                "a/b",
                "drupal-module",
                None
            ),
            Err(Unsupported::UnknownTemplateVar("nope".into()))
        );
    }
}
