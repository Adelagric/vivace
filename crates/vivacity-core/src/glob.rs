//! The `glob()` a `path` repository runs (`PathRepository::getUrlMatches`:
//! `GLOB_MARK | GLOB_ONLYDIR | GLOB_BRACE`) and `Platform::expandPath`,
//! reproduced from the libc semantics PHP exposes on Linux and macOS:
//!
//! - braces expand first, left to right, nesting allowed; every alternative
//!   is globbed on its own and its matches are sorted bytewise among
//!   themselves, so `{b,a}` lists the `b` matches before the `a` ones and a
//!   duplicate alternative yields the same path twice;
//! - `*`, `?` and `[…]` match inside one path segment and never a leading
//!   `.` (no `GLOB_PERIOD`); a segment without a metacharacter is a literal
//!   that must exist; `\` escapes the next character;
//! - the result keeps the pattern's spelling (`./packages/*` → `./packages/x`,
//!   `a//*` → `a//x`); a directory gets a trailing `/` (`GLOB_MARK`), which
//!   a pattern ending in `/` already carries once;
//! - only directories are kept (`GLOB_ONLYDIR`, a link to a directory counts).

use std::path::{Path, PathBuf};

/// `glob($pattern, GLOB_MARK | GLOB_ONLYDIR | GLOB_BRACE)` with a relative
/// pattern resolved against `base` (the project directory, PHP's cwd).
pub fn glob_dirs(pattern: &str, base: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for alternative in brace_expand(pattern) {
        let mut found = glob_one(&alternative, base);
        found.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        out.extend(found);
    }
    out
}

/// `GLOB_BRACE`: the first `{…}` with a matching `}` is expanded, each
/// alternative recursively.
pub fn brace_expand(pattern: &str) -> Vec<String> {
    let bytes = pattern.as_bytes();
    let Some(open) = find_unescaped(bytes, 0, b'{') else {
        return vec![pattern.to_owned()];
    };
    // The matching brace and the top-level commas between them.
    let mut depth = 0usize;
    let mut cuts: Vec<usize> = Vec::new();
    let mut close: Option<usize> = None;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            b',' if depth == 1 => cuts.push(i),
            _ => {}
        }
        i += 1;
    }
    let Some(close) = close else {
        return vec![pattern.to_owned()];
    };
    let head = &pattern[..open];
    let tail = &pattern[close + 1..];
    let mut out = Vec::new();
    let mut start = open + 1;
    for end in cuts.into_iter().chain(std::iter::once(close)) {
        let alt = &pattern[start..end];
        out.extend(brace_expand(&format!("{head}{alt}{tail}")));
        start = end + 1;
    }
    out
}

