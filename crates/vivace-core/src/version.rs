//! Sous-ensemble du versioning Composer nécessaire au platform-check :
//! versions numériques `X[.Y[.Z[.W]]]` avec suffixe de stabilité optionnel
//! (`-dev`, `-alpha.N`, `-beta.N`, `-RC.N`, `-patch.N`), comparées comme
//! `composer/semver` (normalisation 4 composantes, dev < alpha < beta < RC <
//! stable < patch). Les branches (`dev-master`, `1.x-dev`) sont hors de ce
//! sous-ensemble : `parse` renvoie une erreur et l'appelant traite le paquet
//! comme hors-scope plutôt que de deviner.
//!
//! La parité est tenue par les tests différentiels contre
//! `Composer\Semver\Semver::satisfies` (tests/oracle_semver.rs).

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stability {
    Dev,
    Alpha,
    Beta,
    Rc,
    Stable,
    Patch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub parts: [u64; 4],
    pub stability: Stability,
    /// Numéro du pré-release (`-beta2` → 2), 0 si absent.
    pub pre_number: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("version hors du sous-ensemble supporté: {0:?}")]
pub struct UnsupportedVersion(pub String);

impl Version {
    pub fn parse(input: &str) -> Result<Self, UnsupportedVersion> {
        let s = input.trim();
        let s = s
            .strip_prefix('v')
            .or_else(|| s.strip_prefix('V'))
            .unwrap_or(s);
        if s.is_empty() {
            return Err(UnsupportedVersion(input.to_owned()));
        }

        // Sépare suffixe de stabilité : `1.2.3-beta2`, `1.2.3beta2`, `1.2.3-dev`.
        let (num, suffix) = split_stability(s);
        let (stability, pre_number) = parse_stability(suffix, input)?;

        let mut parts = [0u64; 4];
        let mut n = 0usize;
        for piece in num.split('.') {
            if n >= 4 || piece.is_empty() || !piece.bytes().all(|b| b.is_ascii_digit()) {
                return Err(UnsupportedVersion(input.to_owned()));
            }
            parts[n] = piece
                .parse()
                .map_err(|_| UnsupportedVersion(input.to_owned()))?;
            n += 1;
        }
        if n == 0 {
            return Err(UnsupportedVersion(input.to_owned()));
        }
        Ok(Version {
            parts,
            stability,
            pre_number,
        })
    }
}

/// Coupe `1.2.3-beta2` / `1.2.3beta2` / `1.2.3_RC1` en (numérique, suffixe).
fn split_stability(s: &str) -> (&str, &str) {
    match s.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
        Some(i) => {
            let suffix = &s[i..];
            (&s[..i], suffix.trim_start_matches(['-', '_', '.']))
        }
        None => (s, ""),
    }
}

fn parse_stability(suffix: &str, original: &str) -> Result<(Stability, u64), UnsupportedVersion> {
    if suffix.is_empty() {
        return Ok((Stability::Stable, 0));
    }
    let lower = suffix.to_ascii_lowercase();
    let (word, digits) = match lower.find(|c: char| c.is_ascii_digit()) {
        Some(i) => (&lower[..i], &lower[i..]),
        None => (lower.as_str(), ""),
    };
    let word = word.trim_end_matches(['-', '_', '.']);
    let stability = match word {
        "dev" => Stability::Dev,
        "alpha" | "a" => Stability::Alpha,
        "beta" | "b" => Stability::Beta,
        "rc" => Stability::Rc,
        "patch" | "pl" | "p" => Stability::Patch,
        "stable" | "" => Stability::Stable,
        _ => return Err(UnsupportedVersion(original.to_owned())),
    };
    let pre_number = if digits.is_empty() {
        0
    } else {
        digits
            .parse()
            .map_err(|_| UnsupportedVersion(original.to_owned()))?
    };
    Ok((stability, pre_number))
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parts
            .cmp(&other.parts)
            .then(self.stability.cmp(&other.stability))
            .then(self.pre_number.cmp(&other.pre_number))
    }
}

