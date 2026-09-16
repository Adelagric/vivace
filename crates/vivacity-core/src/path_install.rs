//! `Composer\Downloader\PathDownloader` (docs/reference/PathDownloader.php)
//! for Linux and macOS: a `path` package is laid out as a symbolic link to
//! its source — relative through `findShortestPath(..., preferRelative)`
//! when `transport-options.relative` (the default), absolute otherwise — or
//! as a mirror (`symlink: false`, or `COMPOSER_MIRROR_PATH_REPOS`) copied
//! through the `ArchivableFilesFinder` rules (docs/reference/
//! ArchivableFilesFinder.php, GitExcludeFilter.php, BaseExcludeFilter.php,
//! symfony-finder-Glob.php) and Symfony's `Filesystem::mirror`/`copy`
//! (docs/reference/symfony-Filesystem.php):
//!
//! - VCS directories (`.git`, `.svn`, `.hg`, …) are skipped at any depth —
//!   directories only, a `.git` *file* is copied;
//! - the root `.gitattributes` lines `<pattern> export-ignore` /
//!   `-export-ignore` (exactly two fields) exclude/re-include, the pattern
//!   through `Glob::toRegex`, matched at any depth unless it starts with `/`;
//! - a symbolic link is recreated with its raw target when it points to a
//!   file or an empty directory inside the source; a link to a non-empty
//!   directory, a dangling link or a link leaving the source is dropped;
//! - empty directories are kept; a copied file gets `0666 & ~umask` plus the
//!   source's executable bits and the source's mtime.
//!
//! Windows (junctions) is out of scope: `scope` routes such locks to the
//! Composer fallback there.

use crate::error::{Error, Result};
use crate::pathutil::{find_shortest_path_with, normalize_path};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Symlink,
    Mirror,
}

/// `computeAllowedStrategies`: `COMPOSER_MIRROR_PATH_REPOS` (PHP truthiness:
/// anything but empty and `0`) then `transport-options.symlink`.
pub fn strategy(transport_options: Option<&Value>) -> Strategy {
    let mut current = Strategy::Symlink;
    if std::env::var("COMPOSER_MIRROR_PATH_REPOS").is_ok_and(|v| !v.is_empty() && v != "0") {
        current = Strategy::Mirror;
    }
    match transport_options.and_then(|t| t.get("symlink")) {
        Some(Value::Bool(true)) => current = Strategy::Symlink,
        Some(Value::Bool(false)) => current = Strategy::Mirror,
        _ => {}
    }
    current
}

/// `($transportOptions + ['relative' => true])['relative'] === true`: absent
/// means relative, any present value other than `true` means absolute.
fn relative(transport_options: Option<&Value>) -> bool {
    match transport_options.and_then(|t| t.get("relative")) {
        Some(v) => v == &Value::Bool(true),
        None => true,
    }
}

