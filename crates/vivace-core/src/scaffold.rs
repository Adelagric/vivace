//! Port de `drupal/core-composer-scaffold` (docs/reference/drupal-scaffold,
//! source telle qu'installée en 11.4.6) : ce que le plugin fait pendant
//! `composer install` — même avec `--no-scripts`, qui ne coupe que les
//! scripts du composer.json racine.
//!
//! Trois moments, reproduits ici :
//! - PRE_AUTOLOAD_DUMP (`Plugin::preAutoloadDump`) : entrées de classmap
//!   ajoutées à la racine + `vendor/drupal/DrupalInstalled.php` → [`pre_autoload_dump`] ;
//! - POST_INSTALL_CMD (`Handler::scaffold`) : copie/concaténation des
//!   fichiers déclarés par `extra.drupal-scaffold.file-mapping` des paquets
//!   autorisés, `web-root/autoload.php` (+ `autoload_runtime.php`), gestion
//!   des `.gitignore` → [`plan`] puis [`Plan::apply`] ;
//! - le tout sous un [`Profile`] choisi par l'empreinte de la source du
//!   plugin (assets/scaffold-fingerprints.json) : le cœur est identique de
//!   10.3 à 11.4, seules trois fonctionnalités s'ajoutent au fil des versions.
//!
//! Tout ce que Composer ferait et que vivace ne peut pas reproduire à
//! l'identique (ou qui effacerait un répertoire) est refusé au moment du
//! plan, avant toute écriture : la CLI délègue alors à Composer.

use crate::pathutil::{find_shortest_path, normalize_path};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PLUGIN: &str = "drupal/core-composer-scaffold";

const FINGERPRINTS: &str = include_str!("../assets/scaffold-fingerprints.json");
const AUTOLOAD_TPL: &str = include_str!("../assets/scaffold/autoload.php.tpl");
const AUTOLOAD_RUNTIME_TPL: &str = include_str!("../assets/scaffold/autoload_runtime.php.tpl");
const DRUPAL_INSTALLED_TPL: &str = include_str!("../assets/scaffold/DrupalInstalled.php.tpl");

/// Fonctionnalités du plugin selon sa version (voir l'asset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// `Plugin::preAutoloadDump` : classmap + DrupalInstalled.php (≥ 11.3.0).
    pub pre_autoload_dump: bool,
    /// Hash de DrupalInstalled sur les paquets triés (≥ 11.3.4) ; non trié,
    /// l'ordre est celui de la transaction Composer : non reproductible.
    pub sorted_hash: bool,
    /// `web-root/autoload_runtime.php` (≥ 11.4.0).
    pub autoload_runtime: bool,
}

/// sha256 de « <chemin relatif>\n<contenu> » de chaque `*.php` hors tests/,
/// dans l'ordre trié des chemins (tools/plugin-fingerprint.sh).
pub fn fingerprint(plugin_dir: &Path) -> std::io::Result<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_php(plugin_dir, plugin_dir, &mut files)?;
    files.sort();
    let mut h = Sha256::new();
    for f in &files {
        let rel = f.strip_prefix(plugin_dir).unwrap_or(f);
        h.update(rel.to_string_lossy().as_bytes());
        h.update(b"\n");
        h.update(std::fs::read(f)?);
    }
    Ok(format!("{:x}", h.finalize()))
}

fn collect_php(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        let name = entry.file_name();
        if p.is_dir() {
            let rel_first = p
                .strip_prefix(root)
                .ok()
                .and_then(|r| r.components().next());
            let is_tests = rel_first.is_some_and(|c| {
                let s = c.as_os_str();
                s == "tests" || s == "Tests"
            });
            if !is_tests {
                collect_php(root, &p, out)?;
            }
        } else if name.to_string_lossy().ends_with(".php") {
            out.push(p);
        }
    }
    Ok(())
}

/// Profil correspondant à une empreinte, None si la source n'est pas portée.
pub fn profile_for(fingerprint: &str) -> Option<Profile> {
    let table: Value = serde_json::from_str(FINGERPRINTS).ok()?;
    let level = table
        .get("sources")?
        .get(fingerprint)?
        .get("features")?
        .as_u64()?;
    let profile = Profile {
        pre_autoload_dump: level >= 1,
        sorted_hash: level >= 2,
        autoload_runtime: level >= 3,
    };
    // Niveau 1 : hash dans l'ordre de la transaction Composer → non reproductible.
    if profile.pre_autoload_dump && !profile.sorted_hash {
        return None;
    }
    Some(profile)
}

