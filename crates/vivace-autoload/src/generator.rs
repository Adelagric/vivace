//! Port de `AutoloadGenerator::dump` (docs/reference/AutoloadGenerator.php) :
//! package map (racine + paquets installés), tri (PackageSorter), fusion des
//! règles PSR-0/PSR-4/classmap/files/exclude, scan de classmap (avec `-o` :
//! tous les répertoires PSR), puis génération de vendor/autoload.php et de
//! vendor/composer/{autoload_*.php, platform_check.php, ClassLoader.php,
//! LICENSE}. Parité tenue par harness/diff-vendor.sh --with-autoloader.
//!
//! Les noms de classes circulent en octets bruts (voir classmap.rs) ; les
//! fichiers qui les contiennent sont assemblés en `Vec<u8>`.

use crate::classmap::{AutoloadType, Scanner};
use crate::pathutil::{
    find_shortest_path, find_shortest_path_code, normalize_path, php_str, php_str_bytes, preg_quote,
};
use crate::sorter::{sort_packages, SortablePackage};
use crate::templates::{self, PhpKey, PhpVal};
use md5::{Digest, Md5};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use vivace_core::layout::Layout;
use vivace_core::lock::Lock;

#[derive(Debug, thiserror::Error)]
pub enum AutoloadError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    ClassMap(#[from] crate::classmap::ClassMapError),
    #[error("regex d'exclusion invalide: {0}")]
    Regex(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> AutoloadError + '_ {
    move |source| AutoloadError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformCheckMode {
    Off,
    PhpOnly,
    Full,
}

pub struct DumpOptions {
    pub dev_mode: bool,
    pub optimize: bool,
    pub authoritative: bool,
    pub platform_check: PlatformCheckMode,
    pub ignore_all_platform_reqs: bool,
    pub ignored_platform_reqs: Vec<String>,
    pub suffix: Option<String>,
    /// Racine du store + racine du cache : active le cache de classmap par
    /// entrée de store (None = scan complet à chaque fois).
    pub classmap_cache: Option<ClassmapCacheConfig>,
}

#[derive(Debug, Clone)]
pub struct ClassmapCacheConfig {
    pub store_root: PathBuf,
    pub cache_root: PathBuf,
}

#[derive(Debug, Default)]
pub struct DumpReport {
    pub classes: usize,
    pub warnings: Vec<String>,
}

/// Un paquet vu par le générateur.
struct Pkg {
    name: String,
    /// Chemin d'installation absolu normalisé ; None pour un metapackage ;
    /// "" pour la racine.
    install_path: Option<String>,
    target_dir: Option<String>,
    autoload: Map<String, Value>,
    autoload_dev: Map<String, Value>,
    requires: Map<String, Value>,
    replaces: Vec<String>,
    provides_and_replaces: Vec<(String, String)>,
    include_paths: Vec<String>,
    is_root: bool,
}

fn obj(v: Option<&Value>) -> Map<String, Value> {
    v.and_then(Value::as_object).cloned().unwrap_or_default()
}

/// `array_merge_recursive($autoload, $autoloadDev)` restreint aux formes du
/// schéma Composer : listes concaténées, maps fusionnées clé à clé.
fn merge_autoload(a: &Map<String, Value>, b: &Map<String, Value>) -> Map<String, Value> {
    let mut out = a.clone();
    for (k, vb) in b {
        match out.get_mut(k) {
            None => {
                out.insert(k.clone(), vb.clone());
            }
            Some(va) => {
                *va = match (va.clone(), vb) {
                    (Value::Array(mut la), Value::Array(lb)) => {
                        la.extend(lb.iter().cloned());
                        Value::Array(la)
                    }
                    (Value::Object(oa), Value::Object(ob)) => {
                        let mut m = oa;
                        for (nk, nv) in ob {
                            match m.get_mut(nk) {
                                None => {
                                    m.insert(nk.clone(), nv.clone());
                                }
                                Some(existing) => {
                                    let mut list = match existing.clone() {
                                        Value::Array(l) => l,
                                        other => vec![other],
                                    };
                                    match nv {
                                        Value::Array(l) => list.extend(l.iter().cloned()),
                                        other => list.push(other.clone()),
                                    }
                                    *existing = Value::Array(list);
                                }
                            }
                        }
                        Value::Object(m)
                    }
                    (_, other) => other.clone(),
                };
            }
        }
    }
    out
}

/// Chemins d'autoload fusionnés (`parseAutoloads`).
#[derive(Default)]
struct Autoloads {
    psr0: Vec<(String, Vec<String>)>,
    psr4: Vec<(String, Vec<String>)>,
    classmap: Vec<String>,
    files: Vec<(String, String)>,
    exclude: Vec<String>,
}

pub fn dump(
    project_dir: &Path,
    lock: &Lock,
    root_manifest: &Value,
    layout: &Layout,
    opts: &DumpOptions,
) -> Result<DumpReport, AutoloadError> {
    let mut report = DumpReport::default();
    let base_path = normalize_path(
        &std::fs::canonicalize(project_dir)
            .map_err(io(project_dir))?
            .to_string_lossy(),
    );
    let vendor_dir = project_dir.join("vendor");
    std::fs::create_dir_all(&vendor_dir).map_err(io(&vendor_dir))?;
    let vendor_path = normalize_path(
        &std::fs::canonicalize(&vendor_dir)
            .map_err(io(&vendor_dir))?
            .to_string_lossy(),
    );
    let target_dir = vendor_dir.join("composer");
    std::fs::create_dir_all(&target_dir).map_err(io(&target_dir))?;
    let target_path = format!("{vendor_path}/composer");

    // Chemin d'installation absolu d'un paquet : sous vendor/ via le vendor
    // canonicalisé (symlinks résolus comme Composer), sinon sous la racine.
    let install_abs = |name: &str| -> Option<String> {
        let rel = layout.rel(name)?;
        Some(match rel.strip_prefix("vendor/") {
            Some(rest) => format!("{vendor_path}/{rest}"),
            None => format!("{base_path}/{rel}"),
        })
    };

    let vendor_path_code = find_shortest_path_code(&target_path, &vendor_path, false);
    let vendor_to_target_code = find_shortest_path_code(&vendor_path, &target_path, false);
    let app_base_dir_code =
        find_shortest_path_code(&vendor_path, &base_path, false).replace("__DIR__", "$vendorDir");

    // Package map : racine puis paquets installés (état = ce que l'installeur
    // a posé : lock.wanted_packages(dev_mode)).
    let root = Pkg {
        name: root_manifest
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("__root__")
            .to_owned(),
        install_path: Some(String::new()),
        target_dir: root_manifest
            .get("target-dir")
            .and_then(Value::as_str)
            .map(str::to_owned),
        autoload: obj(root_manifest.get("autoload")),
        autoload_dev: obj(root_manifest.get("autoload-dev")),
        requires: obj(root_manifest.get("require")),
        replaces: obj(root_manifest.get("replace")).keys().cloned().collect(),
        provides_and_replaces: links(root_manifest.get("replace"))
            .into_iter()
            .chain(links(root_manifest.get("provide")))
            .collect(),
        include_paths: string_list(root_manifest.get("include-path")),
        is_root: true,
    };
    let mut packages: Vec<Pkg> = vec![root];
    let dev_names: BTreeSet<String> = if opts.dev_mode {
        lock.packages_dev
            .iter()
            .map(|p| p.name().to_owned())
            .collect()
    } else {
        BTreeSet::new()
    };
    for p in lock.wanted_packages(opts.dev_mode) {
        let v = Value::Object(p.raw.clone());
        packages.push(Pkg {
            name: p.name().to_owned(),
            install_path: install_abs(p.name()),
            target_dir: p.target_dir().map(str::to_owned),
            autoload: obj(v.get("autoload")),
            autoload_dev: Map::new(),
            requires: obj(v.get("require")),
            replaces: obj(v.get("replace")).keys().cloned().collect(),
            provides_and_replaces: links(v.get("replace"))
                .into_iter()
                .chain(links(v.get("provide")))
                .collect(),
            include_paths: string_list(v.get("include-path")),
            is_root: false,
        });
    }

    // Cache de classmap : chemin d'installation absolu → entrée de store.
    let store_entries: Vec<(String, PathBuf)> = match &opts.classmap_cache {
        Some(cfg) => {
            let store = vivace_core::store::Store::at(cfg.store_root.clone());
            lock.wanted_packages(opts.dev_mode)
                .filter_map(|p| {
                    Some((
                        install_abs(p.name())?,
                        store.entry_path(p.name(), p.version(), p.dist_reference()),
                    ))
                })
                .collect()
        }
        None => Vec::new(),
    };
    let cache_slot = |abs: &str| -> Option<crate::classmap::CacheSlot> {
        let cfg = opts.classmap_cache.as_ref()?;
        let (install, entry) = store_entries
            .iter()
            .find(|(install, _)| abs == install || abs.starts_with(&format!("{install}/")))?;
        if !entry.is_dir() {
            return None;
        }
        let rel = abs
            .strip_prefix(install)
            .unwrap_or("")
            .trim_start_matches('/');
        Some(crate::classmap::CacheSlot::new(
            &cfg.cache_root,
            entry,
            Path::new(rel),
        ))
    };

    let autoloads = parse_autoloads(&packages, opts.dev_mode, &base_path);

    // autoload_namespaces.php / autoload_psr4.php
    let path_code = |p: &str| get_path_code(&base_path, &vendor_path, p);
    let mut namespaces_body: Vec<u8> = Vec::new();
    for (ns, paths) in &autoloads.psr0 {
        let codes: Vec<String> = paths.iter().map(|p| path_code(p)).collect();
        namespaces_body.extend_from_slice(
            format!("    {} => array({}),\n", php_str(ns), codes.join(", ")).as_bytes(),
        );
    }
    let mut psr4_body: Vec<u8> = Vec::new();
    for (ns, paths) in &autoloads.psr4 {
        let codes: Vec<String> = paths.iter().map(|p| path_code(p)).collect();
        psr4_body.extend_from_slice(
            format!("    {} => array({}),\n", php_str(ns), codes.join(", ")).as_bytes(),
        );
    }

    // Classmap.
    let mut scanner = Scanner::new()?;
    for dir in &autoloads.classmap {
        let abs = absolute(&base_path, dir);
        let excl = build_exclusion_regex(&abs, &autoloads.exclude, &base_path)?;
        let slot = cache_slot(&normalize_path(&abs));
        scanner.scan_path_cached(
            Path::new(&abs),
            excl.as_ref(),
            AutoloadType::ClassMap,
            "",
            slot.as_ref(),
        )?;
    }
    if opts.optimize || opts.authoritative {
        // krsort des namespaces, psr-4 puis psr-0 dans chaque groupe
        let mut by_ns: BTreeMap<String, Vec<(Vec<String>, AutoloadType)>> = BTreeMap::new();
        for (ns, paths) in &autoloads.psr4 {
            by_ns
                .entry(ns.clone())
                .or_default()
                .push((paths.clone(), AutoloadType::Psr4));
        }
        for (ns, paths) in &autoloads.psr0 {
            by_ns
                .entry(ns.clone())
                .or_default()
                .push((paths.clone(), AutoloadType::Psr0));
        }
        for (ns, groups) in by_ns.iter().rev() {
            for (paths, ty) in groups {
                for dir in paths {
                    let abs = normalize_path(&absolute(&base_path, dir));
                    if !Path::new(&abs).is_dir() {
                        continue;
                    }
                    let mut excluded = autoloads.exclude.clone();
                    if vendor_path.contains(&format!("{abs}/")) {
                        excluded.push(format!("{vendor_path}/"));
                    }
                    let excl = build_exclusion_regex(&abs, &excluded, &base_path)?;
                    let slot = cache_slot(&abs);
                    scanner.scan_path_cached(
                        Path::new(&abs),
                        excl.as_ref(),
                        *ty,
                        ns,
                        slot.as_ref(),
                    )?;
                }
            }
        }
    }
    for (class, others) in &scanner.class_map.ambiguous {
        let first = scanner
            .class_map
            .map
            .get(class)
            .cloned()
            .unwrap_or_default();
        report.warnings.push(format!(
            "Warning: Ambiguous class resolution, \"{}\" was found in both \"{first}\" and \"{}\", the first will be used.",
            String::from_utf8_lossy(class),
            others.join("\", \"")
        ));
    }
    for (msg, _, path) in &scanner.class_map.psr_violations {
        if !path.starts_with(&format!("{vendor_path}/")) {
            report.warnings.push(format!("Warning: {msg}"));
        }
    }
    scanner.add_class(
        b"Composer\\InstalledVersions",
        &format!("{vendor_path}/composer/InstalledVersions.php"),
    );
    report.classes = scanner.class_map.map.len();

    let mut classmap_body: Vec<u8> = Vec::new();
    for (class, path) in &scanner.class_map.map {
        classmap_body.extend_from_slice(b"    ");
        classmap_body.extend_from_slice(&php_str_bytes(class));
        classmap_body.extend_from_slice(b" => ");
        classmap_body.extend_from_slice(path_code(path).as_bytes());
        classmap_body.extend_from_slice(b",\n");
    }

    // Suffixe : override, config.autoloader-suffix, autoload.php existant,
    // content-hash du lock, sinon aléatoire.
    let suffix = resolve_suffix(opts, root_manifest, &vendor_dir, lock);

    write(
        &target_dir.join("autoload_namespaces.php"),
        templates::map_file(
            "autoload_namespaces.php",
            &vendor_path_code,
            &app_base_dir_code,
            &namespaces_body,
        ),
    )?;
    write(
        &target_dir.join("autoload_psr4.php"),
        templates::map_file(
            "autoload_psr4.php",
            &vendor_path_code,
            &app_base_dir_code,
            &psr4_body,
        ),
    )?;
    write(
        &target_dir.join("autoload_classmap.php"),
        templates::map_file(
            "autoload_classmap.php",
            &vendor_path_code,
            &app_base_dir_code,
            &classmap_body,
        ),
    )?;

    // include_paths.php
    let mut include_paths: Vec<String> = Vec::new();
    for p in &packages {
        let Some(install) = &p.install_path else {
            continue;
        };
        let mut install = install.clone();
        if let Some(td) = &p.target_dir {
            if !td.is_empty() && !p.is_root {
                install = install.trim_end_matches(&format!("/{td}")).to_owned();
            }
        }
        for ip in &p.include_paths {
            let ip = ip.trim_matches('/');
            include_paths.push(if install.is_empty() {
                ip.to_owned()
            } else {
                format!("{install}/{ip}")
            });
        }
    }
    let include_path_file = target_dir.join("include_paths.php");
    let use_include_path = !include_paths.is_empty();
    if use_include_path {
        let body: Vec<u8> = include_paths
            .iter()
            .flat_map(|p| format!("    {},\n", path_code(p)).into_bytes())
            .collect();
        write(
            &include_path_file,
            templates::map_file(
                "include_paths.php",
                &vendor_path_code,
                &app_base_dir_code,
                &body,
            ),
        )?;
    } else if include_path_file.exists() {
        std::fs::remove_file(&include_path_file).map_err(io(&include_path_file))?;
    }

    // autoload_files.php
    let files_file = target_dir.join("autoload_files.php");
    let use_include_files = !autoloads.files.is_empty();
    if use_include_files {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut body: Vec<u8> = Vec::new();
        for (id, p) in &autoloads.files {
            let code = path_code(p);
            if !seen.insert(code.clone()) {
                report.warnings.push(format!(
                    "Warning: \"files\" autoload rule included multiple times: {code}"
                ));
            }
            body.extend_from_slice(format!("    {} => {},\n", php_str(id), code).as_bytes());
        }
        write(
            &files_file,
            templates::map_file(
                "autoload_files.php",
                &vendor_path_code,
                &app_base_dir_code,
                &body,
            ),
        )?;
    } else if files_file.exists() {
        std::fs::remove_file(&files_file).map_err(io(&files_file))?;
    }

    // autoload_static.php
    let static_file = build_static_file(
        &suffix,
        &autoloads,
        &scanner.class_map.map,
        &base_path,
        &vendor_path,
        &target_path,
    );
    write(&target_dir.join("autoload_static.php"), static_file)?;

    // platform_check.php
    let check_platform =
        opts.platform_check != PlatformCheckMode::Off && !opts.ignore_all_platform_reqs;
    let platform_file = target_dir.join("platform_check.php");
    let mut platform_written = false;
    if check_platform {
        if let Some(content) = platform_check(&packages, &dev_names, opts) {
            write(&platform_file, content)?;
            platform_written = true;
        }
    }
    if !platform_written && platform_file.exists() {
        std::fs::remove_file(&platform_file).map_err(io(&platform_file))?;
    }

    // autoload.php + autoload_real.php + ClassLoader.php + LICENSE
    let real_code = {
        let last = vendor_to_target_code.chars().last().unwrap_or(' ');
        if last == '\'' || last == '"' {
            format!(
                "{}/autoload_real.php{last}",
                &vendor_to_target_code[..vendor_to_target_code.len() - 1]
            )
        } else {
            format!("{vendor_to_target_code} . '/autoload_real.php'")
        }
    };
    write(
        &vendor_dir.join("autoload.php"),
        templates::autoload_php(&real_code, &suffix),
    )?;
    let prepend = root_manifest
        .get("config")
        .and_then(|c| c.get("prepend-autoloader"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let use_global_include_path = root_manifest
        .get("config")
        .and_then(|c| c.get("use-include-path"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    write(
        &target_dir.join("autoload_real.php"),
        templates::autoload_real_php(&templates::RealFileOptions {
            suffix: &suffix,
            check_platform: platform_written,
            use_include_path,
            use_include_files,
            class_map_authoritative: opts.authoritative,
            use_global_include_path,
            prepend_autoloader: prepend,
            target_dir_loader: None,
        }),
    )?;
    write(
        &target_dir.join("ClassLoader.php"),
        templates::CLASS_LOADER_PHP,
    )?;
    write(&target_dir.join("LICENSE"), templates::LICENSE)?;
    Ok(report)
}

fn links(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, c)| c.as_str().map(|c| (k.clone(), c.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn absolute(base: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{base}/{path}")
    }
}

fn write(path: &Path, content: impl AsRef<[u8]>) -> Result<(), AutoloadError> {
    let tmp = path.with_extension("vivace-tmp");
    std::fs::write(&tmp, content.as_ref()).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))?;
    Ok(())
}

fn resolve_suffix(
    opts: &DumpOptions,
    root_manifest: &Value,
    vendor_dir: &Path,
    lock: &Lock,
) -> String {
    if let Some(s) = &opts.suffix {
        if !s.is_empty() {
            return s.clone();
        }
    }
    if let Some(s) = root_manifest
        .get("config")
        .and_then(|c| c.get("autoloader-suffix"))
        .and_then(Value::as_str)
    {
        if !s.is_empty() {
            return s.to_owned();
        }
    }
    if let Ok(existing) = std::fs::read_to_string(vendor_dir.join("autoload.php")) {
        if let Some(pos) = existing.find("ComposerAutoloaderInit") {
            let rest = &existing[pos + "ComposerAutoloaderInit".len()..];
            let s: String = rest
                .chars()
                .take_while(|c| *c != ':' && !c.is_whitespace())
                .collect();
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Some(h) = &lock.content_hash {
        if !h.is_empty()
            && h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return h.clone();
        }
    }
    let mut hasher = Md5::new();
    hasher.update(format!("{:?}", std::time::SystemTime::now()).as_bytes());
    format!("{:x}", hasher.finalize())
}

/// `parseAutoloads` : filtrage dev, tri, puis fusion par type.
fn parse_autoloads(packages: &[Pkg], dev_mode: bool, base_path: &str) -> Autoloads {
    let root = &packages[0];
    let others: Vec<&Pkg> = packages[1..].iter().collect();

    // !devMode : filterPackageMap (atteignables depuis les require de la racine).
    let filtered: Vec<&Pkg> = if dev_mode {
        others
    } else {
        let by_name: BTreeMap<&str, &Pkg> = others.iter().map(|p| (p.name.as_str(), *p)).collect();
        let mut replaced_by: BTreeMap<&str, &str> = BTreeMap::new();
        for p in &others {
            for r in &p.replaces {
                replaced_by.insert(r.as_str(), p.name.as_str());
            }
        }
        let mut include: BTreeSet<String> = BTreeSet::new();
        fn add<'a>(
            p: &'a Pkg,
            by_name: &BTreeMap<&str, &'a Pkg>,
            replaced_by: &BTreeMap<&str, &str>,
            include: &mut BTreeSet<String>,
        ) {
            for target in p.requires.keys() {
                let target = replaced_by
                    .get(target.as_str())
                    .copied()
                    .unwrap_or(target.as_str());
                if include.insert(target.to_owned()) {
                    if let Some(dep) = by_name.get(target) {
                        add(dep, by_name, replaced_by, include);
                    }
                }
            }
        }
        add(root, &by_name, &replaced_by, &mut include);
        others
            .into_iter()
            .filter(|p| include.contains(&p.name) || p.replaces.iter().any(|r| include.contains(r)))
            .collect()
    };

    let sortable: Vec<SortablePackage<'_>> = filtered
        .iter()
        .map(|p| SortablePackage {
            name: &p.name,
            requires: p.requires.keys().map(String::as_str).collect(),
        })
        .collect();
    let order = sort_packages(&sortable);
    let mut sorted: Vec<&Pkg> = order.into_iter().map(|i| filtered[i]).collect();
    sorted.push(root);
    let reversed: Vec<&Pkg> = sorted.iter().rev().copied().collect();

    let mut out = Autoloads {
        psr0: parse_type_map(&reversed, "psr-0", dev_mode),
        psr4: parse_type_map(&reversed, "psr-4", dev_mode),
        classmap: parse_type_list(&reversed, "classmap", dev_mode, base_path)
            .into_iter()
            .map(|(_, p)| p)
            .collect(),
        files: parse_type_list(&sorted, "files", dev_mode, base_path),
        exclude: parse_type_list(&sorted, "exclude-from-classmap", dev_mode, base_path)
            .into_iter()
            .map(|(_, p)| p)
            .collect(),
    };
    // krsort : ordre décroissant des clés (strcmp).
    out.psr0.sort_by(|a, b| b.0.cmp(&a.0));
    out.psr4.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

fn effective_autoload(p: &Pkg, dev_mode: bool) -> Map<String, Value> {
    if dev_mode && p.is_root {
        merge_autoload(&p.autoload, &p.autoload_dev)
    } else {
        p.autoload.clone()
    }
}

/// (clé/namespace, path déclaré ajusté, relativePath) pour un type d'autoload.
fn package_paths(p: &Pkg, ty: &str, dev_mode: bool) -> Vec<(String, String, String)> {
    let Some(install) = &p.install_path else {
        return Vec::new();
    };
    let autoload = effective_autoload(p, dev_mode);
    let Some(entries) = autoload.get(ty) else {
        return Vec::new();
    };
    let mut install = install.clone();
    if let Some(td) = &p.target_dir {
        if !td.is_empty() && !p.is_root {
            install = install.trim_end_matches(&format!("/{td}")).to_owned();
        }
    }
    let mut out = Vec::new();
    let pairs: Vec<(String, Vec<String>)> = match entries {
        Value::Object(m) => m
            .iter()
            .map(|(k, v)| (k.clone(), string_list(Some(v))))
            .collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), string_list(Some(v))))
            .collect(),
        _ => Vec::new(),
    };
    for (key, paths) in pairs {
        for mut path in paths {
            let is_filelike = matches!(ty, "files" | "classmap" | "exclude-from-classmap");
            if is_filelike {
                if let Some(td) = &p.target_dir {
                    if !td.is_empty()
                        && !p.is_root
                        && !Path::new(&format!("{install}/{path}")).exists()
                    {
                        path = format!("{td}/{path}");
                    }
                }
            }
            let relative = if install.is_empty() {
                if path.is_empty() {
                    ".".to_owned()
                } else {
                    path.clone()
                }
            } else {
                format!("{install}/{path}")
            };
            out.push((key.clone(), path, relative));
        }
    }
    out
}

fn parse_type_map(packages: &[&Pkg], ty: &str, dev_mode: bool) -> Vec<(String, Vec<String>)> {
    let mut map: Vec<(String, Vec<String>)> = Vec::new();
    for p in packages {
        for (ns, _, relative) in package_paths(p, ty, dev_mode) {
            let ns = ns.trim_start_matches('\\').to_owned();
            match map.iter_mut().find(|(k, _)| *k == ns) {
                Some((_, v)) => v.push(relative),
                None => map.push((ns, vec![relative])),
            }
        }
    }
    map
}

/// files (id md5 → chemin), classmap et exclude (clé ignorée).
fn parse_type_list(
    packages: &[&Pkg],
    ty: &str,
    dev_mode: bool,
    base_path: &str,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for p in packages {
        for (_, path, relative) in package_paths(p, ty, dev_mode) {
            if ty == "exclude-from-classmap" {
                if let Some(piece) = exclusion_piece(p, &path, base_path) {
                    out.push((String::new(), piece));
                }
                continue;
            }
            if ty == "files" {
                let mut h = Md5::new();
                h.update(format!("{}:{}", p.name, path).as_bytes());
                let id = format!("{:x}", h.finalize());
                match out.iter_mut().find(|(k, _)| *k == id) {
                    Some(slot) => slot.1 = relative,
                    None => out.push((id, relative)),
                }
                continue;
            }
            out.push((String::new(), relative));
        }
    }
    out
}

/// Fragment de regex pour une règle exclude-from-classmap.
fn exclusion_piece(p: &Pkg, path: &str, base_path: &str) -> Option<String> {
    let trimmed = path.replace('\\', "/");
    let trimmed = trimmed.trim_matches('/');
    let mut quoted = preg_quote(trimmed);
    while quoted.contains("//") {
        quoted = quoted.replace("//", "/");
    }
    let pattern = quoted.replace("\\*\\*", ".+?").replace("\\*", "[^/]+?");
    // Préfixe `(\.\.?/)+` → updir
    let mut rest = pattern.as_str();
    let mut updir = String::new();
    loop {
        if let Some(r) = rest.strip_prefix("\\.\\./") {
            updir.push_str("../");
            rest = r;
        } else if let Some(r) = rest.strip_prefix("\\./") {
            updir.push_str("./");
            rest = r;
        } else {
            break;
        }
    }
    let install = match &p.install_path {
        Some(s) if !s.is_empty() => s.clone(),
        _ => base_path.to_owned(),
    };
    let resolved = std::fs::canonicalize(format!("{install}/{updir}")).ok()?;
    let resolved = normalize_path(&resolved.to_string_lossy());
    Some(format!("{}/{rest}($|/)", preg_quote(&resolved)))
}

fn build_exclusion_regex(
    dir: &str,
    excluded: &[String],
    base_path: &str,
) -> Result<Option<pcre2::bytes::Regex>, AutoloadError> {
    if excluded.is_empty() {
        return Ok(None);
    }
    let mut kept: Vec<String> = excluded.to_vec();
    if Path::new(dir).exists() {
        let real = std::fs::canonicalize(dir)
            .map(|p| normalize_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| dir.to_owned());
        let dir_match = preg_quote(&real);
        let abs = normalize_path(&absolute(base_path, dir));
        let dir_match_normalized = preg_quote(&abs);
        let is_symlink = dir_match != dir_match_normalized;
        kept.retain(|pattern| {
            let literal = literal_prefix(pattern);
            let related = |d: &str| literal.starts_with(d) || d.starts_with(&literal);
            related(&dir_match) || (is_symlink && related(&dir_match_normalized))
        });
    }
    if kept.is_empty() {
        return Ok(None);
    }
    let pattern = format!("({})", kept.join("|"));
    pcre2::bytes::RegexBuilder::new()
        .build(&pattern)
        .map(Some)
        .map_err(|e| AutoloadError::Regex(e.to_string()))
}

/// `^(([^.+*?\[^\]$(){}=!<>|:\\#-]+|\\[.+*?\[^\]$(){}=!<>|:#-])*).*` → préfixe littéral.
fn literal_prefix(pattern: &str) -> String {
    let specials = ".+*?[^]$(){}=!<>|:\\#-";
    let bytes: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '\\' && i + 1 < bytes.len() && ".+*?[^]$(){}=!<>|:#-".contains(bytes[i + 1]) {
            out.push('\\');
            out.push(bytes[i + 1]);
            i += 2;
        } else if !specials.contains(c) {
            out.push(c);
            i += 1;
        } else {
            break;
        }
    }
    out
}

/// `getPathCode` : expression PHP (`$vendorDir . '/x'`, `$baseDir . '/y'`, absolu).
fn get_path_code(base_path: &str, vendor_path: &str, path: &str) -> String {
    let abs = normalize_path(&absolute(base_path, path));
    let (mut prefix, rel) = if abs == vendor_path || abs.starts_with(&format!("{vendor_path}/")) {
        (
            "$vendorDir . ".to_owned(),
            abs[vendor_path.len()..].to_owned(),
        )
    } else {
        let short = normalize_path(&find_shortest_path(base_path, &abs));
        if short.starts_with('/') {
            (String::new(), short)
        } else {
            ("$baseDir . ".to_owned(), format!("/{short}"))
        }
    };
    if rel.contains(".phar/") || rel.ends_with(".phar") {
        prefix = format!("'phar://' . {prefix}");
    }
    format!("{prefix}{}", php_str(&rel))
}

/// Valeur absolue d'un chemin d'autoload (ce que PHP obtient en évaluant le
/// code de getPathCode), pour le fichier static.
fn absolute_value(base_path: &str, vendor_path: &str, path: &str) -> String {
    let abs = normalize_path(&absolute(base_path, path));
    let value = if abs == vendor_path || abs.starts_with(&format!("{vendor_path}/")) {
        abs
    } else {
        let short = normalize_path(&find_shortest_path(base_path, &abs));
        if short.starts_with('/') {
            short
        } else {
            format!("{base_path}/{short}")
        }
    };
    if value.contains(".phar/") || value.ends_with(".phar") {
        format!("phar://{value}")
    } else {
        value
    }
}

fn build_static_file(
    suffix: &str,
    autoloads: &Autoloads,
    class_map: &BTreeMap<Vec<u8>, String>,
    base_path: &str,
    vendor_path: &str,
    target_path: &str,
) -> Vec<u8> {
    let abs = |p: &str| absolute_value(base_path, vendor_path, p);
    let vendor_code = find_shortest_path_code(target_path, vendor_path, true);
    let base_code = find_shortest_path_code(target_path, base_path, true);
    let substitutions = vec![
        (
            format!(" => '{vendor_path}/"),
            format!(" => {vendor_code} . '/"),
        ),
        (
            format!(" => 'phar://{vendor_path}/"),
            format!(" => 'phar://' . {vendor_code} . '/"),
        ),
        (
            format!(" => '{base_path}/"),
            format!(" => {base_code} . '/"),
        ),
        (
            format!(" => 'phar://{base_path}/"),
            format!(" => 'phar://' . {base_code} . '/"),
        ),
    ];

    let list = |paths: &[String]| {
        PhpVal::Arr(
            paths
                .iter()
                .enumerate()
                .map(|(i, p)| (PhpKey::Int(i as i64), PhpVal::str(&abs(p))))
                .collect(),
        )
    };

    // Reconstruction de l'état de ClassLoader.
    let mut files: Vec<(PhpKey, PhpVal)> = Vec::new();
    for (id, p) in &autoloads.files {
        files.push((PhpKey::str(id), PhpVal::str(&abs(p))));
    }
    let mut prefix_lengths: Vec<(String, Vec<(PhpKey, PhpVal)>)> = Vec::new();
    let mut prefix_dirs: Vec<(PhpKey, PhpVal)> = Vec::new();
    let mut fallback_psr4: Vec<String> = Vec::new();
    for (ns, paths) in &autoloads.psr4 {
        if ns.is_empty() {
            fallback_psr4.extend(paths.iter().cloned());
            continue;
        }
        let first = ns.chars().next().unwrap_or('_').to_string();
        let entry = (PhpKey::str(ns), PhpVal::Int(ns.len() as i64));
        match prefix_lengths.iter_mut().find(|(c, _)| *c == first) {
            Some((_, v)) => v.push(entry),
            None => prefix_lengths.push((first, vec![entry])),
        }
        prefix_dirs.push((PhpKey::str(ns), list(paths)));
    }
    let mut prefixes_psr0: Vec<(String, Vec<(PhpKey, PhpVal)>)> = Vec::new();
    let mut fallback_psr0: Vec<String> = Vec::new();
    for (ns, paths) in &autoloads.psr0 {
        if ns.is_empty() {
            fallback_psr0.extend(paths.iter().cloned());
            continue;
        }
        let first = ns.chars().next().unwrap_or('_').to_string();
        let entry = (PhpKey::str(ns), list(paths));
        match prefixes_psr0.iter_mut().find(|(c, _)| *c == first) {
            Some((_, v)) => v.push(entry),
            None => prefixes_psr0.push((first, vec![entry])),
        }
    }
    let nested = |v: Vec<(String, Vec<(PhpKey, PhpVal)>)>| {
        PhpVal::Arr(
            v.into_iter()
                .map(|(c, e)| (PhpKey::str(&c), PhpVal::Arr(e)))
                .collect(),
        )
    };

    let mut props: Vec<(&str, PhpVal)> = Vec::new();
    if !files.is_empty() {
        props.push(("files", PhpVal::Arr(files)));
    }
    if !prefix_lengths.is_empty() {
        props.push(("prefixLengthsPsr4", nested(prefix_lengths)));
    }
    if !prefix_dirs.is_empty() {
        props.push(("prefixDirsPsr4", PhpVal::Arr(prefix_dirs)));
    }
    if !fallback_psr4.is_empty() {
        props.push(("fallbackDirsPsr4", list(&fallback_psr4)));
    }
    if !prefixes_psr0.is_empty() {
        props.push(("prefixesPsr0", nested(prefixes_psr0)));
    }
    if !fallback_psr0.is_empty() {
        props.push(("fallbackDirsPsr0", list(&fallback_psr0)));
    }
    if !class_map.is_empty() {
        props.push((
            "classMap",
            PhpVal::Arr(
                class_map
                    .iter()
                    .map(|(c, p)| (PhpKey::Str(c.clone()), PhpVal::str(&abs(p))))
                    .collect(),
            ),
        ));
    }
    let mut properties: Vec<u8> = Vec::new();
    let mut initializer: Vec<&str> = Vec::new();
    for (name, value) in &props {
        properties.extend_from_slice(&templates::static_property(name, value, &substitutions));
        if *name != "files" {
            initializer.push(name);
        }
    }
    templates::autoload_static_php(suffix, &properties, &initializer)
}

fn is_ignored_req(name: &str, ignored: &[String]) -> bool {
    ignored.iter().any(|pat| {
        let pat = pat.strip_suffix('+').unwrap_or(pat);
        pat == "*" || pat == name || pat.strip_suffix('*').is_some_and(|p| name.starts_with(p))
    })
}

fn platform_check(
    packages: &[Pkg],
    dev_names: &BTreeSet<String>,
    opts: &DumpOptions,
) -> Option<String> {
    use vivace_core::constraint::{Constraint, LowerBound};
    let mut lowest: Option<LowerBound> = None;
    let mut php64 = false;
    let mut ext_lines: BTreeMap<String, String> = BTreeMap::new();

    let mut providers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in packages {
        for (target, constraint) in &p.provides_and_replaces {
            if let Some(ext) = target.strip_prefix("ext-") {
                providers
                    .entry(ext.to_ascii_lowercase())
                    .or_default()
                    .push(constraint.clone());
            }
        }
    }
    for p in packages {
        if dev_names.contains(&p.name) {
            continue;
        }
        for (target, constraint) in &p.requires {
            if is_ignored_req(target, &opts.ignored_platform_reqs) {
                continue;
            }
            let Some(c) = constraint.as_str() else {
                continue;
            };
            if target == "php" || target == "php-64bit" {
                if let Ok(parsed) = Constraint::parse(c) {
                    if let Some(lb) = parsed.lower_bound() {
                        let higher = match &lowest {
                            None => true,
                            Some(cur) => {
                                lb.version > cur.version
                                    || (lb.version == cur.version && !lb.inclusive && cur.inclusive)
                            }
                        };
                        if higher {
                            lowest = Some(lb);
                        }
                    }
                }
            }
            if target == "php-64bit" {
                php64 = true;
            }
            if opts.platform_check == PlatformCheckMode::Full {
                if let Some(ext) = target.strip_prefix("ext-") {
                    let ext = ext.to_ascii_lowercase();
                    // Un fournisseur (replace/provide ext-*) couvre l'extension.
                    if providers.contains_key(&ext) {
                        continue;
                    }
                    let ext_name = if ext == "zend-opcache" {
                        "zend opcache".to_owned()
                    } else {
                        ext.clone()
                    };
                    let exported = php_str(&ext_name);
                    let line = if ext_name == "pcntl" || ext_name == "readline" {
                        format!("PHP_SAPI !== 'cli' || extension_loaded({exported}) || $missingExtensions[] = {exported};\n")
                    } else {
                        format!(
                            "extension_loaded({exported}) || $missingExtensions[] = {exported};\n"
                        )
                    };
                    ext_lines.insert(exported, line);
                }
            }
        }
    }
    let php = lowest.map(|lb| {
        let v = &lb.version;
        let id = v.parts[0] as i64 * 10000 + v.parts[1] as i64 * 100 + v.parts[2] as i64;
        let human = format!("{}.{}.{}", v.parts[0], v.parts[1], v.parts[2]);
        (id, if lb.inclusive { ">=" } else { ">" }, human)
    });
    templates::platform_check_php(&templates::PlatformCheckParts {
        php,
        php_64bit: php64,
        extension_lines: ext_lines.into_values().collect(),
    })
}
