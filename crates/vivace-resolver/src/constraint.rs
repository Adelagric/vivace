//! Port de `Composer\Semver\Constraint\*` et de `VersionParser::parseConstraints`
//! (docs/reference/resolver/semver-Constraint.php, semver-MultiConstraint.php,
//! semver-Bound.php, semver-VersionParser.php). Une contrainte est un arbre :
//! feuille (opérateur, version normalisée), conjonction/disjonction, tout,
//! rien. `matches` reproduit `Constraint::matchSpecific` (et donc
//! `CompilingMatcher::match`, qui n'en est qu'une compilation).

use crate::phpver::{version_compare, version_compare_op};
use crate::version::{
    self, group, normalize, regex, VersionError, MODIFIER_REGEX, STABILITIES_REGEX,
};
use pcre2::bytes::Regex;
use std::cmp::Ordering;
use std::fmt;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "=" | "==" => Op::Eq,
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "<>" | "!=" => Op::Ne,
            _ => return None,
        })
    }

    /// `$transOpInt`.
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Ne => "!=",
        }
    }

    /// `str_replace('=', '', op)`.
    fn no_equal(self) -> &'static str {
        match self {
            Op::Eq => "",
            Op::Lt | Op::Le => "<",
            Op::Gt | Op::Ge => ">",
            Op::Ne => "!",
        }
    }
}

/// `Bound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub version: String,
    pub inclusive: bool,
}

impl Bound {
    pub fn zero() -> Bound {
        Bound {
            version: "0.0.0.0-dev".into(),
            inclusive: true,
        }
    }
    pub fn positive_infinity() -> Bound {
        Bound {
            version: format!("{}.0.0.0", i64::MAX),
            inclusive: false,
        }
    }
    pub fn is_zero(&self) -> bool {
        self.version == "0.0.0.0-dev" && self.inclusive
    }
    pub fn is_positive_infinity(&self) -> bool {
        self.version == format!("{}.0.0.0", i64::MAX) && !self.inclusive
    }
    /// `compareTo($other, '<' | '>')`.
    pub fn compare_to(&self, other: &Bound, op: &str) -> bool {
        if self == other {
            return false;
        }
        let c = version_compare(&self.version, &other.version);
        if c != Ordering::Equal {
            return match op {
                ">" => c == Ordering::Greater,
                _ => c == Ordering::Less,
            };
        }
        if op == ">" {
            other.inclusive
        } else {
            !other.inclusive
        }
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}]",
            self.version,
            if self.inclusive {
                "inclusive"
            } else {
                "exclusive"
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    /// `Constraint($op, $version)`.
    Single {
        op: Op,
        version: String,
    },
    /// `MultiConstraint` (au moins deux membres).
    Multi {
        constraints: Vec<Constraint>,
        conjunctive: bool,
    },
    MatchAll,
    MatchNone,
}

impl Constraint {
    pub fn new(op: Op, version: impl Into<String>) -> Constraint {
        Constraint::Single {
            op,
            version: version.into(),
        }
    }

    pub fn is_single(&self) -> bool {
        matches!(self, Constraint::Single { .. })
    }

    /// `Constraint::versionCompare` : branches `dev-*` à part.
    fn version_compare_branches(a: &str, b: &str, op: Op, compare_branches: bool) -> bool {
        let a_branch = a.starts_with("dev-");
        let b_branch = b.starts_with("dev-");
        if op == Op::Ne && (a_branch || b_branch) {
            return a != b;
        }
        if a_branch && b_branch {
            return op == Op::Eq && a == b;
        }
        if !compare_branches && (a_branch || b_branch) {
            return false;
        }
        version_compare_op(a, b, op.as_str())
    }