/// Versions du plugin dont l'empreinte est connue (pour les messages).
pub fn known_versions(fingerprint: &str) -> Vec<String> {
    let table: Value = serde_json::from_str(FINGERPRINTS).unwrap_or(Value::Null);
    table
        .get("sources")
        .and_then(|s| s.get(fingerprint))
        .and_then(|e| e.get("versions"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Options (ScaffoldOptions / ManageOptions)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Options {
    allowed_packages: Vec<String>,
    /// Ordre d'insertion conservé (tableau PHP).
    locations: Vec<(String, String)>,
    symlink: bool,
    file_mapping: Vec<(String, Value)>,
    gitignore: Option<bool>,
}

impl Options {
    fn from_extra(extra: Option<&Value>) -> Options {
        let o = extra
            .and_then(|e| e.get("drupal-scaffold"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let list = |k: &str| -> Vec<String> {
            match o.get(k) {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                Some(Value::Object(m)) => m
                    .values()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                _ => Vec::new(),
            }
        };
        let mut locations: Vec<(String, String)> = match o.get("locations") {
            Some(Value::Object(m)) => m
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect(),
            _ => Vec::new(),
        };
        // `$this->options['locations'] += ['project-root' => '.', 'web-root' => '.']`
        for (k, v) in [("project-root", "."), ("web-root", ".")] {
            if !locations.iter().any(|(n, _)| n == k) {
                locations.push((k.to_owned(), v.to_owned()));
            }
        }
        let file_mapping: Vec<(String, Value)> = match o.get("file-mapping") {
            Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => Vec::new(),
        };
        Options {
            allowed_packages: list("allowed-packages"),
            locations,
            symlink: matches!(o.get("symlink"), Some(v) if php_truthy(v)),
            file_mapping,
            // `isset` : null = absent.
            gitignore: o.get("gitignore").filter(|v| !v.is_null()).map(php_truthy),
        }
    }

    fn location(&self, name: &str) -> Option<&str> {
        self.locations
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// `empty()` inversé de PHP sur une valeur JSON.
fn php_truthy(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !(s.is_empty() || s == "0"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `Interpolator` : `[token]` (`[a-zA-Z0-9._-]+`) remplacé par la donnée,
/// token inconnu → `default`.
fn interpolate(message: &str, data: &[(String, String)], default: &str) -> String {
    let bytes = message.as_bytes();
    let mut out = String::with_capacity(message.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'.' | b'_' | b'-'))
            {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b']' {
                let key = &message[i + 1..j];
                match data.iter().find(|(k, _)| k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str(default),
                }
                i = j + 1;
                continue;
            }
        }
        let ch = message[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

// ---------------------------------------------------------------------------
// Paquets et opérations
// ---------------------------------------------------------------------------

/// Un paquet installé vu par le scaffold : nom, répertoire contenant sa
/// source (entrée de store ou chemin d'installation) et son `extra`.
#[derive(Debug, Clone)]
pub struct ScaffoldPackage {
    pub name: String,
    pub dir: PathBuf,
    pub extra: Option<Value>,
}

#[derive(Debug, Clone)]
enum Op {
    Skip,
    Replace {
        source: PathBuf,
        overwrite: bool,
    },
    Append {
        prepend: Option<PathBuf>,
        append: Option<PathBuf>,
        default: Option<PathBuf>,
        force_append: bool,
        /// `managed` : faux quand l'op arrive sur une destination nouvelle.
        managed: bool,
        /// `originalContents` (contenu de l'op précédente ou du fichier).
        original: Option<Vec<u8>>,
    },
}

impl Op {
    /// `AbstractOperation::contents()`.
    fn contents(&self) -> std::io::Result<Vec<u8>> {
        Ok(match self {
            Op::Skip => Vec::new(),
            Op::Replace { source, .. } => std::fs::read(source)?,
            Op::Append {
                prepend,
                append,
                default,
                original,
                ..
            } => {
                let mut out = Vec::new();
                if let Some(p) = prepend {
                    out.extend(std::fs::read(p)?);
                    out.push(b'\n');
                }
                let mut orig = original.clone().unwrap_or_default();
                // `empty($original_contents)` : "" ou "0".
                if (orig.is_empty() || orig == b"0") && default.is_some() {
                    if let Some(d) = default {
                        orig = std::fs::read(d)?;
                    }
                }
                out.extend(orig);
                if let Some(a) = append {
                    out.push(b'\n');
                    out.extend(std::fs::read(a)?);
                }
                out
            }
        })
    }
}

/// Une destination : chemin complet et paquet qui la fournit (la clé brute,
/// non interpolée, est portée par les listes qui la contiennent).
#[derive(Debug, Clone)]
struct Dest {
    full: PathBuf,
    package: String,
}

/// Fichiers d'un projet : (clé brute, destination, opération).
type ProjectFiles = Vec<(String, Dest, Op)>;

/// Résultat d'une opération (`ScaffoldResult`).
#[derive(Debug, Clone)]
struct Outcome {
    full: PathBuf,
    managed: bool,
}

/// Une écriture à faire par `apply`.
#[derive(Debug, Clone)]
pub struct Write {
    pub path: PathBuf,
    pub contents: Vec<u8>,
    /// `ReplaceOp` : remove + création (répertoire rendu inscriptible au
    /// besoin, perms d'origine du fichier restaurées) ; sinon écriture en
    /// place comme `file_put_contents`.
    pub replace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitIgnoreMode {
    /// Option `gitignore` explicite.
    Forced(bool),
    /// Décidé à l'application : dépôt git et `vendor` ignoré.
    Auto,
}

/// Ce que `Handler::scaffold()` écrirait, calculé sans toucher au disque
/// (hors création des répertoires de `locations`, comme le plugin).
#[derive(Debug, Clone)]
pub struct Plan {
    root: PathBuf,
    writes: Vec<Write>,
    /// Résultats dans l'ordre PHP (`$results[rel]`, remplacement en place).
    results: Vec<(String, Outcome)>,
    gitignore: GitIgnoreMode,
}

impl Plan {
    /// Aucun paquet autorisé : le plugin ne fait rien.
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.results.is_empty()
    }

    pub fn writes(&self) -> &[Write] {
        &self.writes
    }
}

fn abs_from_cwd(root: &Path, p: &str) -> PathBuf {
    if p.starts_with('/') {
        PathBuf::from(p)
    } else {
        root.join(p)
    }
}

/// `realpath()` PHP : None si le chemin n'existe pas.
fn realpath(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

fn under(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// Calcule le plan du scaffold. `root` doit être la racine canonique du
/// projet (`getcwd()` physique) ; `packages` = paquets installés (ce que le
/// dépôt local contiendrait après la transaction) avec leur répertoire source.
pub fn plan(
    profile: Profile,
    root: &Path,
    root_name: &str,
    root_extra: Option<&Value>,
    packages: &[ScaffoldPackage],
) -> Result<Plan, String> {
    let root_opts = Options::from_extra(root_extra);
    if root_opts.symlink {
        return Err(format!(
            "{PLUGIN}: extra.drupal-scaffold.symlink is not emulated"
        ));
    }

    // AllowedPackages::getAllowedPackages (DFS pré-ordre, premier vu conservé).
    let find = |name: &str| -> Option<&ScaffoldPackage> {
        packages.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    };
    let mut allowed: Vec<(String, Option<&ScaffoldPackage>)> = Vec::new();
    fn recurse<'a>(
        names: &[String],
        find: &dyn Fn(&str) -> Option<&'a ScaffoldPackage>,
        allowed: &mut Vec<(String, Option<&'a ScaffoldPackage>)>,
    ) {
        for name in names {
            if let Some(p) = find(name) {
                if allowed.iter().any(|(n, _)| n == name) {
                    continue;
                }
                allowed.push((name.clone(), Some(p)));
                let opts = Options::from_extra(p.extra.as_ref());
                recurse(&opts.allowed_packages, find, allowed);
            }
        }
    }
    let mut top: Vec<String> = vec![
        "drupal/legacy-scaffold-assets".to_owned(),
        "drupal/core".to_owned(),
    ];
    top.extend(root_opts.allowed_packages.iter().cloned());
    recurse(&top, &find, &mut allowed);
    if !root_opts.file_mapping.is_empty() {
        allowed.retain(|(n, _)| n != root_name);
        allowed.push((root_name.to_owned(), None));
    }
    if allowed.is_empty() {
        return Ok(Plan {
            root: root.to_path_buf(),
            writes: Vec::new(),
            results: Vec::new(),
            gitignore: GitIgnoreMode::Auto,
        });
    }

    // ManageOptions::ensureLocations : locations + web_root, mkdir, realpath.
    let mut locations: Vec<(String, String)> = root_opts.locations.clone();
    if !locations.iter().any(|(k, _)| k == "web_root") {
        locations.push(("web_root".to_owned(), "./".to_owned()));
    }
    let mut location_real: Vec<(String, String)> = Vec::new();
    for (k, v) in &locations {
        let dir = abs_from_cwd(root, v);
        let norm = normalize_path(&dir.to_string_lossy());
        if !under(Path::new(&norm), root) {
            return Err(format!(
                "{PLUGIN}: location `{k}` = `{v}` resolves outside the project"
            ));
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("{PLUGIN}: cannot create location `{v}`: {e}"))?;
        let real = realpath(&dir).ok_or_else(|| format!("{PLUGIN}: cannot realpath `{v}`"))?;
        location_real.push((k.clone(), real.to_string_lossy().into_owned()));
    }
    let allowed_roots: Vec<PathBuf> = std::iter::once(root.to_path_buf())
        .chain(location_real.iter().map(|(_, r)| PathBuf::from(r)))
        .collect();

    // Handler::getFileMappingsFromPackages + ScaffoldFileCollection.
    let mut files: BTreeMap<String, (Dest, Op)> = BTreeMap::new();
    let mut by_project: Vec<(String, ProjectFiles)> = Vec::new();
    for (name, pkg) in &allowed {
        let (extra, dir) = match pkg {
            Some(p) => (p.extra.as_ref(), p.dir.clone()),
            None => (root_extra, root.to_path_buf()),
        };
        let opts = Options::from_extra(extra);
        if opts.file_mapping.is_empty() {
            continue;
        }
        let pkg_real = realpath(&dir).unwrap_or(dir.clone());
        let mut entries: ProjectFiles = Vec::new();
        for (dest_rel, value) in &opts.file_mapping {
            let mut op = make_op(name, &dir, &pkg_real, dest_rel, value)?;
            let interpolated = interpolate(dest_rel, &location_real, "");
            let full = abs_from_cwd(root, &interpolated);
            let full_norm = PathBuf::from(normalize_path(&full.to_string_lossy()));
            if !allowed_roots.iter().any(|r| under(&full_norm, r)) {
                return Err(format!(
                    "{PLUGIN}: {name} would scaffold `{dest_rel}` outside the project (`{}`)",
                    full_norm.display()
                ));
            }
            match std::fs::symlink_metadata(&full_norm) {
                Ok(m) if !m.file_type().is_file() => {
                    return Err(format!(
                        "{PLUGIN}: destination `{dest_rel}` exists and is not a regular file (Composer would remove it recursively)"
                    ));
                }
                _ => {}
            }
            if let Some(parent) = full_norm.parent() {
                if parent.exists() && !parent.is_dir() {
                    return Err(format!(
                        "{PLUGIN}: parent of `{dest_rel}` is not a directory"
                    ));
                }
            }
            let dest = Dest {
                full: full_norm,
                package: name.clone(),
            };
            if let Some((prev_dest, prev_op)) = files.get(dest_rel) {
                // scaffoldOverExistingTarget : l'op précédente devient Skip
                // (dans son projet), la nouvelle reçoit son contenu.
                if let Op::Append { original, .. } = &mut op {
                    *original = Some(prev_op.contents().map_err(|e| io_err(name, e))?);
                }
                let prev_pkg = prev_dest.package.clone();
                if let Some((_, list)) = by_project.iter_mut().find(|(p, _)| *p == prev_pkg) {
                    for (rel, _, o) in list.iter_mut() {
                        if rel == dest_rel {
                            *o = Op::Skip;
                        }
                    }
                }
            } else {
                op = scaffold_at_new_location(op, &dest, name)?;
            }
            files.insert(dest_rel.clone(), (dest.clone(), op.clone()));
            entries.push((dest_rel.clone(), dest, op));
        }
        by_project.push((name.clone(), entries));
    }

    // checkUnchanged + filterFiles.
    let mut unchanged: Vec<String> = Vec::new();
    for (_, list) in &by_project {
        for (rel, dest, op) in list {
            let has_changed = match std::fs::read(&dest.full) {
                Ok(current) => op.contents().map_err(|e| io_err(&dest.package, e))? != current,
                Err(_) => true,
            };
            if !has_changed {
                unchanged.push(rel.clone());
            }
        }
    }
    let mut projects: Vec<(String, ProjectFiles)> = Vec::new();
    for (project, list) in by_project {
        let kept: ProjectFiles = list
            .into_iter()
            .filter(|(rel, _, _)| !unchanged.contains(rel))
            .collect();
        // `!empty($contents)` : "" et "0" sont vides.
        let has_content = kept
            .iter()
            .any(|(_, _, op)| matches!(op.contents(), Ok(c) if !(c.is_empty() || c == b"0")));
        if has_content {
            projects.push((project, kept));
        }
    }

    // processScaffoldFiles.
    let mut writes: Vec<Write> = Vec::new();
    let mut results: Vec<(String, Outcome)> = Vec::new();
    let mut push_result = |rel: &str, outcome: Outcome| {
        if let Some((_, o)) = results.iter_mut().find(|(r, _)| r == rel) {
            *o = outcome;
        } else {
            results.push((rel.to_owned(), outcome));
        }
    };
    for (_, list) in &projects {
        for (rel, dest, op) in list {
            let outcome = match op {
                Op::Skip => Outcome {
                    full: dest.full.clone(),
                    managed: false,
                },
                Op::Replace { overwrite, .. } => {
                    if !*overwrite && dest.full.exists() {
                        Outcome {
                            full: dest.full.clone(),
                            managed: false,
                        }
                    } else {
                        writes.push(Write {
                            path: dest.full.clone(),
                            contents: op.contents().map_err(|e| io_err(&dest.package, e))?,
                            replace: true,
                        });
                        Outcome {
                            full: dest.full.clone(),
                            managed: *overwrite,
                        }
                    }
                }
                Op::Append { managed, .. } => {
                    writes.push(Write {
                        path: dest.full.clone(),
                        contents: op.contents().map_err(|e| io_err(&dest.package, e))?,
                        replace: false,
                    });
                    Outcome {
                        full: dest.full.clone(),
                        managed: *managed,
                    }
                }
            };
            push_result(rel, outcome);
        }
    }

    // Fichiers autoload de référence : (ré)écrits sauf si trackés par git.
    let web_root = root_opts.location("web-root").unwrap_or(".");
    let vendor_real = realpath(&root.join("vendor")).unwrap_or_else(|| root.join("vendor"));
    let vendor_s = normalize_path(&vendor_real.to_string_lossy());
    let mut autoload_files = vec![(
        "autoload.php",
        "[web-root]/autoload.php",
        AUTOLOAD_TPL,
        "{relative_autoload_path}",
    )];
    if profile.autoload_runtime {
        autoload_files.push((
            "autoload_runtime.php",
            "[web-root]/autoload_runtime.php",
            AUTOLOAD_RUNTIME_TPL,
            "{relative_autoload_runtime_path}",
        ));
    }
    for (file, rel_key, tpl, placeholder) in autoload_files {
        let full = abs_from_cwd(root, &format!("{web_root}/{file}"));
        let full_norm = PathBuf::from(normalize_path(&full.to_string_lossy()));
        let committed = full_norm.exists()
            && git_ok(
                root,
                &["ls-files", "--error-unmatch", &full_norm.to_string_lossy()],
            );
        if committed {
            continue;
        }
        let relative = find_shortest_path(
            &full.to_string_lossy(),
            &format!("{vendor_s}/{file}"),
            false,
        );
        let relative = relative.strip_prefix("./").unwrap_or(&relative).to_owned();
        writes.push(Write {
            path: full_norm.clone(),
            contents: tpl.replace(placeholder, &relative).into_bytes(),
            replace: false,
        });
        // `$scaffold_results[] = …` : ajouté, jamais fusionné avec une entrée
        // de file-mapping visant le même fichier (deux entrées .gitignore).
        results.push((
            rel_key.to_owned(),
            Outcome {
                full: full_norm,
                managed: true,
            },
        ));
    }

    Ok(Plan {
        root: root.to_path_buf(),
        writes,
        results,
        gitignore: match root_opts.gitignore {
            Some(b) => GitIgnoreMode::Forced(b),
            None => GitIgnoreMode::Auto,
        },
    })
}

fn io_err(package: &str, e: std::io::Error) -> String {
    format!("{PLUGIN}: cannot read a scaffold source of {package}: {e}")
}

/// OperationData::normalizeScaffoldMetadata + OperationFactory::create.
fn make_op(
    package: &str,
    dir: &Path,
    pkg_real: &Path,
    dest: &str,
    value: &Value,
) -> Result<Op, String> {
    let data: Map<String, Value> = match value {
        Value::Bool(false) => return Ok(Op::Skip),
        Value::Bool(true) => {
            return Err(format!(
                "{PLUGIN}: file mapping {dest} in {package} cannot be given the value 'true'"
            ))
        }
        v if !php_truthy(v) => {
            return Err(format!(
                "{PLUGIN}: file mapping {dest} in {package} cannot be empty"
            ))
        }
        Value::String(s) => {
            let mut m = Map::new();
            m.insert("path".to_owned(), Value::String(s.clone()));
            m
        }
        Value::Object(m) => m.clone(),
        other => {
            return Err(format!(
                "{PLUGIN}: file mapping {dest} in {package} has an unsupported value {other}"
            ))
        }
    };
    let mode = match data.get("mode").and_then(Value::as_str) {
        Some(m) => m.to_owned(),
        None if data.contains_key("append") || data.contains_key("prepend") => "append".to_owned(),
        None => "replace".to_owned(),
    };
    let source = |key: &str| -> Result<Option<PathBuf>, String> {
        match data.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                let s = v.as_str().unwrap_or("");
                if s.is_empty() {
                    return Err(format!(
                        "{PLUGIN}: no scaffold file path given for {dest} in package {package}"
                    ));
                }
                let full = dir.join(s);
                if !full.exists() {
                    return Err(format!(
                        "{PLUGIN}: scaffold file {s} not found in package {package}"
                    ));
                }
                if full.is_dir() {
                    return Err(format!(
                        "{PLUGIN}: scaffold file {s} in package {package} is a directory; only files may be scaffolded"
                    ));
                }
                let real = realpath(&full).unwrap_or(full.clone());
                if !under(&real, pkg_real) {
                    return Err(format!(
                        "{PLUGIN}: scaffold file {s} of {package} resolves outside the package"
                    ));
                }
                Ok(Some(full))
            }
        }
    };
    match mode.as_str() {
        "skip" => Ok(Op::Skip),
        "replace" => {
            if !data.contains_key("path") || data.get("path") == Some(&Value::Null) {
                return Err(format!(
                    "{PLUGIN}: 'path' component required for 'replace' operations ({dest} in {package})"
                ));
            }
            let src =
                source("path")?.ok_or_else(|| format!("{PLUGIN}: missing path for {dest}"))?;
            // `overwrite()` = !empty ; défaut true.
            let overwrite = data.get("overwrite").map(php_truthy).unwrap_or(true);
            Ok(Op::Replace {
                source: src,
                overwrite,
            })
        }
        "append" => {
            let prepend = source("prepend")?;
            let append = source("append")?;
            let default = source("default")?;
            let has_content = |p: &Option<PathBuf>| {
                p.as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .is_some_and(|m| m.is_file() && m.len() > 0)
            };
            if !has_content(&prepend) && !has_content(&append) {
                return Ok(Op::Skip);
            }
            let force_append =
                default.is_some() || data.get("force-append").map(php_truthy).unwrap_or(false);
            Ok(Op::Append {
                prepend,
                append,
                default,
                force_append,
                managed: true,
                original: None,
            })
        }
        other => Err(format!(
            "{PLUGIN}: unknown scaffold operation mode {other} ({dest} in {package})"
        )),
    }
}

/// `AppendOp::scaffoldAtNewLocation` (les autres ops se renvoient elles-mêmes).
fn scaffold_at_new_location(op: Op, dest: &Dest, package: &str) -> Result<Op, String> {
    let Op::Append {
        prepend,
        append,
        default,
        force_append,
        ..
    } = op
    else {
        return Ok(op);
    };
    if !force_append {
        return Ok(Op::Skip);
    }
    if !dest.full.exists() {
        if default.is_some() {
            return Ok(Op::Append {
                prepend,
                append,
                default,
                force_append,
                managed: false,
                original: None,
            });
        }
        return Ok(Op::Skip);
    }
    let existing = std::fs::read(&dest.full).map_err(|e| io_err(package, e))?;
    let has_data = |p: &Option<PathBuf>| -> Result<bool, String> {
        match p {
            None => Ok(false),
            Some(p) => {
                let data = std::fs::read(p).map_err(|e| io_err(package, e))?;
                Ok(contains(&existing, &data))
            }
        }
    };
    if has_data(&append)? || has_data(&prepend)? {
        return Ok(Op::Skip);
    }
    Ok(Op::Append {
        prepend,
        append,
        default,
        force_append,
        managed: false,
        original: Some(existing),
    })
}

/// `str_contains` (vrai pour l'aiguille vide).
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// `git <args>` dans `cwd`, sans GIT_DIR (le plugin laisse git découvrir le
/// dépôt, un projet dans un dépôt parent compte) ; binaire absent = échec.
fn git_ok(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("LANGUAGE", "C")
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

impl Plan {
    /// Écrit les fichiers puis gère les `.gitignore` (ManageGitIgnore),
    /// dans cet ordre comme le plugin (git interrogé après les écritures).
    pub fn apply(&self) -> std::io::Result<()> {
        for w in &self.writes {
            if let Some(parent) = w.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if w.replace {
                write_replacing(&w.path, &w.contents)?;
            } else {
                std::fs::write(&w.path, &w.contents)?;
            }
        }
        let enabled = match &self.gitignore {
            GitIgnoreMode::Forced(b) => *b,
            GitIgnoreMode::Auto => {
                git_ok(&self.root, &["rev-parse", "--show-toplevel"])
                    && git_ok(&self.root, &["check-ignore", "vendor"])
            }
        };
        if !enabled {
            return Ok(());
        }
        let mut add: Vec<(PathBuf, Vec<String>)> = Vec::new();
        for (_, outcome) in &self.results {
            let path = outcome.full.to_string_lossy().into_owned();
            let ignored = git_ok(&self.root, &["check-ignore", &path]);
            if ignored {
                continue;
            }
            let tracked = git_ok(&self.root, &["ls-files", "--error-unmatch", &path]);
            if !tracked && outcome.managed {
                let dir = outcome.full.parent().and_then(realpath).unwrap_or_else(|| {
                    outcome
                        .full
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default()
                });
                let name = format!(
                    "/{}",
                    outcome
                        .full
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
                match add.iter_mut().find(|(d, _)| *d == dir) {
                    Some((_, list)) => list.push(name),
                    None => add.push((dir, vec![name])),
                }
            }
        }
        for (dir, mut entries) in add {
            entries.sort();
            let path = dir.join(".gitignore");
            let mut contents = std::fs::read(&path).unwrap_or_default();
            // `!empty($contents)` : "0" compte comme vide.
            if !(contents.is_empty() || contents == b"0" || contents.ends_with(b"\n")) {
                contents.push(b'\n');
            }
            contents.extend(entries.join("\n").into_bytes());
            std::fs::write(&path, contents)?;
        }
        Ok(())
    }
}

/// `ReplaceOp::process` : `makeWritable(dirname)` si nécessaire, `remove`,
/// `file_put_contents`, puis perms d'origine du fichier et du répertoire
/// restaurées — un `.htaccess` en 0444 ou un `sites/default` en 0555 ne
/// font pas échouer le scaffold.
fn write_replacing(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let file_mode = std::fs::symlink_metadata(path)
            .ok()
            .map(|m| m.permissions().mode());
        let mut parent_mode: Option<(PathBuf, u32)> = None;
        if let Some(dir) = path.parent() {
            if let Ok(m) = std::fs::metadata(dir) {
                let mode = m.permissions().mode();
                if mode & 0o200 == 0 {
                    std::fs::set_permissions(
                        dir,
                        std::fs::Permissions::from_mode((mode & 0o777) | 0o200),
                    )?;
                    parent_mode = Some((dir.to_path_buf(), mode));
                }
            }
        }
        let result = (|| {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            std::fs::write(path, contents)?;
            if let Some(mode) = file_mode {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))?;
            }
            Ok(())
        })();
        if let Some((dir, mode)) = parent_mode {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode & 0o7777));
        }
        result
    }
    #[cfg(not(unix))]
    {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::write(path, contents)
    }
}

// ---------------------------------------------------------------------------
// preAutoloadDump
// ---------------------------------------------------------------------------

/// Un paquet du dépôt local pour le hash : `getUniqueName()` =
/// `name-version_normalized`, `getSourceReference()` (chaîne vide si absent),
/// et l'alias éventuel (`name-<alias normalisé>`, même référence).
#[derive(Debug, Clone)]
pub struct HashPackage {
    pub name: String,
    pub version_normalized: String,
    pub source_reference: String,
    pub alias_normalized: Option<String>,
}

/// Sortie de `Plugin::preAutoloadDump`.
#[derive(Debug, Clone)]
pub struct PreAutoloadDump {
    /// Entrées à ajouter à la classmap de la racine (relatives au projet).
    pub root_classmap: Vec<String>,
    /// `vendor/drupal/DrupalInstalled.php`.
    pub drupal_installed: Vec<u8>,
}

/// `findPackage($name, new Constraint('>', ''))` : une version `dev-*` sans
/// alias numérique ne matche pas (`versionCompare` renvoie false).
fn installed_matches(packages: &[HashPackage], name: &str) -> bool {
    packages.iter().any(|p| {
        p.name.eq_ignore_ascii_case(name)
            && (!p.version_normalized.starts_with("dev-") || p.alias_normalized.is_some())
    })
}

/// Paquets du dépôt local après la transaction, depuis le lock.
pub fn hash_packages(lock: &crate::lock::Lock, with_dev: bool) -> Vec<HashPackage> {
    lock.wanted_packages(with_dev)
        .map(|p| {
            let default_branch = p
                .raw
                .get("default-branch")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            HashPackage {
                name: p.name().to_owned(),
                version_normalized: crate::version::normalize_pretty(p.version())
                    .unwrap_or_else(|_| p.version().to_owned()),
                source_reference: p
                    .raw
                    .get("source")
                    .and_then(|s| s.get("reference"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                alias_normalized: crate::root_version::branch_alias_of(
                    p.version(),
                    p.raw.get("extra"),
                    default_branch,
                )
                .map(|(n, _)| n),
            }
        })
        .collect()
}

/// La racine comme `getPackage()` la voit (alias de branche compris).
pub fn root_hash_package(root: &crate::state::RootPackage) -> HashPackage {
    HashPackage {
        name: root.name.clone(),
        version_normalized: root.version.clone(),
        source_reference: root.reference.clone().unwrap_or_default(),
        alias_normalized: root.alias_normalized.clone(),
    }
}

pub fn pre_autoload_dump(
    profile: Profile,
    vendor_dir: &str,
    packages: &[HashPackage],
    root: &HashPackage,
) -> Option<PreAutoloadDump> {
    if !profile.pre_autoload_dump {
        return None;
    }
    let mut classmap: Vec<String> = Vec::new();
    if installed_matches(packages, "symfony/http-foundation") {
        for f in [
            "Request.php",
            "RequestStack.php",
            "ParameterBag.php",
            "FileBag.php",
            "ServerBag.php",
            "HeaderBag.php",
            "HeaderUtils.php",
        ] {
            classmap.push(format!("{vendor_dir}/symfony/http-foundation/{f}"));
        }
    }
    if installed_matches(packages, "symfony/http-kernel") {
        for f in [
            "HttpKernel.php",
            "HttpKernelInterface.php",
            "TerminableInterface.php",
        ] {
            classmap.push(format!("{vendor_dir}/symfony/http-kernel/{f}"));
        }
    }
    if installed_matches(packages, "symfony/dependency-injection") {
        classmap.push(format!(
            "{vendor_dir}/symfony/dependency-injection/ContainerInterface.php"
        ));
    }
    if installed_matches(packages, "psr/container") {
        classmap.push(format!(
            "{vendor_dir}/psr/container/src/ContainerInterface.php"
        ));
    }
    classmap.push(format!("{vendor_dir}/drupal/DrupalInstalled.php"));

    // DrupalInstalledTemplate::getCode (profil trié).
    let mut names: Vec<(String, String)> = Vec::new();
    for p in packages {
        if let Some(alias) = &p.alias_normalized {
            names.push((format!("{}-{alias}", p.name), p.source_reference.clone()));
        }
        names.push((
            format!("{}-{}", p.name, p.version_normalized),
            p.source_reference.clone(),
        ));
    }
    names.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut versions = String::new();
    for (u, r) in &names {
        versions.push_str(u);
        versions.push('-');
        versions.push_str(r);
        versions.push('|');
    }
    let root_version = root
        .alias_normalized
        .as_deref()
        .unwrap_or(&root.version_normalized);
    versions.push_str(&format!(
        "{}-{root_version}-{}",
        root.name, root.source_reference
    ));
    let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(versions.as_bytes()));
    Some(PreAutoloadDump {
        root_classmap: classmap,
        drupal_installed: DRUPAL_INSTALLED_TPL
            .replace("{version_hash}", &hash)
            .into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fingerprint_table_maps_known_sources() {
        let reference =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/drupal-scaffold");
        let fp = fingerprint(&reference).expect("fingerprint");
        let profile = profile_for(&fp).expect("11.4.6 ported");
        assert!(profile.pre_autoload_dump && profile.sorted_hash && profile.autoload_runtime);
        assert!(known_versions(&fp).iter().any(|v| v == "11.4.6"));
        assert!(profile_for("0000").is_none());
        let table: Value = serde_json::from_str(FINGERPRINTS).expect("json");
        let mut versions = 0;
        for (_, e) in table["sources"].as_object().expect("sources") {
            versions += e["versions"].as_array().expect("v").len();
        }
        assert_eq!(versions, 120);
    }

    #[test]
    fn interpolation_and_options() {
        let data = vec![("web-root".to_owned(), "/p/web".to_owned())];
        assert_eq!(
            interpolate("[web-root]/index.php", &data, ""),
            "/p/web/index.php"
        );
        assert_eq!(interpolate("[nope]/x", &data, ""), "/x");
        assert_eq!(interpolate("no tokens", &data, ""), "no tokens");
        assert_eq!(
            interpolate("[web-root]/[web-root]", &data, ""),
            "/p/web//p/web"
        );
        let o = Options::from_extra(Some(
            &json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "gitignore": false, "allowed-packages": ["a/b"]}}),
        ));
        assert_eq!(o.location("web-root"), Some("web/"));
        assert_eq!(o.location("project-root"), Some("."));
        assert_eq!(o.gitignore, Some(false));
        assert_eq!(o.allowed_packages, vec!["a/b"]);
        let d = Options::from_extra(None);
        assert_eq!(d.location("web-root"), Some("."));
        assert_eq!(d.gitignore, None);
    }

    #[test]
    fn drupal_installed_matches_reference_project() {
        // Valeurs du projet de référence (drupal/recommended-project 11.4.6,
        // installé par Composer, dépôt git sans commit → référence vide).
        let profile = Profile {
            pre_autoload_dump: true,
            sorted_hash: true,
            autoload_runtime: true,
        };
        let root = HashPackage {
            name: "drupal/recommended-project".into(),
            version_normalized: "1.0.0.0".into(),
            source_reference: String::new(),
            alias_normalized: None,
        };
        let out = pre_autoload_dump(profile, "vendor", &[], &root).expect("dump");
        assert_eq!(out.root_classmap, vec!["vendor/drupal/DrupalInstalled.php"]);
        let text = String::from_utf8(out.drupal_installed).expect("utf8");
        assert!(text.contains("public const string VERSIONS_HASH = '"));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn plan_replace_append_skip_and_override() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("real");
        let pkg = root.join("vendor/a/core");
        std::fs::create_dir_all(pkg.join("assets")).expect("mkdir");
        std::fs::write(pkg.join("assets/index.php"), "<?php core index\n").expect("w");
        std::fs::write(pkg.join("assets/robots.txt"), "robots\n").expect("w");
        std::fs::write(pkg.join("assets/append.txt"), "APPENDED").expect("w");
        let other = root.join("vendor/b/site");
        std::fs::create_dir_all(other.join("files")).expect("mkdir");
        std::fs::write(other.join("files/index.php"), "<?php site index\n").expect("w");
        std::fs::write(other.join("files/extra.txt"), "EXTRA").expect("w");
        let packages = vec![
            ScaffoldPackage {
                name: "drupal/core".into(),
                dir: pkg.clone(),
                extra: Some(json!({"drupal-scaffold": {"file-mapping": {
                    "[web-root]/index.php": "assets/index.php",
                    "[web-root]/robots.txt": {"path": "assets/robots.txt", "overwrite": false},
                    "[web-root]/skipped.txt": false,
                    "[web-root]/robots.txt.bak": {"append": "assets/append.txt"}
                }}})),
            },
            ScaffoldPackage {
                name: "b/site".into(),
                dir: other.clone(),
                extra: Some(json!({"drupal-scaffold": {"file-mapping": {
                    "[web-root]/index.php": "files/index.php",
                    "[web-root]/robots.txt": {"append": "files/extra.txt"}
                }}})),
            },
        ];
        let root_extra = json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "allowed-packages": ["b/site"], "gitignore": false}});
        let profile = Profile {
            pre_autoload_dump: true,
            sorted_hash: true,
            autoload_runtime: true,
        };
        let plan = plan(profile, &root, "me/root", Some(&root_extra), &packages).expect("plan");
        plan.apply().expect("apply");
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
        // b/site surcharge index.php de drupal/core.
        assert_eq!(read("web/index.php"), "<?php site index\n");
        // append sur une cible déjà scaffoldée : contenu de l'op précédente + append.
        assert_eq!(read("web/robots.txt"), "robots\n\nEXTRA");
        // append sur une nouvelle cible sans force-append : skip.
        assert!(!root.join("web/robots.txt.bak").exists());
        assert!(!root.join("web/skipped.txt").exists());
        assert_eq!(
            read("web/autoload.php"),
            AUTOLOAD_TPL.replace("{relative_autoload_path}", "../vendor/autoload.php")
        );
        assert!(read("web/autoload_runtime.php").contains("'/../vendor/autoload_runtime.php'"));
        // Second plan : rien ne change → seuls les autoload*.php sont réécrits.
        let plan2 =
            super::plan(profile, &root, "me/root", Some(&root_extra), &packages).expect("plan2");
        assert_eq!(plan2.writes().len(), 2);
        // Destination qui est un répertoire → refus.
        std::fs::create_dir_all(root.join("web/dir")).expect("mkdir");
        let hostile = vec![ScaffoldPackage {
            name: "drupal/core".into(),
            dir: pkg.clone(),
            extra: Some(
                json!({"drupal-scaffold": {"file-mapping": {"[web-root]/dir": "assets/index.php"}}}),
            ),
        }];
        let err =
            super::plan(profile, &root, "me/root", Some(&root_extra), &hostile).expect_err("dir");
        assert!(err.contains("not a regular file"), "{err}");
        let outside = vec![ScaffoldPackage {
            name: "drupal/core".into(),
            dir: pkg.clone(),
            extra: Some(
                json!({"drupal-scaffold": {"file-mapping": {"[project-root]/../escape.php": "assets/index.php"}}}),
            ),
        }];
        let err = super::plan(profile, &root, "me/root", Some(&root_extra), &outside)
            .expect_err("outside");
        assert!(err.contains("outside the project"), "{err}");
    }
}
