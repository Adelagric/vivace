//! Port de `Composer\Semver\VersionParser` (docs/reference/resolver/
//! semver-VersionParser.php) : `normalize`, `normalizeBranch`,
//! `parseStability`, `parseNumericAliasPrefix`, avec les mêmes expressions
//! PCRE (pcre2), pour que chaque chaîne acceptée ou refusée le soit
//! exactement comme Composer.

use pcre2::bytes::{Regex, RegexBuilder};
use std::sync::OnceLock;

pub const MODIFIER_REGEX: &str =
    r"[._-]?(?:(stable|beta|b|RC|alpha|a|patch|pl|p)((?:[.-]?\d+)*+)?)?([.-]?dev)?";
pub const STABILITIES_REGEX: &str = "stable|RC|beta|alpha|dev";
pub const DEFAULT_BRANCH_ALIAS: &str = "9999999-dev";

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
#[error("{0}")]
pub struct VersionError(pub String);

/// Regex PCRE compilée une fois ; `caseless` = modificateur `i`.
pub(crate) fn regex(
    cell: &'static OnceLock<Regex>,
    pattern: &str,
    caseless: bool,
) -> &'static Regex {
    cell.get_or_init(|| {
        RegexBuilder::new()
            .caseless(caseless)
            .build(pattern)
            .unwrap_or_else(|e| panic!("regex `{pattern}`: {e}"))
    })
}

/// Groupe capturé (`""` si absent), comme `$matches[$i]` en PHP.
pub(crate) fn group<'a>(caps: &pcre2::bytes::Captures<'a>, i: usize) -> &'a str {
    caps.get(i)
        .map(|m| std::str::from_utf8(m.as_bytes()).unwrap_or(""))
        .unwrap_or("")
}

/// `VersionParser::parseStability`.
pub fn parse_stability(version: &str) -> &'static str {
    // Chemin rapide : une version purement numérique (`1.2.3.0`) est
    // stable — aucun modificateur ne peut s'y trouver.
    if !version.is_empty() && version.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return "stable";
    }
    static HASH: OnceLock<Regex> = OnceLock::new();
    static MOD: OnceLock<Regex> = OnceLock::new();
    let hash = regex(&HASH, r"#.+$", false);
    let version: String = match hash.find(version.as_bytes()).ok().flatten() {
        Some(m) => version[..m.start()].to_owned(),
        None => version.to_owned(),
    };
    if version.starts_with("dev-") || version.ends_with("-dev") {
        return "dev";
    }
    let re = regex(&MOD, &format!("{MODIFIER_REGEX}(?:\\+.*)?$"), true);
    let lower = version.to_lowercase();
    if let Ok(Some(caps)) = re.captures(lower.as_bytes()) {
        if !group(&caps, 3).is_empty() {
            return "dev";
        }
        match group(&caps, 1) {
            "beta" | "b" => return "beta",
            "alpha" | "a" => return "alpha",
            "rc" => return "RC",
            _ => {}
        }
    }
    "stable"
}

/// `BasePackage::STABILITIES` : rang numérique d'une stabilité.
pub fn stability_rank(stability: &str) -> i32 {
    match stability {
        "stable" => 0,
        "RC" => 5,
        "beta" => 10,
        "alpha" => 15,
        "dev" => 20,
        _ => 0,
    }
}

fn expand_stability(stability: &str) -> String {
    match stability.to_lowercase().as_str() {
        "a" => "alpha".to_owned(),
        "b" => "beta".to_owned(),
        "p" | "pl" => "patch".to_owned(),
        "rc" => "RC".to_owned(),
        other => other.to_owned(),
    }
}

