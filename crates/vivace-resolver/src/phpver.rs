//! Port exact de `version_compare()` de PHP (ext/standard/versioning.c) :
//! canonicalisation (`-`, `_`, `+` → `.`, un `.` inséré à chaque transition
//! chiffre/non-chiffre), puis comparaison composant par composant — les
//! formes spéciales sont ordonnées `dev < alpha = a < beta = b < RC = rc <
//! # < pl = p`, un nombre valant `#`. C'est l'ordre sur lequel reposent
//! `Constraint::versionCompare`, les bornes et le tri des versions.

use std::cmp::Ordering;

fn is_special(c: u8) -> bool {
    matches!(c, b'-' | b'_' | b'+')
}

/// `php_canonicalize_version` : le premier caractère est copié tel quel,
/// puis `-`/`_`/`+` et tout non-alphanumérique deviennent `.` (sans
/// doublon), et un `.` est inséré à chaque transition chiffre ↔ non-chiffre.
pub fn canonicalize(version: &str) -> String {
    let bytes = version.as_bytes();
    let Some((&first, rest)) = bytes.split_first() else {
        return String::new();
    };
    let is_dig = |c: u8| c.is_ascii_digit();
    let is_ndig = |c: u8| !c.is_ascii_digit() && c != b'.';
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    out.push(first);
    let mut lp = first;
    for &c in rest {
        if is_special(c) {
            if out.last() != Some(&b'.') {
                out.push(b'.');
            }
        } else if (is_ndig(lp) && is_dig(c)) || (is_dig(lp) && is_ndig(c)) {
            if out.last() != Some(&b'.') {
                out.push(b'.');
            }
            out.push(c);
        } else if !c.is_ascii_alphanumeric() {
            if out.last() != Some(&b'.') {
                out.push(b'.');
            }
        } else {
            out.push(c);
        }
        lp = c;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `compare_special_version_forms` : rang par préfixe, -1 si inconnu.
fn special_rank(form: &str) -> i32 {
    const FORMS: &[(&str, i32)] = &[
        ("dev", 0),
        ("alpha", 1),
        ("a", 1),
        ("beta", 2),
        ("b", 2),
        ("RC", 3),
        ("rc", 3),
        ("#", 4),
        ("pl", 5),
        ("p", 5),
    ];
    for (name, order) in FORMS {
        if form.starts_with(name) {
            return *order;
        }
    }
    -1
}

fn compare_special(a: &str, b: &str) -> Ordering {
    special_rank(a).cmp(&special_rank(b))
}

fn parse_num(s: &str) -> i64 {
    // strtol : préfixe numérique, 0 sinon.
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

fn starts_digit(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

/// `php_version_compare($a, $b)` sur des chaînes déjà canonicalisées.
fn compare_canonical(a: &str, b: &str) -> Ordering {
    if a.is_empty() || b.is_empty() {
        return match (a.is_empty(), b.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => Ordering::Equal,
        };
    }
    let mut pa = a.split('.').peekable();
    let mut pb = b.split('.').peekable();
    let mut result = Ordering::Equal;
    loop {
        match (pa.next(), pb.next()) {
            (Some(x), Some(y)) => {
                result = match (starts_digit(x), starts_digit(y)) {
                    (true, true) => parse_num(x).cmp(&parse_num(y)),
                    (false, false) => compare_special(x, y),
                    (true, false) => compare_special("#N#", y),
                    (false, true) => compare_special(x, "#N#"),
                };
                if result != Ordering::Equal {
                    return result;
                }
            }
            (Some(x), None) => {
                return if starts_digit(x) {
                    Ordering::Greater
                } else {
                    compare_canonical(x, "#N#")
                };
            }
            (None, Some(y)) => {
                return if starts_digit(y) {
                    Ordering::Less
                } else {
                    compare_canonical("#N#", y)
                };
            }
            (None, None) => return result,
        }
    }
}

/// `version_compare($a, $b)` : -1 / 0 / 1.
pub fn version_compare(a: &str, b: &str) -> Ordering {
    compare_canonical(&canonicalize(a), &canonicalize(b))
}

/// `version_compare($a, $b, $op)`.
pub fn version_compare_op(a: &str, b: &str, op: &str) -> bool {
    let c = version_compare(a, b);
    match op {
        "<" | "lt" => c == Ordering::Less,
        "<=" | "le" => c != Ordering::Greater,
        ">" | "gt" => c == Ordering::Greater,
        ">=" | "ge" => c != Ordering::Less,
        "==" | "=" | "eq" => c == Ordering::Equal,
        "!=" | "<>" | "ne" => c != Ordering::Equal,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_forms() {
        assert_eq!(canonicalize("1.0.0-dev"), "1.0.0.dev");
        assert_eq!(canonicalize("1.0RC1"), "1.0.RC.1");
        assert_eq!(canonicalize("1.0.0.0-beta2"), "1.0.0.0.beta.2");
        assert_eq!(canonicalize("9999999-dev"), "9999999.dev");
        assert_eq!(canonicalize("dev-main"), "dev.main");
    }

    #[test]
    fn known_orderings() {
        assert_eq!(version_compare("1.0.0.0-dev", "1.0.0.0"), Ordering::Less);
        assert_eq!(
            version_compare("1.0.0.0-alpha1", "1.0.0.0-beta1"),
            Ordering::Less
        );
        assert_eq!(version_compare("1.0.0.0-RC1", "1.0.0.0"), Ordering::Less);
        assert_eq!(version_compare("1.0.0.0", "1.0.0.0-patch1"), Ordering::Less);
        assert_eq!(version_compare("1.0.0.0", "1.0.0"), Ordering::Greater);
        assert_eq!(version_compare("1.0.0", "1.0.0.0"), Ordering::Less);
        assert_eq!(version_compare("2.0.0.0", "10.0.0.0"), Ordering::Less);
        assert_eq!(
            version_compare("1.9999999.9999999.9999999-dev", "2.0.0.0-dev"),
            Ordering::Less
        );
        assert_eq!(
            version_compare("1.0.0.0-dev", "1.0.0.0-alpha"),
            Ordering::Less
        );
        assert_eq!(
            version_compare("1.0.0.0-b", "1.0.0.0-beta1"),
            Ordering::Less
        );
    }
}