    /// `Constraint::matchSpecific($provider)` : `self` est la contrainte,
    /// `provider` la version proposée (ou une autre contrainte simple).
    fn match_specific(&self, provider: &Constraint, compare_branches: bool) -> bool {
        let (
            Constraint::Single { op, version },
            Constraint::Single {
                op: pop,
                version: pversion,
            },
        ) = (self, provider)
        else {
            return false;
        };
        let no_equal_op = op.no_equal();
        let provider_no_equal_op = pop.no_equal();
        let is_equal_op = *op == Op::Eq;
        let is_non_equal_op = *op == Op::Ne;
        let is_provider_equal_op = *pop == Op::Eq;
        let is_provider_non_equal_op = *pop == Op::Ne;

        if is_non_equal_op || is_provider_non_equal_op {
            if is_non_equal_op
                && !is_provider_non_equal_op
                && !is_provider_equal_op
                && pversion.starts_with("dev-")
            {
                return false;
            }
            if is_provider_non_equal_op
                && !is_non_equal_op
                && !is_equal_op
                && version.starts_with("dev-")
            {
                return false;
            }
            if !is_equal_op && !is_provider_equal_op {
                return true;
            }
            return Self::version_compare_branches(pversion, version, Op::Ne, compare_branches);
        }
        if *op != Op::Eq && no_equal_op == provider_no_equal_op {
            return !(version.starts_with("dev-") || pversion.starts_with("dev-"));
        }
        let (version1, version2, operator) = if is_equal_op {
            (version.as_str(), pversion.as_str(), *pop)
        } else {
            (pversion.as_str(), version.as_str(), *op)
        };
        if Self::version_compare_branches(version1, version2, operator, compare_branches) {
            return !(pop.as_str() == provider_no_equal_op
                && op.as_str() != no_equal_op
                && version_compare_op(pversion, version, "=="));
        }
        false
    }

    /// `ConstraintInterface::matches($provider)`.
    pub fn matches(&self, provider: &Constraint) -> bool {
        match self {
            Constraint::MatchAll => true,
            Constraint::MatchNone => false,
            Constraint::Single { .. } => match provider {
                Constraint::Single { .. } => self.match_specific(provider, false),
                other => other.matches(self),
            },
            Constraint::Multi {
                constraints,
                conjunctive,
            } => {
                if !conjunctive {
                    return constraints.iter().any(|c| provider.matches(c));
                }
                if let Constraint::Multi {
                    conjunctive: false, ..
                } = provider
                {
                    return provider.matches(self);
                }
                constraints.iter().all(|c| provider.matches(c))
            }
        }
    }

    /// `CompilingMatcher::match($constraint, OP_EQ, $version)` — la forme
    /// utilisée partout par Composer pour tester une version.
    pub fn matches_version(&self, version: &str) -> bool {
        self.matches(&Constraint::new(Op::Eq, version))
    }

    /// `MultiConstraint::create($constraints, $conjunctive)`.
    pub fn create(constraints: Vec<Constraint>, conjunctive: bool) -> Constraint {
        if constraints.is_empty() {
            return Constraint::MatchAll;
        }
        if constraints.len() == 1 {
            return constraints
                .into_iter()
                .next()
                .unwrap_or(Constraint::MatchAll);
        }
        if let Some((optimized, conj)) = Self::optimize_constraints(&constraints, conjunctive) {
            if optimized.len() == 1 {
                return optimized.into_iter().next().unwrap_or(Constraint::MatchAll);
            }
            return Constraint::Multi {
                constraints: optimized,
                conjunctive: conj,
            };
        }
        Constraint::Multi {
            constraints,
            conjunctive,
        }
    }

