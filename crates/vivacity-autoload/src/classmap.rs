//! Port of `composer/class-map-generator` (docs/reference/cmg-*.php):
//! - `find_classes`: pre-cleaning (PhpFileCleaner: strings, comments and
//!   heredocs replaced) then THE PCRE pattern of PhpFileParser, run by pcre2
//!   (possessive quantifiers, lookbehind, `\x7f-\xff` bytes: out of reach of
//!   the `regex` crate, plan decision r1/F4);
//! - `Scanner`: Symfony Finder-style walk (php/inc/hh extensions, dot-files
//!   and VCS directories ignored, symlinks followed), PSR-0/PSR-4 filter,
//!   regex exclusion, realpath deduplication, "first one wins" ambiguities.
//!
//! Class names are raw BYTES, as in PHP: symfony/cache declares a class named
//! with a single non-UTF-8 byte, which Composer writes as is into the
//! classmap.
//!
//! Composer first goes through `php_strip_whitespace()` (PHP tokenizer): we do
//! not have it, so the cleaner additionally handles `#` comments (except `#[`
//! attributes), the only observable difference for class detection. Parity is
//! held by tests/oracle_classmap.rs (the phar's findClasses on every fixture
//! file).

use crate::pathutil::normalize_path;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ClassMapError {
    #[error("invalid internal regex: {0}")]
    Regex(String),
    #[error("cannot read {0}")]
    Read(PathBuf),
    #[error(
        "Could not scan for classes inside \"{0}\" which does not appear to be a file nor a folder"
    )]
    MissingPath(String),
}

const TYPE_WORDS: [&str; 4] = ["class", "interface", "trait", "enum"];

/// Minimal cleaning: replaces strings/heredocs with `null`, strips comments,
/// stops early when a single type is expected (maxMatches == 1).
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
            // `#`: line comment, except `#[` attributes (PHP 8); this is the
            // role of php_strip_whitespace in Composer.
            if c == b'#' && !peek(i, b'[') {
                i = skip_to_newline(contents, i);
                continue;
            }
            if max_matches == 1 && matches!(c, b'c' | b'i' | b't' | b'e') {
                for word in TYPE_WORDS {
                    if contents[i..].starts_with(word.as_bytes()) {
                        // pattern anchored at index-1: `.\b(?<![\$:>])type\s++name`
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
            // strcspn over the reject characters
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

/// `<<<[ \t]*(['"]?)(ident)\1(\r\n|\n|\r)` anchored at i: (delimiter, index after).
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

    /// `PhpFileParser::findClasses` on already-read contents (names as bytes).
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

thread_local! {
    /// One `ClassFinder` per thread (compiled pcre2 regexes cannot be
    /// shared, and recompiling them per file is expensive) — reused from one
    /// scan to the next on the same rayon worker.
    static FINDER: std::cell::RefCell<Option<ClassFinder>> =
        const { std::cell::RefCell::new(None) };
}

/// `findClasses` through the current thread's finder (compiled on first
/// use).
fn find_one(contents: &[u8]) -> Result<Vec<Vec<u8>>, ClassMapError> {
    FINDER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(ClassFinder::new()?);
        }
        slot.as_ref().unwrap().find_classes(contents)
    })
}

/// Parallel detection on the rayon pool (one finder per thread), results in
/// input order to preserve "first one wins".
fn find_all(todo: &[(PathBuf, PathBuf, Vec<u8>)]) -> Result<Vec<Vec<Vec<u8>>>, ClassMapError> {
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    todo.par_iter()
        .map(|(_, _, contents)| find_one(contents))
        .collect()
}

/// Version of the scan format/algorithm: bump it whenever detection (or the
/// cache encoding) changes, to invalidate existing caches.
/// v2: length-prefixed binary encoding (replaces the JSON+base64).
const CACHE_FORMAT: &str = "v2";

