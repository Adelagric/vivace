//! Version du paquet racine — port de `RootPackageLoader::load` +
//! `VersionGuesser::guessGitVersion` (docs/reference/RootPackageLoader.php,
//! VersionGuesser.php, Composer 2.10.3). Ordre : `version` du composer.json,
//! sinon `COMPOSER_ROOT_VERSION`, sinon git (branche courante ; HEAD détaché
//! → `dev-<sha>` puis tag exact ; branche de feature → branche parente la plus
//! proche par `git rev-list`), sinon `1.0.0+no-version-set`.
//! hg/fossil/svn ne sont pas portés (fallback au défaut, comme sans VCS).

use crate::version::{normalize_pretty, UnsupportedVersion};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootVersion {
    pub pretty_version: String,
    /// Version normalisée (`version_normalized` de Composer).
    pub version: String,
    pub reference: Option<String>,
}

pub const DEFAULT_PRETTY_VERSION: &str = "1.0.0+no-version-set";

fn normalize_or_raw(v: &str) -> String {
    normalize_pretty(v).unwrap_or_else(|_| v.to_owned())
}

/// `VersionParser::normalizeBranch` (composer/semver) : `1.2` → `1.2.x.x-dev`
/// numérique (x → 9999999), sinon `dev-<name>`.
pub fn normalize_branch(name: &str) -> String {
    let name = name.trim();
    let stripped = name
        .strip_prefix('v')
        .or_else(|| name.strip_prefix('V'))
        .unwrap_or(name);
    let parts: Vec<&str> = stripped.split('.').collect();
    let numeric_or_x = |s: &str| {
        !s.is_empty() && (s.bytes().all(|b| b.is_ascii_digit()) || matches!(s, "x" | "X" | "*"))
    };
    if (1..=4).contains(&parts.len())
        && parts[0].bytes().all(|b| b.is_ascii_digit())
        && !parts[0].is_empty()
        && parts[1..].iter().all(|p| numeric_or_x(p))
    {
        let mut out: Vec<String> = parts
            .iter()
            .map(|p| {
                if matches!(*p, "x" | "X" | "*") {
                    "9999999".to_owned()
                } else {
                    (*p).to_owned()
                }
            })
            .collect();
        while out.len() < 4 {
            out.push("9999999".to_owned());
        }
        return format!("{}-dev", out.join("."));
    }
    format!("dev-{name}")
}

/// `VersionGuesser::isFeatureBranch`.
fn is_feature_branch(manifest: &Value, branch: &str) -> bool {
    let mut non_feature: Vec<String> = manifest
        .get("non-feature-branches")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    non_feature.extend(
        [
            "master", "main", "latest", "next", "current", "support", "tip", "trunk", "default",
            "develop",
        ]
        .iter()
        .map(|s| (*s).to_owned()),
    );
    if non_feature.iter().any(|n| n == branch) {
        return false;
    }
    // `\d+\..+` : branche numérique de type 1.x / 2.2
    let mut it = branch.splitn(2, '.');
    if let (Some(head), Some(rest)) = (it.next(), it.next()) {
        if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty() {
            return false;
        }
    }
    true
}

