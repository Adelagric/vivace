//! Exact ports of `Composer\Util\Filesystem`: `normalizePath`,
//! `findShortestPath`, `findShortestPathCode` (Composer 2.10.3). They decide
//! the paths written into installed.json/installed.php, the autoload files
//! and the bin proxies; checked by tests/oracle_installers.rs.
//! Unix paths only (no `C:`/`file://` prefix).

/// `std::fs::canonicalize` without the Windows verbatim prefix (`\\?\`).
/// PHP `realpath()` (and therefore Composer) returns classic Win32 paths
/// (`C:\x`), and `normalize_path` would turn `\\?\C:\x` into `//?/C:/x` —
/// a form neither the Win32 APIs nor the paths generated into the autoload
/// files understand. Elsewhere: `std::fs::canonicalize` as-is.
pub fn canonicalize(path: impl AsRef<std::path::Path>) -> std::io::Result<std::path::PathBuf> {
    std::fs::canonicalize(path).map(strip_verbatim)
}

#[cfg(not(windows))]
fn strip_verbatim(p: std::path::PathBuf) -> std::path::PathBuf {
    p
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share\x` → `\\srv\share\x`. The classic
/// form is returned EVEN beyond MAX_PATH: Rust (≥1.58) converts back to
/// verbatim in its own syscalls, and PHP (≥7.1) does the same on its side —
/// PHP's `realpath()` in fact returns the classic long form, so that is the
/// form that preserves parity (verified: install + autoload under a
/// 307-character root, `LongPathsEnabled=0`). The verbatim fallback only
/// covers shapes a re-stat cannot find again (reserved component, trailing
/// dot/space… — paths classic Win32 would mangle).
#[cfg(windows)]
fn strip_verbatim(p: std::path::PathBuf) -> std::path::PathBuf {
    use std::path::{Component, Prefix};
    let mut comps = p.components();
    let Some(Component::Prefix(prefix)) = comps.next() else {
        return p;
    };
    let root = match prefix.kind() {
        Prefix::VerbatimDisk(d) => format!("{}:\\", d as char),
        Prefix::VerbatimUNC(server, share) => format!(
            "\\\\{}\\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        ),
        _ => return p,
    };
    let mut out = std::path::PathBuf::from(root);
    for c in comps {
        if !matches!(c, Component::RootDir) {
            out.push(c.as_os_str());
        }
    }
    // The classic form must stay openable (path < MAX_PATH, no reserved
    // component): otherwise keep the verbatim form.
    if out.symlink_metadata().is_ok() {
        out
    } else {
        p
    }
}

/// `Filesystem::isAbsolutePath`: `/…`, `\…`, `C:/…`/`C:\…`, or a stream
/// wrapper (`phar://…`). On Unix only `/` exists in practice; the
/// `starts_with('/')` form of the test missed Windows drive-letter paths.
pub fn is_absolute_path(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') || path.contains("://") {
        return true;
    }
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

/// `Filesystem::normalizePath`: single slashes, `.`/`..` resolution, no
/// trailing slash (except for the root).
pub fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let (absolute, rest) = if path.starts_with("//") && path.len() > 2 {
        ("//", &path[2..])
    } else if let Some(r) = path.strip_prefix('/') {
        ("/", r)
    } else {
        ("", path.as_str())
    };
    let mut parts: Vec<&str> = Vec::new();
    let mut up = false;
    for chunk in rest.split('/') {
        if chunk == ".." && (!absolute.is_empty() || up) {
            parts.pop();
            up = !(parts.is_empty() || parts.last() == Some(&".."));
        } else if chunk != "." && !chunk.is_empty() {
            parts.push(chunk);
            up = chunk != "..";
        }
    }
    format!("{absolute}{}", parts.join("/"))
}

/// PHP `dirname()` on a normalised Unix path.
fn php_dirname(p: &str) -> String {
    match p.rfind('/') {
        None => ".".to_owned(),
        Some(0) => "/".to_owned(),
        Some(i) => p[..i].to_owned(),
    }
}

/// PHP `basename()` on a normalised Unix path.
fn php_basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Loop of `findShortestPath(Code)`: walks `to` up until a prefix of `from`
/// (whole-segment comparison), `/` or `.`. Composer requires absolute paths
/// (exception otherwise); here a relative path stops at `.` and the caller
/// returns `to` as is, without looping.
fn common_path(from: &str, to: &str) -> String {
    let mut common = to.to_owned();
    while !format!("{from}/").starts_with(&format!("{common}/")) && common != "/" && common != "." {
        common = php_dirname(&common);
    }
    common
}

/// `Filesystem::findShortestPath($from, $to, $directories, $preferRelative = false)`.
/// Both paths must be absolute.
pub fn find_shortest_path(from: &str, to: &str, directories: bool) -> String {
    find_shortest_path_with(from, to, directories, false)
}