/// `VersionParser::normalize($version, $fullVersion)`.
pub fn normalize(version: &str, full_version: Option<&str>) -> Result<String, VersionError> {
    static AS: OnceLock<Regex> = OnceLock::new();
    static AT: OnceLock<Regex> = OnceLock::new();
    static PLUS: OnceLock<Regex> = OnceLock::new();
    static NUM: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();
    static DEV: OnceLock<Regex> = OnceLock::new();

    let mut version = version.trim().to_owned();
    let orig = version.clone();
    let full_version = full_version
        .map(str::to_owned)
        .unwrap_or_else(|| version.clone());

    if let Ok(Some(caps)) =
        regex(&AS, r"^([^,\s]++) ++as ++([^,\s]++)$", false).captures(version.as_bytes())
    {
        version = group(&caps, 1).to_owned();
    }
    if let Ok(Some(m)) =
        regex(&AT, &format!("@(?:{STABILITIES_REGEX})$"), true).find(version.as_bytes())
    {
        version.truncate(m.start());
    }
    if matches!(version.as_str(), "master" | "trunk" | "default") {
        version = format!("dev-{version}");
    }
    if version.len() >= 4 && version[..4].eq_ignore_ascii_case("dev-") {
        return Ok(format!("dev-{}", &version[4..]));
    }
    if let Ok(Some(caps)) =
        regex(&PLUS, r"^([^,\s+]++)\+[^\s]++$", false).captures(version.as_bytes())
    {
        version = group(&caps, 1).to_owned();
    }

    let num = regex(
        &NUM,
        &format!(r"^v?(\d{{1,5}}+)(\.\d++)?(\.\d++)?(\.\d++)?{MODIFIER_REGEX}$"),
        true,
    );
    let date = regex(
        &DATE,
        &format!(
            r"^v?(\d{{4}}(?:[.:-]?\d{{2}}){{1,6}}(?:[.:-]?\d{{1,3}}){{0,2}}){MODIFIER_REGEX}$"
        ),
        true,
    );
    // Les groupes sont copiés (Vec<String>) : `version` est réassignée ensuite.
    let capture_groups = |re: &Regex, subject: &str| -> Option<Vec<String>> {
        let caps = re.captures(subject.as_bytes()).ok()??;
        Some(
            (0..caps.len())
                .map(|i| group(&caps, i).to_owned())
                .collect(),
        )
    };
    let mut index: Option<usize> = None;
    let mut groups: Vec<String> = Vec::new();
    if let Some(g) = capture_groups(num, &version) {
        let or0 = |s: &str| {
            if s.is_empty() {
                ".0".to_owned()
            } else {
                s.to_owned()
            }
        };
        version = format!("{}{}{}{}", g[1], or0(&g[2]), or0(&g[3]), or0(&g[4]));
        index = Some(5);
        groups = g;
    } else if let Some(g) = capture_groups(date, &version) {
        version = g[1]
            .chars()
            .map(|c| if c.is_ascii_digit() { c } else { '.' })
            .collect();
        index = Some(2);
        groups = g;
    }
    if let Some(index) = index {
        let at = |i: usize| groups.get(i).map(String::as_str).unwrap_or("");
        let stab = at(index);
        if !stab.is_empty() {
            if stab == "stable" {
                return Ok(version);
            }
            let numbers = at(index + 1).trim_start_matches(['.', '-']);
            version = format!("{version}-{}{numbers}", expand_stability(stab));
        }
        if !at(index + 2).is_empty() {
            version.push_str("-dev");
        }
        return Ok(version);
    }

    if let Ok(Some(caps)) = regex(&DEV, r"(.*?)[.-]?dev$", true).captures(version.as_bytes()) {
        let normalized = normalize_branch(group(&caps, 1));
        if !normalized.contains("dev-") {
            return Ok(normalized);
        }
    }

    let mut extra = String::new();
    let quoted = preg_quote(&version);
    let as_alias = RegexBuilder::new()
        .build(&format!(" +as +{quoted}(?:@(?:{STABILITIES_REGEX}))?$"))
        .ok()
        .and_then(|r| r.is_match(full_version.as_bytes()).ok())
        .unwrap_or(false);
    let as_source = RegexBuilder::new()
        .build(&format!("^{quoted}(?:@(?:{STABILITIES_REGEX}))? +as +"))
        .ok()
        .and_then(|r| r.is_match(full_version.as_bytes()).ok())
        .unwrap_or(false);
    if as_alias {
        extra = format!(" in \"{full_version}\", the alias must be an exact version");
    } else if as_source {
        extra = format!(
            " in \"{full_version}\", the alias source must be an exact version, if it is a branch name you should prefix it with dev-"
        );
    }
    Err(VersionError(format!(
        "Invalid version string \"{orig}\"{extra}"
    )))
}

