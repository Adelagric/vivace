//! Port de `Composer\DependencyResolver\Solver` (CDCL) et du squelette de
//! `Problem` (les règles fautives ; les messages arrivent avec R5).

use crate::decisions::{Decisions, SolverBug};
use crate::package::Package;
use crate::platform_filter::PlatformRequirementFilter;
use crate::policy::DefaultPolicy;
use crate::pool::{Pool, Request};
use crate::rule::{Reason, Rule, RuleKind, RuleSet, RuleType};
use crate::rules_gen::RuleSetGenerator;
use crate::transaction::LockTransaction;
use crate::watch::{RuleWatchGraph, RuleWatchNode};
use std::collections::{HashMap, HashSet};

/// `Composer\DependencyResolver\Problem` : sections de règles.
#[derive(Debug, Clone, Default)]
pub struct Problem {
    /// section → règles (identifiants, ou règles hors jeu pour les
    /// requêtes racine insatisfaisables). Une section n'existe qu'à partir
    /// de sa première règle (`nextSection` ne fait qu'avancer l'index).
    pub sections: Vec<Vec<ProblemRule>>,
    seen: HashSet<usize>,
    pending_section: bool,
}

#[derive(Debug, Clone)]
pub enum ProblemRule {
    InSet(usize),
    Detached(Rule),
}

impl Problem {
    pub fn new() -> Problem {
        Problem {
            sections: Vec::new(),
            seen: HashSet::new(),
            pending_section: true,
        }
    }

    fn push(&mut self, rule: ProblemRule) {
        if self.pending_section || self.sections.is_empty() {
            self.sections.push(Vec::new());
            self.pending_section = false;
        }
        let last = self.sections.len() - 1;
        self.sections[last].push(rule);
    }

    pub fn add_rule(&mut self, rule: usize) {
        if !self.seen.insert(rule) {
            return;
        }
        self.push(ProblemRule::InSet(rule));
    }

    pub fn add_detached(&mut self, rule: Rule) {
        self.push(ProblemRule::Detached(rule));
    }

    pub fn next_section(&mut self) {
        self.pending_section = true;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SolveError {
    #[error("{0}")]
    Bug(String),
    #[error("Your requirements could not be resolved to an installable set of packages.")]
    Problems(Vec<Problem>),
}

impl From<SolverBug> for SolveError {
    fn from(e: SolverBug) -> SolveError {
        SolveError::Bug(e.0)
    }
}

pub struct Solver<'a> {
    pool: &'a Pool,
    arena: &'a [Package],
    pub rules: RuleSet,
    watch_graph: RuleWatchGraph,
    pub decisions: Decisions,
    fixed_map: HashSet<usize>,
    propagate_index: usize,
    /// `(literals, level)`.
    branches: Vec<(Vec<i64>, i64)>,
    pub problems: Vec<Problem>,
    /// `learnedPool[why]` : règles ayant mené à une règle apprise.
    learned_pool: Vec<Vec<usize>>,
    /// règle apprise → `why`.
    learned_why: HashMap<usize, usize>,
}

impl<'a> Solver<'a> {
    pub fn new(pool: &'a Pool, arena: &'a [Package]) -> Solver<'a> {
        Solver {
            pool,
            arena,
            rules: RuleSet::new(),
            watch_graph: RuleWatchGraph::new(),
            decisions: Decisions::new(pool.len()),
            fixed_map: HashSet::new(),
            propagate_index: 0,
            branches: Vec::new(),
            problems: Vec::new(),
            learned_pool: Vec::new(),
            learned_why: HashMap::new(),
        }
    }

    pub fn rule_set_size(&self) -> usize {
        self.rules.len()
    }