fn git(project: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(project)
        .env("GIT_DIR", project.join(".git"))
        .env("GIT_WORK_TREE", project)
        .env_remove("GIT_INDEX_FILE")
        // GitUtil::cleanEnv : sortie en anglais, jamais d'invite interactive.
        .env("LANGUAGE", "C")
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `guessGitVersion` + `postprocess`.
fn guess_git(manifest: &Value, project: &Path) -> Option<RootVersion> {
    if !project.join(".git").exists() {
        return None;
    }
    let output = git(
        project,
        &["branch", "-a", "--no-color", "--no-abbrev", "-v"],
    )?;
    let mut version: Option<String> = None;
    let mut pretty: Option<String> = None;
    let mut commit: Option<String> = None;
    let mut is_feature = false;
    let mut is_detached = false;
    let mut branches: Vec<String> = Vec::new();
    let is_hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());

    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        // Ligne courante : `* <nom|(no branch)|(HEAD detached at X)> <sha> …`
        if let Some(rest) = line.strip_prefix("* ") {
            let rest = rest.trim_start();
            let (name, tail) = if rest.starts_with('(') {
                match rest.find(')') {
                    Some(i) => (&rest[..=i], rest[i + 1..].trim_start()),
                    None => continue,
                }
            } else {
                match rest.find(' ') {
                    Some(i) => (&rest[..i], rest[i..].trim_start()),
                    None => continue,
                }
            };
            let sha = tail.split_whitespace().next().unwrap_or("");
            if !is_hex(sha) {
                continue;
            }
            if name == "(no branch)"
                || name.starts_with("(detached ")
                || name.starts_with("(HEAD detached at")
            {
                version = Some(format!("dev-{sha}"));
                pretty = version.clone();
                is_feature = true;
                is_detached = true;
            } else {
                version = Some(normalize_branch(name));
                pretty = Some(format!("dev-{name}"));
                is_feature = is_feature_branch(manifest, name);
            }
            commit = Some(sha.to_owned());
        }
        // Candidats : `[* ] <nom|remotes/origin/nom> <sha>` (nom sans `/`), hors `*/HEAD`.
        let trimmed = line.trim_start_matches("* ").trim_start();
        let mut parts = trimmed.split_whitespace();
        let (Some(name), Some(sha)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name.ends_with("/HEAD") || !is_hex(sha) {
            continue;
        }
        let bare = name
            .strip_prefix("remotes/origin/")
            .or_else(|| name.strip_prefix("remotes/upstream/"))
            .unwrap_or(name);
        if bare.contains('/') {
            continue;
        }
        branches.push(name.to_owned());
    }

    if is_feature {
        if let (Some(v), Some(_)) = (&version, &pretty) {
            let (nv, np) = guess_feature_version(manifest, v, &branches, project);
            version = Some(nv);
            pretty = Some(np);
        }
    }
    if version.is_none() || is_detached {
        if let Some(tag) = git(project, &["describe", "--exact-match", "--tags"]) {
            let tag = tag.trim();
            if let Ok(norm) = normalize_pretty(tag) {
                version = Some(norm);
                pretty = Some(tag.to_owned());
            }
        }
    }
    if commit.is_none() {
        if let Some(out) = git(project, &["rev-list", "--format=%H", "-n1", "HEAD"]) {
            commit = out
                .lines()
                .find(|l| !l.starts_with("commit "))
                .map(|l| l.trim().to_owned())
                .filter(|s| !s.is_empty());
        }
    }
    let version = version?;
    // postprocess : `X.9999999…-dev` s'affiche `X.x-dev`.
    let pretty = if version.ends_with("-dev") && version.contains(".9999999") {
        collapse_nines(&version)
    } else {
        pretty?
    };
    Some(RootVersion {
        pretty_version: pretty,
        version,
        reference: commit,
    })
}

fn collapse_nines(version: &str) -> String {
    let mut out = version.replace(".9999999", "\u{0}");
    while out.contains("\u{0}\u{0}") {
        out = out.replace("\u{0}\u{0}", "\u{0}");
    }
    out.replace('\u{0}', ".x")
}

/// `guessFeatureVersion` avec `git rev-list %candidate%..%branch%` : la
/// branche parente non-feature dont le delta est le plus court gagne.
fn guess_feature_version(
    manifest: &Value,
    version: &str,
    branches: &[String],
    project: &Path,
) -> (String, String) {
    let has_alias = manifest
        .get("extra")
        .and_then(|e| e.get("branch-alias"))
        .and_then(|b| b.get(version))
        .is_some();
    let has_self_version = manifest.to_string().contains("\"self.version\"");
    if has_alias && !has_self_version {
        return (version.to_owned(), version.to_owned());
    }
    let branch = version.strip_prefix("dev-").unwrap_or(version).to_owned();
    if !is_feature_branch(manifest, &branch) {
        return (version.to_owned(), version.to_owned());
    }
    let mut sorted: Vec<String> = branches.to_vec();
    sorted.sort_by(|a, b| {
        let (ar, br) = (a.starts_with("remotes/"), b.starts_with("remotes/"));
        if ar != br {
            return if ar {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            };
        }
        strnatcasecmp(b, a)
    });
    let mut best_len = usize::MAX;
    let mut result = (version.to_owned(), version.to_owned());
    for candidate in &sorted {
        let candidate_version = candidate
            .strip_prefix("remotes/")
            .and_then(|r| r.split_once('/').map(|x| x.1))
            .unwrap_or(candidate);
        if candidate == &branch || is_feature_branch(manifest, candidate_version) {
            continue;
        }
        let Some(out) = git(project, &["rev-list", &format!("{candidate}..{branch}")]) else {
            continue;
        };
        // À longueur égale, un candidat plus loin dans l'ordre remplace le précédent.
        if out.len() <= best_len {
            best_len = out.len();
            result = (
                normalize_branch(candidate_version),
                format!("dev-{candidate_version}"),
            );
            if best_len == 0 {
                break;
            }
        }
    }
    result
}

