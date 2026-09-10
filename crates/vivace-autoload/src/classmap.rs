//! Port de `composer/class-map-generator` (docs/reference/cmg-*.php) :
//! - `find_classes` : pré-nettoyage (PhpFileCleaner — strings, commentaires,
//!   heredocs remplacés) puis LE pattern PCRE de PhpFileParser, exécuté par
//!   pcre2 (possessifs, lookbehind, octets `\x7f-\xff` : hors de portée du
//!   crate `regex`, décision plan r1/F4) ;
//! - `Scanner` : parcours à la Symfony Finder (extensions php/inc/hh, dot-files
//!   et répertoires VCS ignorés, symlinks suivis), filtre PSR-0/PSR-4,
//!   exclusion par regex, dédoublonnage par realpath, ambiguïtés « le premier
//!   gagne ».
//!
//! Les noms de classes sont des OCTETS bruts, comme chez PHP : symfony/cache
//! déclare une classe nommée d'un seul octet non-UTF-8, que Composer écrit
//! tel quel dans la classmap.
//!
//! Composer passe d'abord par `php_strip_whitespace()` (tokenizer PHP) : nous
//! ne l'avons pas, donc le cleaner gère en plus les commentaires `#` (hors
//! attributs `#[`), seule différence observable pour la détection de classes.
//! Parité tenue par tests/oracle_classmap.rs (findClasses du phar sur tous les
//! fichiers des fixtures).

use crate::pathutil::normalize_path;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ClassMapError {
    #[error("regex interne invalide: {0}")]
    Regex(String),
    #[error("lecture impossible de {0}")]
    Read(PathBuf),
    #[error(
        "Could not scan for classes inside \"{0}\" which does not appear to be a file nor a folder"
    )]
    MissingPath(String),
}

const TYPE_WORDS: [&str; 4] = ["class", "interface", "trait", "enum"];

/// Nettoyage minimal : remplace strings/heredocs par `null`, supprime les
/// commentaires, s'arrête tôt quand un seul type est attendu (maxMatches == 1).
fn clean(contents: &[u8], max_matches: usize, type_pattern: &pcre2::bytes::Regex) -> Vec<u8> {
    let len = contents.len();
    let mut out: Vec<u8> = Vec::with_capacity(len);
    let mut i = 0usize;
    let peek = |i: usize, c: u8| i + 1 < len && contents[i + 1] == c;

    while i < len {
        // skipToPhp
        while i < len {
            if contents[i] == b'<' && peek(i, b'?') {
                i += 2;
                break;
            }
            i += 1;
        }
        out.extend_from_slice(b"<?");
        'php: while i < len {
            let c = contents[i];
            if c == b'?' && peek(i, b'>') {
                out.extend_from_slice(b"?>");
                i += 2;
                break 'php;
            }
            if c == b'"' || c == b'\'' {
                i = skip_string(contents, i, c);
                out.extend_from_slice(b"null");
                continue;
            }
            if c == b'<' && peek(i, b'<') {
                if let Some((delim, end)) = heredoc_start(contents, i) {
                    i = skip_heredoc(contents, end, &delim);
                    out.extend_from_slice(b"null");
                    continue;
                }
            }
            if c == b'/' {
                if peek(i, b'/') {
                    i = skip_to_newline(contents, i);
                    continue;
                }
                if peek(i, b'*') {
                    i = skip_comment(contents, i);
                    continue;
                }
            }
            // `#` : commentaire de ligne, sauf attribut `#[` (PHP 8) — rôle
            // de php_strip_whitespace chez Composer.
            if c == b'#' && !peek(i, b'[') {
                i = skip_to_newline(contents, i);
                continue;
            }
            if max_matches == 1 && matches!(c, b'c' | b'i' | b't' | b'e') {
                for word in TYPE_WORDS {
                    if contents[i..].starts_with(word.as_bytes()) {
                        // pattern ancré à index-1 : `.\b(?<![\$:>])type\s++name`
                        let start = i.saturating_sub(1);
                        if let Ok(Some(m)) = type_pattern.find_at(contents, start) {
                            if m.start() == start {
                                out.extend_from_slice(&contents[m.start()..m.end()]);
                                return out;
                            }
                        }
                    }
                }
            }
            i += 1;
            // strcspn sur les caractères de rejet
            let mut skip = 0;
            while i + skip < len
                && !matches!(
                    contents[i + skip],
                    b'?' | b'"' | b'\'' | b'<' | b'/' | b'#' | b'c' | b'i' | b't' | b'e'
                )
            {
                skip += 1;
            }
            out.push(c);
            if skip > 0 {
                out.extend_from_slice(&contents[i..i + skip]);
                i += skip;
            }
        }
    }
    out
}

