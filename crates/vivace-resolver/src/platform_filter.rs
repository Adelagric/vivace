//! Port de `Composer\Filter\PlatformRequirementFilter\*` : ce que
//! `--ignore-platform-reqs` / `--ignore-platform-req` retirent des règles.

use crate::constraint::{Constraint, Op};
use crate::intervals;
use crate::platform::is_platform_package;
use crate::version::preg_quote;
use pcre2::bytes::Regex;

pub enum PlatformRequirementFilter {
    IgnoreNothing,
    IgnoreAll,
    IgnoreList {
        ignore: Regex,
        ignore_upper_bound: Regex,
    },
}

/// `BasePackage::packageNamesToRegexp` (`{^(?:a|b)$}iD`).
fn package_names_regexp(names: &[String]) -> Regex {
    let parts: Vec<String> = names
        .iter()
        .map(|n| preg_quote(n).replace("\\*", ".*"))
        .collect();
    pcre2::bytes::RegexBuilder::new()
        .caseless(true)
        .build(&format!("^(?:{})\\z", parts.join("|")))
        .unwrap_or_else(|e| panic!("package names regexp: {e}"))
}

impl PlatformRequirementFilter {
    /// `PlatformRequirementFilterFactory::fromBoolOrList`.
    pub fn from_list(reqs: &[String]) -> PlatformRequirementFilter {
        let mut ignore_all = Vec::new();
        let mut ignore_upper = Vec::new();
        for req in reqs {
            match req.strip_suffix('+') {
                Some(base) => ignore_upper.push(base.to_owned()),
                None => ignore_all.push(req.clone()),
            }
        }
        PlatformRequirementFilter::IgnoreList {
            ignore: package_names_regexp(&ignore_all),
            ignore_upper_bound: package_names_regexp(&ignore_upper),
        }
    }

    pub fn is_ignored(&self, req: &str) -> bool {
        match self {
            PlatformRequirementFilter::IgnoreNothing => false,
            PlatformRequirementFilter::IgnoreAll => is_platform_package(req),
            PlatformRequirementFilter::IgnoreList { ignore, .. } => {
                is_platform_package(req) && ignore.is_match(req.as_bytes()).unwrap_or(false)
            }
        }
    }

    pub fn is_upper_bound_ignored(&self, req: &str) -> bool {
        match self {
            PlatformRequirementFilter::IgnoreList {
                ignore_upper_bound, ..
            } => {
                is_platform_package(req)
                    && (self.is_ignored(req)
                        || ignore_upper_bound.is_match(req.as_bytes()).unwrap_or(false))
            }
            _ => self.is_ignored(req),
        }
    }

    /// `IgnoreListPlatformRequirementFilter::filterConstraint` ; identité
    /// pour les deux autres filtres (qui n'ont pas cette méthode).
    pub fn filter_constraint(
        &self,
        req: &str,
        constraint: &Constraint,
        allow_upper_bound_override: bool,
    ) -> Constraint {
        let PlatformRequirementFilter::IgnoreList {
            ignore,
            ignore_upper_bound,
        } = self
        else {
            return constraint.clone();
        };
        if !is_platform_package(req) {
            return constraint.clone();
        }
        if !allow_upper_bound_override
            || !ignore_upper_bound.is_match(req.as_bytes()).unwrap_or(false)
        {
            return constraint.clone();
        }
        if ignore.is_match(req.as_bytes()).unwrap_or(false) {
            return Constraint::MatchAll;
        }
        let ivs = intervals::generate(constraint, false);
        if let Some(last) = ivs.numeric.last() {
            if last.end.to_string() != intervals::until_positive_infinity().to_string() {
                let end_version = match &last.end {
                    Constraint::Single { version, .. } => version.clone(),
                    _ => String::new(),
                };
                return Constraint::Multi {
                    constraints: vec![constraint.clone(), Constraint::new(Op::Ge, end_version)],
                    conjunctive: false,
                };
            }
        }
        constraint.clone()
    }
}
