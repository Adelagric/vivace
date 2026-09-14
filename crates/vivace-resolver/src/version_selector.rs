//! Port de `Composer\Package\Version\VersionSelector`
//! (docs/reference/VersionSelector.php) : le meilleur candidat d'un nom
//! (stabilité préférée, puis version décroissante, filtré par les
//! exigences de plateforme) et la contrainte recommandée pour `require`
//! (`^x.y`, `@stability`, alias de branche).
//!
//! La collecte des candidats (`RepositorySet::findPackages`) est laissée à
//! l'appelant : `find_best_candidate` reçoit les indices déjà chargés.

use std::collections::{BTreeMap, BTreeSet};

use pcre2::bytes::Regex;
use serde_json::{Map, Value};

use crate::constraint::{Constraint, Op};
use crate::package::Package;
use crate::phpver::version_compare;
use crate::platform::is_platform_package;
use crate::platform_filter::PlatformRequirementFilter;
use crate::version::{stability_rank, DEFAULT_BRANCH_ALIAS};

/// `$this->platformConstraints` : nom → `[Constraint('==', version)]` des
/// paquets du `PlatformRepository`.
pub fn platform_constraints(
    platform: &[usize],
    arena: &[Package],
) -> BTreeMap<String, Vec<Constraint>> {
    let mut out: BTreeMap<String, Vec<Constraint>> = BTreeMap::new();
    for &idx in platform {
        let p = &arena[idx];
        out.entry(p.name.clone())
            .or_default()
            .push(Constraint::new(Op::Eq, &p.version));
    }
    out
}

/// Un avertissement `Cannot use …` ; `verbose` vaut `IOInterface::VERBOSE`
/// (répétition pour le même couple paquet/cible, cachée en mode normal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub message: String,
    pub verbose: bool,
}

/// `findBestCandidate` sur des candidats déjà trouvés : tri par stabilité
/// préférée puis version, filtre de plateforme, alias `9999999-dev` rendu
/// par son paquet de base. Les avertissements sont rendus dans l'ordre.
pub fn find_best_candidate(
    candidates: &[usize],
    arena: &[Package],
    preferred_stability: &str,
    filter: &PlatformRequirementFilter,
    platform: &BTreeMap<String, Vec<Constraint>>,
    warnings: &mut Vec<Warning>,
) -> Option<usize> {
    let min_priority = stability_rank(preferred_stability);
    let mut sorted: Vec<usize> = candidates.to_vec();
    // `usort` : les stabilités acceptées d'abord (version décroissante),
    // puis les autres par stabilité croissante ; un ordre total, donc
    // indépendant de l'algorithme de tri (stable des deux côtés).
    sorted.sort_by(|&a, &b| {
        let (pa, pb) = (&arena[a], &arena[b]);
        let (ra, rb) = (stability_rank(pa.stability), stability_rank(pb.stability));
        if min_priority < ra && rb < ra {
            return std::cmp::Ordering::Greater;
        }
        if min_priority < ra && ra < rb {
            return std::cmp::Ordering::Less;
        }
        if min_priority >= ra && min_priority < rb {
            return std::cmp::Ordering::Less;
        }
        version_compare(&pb.version, &pa.version)
    });

    let mut chosen = None;
    if !platform.is_empty() && !matches!(filter, PlatformRequirementFilter::IgnoreAll) {
        let mut already_warned: BTreeSet<String> = BTreeSet::new();
        let mut already_seen: BTreeSet<String> = BTreeSet::new();
        'candidates: for &idx in &sorted {
            let pkg = &arena[idx];
            let mut skip = false;
            'links: for link in pkg.requires.iter() {
                let name = link.key.as_deref().unwrap_or(&link.target);
                if !is_platform_package(name) || filter.is_ignored(name) {
                    continue;
                }
                let reason = if let Some(provided) = platform.get(name) {
                    for provided_constraint in provided {
                        if link.constraint.matches(provided_constraint) {
                            continue 'links;
                        }
                        if filter.is_upper_bound_ignored(name)
                            && filter
                                .filter_constraint(name, &link.constraint, true)
                                .matches(provided_constraint)
                        {
                            continue 'links;
                        }
                    }
                    "is not satisfied by your platform"
                } else {
                    "is missing from your platform"
                };
                let is_latest = !already_seen.contains(&pkg.name);
                already_seen.insert(pkg.name.clone());
                let key = format!("{}/{}", pkg.name, link.target);
                let first = !already_warned.contains(&key);
                already_warned.insert(key);
                let latest = if is_latest { "'s latest version" } else { "" };
                warnings.push(Warning {
                    message: format!(
                        "Cannot use {}{latest} {} as it {} {} {} which {reason}.",
                        pkg.pretty_name,
                        pkg.pretty_version,
                        link.kind.description(),
                        link.target,
                        link.pretty_constraint
                    ),
                    verbose: !first,
                });
                skip = true;
            }
            if skip {
                continue 'candidates;
            }
            chosen = Some(idx);
            break;
        }
    } else {
        chosen = sorted.first().copied();
    }

    let idx = chosen?;
    let pkg = &arena[idx];
    if let Some(base) = pkg.alias_of {
        if pkg.version == DEFAULT_BRANCH_ALIAS {
            return Some(base);
        }
    }
    Some(idx)
}

