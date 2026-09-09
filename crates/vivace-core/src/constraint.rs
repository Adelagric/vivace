//! Sous-ensemble des contraintes composer/semver pour le platform-check.
//! Port de `VersionParser::parseConstraint(s)` (source épinglé :
//! docs/reference/SemverVersionParser.php, extrait du phar 2.10.3).
//!
//! Règles clés reproduites :
//! - `*` / `x.*` : intervalle `[X…-dev, X+1…-dev)`, `*` seul matche tout ;
//! - `^X.Y.Z` / `~X.Y.Z` : borne basse `>= version-dev` (si pas de suffixe de
//!   stabilité explicite), borne haute exclusive `< next-dev` ;
//! - `>=V` et `<V` sans suffixe : la borne devient `V-dev` (`>=8.1` accepte
//!   `8.1.0-beta1`, `<2.0` refuse `2.0.0-beta`) ; `>V` et `<=V` restent sur la
//!   version telle quelle ;
//! - `A - B` : `>= A-dev` ; `<= B` si B a un patch/suffixe, sinon `< next(B)-dev` ;
//! - OR sur `||` ou `|`, AND sur virgules/espaces.
//!
//! Hors sous-ensemble (branches `dev-*`, alias `as`, `@stability`…) :
//! `Unsupported`, l'appelant traite le cas en hors-scope.

use crate::version::{Stability, Version};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("contrainte hors du sous-ensemble supporté: {0:?}")]
pub struct UnsupportedConstraint(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Simple {
    Any,
    Cmp(Op, Version),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Groupes OR de conjonctions de contraintes simples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    groups: Vec<Vec<Simple>>,
}

impl Constraint {
    pub fn parse(input: &str) -> Result<Self, UnsupportedConstraint> {
        let mut groups = Vec::new();
        for or_part in split_or(input) {
            let or_part = or_part.trim();
            if or_part.is_empty() {
                return Err(UnsupportedConstraint(input.to_owned()));
            }
            groups.push(parse_and_group(or_part, input)?);
        }
        if groups.is_empty() {
            return Err(UnsupportedConstraint(input.to_owned()));
        }
        Ok(Constraint { groups })
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.groups.iter().any(|group| {
            group.iter().all(|c| match c {
                Simple::Any => true,
                Simple::Cmp(op, bound) => match op {
                    Op::Eq => v == bound,
                    Op::Ne => v != bound,
                    Op::Lt => v < bound,
                    Op::Le => v <= bound,
                    Op::Gt => v > bound,
                    Op::Ge => v >= bound,
                },
            })
        })
    }
}

fn split_or(s: &str) -> Vec<&str> {
    // `||` d'abord, puis `|` simple (les deux sont acceptés par Composer).
    if s.contains("||") {
        s.split("||").collect()
    } else if s.contains('|') {
        s.split('|').collect()
    } else {
        vec![s]
    }
}

fn parse_and_group(part: &str, original: &str) -> Result<Vec<Simple>, UnsupportedConstraint> {
    // Tokenise sur virgules/espaces puis recolle « op version » et « A - B ».
    let raw: Vec<&str> = part
        .split([',', ' ', '\t'])
        .filter(|t| !t.is_empty())
        .collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let t = raw[i];
        if matches!(t, ">=" | ">" | "<=" | "<" | "==" | "=" | "!=" | "<>") && i + 1 < raw.len() {
            tokens.push(format!("{}{}", t, raw[i + 1]));
            i += 2;
        } else if t == "-" && !tokens.is_empty() && i + 1 < raw.len() {
            let from = tokens.pop().unwrap_or_default();
            tokens.push(format!("{from} - {}", raw[i + 1]));
            i += 2;
        } else {
            tokens.push(t.to_owned());
            i += 1;
        }
    }

    let mut out = Vec::new();
    for token in tokens {
        parse_simple(&token, original, &mut out)?;
    }
    if out.is_empty() {
        return Err(UnsupportedConstraint(original.to_owned()));
    }
    Ok(out)
}

/// La version a-t-elle un suffixe de stabilité explicite (`-beta1`, `-dev`…) ?
fn has_stability_suffix(s: &str) -> bool {
    s.chars()
        .any(|c| !(c.is_ascii_digit() || c == '.' || c == 'v' || c == 'V'))
}