fn realpath(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// `getInstallOperationAppendix`: what follows the operation line —
/// `: Source already present` when the install path already resolves to
/// the source, else the strategy and the dist url as written.
pub fn install_appendix(
    project_dir: &Path,
    install_path: &Path,
    dist_url: &str,
    transport_options: Option<&Value>,
) -> Result<String> {
    let real_url = realpath(&project_dir.join(dist_url))
        .ok_or_else(|| Error::Refused(format!("Failed to realpath {dist_url}")))?;
    if realpath(install_path).as_deref() == Some(real_url.as_path()) {
        return Ok(": Source already present".to_owned());
    }
    Ok(match strategy(transport_options) {
        Strategy::Symlink => format!(": Symlinking from {dist_url}"),
        Strategy::Mirror => format!(": Mirroring from {dist_url}"),
    })
}

/// `download`: the refusal to install a package inside its own source.
pub fn check_not_inside_source(
    project_dir: &Path,
    install_path: &Path,
    dist_url: &str,
    package_name: &str,
) -> Result<()> {
    let real_url = realpath(&project_dir.join(dist_url))
        .filter(|p| p.is_dir())
        .ok_or_else(|| {
            Error::Refused(format!(
                "Source path \"{dist_url}\" is not found for package {package_name}"
            ))
        })?;
    let Some(real_path) = realpath(install_path) else {
        return Ok(());
    };
    if real_path == real_url {
        return Ok(());
    }
    let inside =
        format!("{}/", real_path.display()).starts_with(&format!("{}/", real_url.display()));
    if inside {
        return Err(Error::Refused(format!(
            "Package {package_name} cannot install to \"{}\" inside its source at \"{}\"",
            real_path.display(),
            real_url.display()
        )));
    }
    Ok(())
}

/// `install` after the CLI printed the operation line: the existing path
/// is removed, then the link is created or the source mirrored. Nothing
/// happens when the path already resolves to the source.
pub fn install(
    project_dir: &Path,
    install_path: &Path,
    dist_url: &str,
    transport_options: Option<&Value>,
) -> Result<()> {
    let real_url = realpath(&project_dir.join(dist_url))
        .ok_or_else(|| Error::Refused(format!("Failed to realpath {dist_url}")))?;
    if realpath(install_path).as_deref() == Some(real_url.as_path()) {
        return Ok(());
    }
    remove_path(install_path)?;
    match strategy(transport_options) {
        Strategy::Symlink => {
            // `$absolutePath = cwd/$path`, `findShortestPath($absolutePath,
            // $realUrl, false, true)`: the leaf does not exist any more, the
            // path is composed lexically on the (real) vendor directory.
            let target = if relative(transport_options) {
                // `Platform::getCwd()` is the physical project directory;
                // the install path hangs from it lexically.
                let cwd = realpath(project_dir).unwrap_or_else(|| project_dir.to_path_buf());
                let absolute = match install_path.strip_prefix(project_dir) {
                    Ok(rel) => format!("{}/{}", cwd.display(), rel.display()),
                    Err(_) => install_path.to_string_lossy().into_owned(),
                };
                find_shortest_path_with(&absolute, &real_url.to_string_lossy(), false, true)
            } else {
                real_url.to_string_lossy().into_owned()
            };
            if let Some(parent) = install_path.parent() {
                std::fs::create_dir_all(parent).map_err(Error::io(parent))?;
            }
            symlink(Path::new(&format!("{target}/")), install_path)?;
        }
        Strategy::Mirror => {
            let real_url = PathBuf::from(normalize_path(&real_url.to_string_lossy()));
            mirror(&real_url, install_path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(Error::io(link))
}

#[cfg(not(unix))]
fn symlink(_target: &Path, link: &Path) -> Result<()> {
    Err(Error::Unsupported(format!(
        "path repositories are not installed natively on this platform ({})",
        link.display()
    )))
}

/// `Filesystem::removeDirectory` on a package path: a symbolic link is
/// unlinked (never followed), a directory removed, a missing path ignored.
pub fn remove_path(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(path).map_err(Error::io(path)),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path).map_err(Error::io(path)),
        Ok(_) => std::fs::remove_file(path).map_err(Error::io(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(path)(e)),
    }
}

/// `PathDownloader::remove`: true when the install path *is* the source
/// (`, source is still present in <path>`): nothing is removed then.
pub fn is_own_source(project_dir: &Path, install_path: &str, dist_url: &str) -> bool {
    let abs = |p: &str| {
        if crate::pathutil::is_absolute_path(p) {
            normalize_path(p)
        } else {
            normalize_path(&format!("{}/{p}", project_dir.display()))
        }
    };
    abs(install_path) == abs(dist_url)
}

// ---------------------------------------------------------------------------
// Mirror: ArchivableFilesFinder + Symfony Filesystem::mirror

const VCS_DIRS: &[&str] = &[
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

struct ExcludePattern {
    regex: pcre2::bytes::Regex,
    negate: bool,
}

/// `GitExcludeFilter`: the root `.gitattributes` only.
fn git_exclude_patterns(source: &Path) -> Vec<ExcludePattern> {
    let Ok(text) = std::fs::read_to_string(source.join(".gitattributes")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let rule = match parts.as_slice() {
            [p, "export-ignore"] => (*p).to_owned(),
            [p, "-export-ignore"] => format!("!{p}"),
            _ => continue,
        };
        if let Some(p) = generate_pattern(&rule) {
            out.push(p);
        }
    }
    out
}

/// `BaseExcludeFilter::generatePattern`.
fn generate_pattern(rule: &str) -> Option<ExcludePattern> {
    let (negate, rule) = match rule.strip_prefix('!') {
        Some(r) => (true, r.trim_start_matches('!')),
        None => (false, rule),
    };
    let prefix = match rule.find('/') {
        Some(0) => "^/",
        None => "/",
        Some(i) if i == rule.len() - 1 => "/",
        Some(_) => "",
    };
    let rule = rule.trim_matches('/');
    let inner = glob_to_regex(rule);
    let inner = &inner[2..inner.len() - 2];
    let regex = pcre2::bytes::RegexBuilder::new()
        .build(&format!("{prefix}{inner}(?=$|/)"))
        .ok()?;
    Some(ExcludePattern { regex, negate })
}

/// Symfony `Finder\Glob::toRegex($glob)` with the defaults
/// (`strictLeadingDot`, `strictWildcardSlash`, delimiter `#`).
pub fn glob_to_regex(glob: &str) -> String {
    let bytes = glob.as_bytes();
    let mut first_byte = true;
    let mut escaping = false;
    let mut in_curlies = 0usize;
    let mut regex = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let car = bytes[i] as char;
        if first_byte && car != '.' {
            regex.push_str("(?=[^\\.])");
        }
        first_byte = car == '/';
        if first_byte
            && i + 2 < bytes.len()
            && bytes[i + 1] == b'*'
            && bytes[i + 2] == b'*'
            && (i + 3 >= bytes.len() || bytes[i + 3] == b'/')
        {
            let mut piece = String::from("[^/]++/");
            if i + 3 >= bytes.len() {
                piece.push('?');
            }
            let piece = format!("(?=[^\\.]){piece}");
            regex.push_str(&format!("/(?:{piece})*"));
            i += 2 + usize::from(i + 3 < bytes.len());
            i += 1;
            escaping = false;
            continue;
        }
        match car {
            '#' | '.' | '(' | ')' | '|' | '+' | '^' | '$' => {
                regex.push('\\');
                regex.push(car);
            }
            '*' => regex.push_str(if escaping { "\\*" } else { "[^/]*" }),
            '?' => regex.push_str(if escaping { "\\?" } else { "[^/]" }),
            '{' => {
                if escaping {
                    regex.push_str("\\{");
                } else {
                    regex.push('(');
                    in_curlies += 1;
                }
            }
            '}' if in_curlies > 0 => {
                if escaping {
                    regex.push('}');
                } else {
                    regex.push(')');
                    in_curlies -= 1;
                }
            }
            ',' if in_curlies > 0 => regex.push(if escaping { ',' } else { '|' }),
            '\\' => {
                if escaping {
                    regex.push_str("\\\\");
                    escaping = false;
                } else {
                    escaping = true;
                }
                i += 1;
                continue;
            }
            c => regex.push(c),
        }
        escaping = false;
        i += 1;
    }
    format!("#^{regex}$#")
}

/// One entry the finder yields: its path under the source and what it is.
struct Entry {
    rel: PathBuf,
    kind: EntryKind,
}

enum EntryKind {
    /// A symbolic link, with its raw target.
    Link(PathBuf),
    /// An empty directory.
    Dir,
    File,
}

/// `ArchivableFilesFinder($sources, [])` as an iterator: what `mirror`
/// receives, in traversal order.
fn archivable_entries(source: &Path) -> Result<Vec<Entry>> {
    let source_real = realpath(source).unwrap_or_else(|| source.to_path_buf());
    let source_str = normalize_path(&source_real.to_string_lossy());
    let patterns = git_exclude_patterns(source);
    let mut out = Vec::new();
    walk(source, source, &source_str, &patterns, &mut out)?;
    Ok(out)
}

fn walk(
    source: &Path,
    dir: &Path,
    source_str: &str,
    patterns: &[ExcludePattern],
    out: &mut Vec<Entry>,
) -> Result<()> {
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(dir)
        .map_err(Error::io(dir))?
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let meta = std::fs::symlink_metadata(&path).map_err(Error::io(&path))?;
        let is_link = meta.file_type().is_symlink();
        // Finder::ignoreVCS: directories (a link to one included) by name.
        let name_str = name.to_string_lossy();
        if path.is_dir() && VCS_DIRS.contains(&name_str.as_ref()) {
            continue;
        }
        // The custom filter: no realpath -> dropped; a link leaving the
        // source -> dropped; the exclude patterns on the realpath's relative
        // form.
        let Some(real) = realpath(&path) else {
            continue;
        };
        let real_str = normalize_path(&real.to_string_lossy());
        if is_link && !real_str.starts_with(source_str) {
            continue;
        }
        let relative = real_str.strip_prefix(source_str).unwrap_or(&real_str);
        let mut exclude = false;
        for p in patterns {
            if p.regex.is_match(relative.as_bytes()).unwrap_or(false) {
                exclude = !p.negate;
            }
        }
        let rel = path.strip_prefix(source).unwrap_or(&path).to_path_buf();
        if exclude {
            // The filter sits on the flattened iteration: an excluded
            // directory is still traversed, and a child re-included by a
            // later `-export-ignore` rule is kept.
            if path.is_dir() && !is_link {
                walk(source, &path, source_str, patterns, out)?;
            }
            continue;
        }
        if path.is_dir() {
            // `accept()`: a directory (a link to one included) only when
            // empty; the finder never descends into a link.
            let empty = std::fs::read_dir(&path)
                .map(|mut rd| rd.next().is_none())
                .unwrap_or(false);
            if empty {
                let kind = if is_link {
                    EntryKind::Link(std::fs::read_link(&path).map_err(Error::io(&path))?)
                } else {
                    EntryKind::Dir
                };
                out.push(Entry { rel, kind });
            } else if !is_link {
                walk(source, &path, source_str, patterns, out)?;
            }
        } else if is_link {
            out.push(Entry {
                rel,
                kind: EntryKind::Link(std::fs::read_link(&path).map_err(Error::io(&path))?),
            });
        } else {
            out.push(Entry {
                rel,
                kind: EntryKind::File,
            });
        }
    }
    Ok(())
}

/// Symfony `Filesystem::mirror($originDir, $targetDir, $iterator)`.
fn mirror(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target).map_err(Error::io(target))?;
    for entry in archivable_entries(source)? {
        let dest = target.join(&entry.rel);
        let src = source.join(&entry.rel);
        match entry.kind {
            EntryKind::Link(raw_target) => {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(Error::io(parent))?;
                }
                symlink(&raw_target, &dest)?;
            }
            EntryKind::Dir => {
                std::fs::create_dir_all(&dest).map_err(Error::io(&dest))?;
            }
            EntryKind::File => copy_file(&src, &dest)?,
        }
    }
    Ok(())
}

