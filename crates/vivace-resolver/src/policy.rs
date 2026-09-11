//! Port de `Composer\DependencyResolver\DefaultPolicy` : le choix des
//! versions préférées (stabilité, plus haute/plus basse, alias racine,
//! remplacement, même vendor, identifiant de pool).

use crate::constraint::{Constraint, Op};
use crate::package::Package;
use crate::pool::Pool;
use crate::version::stability_rank;
use std::cmp::Ordering;
use std::collections::HashMap;

pub struct DefaultPolicy {
    pub prefer_stable: bool,
    pub prefer_lowest: bool,
    /// `COMPOSER_PREFER_DEV_OVER_PRERELEASE`.
    pub prefer_dev_over_prerelease: bool,
    /// `--minimal-changes` : name → version préférée.
    pub preferred_versions: Option<HashMap<String, String>>,
    /// `preferredPackageResultCachePerPool` / `sortingCachePerPool` :
    /// (identité du pool, clé).
    result_cache: HashMap<(u64, String), Vec<i64>>,
    sorting_cache: HashMap<(u64, String), Ordering>,
}

impl DefaultPolicy {
    pub fn new(
        prefer_stable: bool,
        prefer_lowest: bool,
        preferred_versions: Option<HashMap<String, String>>,
    ) -> DefaultPolicy {
        let prefer_dev = std::env::var("COMPOSER_PREFER_DEV_OVER_PRERELEASE")
            .map(|v| !(v.is_empty() || v == "0"))
            .unwrap_or(false);
        DefaultPolicy {
            prefer_stable,
            prefer_lowest,
            prefer_dev_over_prerelease: prefer_dev,
            preferred_versions,
            result_cache: HashMap::new(),
            sorting_cache: HashMap::new(),
        }
    }

    /// `versionCompare($a, $b, $operator)`.
    pub fn version_compare(&self, a: &Package, b: &Package, operator: Op) -> bool {
        if self.prefer_stable && a.stability != b.stability {
            let (mut stab_a, mut stab_b) = (a.stability, b.stability);
            if self.prefer_lowest
                && self.prefer_dev_over_prerelease
                && stab_a != "stable"
                && stab_b != "stable"
            {
                if stab_a == "dev" {
                    stab_a = "stable";
                }
                if stab_b == "dev" {
                    stab_b = "stable";
                }
            }
            return stability_rank(stab_a) < stability_rank(stab_b);
        }
        if (a.is_dev() && a.version.starts_with("dev-"))
            || (b.is_dev() && b.version.starts_with("dev-"))
        {
            let constraint = Constraint::new(operator, b.version.clone());
            let version = Constraint::new(Op::Eq, a.version.clone());
            return constraint.match_specific(&version, true);
        }
        Constraint::new(operator, b.version.clone()).matches_version(&a.version)
    }

    /// `selectPreferredPackages($pool, $literals, $requiredPackage)`.
    pub fn select_preferred_packages(
        &mut self,
        pool: &Pool,
        arena: &[Package],
        literals: &[i64],
        required_package: Option<&str>,
    ) -> Vec<i64> {
        let mut literals = literals.to_vec();
        literals.sort_unstable();
        let key = format!(
            "{}{}",
            literals
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(","),
            required_package.unwrap_or("")
        );
        let key = (pool.identity, key);
        if let Some(cached) = self.result_cache.get(&key) {
            return cached.clone();
        }
        // groupLiteralsByName (ordre de première apparition).
        let mut groups: Vec<(String, Vec<i64>)> = Vec::new();
        for &literal in &literals {
            let name = &arena[pool.literal_to_package(literal)].name;
            match groups.iter_mut().find(|(n, _)| n == name) {
                Some((_, g)) => g.push(literal),
                None => groups.push((name.clone(), vec![literal])),
            }
        }
        for (_, group) in groups.iter_mut() {
            let mut sorted = group.clone();
            sorted.sort_by(|&a, &b| {
                self.cached_compare(pool, arena, a, b, required_package, true, "i")
            });
            *group = self.prune_to_best_version(pool, arena, &sorted);
            *group = Self::prune_remote_aliases(pool, arena, group);
        }
        let mut selected: Vec<i64> = groups.into_iter().flat_map(|(_, g)| g).collect();
        selected
            .sort_by(|&a, &b| self.cached_compare(pool, arena, a, b, required_package, false, ""));
        self.result_cache.insert(key, selected.clone());
        selected
    }

    #[allow(clippy::too_many_arguments)]
    fn cached_compare(
        &mut self,
        pool: &Pool,
        arena: &[Package],
        a: i64,
        b: i64,
        required_package: Option<&str>,
        ignore_replace: bool,
        prefix: &str,
    ) -> Ordering {
        let key = (
            pool.identity,
            format!("{prefix}{a}.{b}{}", required_package.unwrap_or("")),
        );
        if let Some(o) = self.sorting_cache.get(&key) {
            return *o;
        }
        let o = Self::compare_by_priority(pool, arena, a, b, required_package, ignore_replace);
        self.sorting_cache.insert(key, o);
        o
    }