    /// `makeAssertionRuleDecisions`.
    fn make_assertion_rule_decisions(&mut self) -> Result<(), SolveError> {
        let decision_start = self.decisions.len() as i64 - 1;
        let rules_count = self.rules.len();
        let mut rule_index: i64 = 0;
        while (rule_index as usize) < rules_count {
            let id = self.rules.rule_by_id[rule_index as usize];
            rule_index += 1;
            let rule = &self.rules.rules[id];
            if !rule.is_assertion() || rule.disabled {
                continue;
            }
            let literal = rule.literals[0];
            if !self.decisions.decided(literal) {
                self.decisions.decide(literal, 1, id)?;
                continue;
            }
            if self.decisions.satisfy(literal) {
                continue;
            }
            if rule.rule_type == Some(RuleType::Learned) {
                self.rules.rules[id].disabled = true;
                continue;
            }
            let conflict = self.decisions.decision_rule(literal)?;
            if self.rules.rules[conflict].rule_type == Some(RuleType::Package) {
                let mut problem = Problem::new();
                problem.add_rule(id);
                problem.add_rule(conflict);
                self.rules.rules[id].disabled = true;
                self.problems.push(problem);
                continue;
            }
            let mut problem = Problem::new();
            problem.add_rule(id);
            problem.add_rule(conflict);
            for &assert_id in self.rules.ids_of_type(RuleType::Request).to_vec().iter() {
                let assert_rule = &self.rules.rules[assert_id];
                if assert_rule.disabled || !assert_rule.is_assertion() {
                    continue;
                }
                if literal.abs() != assert_rule.literals[0].abs() {
                    continue;
                }
                problem.add_rule(assert_id);
                self.rules.rules[assert_id].disabled = true;
            }
            self.problems.push(problem);
            self.decisions.reset_to_offset(decision_start);
            rule_index = 0;
        }
        Ok(())
    }

    /// `checkForRootRequireProblems`.
    fn check_for_root_require_problems(
        &mut self,
        request: &Request,
        filter: &PlatformRequirementFilter,
    ) {
        for (name, constraint) in request.requires.iter() {
            if filter.is_ignored(name) {
                continue;
            }
            let constraint = filter.filter_constraint(name, constraint, true);
            if self
                .pool
                .what_provides(self.arena, name, Some(&constraint))
                .is_empty()
            {
                let mut problem = Problem::new();
                problem.add_detached(Rule::generic(
                    Vec::new(),
                    Reason::RootRequire {
                        package_name: name.clone(),
                        constraint,
                    },
                ));
                self.problems.push(problem);
            }
        }
    }

    /// `solve`.
    pub fn solve(
        &mut self,
        request: &Request,
        policy: &mut DefaultPolicy,
        filter: &PlatformRequirementFilter,
    ) -> Result<LockTransaction, SolveError> {
        self.fixed_map = request
            .fixed_packages
            .iter()
            .filter_map(|&idx| self.pool.id_of(idx))
            .collect();
        let generator = RuleSetGenerator::new(self.pool, self.arena);
        self.rules = generator
            .rules_for(request, filter)
            .map_err(|e| SolveError::Bug(e.0))?;
        self.check_for_root_require_problems(request, filter);
        self.decisions = Decisions::new(self.pool.len());
        self.watch_graph = RuleWatchGraph::new();
        for id in self.rules.ids_in_iterator_order() {
            let node = RuleWatchNode::new(&self.rules, id);
            self.watch_graph.insert(&self.rules, node);
        }
        self.make_assertion_rule_decisions()?;
        self.run_sat(policy)?;
        if !self.problems.is_empty() {
            return Err(SolveError::Problems(std::mem::take(&mut self.problems)));
        }
        Ok(LockTransaction::new(
            self.pool,
            self.arena,
            request,
            &self.decisions,
        ))
    }

    /// `propagate`.
    fn propagate(&mut self, level: i64) -> Result<Option<usize>, SolveError> {
        while self.decisions.valid_offset(self.propagate_index) {
            let decision = self.decisions.at_offset(self.propagate_index);
            let conflict = self.watch_graph.propagate_literal(
                &self.rules,
                decision.literal,
                level,
                &mut self.decisions,
            )?;
            self.propagate_index += 1;
            if conflict.is_some() {
                return Ok(conflict);
            }
        }
        Ok(None)
    }

    /// `revert`.
    fn revert(&mut self, level: i64) {
        while !self.decisions.is_empty() {
            let literal = self.decisions.last_literal();
            if self.decisions.undecided(literal) {
                break;
            }
            let decision_level = self.decisions.decision_level(literal);
            if decision_level <= level {
                break;
            }
            self.decisions.revert_last();
            self.propagate_index = self.decisions.len();
        }
        while self.branches.last().is_some_and(|(_, l)| *l >= level) {
            self.branches.pop();
        }
    }