/// `findRecommendedRequireVersion` ; `php_version` est
/// `PHP_MAJOR.MINOR.RELEASE` du PHP local (pour la règle des `ext-*`).
pub fn find_recommended_require_version(
    pkg: &Package,
    arena: &[Package],
    php_version: &str,
) -> String {
    if pkg.name.starts_with("ext-") {
        let ext_version: Vec<&str> = pkg.version.split('.').take(3).collect();
        if php_version == ext_version.join(".") {
            return "*".to_owned();
        }
    }
    if !pkg.is_dev() {
        return transform_version(&pkg.version, &pkg.pretty_version, pkg.stability);
    }
    // `$loader->getBranchAlias($dumper->dump($package))` : la version
    // jolie, `extra` et `default-branch` du paquet (d'un alias : ceux du
    // paquet aliasé, sauf la version).
    let base = pkg.alias_of.map(|b| &arena[b]).unwrap_or(pkg);
    let mut dumped = Map::new();
    dumped.insert("version".into(), Value::String(pkg.pretty_version.clone()));
    if let Some(extra) = base.raw.get("extra") {
        dumped.insert("extra".into(), extra.clone());
    }
    if base.is_default_branch {
        dumped.insert("default-branch".into(), Value::Bool(true));
    }
    if let Some(extra) = crate::loader::branch_alias(&dumped) {
        if extra != DEFAULT_BRANCH_ALIAS {
            static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
            let re = RE.get_or_init(|| {
                Regex::new(r"^(\d+\.\d+\.\d+)(\.9999999)-dev$").unwrap_or_else(|e| panic!("{e}"))
            });
            if let Ok(Some(m)) = re.captures(extra.as_bytes()) {
                let head = m
                    .get(1)
                    .and_then(|g| std::str::from_utf8(g.as_bytes()).ok())
                    .unwrap_or("");
                let replaced = format!("{head}.0").replace(".9999999", ".0");
                return transform_version(&replaced, &replaced, "dev");
            }
        }
    }
    pkg.pretty_version.clone()
}

/// `transformVersion` : `x.y.z.w` → `^x.y` (`^0.y.z` sous 1.0), suffixe
/// `@stability` hors stable ; sinon la version jolie telle quelle.
fn transform_version(version: &str, pretty_version: &str, stability: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    let fourth_ok = parts.len() == 4 && parts[3].starts_with(|c: char| c.is_ascii_digit());
    if !fourth_ok {
        return pretty_version.to_owned();
    }
    let kept: &[&str] = if parts[0] == "0" {
        &parts[..3]
    } else {
        &parts[..2]
    };
    let mut version = kept.join(".");
    if stability != "stable" {
        version.push('@');
        version.push_str(stability);
    }
    format!("^{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_versions() {
        assert_eq!(transform_version("1.2.3.0", "1.2.3", "stable"), "^1.2");
        assert_eq!(transform_version("0.2.3.0", "0.2.3", "stable"), "^0.2.3");
        assert_eq!(
            transform_version("1.0.0.0-RC1", "1.0.0-RC1", "RC"),
            "^1.0@RC"
        );
        assert_eq!(
            transform_version("2.0.0.0-beta2", "v2.0.0-beta2", "beta"),
            "^2.0@beta"
        );
        assert_eq!(transform_version("dev-main", "dev-main", "dev"), "dev-main");
        assert_eq!(
            transform_version("20240101", "20240101", "stable"),
            "20240101"
        );
    }
}