/// strnatcasecmp minimal (mêmes règles que vivace-autoload::natsort, dupliqué
/// pour éviter une dépendance croisée) — suffit pour trier des noms de branches.
fn strnatcasecmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        if ab[i].is_ascii_digit() && bb[j].is_ascii_digit() {
            let si = i;
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let na: u128 = a[si..i].parse().unwrap_or(0);
            let nb: u128 = b[sj..j].parse().unwrap_or(0);
            if na != nb {
                return na.cmp(&nb);
            }
        } else {
            if ab[i] != bb[j] {
                return ab[i].cmp(&bb[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (ab.len() - i).cmp(&(bb.len() - j))
}

/// Détermine la version racine comme RootPackageLoader.
pub fn detect(manifest: &Value, project: &Path) -> RootVersion {
    if let Some(v) = manifest.get("version").and_then(Value::as_str) {
        return RootVersion {
            pretty_version: v.to_owned(),
            version: normalize_or_raw(v),
            reference: None,
        };
    }
    if let Ok(env) = std::env::var("COMPOSER_ROOT_VERSION") {
        if !env.is_empty() {
            // `1.2-dev` → `1.2.x-dev`
            let v = match env.strip_suffix("-dev") {
                Some(num)
                    if !num.is_empty()
                        && num
                            .split('.')
                            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) =>
                {
                    format!("{num}.x-dev")
                }
                _ => env.clone(),
            };
            return RootVersion {
                pretty_version: v.clone(),
                version: normalize_or_raw(&v),
                reference: None,
            };
        }
    }
    if let Some(g) = guess_git(manifest, project) {
        return g;
    }
    RootVersion {
        pretty_version: DEFAULT_PRETTY_VERSION.to_owned(),
        version: "1.0.0.0".to_owned(),
        reference: None,
    }
}

/// `VersionParser::DEFAULT_BRANCH_ALIAS`.
pub const DEFAULT_BRANCH_ALIAS: &str = "9999999-dev";

/// `VersionParser::parseNumericAliasPrefix` (composer/semver) : `1.2.x-dev` et
/// `1.2-dev` → `1.2.`, sinon None. Insensible à la casse comme le motif PCRE.
pub fn parse_numeric_alias_prefix(branch: &str) -> Option<String> {
    let n = branch.len();
    if n < 4 || !branch.is_char_boundary(n - 4) || !branch[n - 4..].eq_ignore_ascii_case("-dev") {
        return None;
    }
    let mut rest = &branch[..n - 4];
    if let Some(r) = rest.strip_suffix(".x").or_else(|| rest.strip_suffix(".X")) {
        rest = r;
    }
    let numeric = !rest.is_empty()
        && rest
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    numeric.then(|| format!("{rest}."))
}

/// Version jolie d'un alias normalisé, comme ArrayLoader :
/// `preg_replace('{(\.9{7})+}', '.x', …)`.
fn pretty_alias(normalized: &str) -> String {
    const X: &str = ".9999999";
    let mut out = String::with_capacity(normalized.len());
    let mut rest = normalized;
    while let Some(i) = rest.find(X) {
        out.push_str(&rest[..i]);
        out.push_str(".x");
        rest = &rest[i + X.len()..];
        while let Some(r) = rest.strip_prefix(X) {
            rest = r;
        }
    }
    out.push_str(rest);
    out
}

/// `ArrayLoader::getBranchAlias` (Composer 2.10.3) : l'alias que Composer
/// attache à un paquet (racine ou verrouillé) dont la version est une branche
/// (`dev-*` ou `*-dev`). Priorité à `extra.branch-alias` (cible `-dev`,
/// normalisée par normalizeBranch, source égale à la version sans casse,
/// préfixe numérique compatible), sinon `9999999-dev` si `default-branch`
/// est vrai et que la version n'a pas de préfixe numérique.
/// Retourne (alias normalisé, alias joli) — le joli est celui d'installed.php.
pub fn branch_alias_of(
    version: &str,
    extra: Option<&Value>,
    default_branch: bool,
) -> Option<(String, String)> {
    if !(version.starts_with("dev-") || version.ends_with("-dev")) {
        return None;
    }
    if let Some(map) = extra
        .and_then(|e| e.get("branch-alias"))
        .and_then(Value::as_object)
    {
        for (source, target) in map {
            let Some(target) = target.as_str() else {
                continue;
            };
            let Some(target_base) = target.strip_suffix("-dev") else {
                continue;
            };
            let validated = if target == DEFAULT_BRANCH_ALIAS {
                target.to_owned()
            } else {
                normalize_branch(target_base)
            };
            if !validated.ends_with("-dev") {
                continue;
            }
            if version.to_lowercase() != source.to_lowercase() {
                continue;
            }
            if let (Some(sp), Some(tp)) = (
                parse_numeric_alias_prefix(source),
                parse_numeric_alias_prefix(target),
            ) {
                if !tp.to_lowercase().starts_with(&sp.to_lowercase()) {
                    continue;
                }
            }
            let pretty = pretty_alias(&validated);
            return Some((validated, pretty));
        }
    }
    if default_branch {
        let v = version.strip_prefix('v').unwrap_or(version);
        if parse_numeric_alias_prefix(v).is_none() {
            return Some((
                DEFAULT_BRANCH_ALIAS.to_owned(),
                DEFAULT_BRANCH_ALIAS.to_owned(),
            ));
        }
    }
    None
}

/// Alias de branche de la racine : getBranchAlias sur le composer.json, la
/// version étant la version jolie retenue par RootPackageLoader.
pub fn branch_alias(manifest: &Value, root: &RootVersion) -> Option<(String, String)> {
    let default_branch = manifest
        .get("default-branch")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    branch_alias_of(&root.pretty_version, manifest.get("extra"), default_branch)
}

impl std::fmt::Display for UnsupportedVersionAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
#[derive(Debug)]
pub struct UnsupportedVersionAlias(pub UnsupportedVersion);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_branch_matches_semver() {
        assert_eq!(normalize_branch("main"), "dev-main");
        assert_eq!(normalize_branch("2.2"), "2.2.9999999.9999999-dev");
        assert_eq!(normalize_branch("1.x"), "1.9999999.9999999.9999999-dev");
        assert_eq!(normalize_branch("v3"), "3.9999999.9999999.9999999-dev");
        assert_eq!(normalize_branch("feature/x"), "dev-feature/x");
    }

    #[test]
    fn feature_branches() {
        let m = json!({});
        assert!(!is_feature_branch(&m, "main"));
        assert!(!is_feature_branch(&m, "develop"));
        assert!(!is_feature_branch(&m, "2.2"));
        assert!(is_feature_branch(&m, "feature-x"));
        let m = json!({"non-feature-branches": ["release-.*"]});
        assert!(is_feature_branch(&m, "release-1")); // la valeur est utilisée comme regex chez Composer : littéral ici
    }

    #[test]
    fn collapse() {
        assert_eq!(collapse_nines("2.2.9999999.9999999-dev"), "2.2.x-dev");
        assert_eq!(collapse_nines("1.9999999.9999999.9999999-dev"), "1.x-dev");
    }

    #[test]
    fn detect_in_git_repo() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = tmp.path();
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .expect("git");
            assert!(st.success(), "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(p.join("a.txt"), "a").expect("write");
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        let r = detect(&json!({}), p);
        assert_eq!(r.pretty_version, "dev-main");
        assert_eq!(r.version, "dev-main");
        assert_eq!(r.reference.as_deref().map(str::len), Some(40));

        // Branche de feature : la parente (main) est retenue.
        run(&["checkout", "-q", "-b", "feature-x"]);
        std::fs::write(p.join("b.txt"), "b").expect("write");
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "feat"]);
        let r = detect(&json!({}), p);
        assert_eq!(r.pretty_version, "dev-main");

        // Branche numérique + alias.
        run(&["checkout", "-q", "-b", "2.2"]);
        let m = json!({"extra": {"branch-alias": {"dev-2.2": "2.2.x-dev"}}});
        let r = detect(&m, p);
        assert_eq!(r.version, "2.2.9999999.9999999-dev");
        assert_eq!(r.pretty_version, "2.2.x-dev");
        assert_eq!(
            branch_alias(&m, &r),
            None,
            "l'alias est indexé par dev-2.2, pas par la version jolie x-dev"
        );

        // Tag exact sur HEAD détaché.
        run(&["tag", "v1.2.3"]);
        run(&["checkout", "-q", "--detach", "HEAD"]);
        let r = detect(&json!({}), p);
        assert_eq!(r.pretty_version, "v1.2.3");
        assert_eq!(r.version, "1.2.3.0");
    }
}