fn parse_version(s: &str, original: &str) -> Result<Version, UnsupportedConstraint> {
    Version::parse(s).map_err(|_| UnsupportedConstraint(original.to_owned()))
}

/// Décale d'un cran la composante `position` (1-based) et zéroe la suite —
/// l'équivalent de `manipulateVersionString(matches, position, 1)`.
fn bump(v: &Version, position: usize) -> Version {
    let mut parts = v.parts;
    parts[position - 1] += 1;
    for p in parts.iter_mut().skip(position) {
        *p = 0;
    }
    Version {
        parts,
        stability: Stability::Dev,
        pre_number: 0,
    }
}

fn as_dev_floor(mut v: Version) -> Version {
    v.stability = Stability::Dev;
    v.pre_number = 0;
    v
}

fn parse_simple(
    token: &str,
    original: &str,
    out: &mut Vec<Simple>,
) -> Result<(), UnsupportedConstraint> {
    let t = token.trim();

    // Hyphen range « A - B ».
    if let Some((from, to)) = t.split_once(" - ") {
        let low = parse_version(from, original)?;
        let low = if has_stability_suffix(from) {
            low
        } else {
            as_dev_floor(low)
        };
        out.push(Simple::Cmp(Op::Ge, low));

        let to_trim = to.trim();
        let dotted = to_trim.trim_start_matches(['v', 'V']);
        let numeric_parts = dotted.split('.').count();
        let high = parse_version(to_trim, original)?;
        if has_stability_suffix(to_trim) || numeric_parts >= 3 {
            out.push(Simple::Cmp(Op::Le, high));
        } else {
            out.push(Simple::Cmp(Op::Lt, bump(&high, numeric_parts)));
        }
        return Ok(());
    }

    // Wildcards purs.
    if t.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '.' | 'v')) && t.contains(['*', 'x', 'X']) {
        out.push(Simple::Any);
        return Ok(());
    }

    // X-range « 1.2.* ».
    if let Some(stem) = t.strip_suffix(".*").or_else(|| t.strip_suffix(".x")) {
        let base = parse_version(stem, original)?;
        if has_stability_suffix(stem) {
            return Err(UnsupportedConstraint(original.to_owned()));
        }
        let position = stem.trim_start_matches(['v', 'V']).split('.').count();
        if base.parts == [0, 0, 0, 0] {
            out.push(Simple::Cmp(Op::Lt, bump(&base, position)));
        } else {
            out.push(Simple::Cmp(Op::Ge, as_dev_floor(base.clone())));
            out.push(Simple::Cmp(Op::Lt, bump(&base, position)));
        }
        return Ok(());
    }

    // Caret / tilde.
    if let Some(rest) = t.strip_prefix('^') {
        let v = parse_version(rest, original)?;
        let low = if has_stability_suffix(rest) {
            v.clone()
        } else {
            as_dev_floor(v.clone())
        };
        // Position caret : premier composant non nul (0.x → mineur, 0.0.x → patch).
        let stem = rest.split(['-', '+']).next().unwrap_or(rest);
        let given = stem.trim_start_matches(['v', 'V']).split('.').count();
        let position = if v.parts[0] != 0 || given < 2 {
            1
        } else if v.parts[1] != 0 || given < 3 {
            2
        } else {
            3
        };
        out.push(Simple::Cmp(Op::Ge, low));
        out.push(Simple::Cmp(Op::Lt, bump(&v, position)));
        return Ok(());
    }
    if let Some(rest) = t.strip_prefix('~') {
        if rest.starts_with('>') {
            return Err(UnsupportedConstraint(original.to_owned()));
        }
        let v = parse_version(rest, original)?;
        let low = if has_stability_suffix(rest) {
            v.clone()
        } else {
            as_dev_floor(v.clone())
        };
        let stem = rest.split(['-', '+']).next().unwrap_or(rest);
        let given = stem.trim_start_matches(['v', 'V']).split('.').count();
        let position = given.clamp(1, 4).max(2) - 1; // max(1, position-1)
        out.push(Simple::Cmp(Op::Ge, low));
        out.push(Simple::Cmp(Op::Lt, bump(&v, position)));
        return Ok(());
    }

    // Opérateurs simples et version exacte.
    let (op, rest) = if let Some(r) = t.strip_prefix(">=") {
        (Op::Ge, r)
    } else if let Some(r) = t.strip_prefix("<=") {
        (Op::Le, r)
    } else if let Some(r) = t.strip_prefix("<>").or_else(|| t.strip_prefix("!=")) {
        (Op::Ne, r)
    } else if let Some(r) = t.strip_prefix('>') {
        (Op::Gt, r)
    } else if let Some(r) = t.strip_prefix('<') {
        (Op::Lt, r)
    } else if let Some(r) = t.strip_prefix("==").or_else(|| t.strip_prefix('=')) {
        (Op::Eq, r)
    } else {
        (Op::Eq, t)
    };
    let rest = rest.trim();
    let v = parse_version(rest, original)?;
    // `<` et `>=` sans suffixe explicite : borne ramenée au plancher -dev.
    let v = if matches!(op, Op::Lt | Op::Ge) && !has_stability_suffix(rest) {
        as_dev_floor(v)
    } else {
        v
    };
    out.push(Simple::Cmp(op, v));
    Ok(())
}

