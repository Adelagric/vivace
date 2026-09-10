//! Ports exacts de `Composer\Util\Filesystem` : `normalizePath`,
//! `findShortestPath`, `findShortestPathCode` (Composer 2.10.3). Ils décident
//! des chemins écrits dans installed.json/installed.php, les fichiers
//! d'autoload et les proxies bin ; vérifiés par tests/oracle_installers.rs.
//! Chemins Unix uniquement (pas de préfixe `C:`/`file://`).

/// `Filesystem::normalizePath` : slashes uniques, résolution de `.`/`..`,
/// pas de slash final (hors racine).
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

/// PHP `dirname()` sur un chemin normalisé Unix.
fn php_dirname(p: &str) -> String {
    match p.rfind('/') {
        None => ".".to_owned(),
        Some(0) => "/".to_owned(),
        Some(i) => p[..i].to_owned(),
    }
}

/// PHP `basename()` sur un chemin normalisé Unix.
fn php_basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Boucle de `findShortestPath(Code)` : remonte `to` jusqu'à un préfixe de
/// `from` (comparaison en segments entiers), `/` ou `.`. Composer exige des
/// chemins absolus (exception sinon) ; ici un chemin relatif s'arrête à `.`
/// et l'appelant rend `to` tel quel, sans boucler.
fn common_path(from: &str, to: &str) -> String {
    let mut common = to.to_owned();
    while !format!("{from}/").starts_with(&format!("{common}/")) && common != "/" && common != "." {
        common = php_dirname(&common);
    }
    common
}

/// `Filesystem::findShortestPath($from, $to, $directories, $preferRelative = false)`.
/// Les deux chemins doivent être absolus.
pub fn find_shortest_path(from: &str, to: &str, directories: bool) -> String {
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
    if common == "/" && depth > 1 {
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

/// `Filesystem::findShortestPathCode($from, $to, $directories, $staticCode, $preferRelative = false)` :
/// une expression PHP relative à `__DIR__`.
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

#[cfg(test)]
mod tests {
    use super::*;

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
