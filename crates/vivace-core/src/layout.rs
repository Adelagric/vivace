//! Où chaque paquet du lock s'installe : `vendor/<name>[/<target-dir>]` par
//! LibraryInstaller, ou le chemin que composer/installers lui donne quand ce
//! plugin est verrouillé, autorisé (`config.allow-plugins`) et porté
//! (`installers::table_for`). Une seule passe, avant de toucher au disque ;
//! tout ce qui n'est pas reproductible à l'octet près devient une `issue`
//! (→ fallback Composer).
//!
//! Le chemin est relatif à la racine du projet et normalisé (`normalizePath`,
//! sans barre finale) — c'est la forme que Composer normalise avant de
//! calculer `install-path` (FilesystemRepository::write).

use crate::installers::{self, Placement};
use crate::lock::{Lock, LockPackage};
use crate::pathutil::{find_shortest_path, normalize_path};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Layout {
    /// Racine du projet, absolue (telle que donnée, pas canonicalisée : les
    /// chemins relatifs qui en découlent ne dépendent pas des symlinks).
    root: PathBuf,
    /// name → chemin relatif au projet (absent pour un metapackage).
    paths: BTreeMap<String, String>,
    /// Tag de composer/installers émulé, si le plugin est actif.
    pub installers_tag: Option<String>,
    /// Paquets installés (installed.json) à retirer : name → répertoire
    /// relatif à effacer (`vendor/<name>` ou la cible du plugin), après
    /// vérification que Composer recalculerait le même chemin aujourd'hui.
    removals: BTreeMap<String, String>,
}

/// Verdict d'`allow-plugins` pour un paquet, comme PluginManager en mode non
/// interactif.
#[derive(Debug, PartialEq, Eq)]
enum PluginVerdict {
    Allowed,
    /// Explicitement refusé : Composer saute le plugin avec un avertissement.
    Blocked,
    /// Aucune règle ne le couvre : Composer s'arrête en erreur.
    Unlisted,
}

fn absolutize(dir: &Path) -> PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(dir))
            .unwrap_or_else(|_| dir.to_path_buf())
    }
}

/// `BasePackage::packageNameToRegexp` : `{^<quote(pattern) avec * → .*>$}i`.
fn pattern_matches(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(pos) = rest.find(part) else {
                return false;
            };
            rest = &rest[pos + part.len()..];
        }
    }
    true
}

/// `Config::merge` pour `allow-plugins` : la valeur projet remplace la
/// globale sauf si les deux sont des objets (fusion, projet prioritaire).
fn merged_allow_plugins(project: Option<&Value>, global: Option<&Value>) -> Option<Value> {
    match (project, global) {
        (Some(Value::Object(p)), Some(Value::Object(g))) => {
            let mut m = g.clone();
            for (k, v) in p {
                m.insert(k.clone(), v.clone());
            }
            Some(Value::Object(m))
        }
        (Some(p), _) => Some(p.clone()),
        (None, Some(g)) => Some(g.clone()),
        (None, None) => None,
    }
}

/// `PluginManager::parseAllowedPlugins` + `isPluginAllowed` (non interactif).
fn plugin_verdict(allow: Option<&Value>, package: &str) -> PluginVerdict {
    match allow {
        Some(Value::Bool(true)) => PluginVerdict::Allowed,
        Some(Value::Bool(false)) => PluginVerdict::Blocked,
        Some(Value::Object(rules)) => {
            for (pattern, v) in rules {
                if pattern_matches(pattern, package) {
                    return if v == &Value::Bool(true) {
                        PluginVerdict::Allowed
                    } else {
                        PluginVerdict::Blocked
                    };
                }
            }
            PluginVerdict::Unlisted
        }
        _ => PluginVerdict::Unlisted,
    }
}

