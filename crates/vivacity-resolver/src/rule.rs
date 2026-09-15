//! Port of `Rule`, `GenericRule`, `Rule2Literals`, `MultiConflictRule` and
//! `RuleSet` (docs/reference/resolver/). Rules live in the `RuleSet` and
//! are referred to by their `ruleById`; literals are signed pool ids.

use crate::constraint::Constraint;
use crate::package::Link;
use std::collections::HashMap;

/// `Rule::RULE_*`.
#[derive(Debug, Clone)]
pub enum Reason {
    /// `RULE_ROOT_REQUIRE`: `['packageName' => ..., 'constraint' => ...]`.
    RootRequire {
        package_name: String,
        constraint: Constraint,
        /// `$constraint->getPrettyString()`.
        pretty: String,
    },
    /// `RULE_FIXED`: `['package' => ...]` (arena index).
    Fixed { package: usize },
    /// `RULE_PACKAGE_CONFLICT`: the link.
    PackageConflict(Link),
    /// `RULE_PACKAGE_REQUIRES`: the link.
    PackageRequires(Link),
    /// `RULE_PACKAGE_SAME_NAME`: the replaced name.
    PackageSameName(String),
    /// `RULE_LEARNED`: index into `learnedPool`.
    Learned(usize),
    /// `RULE_PACKAGE_ALIAS`: the alias (arena index).
    PackageAlias { alias: usize },
    /// `RULE_PACKAGE_INVERSE_ALIAS`: the aliased package (arena index).
    PackageInverseAlias { package: usize },
    /// `RULE_LOCKED_FILTER_LIST_REMOVED`.
    LockedFilterListRemoved { package: usize },
}

impl Reason {
    pub fn code(&self) -> u8 {
        match self {
            Reason::RootRequire { .. } => 2,
            Reason::Fixed { .. } => 3,
            Reason::PackageConflict(_) => 6,
            Reason::PackageRequires(_) => 7,
            Reason::PackageSameName(_) => 10,
            Reason::Learned(_) => 12,
            Reason::PackageAlias { .. } => 13,
            Reason::PackageInverseAlias { .. } => 14,
            Reason::LockedFilterListRemoved { .. } => 15,
        }
    }
}

/// PHP class of the rule: determines the hash (hence deduplication) and
/// the shape of the watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleKind {
    Generic,
    TwoLiterals,
    MultiConflict,
}

/// `RuleSet::TYPE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleType {
    Package = 0,
    Request = 1,
    Learned = 4,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub kind: RuleKind,
    /// Sorted ascending (`sort($literals)`), except Rule2Literals: [min, max],
    /// which is the same thing.
    pub literals: Vec<i64>,
    pub reason: Reason,
    /// None = 255: never added to the RuleSet (duplicate learned rule, still
    /// used as a reason and as a watch node).
    pub rule_type: Option<RuleType>,
    pub disabled: bool,
}

impl Rule {
    /// `new GenericRule($literals, ...)`.
    pub fn generic(mut literals: Vec<i64>, reason: Reason) -> Rule {
        literals.sort_unstable();
        Rule {
            kind: RuleKind::Generic,
            literals,
            reason,
            rule_type: None,
            disabled: false,
        }
    }

    /// `new Rule2Literals($l1, $l2, ...)`.
    pub fn two_literals(l1: i64, l2: i64, reason: Reason) -> Rule {
        Rule {
            kind: RuleKind::TwoLiterals,
            literals: if l1 < l2 { vec![l1, l2] } else { vec![l2, l1] },
            reason,
            rule_type: None,
            disabled: false,
        }
    }

    /// `new MultiConflictRule($literals, ...)` (at least 3 literals).
    pub fn multi_conflict(mut literals: Vec<i64>, reason: Reason) -> Rule {
        literals.sort_unstable();
        Rule {
            kind: RuleKind::MultiConflict,
            literals,
            reason,
            rule_type: None,
            disabled: false,
        }
    }

    pub fn is_assertion(&self) -> bool {
        self.kind == RuleKind::Generic && self.literals.len() == 1
    }

    pub fn is_enabled(&self) -> bool {
        !self.disabled
    }

    /// `getRequiredPackage`.
    pub fn required_package<'a>(&'a self, arena: &'a [crate::package::Package]) -> Option<&'a str> {
        match &self.reason {
            Reason::RootRequire { package_name, .. } => Some(package_name),
            Reason::Fixed { package } | Reason::LockedFilterListRemoved { package } => {
                Some(&arena[*package].name)
            }
            Reason::PackageRequires(link) => Some(&link.target),
            _ => None,
        }
    }
}

/// Deduplication key: the PHP hash (xxh3 of the literals for GenericRule,
/// `"l1,l2"` for Rule2Literals, xxh3 prefixed with `c:` for
/// MultiConflictRule) followed by `equals`; equivalent to (class,
/// literals), up to 32-bit collisions across classes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuleKey {
    kind: RuleKind,
    literals: Vec<i64>,
}

/// `Composer\DependencyResolver\RuleSet`. `rules` is the storage of all
/// rules (including those rejected by `add` as duplicates, which the solver
/// keeps using); `rule_by_id` is `ruleById`.
#[derive(Debug, Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    /// `ruleById`: registered rules, in insertion order.
    pub rule_by_id: Vec<usize>,
    /// `rules[$type]`: rules by type, in insertion order.
    by_type: [Vec<usize>; 3],
    keys: HashMap<RuleKey, usize>,
}

impl RuleSet {
    pub fn new() -> RuleSet {
        RuleSet::default()
    }

    fn slot(rule_type: RuleType) -> usize {
        match rule_type {
            RuleType::Package => 0,
            RuleType::Request => 1,
            RuleType::Learned => 2,
        }
    }

    /// `add`: the rule is stored; it is registered (type set, `ruleById`)
    /// only if no identical rule exists. Returns (storage index,
    /// registered?).
    pub fn add(&mut self, mut rule: Rule, rule_type: RuleType) -> (usize, bool) {
        let key = RuleKey {
            kind: rule.kind,
            literals: rule.literals.clone(),
        };
        let id = self.rules.len();
        if self.keys.contains_key(&key) {
            self.rules.push(rule);
            return (id, false);
        }
        rule.rule_type = Some(rule_type);
        self.rules.push(rule);
        self.rule_by_id.push(id);
        self.by_type[Self::slot(rule_type)].push(id);
        self.keys.insert(key, id);
        (id, true)
    }

    /// `count()`: registered rules.
    pub fn len(&self) -> usize {
        self.rule_by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rule_by_id.is_empty()
    }

    /// `getIteratorFor($type)`: ids of one type, in order.
    pub fn ids_of_type(&self, rule_type: RuleType) -> &[usize] {
        &self.by_type[Self::slot(rule_type)]
    }

    /// `getIterator()`: PACKAGE then REQUEST then LEARNED (sorted types).
    pub fn ids_in_iterator_order(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.rules.len());
        for slot in &self.by_type {
            out.extend(slot.iter().copied());
        }
        out
    }
}
