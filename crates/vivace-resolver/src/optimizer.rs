//! Port de `Composer\DependencyResolver\PoolOptimizer` : retire du pool les
//! versions dont les dépendances sont identiques à une version préférée,
//! et celles qu'un paquet verrouillé rend impossibles.

use crate::constraint::{Constraint, Op};
use crate::intervals;
use crate::package::Package;
use crate::policy::DefaultPolicy;
use crate::pool::{OrderedMap, Pool, Request};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Tableau PHP à clés chaînes avec index : ordre d'insertion et accès en O(1).
#[derive(Debug, Clone)]
struct IndexedMap<V> {
    entries: Vec<(String, V)>,
    index: HashMap<String, usize>,
}

impl<V> Default for IndexedMap<V> {
    fn default() -> Self {
        IndexedMap {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<V> IndexedMap<V> {
    fn contains(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }
    /// `$map[$key] ??= $default` puis référence mutable.
    fn entry_or_insert_with(&mut self, key: &str, default: impl FnOnce() -> V) -> &mut V {
        let i = match self.index.get(key) {
            Some(&i) => i,
            None => {
                self.entries.push((key.to_owned(), default()));
                let i = self.entries.len() - 1;
                self.index.insert(key.to_owned(), i);
                i
            }
        };
        &mut self.entries[i].1
    }
    fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

pub struct PoolOptimizer {
    /// Forme textuelle d'une contrainte → ses morceaux disjonctifs
    /// (`expandDisjunctiveMultiConstraints`), mémoïsés : le même texte de
    /// lien revient des milliers de fois dans un pool.
    expansion_cache: HashMap<String, Vec<(String, Constraint)>>,
    irremovable: HashSet<usize>,
    /// name → contraintes (dédoublonnées par forme textuelle, ordre d'insertion).
    require_constraints: HashMap<String, IndexedMap<Constraint>>,
    conflict_constraints: HashMap<String, IndexedMap<Constraint>>,
    to_remove: HashSet<usize>,
    /// identifiant de base → identifiants d'alias.
    aliases_per_package: HashMap<usize, Vec<usize>>,
}

impl PoolOptimizer {
    pub fn new() -> PoolOptimizer {
        PoolOptimizer {
            expansion_cache: HashMap::new(),
            irremovable: HashSet::new(),
            require_constraints: HashMap::new(),
            conflict_constraints: HashMap::new(),
            to_remove: HashSet::new(),
            aliases_per_package: HashMap::new(),
        }
    }

    /// `optimize($request, $pool)` : le pool réduit (mêmes index d'arène,
    /// nouveaux identifiants).
    pub fn optimize(
        mut self,
        request: &Request,
        pool: &Pool,
        arena: &[Package],
        policy: &mut DefaultPolicy,
    ) -> Pool {
        self.prepare(request, pool, arena);
        self.optimize_by_identical_dependencies(pool, arena, policy);
        self.optimize_impossible_packages_away(request, pool, arena);
        let kept: Vec<usize> = (1..=pool.len())
            .filter(|id| !self.to_remove.contains(id))
            .map(|id| pool.package_by_id(id))
            .collect();
        Pool::new(kept, pool.unacceptable_fixed_or_locked.clone(), arena)
    }

    fn expand_disjunctive(constraint: &Constraint) -> Vec<Constraint> {
        let compact = intervals::compact_constraint(constraint);
        match &compact {
            Constraint::Multi {
                constraints,
                conjunctive: false,
            } => constraints.clone(),
            _ => vec![compact],
        }
    }

    /// `extractRequireConstraintsPerPackage` / `…Conflict…` : `pretty` est
    /// la clé de mémoïsation (même texte → même contrainte parsée).
    fn extract(
        cache: &mut HashMap<String, Vec<(String, Constraint)>>,
        map: &mut HashMap<String, IndexedMap<Constraint>>,
        package: &str,
        pretty: &str,
        constraint: &Constraint,
    ) {
        // `self.version` : même texte, contrainte différente par paquet.
        let pretty = if pretty == "self.version" {
            format!("\u{0}{constraint}")
        } else {
            pretty.to_owned()
        };
        let expansions = cache.entry(pretty).or_insert_with(|| {
            Self::expand_disjunctive(constraint)
                .into_iter()
                .map(|c| (c.to_string(), c))
                .collect()
        });
        let per_name = map.entry(package.to_owned()).or_default();
        for (key, expanded) in expansions {
            // `$map[$package][(string) $expanded] = $expanded` : réécriture
            // en place, même forme textuelle → même contrainte.
            if !per_name.contains(key) {
                per_name.entry_or_insert_with(key, || expanded.clone());
            }
        }
    }

    /// `prepare`.
    fn prepare(&mut self, request: &Request, pool: &Pool, arena: &[Package]) {
        let mut irremovable_groups: OrderedMap<Vec<Constraint>> = OrderedMap::default();
        for idx in request.fixed_or_locked_packages() {
            let p = &arena[idx];
            let c = Constraint::new(Op::Eq, p.version.clone());
            match irremovable_groups.0.iter_mut().find(|(k, _)| *k == p.name) {
                Some((_, list)) => list.push(c),
                None => irremovable_groups.insert(&p.name, vec![c]),
            }
        }
        for (name, constraint) in request.requires.iter() {
            // Les contraintes racine n'ont pas de texte stable sous la main :
            // leur forme affichée sert de clé.
            let pretty = format!("\u{0}{constraint}");
            Self::extract(
                &mut self.expansion_cache,
                &mut self.require_constraints,
                name,
                &pretty,
                constraint,
            );
        }
        for id in 1..=pool.len() {
            let p = &arena[pool.package_by_id(id)];
            for link in p.requires.iter() {
                Self::extract(
                    &mut self.expansion_cache,
                    &mut self.require_constraints,
                    &link.target,
                    &link.pretty_constraint,
                    &link.constraint,
                );
            }
            for link in p.conflicts.iter() {
                Self::extract(
                    &mut self.expansion_cache,
                    &mut self.conflict_constraints,
                    &link.target,
                    &link.pretty_constraint,
                    &link.constraint,
                );
            }
            if let Some(base) = p.alias_of.and_then(|b| pool.id_of(b)) {
                self.aliases_per_package.entry(base).or_default().push(id);
            }
        }
        let irremovable: HashMap<String, Constraint> = irremovable_groups
            .0
            .into_iter()
            .map(|(name, constraints)| {
                let c = if constraints.len() == 1 {
                    constraints.into_iter().next().expect("one")
                } else {
                    Constraint::Multi {
                        constraints,
                        conjunctive: false,
                    }
                };
                (name, c)
            })
            .collect();
        for id in 1..=pool.len() {
            let p = &arena[pool.package_by_id(id)];
            let Some(c) = irremovable.get(&p.name) else {
                continue;
            };
            if c.matches_version(&p.version) {
                self.mark_irremovable(id, pool, arena);
            }
        }
    }

    fn mark_irremovable(&mut self, id: usize, pool: &Pool, arena: &[Package]) {
        self.irremovable.insert(id);
        if let Some(base) = arena[pool.package_by_id(id)]
            .alias_of
            .and_then(|b| pool.id_of(b))
        {
            self.mark_irremovable(base, pool, arena);
        }
        if let Some(aliases) = self.aliases_per_package.get(&id) {
            for a in aliases.clone() {
                self.irremovable.insert(a);
            }
        }
    }

    /// `calculateDependencyHash`.
    fn dependency_hash(p: &Package) -> String {
        let mut hash = String::new();
        for (key, links) in [
            ("requires", &p.requires),
            ("conflicts", &p.conflicts),
            ("replaces", &p.replaces),
            ("provides", &p.provides),
        ] {
            if links.is_empty() {
                continue;
            }
            hash.push_str(key);
            hash.push(':');
            let mut sub: BTreeMap<Vec<u8>, String> = BTreeMap::new();
            for link in links.iter() {
                sub.insert(link.target.as_bytes().to_vec(), link.constraint.to_string());
            }
            for (target, constraint) in sub {
                hash.push_str(&String::from_utf8_lossy(&target));
                hash.push('@');
                hash.push_str(&constraint);
            }
        }
        hash
    }

    /// `optimizeByIdenticalDependencies`.
    fn optimize_by_identical_dependencies(
        &mut self,
        pool: &Pool,
        arena: &[Package],
        policy: &mut DefaultPolicy,
    ) {
        // name → groupHash → dependencyHash → ids (ordres d'insertion).
        let mut identical: IndexedMap<IndexedMap<IndexedMap<Vec<usize>>>> = IndexedMap::default();
        for id in 1..=pool.len() {
            if self.irremovable.contains(&id) {
                continue;
            }
            self.to_remove.insert(id);
            let p = &arena[pool.package_by_id(id)];
            let dependency_hash = Self::dependency_hash(p);
            // Les morceaux `replace` ne dépendent pas de la contrainte
            // examinée : une fois par paquet.
            let replace_parts: String = p
                .replaces
                .iter()
                .filter(|l| l.constraint.matches_version(&p.version))
                .map(|l| format!("require:{}", l.constraint))
                .collect();
            for name in p.names(false) {
                let Some(requires) = self.require_constraints.get(&name) else {
                    continue;
                };
                // Idem pour les conflits : une fois par nom.
                let conflict_parts: String = match self.conflict_constraints.get(&name) {
                    Some(conflicts) => conflicts
                        .iter()
                        .filter(|(_, c)| c.matches_version(&p.version))
                        .map(|(key, _)| format!("conflict:{key}"))
                        .collect(),
                    None => String::new(),
                };
                for (key, require_constraint) in requires.iter() {
                    let matched = require_constraint.matches_version(&p.version);
                    if !matched && replace_parts.is_empty() && conflict_parts.is_empty() {
                        continue;
                    }
                    let mut group_hash = String::with_capacity(
                        key.len() + 8 + replace_parts.len() + conflict_parts.len(),
                    );
                    if matched {
                        group_hash.push_str("require:");
                        group_hash.push_str(key);
                    }
                    group_hash.push_str(&replace_parts);
                    group_hash.push_str(&conflict_parts);
                    identical
                        .entry_or_insert_with(&name, IndexedMap::default)
                        .entry_or_insert_with(&group_hash, IndexedMap::default)
                        .entry_or_insert_with(&dependency_hash, Vec::new)
                        .push(id);
                }
            }
        }
        for (name, groups) in identical.entries {
            for (_, group) in groups.entries {
                for (_, ids) in group.entries {
                    if ids.len() == 1 {
                        self.keep_package_in_group(ids[0], pool, arena, &name, &ids);
                        continue;
                    }
                    let literals: Vec<i64> = ids.iter().map(|&i| i as i64).collect();
                    for preferred in policy.select_preferred_packages(pool, arena, &literals, None)
                    {
                        self.keep_package_in_group(preferred as usize, pool, arena, &name, &ids);
                    }
                }
            }
        }
    }

    /// `keepPackageInGroup` (sans `recordRemovedVersionsForPackage`, qui ne
    /// sert qu'aux messages).
    fn keep_package_in_group(
        &mut self,
        id: usize,
        pool: &Pool,
        arena: &[Package],
        _name: &str,
        _ids: &[usize],
    ) {
        if !self.to_remove.contains(&id) {
            return;
        }
        self.to_remove.remove(&id);
        let p = &arena[pool.package_by_id(id)];
        if let Some(base) = p.alias_of.and_then(|b| pool.id_of(b)) {
            self.to_remove.remove(&base);
            if let Some(aliases) = self.aliases_per_package.get(&base) {
                for a in aliases {
                    self.to_remove.remove(a);
                }
            }
            return;
        }
        if let Some(aliases) = self.aliases_per_package.get(&id) {
            for a in aliases {
                self.to_remove.remove(a);
            }
        }
    }

    /// `optimizeImpossiblePackagesAway`.
    fn optimize_impossible_packages_away(
        &mut self,
        request: &Request,
        pool: &Pool,
        arena: &[Package],
    ) {
        let locked = request.locked_packages_all();
        if locked.is_empty() {
            return;
        }
        // name → [(id, arena idx)] (ordre du pool).
        let mut index: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for id in 1..=pool.len() {
            if self.irremovable.contains(&id) {
                continue;
            }
            let idx = pool.package_by_id(id);
            let p = &arena[idx];
            if self.aliases_per_package.contains_key(&id) || p.is_alias() {
                continue;
            }
            if request.is_fixed_package(idx) || request.is_locked_package(idx) {
                continue;
            }
            index.entry(p.name.clone()).or_default().push((id, idx));
        }
        for locked_idx in locked {
            let p = &arena[locked_idx];
            let unused = p
                .names(false)
                .iter()
                .all(|n| !self.require_constraints.contains_key(n));
            if unused {
                continue;
            }
            for link in p.requires.iter() {
                let Some(candidates) = index.get_mut(&link.target) else {
                    continue;
                };
                let mut kept = Vec::new();
                for (id, idx) in candidates.iter().copied() {
                    if !link.constraint.matches_version(&arena[idx].version) {
                        self.to_remove.insert(id);
                    } else {
                        kept.push((id, idx));
                    }
                }
                *candidates = kept;
            }
        }
    }
}

impl Default for PoolOptimizer {
    fn default() -> Self {
        Self::new()
    }
}