/// Normalisation « pretty → normalized » de Composer (VersionParser::normalize),
/// pour le sous-ensemble rencontré dans les locks : versions numériques
/// (→ 4 composantes + suffixe canonique), branches `dev-*` (inchangées) et
/// branches numériques `N.x-dev` (x → 9999999, complété à 4 composantes).
/// Parité tenue par tests/oracle_normalize.rs.
pub fn normalize_pretty(input: &str) -> Result<String, UnsupportedVersion> {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix("dev-") {
        if rest.is_empty() {
            return Err(UnsupportedVersion(input.to_owned()));
        }
        return Ok(format!("dev-{rest}"));
    }
    let stripped = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);

    // Branche numérique `1.2.x-dev` / `1.x-dev`.
    if let Some(stem) = stripped
        .strip_suffix(".x-dev")
        .or_else(|| stripped.strip_suffix(".X-dev"))
    {
        let mut parts: Vec<u64> = Vec::new();
        for piece in stem.split('.') {
            if piece.is_empty() || !piece.bytes().all(|b| b.is_ascii_digit()) || parts.len() >= 3 {
                return Err(UnsupportedVersion(input.to_owned()));
            }
            parts.push(
                piece
                    .parse()
                    .map_err(|_| UnsupportedVersion(input.to_owned()))?,
            );
        }
        let mut out: Vec<String> = parts.iter().map(u64::to_string).collect();
        while out.len() < 4 {
            out.push("9999999".to_owned());
        }
        return Ok(format!("{}-dev", out.join(".")));
    }

    let (num, suffix) = split_stability(stripped);
    let (stability, pre_number) = parse_stability(suffix, input)?;
    let mut count = 0usize;
    let mut parts = [0u64; 4];
    for piece in num.split('.') {
        if count >= 4 || piece.is_empty() || !piece.bytes().all(|b| b.is_ascii_digit()) {
            return Err(UnsupportedVersion(input.to_owned()));
        }
        parts[count] = piece
            .parse()
            .map_err(|_| UnsupportedVersion(input.to_owned()))?;
        count += 1;
    }
    if count == 0 {
        return Err(UnsupportedVersion(input.to_owned()));
    }
    let base = format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]);
    let word = match stability {
        Stability::Stable => return Ok(base),
        Stability::Dev => "dev",
        Stability::Alpha => "alpha",
        Stability::Beta => "beta",
        Stability::Rc => "RC",
        Stability::Patch => "patch",
    };
    // Les suffixes sans numéro restent nus (`-alpha`), sinon numéro accolé.
    let had_number = suffix.chars().any(|c| c.is_ascii_digit());
    if had_number {
        Ok(format!("{base}-{word}{pre_number}"))
    } else {
        Ok(format!("{base}-{word}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).expect(s)
    }

    #[test]
    fn parses_and_orders() {
        assert_eq!(v("8.5.10").parts, [8, 5, 10, 0]);
        assert_eq!(v("v1.2").parts, [1, 2, 0, 0]);
        assert!(v("8.1") < v("8.1.1"));
        assert!(v("7.4.33") < v("8.0.0"));
        assert!(v("1.0.0-dev") < v("1.0.0-alpha1"));
        assert!(v("1.0.0-alpha2") < v("1.0.0-beta1"));
        assert!(v("1.0.0-RC1") < v("1.0.0"));
        assert!(v("1.0.0") < v("1.0.0-patch1"));
        assert!(v("1.0.0-beta1") < v("1.0.0-beta2"));
        assert_eq!(v("1.0"), v("1.0.0.0"));
    }

    #[test]
    fn rejects_out_of_subset() {
        for s in ["dev-master", "1.x-dev", "abc", "", "1.2.3.4.5", "1.2-foo"] {
            assert!(Version::parse(s).is_err(), "{s} aurait dû être rejetée");
        }
    }
}
