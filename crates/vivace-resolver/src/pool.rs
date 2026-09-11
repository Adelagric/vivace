//! Port de `Composer\DependencyResolver\{Request, PoolBuilder, Pool}` et de
//! `Composer\Repository\RepositorySet` (docs/reference/resolver/). Le pool
//! est la liste ordonnée des paquets que le solveur verra : l'ordre est
//! celui de Composer, index par index, parce que les identifiants de
//! littéraux en dépendent.

use crate::constraint::Constraint;
use crate::intervals;
use crate::package::{Origin, Package};
use crate::platform::is_platform_package;
use crate::repository::{is_package_acceptable, ComposerRepository, RepoError};
use crate::root::RootAlias;
use crate::version::{regex, stability_rank};
use pcre2::bytes::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PoolError(pub String);

impl From<RepoError> for PoolError {
    fn from(e: RepoError) -> PoolError {
        PoolError(e.0)
    }
}

/// Tableau PHP à clés chaînes : ordre d'insertion, réécriture en place.
#[derive(Debug, Clone)]
pub struct OrderedMap<V>(pub Vec<(String, V)>);

impl<V> Default for OrderedMap<V> {
    fn default() -> OrderedMap<V> {
        OrderedMap(Vec::new())
    }
}

impl<V> OrderedMap<V> {
    pub fn get(&self, key: &str) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
    pub fn insert(&mut self, key: &str, value: V) {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            self.0.push((key.to_owned(), value));
        }
    }
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let pos = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(pos).1)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.0.iter().map(|(k, v)| (k, v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }
}

/// `Request::UPDATE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    OnlyListed,
    ListedWithTransitiveDepsNoRootRequire,
    ListedWithTransitiveDeps,
}

/// `Composer\DependencyResolver\Request`. Les paquets sont des index
/// d'arène ; les tableaux PHP indexés par `spl_object_id` deviennent des
/// listes ordonnées sans doublon.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub locked_repository: Option<Vec<usize>>,
    pub requires: OrderedMap<Constraint>,
    pub fixed_packages: Vec<usize>,
    pub locked_packages: Vec<usize>,
    pub fixed_locked_packages: Vec<usize>,
    pub update_allow_list: Vec<String>,
    pub update_mode: Option<UpdateMode>,
    pub restricted_packages: Option<Vec<String>>,
}

fn push_unique(list: &mut Vec<usize>, idx: usize) {
    if !list.contains(&idx) {
        list.push(idx);
    }
}

impl Request {
    pub fn new(locked_repository: Option<Vec<usize>>) -> Request {
        Request {
            locked_repository,
            ..Request::default()
        }
    }

    pub fn require_name(
        &mut self,
        name: &str,
        constraint: Option<Constraint>,
    ) -> Result<(), PoolError> {
        let name = name.to_lowercase();
        let constraint = constraint.unwrap_or(Constraint::MatchAll);
        if let Some(existing) = self.requires.get(&name) {
            return Err(PoolError(format!(
                "Overwriting requires seems like a bug ({name} {existing} => {constraint}, check why it is happening, might be a root alias"
            )));
        }
        self.requires.insert(&name, constraint);
        Ok(())
    }

    pub fn fix_package(&mut self, idx: usize) {
        push_unique(&mut self.fixed_packages, idx);
    }

    pub fn lock_package(&mut self, idx: usize) {
        push_unique(&mut self.locked_packages, idx);
    }

    pub fn fix_locked_package(&mut self, idx: usize) {
        push_unique(&mut self.fixed_packages, idx);
        push_unique(&mut self.fixed_locked_packages, idx);
    }

    pub fn unlock_package(&mut self, idx: usize) {
        self.locked_packages.retain(|i| *i != idx);
    }

    pub fn set_update_allow_list(&mut self, list: Vec<String>, mode: UpdateMode) {
        self.update_allow_list = list;
        self.update_mode = Some(mode);
    }

    pub fn update_allow_transitive_dependencies(&self) -> bool {
        // `$this->updateAllowTransitiveDependencies !== self::UPDATE_ONLY_LISTED`
        // : vrai aussi quand aucune liste n'a été posée (`false !== 0`).
        self.update_mode != Some(UpdateMode::OnlyListed)
    }

    pub fn update_allow_transitive_root_dependencies(&self) -> bool {
        self.update_mode == Some(UpdateMode::ListedWithTransitiveDeps)
    }

    pub fn is_fixed_package(&self, idx: usize) -> bool {
        self.fixed_packages.contains(&idx)
    }

    pub fn is_locked_package(&self, idx: usize) -> bool {
        self.locked_packages.contains(&idx) || self.fixed_locked_packages.contains(&idx)
    }

    /// `array_merge($lockedPackages, $fixedLockedPackages)`.
    pub fn locked_packages_all(&self) -> Vec<usize> {
        let mut out = self.locked_packages.clone();
        out.extend(self.fixed_locked_packages.iter().copied());
        out
    }

