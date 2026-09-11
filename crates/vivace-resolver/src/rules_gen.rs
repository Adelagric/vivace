//! Port de `Composer\DependencyResolver\RuleSetGenerator`.

use crate::constraint::Constraint;
use crate::package::Package;
use crate::platform_filter::PlatformRequirementFilter;
use crate::pool::{Pool, Request};
use crate::rule::{Reason, Rule, RuleSet, RuleType};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RulesError(pub String);

pub struct RuleSetGenerator<'a> {
    pool: &'a Pool,
    arena: &'a [Package],
    rules: RuleSet,
    /// `addedMap` : identifiants de pool déjà traités (ordre d'ajout).
    added: HashSet<usize>,
    added_order: Vec<usize>,
    /// `addedPackagesByNames` : name → identifiants de pool (ordre).
    added_by_name: Vec<(String, Vec<usize>)>,
    added_by_name_index: HashMap<String, usize>,
}

impl<'a> RuleSetGenerator<'a> {
    pub fn new(pool: &'a Pool, arena: &'a [Package]) -> RuleSetGenerator<'a> {
        RuleSetGenerator {
            pool,
            arena,
            rules: RuleSet::new(),
            added: HashSet::new(),
            added_order: Vec::new(),
            added_by_name: Vec::new(),
            added_by_name_index: HashMap::new(),
        }
    }

    fn package(&self, id: usize) -> &'a Package {
        &self.arena[self.pool.package_by_id(id)]
    }

    /// `createRequireRule` : None si le paquet est parmi ses fournisseurs.
    fn create_require_rule(&self, id: usize, providers: &[usize], reason: Reason) -> Option<Rule> {
        let mut literals = vec![-(id as i64)];
        for &p in providers {
            if p == id {
                return None;
            }
            literals.push(p as i64);
        }
        Some(Rule::generic(literals, reason))
    }

    fn add_rule(&mut self, rule_type: RuleType, rule: Option<Rule>) {
        if let Some(r) = rule {
            let _ = self.rules.add(r, rule_type);
        }
    }

    fn add_by_name(&mut self, name: &str, id: usize) {
        match self.added_by_name_index.get(name) {
            Some(&i) => self.added_by_name[i].1.push(id),
            None => {
                self.added_by_name_index
                    .insert(name.to_owned(), self.added_by_name.len());
                self.added_by_name.push((name.to_owned(), vec![id]));
            }
        }
    }

    /// `addRulesForPackage`.
    fn add_rules_for_package(&mut self, id: usize, filter: &PlatformRequirementFilter) {
        let mut queue: VecDeque<usize> = VecDeque::new();
        queue.push_back(id);
        while let Some(id) = queue.pop_front() {
            if !self.added.insert(id) {
                continue;
            }
            self.added_order.push(id);
            let package = self.package(id);
            if let Some(base_idx) = package.alias_of {
                let base = self
                    .pool
                    .id_of(base_idx)
                    .expect("aliased package is in the pool");
                queue.push_back(base);
                let alias_arena = self.pool.package_by_id(id);
                let r1 = self.create_require_rule(
                    id,
                    &[base],
                    Reason::PackageAlias { alias: alias_arena },
                );
                self.add_rule(RuleType::Package, r1);
                let r2 = self.create_require_rule(
                    base,
                    &[id],
                    Reason::PackageInverseAlias { package: base_idx },
                );
                self.add_rule(RuleType::Package, r2);
                if !package.has_self_version_requires {
                    continue;
                }
            } else {
                for name in package.names(false) {
                    self.add_by_name(&name, id);
                }
            }
            for link in package.requires.iter() {
                if filter.is_ignored(&link.target) {
                    continue;
                }
                let constraint = filter.filter_constraint(&link.target, &link.constraint, true);
                let possible = self
                    .pool
                    .what_provides(self.arena, &link.target, Some(&constraint));
                let rule =
                    self.create_require_rule(id, &possible, Reason::PackageRequires(link.clone()));
                self.add_rule(RuleType::Package, rule);
                for p in possible {
                    queue.push_back(p);
                }
            }
        }
    }

    /// `addConflictRules`.
    fn add_conflict_rules(&mut self, filter: &PlatformRequirementFilter) {
        for &id in &self.added_order.clone() {
            let package = self.package(id);
            for link in package.conflicts.iter() {
                if !self.added_by_name_index.contains_key(&link.target) {
                    continue;
                }
                if filter.is_ignored(&link.target) {
                    continue;
                }
                let constraint = filter.filter_constraint(&link.target, &link.constraint, false);
                let conflicts =
                    self.pool
                        .what_provides(self.arena, &link.target, Some(&constraint));
                for conflict in conflicts {
                    let cp = self.package(conflict);
                    if !cp.is_alias() || cp.name == link.target {
                        if conflict == id {
                            continue;
                        }
                        let rule = Rule::two_literals(
                            -(id as i64),
                            -(conflict as i64),
                            Reason::PackageConflict(link.clone()),
                        );
                        let _ = self.rules.add(rule, RuleType::Package);
                    }
                }
            }
        }
        for (name, ids) in self.added_by_name.clone() {
            if ids.len() > 1 {
                let literals: Vec<i64> = ids.iter().map(|&i| -(i as i64)).collect();
                let reason = Reason::PackageSameName(name.clone());
                let rule = if literals.len() == 2 {
                    Rule::two_literals(literals[0], literals[1], reason)
                } else {
                    Rule::multi_conflict(literals, reason)
                };
                let _ = self.rules.add(rule, RuleType::Package);
            }
        }
    }

    /// `addRulesForRequest`.
    fn add_rules_for_request(
        &mut self,
        request: &Request,
        filter: &PlatformRequirementFilter,
    ) -> Result<(), RulesError> {
        for &fixed in &request.fixed_packages {
            let Some(id) = self.pool.id_of(fixed) else {
                if self.pool.is_unacceptable_fixed_or_locked(fixed) {
                    continue;
                }
                return Err(RulesError(format!(
                    "Fixed package {} was not added to solver pool.",
                    self.arena[fixed].pretty_string()
                )));
            };
            self.add_rules_for_package(id, filter);
            let rule = Rule::generic(vec![id as i64], Reason::Fixed { package: fixed });
            let _ = self.rules.add(rule, RuleType::Request);
        }
        for (name, constraint) in request.requires.iter() {
            if filter.is_ignored(name) {
                continue;
            }
            let constraint: Constraint = filter.filter_constraint(name, constraint, true);
            let packages = self.pool.what_provides(self.arena, name, Some(&constraint));
            if !packages.is_empty() {
                for &p in &packages {
                    self.add_rules_for_package(p, filter);
                }
                let rule = Rule::generic(
                    packages.iter().map(|&p| p as i64).collect(),
                    Reason::RootRequire {
                        package_name: name.clone(),
                        constraint,
                    },
                );
                let _ = self.rules.add(rule, RuleType::Request);
            }
        }
        Ok(())
    }

    /// `addRulesForRootAliases`.
    fn add_rules_for_root_aliases(&mut self, filter: &PlatformRequirementFilter) {
        for id in 1..=self.pool.len() {
            if self.added.contains(&id) {
                continue;
            }
            let package = self.package(id);
            let Some(base_idx) = package.alias_of else {
                continue;
            };
            let base_added = self
                .pool
                .id_of(base_idx)
                .is_some_and(|b| self.added.contains(&b));
            if package.root_package_alias || base_added {
                self.add_rules_for_package(id, filter);
            }
        }
    }

    /// `getRulesFor`.
    pub fn rules_for(
        mut self,
        request: &Request,
        filter: &PlatformRequirementFilter,
    ) -> Result<RuleSet, RulesError> {
        self.add_rules_for_request(request, filter)?;
        self.add_rules_for_root_aliases(filter);
        self.add_conflict_rules(filter);
        Ok(self.rules)
    }
}
