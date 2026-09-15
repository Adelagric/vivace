//! Port of `Transaction` and `LockTransaction`: the packages kept by the
//! decisions, the operations relative to the present lock, the packages to
//! write into the lock.

use crate::decisions::Decisions;
use crate::package::Package;
use crate::platform::is_platform_package;
use crate::pool::{Pool, Request};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Install(usize),
    /// (initial, target).
    Update(usize, usize),
    Uninstall(usize),
    MarkAliasInstalled(usize),
    MarkAliasUninstalled(usize),
}

/// `CompletePackage::isAbandoned` / `getReplacementPackage`.
fn abandoned(p: &Package) -> (bool, Option<String>) {
    match p.raw.get("abandoned") {
        None | Some(Value::Null) => (false, None),
        // `(bool) "0"` is false, but `getReplacementPackage()` returns "0".
        Some(Value::String(s)) => (!s.is_empty() && s != "0", Some(s.clone())),
        Some(Value::Bool(b)) => (*b, None),
        Some(Value::Number(n)) => (n.as_f64() != Some(0.0), None),
        Some(Value::Array(a)) => (!a.is_empty(), None),
        Some(Value::Object(o)) => (!o.is_empty(), None),
    }
}

/// `Transaction::$packageSort`.
fn package_sort(a: &Package, b: &Package) -> Ordering {
    if a.name == b.name {
        if a.is_alias() != b.is_alias() {
            return if a.is_alias() {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        return b.version.as_bytes().cmp(a.version.as_bytes());
    }
    b.name.as_bytes().cmp(a.name.as_bytes())
}

pub struct Transaction {
    pub operations: Vec<Operation>,
}

impl Transaction {
    /// `__construct($presentPackages, $resultPackages)` (arena indices).
    pub fn new(arena: &[Package], present: &[usize], result: &[usize]) -> Transaction {
        // setResultPackageMaps
        let mut result_map: Vec<usize> = result.to_vec();
        result_map.sort_by(|&a, &b| package_sort(&arena[a], &arena[b]));
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for &idx in result {
            for name in arena[idx].names(true) {
                by_name.entry(name).or_default().push(idx);
            }
        }
        for list in by_name.values_mut() {
            list.sort_by(|&a, &b| package_sort(&arena[a], &arena[b]));
        }
        let providers =
            |target: &str| -> Vec<usize> { by_name.get(target).cloned().unwrap_or_default() };

        // calculateOperations
        let mut operations: Vec<Operation> = Vec::new();
        let mut present_package_map: HashMap<String, usize> = HashMap::new();
        let mut remove_map: Vec<(String, usize)> = Vec::new();
        let mut present_alias_map: HashSet<String> = HashSet::new();
        let mut remove_alias_map: Vec<(String, usize)> = Vec::new();
        for &idx in present {
            let p = &arena[idx];
            if p.is_alias() {
                let key = format!("{}::{}", p.name, p.version);
                present_alias_map.insert(key.clone());
                if let Some(slot) = remove_alias_map.iter_mut().find(|(k, _)| *k == key) {
                    slot.1 = idx;
                } else {
                    remove_alias_map.push((key, idx));
                }
            } else {
                present_package_map.insert(p.name.clone(), idx);
                if let Some(slot) = remove_map.iter_mut().find(|(k, _)| *k == p.name) {
                    slot.1 = idx;
                } else {
                    remove_map.push((p.name.clone(), idx));
                }
            }
        }

        // getRootPackages
        let mut roots: Vec<usize> = result_map.clone();
        for &idx in &result_map {
            if !roots.contains(&idx) {
                continue;
            }
            for link in arena[idx].requires.iter() {
                for require in providers(&link.target) {
                    if require != idx {
                        roots.retain(|&r| r != require);
                    }
                }
            }
        }

        let mut stack: Vec<usize> = roots;
        let mut visited: HashSet<usize> = HashSet::new();
        let mut processed: HashSet<usize> = HashSet::new();
        while let Some(idx) = stack.pop() {
            if processed.contains(&idx) {
                continue;
            }
            if !visited.contains(&idx) {
                visited.insert(idx);
                stack.push(idx);
                let p = &arena[idx];
                if let Some(base) = p.alias_of {
                    stack.push(base);
                } else {
                    for link in p.requires.iter() {
                        for require in providers(&link.target) {
                            stack.push(require);
                        }
                    }
                }
            } else {
                processed.insert(idx);
                let p = &arena[idx];
                if p.is_alias() {
                    let key = format!("{}::{}", p.name, p.version);
                    if present_alias_map.contains(&key) {
                        remove_alias_map.retain(|(k, _)| *k != key);
                    } else {
                        operations.push(Operation::MarkAliasInstalled(idx));
                    }
                } else if let Some(&source) = present_package_map.get(&p.name) {
                    let s = &arena[source];
                    let (pa, pr) = abandoned(p);
                    let (sa, sr) = abandoned(s);
                    if p.version != s.version
                        || p.dist_reference() != s.dist_reference()
                        || p.source_reference() != s.source_reference()
                        || pa != sa
                        || pr != sr
                    {
                        operations.push(Operation::Update(source, idx));
                    }
                    remove_map.retain(|(k, _)| *k != p.name);
                } else {
                    operations.push(Operation::Install(idx));
                    remove_map.retain(|(k, _)| *k != p.name);
                }
            }
        }
        for (_, idx) in &remove_map {
            operations.insert(0, Operation::Uninstall(*idx));
        }
        for (_, idx) in &remove_alias_map {
            operations.push(Operation::MarkAliasUninstalled(*idx));
        }
        let operations = Self::move_plugins_to_front(arena, operations);
        let operations = Self::move_uninstalls_to_front(operations);
        Transaction { operations }
    }

    fn op_package(op: &Operation) -> Option<usize> {
        match op {
            Operation::Install(p) => Some(*p),
            Operation::Update(_, t) => Some(*t),
            _ => None,
        }
    }

    /// `movePluginsToFront`.
    fn move_plugins_to_front(arena: &[Package], mut operations: Vec<Operation>) -> Vec<Operation> {
        let mut dl_no_deps: Vec<Operation> = Vec::new();
        let mut dl_with_deps: Vec<Operation> = Vec::new();
        let mut dl_requires: Vec<String> = Vec::new();
        let mut plugins_no_deps: Vec<Operation> = Vec::new();
        let mut plugins_with_deps: Vec<Operation> = Vec::new();
        let mut plugin_requires: Vec<String> = Vec::new();
        let mut removed: Vec<usize> = Vec::new();
        for idx in (0..operations.len()).rev() {
            let op = &operations[idx];
            let Some(pkg) = Self::op_package(op) else {
                continue;
            };
            let package = &arena[pkg];
            let is_dl_plugin = package.package_type == "composer-plugin"
                && package
                    .raw
                    .get("extra")
                    .and_then(|e| e.get("plugin-modifies-downloads"))
                    == Some(&Value::Bool(true));
            let names = package.names(true);
            let non_platform_requires = || -> Vec<String> {
                package
                    .requires
                    .iter()
                    .filter_map(|l| l.key.clone())
                    .filter(|req| !is_platform_package(req))
                    .collect()
            };
            if is_dl_plugin || names.iter().any(|n| dl_requires.contains(n)) {
                let requires = non_platform_requires();
                if is_dl_plugin && requires.is_empty() {
                    dl_no_deps.insert(0, op.clone());
                } else {
                    dl_requires.extend(requires);
                    dl_with_deps.insert(0, op.clone());
                }
                removed.push(idx);
                continue;
            }
            let is_plugin = package.package_type == "composer-plugin"
                || package.package_type == "composer-installer";
            if is_plugin || names.iter().any(|n| plugin_requires.contains(n)) {
                let requires = non_platform_requires();
                if is_plugin && requires.is_empty() {
                    plugins_no_deps.insert(0, op.clone());
                } else {
                    plugin_requires.extend(requires);
                    plugins_with_deps.insert(0, op.clone());
                }
                removed.push(idx);
            }
        }
        removed.sort_unstable_by(|a, b| b.cmp(a));
        for idx in removed {
            operations.remove(idx);
        }
        let mut out = dl_no_deps;
        out.extend(dl_with_deps);
        out.extend(plugins_no_deps);
        out.extend(plugins_with_deps);
        out.extend(operations);
        out
    }

    /// `moveUninstallsToFront`.
    fn move_uninstalls_to_front(operations: Vec<Operation>) -> Vec<Operation> {
        let (uninst, rest): (Vec<Operation>, Vec<Operation>) =
            operations.into_iter().partition(|op| {
                matches!(
                    op,
                    Operation::Uninstall(_) | Operation::MarkAliasUninstalled(_)
                )
            });
        let mut out = uninst;
        out.extend(rest);
        out
    }
}

/// `Composer\DependencyResolver\LockTransaction`.
pub struct LockTransaction {
    pub transaction: Transaction,
    /// `resultPackages['all'|'non-dev'|'dev']` (arena indices).
    pub all: Vec<usize>,
    pub non_dev: Vec<usize>,
    pub dev: Vec<usize>,
    /// `presentMap` (arena indices, lock order then fixed packages).
    pub present: Vec<usize>,
}

impl LockTransaction {
    pub fn empty() -> LockTransaction {
        LockTransaction {
            transaction: Transaction {
                operations: Vec::new(),
            },
            all: Vec::new(),
            non_dev: Vec::new(),
            dev: Vec::new(),
            present: Vec::new(),
        }
    }

    pub fn new(
        pool: &Pool,
        arena: &[Package],
        request: &Request,
        decisions: &Decisions,
    ) -> LockTransaction {
        // getPresentMap: lock then fixed packages (no duplicates).
        let mut present: Vec<usize> = Vec::new();
        for idx in request
            .locked_repository
            .iter()
            .flatten()
            .copied()
            .chain(request.fixed_packages.iter().copied())
        {
            if !present.contains(&idx) {
                present.push(idx);
            }
        }
        let unlockable: HashSet<usize> = request.fixed_packages.iter().copied().collect();
        // setResultPackages: `foreach ($decisions ...)` from last to first.
        let mut all = Vec::new();
        let mut non_dev = Vec::new();
        for i in (0..decisions.len()).rev() {
            let literal = decisions.at_offset(i).literal;
            if literal > 0 {
                let idx = pool.literal_to_package(literal);
                all.push(idx);
                if !unlockable.contains(&idx) {
                    non_dev.push(idx);
                }
            }
        }
        let transaction = Transaction::new(arena, &present, &all);
        LockTransaction {
            transaction,
            all,
            non_dev,
            dev: Vec::new(),
            present,
        }
    }

    /// `setNonDevPackages($extractionResult)`.
    pub fn set_non_dev_packages(&mut self, arena: &[Package], extraction: &LockTransaction) {
        let packages = extraction.new_lock_packages(arena, false);
        self.dev = std::mem::take(&mut self.non_dev);
        self.non_dev = Vec::new();
        for pkg in packages {
            let name = &arena[pkg].name;
            let mut i = 0;
            while i < self.dev.len() {
                if arena[self.dev[i]].name == *name {
                    let moved = self.dev.remove(i);
                    self.non_dev.push(moved);
                } else {
                    i += 1;
                }
            }
        }
    }

    /// `getNewLockPackages($devMode)` without `updateMirrors`.
    pub fn new_lock_packages(&self, arena: &[Package], dev_mode: bool) -> Vec<usize> {
        let source = if dev_mode { &self.dev } else { &self.non_dev };
        source
            .iter()
            .copied()
            .filter(|&idx| !arena[idx].is_alias())
            .collect()
    }

    /// `getAliases($aliases)`: the root aliases in use, sorted by name.
    pub fn aliases(
        &self,
        arena: &[Package],
        aliases: &[crate::root::RootAlias],
    ) -> Vec<crate::root::RootAlias> {
        let mut remaining: Vec<Option<&crate::root::RootAlias>> =
            aliases.iter().map(Some).collect();
        let mut used = Vec::new();
        for &idx in &self.all {
            if !arena[idx].is_alias() {
                continue;
            }
            for slot in remaining.iter_mut() {
                if let Some(a) = slot {
                    if a.package == arena[idx].name {
                        used.push((*a).clone());
                        *slot = None;
                    }
                }
            }
        }
        used.sort_by(|a, b| a.package.as_bytes().cmp(b.package.as_bytes()));
        used
    }
}

/// `PackageInterface::DISPLAY_*` of `getFullPrettyVersion`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DisplayRef {
    SourceRefIfDev,
    SourceRef,
    DistRef,
}

/// `BasePackage::getFullPrettyVersion($truncate, $displayMode)`.
pub fn full_pretty_version(p: &Package, truncate: bool, mode: DisplayRef) -> String {
    let source_type = p.source.as_ref().map(|s| s.kind.as_str()).unwrap_or("");
    let dist_ref = p
        .dist
        .as_ref()
        .and_then(|d| d.reference.as_deref())
        .unwrap_or("");
    if mode == DisplayRef::SourceRefIfDev
        && (!p.is_dev()
            || (!matches!(source_type, "hg" | "git")
                && (!source_type.is_empty() || dist_ref.is_empty())))
    {
        return p.pretty_version.clone();
    }
    let source_ref = p.source.as_ref().and_then(|s| s.reference.as_deref());
    let reference: Option<&str> = match mode {
        DisplayRef::SourceRefIfDev => match source_ref {
            Some(r) if !r.is_empty() => Some(r),
            _ => p.dist.as_ref().and_then(|d| d.reference.as_deref()),
        },
        DisplayRef::SourceRef => source_ref,
        DisplayRef::DistRef => p.dist.as_ref().and_then(|d| d.reference.as_deref()),
    };
    let Some(reference) = reference else {
        return p.pretty_version.clone();
    };
    if truncate && reference.len() == 40 && source_type != "svn" {
        let short = String::from_utf8_lossy(&reference.as_bytes()[..7]);
        return format!("{} {}", p.pretty_version, short);
    }
    format!("{} {}", p.pretty_version, reference)
}

/// `VersionParser::isUpgrade`.
pub fn is_upgrade(from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let norm = |v: &str| -> String {
        if matches!(v, "dev-master" | "dev-trunk" | "dev-default") {
            "9999999-dev".to_owned()
        } else {
            v.to_owned()
        }
    };
    let from = norm(from);
    let to = norm(to);
    if from.starts_with("dev-") || to.starts_with("dev-") {
        return true;
    }
    // `Semver::sort([$to, $from])[0] === $from`: from sorts first unless
    // it is strictly greater (a stable sort keeps `to` first on a tie).
    !crate::phpver::version_compare_op(&to, &from, "<")
}

impl Operation {
    /// `OperationInterface::show($lock)` without the output styles;
    /// `None` for the alias operations (only shown in debug mode).
    pub fn show(&self, arena: &[Package], lock: bool) -> Option<String> {
        let full = |idx: usize| full_pretty_version(&arena[idx], true, DisplayRef::SourceRefIfDev);
        match *self {
            Operation::Install(p) => Some(format!(
                "{} {} ({})",
                if lock { "Locking" } else { "Installing" },
                arena[p].pretty_name,
                full(p)
            )),
            Operation::Uninstall(p) => {
                Some(format!("Removing {} ({})", arena[p].pretty_name, full(p)))
            }
            Operation::Update(initial, target) => {
                // `UpdateOperation::format`.
                let (i, t) = (&arena[initial], &arena[target]);
                let mut from = full(initial);
                let mut to = full(target);
                let source_ref = |p: &Package| p.source.as_ref().and_then(|s| s.reference.clone());
                let dist_ref = |p: &Package| p.dist.as_ref().and_then(|d| d.reference.clone());
                if from == to && source_ref(i) != source_ref(t) {
                    from = full_pretty_version(i, true, DisplayRef::SourceRef);
                    to = full_pretty_version(t, true, DisplayRef::SourceRef);
                } else if from == to && dist_ref(i) != dist_ref(t) {
                    from = full_pretty_version(i, true, DisplayRef::DistRef);
                    to = full_pretty_version(t, true, DisplayRef::DistRef);
                }
                let action = if is_upgrade(&i.version, &t.version) {
                    "Upgrading"
                } else {
                    "Downgrading"
                };
                Some(format!("{action} {} ({from} => {to})", i.pretty_name))
            }
            Operation::MarkAliasInstalled(_) | Operation::MarkAliasUninstalled(_) => None,
        }
    }

    /// The package the `usort` of `Installer::doUpdate` sorts on.
    pub fn sort_package(&self) -> usize {
        match *self {
            Operation::Install(p)
            | Operation::Uninstall(p)
            | Operation::MarkAliasInstalled(p)
            | Operation::MarkAliasUninstalled(p) => p,
            Operation::Update(_, t) => t,
        }
    }
}

/// The `  - …` lines of `Installer::doUpdate` (`show(true)`): removals
/// first, then installs and updates, each group sorted by name
/// (`strcmp`), alias operations left out.
pub fn lock_operation_lines(arena: &[Package], operations: &[Operation]) -> Vec<String> {
    let mut uninstalls: Vec<&Operation> = Vec::new();
    let mut installs_updates: Vec<&Operation> = Vec::new();
    for op in operations {
        match op {
            Operation::Uninstall(_) => uninstalls.push(op),
            Operation::Install(_) | Operation::Update(..) => installs_updates.push(op),
            Operation::MarkAliasInstalled(_) | Operation::MarkAliasUninstalled(_) => {}
        }
    }
    let by_name = |a: &&Operation, b: &&Operation| {
        arena[a.sort_package()]
            .name
            .as_bytes()
            .cmp(arena[b.sort_package()].name.as_bytes())
    };
    uninstalls.sort_by(by_name);
    installs_updates.sort_by(by_name);
    uninstalls
        .into_iter()
        .chain(installs_updates)
        .filter_map(|op| op.show(arena, true).map(|s| format!("  - {s}")))
        .collect()
}