/// Cache location for the scan of a store entry's directory:
/// key = entry (name/version/ref) + relative subdirectory + format version.
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
            file: cache_root.join("classmap").join(format!("{key}.bin")),
        }
    }

    /// Entries (relative path, raw classes) in walk order.
    ///
    /// Length-prefixed binary format (little-endian):
    ///   u32 file_count
    ///   [ u32 rel_len, rel(utf-8 bytes),
    ///     u32 class_count, [ u32 class_len, class(raw bytes) ]* ]*
    /// Class names are raw bytes (see the module header): no base64, no
    /// lossy round-trip — the bytes are written and read back as-is. The
    /// relative paths come from `to_string_lossy` (like the JSON version
    /// this replaces), so they are identical to what was written.
    fn load(&self) -> Option<Vec<(PathBuf, Vec<Vec<u8>>)>> {
        let bytes = std::fs::read(&self.file).ok()?;
        let mut r = ByteReader::new(&bytes);
        let n = r.u32()? as usize;
        // The counts come from the (possibly corrupt) file: clamp the
        // pre-allocations to what the remaining bytes could actually encode
        // (a file entry is ≥ 8 bytes, a class entry ≥ 4), so a garbage
        // count is caught by the truncation checks below instead of asking
        // the allocator for gigabytes. Exact for every well-formed file.
        let mut out = Vec::with_capacity(n.min(r.remaining() / 8));
        for _ in 0..n {
            let rel = r.slice()?;
            let rel = String::from_utf8(rel.to_vec()).ok()?;
            let nc = r.u32()? as usize;
            let mut classes = Vec::with_capacity(nc.min(r.remaining() / 4));
            for _ in 0..nc {
                classes.push(r.slice()?.to_vec());
            }
            out.push((PathBuf::from(rel), classes));
        }
        if !r.is_empty() {
            return None; // trailing bytes: corrupt file, rescan
        }
        Some(out)
    }

    fn store(&self, base: &Path, files: &[(PathBuf, PathBuf, Vec<Vec<u8>>)]) {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (file, _, classes) in files {
            let Ok(rel) = file.strip_prefix(base) else {
                return; // outside the base: do not cache
            };
            let rel = rel.to_string_lossy();
            push_slice(&mut buf, rel.as_bytes());
            buf.extend_from_slice(&(classes.len() as u32).to_le_bytes());
            for c in classes {
                push_slice(&mut buf, c);
            }
        }
        if let Some(parent) = self.file.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        // A unique temporary name: the same slot can be written by two
        // directories scanned in parallel (or by two processes); each
        // writer renames its own complete file over the slot.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = self.file.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        if std::fs::write(&tmp, &buf).is_ok() && std::fs::rename(&tmp, &self.file).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Appends `u32 len` then the bytes.
fn push_slice(buf: &mut Vec<u8>, s: &[u8]) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s);
}