/// `findShortestPath` with `$preferRelative`: when true, a path that only
/// shares the root with `from` is still written relative (`../../..`),
/// which is how a symlinked `path` package points at its source.
pub fn find_shortest_path_with(
    from: &str,
    to: &str,
    directories: bool,
    prefer_relative: bool,
) -> String {
    let mut from = normalize_path(from);
    let to = normalize_path(to);
    if directories {
        from = format!("{}/dummy_file", from.trim_end_matches('/'));
    }
    if php_dirname(&from) == php_dirname(&to) {
        return format!("./{}", php_basename(&to));
    }
    let common = common_path(&from, &to);
    if !from.starts_with(&common) || common == "." {
        return to;
    }
    let common = format!("{}/", common.trim_end_matches('/'));
    let depth = from[common.len().min(from.len())..].matches('/').count();
    if !prefer_relative && common == "/" && depth > 1 {
        return to;
    }
    let result = format!(
        "{}{}",
        "../".repeat(depth),
        &to[common.len().min(to.len())..]
    );
    if result.is_empty() {
        "./".to_owned()
    } else {
        result
    }
}

/// `Filesystem::findShortestPathCode($from, $to, $directories, $staticCode, $preferRelative = false)`:
/// a PHP expression relative to `__DIR__`.
pub fn find_shortest_path_code(
    from: &str,
    to: &str,
    directories: bool,
    static_code: bool,
) -> String {
    let from = normalize_path(from);
    let to = normalize_path(to);
    if from == to {
        return if directories { "__DIR__" } else { "__FILE__" }.to_owned();
    }
    let common = common_path(&from, &to);
    if !from.starts_with(&common) || common == "." {
        return php_str(&to);
    }
    let common = format!("{}/", common.trim_end_matches('/'));
    if to.starts_with(&format!("{from}/")) {
        return format!("__DIR__ . {}", php_str(&to[from.len()..]));
    }
    let depth =
        from[common.len().min(from.len())..].matches('/').count() + usize::from(directories);
    if common == "/" && depth > 1 {
        return php_str(&to);
    }
    let code = if static_code {
        format!("__DIR__ . '{}'", "/..".repeat(depth))
    } else {
        format!("{}__DIR__{}", "dirname(".repeat(depth), ")".repeat(depth))
    };
    let rel = &to[common.len().min(to.len())..];
    if rel.is_empty() {
        code
    } else {
        format!("{code}.{}", php_str(&format!("/{rel}")))
    }
}

/// `var_export()` of a string: single quotes, `\` and `'` escaped.
pub fn php_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_has_no_verbatim_prefix() {
        let tmp = tempfile::tempdir().expect("tmp");
        let real = canonicalize(tmp.path()).expect("canonicalize");
        let s = real.to_string_lossy().into_owned();
        assert!(!s.starts_with(r"\\?\"), "verbatim prefix not stripped: {s}");
        // The returned form must stay openable and normalize cleanly.
        assert!(real.is_dir());
        assert!(!normalize_path(&s).starts_with("//?/"), "{s}");
    }

    /// Beyond MAX_PATH, `canonicalize` returns the classic form (no `\\?\`)
    /// and that form stays readable — it is what PHP `realpath()` would
    /// return, hence what the generated files must contain.
    #[cfg(windows)]
    #[test]
    fn canonicalize_strips_verbatim_beyond_max_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut deep = tmp.path().to_path_buf();
        while deep.as_os_str().len() < 300 {
            deep.push("abcdefghijklmnopqrstuvwxyz0123456789");
        }
        std::fs::create_dir_all(&deep).expect("mkdir deep");
        std::fs::write(deep.join("f.txt"), b"x").expect("write");
        let real = canonicalize(&deep).expect("canonicalize");
        assert!(real.as_os_str().len() > 260, "{}", real.display());
        assert!(
            !real.to_string_lossy().starts_with(r"\\?\"),
            "verbatim prefix beyond MAX_PATH: {}",
            real.display()
        );
        assert!(std::fs::read(real.join("f.txt")).is_ok(), "unreadable form");
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_path("/a/b/../c/./d/"), "/a/c/d");
        assert_eq!(normalize_path("app/"), "app");
        assert_eq!(normalize_path("/a//b"), "/a/b");
        assert_eq!(normalize_path("../x"), "../x");
        assert_eq!(
            normalize_path("/p/web/app/plugins/x/"),
            "/p/web/app/plugins/x"
        );
    }

    #[test]
    fn shortest_paths_directories() {
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/p/vendor", true),
            "../"
        );
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/p", true),
            "../../"
        );
        assert_eq!(find_shortest_path("/p/vendor", "/p/app", true), "../app");
        assert_eq!(find_shortest_path("/p", "/p/app/x", true), "app/x");
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/p/vendor/a/b", true),
            "../a/b"
        );
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/p/vendor/composer/x", true),
            "./x"
        );
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/p/web/app/plugins/x", true),
            "../../web/app/plugins/x"
        );
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/q/x", true),
            "/q/x"
        );
        assert_eq!(
            find_shortest_path("/p/vendor/composer", "/p/vendor/composer", true),
            "./"
        );
    }

    #[test]
    fn shortest_path_codes() {
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/vendor", true, true),
            "__DIR__ . '/..'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p", true, true),
            "__DIR__ . '/../..'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/vendor", true, false),
            "dirname(__DIR__)"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p", true, false),
            "dirname(__DIR__)"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p/vendor/composer", true, false),
            "__DIR__ . '/composer'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/vendor/composer", true, false),
            "__DIR__"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/web/x", true, true),
            "__DIR__ . '/../..'.'/web/x'"
        );
    }
}