    /// `MultiConstraint::optimizeConstraints` : fusion de `>=a <b || >=b <c`.
    fn optimize_constraints(
        constraints: &[Constraint],
        conjunctive: bool,
    ) -> Option<(Vec<Constraint>, bool)> {
        if conjunctive {
            return None;
        }
        let two_bounds = |c: &Constraint| -> Option<(String, String, Constraint, Constraint)> {
            let Constraint::Multi {
                constraints: parts,
                conjunctive: true,
            } = c
            else {
                return None;
            };
            if parts.len() != 2 {
                return None;
            }
            let s0 = parts[0].to_string();
            let s1 = parts[1].to_string();
            if s0.starts_with(">=") && s1.starts_with('<') && !s1.starts_with("<=") {
                Some((s0, s1, parts[0].clone(), parts[1].clone()))
            } else {
                None
            }
        };
        let mut left = constraints[0].clone();
        let mut merged: Vec<Constraint> = Vec::new();
        let mut optimized = false;
        for right in &constraints[1..] {
            let fused = match (two_bounds(&left), two_bounds(right)) {
                (Some((_, l1, l_lo, _)), Some((r0, _, _, r_hi))) if l1[2..] == r0[3..] => {
                    Some(Constraint::Multi {
                        constraints: vec![l_lo, r_hi],
                        conjunctive: true,
                    })
                }
                _ => None,
            };
            if let Some(f) = fused {
                optimized = true;
                left = f;
            } else {
                merged.push(left);
                left = right.clone();
            }
        }
        if optimized {
            merged.push(left);
            return Some((merged, false));
        }
        None
    }

    pub fn lower_bound(&self) -> Bound {
        self.bounds().0
    }

    pub fn upper_bound(&self) -> Bound {
        self.bounds().1
    }

    /// `extractBounds`.
    fn bounds(&self) -> (Bound, Bound) {
        match self {
            Constraint::MatchAll => (Bound::zero(), Bound::positive_infinity()),
            Constraint::MatchNone => {
                let b = Bound {
                    version: "0.0.0.0-dev".into(),
                    inclusive: false,
                };
                (b.clone(), b)
            }
            Constraint::Single { op, version } => {
                if version.starts_with("dev-") {
                    return (Bound::zero(), Bound::positive_infinity());
                }
                let b = |inc: bool| Bound {
                    version: version.clone(),
                    inclusive: inc,
                };
                match op {
                    Op::Eq => (b(true), b(true)),
                    Op::Lt => (Bound::zero(), b(false)),
                    Op::Le => (Bound::zero(), b(true)),
                    Op::Gt => (b(false), Bound::positive_infinity()),
                    Op::Ge => (b(true), Bound::positive_infinity()),
                    Op::Ne => (Bound::zero(), Bound::positive_infinity()),
                }
            }
            Constraint::Multi {
                constraints,
                conjunctive,
            } => {
                let mut lower: Option<Bound> = None;
                let mut upper: Option<Bound> = None;
                for c in constraints {
                    let (l, u) = c.bounds();
                    match (&lower, &upper) {
                        (Some(cl), Some(cu)) => {
                            if l.compare_to(cl, if *conjunctive { ">" } else { "<" }) {
                                lower = Some(l);
                            }
                            if u.compare_to(cu, if *conjunctive { "<" } else { ">" }) {
                                upper = Some(u);
                            }
                        }
                        _ => {
                            lower = Some(l);
                            upper = Some(u);
                        }
                    }
                }
                (
                    lower.unwrap_or_else(Bound::zero),
                    upper.unwrap_or_else(Bound::positive_infinity),
                )
            }
        }
    }
}

impl fmt::Display for Constraint {
    /// `__toString` : `>= 1.0.0.0`, `[>= 1.0.0.0 < 2.0.0.0-dev]`, `*`, `[]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constraint::Single { op, version } => write!(f, "{} {}", op.as_str(), version),
            Constraint::Multi {
                constraints,
                conjunctive,
            } => {
                let parts: Vec<String> = constraints.iter().map(|c| c.to_string()).collect();
                write!(
                    f,
                    "[{}]",
                    parts.join(if *conjunctive { " " } else { " || " })
                )
            }
            Constraint::MatchAll => write!(f, "*"),
            Constraint::MatchNone => write!(f, "[]"),
        }
    }
}

/// Une contrainte parsée avec sa chaîne jolie d'origine (`getPrettyString`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConstraint {
    pub constraint: Constraint,
    pub pretty: String,
}

