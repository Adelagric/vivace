//! Port de `RuleWatchGraph`, `RuleWatchNode`, `RuleWatchChain` : deux
//! littéraux surveillés par règle (tous pour une MultiConflictRule), une
//! chaîne par littéral, parcourue en tête de liste (`unshift`).

use crate::decisions::{Decisions, SolverBug};
use crate::rule::{RuleKind, RuleSet};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone)]
pub struct RuleWatchNode {
    pub rule: usize,
    pub watch1: i64,
    pub watch2: i64,
}

impl RuleWatchNode {
    pub fn new(rules: &RuleSet, rule: usize) -> RuleWatchNode {
        let literals = &rules.rules[rule].literals;
        RuleWatchNode {
            rule,
            watch1: literals.first().copied().unwrap_or(0),
            watch2: literals.get(1).copied().unwrap_or(0),
        }
    }

    /// `watch2OnHighest`.
    pub fn watch2_on_highest(&mut self, rules: &RuleSet, decisions: &Decisions) {
        let rule = &rules.rules[self.rule];
        if rule.literals.len() < 3 || rule.kind == RuleKind::MultiConflict {
            return;
        }
        let mut watch_level = 0;
        for &literal in &rule.literals {
            let level = decisions.decision_level(literal);
            if level > watch_level {
                self.watch2 = literal;
                watch_level = level;
            }
        }
    }

    pub fn other_watch(&self, literal: i64) -> i64 {
        if self.watch1 == literal {
            self.watch2
        } else {
            self.watch1
        }
    }

    pub fn move_watch(&mut self, from: i64, to: i64) {
        if self.watch1 == from {
            self.watch1 = to;
        } else {
            self.watch2 = to;
        }
    }
}

#[derive(Debug, Default)]
pub struct RuleWatchGraph {
    pub nodes: Vec<RuleWatchNode>,
    /// littéral → nœuds, tête de liste en premier.
    chains: HashMap<i64, VecDeque<usize>>,
}

impl RuleWatchGraph {
    pub fn new() -> RuleWatchGraph {
        RuleWatchGraph::default()
    }

    /// `insert` : rien pour une assertion.
    pub fn insert(&mut self, rules: &RuleSet, node: RuleWatchNode) {
        let rule = &rules.rules[node.rule];
        if rule.is_assertion() {
            return;
        }
        let node_id = self.nodes.len();
        let literals: Vec<i64> = if rule.kind != RuleKind::MultiConflict {
            vec![node.watch1, node.watch2]
        } else {
            rule.literals.clone()
        };
        self.nodes.push(node);
        for literal in literals {
            self.chains.entry(literal).or_default().push_front(node_id);
        }
    }

    /// `propagateLiteral` : rend la règle en conflit, le cas échéant.
    pub fn propagate_literal(
        &mut self,
        rules: &RuleSet,
        decided_literal: i64,
        level: i64,
        decisions: &mut Decisions,
    ) -> Result<Option<usize>, SolverBug> {
        let literal = -decided_literal;
        if !self.chains.contains_key(&literal) {
            return Ok(None);
        }
        let mut cursor = 0;
        while let Some(&node_id) = self.chains.get(&literal).and_then(|c| c.get(cursor)) {
            let rule_id = self.nodes[node_id].rule;
            let rule = &rules.rules[rule_id];
            if rule.kind != RuleKind::MultiConflict {
                let other_watch = self.nodes[node_id].other_watch(literal);
                if !rule.disabled && !decisions.satisfy(other_watch) {
                    let alternative = rule
                        .literals
                        .iter()
                        .copied()
                        .find(|&l| l != literal && l != other_watch && !decisions.conflict(l));
                    if let Some(alternative) = alternative {
                        self.move_watch(literal, alternative, node_id, cursor);
                        // `continue` sans `next()` : l'élément suivant a pris
                        // cette position.
                        continue;
                    }
                    if decisions.conflict(other_watch) {
                        return Ok(Some(rule_id));
                    }
                    decisions.decide(other_watch, level, rule_id)?;
                }
            } else {
                for &other in &rule.literals {
                    if literal != other && !decisions.satisfy(other) {
                        if decisions.conflict(other) {
                            return Ok(Some(rule_id));
                        }
                        decisions.decide(other, level, rule_id)?;
                    }
                }
            }
            cursor += 1;
        }
        Ok(None)
    }

    /// `moveWatch` : retire le nœud de la chaîne courante (à la position du
    /// curseur) et le met en tête de la chaîne cible.
    fn move_watch(&mut self, from: i64, to: i64, node_id: usize, cursor: usize) {
        self.nodes[node_id].move_watch(from, to);
        if let Some(chain) = self.chains.get_mut(&from) {
            chain.remove(cursor);
        }
        self.chains.entry(to).or_default().push_front(node_id);
    }
}
