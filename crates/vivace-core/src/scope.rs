//! Détecteur hors-scope : décide, AVANT de toucher au disque, si vivace peut
//! installer ce lock nativement ou s'il doit déléguer à `composer install`
//! (fallback par défaut) / échouer explicitement (sans Composer disponible).
//!
//! Principe (plan r1/F3-F5) : jamais un vendor/ silencieusement divergent.
//! Un plugin inconnu ou modifiant le layout → hors scope. Les plugins prouvés
//! bénins au boot (qualification des fixtures) sont installés comme des
//! libraries ordinaires, avec un avertissement.

use crate::layout::Layout;
use crate::lock::{DistKind, Lock, LockPackage};
use serde_json::Value;
use std::path::Path;

/// Plugins émulés nativement par vivace (sortie identique, test de drift).
/// composer/installers (voir `layout`) et drupal/core-composer-scaffold (voir
/// `scaffold`) le sont sous conditions vérifiées avant toute écriture.
pub const EMULATED_PLUGINS: &[&str] = &[
    "symfony/runtime",
    "composer/installers",
    "drupal/core-composer-scaffold",
];

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
    // N'écoute que POST_CREATE_PROJECT_CMD / POST_INSTALL_CMD pour afficher
    // un message (MessagePlugin::getSubscribedEvents) : aucun effet disque.
    "drupal/core-project-message",
    // N'écoute que POST_UPDATE_CMD / POST_CREATE_PROJECT_CMD, et n'agit que
    // dans un contexte `require` (Plugin::getSubscribedEvents) : inerte à l'install.
    "drupal/core-recipe-unpack",
];

/// Plugins connus pour modifier le layout d'installation ou le contenu des
/// paquets : toujours hors scope.
pub const LAYOUT_PLUGINS: &[&str] = &[
    "cweagans/composer-patches",
    "oomphinc/composer-installers-extender",
    "mnsami/composer-custom-directory-installer",
];

#[derive(Debug, PartialEq, Eq)]
pub enum ScopeIssue {
    /// Plugin absent des listes connues — comportement imprévisible.
    UnknownPlugin(String),
    /// Plugin connu pour changer le layout (patches, installers-extender…).
    LayoutPlugin(String),
    /// Disposition non reproductible (composer/installers : version non
    /// portée, framework à logique personnalisée, cible refusée…).
    Layout(String),
    /// Paquet sans dist zip exploitable (source-only, dist exotique).
    NoUsableDist(String),
}

impl std::fmt::Display for ScopeIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeIssue::UnknownPlugin(p) => {
                write!(f, "plugin {p} is not on vivace's known-plugin list")
            }
            ScopeIssue::LayoutPlugin(p) => {
                write!(f, "plugin {p} changes the install layout (not emulated)")
            }
            ScopeIssue::Layout(why) => write!(f, "{why}"),
            ScopeIssue::NoUsableDist(p) => write!(f, "package {p} has no zip dist (source-only)"),
        }
    }
}

#[derive(Debug, Default)]
pub struct ScopeReport {
    /// Bloquants : au moins un → fallback (ou erreur sans Composer).
    pub issues: Vec<ScopeIssue>,
    /// Non bloquants : plugins bénins ignorés, à signaler sur stderr.
    pub skipped_plugins: Vec<String>,
    /// Disposition résolue (None si une issue de layout bloque).
    pub layout: Option<Layout>,
    /// drupal/core-composer-scaffold verrouillé et autorisé : l'installeur
    /// vérifie l'empreinte de sa source et planifie le scaffold.
    pub scaffold: bool,
}