fn global_allow_plugins() -> Option<Value> {
    let path = crate::fetch::composer_home()?.join("config.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("config")?.get("allow-plugins").cloned()
}

/// Chemin relatif au projet d'un paquet géré par LibraryInstaller.
fn vendor_rel(name: &str, target_dir: Option<&str>) -> String {
    match target_dir {
        Some(t) => format!("vendor/{name}/{t}"),
        None => format!("vendor/{name}"),
    }
}

/// Décision pour un paquet (nom, type, extra) sous la configuration courante.
fn place(
    table: Option<&installers::Table>,
    root_extra: Option<&Value>,
    name: &str,
    package_type: &str,
    package_extra: Option<&Value>,
    target_dir: Option<&str>,
) -> Result<String, String> {
    let Some(table) = table else {
        return Ok(vendor_rel(name, target_dir));
    };
    match installers::placement(table, root_extra, name, package_type, package_extra) {
        Ok(Placement::Vendor) => Ok(vendor_rel(name, target_dir)),
        Ok(Placement::Custom(p)) => {
            if p.starts_with('/') || p.starts_with('\\') {
                return Err(format!(
                    "installers: {name} would install at an absolute path `{p}`"
                ));
            }
            let rel = normalize_path(&p);
            if rel.is_empty() || rel == "." {
                return Err(format!(
                    "installers: {name} would install at the project root"
                ));
            }
            if rel.starts_with("../") || rel == ".." {
                return Err(format!(
                    "installers: {name} would install outside the project (`{p}`)"
                ));
            }
            if rel == "vendor" || rel.starts_with("vendor/") {
                return Err(format!(
                    "installers: {name} targets `{p}` inside vendor/ (not emulated: use the default vendor layout)"
                ));
            }
            Ok(rel)
        }
        Err(e) => Err(format!("installers: {name} ({package_type}): {e}")),
    }
}

impl Layout {
    /// Tout dans vendor/ (sans plugin de layout) — pour les tests et les
    /// chemins de code qui n'ont pas de lock plugin-aware.
    pub fn vendor_only(project_dir: &Path, lock: &Lock, with_dev: bool) -> Layout {
        let mut paths = BTreeMap::new();
        for p in lock.wanted_packages(with_dev) {
            if !p.is_metapackage() {
                paths.insert(p.name().to_owned(), vendor_rel(p.name(), p.target_dir()));
            }
        }
        Layout {
            root: absolutize(project_dir),
            paths,
            installers_tag: None,
            removals: BTreeMap::new(),
        }
    }

    /// La passe complète : plugin, allow-plugins, chemins, cibles refusées,
    /// et plan de suppression pour les paquets d'installed.json disparus.
    pub fn resolve(
        project_dir: &Path,
        lock: &Lock,
        manifest: &Value,
        with_dev: bool,
        plugins_enabled: bool,
    ) -> Result<Layout, Vec<String>> {
        let root = absolutize(project_dir);
        let mut issues: Vec<String> = Vec::new();
        let wanted: Vec<&LockPackage> = lock.wanted_packages(with_dev).collect();
        let previous = installed_packages(&root);
        let has_state = root.join("vendor/composer/installed.json").is_file();

        // Le plugin est-il actif ? Composer le charge depuis installed.json
        // (PluginManager::loadInstalledPlugins) et l'installe en premier dans
        // la transaction ; vivace n'émule que les états où les deux vues
        // concordent — un plugin présent d'un seul côté (ajouté, retiré, ou
        // en require-dev avec --no-dev) est une transition laissée à Composer.
        let lock_plugin = wanted.iter().find(|p| p.name() == "composer/installers");
        let prev_plugin = previous
            .iter()
            .find(|p| p["name"].as_str() == Some("composer/installers"));
        let mut table: Option<&installers::Table> = None;
        let mut installers_tag = None;
        if plugins_enabled && (lock_plugin.is_some() || prev_plugin.is_some()) {
            let allow = merged_allow_plugins(
                manifest.get("config").and_then(|c| c.get("allow-plugins")),
                global_allow_plugins().as_ref(),
            );
            match plugin_verdict(allow.as_ref(), "composer/installers") {
                PluginVerdict::Allowed => {
                    let version = lock_plugin
                        .map(|p| p.version().to_owned())
                        .or_else(|| {
                            prev_plugin.and_then(|p| p["version"].as_str().map(str::to_owned))
                        })
                        .unwrap_or_default();
                    let Some(t) = installers::table_for(&version) else {
                        return Err(vec![format!(
                            "composer/installers {version} is not a ported version (ported: {})",
                            installers::ported_versions().collect::<Vec<_>>().join(", ")
                        )]);
                    };
                    if has_state && lock_plugin.is_some() != prev_plugin.is_some() {
                        let root_extra = manifest.get("extra");
                        let taken = |name: &str, ty: &str, extra: Option<&Value>| {
                            !matches!(
                                installers::placement(t, root_extra, name, ty, extra),
                                Ok(Placement::Vendor)
                            )
                        };
                        let any_taken = wanted
                            .iter()
                            .any(|p| taken(p.name(), p.package_type(), p.raw.get("extra")))
                            || previous.iter().any(|p| {
                                taken(
                                    p["name"].as_str().unwrap_or(""),
                                    p["type"].as_str().unwrap_or("library"),
                                    p.get("extra"),
                                )
                            });
                        if any_taken {
                            let how = if lock_plugin.is_some() {
                                "added to"
                            } else {
                                "removed from"
                            };
                            return Err(vec![format!(
                                "composer/installers is being {how} an existing install (installed.json and composer.lock disagree): let Composer handle this transition"
                            )]);
                        }
                    }
                    if lock_plugin.is_some() {
                        installers_tag = Some(t.tag.clone());
                        table = Some(t);
                    }
                }
                PluginVerdict::Blocked => {} // Composer l'ignore : tout dans vendor/
                PluginVerdict::Unlisted => {
                    return Err(vec![
                        "composer/installers is a plugin not covered by config.allow-plugins (Composer would refuse to run it)"
                            .to_owned(),
                    ]);
                }
            }
        }

        let root_extra = manifest.get("extra");
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        for p in &wanted {
            if p.is_metapackage() {
                continue;
            }
            match place(
                table,
                root_extra,
                p.name(),
                p.package_type(),
                p.raw.get("extra"),
                p.target_dir(),
            ) {
                Ok(rel) => {
                    paths.insert(p.name().to_owned(), rel);
                }
                Err(e) => issues.push(e),
            }
        }

        // Cibles en conflit : deux paquets au même endroit, ou l'un sous l'autre.
        if table.is_some() {
            let mut by_path: BTreeMap<&str, &str> = BTreeMap::new();
            for (name, rel) in &paths {
                if let Some(other) = by_path.insert(rel.as_str(), name.as_str()) {
                    issues.push(format!(
                        "installers: {name} and {other} would both install at `{rel}`"
                    ));
                }
            }
            let customs: Vec<(&str, &str)> = paths
                .iter()
                .filter(|(_, rel)| !rel.starts_with("vendor/"))
                .map(|(n, r)| (n.as_str(), r.as_str()))
                .collect();
            for (name, rel) in &customs {
                for (other, other_rel) in &paths {
                    if other.as_str() != *name && other_rel.starts_with(&format!("{rel}/")) {
                        issues.push(format!(
                            "installers: {name} at `{rel}` would contain {other} at `{other_rel}`"
                        ));
                    }
                }
            }
        }

        // Plan de suppression : Composer recalcule le chemin d'un paquet retiré
        // avec la configuration courante ; on n'efface que si ce chemin est
        // celui où le paquet a été posé (installed.json), sinon fallback.
        let mut removals = BTreeMap::new();
        let wanted_names: std::collections::BTreeSet<&str> =
            wanted.iter().map(|p| p.name()).collect();
        let vendor_composer =
            normalize_path(&format!("{}/vendor/composer", root.to_string_lossy()));
        let root_norm = normalize_path(&root.to_string_lossy());
        for prev in &previous {
            let name = prev["name"].as_str().unwrap_or("");
            if name.is_empty() || wanted_names.contains(name) {
                continue;
            }
            let Some(old_ip) = prev.get("install-path").and_then(Value::as_str) else {
                continue; // metapackage
            };
            let old_abs = if old_ip.starts_with('/') {
                normalize_path(old_ip)
            } else {
                normalize_path(&format!("{vendor_composer}/{old_ip}"))
            };
            let Some(old_rel) = old_abs
                .strip_prefix(&format!("{root_norm}/"))
                .filter(|r| !r.is_empty())
            else {
                issues.push(format!(
                    "installed package {name} lives outside the project (`{old_ip}`): not removing it"
                ));
                continue;
            };
            let expected = place(
                table,
                root_extra,
                name,
                prev.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("library"),
                prev.get("extra"),
                prev.get("target-dir")
                    .and_then(Value::as_str)
                    .map(|t| t.trim_matches('/'))
                    .filter(|t| !t.is_empty()),
            );
            match expected {
                Ok(rel) if rel == old_rel => {
                    // LibraryInstaller::removeCode efface getPackageBasePath :
                    // vendor/<name> sans le target-dir.
                    let dir = if rel.starts_with("vendor/") {
                        format!("vendor/{name}")
                    } else {
                        rel
                    };
                    removals.insert(name.to_owned(), dir);
                }
                Ok(rel) => issues.push(format!(
                    "installed package {name} is at `{old_rel}` but the current layout puts it at `{rel}`: let Composer handle this removal"
                )),
                Err(e) => issues.push(format!("removal of {name}: {e}")),
            }
        }

        if issues.is_empty() {
            Ok(Layout {
                root,
                paths,
                installers_tag,
                removals,
            })
        } else {
            Err(issues)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Chemin relatif au projet (None : metapackage ou paquet inconnu).
    pub fn rel(&self, name: &str) -> Option<&str> {
        self.paths.get(name).map(String::as_str)
    }

    /// Chemin absolu d'installation.
    pub fn abs(&self, name: &str) -> Option<PathBuf> {
        self.rel(name).map(|r| self.root.join(r))
    }

    /// Racine à vider avant de poser le paquet : `vendor/<name>` (target-dir
    /// compris) pour LibraryInstaller, la cible elle-même sinon.
    pub fn package_root(&self, name: &str) -> Option<PathBuf> {
        let rel = self.rel(name)?;
        Some(if rel.starts_with("vendor/") {
            self.root.join("vendor").join(name)
        } else {
            self.root.join(rel)
        })
    }

    /// `install-path` d'installed.json / installed.php : relatif à
    /// vendor/composer (`findShortestPath($repoDir, $path, true)`).
    pub fn install_path(&self, name: &str) -> Option<String> {
        let rel = self.rel(name)?;
        let root = self.root.to_string_lossy();
        Some(find_shortest_path(
            &format!("{root}/vendor/composer"),
            &format!("{root}/{rel}"),
            true,
        ))
    }

    /// Paquets d'installed.json à retirer, avec leur chemin absolu.
    pub fn removals(&self) -> impl Iterator<Item = (&str, PathBuf)> {
        self.removals
            .iter()
            .map(|(n, rel)| (n.as_str(), self.root.join(rel)))
    }
}

fn installed_packages(root: &Path) -> Vec<Value> {
    let path = root.join("vendor/composer/installed.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v.get("packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lock_with(packages: Value) -> Lock {
        Lock::parse(
            &json!({"packages": packages, "packages-dev": [], "plugin-api-version": "2.6.0"})
                .to_string(),
        )
        .expect("lock")
    }

    fn pkg(name: &str, ty: &str, version: &str) -> Value {
        json!({"name": name, "version": version, "type": ty,
               "dist": {"type": "zip", "url": "https://x/y.zip", "reference": "r"}})
    }

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    #[test]
    fn allow_plugins_patterns_and_merge() {
        assert!(pattern_matches("composer/*", "composer/installers"));
        assert!(pattern_matches(
            "Composer/Installers",
            "composer/installers"
        ));
        assert!(pattern_matches("*", "anything/here"));
        assert!(!pattern_matches("composer/*", "other/installers"));
        assert!(pattern_matches("*/installers", "composer/installers"));
        let rules = json!({"composer/*": false, "composer/installers": true});
        // Première règle qui matche : `composer/*` → refusé.
        assert_eq!(
            plugin_verdict(Some(&rules), "composer/installers"),
            PluginVerdict::Blocked
        );
        assert_eq!(
            plugin_verdict(Some(&json!(true)), "x/y"),
            PluginVerdict::Allowed
        );
        assert_eq!(
            plugin_verdict(Some(&json!({})), "x/y"),
            PluginVerdict::Unlisted
        );
        assert_eq!(plugin_verdict(None, "x/y"), PluginVerdict::Unlisted);
        let merged = merged_allow_plugins(
            Some(&json!({"a/b": false})),
            Some(&json!({"a/b": true, "c/d": true})),
        );
        assert_eq!(merged, Some(json!({"a/b": false, "c/d": true})));
    }

    #[test]
    fn without_plugin_everything_goes_to_vendor() {
        let lock = lock_with(json!([
            pkg("a/b", "wordpress-plugin", "1.0.0"),
            pkg("a/meta", "metapackage", "1.0.0")
        ]));
        let manifest =
            json!({"extra": {"installer-paths": {"web/{$name}": ["type:wordpress-plugin"]}}});
        let l = Layout::resolve(&root(), &lock, &manifest, true, true).expect("layout");
        assert_eq!(l.rel("a/b"), Some("vendor/a/b"));
        assert_eq!(l.rel("a/meta"), None);
        assert_eq!(l.install_path("a/b").as_deref(), Some("../a/b"));
        assert!(l.installers_tag.is_none());
    }

    #[test]
    fn plugin_allowed_places_packages_and_blocked_keeps_vendor() {
        let lock = lock_with(json!([
            pkg("composer/installers", "composer-plugin", "v2.3.0"),
            pkg("wpackagist-plugin/akismet", "wordpress-plugin", "5.3"),
            pkg(
                "wpackagist-theme/twentytwentyfour",
                "wordpress-theme",
                "1.0"
            ),
            pkg("monolog/monolog", "library", "3.0.0"),
            pkg("composer/pcre", "library", "3.0.0"),
        ]));
        let manifest = json!({
            "config": {"allow-plugins": {"composer/installers": true}},
            "extra": {"installer-paths": {"web/app/plugins/{$name}/": ["type:wordpress-plugin"]}}
        });
        let l = Layout::resolve(&root(), &lock, &manifest, true, true).expect("layout");
        assert_eq!(l.installers_tag.as_deref(), Some("v2.3.0"));
        assert_eq!(
            l.rel("wpackagist-plugin/akismet"),
            Some("web/app/plugins/akismet")
        );
        assert_eq!(
            l.rel("wpackagist-theme/twentytwentyfour"),
            Some("wp-content/themes/twentytwentyfour")
        );
        assert_eq!(l.rel("monolog/monolog"), Some("vendor/monolog/monolog"));
        assert_eq!(
            l.rel("composer/installers"),
            Some("vendor/composer/installers")
        );
        assert_eq!(
            l.install_path("wpackagist-plugin/akismet").as_deref(),
            Some("../../web/app/plugins/akismet")
        );
        assert_eq!(l.install_path("composer/pcre").as_deref(), Some("./pcre"));
        assert_eq!(
            l.install_path("monolog/monolog").as_deref(),
            Some("../monolog/monolog")
        );
        assert_eq!(
            l.package_root("wpackagist-plugin/akismet"),
            Some(PathBuf::from("/proj/web/app/plugins/akismet"))
        );

        let blocked = json!({"config": {"allow-plugins": {"composer/installers": false}}});
        let l = Layout::resolve(&root(), &lock, &blocked, true, true).expect("layout");
        assert_eq!(
            l.rel("wpackagist-plugin/akismet"),
            Some("vendor/wpackagist-plugin/akismet")
        );

        let unlisted = json!({"config": {"allow-plugins": {"other/x": true}}});
        let err = Layout::resolve(&root(), &lock, &unlisted, true, true).expect_err("fallback");
        assert!(err[0].contains("allow-plugins"), "{err:?}");

        let missing = json!({});
        assert!(Layout::resolve(&root(), &lock, &missing, true, true).is_err());
    }

    #[test]
    fn unported_version_custom_framework_and_dangerous_targets_are_issues() {
        let manifest = json!({"config": {"allow-plugins": true}});
        let old = lock_with(json!([
            pkg("composer/installers", "composer-plugin", "v1.12.0"),
            pkg("a/b", "drupal-module", "1.0")
        ]));
        let err = Layout::resolve(&root(), &old, &manifest, true, true).expect_err("v1");
        assert!(err[0].contains("not a ported version"), "{err:?}");

        let cake = lock_with(json!([
            pkg("composer/installers", "composer-plugin", "v2.2.0"),
            pkg("a/b", "cakephp-plugin", "1.0")
        ]));
        let err = Layout::resolve(&root(), &cake, &manifest, true, true).expect_err("cake");
        assert!(err[0].contains("custom path logic"), "{err:?}");

        let lock = lock_with(json!([
            pkg("composer/installers", "composer-plugin", "v2.3.0"),
            pkg("a/b", "drupal-module", "1.0"),
            pkg("a/c", "drupal-module", "1.0")
        ]));
        for (paths, needle) in [
            (json!({"{$name}/../../x": ["a/b"]}), "outside the project"),
            (json!({"vendor/{$name}": ["a/b"]}), "inside vendor/"),
            (json!({"/abs/{$name}": ["a/b"]}), "absolute path"),
            (json!({"same/": ["a/b", "a/c"]}), "would both install"),
            (json!({"modules/": ["a/b"]}), "would contain"),
        ] {
            let m = json!({"config": {"allow-plugins": true}, "extra": {"installer-paths": paths}});
            let err = Layout::resolve(&root(), &lock, &m, true, true).expect_err(needle);
            assert!(err.iter().any(|e| e.contains(needle)), "{needle}: {err:?}");
        }
        // Le projet racine : template vide.
        let m =
            json!({"config": {"allow-plugins": true}, "extra": {"installer-paths": {"": ["a/b"]}}});
        let err = Layout::resolve(&root(), &lock, &m, true, true).expect_err("root");
        assert!(err.iter().any(|e| e.contains("project root")), "{err:?}");
    }

    #[test]
    fn removals_follow_installed_json_when_paths_agree() {
        let dir = tempfile::tempdir().expect("tmp");
        let vc = dir.path().join("vendor/composer");
        std::fs::create_dir_all(&vc).expect("mkdir");
        let plugin_entry = json!({"name": "composer/installers", "version": "v2.3.0", "type": "composer-plugin", "install-path": "./installers"});
        std::fs::write(
            vc.join("installed.json"),
            json!({"packages": [
                plugin_entry,
                {"name": "gone/lib", "version": "1.0", "type": "library", "install-path": "../gone/lib"},
                {"name": "gone/legacy", "version": "1.0", "type": "library", "target-dir": "Acme/Legacy", "install-path": "../gone/legacy/Acme/Legacy"},
                {"name": "gone/plugin", "version": "1.0", "type": "wordpress-plugin", "install-path": "../../wp-content/plugins/plugin"},
                {"name": "moved/plugin", "version": "1.0", "type": "wordpress-plugin", "install-path": "../../old/plugin"},
                {"name": "gone/meta", "version": "1.0", "type": "metapackage", "install-path": null}
            ], "dev": true, "dev-package-names": []})
            .to_string(),
        )
        .expect("write");
        let lock = lock_with(json!([pkg(
            "composer/installers",
            "composer-plugin",
            "v2.3.0"
        )]));
        let manifest = json!({"config": {"allow-plugins": true}});
        let err = Layout::resolve(dir.path(), &lock, &manifest, true, true).expect_err("moved");
        assert!(
            err.iter()
                .any(|e| e.contains("moved/plugin") && e.contains("old/plugin")),
            "{err:?}"
        );

        // Sans le paquet déplacé, le plan est accepté ; un paquet à target-dir
        // est effacé à vendor/<name> (getPackageBasePath), pas au sous-chemin.
        std::fs::write(
            vc.join("installed.json"),
            json!({"packages": [
                plugin_entry,
                {"name": "gone/lib", "version": "1.0", "type": "library", "install-path": "../gone/lib"},
                {"name": "gone/legacy", "version": "1.0", "type": "library", "target-dir": "Acme/Legacy", "install-path": "../gone/legacy/Acme/Legacy"},
                {"name": "gone/plugin", "version": "1.0", "type": "wordpress-plugin", "install-path": "../../wp-content/plugins/plugin"}
            ], "dev": true, "dev-package-names": []})
            .to_string(),
        )
        .expect("write");
        let l = Layout::resolve(dir.path(), &lock, &manifest, true, true).expect("layout");
        let removals: Vec<(String, PathBuf)> =
            l.removals().map(|(n, p)| (n.to_owned(), p)).collect();
        assert_eq!(
            removals,
            vec![
                (
                    "gone/legacy".to_owned(),
                    l.root().join("vendor/gone/legacy")
                ),
                ("gone/lib".to_owned(), l.root().join("vendor/gone/lib")),
                (
                    "gone/plugin".to_owned(),
                    l.root().join("wp-content/plugins/plugin")
                ),
            ]
        );
    }

    #[test]
    fn plugin_present_on_one_side_only_is_a_transition_for_composer() {
        let dir = tempfile::tempdir().expect("tmp");
        let vc = dir.path().join("vendor/composer");
        std::fs::create_dir_all(&vc).expect("mkdir");
        let manifest = json!({"config": {"allow-plugins": true}});
        let with_plugin = lock_with(json!([
            pkg("composer/installers", "composer-plugin", "v2.3.0"),
            pkg("a/wp", "wordpress-plugin", "1.0"),
        ]));
        let without_plugin = lock_with(json!([pkg("a/wp", "wordpress-plugin", "1.0")]));
        let libs_only = lock_with(json!([
            pkg("composer/installers", "composer-plugin", "v2.3.0"),
            pkg("a/lib", "library", "1.0"),
        ]));

        // Ajout : installed.json sans le plugin, lock avec, et un paquet concerné.
        std::fs::write(
            vc.join("installed.json"),
            json!({"packages": [{"name": "a/wp", "version": "1.0", "type": "wordpress-plugin", "install-path": "../a/wp"}], "dev": true, "dev-package-names": []}).to_string(),
        )
        .expect("write");
        let err =
            Layout::resolve(dir.path(), &with_plugin, &manifest, true, true).expect_err("added");
        assert!(err[0].contains("added to an existing install"), "{err:?}");
        // Même ajout sans paquet d'un type pris par le plugin : rien à transiter.
        std::fs::write(
            vc.join("installed.json"),
            json!({"packages": [{"name": "a/lib", "version": "1.0", "type": "library", "install-path": "../a/lib"}], "dev": true, "dev-package-names": []}).to_string(),
        )
        .expect("write");
        assert!(Layout::resolve(dir.path(), &libs_only, &manifest, true, true).is_ok());

        // Retrait : installed.json avec le plugin et un paquet hors vendor/, lock sans.
        std::fs::write(
            vc.join("installed.json"),
            json!({"packages": [
                {"name": "composer/installers", "version": "v2.3.0", "type": "composer-plugin", "install-path": "./installers"},
                {"name": "a/wp", "version": "1.0", "type": "wordpress-plugin", "install-path": "../../wp-content/plugins/wp"}
            ], "dev": true, "dev-package-names": []}).to_string(),
        )
        .expect("write");
        let err = Layout::resolve(dir.path(), &without_plugin, &manifest, true, true)
            .expect_err("removed");
        assert!(
            err[0].contains("removed from an existing install"),
            "{err:?}"
        );

        // --no-plugins : Composer ignore le plugin des deux côtés, tout en vendor/.
        let l =
            Layout::resolve(dir.path(), &with_plugin, &manifest, true, false).expect("no-plugins");
        assert_eq!(l.rel("a/wp"), Some("vendor/a/wp"));
        assert!(l.installers_tag.is_none());
        // Pas d'état sur disque : le lock décide (install frais).
        std::fs::remove_file(vc.join("installed.json")).expect("rm");
        let l = Layout::resolve(dir.path(), &with_plugin, &manifest, true, true).expect("fresh");
        assert_eq!(l.rel("a/wp"), Some("wp-content/plugins/wp"));
    }
}
