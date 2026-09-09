//! Détecteur hors-scope : décide, AVANT de toucher au disque, si vivace peut
//! installer ce lock nativement ou s'il doit déléguer à `composer install`
//! (fallback par défaut) / échouer explicitement (sans Composer disponible).
//!
//! Principe (plan r1/F3-F5) : jamais un vendor/ silencieusement divergent.
//! Un plugin inconnu ou modifiant le layout → hors scope. Les plugins prouvés
//! bénins au boot (qualification des fixtures) sont installés comme des
//! libraries ordinaires, avec un avertissement.

use crate::lock::{DistKind, Lock, LockPackage};
use serde_json::Value;

/// Plugins émulés nativement par vivace (sortie identique, test de drift).
pub const EMULATED_PLUGINS: &[&str] = &["symfony/runtime"];

/// Plugins dont l'inaction est prouvée sans effet sur le contenu de vendor/
/// nécessaire au boot (fixtures qualifiées avec `--no-plugins`). Installés
/// comme libraries, signalés par un avertissement.
pub const BENIGN_PLUGINS: &[&str] = &[
    "symfony/flex",
    "composer/package-versions-deprecated",
    "php-http/discovery",
    "dealerdirect/phpcodesniffer-composer-installer",
    "phpstan/extension-installer",
    "rector/extension-installer",
    "pestphp/pest-plugin",
];

/// Plugins connus pour modifier le layout d'installation ou le contenu des
/// paquets : toujours hors scope.
pub const LAYOUT_PLUGINS: &[&str] = &[
    "composer/installers",
    "cweagans/composer-patches",
    "oomphinc/composer-installers-extender",
    "mnsami/composer-custom-directory-installer",
];

#[derive(Debug, PartialEq, Eq)]
pub enum ScopeIssue {
    /// Plugin absent des listes connues — comportement imprévisible.
    UnknownPlugin(String),
    /// Plugin connu pour changer le layout (installers, patches…).
    LayoutPlugin(String),
    /// `extra.installer-paths` dans le composer.json racine.
    InstallerPaths,
    /// Paquet sans dist zip exploitable (source-only, dist exotique).
    NoUsableDist(String),
}

#[derive(Debug, Default)]
pub struct ScopeReport {
    /// Bloquants : au moins un → fallback (ou erreur sans Composer).
    pub issues: Vec<ScopeIssue>,
    /// Non bloquants : plugins bénins ignorés, à signaler sur stderr.
    pub skipped_plugins: Vec<String>,
}

impl ScopeReport {
    pub fn is_native_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

pub fn analyze(lock: &Lock, root_manifest: &Value, with_dev: bool) -> ScopeReport {
    let mut report = ScopeReport::default();

    if root_manifest
        .get("extra")
        .and_then(|e| e.get("installer-paths"))
        .is_some()
    {
        report.issues.push(ScopeIssue::InstallerPaths);
    }

    for p in lock.wanted_packages(with_dev) {
        classify_package(p, &mut report);
    }
    report
}

fn classify_package(p: &LockPackage, report: &mut ScopeReport) {
    let name = p.name().to_owned();

    if p.package_type() == "composer-plugin" {
        if EMULATED_PLUGINS.contains(&name.as_str()) {
            // Émulé nativement : rien à signaler.
        } else if BENIGN_PLUGINS.contains(&name.as_str()) {
            report.skipped_plugins.push(name.clone());
        } else if LAYOUT_PLUGINS.contains(&name.as_str()) {
            report.issues.push(ScopeIssue::LayoutPlugin(name.clone()));
        } else {
            report.issues.push(ScopeIssue::UnknownPlugin(name.clone()));
        }
    }

    if !p.is_metapackage() && p.dist_kind() != DistKind::Zip {
        report.issues.push(ScopeIssue::NoUsableDist(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::Lock;
    use serde_json::json;

    fn lock_with(packages: serde_json::Value) -> Lock {
        Lock::parse(&json!({ "packages": packages, "packages-dev": [] }).to_string()).expect("lock")
    }

    fn zip_pkg(name: &str, r#type: &str) -> serde_json::Value {
        json!({"name": name, "version": "1.0.0", "type": r#type,
               "dist": {"type": "zip", "url": "https://x/y.zip", "reference": "r"}})
    }

    #[test]
    fn plain_library_is_native() {
        let lock = lock_with(json!([zip_pkg("a/b", "library")]));
        let r = analyze(&lock, &json!({}), true);
        assert!(r.is_native_ok());
        assert!(r.skipped_plugins.is_empty());
    }

    #[test]
    fn emulated_and_benign_plugins_stay_native() {
        let lock = lock_with(json!([
            zip_pkg("symfony/runtime", "composer-plugin"),
            zip_pkg("symfony/flex", "composer-plugin"),
        ]));
        let r = analyze(&lock, &json!({}), true);
        assert!(r.is_native_ok());
        assert_eq!(r.skipped_plugins, vec!["symfony/flex"]);
    }

    #[test]
    fn unknown_or_layout_plugin_is_out_of_scope() {
        let lock = lock_with(json!([
            zip_pkg("acme/mystery-plugin", "composer-plugin"),
            zip_pkg("composer/installers", "composer-plugin"),
        ]));
        let r = analyze(&lock, &json!({}), true);
        assert_eq!(
            r.issues,
            vec![
                ScopeIssue::UnknownPlugin("acme/mystery-plugin".into()),
                ScopeIssue::LayoutPlugin("composer/installers".into()),
            ]
        );
    }

    #[test]
    fn installer_paths_and_sourceless_dist_are_out_of_scope() {
        let lock = lock_with(json!([
            {"name": "a/src-only", "version": "1.0.0", "type": "library",
             "source": {"type": "git", "url": "https://g/x.git", "reference": "r"}},
            {"name": "a/meta", "version": "1.0.0", "type": "metapackage"},
        ]));
        let manifest = json!({"extra": {"installer-paths": {"web/modules/{$name}": []}}});
        let r = analyze(&lock, &manifest, true);
        assert_eq!(
            r.issues,
            vec![
                ScopeIssue::InstallerPaths,
                ScopeIssue::NoUsableDist("a/src-only".into()),
            ]
        );
    }

    #[test]
    fn no_dev_skips_dev_packages() {
        let lock = Lock::parse(
            &json!({
                "packages": [zip_pkg("a/b", "library")],
                "packages-dev": [zip_pkg("acme/mystery-plugin", "composer-plugin")]
            })
            .to_string(),
        )
        .expect("lock");
        assert!(analyze(&lock, &json!({}), false).is_native_ok());
        assert!(!analyze(&lock, &json!({}), true).is_native_ok());
    }
}