/// Symfony `Filesystem::copy`: a fresh file (`0666 & ~umask`), the source's
/// executable bits added, the source's mtime.
fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(Error::io(parent))?;
    }
    let meta = std::fs::metadata(src).map_err(Error::io(src))?;
    {
        let mut from = std::fs::File::open(src).map_err(Error::io(src))?;
        let mut to = std::fs::File::create(dest).map_err(Error::io(dest))?;
        std::io::copy(&mut from, &mut to).map_err(Error::io(dest))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let current = std::fs::metadata(dest)
            .map_err(Error::io(dest))?
            .permissions()
            .mode();
        let mode = current | (meta.permissions().mode() & 0o111);
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode))
            .map_err(Error::io(dest))?;
    }
    if let Ok(modified) = meta.modified() {
        // `touch($target, filemtime($origin))`: whole seconds, atime too.
        let modified = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| std::time::UNIX_EPOCH + std::time::Duration::from_secs(d.as_secs()))
            .unwrap_or(modified);
        let times = std::fs::FileTimes::new()
            .set_modified(modified)
            .set_accessed(modified);
        let f = std::fs::File::options()
            .write(true)
            .open(dest)
            .map_err(Error::io(dest))?;
        f.set_times(times).map_err(Error::io(dest))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_to_regex_like_symfony() {
        // `php -r 'echo Symfony\Component\Finder\Glob::toRegex($g);'`
        for (glob, regex) in [
            ("*.md", "#^(?=[^\\.])[^/]*\\.md$#"),
            ("docs", "#^(?=[^\\.])docs$#"),
            (
                "a/**/b",
                "#^(?=[^\\.])a/(?:(?=[^\\.])[^/]++/)*(?=[^\\.])b$#",
            ),
            ("{a,b}.txt", "#^(?=[^\\.])(a|b)\\.txt$#"),
            ("a/**", "#^(?=[^\\.])a/(?:(?=[^\\.])[^/]++/?)*$#"),
            ("/docs", "#^(?=[^\\.])/(?=[^\\.])docs$#"),
            (".hidden", "#^\\.hidden$#"),
            ("a\\*b", "#^(?=[^\\.])a\\*b$#"),
            ("x/*.php", "#^(?=[^\\.])x/(?=[^\\.])[^/]*\\.php$#"),
        ] {
            assert_eq!(glob_to_regex(glob), regex, "{glob}");
        }
    }

    #[test]
    fn exclude_patterns_like_composer() {
        let p = generate_pattern("/docs").unwrap();
        assert!(p.regex.is_match(b"/docs/guide.md").unwrap());
        assert!(!p.regex.is_match(b"/src/docs").unwrap());
        let p = generate_pattern("*.md").unwrap();
        assert!(p.regex.is_match(b"/docs/guide.md").unwrap());
        assert!(p.regex.is_match(b"/README.md").unwrap());
        assert!(!p.regex.is_match(b"/README.md.txt").unwrap());
        let p = generate_pattern("tests").unwrap();
        assert!(p.regex.is_match(b"/src/tests/x.php").unwrap());
        let p = generate_pattern("!README.md").unwrap();
        assert!(p.negate);
    }
}