fn skip_string(contents: &[u8], mut i: usize, delim: u8) -> usize {
    let len = contents.len();
    i += 1;
    while i < len {
        let c = contents[i];
        if c == b'\\' && i + 1 < len && (contents[i + 1] == b'\\' || contents[i + 1] == delim) {
            i += 2;
            continue;
        }
        if c == delim {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn skip_comment(contents: &[u8], mut i: usize) -> usize {
    let len = contents.len();
    i += 2;
    while i < len {
        if contents[i] == b'*' && i + 1 < len && contents[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    i
}

fn skip_to_newline(contents: &[u8], mut i: usize) -> usize {
    while i < contents.len() && contents[i] != b'\n' && contents[i] != b'\r' {
        i += 1;
    }
    i
}

/// `<<<[ \t]*(['"]?)(ident)\1(\r\n|\n|\r)` ancré à i → (délimiteur, index après).
fn heredoc_start(contents: &[u8], i: usize) -> Option<(Vec<u8>, usize)> {
    if !contents[i..].starts_with(b"<<<") {
        return None;
    }
    let mut j = i + 3;
    while j < contents.len() && (contents[j] == b' ' || contents[j] == b'\t') {
        j += 1;
    }
    let quote = match contents.get(j) {
        Some(b'\'') | Some(b'"') => {
            let q = contents[j];
            j += 1;
            Some(q)
        }
        _ => None,
    };
    let start = j;
    let is_ident_start = |c: u8| c.is_ascii_alphabetic() || c == b'_' || c >= 0x80;
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    if !contents.get(j).copied().is_some_and(is_ident_start) {
        return None;
    }
    while contents.get(j).copied().is_some_and(is_ident) {
        j += 1;
    }
    let delim = contents[start..j].to_vec();
    if let Some(q) = quote {
        if contents.get(j) != Some(&q) {
            return None;
        }
        j += 1;
    }
    match contents.get(j) {
        Some(b'\n') => Some((delim, j + 1)),
        Some(b'\r') => {
            if contents.get(j + 1) == Some(&b'\n') {
                Some((delim, j + 2))
            } else {
                Some((delim, j + 1))
            }
        }
        _ => None,
    }
}

fn skip_heredoc(contents: &[u8], mut i: usize, delim: &[u8]) -> usize {
    let len = contents.len();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    while i < len {
        match contents[i] {
            b'\t' | b' ' => {
                i += 1;
                continue;
            }
            c if c == delim[0]
                && contents[i..].starts_with(delim)
                && !contents.get(i + delim.len()).copied().is_some_and(is_ident) =>
            {
                return i + delim.len();
            }
            _ => {}
        }
        i = skip_to_newline(contents, i);
        while i < len && (contents[i] == b'\n' || contents[i] == b'\r') {
            i += 1;
        }
    }
    i
}

pub struct ClassFinder {
    quick: pcre2::bytes::Regex,
    type_anchor: pcre2::bytes::Regex,
    main: pcre2::bytes::Regex,
}

impl ClassFinder {
    pub fn new() -> Result<ClassFinder, ClassMapError> {
        let build = |p: &str, extended: bool| {
            pcre2::bytes::RegexBuilder::new()
                .caseless(true)
                .extended(extended)
                .build(p)
                .map_err(|e| ClassMapError::Regex(e.to_string()))
        };
        Ok(ClassFinder {
            quick: build(r"\b(?:class|interface|trait|enum)\s", false)?,
            type_anchor: pcre2::bytes::RegexBuilder::new()
                .caseless(true)
                .dotall(true)
                .build(r".\b(?<![\$:>])(?:class|interface|trait|enum)\s++[a-zA-Z_\x7f-\xff:][a-zA-Z0-9_\x7f-\xff:\-]*+")
                .map_err(|e| ClassMapError::Regex(e.to_string()))?,
            main: build(
                r"(?:
                 \b(?<![\\$:>])(?P<type>class|interface|trait|enum) \s++ (?P<name>[a-zA-Z_\x7f-\xff:][a-zA-Z0-9_\x7f-\xff:\-]*+)
               | \b(?<![\\$:>])(?P<ns>namespace) (?P<nsname>\s++[a-zA-Z_\x7f-\xff][a-zA-Z0-9_\x7f-\xff]*+(?:\s*+\\\s*+[a-zA-Z_\x7f-\xff][a-zA-Z0-9_\x7f-\xff]*+)*+)? \s*+ [\{;]
            )",
                true,
            )?,
        })
    }

    /// `PhpFileParser::findClasses` sur un contenu déjà lu (noms en octets).
    pub fn find_classes(&self, contents: &[u8]) -> Result<Vec<Vec<u8>>, ClassMapError> {
        if contents.iter().all(u8::is_ascii_whitespace) {
            return Ok(Vec::new());
        }
        let quick_count = self
            .quick
            .find_iter(contents)
            .filter_map(Result::ok)
            .count();
        if quick_count == 0 {
            return Ok(Vec::new());
        }
        let cleaned = clean(contents, quick_count, &self.type_anchor);

        let mut classes: Vec<Vec<u8>> = Vec::new();
        let mut namespace: Vec<u8> = Vec::new();
        for caps in self.main.captures_iter(&cleaned) {
            let caps = caps.map_err(|e| ClassMapError::Regex(e.to_string()))?;
            if let Some(ns) = caps.name("ns") {
                if !ns.as_bytes().is_empty() {
                    let nsname = caps.name("nsname").map(|m| m.as_bytes()).unwrap_or(b"");
                    namespace = nsname
                        .iter()
                        .copied()
                        .filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                        .collect();
                    namespace.push(b'\\');
                    continue;
                }
            }
            let Some(name_m) = caps.name("name") else {
                continue;
            };
            let name = name_m.as_bytes();
            if name == b"extends" || name == b"implements" {
                continue;
            }
            let is_enum = caps
                .name("type")
                .is_some_and(|m| m.as_bytes().eq_ignore_ascii_case(b"enum"));
            let name: Vec<u8> = if name.first() == Some(&b':') {
                let mut out = b"xhp".to_vec();
                for &b in &name[1..] {
                    match b {
                        b'-' => out.push(b'_'),
                        b':' => out.extend_from_slice(b"__"),
                        b => out.push(b),
                    }
                }
                out
            } else if is_enum {
                match name.iter().rposition(|b| *b == b':') {
                    Some(pos) => name[..pos].to_vec(),
                    None => name.to_vec(),
                }
            } else {
                name.to_vec()
            };
            let mut class_name = namespace.clone();
            class_name.extend_from_slice(&name);
            let start = class_name
                .iter()
                .position(|b| *b != b'\\')
                .unwrap_or(class_name.len());
            classes.push(class_name[start..].to_vec());
        }
        Ok(classes)
    }
}

/// Détection en parallèle (threads scoped, un `ClassFinder` par thread —
/// les regex pcre2 ne se partagent pas), résultats dans l'ordre d'entrée
/// pour préserver « le premier gagne ».
fn find_all_parallel(
    todo: &[(PathBuf, PathBuf, Vec<u8>)],
) -> Result<Vec<Vec<Vec<u8>>>, ClassMapError> {
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(todo.len())
        .max(1);
    let chunk_size = todo.len().div_ceil(threads);
    let results: Vec<Result<Vec<Vec<Vec<u8>>>, ClassMapError>> = std::thread::scope(|s| {
        let handles: Vec<_> = todo
            .chunks(chunk_size)
            .map(|chunk| {
                s.spawn(move || {
                    let finder = ClassFinder::new()?;
                    chunk
                        .iter()
                        .map(|(_, _, contents)| finder.find_classes(contents))
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    Err(ClassMapError::Regex("thread de scan interrompu".into()))
                })
            })
            .collect()
    });
    let mut out = Vec::with_capacity(todo.len());
    for r in results {
        out.extend(r?);
    }
    Ok(out)
}

/// Version du format/algorithme de scan : à incrémenter dès que la détection
/// change, pour invalider les caches existants.
const CACHE_FORMAT: &str = "v1";

/// Emplacement de cache pour le scan d'un répertoire d'une entrée de store :
/// clé = entrée (nom/version/ref) + sous-répertoire relatif + version du format.
pub struct CacheSlot {
    file: PathBuf,
}

impl CacheSlot {
    pub fn new(cache_root: &Path, store_entry: &Path, rel_subdir: &Path) -> CacheSlot {
        use sha1::{Digest, Sha1};
        let mut h = Sha1::new();
        h.update(CACHE_FORMAT.as_bytes());
        h.update(b"\0");
        h.update(store_entry.to_string_lossy().as_bytes());
        h.update(b"\0");
        h.update(rel_subdir.to_string_lossy().as_bytes());
        let key: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        CacheSlot {
            file: cache_root.join("classmap").join(format!("{key}.json")),
        }
    }

    /// Entrées (chemin relatif, classes brutes) dans l'ordre de parcours.
    fn load(&self) -> Option<Vec<(PathBuf, Vec<Vec<u8>>)>> {
        use base64::Engine as _;
        let text = std::fs::read(&self.file).ok()?;
        let raw: Vec<(String, Vec<String>)> = serde_json::from_slice(&text).ok()?;
        let engine = base64::engine::general_purpose::STANDARD;
        let mut out = Vec::with_capacity(raw.len());
        for (rel, classes) in raw {
            let mut decoded = Vec::with_capacity(classes.len());
            for c in classes {
                decoded.push(engine.decode(c).ok()?);
            }
            out.push((PathBuf::from(rel), decoded));
        }
        Some(out)
    }

    fn store(&self, base: &Path, files: &[(PathBuf, PathBuf, Vec<Vec<u8>>)]) {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        let mut raw: Vec<(String, Vec<String>)> = Vec::with_capacity(files.len());
        for (file, _, classes) in files {
            let Ok(rel) = file.strip_prefix(base) else {
                return; // hors de la base : on ne cache pas
            };
            raw.push((
                rel.to_string_lossy().into_owned(),
                classes.iter().map(|c| engine.encode(c)).collect(),
            ));
        }
        let Ok(json) = serde_json::to_vec(&raw) else {
            return;
        };
        if let Some(parent) = self.file.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoloadType {
    ClassMap,
    Psr0,
    Psr4,
}

#[derive(Default)]
pub struct ClassMap {
    /// classe (octets bruts) → chemin normalisé ; BTreeMap = ksort (octets).
    pub map: BTreeMap<Vec<u8>, String>,
    pub ambiguous: BTreeMap<Vec<u8>, Vec<String>>,
    /// (message, classe, chemin)
    pub psr_violations: Vec<(String, Vec<u8>, String)>,
}

pub struct Scanner {
    pub class_map: ClassMap,
    scanned: BTreeSet<PathBuf>,
}

const VCS_DIRS: [&str; 9] = [
    ".svn",
    "_svn",
    "CVS",
    "_darcs",
    ".arch-params",
    ".monotone",
    ".bzr",
    ".git",
    ".hg",
];

impl Scanner {
    pub fn new() -> Result<Scanner, ClassMapError> {
        ClassFinder::new()?; // valide les regex tôt
        Ok(Scanner {
            class_map: ClassMap::default(),
            scanned: BTreeSet::new(),
        })
    }

    pub fn add_class(&mut self, class: &[u8], path: &str) {
        self.class_map.map.insert(class.to_vec(), path.to_owned());
    }

    /// `scanPaths($path, $excluded, $autoloadType, $namespace)` ; `path`
    /// absolu (fichier ou répertoire). Un chemin absent est une erreur pour
    /// une règle classmap (comme Composer) ; les répertoires PSR absents sont
    /// filtrés en amont par l'appelant. Les fichiers sont visités dans l'ordre
    /// lexicographique — Composer suit l'ordre du système de fichiers, ce qui
    /// n'affecte que le gagnant d'une ambiguïté.
    pub fn scan_path(
        &mut self,
        path: &Path,
        excluded: Option<&pcre2::bytes::Regex>,
        autoload_type: AutoloadType,
        namespace: &str,
    ) -> Result<(), ClassMapError> {
        self.scan_path_cached(path, excluded, autoload_type, namespace, None)
    }

    /// `scan_path` avec, pour un répertoire d'une entrée de store (immuable),
    /// un cache des classes brutes par fichier : la lecture et la détection
    /// sont sautées, tout le reste (exclusions, dédoublonnage, filtre PSR,
    /// ambiguïtés) est rejoué à l'identique.
    pub fn scan_path_cached(
        &mut self,
        path: &Path,
        excluded: Option<&pcre2::bytes::Regex>,
        autoload_type: AutoloadType,
        namespace: &str,
        cache: Option<&CacheSlot>,
    ) -> Result<(), ClassMapError> {
        let base_path = normalize_path(&path.to_string_lossy());

        // (chemin, chemin réel, classes brutes) dans l'ordre de parcours.
        let mut scanned_files: Vec<(PathBuf, PathBuf, Vec<Vec<u8>>)> = Vec::new();

        let cached = cache.and_then(|c| c.load());
        if let Some(entries) = cached {
            let base_real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            for (rel, classes) in entries {
                scanned_files.push((path.join(&rel), base_real.join(&rel), classes));
            }
        } else {
            let (files, saw_symlink) = self.collect_files(path)?;
            // Lecture séquentielle (la lecture parallèle est plus lente sur
            // APFS), détection en parallèle sur le CPU.
            let mut todo: Vec<(PathBuf, PathBuf, Vec<u8>)> = Vec::new();
            for (file, real) in files {
                let contents =
                    std::fs::read(&file).map_err(|_| ClassMapError::Read(file.clone()))?;
                todo.push((file, real, contents));
            }
            let found = find_all_parallel(&todo)?;
            for ((file, real, _), classes) in todo.into_iter().zip(found) {
                scanned_files.push((file, real, classes));
            }
            if let Some(c) = cache {
                if !saw_symlink {
                    c.store(path, &scanned_files);
                }
            }
        }

        for (file, real, classes) in scanned_files {
            let file_path = normalize_path(&file.to_string_lossy());
            if self.scanned.contains(&real) {
                continue;
            }
            if let Some(re) = excluded {
                let real_s = real.to_string_lossy();
                if re.is_match(real_s.as_bytes()).unwrap_or(false)
                    || re.is_match(file_path.as_bytes()).unwrap_or(false)
                {
                    continue;
                }
            }
            let mut classes = classes;
            if autoload_type != AutoloadType::ClassMap {
                classes = self.filter_by_namespace(
                    classes,
                    &file_path,
                    namespace,
                    autoload_type,
                    &base_path,
                );
                if !classes.is_empty() {
                    self.scanned.insert(real);
                }
            } else {
                self.scanned.insert(real);
            }
            for class in classes {
                if let Some(existing) = self.class_map.map.get(&class) {
                    if existing != &file_path {
                        self.class_map
                            .ambiguous
                            .entry(class)
                            .or_default()
                            .push(file_path.clone());
                    }
                } else {
                    self.class_map.map.insert(class, file_path.clone());
                }
            }
        }
        Ok(())
    }

    /// Parcours à la Finder : (chemin, chemin réel) des fichiers php/inc/hh,
    /// et si un symlink a été traversé (le cache est alors désactivé).
    fn collect_files(&self, path: &Path) -> Result<(Vec<(PathBuf, PathBuf)>, bool), ClassMapError> {
        let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut saw_symlink = false;
        if path.is_file() {
            let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            files.push((path.to_path_buf(), real));
        } else if path.is_dir() {
            let base_real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let mut symlinked_dirs: Vec<PathBuf> = Vec::new();
            for entry in walkdir::WalkDir::new(path)
                .follow_links(true)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|e| {
                    if e.depth() == 0 {
                        return true;
                    }
                    let name = e.file_name().to_string_lossy();
                    !(name.starts_with('.') || VCS_DIRS.contains(&name.as_ref()))
                })
                .filter_map(Result::ok)
            {
                if entry.path_is_symlink() {
                    saw_symlink = true;
                }
                if entry.file_type().is_dir() {
                    if entry.path_is_symlink() {
                        symlinked_dirs.push(entry.path().to_path_buf());
                    }
                    continue;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let p = entry.into_path();
                let ext = p
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !matches!(ext.as_str(), "php" | "inc" | "hh") {
                    continue;
                }
                let under_symlink = symlinked_dirs.iter().any(|d| p.starts_with(d));
                let real = if under_symlink {
                    std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone())
                } else {
                    match p.strip_prefix(path) {
                        Ok(rel) => base_real.join(rel),
                        Err(_) => std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone()),
                    }
                };
                files.push((p, real));
            }
        } else {
            return Err(ClassMapError::MissingPath(
                path.to_string_lossy().into_owned(),
            ));
        }
        Ok((files, saw_symlink))
    }

    fn filter_by_namespace(
        &mut self,
        classes: Vec<Vec<u8>>,
        file_path: &str,
        base_namespace: &str,
        autoload_type: AutoloadType,
        base_path: &str,
    ) -> Vec<Vec<u8>> {
        let mut valid = Vec::new();
        let mut rejected = Vec::new();
        let real_sub = file_path.get(base_path.len() + 1..).unwrap_or("");
        let real_sub = match real_sub.rfind('.') {
            Some(p) => &real_sub[..p],
            None => real_sub,
        };
        let base_ns = base_namespace.as_bytes();
        let map_sep = |bytes: &[u8], from: u8| -> Vec<u8> {
            bytes
                .iter()
                .map(|b| if *b == from { b'/' } else { *b })
                .collect()
        };
        for class in classes {
            let sub_path: Vec<u8> = match autoload_type {
                AutoloadType::Psr0 => {
                    if !base_ns.is_empty() && !class.starts_with(base_ns) {
                        rejected.push(class);
                        continue;
                    }
                    match class.iter().rposition(|b| *b == b'\\') {
                        Some(pos) => {
                            let mut v = map_sep(&class[..=pos], b'\\');
                            v.extend(map_sep(&class[pos + 1..], b'_'));
                            v
                        }
                        None => map_sep(&class, b'_'),
                    }
                }
                AutoloadType::Psr4 => {
                    let sub_ns = if base_ns.is_empty() {
                        &class[..]
                    } else {
                        class.get(base_ns.len()..).unwrap_or(b"")
                    };
                    map_sep(sub_ns, b'\\')
                }
                AutoloadType::ClassMap => unreachable!("filtré en amont"),
            };
            if sub_path == real_sub.as_bytes() {
                valid.push(class);
            } else {
                rejected.push(class);
            }
        }
        if valid.is_empty() {
            for class in rejected {
                self.class_map.psr_violations.push((
                    format!(
                        "Class {} located in {file_path} does not comply with {} autoloading standard (rule: {base_namespace} => {base_path}). Skipping.",
                        String::from_utf8_lossy(&class),
                        match autoload_type {
                            AutoloadType::Psr0 => "psr-0",
                            _ => "psr-4",
                        }
                    ),
                    class.clone(),
                    file_path.to_owned(),
                ));
            }
            return Vec::new();
        }
        valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(src: &str) -> Vec<String> {
        ClassFinder::new()
            .expect("regex")
            .find_classes(src.as_bytes())
            .expect("find")
            .into_iter()
            .map(|c| String::from_utf8_lossy(&c).into_owned())
            .collect()
    }

    #[test]
    fn finds_namespaced_types_and_skips_noise() {
        let src = r#"<?php
namespace Foo\Bar;
// class NotMe
/* class NorMe */
# class NotEither
$s = "class InString"; $t = 'class InSingle';
$h = <<<EOT
class InHeredoc
EOT;
abstract class Baz extends \Other {}
interface Qux {}
trait T1 {}
enum Suit: string { case A = 'a'; }
final class Last implements Qux {}
"#;
        assert_eq!(
            classes(src),
            vec![
                "Foo\\Bar\\Baz",
                "Foo\\Bar\\Qux",
                "Foo\\Bar\\T1",
                "Foo\\Bar\\Suit",
                "Foo\\Bar\\Last"
            ]
        );
    }

    #[test]
    fn attribute_is_not_a_comment_and_braced_namespace_works() {
        let src = "<?php\nnamespace A { #[Attr]\nclass X {} }\nnamespace B;\nclass Y {}\n";
        assert_eq!(classes(src), vec!["A\\X", "B\\Y"]);
    }

    #[test]
    fn variable_and_static_uses_are_not_declarations() {
        let src = "<?php\n$x = new class {};\nFoo::class;\n$this->class = 1;\nclass Real {}\n";
        assert_eq!(classes(src), vec!["Real"]);
    }

    #[test]
    fn raw_bytes_are_preserved() {
        let src = b"<?php\nclass \x7f {}\nclass \xc3\xa9t\xc3\xa9 {}\n";
        let found = ClassFinder::new()
            .expect("regex")
            .find_classes(src)
            .expect("find");
        assert_eq!(found, vec![vec![0x7f], b"\xc3\xa9t\xc3\xa9".to_vec()]);
    }

    #[test]
    fn no_php_tag_means_nothing() {
        assert!(classes("class Foo {}").is_empty());
        assert!(classes("   \n").is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn write(p: &Path, content: &[u8]) {
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, content).expect("write");
    }

    /// Un scan servi par le cache doit produire exactement la même classmap
    /// (classes, chemins, ambiguïtés) qu'un scan direct — y compris pour les
    /// noms non-UTF-8 et les fichiers sans classe.
    #[test]
    fn cached_scan_equals_direct_scan() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pkg = tmp.path().join("store/acme/lib/1.0.0-abc");
        write(
            &pkg.join("src/A.php"),
            b"<?php\nnamespace Acme;\nclass A {}\n",
        );
        write(
            &pkg.join("src/Sub/B.php"),
            b"<?php\nnamespace Acme\\Sub;\nclass B {}\ninterface I {}\n",
        );
        write(&pkg.join("src/raw.php"), b"<?php\nclass \x7f {}\n");
        write(&pkg.join("src/nothing.php"), b"<?php\n// rien\n");
        write(
            &pkg.join("src/dup.php"),
            b"<?php\nnamespace Acme;\nclass A {}\n",
        );
        let cache_root = tmp.path().join("cache");
        let slot = CacheSlot::new(&cache_root, &pkg, Path::new("src"));

        let run = |slot: Option<&CacheSlot>| {
            let mut s = Scanner::new().expect("scanner");
            s.scan_path_cached(&pkg.join("src"), None, AutoloadType::ClassMap, "", slot)
                .expect("scan");
            (s.class_map.map, s.class_map.ambiguous)
        };
        let direct = run(None);
        let first = run(Some(&slot)); // remplit le cache
        assert!(slot.file.is_file(), "cache non écrit");
        let cached = run(Some(&slot)); // servi par le cache
        assert_eq!(direct, first);
        assert_eq!(direct, cached);
        assert_eq!(direct.0.len(), 4);
        assert!(direct.0.contains_key(&vec![0x7fu8]));
        assert_eq!(direct.1.len(), 1, "l'ambiguïté Acme\\A doit être rejouée");
    }

    #[test]
    fn symlinked_tree_is_not_cached() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pkg = tmp.path().join("pkg");
        write(&pkg.join("real/X.php"), b"<?php\nclass X {}\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(pkg.join("real"), pkg.join("link")).expect("ln");
        let slot = CacheSlot::new(tmp.path(), &pkg, Path::new(""));
        let mut s = Scanner::new().expect("scanner");
        s.scan_path_cached(&pkg, None, AutoloadType::ClassMap, "", Some(&slot))
            .expect("scan");
        #[cfg(unix)]
        assert!(
            !slot.file.exists(),
            "un arbre avec symlink ne doit pas être caché"
        );
    }
}
