//! Port de `strnatcasecmp` (ext/standard/strnatcmp.c) : comparaison
//! « naturelle » insensible à la casse, utilisée par PackageSorter pour
//! départager les paquets de même poids (`symfony/polyfill-php80` avant
//! `-php82`, ce que strcmp ne garantit pas pour des nombres de longueurs
//! différentes).

use std::cmp::Ordering;

fn compare_right(a: &[u8], b: &[u8]) -> (Ordering, usize, usize) {
    // Le plus long run de chiffres gagne ; à longueur égale, le premier
    // chiffre différent tranche (biais mémorisé).
    let mut bias = Ordering::Equal;
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        let ca = a.get(i).copied().filter(|c| c.is_ascii_digit());
        let cb = b.get(j).copied().filter(|c| c.is_ascii_digit());
        match (ca, cb) {
            (None, None) => return (bias, i, j),
            (None, Some(_)) => return (Ordering::Less, i, j),
            (Some(_), None) => return (Ordering::Greater, i, j),
            (Some(x), Some(y)) => {
                if bias == Ordering::Equal {
                    bias = x.cmp(&y);
                }
            }
        }
        i += 1;
        j += 1;
    }
}

fn compare_left(a: &[u8], b: &[u8]) -> (Ordering, usize, usize) {
    // Fractions (un zéro en tête) : le premier chiffre différent tranche.
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        let ca = a.get(i).copied().filter(|c| c.is_ascii_digit());
        let cb = b.get(j).copied().filter(|c| c.is_ascii_digit());
        match (ca, cb) {
            (None, None) => return (Ordering::Equal, i, j),
            (None, Some(_)) => return (Ordering::Less, i, j),
            (Some(_), None) => return (Ordering::Greater, i, j),
            (Some(x), Some(y)) => {
                if x != y {
                    return (x.cmp(&y), i, j);
                }
            }
        }
        i += 1;
        j += 1;
    }
}

pub fn strnatcasecmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut ai, mut bi) = (0usize, 0usize);
    let mut leading = true;
    loop {
        // Zéros de tête (au tout début seulement).
        while leading && ai + 1 < a.len() && a[ai] == b'0' && a[ai + 1].is_ascii_digit() {
            ai += 1;
        }
        while leading && bi + 1 < b.len() && b[bi] == b'0' && b[bi + 1].is_ascii_digit() {
            bi += 1;
        }
        leading = false;
        // Blancs consécutifs.
        while ai < a.len() && a[ai].is_ascii_whitespace() {
            ai += 1;
        }
        while bi < b.len() && b[bi].is_ascii_whitespace() {
            bi += 1;
        }
        let (ca, cb) = (a.get(ai).copied(), b.get(bi).copied());
        if let (Some(x), Some(y)) = (ca, cb) {
            if x.is_ascii_digit() && y.is_ascii_digit() {
                let fractional = x == b'0' || y == b'0';
                let (res, da, db) = if fractional {
                    compare_left(&a[ai..], &b[bi..])
                } else {
                    compare_right(&a[ai..], &b[bi..])
                };
                if res != Ordering::Equal {
                    return res;
                }
                ai += da;
                bi += db;
                match (ai >= a.len(), bi >= b.len()) {
                    (true, true) => return Ordering::Equal,
                    (true, false) => return Ordering::Less,
                    (false, true) => return Ordering::Greater,
                    _ => continue,
                }
            }
        }
        let fold = |c: Option<u8>| c.map(|c| c.to_ascii_uppercase());
        let (x, y) = (fold(ca), fold(cb));
        match (x, y) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
            }
        }
        ai += 1;
        bi += 1;
        match (ai >= a.len(), bi >= b.len()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_and_case_insensitive() {
        assert_eq!(
            strnatcasecmp("symfony/polyfill-php80", "symfony/polyfill-php82"),
            Ordering::Less
        );
        assert_eq!(strnatcasecmp("a10", "a9"), Ordering::Greater);
        assert_eq!(strnatcasecmp("A1", "a1"), Ordering::Equal);
        assert_eq!(strnatcasecmp("abc", "abcd"), Ordering::Less);
        assert_eq!(strnatcasecmp("x01", "x1"), Ordering::Less);
        assert_eq!(strnatcasecmp("v1.10", "v1.9"), Ordering::Greater);
        assert_eq!(
            strnatcasecmp("psr/log", "psr/http-message"),
            Ordering::Greater
        );
    }
}