    /// `array_merge($fixedPackages, $lockedPackages)`.
    pub fn fixed_or_locked_packages(&self) -> Vec<usize> {
        let mut out = self.fixed_packages.clone();
        out.extend(self.locked_packages.iter().copied());
        out
    }
}

/// Un dépôt du `RepositorySet`, dans l'ordre d'ajout.
pub enum Repository {
    /// `RootPackageRepository` : [alias racine ?, racine].
    Root(Vec<usize>),
    /// `PlatformRepository`.
    Platform(Vec<usize>),
    Composer(Box<ComposerRepository>),
    /// `LockArrayRepository`.
    Locked(Vec<usize>),
}

/// `Composer\Repository\RepositorySet`.
pub struct RepositorySet {
    /// name → version → (alias, alias_normalized) (`getRootAliasesPerPackage`).
    pub root_aliases: BTreeMap<String, BTreeMap<String, (String, String)>>,
    pub root_references: BTreeMap<String, String>,
    pub acceptable_stabilities: BTreeMap<String, i32>,
    pub stability_flags: BTreeMap<String, i32>,
    pub root_requires: OrderedMap<Constraint>,
    pub temporary_constraints: BTreeMap<String, Constraint>,
    pub repositories: Vec<Repository>,
}

impl RepositorySet {
    pub fn new(
        minimum_stability: &str,
        stability_flags: BTreeMap<String, i32>,
        root_aliases: &[RootAlias],
        root_references: BTreeMap<String, String>,
        root_requires: OrderedMap<Constraint>,
        temporary_constraints: BTreeMap<String, Constraint>,
    ) -> RepositorySet {
        let mut aliases: BTreeMap<String, BTreeMap<String, (String, String)>> = BTreeMap::new();
        for a in root_aliases {
            aliases.entry(a.package.clone()).or_default().insert(
                a.version.clone(),
                (a.alias.clone(), a.alias_normalized.clone()),
            );
        }
        let min = stability_rank(minimum_stability);
        let mut acceptable = BTreeMap::new();
        for s in ["stable", "RC", "beta", "alpha", "dev"] {
            let rank = stability_rank(s);
            if rank <= min {
                acceptable.insert(s.to_owned(), rank);
            }
        }
        let mut requires = OrderedMap::default();
        for (name, c) in root_requires.iter() {
            if !is_platform_package(name) {
                requires.insert(name, c.clone());
            }
        }
        RepositorySet {
            root_aliases: aliases,
            root_references,
            acceptable_stabilities: acceptable,
            stability_flags,
            root_requires: requires,
            temporary_constraints,
            repositories: Vec::new(),
        }
    }

    pub fn add_repository(&mut self, repo: Repository) {
        self.repositories.push(repo);
    }

    /// `createPool` sans optimiseur ni filtres.
    pub fn create_pool(
        &self,
        request: &mut Request,
        arena: &mut Vec<Package>,
    ) -> Result<Pool, PoolError> {
        let mut builder = PoolBuilder::new(self);
        builder.build_pool(&self.repositories, request, arena)
    }
}

/// `Composer\DependencyResolver\Pool` : identifiants 1-based dans l'ordre
/// de construction.
#[derive(Debug, Clone, Default)]
pub struct Pool {
    /// Identité du pool (`spl_object_id($pool)` chez Composer) : les caches
    /// de la politique sont indexés par pool.
    pub identity: u64,
    /// id - 1 → index d'arène.
    pub packages: Vec<usize>,
    id_of: HashMap<usize, usize>,
    package_by_name: HashMap<String, Vec<usize>>,
    pub unacceptable_fixed_or_locked: Vec<usize>,
    pub warnings: Vec<String>,
}