    /// `setPropagateLearn`.
    fn set_propagate_learn(
        &mut self,
        mut level: i64,
        literal: i64,
        rule: usize,
    ) -> Result<i64, SolveError> {
        level += 1;
        self.decisions.decide(literal, level, rule)?;
        loop {
            let Some(conflict) = self.propagate(level)? else {
                break;
            };
            if level == 1 {
                self.analyze_unsolvable(conflict);
                return Ok(0);
            }
            let (learn_literal, new_level, new_rule, why) = self.analyze(level, conflict)?;
            if new_level <= 0 || new_level >= level {
                return Err(SolveError::Bug(format!(
                    "Trying to revert to invalid level {new_level} from level {level}."
                )));
            }
            level = new_level;
            self.revert(level);
            // Un doublon n'entre pas dans le RuleSet mais sert quand même
            // de nœud de surveillance et de raison (`add` rend sans rien
            // faire, le reste du code PHP continue avec l'objet).
            let (new_id, _) = self.rules.add(new_rule, RuleType::Learned);
            self.learned_why.insert(new_id, why);
            let mut node = RuleWatchNode::new(&self.rules, new_id);
            node.watch2_on_highest(&self.rules, &self.decisions);
            self.watch_graph.insert(&self.rules, node);
            self.decisions.decide(learn_literal, level, new_id)?;
        }
        Ok(level)
    }

    /// `selectAndInstall`.
    fn select_and_install(
        &mut self,
        level: i64,
        decision_queue: &[i64],
        rule: usize,
        policy: &mut DefaultPolicy,
    ) -> Result<i64, SolveError> {
        let required = self.rules.rules[rule]
            .required_package(self.arena)
            .map(str::to_owned);
        let mut literals = policy.select_preferred_packages(
            self.pool,
            self.arena,
            decision_queue,
            required.as_deref(),
        );
        let selected = literals.remove(0);
        if !literals.is_empty() {
            self.branches.push((literals, level));
        }
        self.set_propagate_learn(level, selected, rule)
    }