impl ScopeReport {
    pub fn is_native_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

/// `plugins_enabled` = pas de `--no-plugins` : avec le flag, Composer ignore
/// tout plugin, composer/installers compris — tout va dans vendor/.
pub fn analyze(
    project_dir: &Path,
    lock: &Lock,
    root_manifest: &Value,
    with_dev: bool,
    plugins_enabled: bool,
) -> ScopeReport {
    let mut report = ScopeReport::default();

    for p in lock.wanted_packages(with_dev) {
        classify_package(p, &mut report);
    }
    if plugins_enabled
        && lock
            .wanted_packages(with_dev)
            .any(|p| p.name() == crate::scaffold::PLUGIN)
    {
        match crate::layout::plugin_allowed(root_manifest, crate::scaffold::PLUGIN) {
            crate::layout::PluginVerdict::Allowed => report.scaffold = true,
            crate::layout::PluginVerdict::Blocked => {}
            crate::layout::PluginVerdict::Unlisted => report.issues.push(ScopeIssue::Layout(format!(
                "{} is a plugin not covered by config.allow-plugins (Composer would refuse to run it)",
                crate::scaffold::PLUGIN
            ))),
        }
    }
    match Layout::resolve(project_dir, lock, root_manifest, with_dev, plugins_enabled) {
        Ok(layout) => report.layout = Some(layout),
        Err(issues) => report
            .issues
            .extend(issues.into_iter().map(ScopeIssue::Layout)),
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

    fn proj() -> std::path::PathBuf {
        std::path::PathBuf::from("/nonexistent-vivace-scope")
    }

    #[test]
    fn plain_library_is_native() {
        let lock = lock_with(json!([zip_pkg("a/b", "library")]));
        let r = analyze(&proj(), &lock, &json!({}), true, true);
        assert!(r.is_native_ok());
        assert!(r.skipped_plugins.is_empty());
        assert_eq!(r.layout.expect("layout").rel("a/b"), Some("vendor/a/b"));
    }

    #[test]
    fn emulated_and_benign_plugins_stay_native() {
        let lock = lock_with(json!([
            zip_pkg("symfony/runtime", "composer-plugin"),
            zip_pkg("symfony/flex", "composer-plugin"),
        ]));
        let r = analyze(&proj(), &lock, &json!({}), true, true);
        assert!(r.is_native_ok());
        assert_eq!(r.skipped_plugins, vec!["symfony/flex"]);
    }

    #[test]
    fn unknown_or_layout_plugin_is_out_of_scope() {
        let lock = lock_with(json!([
            zip_pkg("acme/mystery-plugin", "composer-plugin"),
            zip_pkg("cweagans/composer-patches", "composer-plugin"),
        ]));
        let r = analyze(&proj(), &lock, &json!({}), true, true);
        assert_eq!(
            r.issues,
            vec![
                ScopeIssue::UnknownPlugin("acme/mystery-plugin".into()),
                ScopeIssue::LayoutPlugin("cweagans/composer-patches".into()),
            ]
        );
    }

    #[test]
    fn installers_without_allow_plugins_and_sourceless_dist_are_out_of_scope() {
        let lock = lock_with(json!([
            {"name": "a/src-only", "version": "1.0.0", "type": "library",
             "source": {"type": "git", "url": "https://g/x.git", "reference": "r"}},
            {"name": "a/meta", "version": "1.0.0", "type": "metapackage"},
            zip_pkg("composer/installers", "composer-plugin"),
        ]));
        // installer-paths seul est inerte (comme chez Composer) ; le plugin
        // sans allow-plugins, lui, bloque.
        let manifest = json!({"extra": {"installer-paths": {"web/modules/{$name}": []}}});
        let r = analyze(&proj(), &lock, &manifest, true, true);
        assert_eq!(r.issues.len(), 2, "{:?}", r.issues);
        assert_eq!(r.issues[0], ScopeIssue::NoUsableDist("a/src-only".into()));
        assert!(matches!(&r.issues[1], ScopeIssue::Layout(m) if m.contains("allow-plugins")));
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
        assert!(analyze(&proj(), &lock, &json!({}), false, true).is_native_ok());
        assert!(!analyze(&proj(), &lock, &json!({}), true, true).is_native_ok());
    }
}