/// Byte reader that fails cleanly (None) on any truncation.
struct ByteReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(bytes: &'a [u8]) -> ByteReader<'a> {
        ByteReader { bytes, pos: 0 }
    }
    fn u32(&mut self) -> Option<u32> {
        let end = self.pos.checked_add(4)?;
        let raw = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(u32::from_le_bytes(raw.try_into().ok()?))
    }
    fn slice(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        let end = self.pos.checked_add(len)?;
        let out = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
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
    /// class (raw bytes) -> normalized path; BTreeMap = ksort (bytes).
    pub map: BTreeMap<Vec<u8>, String>,
    pub ambiguous: BTreeMap<Vec<u8>, Vec<String>>,
    /// (message, class, path)
    pub psr_violations: Vec<(String, Vec<u8>, String)>,
}

pub struct Scanner {
    pub class_map: ClassMap,
    scanned: BTreeSet<PathBuf>,
}

/// `(path, real path, raw classes)` of one directory, in walk order — the
/// output of the pure `scan_only` phase.
pub type ScannedFiles = Vec<(PathBuf, PathBuf, Vec<Vec<u8>>)>;

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
        ClassFinder::new()?; // validates the regexes early
        Ok(Scanner {
            class_map: ClassMap::default(),
            scanned: BTreeSet::new(),
        })
    }

    pub fn add_class(&mut self, class: &[u8], path: &str) {
        self.class_map.map.insert(class.to_vec(), path.to_owned());
    }

    /// `scanPaths($path, $excluded, $autoloadType, $namespace)`; `path` is
    /// absolute (file or directory). A missing path is an error for a classmap
    /// rule (as in Composer); missing PSR directories are filtered out upstream
    /// by the caller. Files are visited in lexicographic order; Composer
    /// follows filesystem order, which only affects the winner of an ambiguity.
    pub fn scan_path(
        &mut self,
        path: &Path,
        excluded: Option<&pcre2::bytes::Regex>,
        autoload_type: AutoloadType,
        namespace: &str,
    ) -> Result<(), ClassMapError> {
        self.scan_path_cached(path, excluded, autoload_type, namespace, None)
    }

    /// `scan_path` with, for a (immutable) store entry directory, a cache of
    /// raw classes per file: reading and detection are skipped, everything
    /// else (exclusions, deduplication, PSR filter, ambiguities) is replayed
    /// identically.
    pub fn scan_path_cached(
        &mut self,
        path: &Path,
        excluded: Option<&pcre2::bytes::Regex>,
        autoload_type: AutoloadType,
        namespace: &str,
        cache: Option<&CacheSlot>,
    ) -> Result<(), ClassMapError> {
        let scanned_files = Scanner::scan_only(path, cache)?;
        self.merge_scanned(scanned_files, path, excluded, autoload_type, namespace)
    }

    /// The pure, parallelizable phase of a scan: produces `(path, real path,
    /// raw classes)` in walk order, served from the cache when present. Uses
    /// no `Scanner` state (deduplication/ambiguities/exclusions are applied
    /// by `merge_scanned`), so several directories can be scanned in
    /// parallel and then merged sequentially in order.
    pub fn scan_only(
        path: &Path,
        cache: Option<&CacheSlot>,
    ) -> Result<ScannedFiles, ClassMapError> {
        // (path, real path, raw classes) in walk order.
        let mut scanned_files: ScannedFiles = Vec::new();

        let cached = cache.and_then(|c| c.load());
        if let Some(entries) = cached {
            let base_real =
                vivacity_core::pathutil::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            for (rel, classes) in entries {
                scanned_files.push((path.join(&rel), base_real.join(&rel), classes));
            }
        } else {
            let (files, saw_symlink) = Scanner::collect_files(path)?;
            // Sequential reads (parallel reading is slower on APFS),
            // parallel detection on the CPU.
            let mut todo: Vec<(PathBuf, PathBuf, Vec<u8>)> = Vec::new();
            for (file, real) in files {
                let contents =
                    std::fs::read(&file).map_err(|_| ClassMapError::Read(file.clone()))?;
                todo.push((file, real, contents));
            }
            let found = find_all(&todo)?;
            for ((file, real, _), classes) in todo.into_iter().zip(found) {
                scanned_files.push((file, real, classes));
            }
            if let Some(c) = cache {
                if !saw_symlink {
                    c.store(path, &scanned_files);
                }
            }
        }
        Ok(scanned_files)
    }

    /// Merge phase (sequential, ordered): applies exclusions, realpath
    /// deduplication, the PSR filter and "first one wins" on ambiguities —
    /// the shared state that forces the order across directories.
    pub fn merge_scanned(
        &mut self,
        scanned_files: ScannedFiles,
        path: &Path,
        excluded: Option<&pcre2::bytes::Regex>,
        autoload_type: AutoloadType,
        namespace: &str,
    ) -> Result<(), ClassMapError> {
        let base_path = normalize_path(&path.to_string_lossy());
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

    /// Finder-style walk: (path, real path) of php/inc/hh files, and whether
    /// a symlink was traversed (the cache is then disabled).
    fn collect_files(path: &Path) -> Result<(Vec<(PathBuf, PathBuf)>, bool), ClassMapError> {
        let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut saw_symlink = false;
        if path.is_file() {
            let real =
                vivacity_core::pathutil::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            files.push((path.to_path_buf(), real));
        } else if path.is_dir() {
            let base_real =
                vivacity_core::pathutil::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
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
                    vivacity_core::pathutil::canonicalize(&p).unwrap_or_else(|_| p.clone())
                } else {
                    match p.strip_prefix(path) {
                        Ok(rel) => base_real.join(rel),
                        Err(_) => {
                            vivacity_core::pathutil::canonicalize(&p).unwrap_or_else(|_| p.clone())
                        }
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
                AutoloadType::ClassMap => unreachable!("filtered upstream"),
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

    /// A scan served from the cache must produce exactly the same classmap
    /// (classes, paths, ambiguities) as a direct scan, including for non-UTF-8
    /// names and files without any class.
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
        let first = run(Some(&slot)); // fills the cache
        assert!(slot.file.is_file(), "cache not written");
        let cached = run(Some(&slot)); // served from the cache
        assert_eq!(direct, first);
        assert_eq!(direct, cached);
        assert_eq!(direct.0.len(), 4);
        assert!(direct.0.contains_key(&vec![0x7fu8]));
        assert_eq!(direct.1.len(), 1, "the Acme\\A ambiguity must be replayed");
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
            "a tree with a symlink must not be cached"
        );
    }

    /// A corrupt cache file — truncated, trailing garbage, or a garbage
    /// count field claiming billions of entries — must be ignored (rescan),
    /// never panic, never abort in the allocator, never return a partial
    /// classmap.
    #[test]
    fn corrupt_cache_is_rescanned() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pkg = tmp.path().join("store/acme/lib/1.0.0-abc");
        write(
            &pkg.join("src/A.php"),
            b"<?php\nnamespace Acme;\nclass A {}\n",
        );
        let slot = CacheSlot::new(&tmp.path().join("cache"), &pkg, Path::new("src"));

        let run = |slot: Option<&CacheSlot>| {
            let mut s = Scanner::new().expect("scanner");
            s.scan_path_cached(&pkg.join("src"), None, AutoloadType::ClassMap, "", slot)
                .expect("scan");
            (s.class_map.map, s.class_map.ambiguous)
        };
        let direct = run(None);
        run(Some(&slot)); // fills the cache
        let valid = std::fs::read(&slot.file).expect("cache bytes");
        assert!(slot.load().is_some(), "sanity: the valid cache loads");

        // count=1, rel="a", then an absurd class count (and no class bytes).
        let mut huge_class_count = Vec::new();
        huge_class_count.extend_from_slice(&1u32.to_le_bytes());
        huge_class_count.extend_from_slice(&1u32.to_le_bytes());
        huge_class_count.push(b'a');
        huge_class_count.extend_from_slice(&u32::MAX.to_le_bytes());
        let corruptions: Vec<Vec<u8>> = vec![
            valid[..valid.len() - 1].to_vec(),        // truncated
            [valid.clone(), vec![0u8]].concat(),      // trailing bytes
            vec![0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x01], // absurd file count
            huge_class_count,
            Vec::new(), // empty file
        ];
        for (i, bad) in corruptions.iter().enumerate() {
            write(&slot.file, bad);
            assert!(slot.load().is_none(), "corruption #{i} must not load");
            assert_eq!(run(Some(&slot)), direct, "corruption #{i} must rescan");
        }
    }
}
