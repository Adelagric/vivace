//! Path utilities for the generator: the exact ports of
//! Composer\Util\Filesystem live in vivacity-core::pathutil; this module holds
//! the "directories" variants used by AutoloadGenerator (`$directories =
//! true`), plus raw-byte `var_export` and `preg_quote`.

pub use vivacity_core::pathutil::{normalize_path, php_str};

/// `Filesystem::findShortestPath($from, $to, true)`.
pub fn find_shortest_path(from: &str, to: &str) -> String {
    vivacity_core::pathutil::find_shortest_path(from, to, true)
}

/// `Filesystem::findShortestPathCode($from, $to, true, $staticCode)`.
pub fn find_shortest_path_code(from: &str, to: &str, static_code: bool) -> String {
    vivacity_core::pathutil::find_shortest_path_code(from, to, true, static_code)
}

/// `var_export()` of a PHP string as raw bytes (non-UTF-8 class names).
pub fn php_str_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s {
        match b {
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b => out.push(b),
        }
    }
    out.push(b'\'');
    out
}

/// `preg_quote($s)` (no `{` delimiter passed: Composer quotes without a delimiter).
pub fn preg_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".\\+*?[^]$(){}=!<>|:-#/".contains(c) && c != '/' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_paths() {
        assert_eq!(find_shortest_path("/p/vendor/composer", "/p/vendor"), "../");
        assert_eq!(find_shortest_path("/p/vendor/composer", "/p"), "../../");
        assert_eq!(find_shortest_path("/p/vendor", "/p/app"), "../app");
        assert_eq!(find_shortest_path("/p", "/p/app/x"), "app/x");
    }

    #[test]
    fn shortest_path_codes() {
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/vendor", true),
            "__DIR__ . '/..'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p", true),
            "__DIR__ . '/../..'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/vendor", false),
            "dirname(__DIR__)"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p", false),
            "dirname(__DIR__)"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p/vendor/composer", false),
            "__DIR__ . '/composer'"
        );
    }

    #[test]
    fn php_string_export() {
        assert_eq!(php_str("Foo\\Bar"), "'Foo\\\\Bar'");
        assert_eq!(php_str("it's"), "'it\\'s'");
        assert_eq!(preg_quote("a.b/c-d"), "a\\.b/c\\-d");
    }
}