impl Pool {
    pub fn new(packages: Vec<usize>, unacceptable: Vec<usize>, arena: &[Package]) -> Pool {
        static NEXT_IDENTITY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut pool = Pool {
            identity: NEXT_IDENTITY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            packages: Vec::with_capacity(packages.len()),
            id_of: HashMap::new(),
            package_by_name: HashMap::new(),
            unacceptable_fixed_or_locked: unacceptable,
            warnings: Vec::new(),
        };
        for idx in packages {
            pool.packages.push(idx);
            let id = pool.packages.len();
            pool.id_of.insert(idx, id);
            for name in arena[idx].names(true) {
                pool.package_by_name.entry(name).or_default().push(id);
            }
        }
        pool
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// `packageById` → index d'arène.
    pub fn package_by_id(&self, id: usize) -> usize {
        self.packages[id - 1]
    }

    pub fn id_of(&self, arena_idx: usize) -> Option<usize> {
        self.id_of.get(&arena_idx).copied()
    }

    pub fn literal_to_package(&self, literal: i64) -> usize {
        self.package_by_id(literal.unsigned_abs() as usize)
    }

    /// `whatProvides` → identifiants de pool.
    pub fn what_provides(
        &self,
        arena: &[Package],
        name: &str,
        constraint: Option<&Constraint>,
    ) -> Vec<usize> {
        let Some(candidates) = self.package_by_name.get(name) else {
            return Vec::new();
        };
        candidates
            .iter()
            .copied()
            .filter(|id| Self::matches(&arena[self.packages[id - 1]], name, constraint))
            .collect()
    }

    /// `Pool::match`.
    pub fn matches(candidate: &Package, name: &str, constraint: Option<&Constraint>) -> bool {
        if candidate.name == name {
            return match constraint {
                None => true,
                Some(c) => c.matches_version(&candidate.version),
            };
        }
        let provides = &candidate.provides;
        let replaces = &candidate.replaces;
        // `isset($replaces[0]) || isset($provides[0])` : clés numériques
        // (liens self.version d'un alias) → parcours par cible ; sinon
        // recherche par clé (`isset($provides[$name])`), qui n'est pas
        // toujours la cible (lib-* de la plateforme).
        if replaces.has_numeric_keys() || provides.has_numeric_keys() {
            for link in provides.iter().chain(replaces.iter()) {
                if link.target == name && constraint.is_none_or(|c| c.matches(&link.constraint)) {
                    return true;
                }
            }
            return false;
        }
        if let Some(link) = provides.get(name) {
            if constraint.is_none_or(|c| c.matches(&link.constraint)) {
                return true;
            }
        }
        if let Some(link) = replaces.get(name) {
            if constraint.is_none_or(|c| c.matches(&link.constraint)) {
                return true;
            }
        }
        false
    }

    pub fn is_unacceptable_fixed_or_locked(&self, arena_idx: usize) -> bool {
        self.unacceptable_fixed_or_locked.contains(&arena_idx)
    }
}

const LOAD_BATCH_SIZE: usize = 50;

/// `BasePackage::packageNameToRegexp`.
pub fn package_name_regexp(pattern: &str) -> Regex {
    let quoted = crate::version::preg_quote(pattern).replace("\\*", ".*");
    pcre2::bytes::RegexBuilder::new()
        .caseless(true)
        .build(&format!("^{quoted}$"))
        .unwrap_or_else(|e| panic!("pattern {pattern}: {e}"))
}

struct PoolBuilder<'a> {
    set: &'a RepositorySet,
    /// base arena idx → [(index de pool, alias arena idx)].
    alias_map: HashMap<usize, Vec<(usize, usize)>>,
    packages_to_load: OrderedMap<Constraint>,
    loaded_packages: BTreeMap<String, Constraint>,
    loaded_per_repo: BTreeMap<usize, BTreeMap<String, BTreeSet<String>>>,
    /// Index de pool → arena idx (`unset` = None).
    packages: Vec<Option<usize>>,
    unacceptable: Vec<usize>,
    update_allow_list: Vec<String>,
    update_allow_patterns: Vec<Regex>,
    skipped_load: BTreeMap<String, Vec<usize>>,
    ignored_types: Vec<String>,
    allowed_types: Option<Vec<String>>,
    restricted: Option<BTreeSet<String>>,
    path_repo_unlocked: BTreeSet<String>,
    max_extended_reqs: BTreeSet<String>,
    update_allow_warned: BTreeSet<String>,
    warnings: Vec<String>,
}

