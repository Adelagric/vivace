//! Port de `Rule`, `GenericRule`, `Rule2Literals`, `MultiConflictRule` et
//! `RuleSet` (docs/reference/resolver/). Les règles vivent dans le `RuleSet`
//! et se désignent par leur `ruleById` ; les littéraux sont des
//! identifiants de pool signés.

use crate::constraint::Constraint;
use crate::package::Link;
use std::collections::HashMap;

/// `Rule::RULE_*`.
#[derive(Debug, Clone)]
pub enum Reason {
    /// `RULE_ROOT_REQUIRE` : `['packageName' => …, 'constraint' => …]`.
    RootRequire {
        package_name: String,
        constraint: Constraint,
    },
    /// `RULE_FIXED` : `['package' => …]` (index d'arène).
    Fixed { package: usize },
    /// `RULE_PACKAGE_CONFLICT` : le lien.
    PackageConflict(Link),
    /// `RULE_PACKAGE_REQUIRES` : le lien.
    PackageRequires(Link),
    /// `RULE_PACKAGE_SAME_NAME` : le nom remplacé.
    PackageSameName(String),
    /// `RULE_LEARNED` : index dans `learnedPool`.
    Learned(usize),
    /// `RULE_PACKAGE_ALIAS` : l'alias (index d'arène).
    PackageAlias { alias: usize },
    /// `RULE_PACKAGE_INVERSE_ALIAS` : le paquet aliasé (index d'arène).
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

/// Classe PHP de la règle : détermine le hachage (donc la déduplication)
/// et la forme des watches.
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
    /// Triés croissants (`sort($literals)`), sauf Rule2Literals : [min, max]
    /// — identique.
    pub literals: Vec<i64>,
    pub reason: Reason,
    /// None = 255 : jamais ajoutée au RuleSet (règle apprise en doublon,
    /// qui sert quand même de raison et de nœud de surveillance).
    pub rule_type: Option<RuleType>,
    pub disabled: bool,
}

impl Rule {
    /// `new GenericRule($literals, …)`.
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

    /// `new Rule2Literals($l1, $l2, …)`.
    pub fn two_literals(l1: i64, l2: i64, reason: Reason) -> Rule {
        Rule {
            kind: RuleKind::TwoLiterals,
            literals: if l1 < l2 { vec![l1, l2] } else { vec![l2, l1] },
            reason,
            rule_type: None,
            disabled: false,
        }
    }

    /// `new MultiConflictRule($literals, …)` (au moins 3 littéraux).
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

/// Clé de déduplication : le hachage PHP (xxh3 des littéraux pour
/// GenericRule, `"l1,l2"` pour Rule2Literals, xxh3 préfixé `c:` pour
/// MultiConflictRule) suivi d'`equals` — équivalent à (classe, littéraux),
/// aux collisions 32 bits près entre classes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuleKey {
    kind: RuleKind,
    literals: Vec<i64>,
}

/// `Composer\DependencyResolver\RuleSet`. `rules` est le stockage de
/// toutes les règles (y compris celles refusées par `add` en doublon, que
/// le solveur continue d'utiliser) ; `rule_by_id` est `ruleById`.
#[derive(Debug, Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    /// `ruleById` : règles enregistrées, dans l'ordre d'ajout.
    pub rule_by_id: Vec<usize>,
    /// `rules[$type]` : règles par type, dans l'ordre d'ajout.
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

    /// `add` : la règle est stockée ; elle n'est enregistrée (type posé,
    /// `ruleById`) que si aucune règle identique n'existe. Rend
    /// (index de stockage, enregistrée ?).
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

    /// `count()` : règles enregistrées.
    pub fn len(&self) -> usize {
        self.rule_by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rule_by_id.is_empty()
    }

    /// `getIteratorFor($type)` : identifiants d'un type, dans l'ordre.
    pub fn ids_of_type(&self, rule_type: RuleType) -> &[usize] {
        &self.by_type[Self::slot(rule_type)]
    }

    /// `getIterator()` : PACKAGE puis REQUEST puis LEARNED (types triés).
    pub fn ids_in_iterator_order(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.rules.len());
        for slot in &self.by_type {
            out.extend(slot.iter().copied());
        }
        out
    }
}