fn find_unescaped(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn has_magic(segment: &str) -> bool {
    let b = segment.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'*' | b'?' | b'[' => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

fn unescape(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn glob_one(pattern: &str, base: &Path) -> Vec<String> {
    let segments: Vec<&str> = pattern.split('/').collect();
    // (spelling so far, filesystem path so far)
    let mut current: Vec<(String, PathBuf)> = vec![(String::new(), base.to_path_buf())];
    let last = segments.len() - 1;
    for (i, seg) in segments.iter().enumerate() {
        let mut next: Vec<(String, PathBuf)> = Vec::new();
        for (spelling, dir) in &current {
            let prefix = if i == 0 {
                String::new()
            } else {
                format!("{spelling}/")
            };
            if seg.is_empty() {
                // A leading `/` (absolute), a doubled `//`, or a trailing `/`
                // (directories only): the spelling keeps the slash, the
                // filesystem path is unchanged (or the root).
                let fs = if i == 0 {
                    PathBuf::from("/")
                } else {
                    dir.clone()
                };
                if i == last && !fs.is_dir() {
                    continue;
                }
                next.push((prefix, fs));
            } else if has_magic(seg) {
                let Ok(rd) = std::fs::read_dir(dir) else {
                    continue;
                };
                // readdir lists `.` and `..` too: a pattern starting with a
                // dot matches them (`.*` → `packages/./`, `packages/../`).
                let mut names: Vec<String> = vec![".".into(), "..".into()];
                names.extend(
                    rd.flatten()
                        .filter_map(|e| e.file_name().to_str().map(str::to_owned)),
                );
                for name in names {
                    if !fnmatch_period(seg, &name) {
                        continue;
                    }
                    let fs = dir.join(&name);
                    if i == last && !fs.is_dir() {
                        continue;
                    }
                    next.push((format!("{prefix}{name}"), fs));
                }
            } else {
                let literal = unescape(seg);
                let fs = dir.join(&literal);
                let exists = if i == last { fs.is_dir() } else { fs.exists() };
                if !exists {
                    continue;
                }
                next.push((format!("{prefix}{seg}"), fs));
            }
        }
        current = next;
    }
    current
        .into_iter()
        .map(|(spelling, _)| {
            if spelling.ends_with('/') {
                spelling
            } else {
                format!("{spelling}/")
            }
        })
        .collect()
}

/// `fnmatch(pattern, name, FNM_PERIOD)` on one segment.
fn fnmatch_period(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    fnmatch(pattern.as_bytes(), name.as_bytes())
}

fn fnmatch(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    match p[0] {
        b'*' => {
            // Collapse consecutive stars; try every split.
            let rest = &p[1..];
            (0..=s.len()).any(|k| fnmatch(rest, &s[k..]))
        }
        b'?' => !s.is_empty() && fnmatch(&p[1..], &s[1..]),
        b'[' => {
            let Some((matched, consumed)) = bracket(p, s) else {
                // No closing bracket: a literal `[`.
                return !s.is_empty() && s[0] == b'[' && fnmatch(&p[1..], &s[1..]);
            };
            matched && fnmatch(&p[consumed..], &s[1..])
        }
        b'\\' if p.len() > 1 => !s.is_empty() && s[0] == p[1] && fnmatch(&p[2..], &s[1..]),
        c => !s.is_empty() && s[0] == c && fnmatch(&p[1..], &s[1..]),
    }
}

/// `[…]` at the start of `p` against the first byte of `s`: (matched, bytes
/// of the pattern consumed), None without a closing bracket.
fn bracket(p: &[u8], s: &[u8]) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = i < p.len() && (p[i] == b'!' || p[i] == b'^');
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    let c = s.first().copied();
    while i < p.len() {
        let mut lo = p[i];
        if lo == b']' && !first {
            let hit = matched != negate;
            return Some((c.is_some() && hit, i + 1));
        }
        first = false;
        if lo == b'\\' && i + 1 < p.len() {
            i += 1;
            lo = p[i];
        }
        let mut hi = lo;
        if i + 2 < p.len() && p[i + 1] == b'-' && p[i + 2] != b']' {
            hi = p[i + 2];
            if hi == b'\\' && i + 3 < p.len() {
                hi = p[i + 3];
                i += 1;
            }
            i += 2;
        }
        if let Some(c) = c {
            if lo <= c && c <= hi {
                matched = true;
            }
        }
        i += 1;
    }
    None
}

/// PHP `dirname()` for the "does not exist" walk of `PathRepository`.
pub fn php_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.starts_with('/') {
            "/".into()
        } else {
            ".".into()
        };
    }
    match trimmed.rfind('/') {
        None => ".".into(),
        Some(0) => "/".into(),
        Some(i) => trimmed[..i]
            .trim_end_matches('/')
            .to_owned()
            .replace_empty_with_root(),
    }
}

trait ReplaceEmpty {
    fn replace_empty_with_root(self) -> String;
}
impl ReplaceEmpty for String {
    fn replace_empty_with_root(self) -> String {
        if self.is_empty() {
            "/".into()
        } else {
            self
        }
    }
}

/// `Platform::expandPath`: `~/x` → `$HOME/x`; a leading `$VAR` or `%VAR%`
/// is replaced by the variable (empty when unset), the rest kept.
pub fn expand_path(path: &str) -> String {
    if path.starts_with("~/") || path.starts_with("~\\") {
        return format!("{}{}", user_directory(), &path[1..]);
    }
    let b = path.as_bytes();
    let (percent, start) = match b.first() {
        Some(b'$') => (false, 1),
        Some(b'%') => (true, 1),
        _ => return path.to_owned(),
    };
    let mut end = start;
    while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
        end += 1;
    }
    if end == start {
        return path.to_owned();
    }
    let var = &path[start..end];
    let rest = if percent {
        match path[end..].strip_prefix('%') {
            Some(r) => r,
            None => return path.to_owned(),
        }
    } else {
        &path[end..]
    };
    let value = if cfg!(windows) && var == "HOME" {
        match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => h,
            _ => std::env::var("USERPROFILE").unwrap_or_default(),
        }
    } else {
        std::env::var(var).unwrap_or_default()
    };
    format!("{value}{rest}")
}

