//! Utilitaires de chemins pour le générateur : les ports exacts de
//! Composer\Util\Filesystem vivent dans vivace-core::pathutil ; ici les
//! variantes « répertoires » qu'utilise AutoloadGenerator (`$directories =
//! true`), et `var_export` en octets bruts / `preg_quote`.

pub use vivace_core::pathutil::{normalize_path, php_str};

/// `Filesystem::findShortestPath($from, $to, true)`.
pub fn find_shortest_path(from: &str, to: &str) -> String {
    vivace_core::pathutil::find_shortest_path(from, to, true)
}

/// `Filesystem::findShortestPathCode($from, $to, true, $staticCode)`.
pub fn find_shortest_path_code(from: &str, to: &str, static_code: bool) -> String {
    vivace_core::pathutil::find_shortest_path_code(from, to, true, static_code)
}

/// `var_export()` d'une chaîne PHP en octets bruts (noms de classes non-UTF-8).
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

/// `preg_quote($s)` (délimiteur `{`… non passé : Composer quote sans délimiteur).
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