/// `preg_quote` (sans délimiteur).
pub fn preg_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".\\+*?[^]$(){}=!<>|:-#/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `VersionParser::normalizeBranch`.
pub fn normalize_branch(name: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let name = name.trim();
    let re = regex(
        &RE,
        r"^v?(\d++)(\.(?:\d++|[xX*]))?(\.(?:\d++|[xX*]))?(\.(?:\d++|[xX*]))?$",
        true,
    );
    if let Ok(Some(caps)) = re.captures(name.as_bytes()) {
        let mut version = String::new();
        for i in 1..5 {
            let g = group(&caps, i);
            if caps.get(i).is_some() {
                version.push_str(&g.replace(['*', 'X'], "x"));
            } else {
                version.push_str(".x");
            }
        }
        return format!("{}-dev", version.replace('x', "9999999"));
    }
    format!("dev-{name}")
}

/// `VersionParser::parseNumericAliasPrefix`.
pub fn parse_numeric_alias_prefix(branch: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(&RE, r"^(?P<version>(\d++\.)*\d++)(?:\.x)?-dev$", true);
    let caps = re.captures(branch.as_bytes()).ok()??;
    Some(format!("{}.", group(&caps, 1)))
}

/// `VersionParser::normalizeDefaultBranch`.
pub fn normalize_default_branch(name: &str) -> String {
    if matches!(name, "dev-master" | "dev-default" | "dev-trunk") {
        DEFAULT_BRANCH_ALIAS.to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_composer() {
        assert_eq!(normalize("1.0", None).unwrap(), "1.0.0.0");
        assert_eq!(normalize("v1.2.3-beta2", None).unwrap(), "1.2.3.0-beta2");
        assert_eq!(normalize("1.0.0RC1", None).unwrap(), "1.0.0.0-RC1");
        assert_eq!(normalize("dev-main", None).unwrap(), "dev-main");
        assert_eq!(normalize("master", None).unwrap(), "dev-master");
        assert_eq!(
            normalize("1.x-dev", None).unwrap(),
            "1.9999999.9999999.9999999-dev"
        );
        assert_eq!(
            normalize("2.0.x-dev", None).unwrap(),
            "2.0.9999999.9999999-dev"
        );
        assert_eq!(normalize("1.0.0+build.1", None).unwrap(), "1.0.0.0");
        assert_eq!(normalize("1.0.0-dev", None).unwrap(), "1.0.0.0-dev");
        assert_eq!(normalize("2020-01-02", None).unwrap(), "2020.01.02");
        assert_eq!(normalize("1.0@dev", None).unwrap(), "1.0.0.0");
        assert_eq!(normalize("dev-main as 1.0", None).unwrap(), "dev-main");
        assert_eq!(normalize("1.0.0-p1", None).unwrap(), "1.0.0.0-patch1");
        assert!(normalize("nope", None).is_err());
        assert_eq!(parse_stability("1.0.0-RC1"), "RC");
        assert_eq!(parse_stability("1.0.0-beta"), "beta");
        assert_eq!(parse_stability("dev-main"), "dev");
        assert_eq!(parse_stability("1.x-dev"), "dev");
        assert_eq!(parse_stability("1.0.0"), "stable");
        assert_eq!(parse_stability("1.0.0-alpha1#abc"), "alpha");
        assert_eq!(normalize_branch("2.2"), "2.2.9999999.9999999-dev");
        assert_eq!(parse_numeric_alias_prefix("1.2.x-dev"), Some("1.2.".into()));
        assert_eq!(parse_numeric_alias_prefix("dev-main"), None);
    }
}