fn user_directory() -> String {
    if let Ok(h) = std::env::var("HOME") {
        return h;
    }
    if cfg!(windows) {
        if let Ok(h) = std::env::var("USERPROFILE") {
            return h;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braces() {
        assert_eq!(brace_expand("a{b,c}d"), vec!["abd", "acd"]);
        assert_eq!(
            brace_expand("a{b,c}d{e,f}"),
            vec!["abde", "abdf", "acde", "acdf"]
        );
        assert_eq!(brace_expand("x{a,{b,c}}"), vec!["xa", "xb", "xc"]);
        assert_eq!(brace_expand("plain"), vec!["plain"]);
        assert_eq!(brace_expand("un{closed"), vec!["un{closed"]);
    }

    #[test]
    fn matching() {
        assert!(fnmatch_period("*", "alpha"));
        assert!(!fnmatch_period("*", ".hidden"));
        assert!(fnmatch_period(".*", ".hidden"));
        assert!(fnmatch_period("a?pha", "alpha"));
        assert!(fnmatch_period("[a-c]*", "beta"));
        assert!(!fnmatch_period("[!a-c]*", "beta"));
        assert!(fnmatch_period("*a", "alpha"));
        assert!(!fnmatch_period("*a", "alphabet"));
        assert!(fnmatch_period("\\*", "*"));
    }

    #[test]
    fn dirname_like_php() {
        assert_eq!(php_dirname("packages/x*"), "packages");
        assert_eq!(php_dirname("packages/*/"), "packages");
        assert_eq!(php_dirname("*"), ".");
        assert_eq!(php_dirname("/a"), "/");
        assert_eq!(php_dirname("/a/b"), "/a");
        assert_eq!(php_dirname("a//b"), "a");
    }

    /// Against PHP's own `glob()` on a temporary tree, when a `php` binary is
    /// on the PATH (the CI images of the harness have one).
    #[test]
    fn matches_php_glob() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        for d in [
            "packages/alpha/src",
            "packages/delta",
            "packages/.hidden",
            "packages/gamma",
            "libs/beta",
            "libs/epsilon",
            "src-zeta",
            "a-b",
        ] {
            std::fs::create_dir_all(root.join(d)).expect("mkdir");
        }
        std::fs::write(root.join("packages/file"), "").expect("file");
        #[cfg(unix)]
        std::os::unix::fs::symlink("packages/alpha", root.join("link")).expect("link");
        let patterns = [
            "packages/*",
            "./packages/*/",
            "packages//*",
            "libs/{epsilon,beta}",
            "packages/{alpha,alpha}",
            "packages/*/src",
            "packages/*a",
            "packages/.*",
            "packages/?elta",
            "packages/[ad]*",
            "packages/[!a]*",
            "packages/file",
            "packages/alpha",
            "nowhere/*",
            "*",
            "{packages,libs}/*",
            "a-b",
            "l?nk",
        ];
        let php = std::process::Command::new("php")
            .arg("-r")
            .arg("chdir($argv[1]); foreach (array_slice($argv, 2) as $p) { echo json_encode(glob($p, GLOB_MARK|GLOB_ONLYDIR|GLOB_BRACE), JSON_UNESCAPED_SLASHES), \"\\n\"; }")
            .arg(root)
            .args(patterns)
            .output();
        let Ok(php) = php else {
            eprintln!("php not found: oracle skipped");
            return;
        };
        assert!(
            php.status.success(),
            "{}",
            String::from_utf8_lossy(&php.stderr)
        );
        let lines: Vec<String> = String::from_utf8_lossy(&php.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        for (p, expected) in patterns.iter().zip(lines) {
            let ours = serde_json::to_string(&glob_dirs(p, root)).unwrap();
            assert_eq!(ours, expected, "pattern {p}");
        }
    }
}
