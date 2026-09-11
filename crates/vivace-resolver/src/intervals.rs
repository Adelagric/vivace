//! Port de `Composer\Semver\Intervals` (docs/reference/resolver/
//! semver-Intervals.php) : une contrainte devient une liste d'intervalles
//! numériques `[borne, borne]` plus un ensemble de branches (`dev-*`) inclus
//! ou exclus ; `isSubsetOf`, `haveIntersections` et `compactConstraint` en
//! découlent. PoolBuilder s'en sert pour savoir si un paquet déjà chargé
//! couvre une nouvelle contrainte, et pour fusionner des contraintes.

use crate::constraint::{Constraint, Op};
use crate::phpver::version_compare;
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interval {
    pub start: Constraint,
    pub end: Constraint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branches {
    pub names: Vec<String>,
    pub exclude: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intervals {
    pub numeric: Vec<Interval>,
    pub branches: Branches,
}

pub fn from_zero() -> Constraint {
    Constraint::new(Op::Ge, "0.0.0.0-dev")
}

pub fn until_positive_infinity() -> Constraint {
    Constraint::new(Op::Lt, format!("{}.0.0.0", i64::MAX))
}

fn any_dev() -> Branches {
    Branches {
        names: Vec::new(),
        exclude: true,
    }
}

fn no_dev() -> Branches {
    Branches {
        names: Vec::new(),
        exclude: false,
    }
}

fn op_of(c: &Constraint) -> Op {
    match c {
        Constraint::Single { op, .. } => *op,
        _ => Op::Eq,
    }
}

fn version_of(c: &Constraint) -> &str {
    match c {
        Constraint::Single { version, .. } => version,
        _ => "",
    }
}

/// `Intervals::isSubsetOf($candidate, $constraint)`.
pub fn is_subset_of(candidate: &Constraint, constraint: &Constraint) -> bool {
    if matches!(constraint, Constraint::MatchAll) {
        return true;
    }
    if matches!(candidate, Constraint::MatchNone) || matches!(constraint, Constraint::MatchNone) {
        return false;
    }
    let intersection = generate(
        &Constraint::Multi {
            constraints: vec![candidate.clone(), constraint.clone()],
            conjunctive: true,
        },
        false,
    );
    let cand = generate(candidate, false);
    if intersection.numeric.len() != cand.numeric.len() {
        return false;
    }
    for (a, b) in intersection.numeric.iter().zip(cand.numeric.iter()) {
        if b.start.to_string() != a.start.to_string() || b.end.to_string() != a.end.to_string() {
            return false;
        }
    }
    if intersection.branches.exclude != cand.branches.exclude {
        return false;
    }
    if intersection.branches.names.len() != cand.branches.names.len() {
        return false;
    }
    intersection
        .branches
        .names
        .iter()
        .zip(cand.branches.names.iter())
        .all(|(a, b)| a == b)
}

/// `Intervals::haveIntersections($a, $b)`.
pub fn have_intersections(a: &Constraint, b: &Constraint) -> bool {
    if matches!(a, Constraint::MatchAll) || matches!(b, Constraint::MatchAll) {
        return true;
    }
    if matches!(a, Constraint::MatchNone) || matches!(b, Constraint::MatchNone) {
        return false;
    }
    let i = generate(
        &Constraint::Multi {
            constraints: vec![a.clone(), b.clone()],
            conjunctive: true,
        },
        true,
    );
    !i.numeric.is_empty() || i.branches.exclude || !i.branches.names.is_empty()
}

/// `Intervals::compactConstraint($constraint)`.
pub fn compact_constraint(constraint: &Constraint) -> Constraint {
    if !matches!(constraint, Constraint::Multi { .. }) {
        return constraint.clone();
    }
    let intervals = generate(constraint, false);
    let mut constraints: Vec<Constraint> = Vec::new();
    let mut has_numeric_match_all = false;
    let zero = from_zero().to_string();
    let inf = until_positive_infinity().to_string();
    if intervals.numeric.len() == 1
        && intervals.numeric[0].start.to_string() == zero
        && intervals.numeric[0].end.to_string() == inf
    {
        constraints.push(intervals.numeric[0].start.clone());
        has_numeric_match_all = true;
    } else {
        let mut unequal: Vec<Constraint> = Vec::new();
        let count = intervals.numeric.len();
        for i in 0..count {
            let interval = &intervals.numeric[i];
            if op_of(&interval.end) == Op::Lt && i + 1 < count {
                let next = &intervals.numeric[i + 1];
                if version_of(&interval.end) == version_of(&next.start)
                    && op_of(&next.start) == Op::Gt
                {
                    if unequal.is_empty() && interval.start.to_string() != zero {
                        unequal.push(interval.start.clone());
                    }
                    unequal.push(Constraint::new(Op::Ne, version_of(&interval.end)));
                    continue;
                }
            }
            if !unequal.is_empty() {
                if interval.end.to_string() != inf {
                    unequal.push(interval.end.clone());
                }
                if unequal.len() > 1 {
                    constraints.push(Constraint::Multi {
                        constraints: std::mem::take(&mut unequal),
                        conjunctive: true,
                    });
                } else {
                    constraints.push(unequal.remove(0));
                }
                continue;
            }
            if version_of(&interval.start) == version_of(&interval.end)
                && op_of(&interval.start) == Op::Ge
                && op_of(&interval.end) == Op::Le
            {
                constraints.push(Constraint::new(Op::Eq, version_of(&interval.start)));
                continue;
            }
            if interval.start.to_string() == zero {
                constraints.push(interval.end.clone());
            } else if interval.end.to_string() == inf {
                constraints.push(interval.start.clone());
            } else {
                constraints.push(Constraint::Multi {
                    constraints: vec![interval.start.clone(), interval.end.clone()],
                    conjunctive: true,
                });
            }
        }
    }

    let mut dev: Vec<Constraint> = Vec::new();
    if intervals.branches.names.is_empty() {
        if intervals.branches.exclude && has_numeric_match_all {
            return Constraint::MatchAll;
        }
    } else {
        for name in &intervals.branches.names {
            dev.push(if intervals.branches.exclude {
                Constraint::new(Op::Ne, name.clone())
            } else {
                Constraint::new(Op::Eq, name.clone())
            });
        }
        if intervals.branches.exclude {
            if constraints.len() > 1 {
                let mut all = vec![Constraint::Multi {
                    constraints,
                    conjunctive: false,
                }];
                all.extend(dev);
                return Constraint::Multi {
                    constraints: all,
                    conjunctive: true,
                };
            }
            if constraints.len() == 1 && constraints[0].to_string() == zero {
                if dev.len() > 1 {
                    return Constraint::Multi {
                        constraints: dev,
                        conjunctive: true,
                    };
                }
                return dev.remove(0);
            }
            constraints.extend(dev);
            return Constraint::Multi {
                constraints,
                conjunctive: true,
            };
        }
        constraints.extend(dev);
    }
    if constraints.len() > 1 {
        return Constraint::Multi {
            constraints,
            conjunctive: false,
        };
    }
    if constraints.len() == 1 {
        return constraints.remove(0);
    }
    Constraint::MatchNone
}

/// `Intervals::get` / `generateIntervals`.
pub fn generate(constraint: &Constraint, stop_on_first_valid: bool) -> Intervals {
    // `Intervals::$intervalsCache` (par forme textuelle chez Composer) :
    // ici par valeur structurelle, même résultat.
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<(Constraint, bool), Intervals>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let key = (constraint.clone(), stop_on_first_valid);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let out = generate_uncached(constraint, stop_on_first_valid);
    CACHE.with(|c| {
        c.borrow_mut().insert(key, out.clone());
    });
    out
}

fn generate_uncached(constraint: &Constraint, stop_on_first_valid: bool) -> Intervals {
    match constraint {
        Constraint::MatchAll => Intervals {
            numeric: vec![Interval {
                start: from_zero(),
                end: until_positive_infinity(),
            }],
            branches: any_dev(),
        },
        Constraint::MatchNone => Intervals {
            numeric: Vec::new(),
            branches: no_dev(),
        },
        Constraint::Single { .. } => single(constraint),
        Constraint::Multi {
            constraints,
            conjunctive,
        } => multi(constraints, *conjunctive, stop_on_first_valid),
    }
}

/// `array_diff($a, $b)` / `array_intersect` / `array_merge` sur des listes
/// de chaînes (ordre de `$a` conservé, doublons conservés).
fn diff(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|x| !b.contains(x)).cloned().collect()
}
fn intersect(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|x| b.contains(x)).cloned().collect()
}
fn unique(a: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in a {
        if !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

fn multi(constraints: &[Constraint], conjunctive: bool, stop_on_first_valid: bool) -> Intervals {
    let mut numeric_groups: Vec<Vec<Interval>> = Vec::new();
    let mut constraint_branches: Vec<Branches> = Vec::new();
    for c in constraints {
        let res = generate(c, false);
        numeric_groups.push(res.numeric);
        constraint_branches.push(res.branches);
    }
    let mut branches;
    if !conjunctive {
        branches = no_dev();
        for b in &constraint_branches {
            if b.exclude {
                if branches.exclude {
                    branches.names = intersect(&branches.names, &b.names);
                } else {
                    branches.exclude = true;
                    branches.names = diff(&b.names, &branches.names);
                }
            } else if branches.exclude {
                branches.names = diff(&branches.names, &b.names);
            } else {
                branches.names.extend(b.names.iter().cloned());
            }
        }
    } else {
        branches = any_dev();
        for b in &constraint_branches {
            if b.exclude {
                if branches.exclude {
                    branches.names.extend(b.names.iter().cloned());
                } else {
                    branches.names = diff(&branches.names, &b.names);
                }
            } else if branches.exclude {
                branches.names = diff(&b.names, &branches.names);
                branches.exclude = false;
            } else {
                branches.names = intersect(&branches.names, &b.names);
            }
        }
    }
    branches.names = unique(branches.names);

    if numeric_groups.len() == 1 {
        return Intervals {
            numeric: numeric_groups.remove(0),
            branches,
        };
    }

    struct Border {
        version: String,
        op: Op,
        start: bool,
    }
    let mut borders: Vec<Border> = Vec::new();
    for group in &numeric_groups {
        for interval in group {
            borders.push(Border {
                version: version_of(&interval.start).to_owned(),
                op: op_of(&interval.start),
                start: true,
            });
            borders.push(Border {
                version: version_of(&interval.end).to_owned(),
                op: op_of(&interval.end),
                start: false,
            });
        }
    }
    let sort_order = |op: Op| -> i32 {
        match op {
            Op::Ge => -3,
            Op::Lt => -2,
            Op::Gt => 2,
            Op::Le => 3,
            _ => 0,
        }
    };
    // usort : tri stable sur (version_compare, ordre des opérateurs).
    borders.sort_by(|a, b| {
        let order = version_compare(&a.version, &b.version);
        if order == Ordering::Equal {
            sort_order(a.op).cmp(&sort_order(b.op))
        } else {
            order
        }
    });

    let mut active = 0i32;
    let mut intervals: Vec<Interval> = Vec::new();
    let threshold = if conjunctive {
        numeric_groups.len() as i32
    } else {
        1
    };
    let mut start: Option<Constraint> = None;
    for border in &borders {
        if border.start {
            active += 1;
        } else {
            active -= 1;
        }
        if start.is_none() && active >= threshold {
            start = Some(Constraint::new(border.op, border.version.clone()));
        } else if let Some(s) = &start {
            if active < threshold {
                let empty = version_compare(version_of(s), &border.version) == Ordering::Equal
                    && ((op_of(s) == Op::Gt && border.op == Op::Le)
                        || (op_of(s) == Op::Ge && border.op == Op::Lt));
                if !empty {
                    intervals.push(Interval {
                        start: s.clone(),
                        end: Constraint::new(border.op, border.version.clone()),
                    });
                    if stop_on_first_valid {
                        break;
                    }
                }
                start = None;
            }
        }
    }
    Intervals {
        numeric: intervals,
        branches,
    }
}

fn single(constraint: &Constraint) -> Intervals {
    let op = op_of(constraint);
    let version = version_of(constraint);
    if version.starts_with("dev-") {
        let mut numeric = Vec::new();
        let mut branches = no_dev();
        if op == Op::Ne {
            numeric.push(Interval {
                start: from_zero(),
                end: until_positive_infinity(),
            });
            branches = Branches {
                names: vec![version.to_owned()],
                exclude: true,
            };
        } else if op == Op::Eq {
            branches.names.push(version.to_owned());
        }
        return Intervals { numeric, branches };
    }
    match op {
        Op::Gt | Op::Ge => Intervals {
            numeric: vec![Interval {
                start: constraint.clone(),
                end: until_positive_infinity(),
            }],
            branches: no_dev(),
        },
        Op::Lt | Op::Le => Intervals {
            numeric: vec![Interval {
                start: from_zero(),
                end: constraint.clone(),
            }],
            branches: no_dev(),
        },
        Op::Ne => Intervals {
            numeric: vec![
                Interval {
                    start: from_zero(),
                    end: Constraint::new(Op::Lt, version),
                },
                Interval {
                    start: Constraint::new(Op::Gt, version),
                    end: until_positive_infinity(),
                },
            ],
            branches: any_dev(),
        },
        Op::Eq => Intervals {
            numeric: vec![Interval {
                start: Constraint::new(Op::Ge, version),
                end: Constraint::new(Op::Le, version),
            }],
            branches: no_dev(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::parse_constraints;

    fn c(s: &str) -> Constraint {
        parse_constraints(s).unwrap().constraint
    }

    #[test]
    fn subsets_and_compaction() {
        assert!(is_subset_of(&c("^1.2"), &c("^1.0")));
        assert!(!is_subset_of(&c("^1.0"), &c("^1.2")));
        assert!(is_subset_of(&c("1.5.0"), &c("^1.0")));
        assert!(!is_subset_of(&c("dev-main"), &c("^1.0")));
        assert!(is_subset_of(&c("dev-main"), &c("*")));
        assert!(have_intersections(&c("^1.0"), &c(">=1.5")));
        assert!(!have_intersections(&c("^1.0"), &c("^2.0")));
        let merged = compact_constraint(&Constraint::Multi {
            constraints: vec![c("^1.0"), c("^1.5")],
            conjunctive: false,
        });
        assert_eq!(merged.to_string(), "[>= 1.0.0.0-dev < 2.0.0.0-dev]");
        let both = compact_constraint(&Constraint::Multi {
            constraints: vec![c("^1.0"), c("^2.0")],
            conjunctive: false,
        });
        assert_eq!(both.to_string(), "[>= 1.0.0.0-dev < 3.0.0.0-dev]");
        let ne = compact_constraint(&Constraint::Multi {
            constraints: vec![c(">=1.0"), c("!=1.5.0")],
            conjunctive: true,
        });
        assert_eq!(ne.to_string(), "[>= 1.0.0.0-dev != 1.5.0.0]");
    }
}
