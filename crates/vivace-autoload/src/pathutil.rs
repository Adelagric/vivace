//! Utilitaires de chemins de Composer\Util\Filesystem, portés pour produire
//! les mêmes chaînes que le générateur : `normalizePath`, `findShortestPath`,
//! `findShortestPathCode`, et `var_export` d'une chaîne PHP.

/// `Filesystem::normalizePath` : slashes uniques, résolution de `.`/`..`,
/// pas de slash final (hors racine).
pub fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let (absolute, rest) = if let Some(r) = path.strip_prefix('/') {
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

/// `Filesystem::findShortestPath($from, $to, $directories = true)` — chemin
/// relatif le plus court de `from` (un répertoire) vers `to`, ou absolu si
/// aucun préfixe commun utile. Les deux entrées sont absolues et normalisées.
pub fn find_shortest_path(from: &str, to: &str) -> String {
    let from = normalize_path(from);
    let to = normalize_path(to);
    if from == to {
        return "./".to_owned();
    }
    let common = common_prefix_dirs(&from, &to);
    // Composer : si le préfixe commun est trop court (< 2 segments hors
    // racine) pour un chemin qui n'est pas un simple parent, il renvoie
    // l'absolu. Approximation fidèle pour nos cas : vendor/composer ↔ projet.
    if common.is_empty() || common == "/" {
        return to;
    }
    let from_rest = from[common.len()..].trim_start_matches('/');
    let to_rest = to[common.len()..].trim_start_matches('/');
    let ups = if from_rest.is_empty() {
        0
    } else {
        from_rest.split('/').count()
    };
    let mut out = String::new();
    for _ in 0..ups {
        out.push_str("../");
    }
    if to_rest.is_empty() {
        if out.is_empty() {
            "./".to_owned()
        } else {
            out
        }
    } else {
        out.push_str(to_rest);
        out
    }
}

fn common_prefix_dirs(a: &str, b: &str) -> String {
    let a_parts: Vec<&str> = a.split('/').collect();
    let b_parts: Vec<&str> = b.split('/').collect();
    let mut common: Vec<&str> = Vec::new();
    for (x, y) in a_parts.iter().zip(b_parts.iter()) {
        if x == y {
            common.push(x);
        } else {
            break;
        }
    }
    let joined = common.join("/");
    if joined.is_empty() && a.starts_with('/') {
        "/".to_owned()
    } else {
        joined
    }
}

/// `Filesystem::findShortestPathCode($from, $to, true, $staticCode)` : une
/// expression PHP relative à `__DIR__` (ou `$vendorDir` après substitution).
/// Forme observée : `__DIR__ . '/..'` ; `dirname(__DIR__)` en mode dynamique.
pub fn find_shortest_path_code(from: &str, to: &str, static_code: bool) -> String {
    let from = normalize_path(from);
    let to = normalize_path(to);
    if from == to {
        return "__DIR__".to_owned();
    }
    let common = common_prefix_dirs(&from, &to);
    if common.is_empty() || common == "/" {
        return php_str(&to);
    }
    let from_rest = from[common.len()..].trim_start_matches('/');
    let to_rest = to[common.len()..].trim_start_matches('/');
    let ups = if from_rest.is_empty() {
        0
    } else {
        from_rest.split('/').count()
    };
    if static_code {
        let mut rel = String::new();
        for i in 0..ups {
            if i > 0 {
                rel.push('/');
            }
            rel.push_str("..");
        }
        if !to_rest.is_empty() {
            if !rel.is_empty() {
                rel.push('/');
            }
            rel.push_str(to_rest);
        }
        format!("__DIR__ . {}", php_str(&format!("/{rel}")))
    } else {
        let mut code = "__DIR__".to_owned();
        for _ in 0..ups {
            code = format!("dirname({code})");
        }
        if to_rest.is_empty() {
            code
        } else {
            format!("{code} . {}", php_str(&format!("/{to_rest}")))
        }
    }
}

/// `var_export()` d'une chaîne : quotes simples, `\` et `'` échappés.
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
    fn normalize() {
        assert_eq!(normalize_path("/a/b/../c/./d/"), "/a/c/d");
        assert_eq!(normalize_path("app/"), "app");
        assert_eq!(normalize_path("/a//b"), "/a/b");
        assert_eq!(normalize_path("../x"), "../x");
    }

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