    /// `analyze` : (literal appris, niveau, nouvelle règle, why).
    fn analyze(&mut self, level: i64, rule: usize) -> Result<(i64, i64, Rule, usize), SolveError> {
        let analyzed_rule = rule;
        let mut rule = rule;
        let mut rule_level: i64 = 1;
        let mut num = 0;
        let mut l1num = 0;
        let mut seen: HashSet<i64> = HashSet::new();
        let mut learned_literal: Option<i64> = None;
        let mut other_learned_literals: Vec<i64> = Vec::new();
        let mut decision_id = self.decisions.len() as i64;
        self.learned_pool.push(Vec::new());
        let why = self.learned_pool.len() - 1;
        'outer: loop {
            self.learned_pool[why].push(rule);
            let is_multi = self.rules.rules[rule].kind == RuleKind::MultiConflict;
            for &literal in &self.rules.rules[rule].literals {
                if is_multi && !self.decisions.decided(literal) {
                    continue;
                }
                if self.decisions.satisfy(literal) {
                    continue;
                }
                if seen.contains(&literal.abs()) {
                    continue;
                }
                seen.insert(literal.abs());
                let l = self.decisions.decision_level(literal);
                if l == 1 {
                    l1num += 1;
                } else if level == l {
                    num += 1;
                } else {
                    other_learned_literals.push(literal);
                    if l > rule_level {
                        rule_level = l;
                    }
                }
            }
            let mut l1retry = true;
            let mut literal: i64;
            while l1retry {
                l1retry = false;
                if num == 0 {
                    l1num -= 1;
                    if l1num == 0 {
                        break 'outer;
                    }
                }
                loop {
                    if decision_id <= 0 {
                        return Err(SolveError::Bug(format!(
                            "Reached invalid decision id {decision_id} while looking through rule {rule} for a literal present in the analyzed rule {analyzed_rule}."
                        )));
                    }
                    decision_id -= 1;
                    let decision = self.decisions.at_offset(decision_id as usize);
                    literal = decision.literal;
                    if seen.contains(&literal.abs()) {
                        break;
                    }
                }
                seen.remove(&literal.abs());
                if num != 0 {
                    num -= 1;
                    if num == 0 {
                        learned_literal = Some(-literal);
                        if l1num == 0 {
                            break 'outer;
                        }
                        for other in &other_learned_literals {
                            seen.remove(&other.abs());
                        }
                        l1num += 1;
                        l1retry = true;
                        continue;
                    }
                }
                // `else` de `if (0 !== $num && 0 === --$num)` : atteint quand
                // num était 0, ou quand la décrémentation ne l'a pas annulé.
                let decision = self.decisions.at_offset(decision_id as usize);
                rule = decision.reason;
                if self.rules.rules[rule].kind == RuleKind::MultiConflict {
                    let literals = self.rules.rules[rule].literals.clone();
                    for rule_literal in literals {
                        if !seen.contains(&rule_literal.abs())
                            && self.decisions.satisfy(-rule_literal)
                        {
                            self.learned_pool[why].push(rule);
                            let l = self.decisions.decision_level(rule_literal);
                            if l == 1 {
                                l1num += 1;
                            } else if level == l {
                                num += 1;
                            } else {
                                other_learned_literals.push(rule_literal);
                                if l > rule_level {
                                    rule_level = l;
                                }
                            }
                            seen.insert(rule_literal.abs());
                            break;
                        }
                    }
                    l1retry = true;
                }
            }
            let decision = self.decisions.at_offset(decision_id as usize);
            rule = decision.reason;
        }
        let Some(learned_literal) = learned_literal else {
            return Err(SolveError::Bug(format!(
                "Did not find a learnable literal in analyzed rule {analyzed_rule}."
            )));
        };
        other_learned_literals.insert(0, learned_literal);
        let new_rule = Rule::generic(other_learned_literals, Reason::Learned(why));
        Ok((learned_literal, rule_level, new_rule, why))
    }

    /// `analyzeUnsolvableRule`.
    fn analyze_unsolvable_rule(
        &self,
        problem: &mut Problem,
        conflict_rule: usize,
        rule_seen: &mut HashSet<usize>,
    ) {
        rule_seen.insert(conflict_rule);
        let rule = &self.rules.rules[conflict_rule];
        if rule.rule_type == Some(RuleType::Learned) {
            let learned_why = self.learned_why[&conflict_rule];
            let problem_rules = self.learned_pool[learned_why].clone();
            for problem_rule in problem_rules {
                if !rule_seen.contains(&problem_rule) {
                    self.analyze_unsolvable_rule(problem, problem_rule, rule_seen);
                }
            }
            return;
        }
        if rule.rule_type == Some(RuleType::Package) {
            return;
        }
        problem.next_section();
        problem.add_rule(conflict_rule);
    }

    /// `analyzeUnsolvable`.
    fn analyze_unsolvable(&mut self, conflict_rule: usize) {
        let mut problem = Problem::new();
        problem.add_rule(conflict_rule);
        let mut rule_seen: HashSet<usize> = HashSet::new();
        self.analyze_unsolvable_rule(&mut problem, conflict_rule, &mut rule_seen);
        let mut seen: HashSet<i64> = HashSet::new();
        for &literal in &self.rules.rules[conflict_rule].literals {
            if self.decisions.satisfy(literal) {
                continue;
            }
            seen.insert(literal.abs());
        }
        // `foreach ($this->decisions …)` : de la dernière à la première.
        for i in (0..self.decisions.len()).rev() {
            let decision = self.decisions.at_offset(i);
            if !seen.contains(&decision.literal.abs()) {
                continue;
            }
            let why = decision.reason;
            problem.add_rule(why);
            self.analyze_unsolvable_rule(&mut problem, why, &mut rule_seen);
            for &literal in &self.rules.rules[why].literals {
                if self.decisions.satisfy(literal) {
                    continue;
                }
                seen.insert(literal.abs());
            }
        }
        self.problems.push(problem);
    }

    /// `runSat`.
    fn run_sat(&mut self, policy: &mut DefaultPolicy) -> Result<(), SolveError> {
        self.propagate_index = 0;
        let mut level: i64 = 1;
        let mut system_level = level + 1;
        loop {
            if level == 1 {
                if let Some(conflict) = self.propagate(level)? {
                    self.analyze_unsolvable(conflict);
                    return Ok(());
                }
            }
            if level < system_level {
                // `foreach ($iterator as $rule)` sur les règles REQUEST, puis
                // `$iterator->next(); if valid → continue` : après un
                // `break` (retour en arrière) ailleurs qu'à la dernière règle,
                // on repart du début.
                let request_rules: Vec<usize> = self.rules.ids_of_type(RuleType::Request).to_vec();
                let mut position = 0;
                let mut broke = false;
                while position < request_rules.len() {
                    let rule_id = request_rules[position];
                    if self.rules.rules[rule_id].is_enabled() {
                        let mut decision_queue: Vec<i64> = Vec::new();
                        let mut none_satisfied = true;
                        for &literal in &self.rules.rules[rule_id].literals {
                            if self.decisions.satisfy(literal) {
                                none_satisfied = false;
                                break;
                            }
                            if literal > 0 && self.decisions.undecided(literal) {
                                decision_queue.push(literal);
                            }
                        }
                        if none_satisfied && !decision_queue.is_empty() {
                            let pruned: Vec<i64> = decision_queue
                                .iter()
                                .copied()
                                .filter(|l| self.fixed_map.contains(&(l.unsigned_abs() as usize)))
                                .collect();
                            if !pruned.is_empty() {
                                decision_queue = pruned;
                            }
                        }
                        if none_satisfied && !decision_queue.is_empty() {
                            let o_level = level;
                            level =
                                self.select_and_install(level, &decision_queue, rule_id, policy)?;
                            if level == 0 {
                                return Ok(());
                            }
                            if level <= o_level {
                                broke = true;
                                break;
                            }
                        }
                    }
                    position += 1;
                }
                system_level = level + 1;
                if broke {
                    position += 1;
                    if position < request_rules.len() {
                        continue;
                    }
                }
            }
            if level < system_level {
                system_level = level;
            }
            let mut rules_count = self.rules.len();
            let mut i = 0;
            let mut n = 0;
            while n < rules_count {
                if i == rules_count {
                    i = 0;
                }
                let rule_id = self.rules.rule_by_id[i];
                i += 1;
                n += 1;
                let rule = &self.rules.rules[rule_id];
                if rule.disabled {
                    continue;
                }
                let mut decision_queue: Vec<i64> = Vec::new();
                let mut skip = false;
                for &literal in &rule.literals {
                    if literal <= 0 {
                        if !self.decisions.decided_install(literal) {
                            skip = true;
                            break;
                        }
                    } else {
                        if self.decisions.decided_install(literal) {
                            skip = true;
                            break;
                        }
                        if self.decisions.undecided(literal) {
                            decision_queue.push(literal);
                        }
                    }
                }
                if skip || decision_queue.len() < 2 {
                    continue;
                }
                level = self.select_and_install(level, &decision_queue, rule_id, policy)?;
                if level == 0 {
                    return Ok(());
                }
                rules_count = self.rules.len();
                n = 0;
            }
            if level < system_level {
                continue;
            }
            if !self.branches.is_empty() {
                let mut last_literal: Option<i64> = None;
                let mut last_level: i64 = 0;
                let mut last_branch_index = 0;
                let mut last_branch_offset = 0;
                for bi in (0..self.branches.len()).rev() {
                    let (literals, l) = &self.branches[bi];
                    for (offset, &literal) in literals.iter().enumerate() {
                        if literal > 0 && self.decisions.decision_level(literal) > l + 1 {
                            last_literal = Some(literal);
                            last_branch_index = bi;
                            last_branch_offset = offset;
                            last_level = *l;
                        }
                    }
                }
                if let Some(last_literal) = last_literal {
                    self.branches[last_branch_index]
                        .0
                        .remove(last_branch_offset);
                    level = last_level;
                    self.revert(level);
                    let why = self.decisions.last_reason();
                    level = self.set_propagate_learn(level, last_literal, why)?;
                    if level == 0 {
                        return Ok(());
                    }
                    continue;
                }
            }
            break;
        }
        Ok(())
    }
}
