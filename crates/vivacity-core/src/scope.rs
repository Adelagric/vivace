//! Out-of-scope detector: decides, BEFORE touching the disk, whether vivacity
//! can install this lock natively or must delegate to `composer install`
//! (default fallback) / fail explicitly (when Composer is not available).
//!
//! Principle (plan r1/F3-F5): never a silently divergent vendor/. An unknown
//! plugin, or one that changes the layout, is out of scope. Plugins proven
//! harmless at boot (fixture qualification) are installed like ordinary
//! libraries, with a warning.

use crate::layout::Layout;
use crate::lock::{DistKind, Lock, LockPackage};
use serde_json::Value;
use std::path::Path;

/// Plugins emulated natively by vivacity (identical output, drift test).
/// composer/installers (see `layout`) is, under conditions checked before
/// any write. drupal/core-composer-scaffold is deliberately absent: its
/// source is GPL-2.0-or-later and cannot be ported here (NOTICE.md).
pub const EMULATED_PLUGINS: &[&str] = &["symfony/runtime", "composer/installers"];

/// Plugins whose inaction is proven to have no effect on the vendor/ content
/// needed at boot (fixtures qualified with `--no-plugins`). Installed as
/// libraries, reported with a warning.
pub const BENIGN_PLUGINS: &[&str] = &[
    "symfony/flex",
    "composer/package-versions-deprecated",
    "php-http/discovery",
    "dealerdirect/phpcodesniffer-composer-installer",
    "phpstan/extension-installer",
    "rector/extension-installer",
    "pestphp/pest-plugin",
    // Only listens to POST_CREATE_PROJECT_CMD / POST_INSTALL_CMD to print a
    // message (MessagePlugin::getSubscribedEvents): no disk effect.
    "drupal/core-project-message",
    // Only listens to POST_UPDATE_CMD / POST_CREATE_PROJECT_CMD, and only acts
    // in a `require` context (Plugin::getSubscribedEvents): inert at install.
    "drupal/core-recipe-unpack",
];

/// Plugins known to change the install layout or the package contents:
/// always out of scope.
pub const LAYOUT_PLUGINS: &[&str] = &[
    "cweagans/composer-patches",
    "oomphinc/composer-installers-extender",
    "mnsami/composer-custom-directory-installer",
];

#[derive(Debug, PartialEq, Eq)]
pub enum ScopeIssue {
    /// Plugin absent from the known lists: unpredictable behaviour.
    UnknownPlugin(String),
    /// Plugin known to change the layout (patches, installers-extender...).
    LayoutPlugin(String),
    /// Non-reproducible layout (composer/installers: version not ported,
    /// framework with custom logic, refused target...).
    Layout(String),
    /// Package without a usable zip dist (source-only, exotic dist).
    NoUsableDist(String),
}

impl std::fmt::Display for ScopeIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeIssue::UnknownPlugin(p) => {
                write!(f, "plugin {p} is not on vivacity's known-plugin list")
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
    /// Blocking: at least one -> fallback (or error without Composer).
    pub issues: Vec<ScopeIssue>,
    /// Non-blocking: harmless plugins ignored, to report on stderr.
    pub skipped_plugins: Vec<String>,
    /// Resolved layout (None if a layout issue blocks).
    pub layout: Option<Layout>,
}

impl ScopeReport {
    pub fn is_native_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

/// `plugins_enabled` = no `--no-plugins`: with the flag, Composer ignores
/// every plugin, composer/installers included; everything goes into vendor/.
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
    match Layout::resolve(project_dir, lock, root_manifest, with_dev, plugins_enabled) {
        Ok(layout) => report.layout = Some(layout),
        Err(issues) => report
            .issues
            .extend(issues.into_iter().map(ScopeIssue::Layout)),
    }
    report
}

/// Blocking plugin issues alone (unknown or layout-changing plugins in the
/// lock), for commands that do not install but would still let Composer run
/// plugin listeners — `dump-autoload` and its PRE_AUTOLOAD_DUMP.
pub fn plugin_issues(lock: &Lock, with_dev: bool) -> Vec<ScopeIssue> {
    let mut report = ScopeReport::default();
    for p in lock.wanted_packages(with_dev) {
        classify_plugin(p, &mut report);
    }
    report.issues
}

fn classify_package(p: &LockPackage, report: &mut ScopeReport) {
    classify_plugin(p, report);
    if !p.is_metapackage() && p.dist_kind() != DistKind::Zip {
        report
            .issues
            .push(ScopeIssue::NoUsableDist(p.name().to_owned()));
    }
}

fn classify_plugin(p: &LockPackage, report: &mut ScopeReport) {
    if p.package_type() != "composer-plugin" {
        return;
    }
    let name = p.name().to_owned();
    if EMULATED_PLUGINS.contains(&name.as_str()) {
        // Emulated natively: nothing to report.
    } else if BENIGN_PLUGINS.contains(&name.as_str()) {
        report.skipped_plugins.push(name);
    } else if LAYOUT_PLUGINS.contains(&name.as_str()) {
        report.issues.push(ScopeIssue::LayoutPlugin(name));
    } else {
        report.issues.push(ScopeIssue::UnknownPlugin(name));
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
        std::path::PathBuf::from("/nonexistent-vivacity-scope")
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
        // installer-paths alone is inert (as in Composer); the plugin without
        // allow-plugins, however, blocks.
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