/// `VersionParser::parseConstraints`.
pub fn parse_constraints(input: &str) -> Result<ParsedConstraint, VersionError> {
    static OR: OnceLock<Regex> = OnceLock::new();
    static AND: OnceLock<Regex> = OnceLock::new();
    let pretty = input.to_owned();
    let trimmed = input.trim();
    let or_parts = preg_split(regex(&OR, r"\s*\|\|?\s*", false), trimmed);
    let mut or_groups: Vec<Constraint> = Vec::new();
    for or_part in or_parts {
        let and_parts = preg_split(
            regex(
                &AND,
                r"(?<!^|as|[=>< ,]) *(?<!-)[, ](?!-) *(?!,|as|$)",
                false,
            ),
            &or_part,
        );
        let objects: Vec<Constraint> = if and_parts.len() > 1 {
            let mut out = Vec::new();
            for p in &and_parts {
                out.extend(parse_constraint(p)?);
            }
            out
        } else {
            parse_constraint(&and_parts[0])?
        };
        or_groups.push(if objects.len() == 1 {
            objects.into_iter().next().unwrap_or(Constraint::MatchAll)
        } else {
            Constraint::Multi {
                constraints: objects,
                conjunctive: true,
            }
        });
    }
    Ok(ParsedConstraint {
        constraint: Constraint::create(or_groups, false),
        pretty,
    })
}

/// `preg_split` sans limite ni flags : morceaux entre les correspondances
/// (chaînes vides conservées).
fn preg_split(re: &Regex, subject: &str) -> Vec<String> {
    let bytes = subject.as_bytes();
    let mut out = Vec::new();
    let mut last = 0;
    for m in re.find_iter(bytes).flatten() {
        if m.start() == m.end() && m.start() == last && last == bytes.len() {
            break;
        }
        out.push(subject[last..m.start()].to_owned());
        last = m.end();
    }
    out.push(subject[last..].to_owned());
    out
}

/// `manipulateVersionString($matches, $position, $increment, $pad)`.
fn manipulate_version_string(
    matches: &[String],
    position: usize,
    increment: i64,
) -> Option<String> {
    let mut m: Vec<String> = matches.to_vec();
    let mut position = position;
    let mut i = 4;
    while i > 0 {
        if i > position {
            m[i] = "0".to_owned();
        } else if i == position && increment != 0 {
            let v: i64 = m[i].parse().unwrap_or(0) + increment;
            if v < 0 {
                m[i] = "0".to_owned();
                position -= 1;
                if i == 1 {
                    return None;
                }
            } else {
                m[i] = v.to_string();
            }
        }
        i -= 1;
    }
    Some(format!("{}.{}.{}.{}", m[1], m[2], m[3], m[4]))
}

fn empty(s: &str) -> bool {
    s.is_empty() || s == "0"
}