impl<'a> PoolBuilder<'a> {
    fn new(set: &'a RepositorySet) -> PoolBuilder<'a> {
        PoolBuilder {
            set,
            alias_map: HashMap::new(),
            packages_to_load: OrderedMap::default(),
            loaded_packages: BTreeMap::new(),
            loaded_per_repo: BTreeMap::new(),
            packages: Vec::new(),
            unacceptable: Vec::new(),
            update_allow_list: Vec::new(),
            update_allow_patterns: Vec::new(),
            skipped_load: BTreeMap::new(),
            ignored_types: Vec::new(),
            allowed_types: None,
            restricted: None,
            path_repo_unlocked: BTreeSet::new(),
            max_extended_reqs: BTreeSet::new(),
            update_allow_warned: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    fn loaded_packages_in_pool(&self) -> Vec<usize> {
        self.packages.iter().flatten().copied().collect()
    }

    fn build_pool(
        &mut self,
        repositories: &[Repository],
        request: &mut Request,
        arena: &mut Vec<Package>,
    ) -> Result<Pool, PoolError> {
        self.restricted = request
            .restricted_packages
            .as_ref()
            .map(|l| l.iter().cloned().collect());

        if !request.update_allow_list.is_empty() {
            self.update_allow_list = request.update_allow_list.clone();
            self.update_allow_patterns = self
                .update_allow_list
                .iter()
                .map(|p| package_name_regexp(p))
                .collect();
            self.warn_about_non_matching_update_allow_list(request, arena)?;
            let Some(locked) = request.locked_repository.clone() else {
                return Err(PoolError(
                    "No lock repo present and yet a partial update was requested.".into(),
                ));
            };
            for locked_idx in locked {
                if !self.is_update_allowed(&arena[locked_idx]) {
                    let p = &arena[locked_idx];
                    self.skipped_load
                        .entry(p.name.clone())
                        .or_default()
                        .push(locked_idx);
                    for link in p.replaces.iter() {
                        self.skipped_load
                            .entry(link.target.clone())
                            .or_default()
                            .push(locked_idx);
                    }
                    if p.dist.as_ref().is_some_and(|d| d.kind == "path") {
                        let symlink = p
                            .raw
                            .get("transport-options")
                            .and_then(|t| t.get("symlink"))
                            .cloned();
                        if symlink != Some(serde_json::Value::Bool(false)) {
                            self.path_repo_unlocked.insert(p.name.clone());
                            continue;
                        }
                    }
                    request.lock_package(locked_idx);
                }
            }
        }

        for idx in request.fixed_or_locked_packages() {
            let (name, replaces, names, stability, origin) = {
                let p = &arena[idx];
                (
                    p.name.clone(),
                    p.replaces
                        .iter()
                        .map(|l| l.target.clone())
                        .collect::<Vec<_>>(),
                    p.names(true),
                    p.stability,
                    p.origin,
                )
            };
            self.loaded_packages.insert(name, Constraint::MatchAll);
            for target in replaces {
                self.loaded_packages.insert(target, Constraint::MatchAll);
            }
            if matches!(origin, Origin::Root | Origin::Platform)
                || is_package_acceptable(
                    &self.set.acceptable_stabilities,
                    &self.set.stability_flags,
                    &names,
                    stability,
                )
            {
                self.load_package(request, idx, false, arena);
            } else {
                self.unacceptable.push(idx);
            }
        }

        let requires: Vec<(String, Constraint)> = request.requires.0.clone();
        for (name, constraint) in &requires {
            if self.loaded_packages.contains_key(name) {
                continue;
            }
            self.packages_to_load.insert(name, constraint.clone());
            self.max_extended_reqs.insert(name.clone());
        }
        let already: Vec<String> = self
            .packages_to_load
            .keys()
            .filter(|n| self.loaded_packages.contains_key(*n))
            .cloned()
            .collect();
        for name in already {
            self.packages_to_load.remove(&name);
        }

        while !self.packages_to_load.is_empty() {
            self.load_packages_marked_for_loading(request, repositories, arena)?;
        }

        if !self.set.temporary_constraints.is_empty() {
            let entries: Vec<(usize, usize)> = self
                .packages
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.map(|idx| (i, idx)))
                .collect();
            for (i, idx) in entries {
                if arena[idx].is_alias() {
                    continue;
                }
                for name in arena[idx].names(true) {
                    let Some(constraint) = self.set.temporary_constraints.get(&name) else {
                        continue;
                    };
                    let mut package_and_aliases: Vec<(usize, usize)> = vec![(i, idx)];
                    if let Some(aliases) = self.alias_map.get(&idx) {
                        package_and_aliases.extend(aliases.iter().copied());
                    }
                    let found = package_and_aliases
                        .iter()
                        .any(|(_, p)| constraint.matches_version(&arena[*p].version));
                    if !found {
                        for (pool_index, _) in package_and_aliases {
                            self.packages[pool_index] = None;
                        }
                    }
                }
            }
        }

        let packages = self.loaded_packages_in_pool();
        let mut pool = Pool::new(packages, std::mem::take(&mut self.unacceptable), arena);
        pool.warnings = std::mem::take(&mut self.warnings);
        Ok(pool)
    }

    /// `markPackageNameForLoading`.
    fn mark_package_name_for_loading(
        &mut self,
        request: &Request,
        name: &str,
        constraint: &Constraint,
    ) {
        if is_platform_package(name) {
            return;
        }
        if self.max_extended_reqs.contains(name) {
            return;
        }
        let mut constraint = constraint.clone();
        if let Some(root) = request.requires.get(name) {
            if !intervals::is_subset_of(&constraint, root) {
                constraint = root.clone();
            }
        }
        if !self.loaded_packages.contains_key(name) {
            if let Some(pending) = self.packages_to_load.get(name) {
                if intervals::is_subset_of(&constraint, pending) {
                    return;
                }
                constraint = intervals::compact_constraint(&Constraint::create(
                    vec![pending.clone(), constraint],
                    false,
                ));
            }
            self.packages_to_load.insert(name, constraint);
            return;
        }
        let loaded = &self.loaded_packages[name];
        if intervals::is_subset_of(&constraint, loaded) {
            return;
        }
        let merged = intervals::compact_constraint(&Constraint::create(
            vec![loaded.clone(), constraint],
            false,
        ));
        self.packages_to_load.insert(name, merged);
        self.loaded_packages.remove(name);
    }

    /// `loadPackagesMarkedForLoading`.
    fn load_packages_marked_for_loading(
        &mut self,
        request: &mut Request,
        repositories: &[Repository],
        arena: &mut Vec<Package>,
    ) -> Result<(), PoolError> {
        let names: Vec<String> = self.packages_to_load.keys().cloned().collect();
        for name in names {
            if let Some(restricted) = &self.restricted {
                if !restricted.contains(&name) {
                    self.packages_to_load.remove(&name);
                    continue;
                }
            }
            let c = self
                .packages_to_load
                .get(&name)
                .cloned()
                .unwrap_or(Constraint::MatchAll);
            self.loaded_packages.insert(name, c);
        }
        let mut remaining: Vec<(String, Constraint)> = std::mem::take(&mut self.packages_to_load).0;
        for (repo_index, repository) in repositories.iter().enumerate() {
            if matches!(repository, Repository::Platform(_) | Repository::Locked(_)) {
                continue;
            }
            if remaining.is_empty() {
                break;
            }
            let batches: Vec<Vec<(String, Constraint)>> = remaining
                .chunks(LOAD_BATCH_SIZE)
                .map(|c| c.to_vec())
                .collect();
            let mut kept: Vec<Vec<(String, Constraint)>> = Vec::new();
            for batch in batches {
                let empty = BTreeMap::new();
                let already = self.loaded_per_repo.get(&repo_index).unwrap_or(&empty);
                let (names_found, ids) = match repository {
                    Repository::Composer(repo) => repo.load_packages(
                        &batch,
                        &self.set.acceptable_stabilities,
                        &self.set.stability_flags,
                        already,
                        Origin::Repository(repo_index),
                        arena,
                    )?,
                    Repository::Root(members) => array_repository_load_packages(
                        members,
                        &batch,
                        &self.set.acceptable_stabilities,
                        &self.set.stability_flags,
                        already,
                        arena,
                    ),
                    Repository::Platform(_) | Repository::Locked(_) => unreachable!(),
                };
                let mut batch = batch;
                batch.retain(|(n, _)| !names_found.contains(n));
                kept.push(batch);
                for idx in ids {
                    let (name, version, package_type) = {
                        let p = &arena[idx];
                        (p.name.clone(), p.version.clone(), p.package_type.clone())
                    };
                    self.loaded_per_repo
                        .entry(repo_index)
                        .or_default()
                        .entry(name.clone())
                        .or_default()
                        .insert(version);
                    if self.ignored_types.contains(&package_type)
                        || self
                            .allowed_types
                            .as_ref()
                            .is_some_and(|a| !a.contains(&package_type))
                    {
                        continue;
                    }
                    let propagate = !self.path_repo_unlocked.contains(&name);
                    self.load_package(request, idx, propagate, arena);
                }
            }
            remaining = kept.into_iter().flatten().collect();
        }
        Ok(())
    }

    /// `loadPackage`.
    fn load_package(
        &mut self,
        request: &mut Request,
        idx: usize,
        propagate_update: bool,
        arena: &mut Vec<Package>,
    ) {
        let index = self.packages.len();
        self.packages.push(Some(idx));
        if let Some(base) = arena[idx].alias_of {
            self.alias_map.entry(base).or_default().push((index, idx));
        }
        let name = arena[idx].name.clone();
        if let Some(reference) = self.set.root_references.get(&name) {
            if !request.is_locked_package(idx) && !request.is_fixed_package(idx) {
                set_source_dist_references(arena, idx, reference);
            }
        }
        if propagate_update || self.path_repo_unlocked.contains(&name) {
            let version = arena[idx].version.clone();
            if let Some((alias, alias_normalized)) = self
                .set
                .root_aliases
                .get(&name)
                .and_then(|m| m.get(&version))
            {
                let base = arena[idx].alias_of.unwrap_or(idx);
                let mut alias_package = arena[base].alias(base, alias_normalized, alias);
                alias_package.root_package_alias = true;
                alias_package.origin = Origin::Detached;
                arena.push(alias_package);
                let alias_idx = arena.len() - 1;
                let new_index = self.packages.len();
                self.packages.push(Some(alias_idx));
                self.alias_map
                    .entry(base)
                    .or_default()
                    .push((new_index, alias_idx));
            }
        }
        let requires: Vec<(String, Constraint)> = arena[idx]
            .requires
            .iter()
            .map(|l| (l.target.clone(), l.constraint.clone()))
            .collect();
        for (require, link_constraint) in requires {
            if self.skipped_load.contains_key(&require) {
                if propagate_update && request.update_allow_transitive_dependencies() {
                    let skipped_root_requires =
                        self.skipped_root_requires(request, &require, arena);
                    if request.update_allow_transitive_root_dependencies()
                        || skipped_root_requires.is_empty()
                    {
                        self.unlock_package(request, &require, arena);
                        self.mark_package_name_for_loading(request, &require, &link_constraint);
                    } else {
                        self.warn_root_requires(&skipped_root_requires);
                    }
                } else if self.path_repo_unlocked.contains(&require)
                    && !self.loaded_packages.contains_key(&require)
                {
                    self.mark_package_name_for_loading(request, &require, &link_constraint);
                }
            } else {
                self.mark_package_name_for_loading(request, &require, &link_constraint);
            }
        }
        if propagate_update && request.update_allow_transitive_dependencies() {
            let replaces: Vec<String> = arena[idx]
                .replaces
                .iter()
                .map(|l| l.target.clone())
                .collect();
            for replace in replaces {
                if self.loaded_packages.contains_key(&replace)
                    && self.skipped_load.contains_key(&replace)
                {
                    let skipped_root_requires =
                        self.skipped_root_requires(request, &replace, arena);
                    if request.update_allow_transitive_root_dependencies()
                        || skipped_root_requires.is_empty()
                    {
                        self.unlock_package(request, &replace, arena);
                        self.mark_package_name_for_loading_if_required(request, &replace, arena);
                    } else {
                        self.warn_root_requires(&skipped_root_requires);
                    }
                }
            }
        }
    }

    fn warn_root_requires(&mut self, root_requires: &[String]) {
        for root_require in root_requires {
            if self.update_allow_warned.insert(root_require.clone()) {
                self.warnings.push(format!(
                    "Dependency {root_require} is also a root requirement. Package has not been listed as an update argument, so keeping locked at old version. Use --with-all-dependencies (-W) to include root dependencies."
                ));
            }
        }
    }

    fn is_root_require(request: &Request, name: &str) -> bool {
        request.requires.contains(name)
    }

    /// `getSkippedRootRequires`.
    fn skipped_root_requires(
        &self,
        request: &Request,
        name: &str,
        arena: &[Package],
    ) -> Vec<String> {
        let Some(skipped) = self.skipped_load.get(name) else {
            return Vec::new();
        };
        if request.requires.contains(name) {
            return skipped
                .iter()
                .map(|idx| {
                    let p = &arena[*idx];
                    if p.name != name {
                        format!("{} (via replace of {name})", p.name)
                    } else {
                        p.name.clone()
                    }
                })
                .collect();
        }
        let mut matches = Vec::new();
        for idx in skipped {
            let p = &arena[*idx];
            if request.requires.contains(&p.name) {
                matches.push(p.name.clone());
            }
            for link in p.replaces.iter() {
                if request.requires.contains(&link.target) {
                    if p.name != name {
                        matches.push(format!("{} (via replace of {name})", p.name));
                    } else {
                        matches.push(p.name.clone());
                    }
                    break;
                }
            }
        }
        matches
    }

    /// `isUpdateAllowed`.
    fn is_update_allowed(&self, package: &Package) -> bool {
        self.update_allow_patterns
            .iter()
            .any(|re| re.is_match(package.name.as_bytes()).unwrap_or(false))
    }

    /// `warnAboutNonMatchingUpdateAllowList`.
    fn warn_about_non_matching_update_allow_list(
        &mut self,
        request: &Request,
        arena: &[Package],
    ) -> Result<(), PoolError> {
        let Some(locked) = &request.locked_repository else {
            return Err(PoolError(
                "No lock repo present and yet a partial update was requested.".into(),
            ));
        };
        'patterns: for pattern in &self.update_allow_list.clone() {
            let mut matched_platform = false;
            let re = package_name_regexp(pattern);
            for idx in locked {
                if re.is_match(arena[*idx].name.as_bytes()).unwrap_or(false) {
                    continue 'patterns;
                }
            }
            for name in request.requires.keys() {
                if re.is_match(name.as_bytes()).unwrap_or(false) {
                    if is_platform_package(name) {
                        matched_platform = true;
                        continue;
                    }
                    continue 'patterns;
                }
            }
            if matched_platform {
                self.warnings.push(format!(
                    "Pattern \"{pattern}\" listed for update matches platform packages, but these cannot be updated by Composer."
                ));
            } else if pattern.contains('*') {
                self.warnings.push(format!(
                    "Pattern \"{pattern}\" listed for update does not match any locked packages."
                ));
            } else {
                self.warnings.push(format!(
                    "Package \"{pattern}\" listed for update is not locked."
                ));
            }
        }
        Ok(())
    }

    /// `unlockPackage`.
    fn unlock_package(&mut self, request: &mut Request, name: &str, arena: &mut Vec<Package>) {
        let skipped: Vec<usize> = self.skipped_load.get(name).cloned().unwrap_or_default();
        for idx in skipped {
            let replacer_name = arena[idx].name.clone();
            if replacer_name != name
                && self.skipped_load.contains_key(&replacer_name)
                && (request.update_allow_transitive_root_dependencies()
                    || (!Self::is_root_require(request, name)
                        && !Self::is_root_require(request, &replacer_name)))
            {
                self.unlock_package(request, &replacer_name, arena);
                if Self::is_root_require(request, &replacer_name) {
                    self.mark_package_name_for_loading(
                        request,
                        &replacer_name,
                        &Constraint::MatchAll,
                    );
                } else {
                    for loaded in self.loaded_packages_in_pool() {
                        let c = arena[loaded]
                            .requires
                            .get(&replacer_name)
                            .map(|l| l.constraint.clone());
                        if let Some(c) = c {
                            self.mark_package_name_for_loading(request, &replacer_name, &c);
                        }
                    }
                }
            }
        }
        if self.path_repo_unlocked.contains(name) {
            let entries: Vec<(usize, usize)> = self
                .packages
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.map(|idx| (i, idx)))
                .filter(|(_, idx)| arena[*idx].name == name)
                .collect();
            for (index, idx) in entries {
                self.remove_loaded_package(idx, index, arena);
            }
        }
        self.skipped_load.remove(name);
        self.loaded_packages.remove(name);
        self.max_extended_reqs.remove(name);
        self.path_repo_unlocked.remove(name);
        for locked_idx in request.locked_packages_all() {
            if !arena[locked_idx].is_alias() && arena[locked_idx].name == name {
                if let Some(index) = self.packages.iter().position(|p| *p == Some(locked_idx)) {
                    request.unlock_package(locked_idx);
                    self.remove_loaded_package(locked_idx, index, arena);
                    for fixed_or_locked in request.fixed_or_locked_packages() {
                        if fixed_or_locked == locked_idx {
                            continue;
                        }
                        if self.skipped_load.contains_key(&arena[fixed_or_locked].name) {
                            let locked_name = arena[locked_idx].name.clone();
                            let requires = arena[fixed_or_locked].requires.clone();
                            if let Some(link) = requires.get(&locked_name) {
                                let c = link.constraint.clone();
                                self.mark_package_name_for_loading(request, &locked_name, &c);
                            }
                            let replaces = arena[locked_idx].replaces.clone();
                            for replace in replaces.iter() {
                                if requires.get(&replace.target).is_some()
                                    && self.skipped_load.contains_key(&replace.target)
                                {
                                    self.unlock_package(request, &replace.target, arena);
                                    self.mark_package_name_for_loading(
                                        request,
                                        &replace.target,
                                        &replace.constraint,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// `markPackageNameForLoadingIfRequired`.
    fn mark_package_name_for_loading_if_required(
        &mut self,
        request: &Request,
        name: &str,
        arena: &[Package],
    ) {
        if let Some(c) = request.requires.get(name).cloned() {
            self.mark_package_name_for_loading(request, name, &c);
        }
        for loaded in self.loaded_packages_in_pool() {
            let links: Vec<Constraint> = arena[loaded]
                .requires
                .iter()
                .filter(|l| l.target == name)
                .map(|l| l.constraint.clone())
                .collect();
            for c in links {
                self.mark_package_name_for_loading(request, name, &c);
            }
        }
    }

    /// `removeLoadedPackage`.
    fn remove_loaded_package(&mut self, idx: usize, index: usize, arena: &[Package]) {
        let repo_index = match arena[idx].origin {
            Origin::Repository(i) => Some(i),
            _ => None,
        };
        let mut forget = |p: &Package| {
            if let Some(r) = repo_index {
                if let Some(by_name) = self
                    .loaded_per_repo
                    .get_mut(&r)
                    .and_then(|m| m.get_mut(&p.name))
                {
                    by_name.remove(&p.version);
                }
            }
        };
        forget(&arena[idx]);
        self.packages[index] = None;
        if let Some(aliases) = self.alias_map.remove(&idx) {
            for (alias_index, alias_idx) in aliases {
                forget(&arena[alias_idx]);
                self.packages[alias_index] = None;
            }
        }
    }
}

/// `ArrayRepository::loadPackages` (dépôt racine).
pub fn array_repository_load_packages(
    members: &[usize],
    package_name_map: &[(String, Constraint)],
    acceptable: &BTreeMap<String, i32>,
    flags: &BTreeMap<String, i32>,
    already_loaded: &BTreeMap<String, BTreeSet<String>>,
    arena: &[Package],
) -> (Vec<String>, Vec<usize>) {
    let mut result: Vec<usize> = Vec::new();
    let mut names_found: Vec<String> = Vec::new();
    for idx in members {
        let p = &arena[*idx];
        let Some((_, constraint)) = package_name_map.iter().find(|(n, _)| *n == p.name) else {
            continue;
        };
        let matches =
            matches!(constraint, Constraint::MatchAll) || constraint.matches_version(&p.version);
        if matches
            && is_package_acceptable(acceptable, flags, &p.names(true), p.stability)
            && !already_loaded
                .get(&p.name)
                .is_some_and(|s| s.contains(&p.version))
        {
            push_unique(&mut result, *idx);
            if let Some(base) = p.alias_of {
                push_unique(&mut result, base);
            }
        }
        if !names_found.contains(&p.name) {
            names_found.push(p.name.clone());
        }
    }
    for idx in members {
        if let Some(base) = arena[*idx].alias_of {
            if result.contains(&base) {
                push_unique(&mut result, *idx);
            }
        }
    }
    (names_found, result)
}

/// `Package::setSourceDistReferences`, appliqué au paquet de base et à ses
/// alias (un `AliasPackage` délègue ses références au paquet aliasé).
pub fn set_source_dist_references(arena: &mut [Package], idx: usize, reference: &str) {
    static HOSTS: OnceLock<Regex> = OnceLock::new();
    static SHA: OnceLock<Regex> = OnceLock::new();
    let base = arena[idx].alias_of.unwrap_or(idx);
    let targets: Vec<usize> = (0..arena.len())
        .filter(|i| *i == base || arena[*i].alias_of == Some(base))
        .collect();
    for t in targets {
        let p = &mut arena[t];
        if let Some(s) = &mut p.source {
            s.reference = Some(reference.to_owned());
        }
        let dist_url = p.dist.as_ref().map(|d| d.url.clone());
        let hosts = regex(
            &HOSTS,
            r"^https?://(?:(?:www\.)?bitbucket\.org|(api\.)?github\.com|(?:www\.)?gitlab\.com)/",
            true,
        );
        match dist_url {
            Some(url) if hosts.is_match(url.as_bytes()).unwrap_or(false) => {
                let sha = regex(&SHA, r"(?<=/|sha=)[a-f0-9]{40}(?=/|$)", true);
                let replaced = replace_all(sha, &url, reference);
                if let Some(d) = &mut p.dist {
                    d.reference = Some(reference.to_owned());
                    d.url = replaced;
                }
            }
            _ => {
                if let Some(d) = &mut p.dist {
                    if d.reference
                        .as_deref()
                        .is_some_and(|r| !r.is_empty() && r != "0")
                    {
                        d.reference = Some(reference.to_owned());
                    }
                }
            }
        }
    }
}

fn replace_all(re: &Regex, subject: &str, replacement: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for m in re.find_iter(subject.as_bytes()).flatten() {
        out.push_str(&subject[last..m.start()]);
        out.push_str(replacement);
        last = m.end();
    }
    out.push_str(&subject[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{Link, LinkType};
    use serde_json::json;

    #[test]
    fn match_looks_up_links_by_php_key() {
        // lib-libxslt replace : clé `xsl`, cible `lib-xsl` → introuvable par
        // `lib-xsl` (isset($replaces['lib-xsl']) est faux chez Composer).
        let mut lib = Package::new("lib-libxslt", "1.1.35.0", "1.1.35", Origin::Platform);
        lib.replaces.insert(Link {
            key: Some("xsl".into()),
            source: "lib-libxslt".into(),
            target: "lib-xsl".into(),
            constraint: Constraint::new(crate::constraint::Op::Eq, "1.1.35.0"),
            pretty_constraint: "1.1.35".into(),
            kind: LinkType::Replace,
        });
        assert!(!Pool::matches(&lib, "lib-xsl", None));
        // Par la clé, `match` répond oui — mais `packageByName` ne connaît
        // que les cibles, donc `whatProvides('xsl')` reste vide.
        assert!(Pool::matches(&lib, "xsl", None));
        let pool = Pool::new(vec![0], Vec::new(), std::slice::from_ref(&lib));
        assert_eq!(
            pool.what_provides(std::slice::from_ref(&lib), "xsl", None),
            Vec::<usize>::new()
        );
        assert_eq!(
            pool.what_provides(std::slice::from_ref(&lib), "lib-xsl", None),
            Vec::<usize>::new()
        );
        // Avec des clés numériques (alias self.version), parcours par cible.
        let cfg = json!({"name": "acme/lib", "version": "dev-main", "default-branch": true,
            "replace": {"acme/old": "self.version"}});
        let mut arena = Vec::new();
        let ids =
            crate::loader::load_packages(&[cfg], Origin::Repository(0), &mut arena, false).unwrap();
        let alias = &arena[ids[0]];
        assert!(alias.replaces.has_numeric_keys());
        assert!(Pool::matches(
            alias,
            "acme/old",
            Some(&Constraint::new(crate::constraint::Op::Eq, "9999999-dev"))
        ));
        assert!(Pool::matches(
            alias,
            "acme/old",
            Some(&Constraint::new(crate::constraint::Op::Eq, "dev-main"))
        ));
        let base = &arena[ids[1]];
        assert!(!base.replaces.has_numeric_keys());
        assert!(!Pool::matches(
            base,
            "acme/old",
            Some(&Constraint::new(crate::constraint::Op::Eq, "9999999-dev"))
        ));
        let pool = Pool::new(vec![ids[1], ids[0]], Vec::new(), &arena);
        assert_eq!(pool.what_provides(&arena, "acme/old", None), vec![1, 2]);
        assert_eq!(
            pool.what_provides(&arena, "lib-xsl", None),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn transitive_dependencies_allowed_without_allow_list() {
        let r = Request::new(None);
        assert!(r.update_allow_transitive_dependencies());
        assert!(!r.update_allow_transitive_root_dependencies());
    }
}