/// Raccourci : `version` satisfait-elle `constraint` ?
pub fn satisfies(version: &str, constraint: &str) -> Result<bool, UnsupportedConstraint> {
    let v =
        Version::parse(version).map_err(|_| UnsupportedConstraint(format!("version {version}")))?;
    Ok(Constraint::parse(constraint)?.matches(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sat(v: &str, c: &str) -> bool {
        satisfies(v, c).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn operators_and_dev_floors() {
        assert!(sat("8.5.10", ">=8.1"));
        assert!(sat("8.1.0-beta1", ">=8.1")); // >= sans suffixe → plancher -dev
        assert!(!sat("2.0.0-beta1", "<2.0")); // < sans suffixe → plancher -dev
        assert!(sat("1.9.9", "<2.0"));
        assert!(!sat("8.1.0-beta1", ">8.1")); // > reste sur la version stable
        assert!(sat("8.1.1", ">8.1"));
        assert!(sat("2.0.0", "<=2.0"));
        assert!(!sat("2.0.1", "<=2.0"));
        assert!(sat("1.2.3", "1.2.3"));
        assert!(!sat("1.2.3", "!=1.2.3"));
    }

    #[test]
    fn caret_tilde_wildcards() {
        assert!(sat("8.5.10", "^8.1"));
        assert!(!sat("9.0.0-alpha1", "^8.1"));
        assert!(sat("8.1.0-RC1", "^8.1")); // borne basse -dev
        assert!(sat("0.3.7", "^0.3"));
        assert!(!sat("0.4.0", "^0.3"));
        assert!(!sat("0.0.4", "^0.0.3"));
        assert!(sat("1.2.9", "~1.2.3"));
        assert!(!sat("1.3.0", "~1.2.3"));
        assert!(sat("1.9.0", "~1.2"));
        assert!(!sat("2.0.0-alpha1", "~1.2"));
        assert!(sat("123.4.5", "*"));
        assert!(sat("1.2.9-beta1", "1.2.*"));
        assert!(!sat("1.3.0-dev", "1.2.*"));
    }

    #[test]
    fn and_or_and_hyphen() {
        assert!(sat("7.4.33", "^7.4|^8.0"));
        assert!(sat("8.0.2", "^7.4||^8.0"));
        assert!(!sat("8.0.2", "^8.1|^7.4"));
        assert!(sat("1.5.0", ">=1.0 <2.0"));
        assert!(sat("1.5.0", ">=1.0,<2.0"));
        assert!(sat("1.5.0", ">= 1.0 , < 2.0"));
        assert!(sat("2.0.0", "1.0 - 2.0")); // borne haute élargie: < 2.1-dev
        assert!(sat("2.0.9", "1.0 - 2.0"));
        assert!(!sat("2.1.0", "1.0 - 2.0"));
        assert!(sat("2.0.0", "1.0.0 - 2.0.0")); // patch explicite: <= 2.0.0
        assert!(!sat("2.0.1", "1.0.0 - 2.0.0"));
    }

    #[test]
    fn unsupported_forms_error_out() {
        for c in ["dev-master", "1.0 as 2.0", "@dev", "~>1.2"] {
            assert!(satisfies("1.0.0", c).is_err(), "{c} aurait dû être rejetée");
        }
    }
}