    /// `compareByPriority` sur deux littéraux (positifs) du pool.
    pub fn compare_by_priority(
        pool: &Pool,
        arena: &[Package],
        la: i64,
        lb: i64,
        required_package: Option<&str>,
        ignore_replace: bool,
    ) -> Ordering {
        let a = &arena[pool.literal_to_package(la)];
        let b = &arena[pool.literal_to_package(lb)];
        if a.name == b.name {
            let (a_alias, b_alias) = (a.is_alias(), b.is_alias());
            if a_alias && !b_alias {
                return Ordering::Less;
            }
            if !a_alias && b_alias {
                return Ordering::Greater;
            }
        }
        if !ignore_replace {
            if Self::replaces(a, b) {
                return Ordering::Greater;
            }
            if Self::replaces(b, a) {
                return Ordering::Less;
            }
            if let Some(req) = required_package {
                if let Some(pos) = req.find('/') {
                    let vendor = &req[..pos];
                    let a_same = a.name.starts_with(vendor);
                    let b_same = b.name.starts_with(vendor);
                    if a_same != b_same {
                        return if a_same {
                            Ordering::Less
                        } else {
                            Ordering::Greater
                        };
                    }
                }
            }
        }
        la.abs().cmp(&lb.abs())
    }

    fn replaces(source: &Package, target: &Package) -> bool {
        source.replaces.iter().any(|l| l.target == target.name)
    }

    /// `pruneToBestVersion`.
    fn prune_to_best_version(&self, pool: &Pool, arena: &[Package], literals: &[i64]) -> Vec<i64> {
        if let Some(preferred) = &self.preferred_versions {
            let name = &arena[pool.literal_to_package(literals[0])].name;
            if let Some(version) = preferred.get(name) {
                let best: Vec<i64> = literals
                    .iter()
                    .copied()
                    .filter(|&l| arena[pool.literal_to_package(l)].version == *version)
                    .collect();
                if !best.is_empty() {
                    return best;
                }
            }
        }
        let operator = if self.prefer_lowest { Op::Lt } else { Op::Gt };
        let mut best_literals = vec![literals[0]];
        let mut best = pool.literal_to_package(literals[0]);
        for &literal in &literals[1..] {
            let package = pool.literal_to_package(literal);
            if self.version_compare(&arena[package], &arena[best], operator) {
                best = package;
                best_literals = vec![literal];
            } else if self.version_compare(&arena[package], &arena[best], Op::Eq) {
                best_literals.push(literal);
            }
        }
        best_literals
    }

    /// `pruneRemoteAliases`.
    fn prune_remote_aliases(pool: &Pool, arena: &[Package], literals: &[i64]) -> Vec<i64> {
        let is_root_alias = |l: i64| {
            let p = &arena[pool.literal_to_package(l)];
            p.is_alias() && p.root_package_alias
        };
        if !literals.iter().any(|&l| is_root_alias(l)) {
            return literals.to_vec();
        }
        literals
            .iter()
            .copied()
            .filter(|&l| is_root_alias(l))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Origin;

    /// Les caches sont par pool (`spl_object_id($pool)`) : la même politique
    /// sert à l'optimiseur (pool complet) puis au solveur (pool réduit, ids
    /// renumérotés).
    #[test]
    fn caches_are_scoped_to_the_pool() {
        let mut arena = vec![Package::new(
            "a/a",
            "1.0.0.0",
            "1.0.0",
            Origin::Repository(0),
        )];
        let alias = arena[0].alias(0, "9999999-dev", "dev-main");
        arena.push(alias);
        arena.push(Package::new(
            "b/b",
            "1.0.0.0",
            "1.0.0",
            Origin::Repository(0),
        ));
        arena.push(Package::new(
            "c/c",
            "1.0.0.0",
            "1.0.0",
            Origin::Repository(0),
        ));
        let pool_a = Pool::new(vec![0, 1], Vec::new(), &arena);
        let pool_b = Pool::new(vec![2, 3], Vec::new(), &arena);
        let mut fresh = DefaultPolicy::new(true, false, None);
        let expected = fresh.select_preferred_packages(&pool_b, &arena, &[1, 2], None);
        let mut shared = DefaultPolicy::new(true, false, None);
        shared.select_preferred_packages(&pool_a, &arena, &[1, 2], None);
        assert_eq!(
            shared.select_preferred_packages(&pool_b, &arena, &[1, 2], None),
            expected
        );
        assert_eq!(expected, vec![1, 2]);
    }
}