/// `VersionParser::parseConstraint` (une contrainte élémentaire → 1 ou 2
/// bornes).
fn parse_constraint(input: &str) -> Result<Vec<Constraint>, VersionError> {
    static AS: OnceLock<Regex> = OnceLock::new();
    static STAB: OnceLock<Regex> = OnceLock::new();
    static REF: OnceLock<Regex> = OnceLock::new();
    static ANY: OnceLock<Regex> = OnceLock::new();
    static TILDE: OnceLock<Regex> = OnceLock::new();
    static CARET: OnceLock<Regex> = OnceLock::new();
    static XRANGE: OnceLock<Regex> = OnceLock::new();
    static HYPHEN: OnceLock<Regex> = OnceLock::new();
    static BASIC: OnceLock<Regex> = OnceLock::new();
    static MODIFIER_END: OnceLock<Regex> = OnceLock::new();

    let mut constraint = input.to_owned();
    let mut stability_modifier: Option<String> = None;

    if let Ok(Some(caps)) =
        regex(&AS, r"^([^,\s]++) ++as ++([^,\s]++)$", false).captures(constraint.as_bytes())
    {
        constraint = group(&caps, 1).to_owned();
    }
    if let Ok(Some(caps)) = regex(&STAB, &format!(r"^([^,\s]*?)@({STABILITIES_REGEX})$"), true)
        .captures(constraint.as_bytes())
    {
        let head = group(&caps, 1).to_owned();
        let stab = group(&caps, 2).to_owned();
        constraint = if head.is_empty() {
            "*".to_owned()
        } else {
            head
        };
        if stab != "stable" {
            stability_modifier = Some(stab);
        }
    }
    if let Ok(Some(caps)) =
        regex(&REF, r"^(dev-[^,\s@]+?|[^,\s@]+?\.x-dev)#.+$", true).captures(constraint.as_bytes())
    {
        constraint = group(&caps, 1).to_owned();
    }
    if let Ok(Some(caps)) =
        regex(&ANY, r"^(v)?[xX*](\.[xX*])*$", true).captures(constraint.as_bytes())
    {
        if !group(&caps, 1).is_empty() || !group(&caps, 2).is_empty() {
            return Ok(vec![Constraint::new(Op::Ge, "0.0.0.0-dev")]);
        }
        return Ok(vec![Constraint::MatchAll]);
    }

    let version_regex = format!(
        r"v?(\d++)(?:\.(\d++))?(?:\.(\d++))?(?:\.(\d++))?(?:{MODIFIER_REGEX}|\.([xX*][.-]?dev))(?:\+[^\s]+)?"
    );
    let groups_of = |re: &Regex, s: &str| -> Option<Vec<String>> {
        let caps = re.captures(s.as_bytes()).ok()??;
        Some(
            (0..caps.len())
                .map(|i| group(&caps, i).to_owned())
                .collect(),
        )
    };

    // ~ (tilde)
    if let Some(m) = groups_of(
        regex(&TILDE, &format!("^~>?{version_regex}$"), true),
        &constraint,
    ) {
        if constraint.starts_with("~>") {
            return Err(VersionError(format!(
                "Could not parse version constraint {constraint}: Invalid operator \"~>\", you probably meant to use the \"~\" operator"
            )));
        }
        let at = |i: usize| m.get(i).map(String::as_str).unwrap_or("");
        let mut position = if !at(4).is_empty() {
            4
        } else if !at(3).is_empty() {
            3
        } else if !at(2).is_empty() {
            2
        } else {
            1
        };
        if !empty(at(8)) {
            position += 1;
        }
        let mut stability_suffix = String::new();
        if empty(at(5)) && empty(at(7)) && empty(at(8)) {
            stability_suffix.push_str("-dev");
        }
        let low = normalize(&format!("{constraint}{stability_suffix}")[1..], None)?;
        let high_position = std::cmp::max(1, position - 1);
        let high = manipulate_version_string(&m, high_position, 1)
            .map(|v| format!("{v}-dev"))
            .ok_or_else(|| {
                VersionError(format!("Could not parse version constraint {constraint}"))
            })?;
        return Ok(vec![
            Constraint::new(Op::Ge, low),
            Constraint::new(Op::Lt, high),
        ]);
    }
    // ^ (caret)
    if let Some(m) = groups_of(
        regex(&CARET, &format!(r"^\^{version_regex}($)"), true),
        &constraint,
    ) {
        let at = |i: usize| m.get(i).map(String::as_str).unwrap_or("");
        let position = if at(1) != "0" || at(2).is_empty() {
            1
        } else if at(2) != "0" || at(3).is_empty() {
            2
        } else {
            3
        };
        let mut stability_suffix = String::new();
        if empty(at(5)) && empty(at(7)) && empty(at(8)) {
            stability_suffix.push_str("-dev");
        }
        let low = normalize(&format!("{constraint}{stability_suffix}")[1..], None)?;
        let high = manipulate_version_string(&m, position, 1)
            .map(|v| format!("{v}-dev"))
            .ok_or_else(|| {
                VersionError(format!("Could not parse version constraint {constraint}"))
            })?;
        return Ok(vec![
            Constraint::new(Op::Ge, low),
            Constraint::new(Op::Lt, high),
        ]);
    }
    // X ranges
    if let Some(m) = groups_of(
        regex(
            &XRANGE,
            r"^v?(\d++)(?:\.(\d++))?(?:\.(\d++))?(?:\.[xX*])++$",
            false,
        ),
        &constraint,
    ) {
        let at = |i: usize| m.get(i).map(String::as_str).unwrap_or("");
        let position = if !at(3).is_empty() {
            3
        } else if !at(2).is_empty() {
            2
        } else {
            1
        };
        let mut mm = m.clone();
        while mm.len() < 5 {
            mm.push(String::new());
        }
        let low = manipulate_version_string(&mm, position, 0)
            .map(|v| format!("{v}-dev"))
            .ok_or_else(|| {
                VersionError(format!("Could not parse version constraint {constraint}"))
            })?;
        let high = manipulate_version_string(&mm, position, 1)
            .map(|v| format!("{v}-dev"))
            .ok_or_else(|| {
                VersionError(format!("Could not parse version constraint {constraint}"))
            })?;
        if low == "0.0.0.0-dev" {
            return Ok(vec![Constraint::new(Op::Lt, high)]);
        }
        return Ok(vec![
            Constraint::new(Op::Ge, low),
            Constraint::new(Op::Lt, high),
        ]);
    }
    // hyphen range
    let hyphen = regex(
        &HYPHEN,
        &format!(r"^(?P<from>{version_regex}) +- +(?P<to>{version_regex})($)"),
        true,
    );
    if let Some(m) = groups_of(hyphen, &constraint) {
        // Groupes : 1 = from, 2..9 = composants de from, 10 = to, 11..18 = composants de to.
        let at = |i: usize| m.get(i).map(String::as_str).unwrap_or("");
        let mut low_suffix = String::new();
        if empty(at(6)) && empty(at(8)) && empty(at(9)) {
            low_suffix.push_str("-dev");
        }
        let low = normalize(at(1), None)?;
        let lower = Constraint::new(Op::Ge, format!("{low}{low_suffix}"));
        // `$empty` : "0" n'est pas vide, "" l'est.
        let php_empty = |s: &str| s.is_empty();
        let upper = if (!php_empty(at(12)) && !php_empty(at(13)))
            || !empty(at(15))
            || !empty(at(17))
            || !empty(at(18))
        {
            Constraint::new(Op::Le, normalize(at(10), None)?)
        } else {
            let high_match = vec![
                String::new(),
                at(11).to_owned(),
                at(12).to_owned(),
                at(13).to_owned(),
                at(14).to_owned(),
            ];
            normalize(at(10), None)?;
            let pos = if php_empty(at(12)) { 1 } else { 2 };
            let high = manipulate_version_string(&high_match, pos, 1)
                .map(|v| format!("{v}-dev"))
                .ok_or_else(|| {
                    VersionError(format!("Could not parse version constraint {constraint}"))
                })?;
            Constraint::new(Op::Lt, high)
        };
        return Ok(vec![lower, upper]);
    }
    // basic comparators
    if let Some(m) = groups_of(
        regex(&BASIC, r"^(<>|!=|>=?|<=?|==?)?\s*(.*)", false),
        &constraint,
    ) {
        let at = |i: usize| m.get(i).map(String::as_str).unwrap_or("");
        let op_str = at(1);
        let raw = at(2);
        let version = match normalize(raw, None) {
            Ok(v) => v,
            Err(e) => {
                if raw.ends_with("-dev")
                    && raw
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'/'))
                {
                    normalize(&format!("dev-{}", &raw[..raw.len() - 4]), None)?
                } else {
                    return Err(VersionError(format!(
                        "Could not parse version constraint {constraint}: {}",
                        e.0
                    )));
                }
            }
        };
        let op = if op_str.is_empty() { "=" } else { op_str };
        let mut version = version;
        if op != "=="
            && op != "="
            && stability_modifier.is_some()
            && version::parse_stability(&version) == "stable"
        {
            if let Some(s) = &stability_modifier {
                version = format!("{version}-{s}");
            }
        } else if op == "<" || op == ">=" {
            let modifier_end = regex(&MODIFIER_END, &format!("-{MODIFIER_REGEX}$"), false);
            let lower = raw.to_lowercase();
            if !modifier_end.is_match(lower.as_bytes()).unwrap_or(false) && !raw.starts_with("dev-")
            {
                version.push_str("-dev");
            }
        }
        let op = Op::parse(op).ok_or_else(|| {
            VersionError(format!("Could not parse version constraint {constraint}"))
        })?;
        return Ok(vec![Constraint::new(op, version)]);
    }
    Err(VersionError(format!(
        "Could not parse version constraint {constraint}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        parse_constraints(s).unwrap().constraint.to_string()
    }

    #[test]
    fn parses_common_forms() {
        assert_eq!(p("^1.2"), "[>= 1.2.0.0-dev < 2.0.0.0-dev]");
        assert_eq!(p("~1.2"), "[>= 1.2.0.0-dev < 2.0.0.0-dev]");
        assert_eq!(p("~1.2.3"), "[>= 1.2.3.0-dev < 1.3.0.0-dev]");
        assert_eq!(p("^0.3"), "[>= 0.3.0.0-dev < 0.4.0.0-dev]");
        assert_eq!(p("1.0.*"), "[>= 1.0.0.0-dev < 1.1.0.0-dev]");
        assert_eq!(p("*"), "*");
        assert_eq!(p(">=1.0"), ">= 1.0.0.0-dev");
        assert_eq!(p("<2"), "< 2.0.0.0-dev");
        assert_eq!(p("1.0.0"), "== 1.0.0.0");
        assert_eq!(p("^1.0 || ^2.0"), "[>= 1.0.0.0-dev < 3.0.0.0-dev]");
        assert_eq!(
            p("~1.2 || ~2.0 || ~4.0"),
            "[[>= 1.2.0.0-dev < 3.0.0.0-dev] || [>= 4.0.0.0-dev < 5.0.0.0-dev]]"
        );
        assert_eq!(p(">=1.0 <2.0"), "[>= 1.0.0.0-dev < 2.0.0.0-dev]");
        assert_eq!(p("1.0 - 2.0"), "[>= 1.0.0.0-dev < 2.1.0.0-dev]");
        assert_eq!(p("dev-main"), "== dev-main");
        assert_eq!(p("1.x-dev"), "== 1.9999999.9999999.9999999-dev");
        assert_eq!(p("^8.1@dev"), "[>= 8.1.0.0-dev < 9.0.0.0-dev]");
        assert_eq!(p("!=1.0"), "!= 1.0.0.0");
        assert_eq!(p("dev-main#abc"), "== dev-main");
        assert_eq!(p("^1.0@beta"), "[>= 1.0.0.0-dev < 2.0.0.0-dev]");
        assert_eq!(p("2.0.x-dev"), "== 2.0.9999999.9999999-dev");
    }

    #[test]
    fn matching_and_bounds() {
        let c = parse_constraints("^1.2").unwrap().constraint;
        assert!(c.matches_version("1.5.0.0"));
        assert!(!c.matches_version("2.0.0.0"));
        assert!(c.matches_version("1.2.0.0-beta1"));
        assert!(!c.matches_version("dev-main"));
        let d = parse_constraints("dev-main").unwrap().constraint;
        assert!(d.matches_version("dev-main"));
        assert!(!d.matches_version("1.0.0.0"));
        let ne = parse_constraints("!=1.0").unwrap().constraint;
        assert!(ne.matches_version("dev-main"));
        assert_eq!(c.lower_bound().to_string(), "1.2.0.0-dev [inclusive]");
        assert_eq!(c.upper_bound().to_string(), "2.0.0.0-dev [exclusive]");
    }
}
