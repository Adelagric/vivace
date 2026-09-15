//! Composer's explanations of an unsolvable set: port of
//! `SolverProblemsException::getPrettyString`, `Problem::getPrettyString`
//! (with `getMissingPackageReason`, `getPackageList`,
//! `formatDeduplicatedRules`…) and `Rule::getPrettyString`
//! (docs/reference/resolver/). The text is Composer's `--no-ansi`
//! rendering: the output styles (`<error>`, `<warning>`, `<info>`,
//! `<comment>`, `<href=…>`) are stripped, any other angle-bracket text is
//! kept verbatim.
//!
//! The repository lookups of the reference (`RepositorySet::findPackages`,
//! `getProviders`) load metadata, hence the mutable arena; the pool must
//! be the one the solver ran on (removed versions, policy removals).

use crate::constraint::{Constraint, Op};
use crate::package::{Origin, Package};
use crate::phpver::version_compare;
use crate::platform::is_platform_package;
use crate::pool::{Pool, Repository, RepositorySet, Request};
use crate::repository::{is_package_acceptable, Advisory};
use crate::rule::{Reason, Rule};
use crate::solver::SolvedProblem;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Everything the messages read.
pub struct MessageContext<'a> {
    pub arena: &'a mut Vec<Package>,
    pub set: &'a RepositorySet,
    pub pool: &'a Pool,
    pub request: &'a Request,
    pub is_verbose: bool,
    /// `IniHelper::getAll()` of the PHP in use.
    pub ini_files: &'a [String],
    /// `ext-*` the PHP in use has loaded (`extension_loaded`).
    pub loaded_extensions: &'a BTreeSet<String>,
}

/// `RepositorySet::ALLOW_UNACCEPTABLE_STABILITIES`.
const ALLOW_UNACCEPTABLE_STABILITIES: u8 = 4;
/// `RepositorySet::ALLOW_SHADOWED_REPOSITORIES`.
const ALLOW_SHADOWED_REPOSITORIES: u8 = 8;

/// A constraint with its pretty string (`getPrettyString`).
#[derive(Clone, Copy)]
pub struct PrettyConstraint<'a> {
    pub constraint: &'a Constraint,
    pub pretty: &'a str,
}

// ---------------------------------------------------------------------------
// SolverProblemsException

/// `SolverProblemsException::getPrettyString(..., $isDevExtraction)`: the
/// numbered problems, deduplicated, then the hints.
pub fn pretty_string(
    ctx: &mut MessageContext<'_>,
    problems: &[SolvedProblem],
    is_dev_extraction: bool,
) -> String {
    let installed_map = present_map(ctx);
    let mut missing_extensions: Vec<String> = Vec::new();
    let mut caused_by_lock = false;
    let mut texts: Vec<String> = Vec::new();
    for problem in problems {
        texts.push(format!(
            "{}\n",
            problem_pretty_string(ctx, problem, &installed_map)
        ));
        // `getExtensionProblems($problem->getReasons())`: sections in their
        // original order.
        for rule in problem.sections.iter().flatten() {
            if let Some(required) = required_package(ctx.arena, rule) {
                if required.starts_with("ext-") && !missing_extensions.contains(&required) {
                    missing_extensions.push(required);
                }
            }
        }
        caused_by_lock = caused_by_lock
            || problem
                .reasons()
                .iter()
                .any(|rule| is_caused_by_lock(ctx, rule));
    }
    let mut text = String::from("\n");
    let mut seen: Vec<&String> = Vec::new();
    let mut i = 1;
    for t in &texts {
        if seen.contains(&t) {
            continue;
        }
        seen.push(t);
        text.push_str(&format!("  Problem {i}{t}"));
        i += 1;
    }
    let mut hints: Vec<String> = Vec::new();
    if !is_dev_extraction
        && (text.contains("could not be found") || text.contains("no matching package found"))
    {
        hints.push("Potential causes:\n - A typo in the package name\n - The package is not available in a stable-enough version according to your minimum-stability setting\n   see <https://getcomposer.org/doc/04-schema.md#minimum-stability> for more details.\n - It's a private package and you forgot to add a custom repository to find it\n\nRead <https://getcomposer.org/doc/articles/troubleshooting.md> for further common problems.".to_owned());
    }
    if !missing_extensions.is_empty() {
        hints.push(extension_hint(ctx.ini_files, &missing_extensions));
    }
    if caused_by_lock
        && !is_dev_extraction
        && !ctx.request.update_allow_transitive_root_dependencies()
    {
        hints.push("Use the option --with-all-dependencies (-W) to allow upgrades, downgrades and removals for packages currently locked to specific versions.".to_owned());
    }
    if text.contains("found composer-plugin-api[2.0.0] but it does not match")
        && text.contains("- ocramius/package-versions")
    {
        hints.push("<warning>ocramius/package-versions only provides support for Composer 2 in 1.8+, which requires PHP 7.4.</warning>\nIf you can not upgrade PHP you can require <info>composer/package-versions-deprecated</info> to resolve this with PHP 7.0+.".to_owned());
    }
    if text.contains("found composer-plugin-api[2.0.0] but it does not match") {
        hints.push("You are using Composer 2, which some of your plugins seem to be incompatible with. Make sure you update your plugins or report a plugin-issue to ask them to support Composer 2.".to_owned());
    }
    if !hints.is_empty() {
        text.push('\n');
        text.push_str(&hints.join("\n\n"));
    }
    strip_output_styles(&text)
}

/// `createExtensionHint`.
fn extension_hint(ini_files: &[String], missing: &[String]) -> String {
    let mut paths: Vec<&String> = ini_files.iter().collect();
    if paths.first().is_some_and(|p| p.is_empty()) {
        if paths.len() == 1 {
            return String::new();
        }
        paths.remove(0);
    }
    let args: Vec<String> = missing
        .iter()
        .map(|e| format!("--ignore-platform-req={e}"))
        .collect();
    let mut text = String::from(
        "To enable extensions, verify that they are enabled in your .ini files:\n    - ",
    );
    text.push_str(
        &paths
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n    - "),
    );
    text.push_str(
        "\nYou can also run `php --ini` in a terminal to see which files are used by PHP in CLI mode.",
    );
    text.push_str(&format!(
        "\nAlternatively, you can run Composer with `{}` to temporarily ignore these required extensions.",
        args.join(" ")
    ));
    text
}

/// `Rule::getRequiredPackage`.
fn required_package(arena: &[Package], rule: &Rule) -> Option<String> {
    match &rule.reason {
        Reason::RootRequire { package_name, .. } => Some(package_name.clone()),
        Reason::PackageRequires(link) => Some(link.target.clone()),
        Reason::Fixed { package } | Reason::LockedFilterListRemoved { package } => {
            Some(arena[*package].name.clone())
        }
        _ => None,
    }
}

/// `Request::getPresentMap(true)`: pool ids of the locked repository's
/// packages and of the fixed packages.
fn present_map(ctx: &MessageContext<'_>) -> HashSet<usize> {
    let mut map = HashSet::new();
    if let Some(locked) = &ctx.request.locked_repository {
        for &idx in locked {
            if let Some(id) = ctx.pool.id_of(idx) {
                map.insert(id);
            }
        }
    }
    for &idx in &ctx.request.fixed_packages {
        if let Some(id) = ctx.pool.id_of(idx) {
            map.insert(id);
        }
    }
    map
}

/// Symfony's `OutputFormatter` without decoration: the known styles and
/// `<href=…>` disappear, everything else (`<https://…>`) is printed as is.
pub fn strip_output_styles(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(rest);
            return out;
        };
        let tag = &after[..end];
        let name = tag.trim_start_matches('/');
        let known = matches!(
            name,
            "error" | "warning" | "info" | "comment" | "question" | "highlight" | ""
        ) || name.starts_with("href=")
            || name.starts_with("fg=")
            || name.starts_with("bg=")
            || name.starts_with("options=");
        if known && !tag.contains('\n') {
            rest = &after[end + 1..];
        } else {
            out.push('<');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Problem

/// `Problem::getPrettyString`.
fn problem_pretty_string(
    ctx: &mut MessageContext<'_>,
    problem: &SolvedProblem,
    installed_map: &HashSet<usize>,
) -> String {
    let reasons: Vec<Rule> = problem.reasons().into_iter().cloned().collect();
    if reasons.len() == 1 {
        let rule = &reasons[0];
        match &rule.reason {
            Reason::RootRequire {
                package_name,
                constraint,
                pretty,
            } if ctx
                .pool
                .what_provides(ctx.arena, package_name, Some(constraint))
                .is_empty() =>
            {
                let (a, b) = missing_package_reason(
                    ctx,
                    package_name,
                    Some(PrettyConstraint { constraint, pretty }),
                );
                return format!("\n    {a}{b}");
            }
            Reason::LockedFilterListRemoved { package } => {
                let (a, b) = missing_locked_package_reason(ctx, *package);
                return format!("\n    {a}{b}");
            }
            _ => {}
        }
    }
    let mut sorted = reasons;
    sorted.sort_by(|r1, r2| {
        let p1 = rule_priority(r1);
        let p2 = rule_priority(r2);
        if p1 != p2 {
            return p2.cmp(&p1);
        }
        php_string_cmp(&sortable_string(ctx, r1), &sortable_string(ctx, r2))
    });
    format_deduplicated_rules(ctx, &sorted, "    ", installed_map)
}

/// `getRulePriority`.
fn rule_priority(rule: &Rule) -> i32 {
    match &rule.reason {
        Reason::Fixed { .. } | Reason::LockedFilterListRemoved { .. } => 3,
        Reason::RootRequire { .. } => 2,
        Reason::PackageConflict(_) | Reason::PackageRequires(_) => 1,
        Reason::PackageSameName(_)
        | Reason::Learned(_)
        | Reason::PackageAlias { .. }
        | Reason::PackageInverseAlias { .. } => 0,
    }
}

/// `getSortableString`.
fn sortable_string(ctx: &MessageContext<'_>, rule: &Rule) -> String {
    match &rule.reason {
        Reason::RootRequire { package_name, .. } => package_name.clone(),
        Reason::Fixed { package } | Reason::LockedFilterListRemoved { package } => {
            package_to_string(ctx, *package)
        }
        Reason::PackageConflict(link) | Reason::PackageRequires(link) => {
            let source = source_package(ctx, rule);
            format!(
                "{}//{}",
                package_to_string(ctx, source),
                link_pretty_string(&ctx.arena[source], link)
            )
        }
        Reason::PackageSameName(name) => name.clone(),
        Reason::PackageAlias { alias } => package_to_string(ctx, *alias),
        Reason::PackageInverseAlias { package } => package_to_string(ctx, *package),
        Reason::Learned(_) => rule
            .literals
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("-"),
    }
}

/// `BasePackage::__toString` (`getUniqueName`), with `AliasPackage`'s
/// ` (alias of <version>)` / ` (root alias of <version>)` suffix.
fn package_to_string(ctx: &MessageContext<'_>, idx: usize) -> String {
    let p = &ctx.arena[idx];
    match p.alias_of {
        Some(base) => format!(
            "{} ({}alias of {})",
            p.unique_name(),
            if p.root_package_alias { "root " } else { "" },
            ctx.arena[base].version
        ),
        None => p.unique_name(),
    }
}

/// PHP 8 `<=>` on strings: two numeric strings compare as numbers.
pub fn php_string_cmp(a: &str, b: &str) -> Ordering {
    match (php_numeric(a), php_numeric(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => a.as_bytes().cmp(b.as_bytes()),
    }
}

/// `is_numeric` (leading whitespace allowed, trailing whitespace allowed
/// since PHP 8), as a float.
fn php_numeric(s: &str) -> Option<f64> {
    let t = s.trim_matches([' ', '\t', '\n', '\r', '\x0B', '\x0C']);
    if t.is_empty() {
        return None;
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (int_part, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match frac {
        Some(f) => {
            (digits(int_part) || int_part.is_empty())
                && (digits(f) || f.is_empty())
                && !(int_part.is_empty() && f.is_empty())
        }
        None => digits(int_part),
    };
    if !mantissa_ok {
        return None;
    }
    if let Some(e) = exponent {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if !digits(e) {
            return None;
        }
    }
    t.parse::<f64>().ok()
}

/// `Rule::getSourcePackage` (conflict or requires rules only).
fn source_package(ctx: &MessageContext<'_>, rule: &Rule) -> usize {
    match &rule.reason {
        Reason::PackageConflict(link) => {
            let p1 = dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(rule.literals[0]));
            let p2 = dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(rule.literals[1]));
            if link.source == ctx.arena[p1].name {
                p1
            } else {
                p2
            }
        }
        _ => dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(rule.literals[0])),
    }
}

/// `deduplicateDefaultBranchAlias`.
fn dedup_default_branch_alias(ctx: &MessageContext<'_>, idx: usize) -> usize {
    let p = &ctx.arena[idx];
    match p.alias_of {
        Some(base) if p.pretty_version == "9999999-dev" => base,
        _ => idx,
    }
}

/// `Link::getPrettyString($sourcePackage)`: the CONSTRAINT's pretty string
/// (`$this->constraint->getPrettyString()`), not `getPrettyConstraint()`.
fn link_pretty_string(source: &Package, link: &crate::package::Link) -> String {
    format!(
        "{} {} {} {}",
        source.pretty_string(),
        link.kind.description(),
        link.target,
        link_constraint_pretty(source, link)
    )
}

/// `$link->getConstraint()->getPrettyString()`: `ArrayLoader::createLink`
/// parses `self.version` as the source's pretty version, so that is the
/// constraint's own pretty string; every other link keeps its text.
fn link_constraint_pretty(source: &Package, link: &crate::package::Link) -> String {
    if link.pretty_constraint == "self.version" {
        source.pretty_version.clone()
    } else {
        link.pretty_constraint.clone()
    }
}

/// Package pretty name -> normalized version -> pretty version, in a
/// deduplication template.
type TemplatePackages = Vec<(String, BTreeMap<String, String>)>;

/// `formatDeduplicatedRules`.
fn format_deduplicated_rules(
    ctx: &mut MessageContext<'_>,
    rules: &[Rule],
    indent: &str,
    installed_map: &HashSet<usize>,
) -> String {
    // template -> package pretty name -> normalized version -> pretty
    let mut messages: Vec<String> = Vec::new();
    let mut templates: Vec<(String, TemplatePackages)> = Vec::new();
    for rule in rules {
        let message = rule_pretty_string(ctx, rule, installed_map);
        let deduplicatable = matches!(
            rule.reason,
            Reason::PackageRequires(_) | Reason::PackageConflict(_)
        );
        let parsed = deduplicatable.then(|| parse_dedup_head(&message)).flatten();
        if let Some((package, version, _)) = parsed {
            let escaped = message.replace('%', "%%");
            let head_len = escaped
                .find(' ')
                .and_then(|i| escaped[i + 1..].find(' ').map(|j| i + 1 + j + 1));
            let template = match head_len {
                Some(n) => format!("%s%s {}", &escaped[n..]),
                None => escaped.clone(),
            };
            messages.push(template.clone());
            let normalized = crate::version::normalize(&version, None).unwrap_or(version.clone());
            let source = source_package(ctx, rule);
            let entry = match templates.iter_mut().find(|(t, _)| *t == template) {
                Some((_, e)) => e,
                None => {
                    templates.push((template.clone(), Vec::new()));
                    &mut templates.last_mut().expect("just pushed").1
                }
            };
            let versions = match entry.iter_mut().find(|(p, _)| *p == package) {
                Some((_, v)) => v,
                None => {
                    entry.push((package.clone(), BTreeMap::new()));
                    &mut entry.last_mut().expect("just pushed").1
                }
            };
            versions.insert(normalized, version);
            for (v, pretty) in ctx.pool.removed_versions_by_package(source) {
                versions.insert(v, pretty);
            }
        } else if !message.is_empty() {
            messages.push(message);
        }
    }
    let mut result: Vec<String> = Vec::new();
    let mut seen: Vec<&String> = Vec::new();
    for message in &messages {
        if seen.contains(&message) {
            continue;
        }
        seen.push(message);
        if let Some((_, packages)) = templates.iter().find(|(t, _)| t == message) {
            for (package, versions) in packages {
                let mut list: Vec<(&String, &String)> = versions.iter().collect();
                list.sort_by(|a, b| version_compare(a.0, b.0));
                let mut pretty: Vec<String> = list.into_iter().map(|(_, p)| p.clone()).collect();
                if !ctx.is_verbose {
                    pretty = condense_version_list(&versions_in_order(versions), 1, 16);
                }
                let rest = message.replacen("%s%s ", "", 1).replace("%%", "%");
                if pretty.len() > 1 {
                    // `^(%s%s (?:require|conflict))s` -> plural verb.
                    let rest = if let Some(r) = rest.strip_prefix("requires") {
                        format!("require{r}")
                    } else if let Some(r) = rest.strip_prefix("conflicts") {
                        format!("conflict{r}")
                    } else {
                        rest
                    };
                    result.push(format!("{package}[{}] {rest}", pretty.join(", ")));
                } else {
                    result.push(format!("{package} {} {rest}", pretty[0]));
                }
            }
        } else {
            result.push(message.clone());
        }
    }
    format!("\n{indent}- {}", result.join(&format!("\n{indent}- ")))
}

/// `{^(?P<package>\S+) (?P<version>\S+) (?P<type>requires|conflicts)}`.
fn parse_dedup_head(message: &str) -> Option<(String, String, String)> {
    let mut parts = message.splitn(3, ' ');
    let package = parts.next()?;
    let version = parts.next()?;
    let rest = parts.next()?;
    if package.is_empty() || version.is_empty() {
        return None;
    }
    let kind = if rest.starts_with("requires") {
        "requires"
    } else if rest.starts_with("conflicts") {
        "conflicts"
    } else {
        return None;
    };
    Some((package.to_owned(), version.to_owned(), kind.to_owned()))
}

/// The versions of a normalized->pretty map in `version_compare` order.
fn versions_in_order(versions: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = versions
        .iter()
        .map(|(v, p)| (v.clone(), p.clone()))
        .collect();
    list.sort_by(|a, b| version_compare(&a.0, &b.0));
    list
}

/// `condenseVersionList($versions, $max, $maxDev)`: `versions` already in
/// `version_compare` order, keyed by normalized version.
fn condense_version_list(versions: &[(String, String)], max: usize, max_dev: usize) -> Vec<String> {
    if versions.len() <= max {
        return versions.iter().map(|(_, p)| p.clone()).collect();
    }
    let mut by_major: Vec<(String, Vec<String>)> = Vec::new();
    for (version, pretty) in versions {
        let major = if version.to_lowercase().starts_with("dev-") {
            "dev".to_owned()
        } else {
            let digits: String = version.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() && version[digits.len()..].starts_with('.') {
                digits
            } else {
                version.clone()
            }
        };
        match by_major.iter_mut().find(|(m, _)| *m == major) {
            Some((_, list)) => list.push(pretty.clone()),
            None => by_major.push((major, vec![pretty.clone()])),
        }
    }
    let mut filtered: Vec<String> = Vec::new();
    for (major, list) in by_major {
        let cap = if major == "dev" { max_dev } else { max };
        if list.len() > cap {
            filtered.push(list[0].clone());
            filtered.push("...".to_owned());
            filtered.push(list[list.len() - 1].clone());
        } else {
            filtered.extend(list);
        }
    }
    filtered
}

/// `Problem::isCausedByLock` for one rule (`Rule::isCausedByLock`).
fn is_caused_by_lock(ctx: &MessageContext<'_>, rule: &Rule) -> bool {
    let (target, constraint) = match &rule.reason {
        Reason::PackageRequires(link) => (link.target.as_str(), &link.constraint),
        Reason::RootRequire {
            package_name,
            constraint,
            ..
        } => (package_name.as_str(), constraint),
        _ => return false,
    };
    if is_platform_package(target) {
        return false;
    }
    let Some(locked) = &ctx.request.locked_repository else {
        return false;
    };
    for &idx in locked {
        let p = &ctx.arena[idx];
        if p.name != target {
            continue;
        }
        if ctx.pool.is_unacceptable_fixed_or_locked(idx) {
            return true;
        }
        if !constraint.matches(&Constraint::new(Op::Eq, p.version.clone())) {
            return true;
        }
        if matches!(rule.reason, Reason::PackageRequires(_)) && !ctx.request.is_locked_package(idx)
        {
            return true;
        }
        break;
    }
    false
}

// ---------------------------------------------------------------------------
// Rule

/// `Rule::getPrettyString`.
fn rule_pretty_string(
    ctx: &mut MessageContext<'_>,
    rule: &Rule,
    installed_map: &HashSet<usize>,
) -> String {
    let literals = &rule.literals;
    match &rule.reason {
        Reason::RootRequire {
            package_name,
            constraint,
            pretty,
        } => {
            let packages = ctx
                .pool
                .what_provides(ctx.arena, package_name, Some(constraint));
            if packages.is_empty() {
                return format!(
                    "No package found to satisfy root composer.json require {package_name} {pretty}"
                );
            }
            let non_alias: Vec<usize> = packages
                .iter()
                .map(|&id| ctx.pool.package_by_id(id))
                .filter(|&idx| ctx.arena[idx].alias_of.is_none())
                .collect();
            if non_alias.len() == 1 && ctx.request.is_locked_package(non_alias[0]) {
                let p = &ctx.arena[non_alias[0]];
                return format!(
                    "{} is locked to version {} and an update of this package was not requested.",
                    p.pretty_name, p.pretty_version
                );
            }
            let idxs: Vec<usize> = packages
                .iter()
                .map(|&id| ctx.pool.package_by_id(id))
                .collect();
            format!(
                "Root composer.json requires {package_name} {pretty} -> satisfiable by {}.",
                package_list(ctx, &idxs, Some(constraint), false)
            )
        }
        Reason::Fixed { package } => {
            let idx = dedup_default_branch_alias(ctx, *package);
            let p = &ctx.arena[idx];
            if ctx.request.is_locked_package(idx) {
                return format!(
                    "{} is locked to version {} and an update of this package was not requested.",
                    p.pretty_name, p.pretty_version
                );
            }
            format!(
                "{} is present at version {} and cannot be modified by Composer",
                p.pretty_name, p.pretty_version
            )
        }
        Reason::LockedFilterListRemoved { package } => {
            let idx = dedup_default_branch_alias(ctx, *package);
            let p = &ctx.arena[idx];
            format!(
                "{} {} was removed by a dependency policy (e.g. malware) and cannot be installed.",
                p.pretty_name, p.pretty_version
            )
        }
        Reason::PackageConflict(link) => {
            let mut package1 =
                dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(literals[0]));
            let mut package2 =
                dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(literals[1]));
            let mut conflict_target = ctx.arena[package1].pretty_string();
            if link.source == ctx.arena[package1].name {
                std::mem::swap(&mut package1, &mut package2);
                conflict_target = format!(
                    "{} {}",
                    ctx.arena[package1].pretty_name, link.pretty_constraint
                );
            }
            if link.target != ctx.arena[package1].name {
                let p1 = &ctx.arena[package1];
                // Both loops run in the reference: a `replaces` link wins
                // over a `provides` one for the same target.
                let mut provided: Option<(&str, &str)> = None;
                for l in p1.provides.iter() {
                    if l.target == link.target {
                        provided = Some(("provides", &l.pretty_constraint));
                        break;
                    }
                }
                for l in p1.replaces.iter() {
                    if l.target == link.target {
                        provided = Some(("replaces", &l.pretty_constraint));
                        break;
                    }
                }
                if let Some((kind, constraint)) = provided {
                    conflict_target = format!(
                        "{} {} ({} {} {} {})",
                        link.target,
                        link.pretty_constraint,
                        p1.pretty_string(),
                        kind,
                        link.target,
                        constraint
                    );
                }
            }
            format!(
                "{} conflicts with {}.",
                ctx.arena[package2].pretty_string(),
                conflict_target
            )
        }
        Reason::PackageRequires(link) => {
            let source = dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(literals[0]));
            let requires: Vec<usize> = literals[1..]
                .iter()
                .map(|&l| ctx.pool.literal_to_package(l))
                .collect();
            let text = link_pretty_string(&ctx.arena[source], link);
            if !requires.is_empty() {
                return format!(
                    "{text} -> satisfiable by {}.",
                    package_list(ctx, &requires, Some(&link.constraint), false)
                );
            }
            let pretty = link_constraint_pretty(&ctx.arena[source], link);
            let (_, reason) = missing_package_reason(
                ctx,
                &link.target,
                Some(PrettyConstraint {
                    constraint: &link.constraint,
                    pretty: &pretty,
                }),
            );
            format!("{text} -> {reason}")
        }
        Reason::PackageSameName(replaced_name) => {
            let mut package_names: Vec<String> = Vec::new();
            for &l in literals {
                let name = ctx.arena[ctx.pool.literal_to_package(l)].name.clone();
                if !package_names.contains(&name) {
                    package_names.push(name);
                }
            }
            let all: Vec<usize> = literals
                .iter()
                .map(|&l| ctx.pool.literal_to_package(l))
                .collect();
            if package_names.len() > 1 {
                let reason = if !package_names.contains(replaced_name) {
                    format!(
                        "They {} replace {replaced_name} and thus cannot coexist.",
                        if literals.len() == 2 { "both" } else { "all" }
                    )
                } else {
                    let replacers: Vec<&String> = package_names
                        .iter()
                        .filter(|n| *n != replaced_name)
                        .collect();
                    let head = if replacers.len() == 1 {
                        format!("{} replaces ", replacers[0])
                    } else {
                        format!(
                            "[{}] replace ",
                            replacers
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    format!("{head}{replaced_name} and thus cannot coexist with it.")
                };
                let mut installed: Vec<usize> = Vec::new();
                let mut removable: Vec<usize> = Vec::new();
                for &l in literals {
                    let idx = ctx.pool.literal_to_package(l);
                    if installed_map.contains(&(l.unsigned_abs() as usize)) {
                        installed.push(idx);
                    } else {
                        removable.push(idx);
                    }
                }
                if !installed.is_empty() && !removable.is_empty() {
                    return format!(
                        "{} cannot be installed as that would require removing {}. {reason}",
                        package_list(ctx, &removable, None, true),
                        package_list(ctx, &installed, None, true)
                    );
                }
                return format!(
                    "Only one of these can be installed: {}. {reason}",
                    package_list(ctx, &all, None, true)
                );
            }
            format!(
                "You can only install one version of a package, so only one of these can be installed: {}.",
                package_list(ctx, &all, None, true)
            )
        }
        Reason::Learned(_) => {
            let rule_text = if literals.len() == 1 {
                literal_to_pretty_string(ctx, literals[0], installed_map)
            } else {
                let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
                for &l in literals {
                    let idx = ctx.pool.literal_to_package(l);
                    let id = l.unsigned_abs() as usize;
                    let group = if installed_map.contains(&id) {
                        if l > 0 {
                            "keep"
                        } else {
                            "remove"
                        }
                    } else if l > 0 {
                        "install"
                    } else {
                        "don't install"
                    };
                    let dedup = dedup_default_branch_alias(ctx, idx);
                    match groups.iter_mut().find(|(g, _)| *g == group) {
                        Some((_, list)) => list.push(dedup),
                        None => groups.push((group, vec![dedup])),
                    }
                }
                let mut texts: Vec<String> = Vec::new();
                for (group, packages) in groups {
                    texts.push(format!(
                        "{group}{} {}",
                        if packages.len() > 1 { " one of" } else { "" },
                        package_list(ctx, &packages, None, false)
                    ));
                }
                texts.join(" | ")
            };
            format!("Conclusion: {rule_text} (conflict analysis result)")
        }
        Reason::PackageAlias { .. } => {
            let alias = ctx.pool.literal_to_package(literals[0]);
            if ctx.arena[alias].version == "9999999-dev" {
                return String::new();
            }
            let package = dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(literals[1]));
            format!(
                "{} is an alias of {} and thus requires it to be installed too.",
                ctx.arena[alias].pretty_string(),
                ctx.arena[package].pretty_string()
            )
        }
        Reason::PackageInverseAlias { .. } => {
            let alias = ctx.pool.literal_to_package(literals[1]);
            if ctx.arena[alias].version == "9999999-dev" {
                return String::new();
            }
            let package = dedup_default_branch_alias(ctx, ctx.pool.literal_to_package(literals[0]));
            format!(
                "{} is an alias of {} and must be installed with it.",
                ctx.arena[alias].pretty_string(),
                ctx.arena[package].pretty_string()
            )
        }
    }
}

/// `Pool::literalToPrettyString`.
fn literal_to_pretty_string(
    ctx: &MessageContext<'_>,
    literal: i64,
    installed_map: &HashSet<usize>,
) -> String {
    let idx = ctx.pool.literal_to_package(literal);
    let prefix = if installed_map.contains(&(literal.unsigned_abs() as usize)) {
        if literal > 0 {
            "keep"
        } else {
            "remove"
        }
    } else if literal > 0 {
        "install"
    } else {
        "don't install"
    };
    format!("{prefix} {}", ctx.arena[idx].pretty_string())
}

// ---------------------------------------------------------------------------
// Package lists

/// `Problem::getPackageList` on arena indices.
fn package_list(
    ctx: &MessageContext<'_>,
    packages: &[usize],
    constraint: Option<&Constraint>,
    use_removed_version_group: bool,
) -> String {
    // name -> (pretty name, normalized -> pretty version)
    let mut prepared: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();
    let mut has_default_branch: HashSet<String> = HashSet::new();
    for &idx in packages {
        let p = &ctx.arena[idx];
        let slot = match prepared.iter_mut().position(|(n, _, _)| *n == p.name) {
            Some(i) => i,
            None => {
                prepared.push((p.name.clone(), p.pretty_name.clone(), BTreeMap::new()));
                prepared.len() - 1
            }
        };
        prepared[slot].1 = p.pretty_name.clone();
        let pretty = match p.alias_of {
            Some(base) => format!(
                "{} (alias of {})",
                p.pretty_version, ctx.arena[base].pretty_version
            ),
            None => p.pretty_version.clone(),
        };
        prepared[slot].2.insert(p.version.clone(), pretty);
        if let Some(c) = constraint {
            for (v, pretty) in ctx.pool.removed_versions(&p.name, c) {
                prepared[slot].2.insert(v, pretty);
            }
        }
        if use_removed_version_group {
            for (v, pretty) in ctx.pool.removed_versions_by_package(idx) {
                prepared[slot].2.insert(v, pretty);
            }
        }
        if p.is_default_branch {
            has_default_branch.insert(p.name.clone());
        }
    }
    let mut strings: Vec<String> = Vec::new();
    for (name, pretty_name, mut versions) in prepared {
        if has_default_branch.contains(&name) {
            versions.remove("9999999-dev");
        }
        let ordered = versions_in_order(&versions);
        let list: Vec<String> = if ctx.is_verbose {
            ordered.into_iter().map(|(_, p)| p).collect()
        } else {
            condense_version_list(&ordered, 4, 16)
        };
        strings.push(format!("{pretty_name}[{}]", list.join(", ")));
    }
    strings.join(", ")
}

/// `hasMultipleNames`.
fn has_multiple_names(ctx: &MessageContext<'_>, packages: &[usize]) -> bool {
    let mut name: Option<&str> = None;
    for &idx in packages {
        let n = ctx.arena[idx].name.as_str();
        match name {
            None => name = Some(n),
            Some(current) if current == n => {}
            Some(_) => return true,
        }
    }
    false
}

/// `constraintToText`.
fn constraint_to_text(constraint: Option<PrettyConstraint<'_>>) -> String {
    let Some(pc) = constraint else {
        return String::new();
    };
    if let Constraint::Single {
        op: Op::Eq,
        version,
    } = pc.constraint
    {
        if !version.starts_with("dev-") {
            let plain = !pc.pretty.is_empty()
                && pc
                    .pretty
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
            if !plain {
                return format!(" {} (exact version match)", pc.pretty);
            }
            let mut versions = vec![pc.pretty.to_owned()];
            let dots = pc.pretty.matches('.').count();
            let mut i = 3i64 - dots as i64;
            while i > 0 {
                let last = versions.last().expect("non-empty").clone();
                versions.push(format!("{last}.0"));
                i -= 1;
            }
            let text = if versions.len() > 1 {
                format!(
                    "{} or {}",
                    versions[..versions.len() - 1].join(", "),
                    versions[versions.len() - 1]
                )
            } else {
                versions[0].clone()
            };
            return format!(" {} (exact version match: {text})", pc.pretty);
        }
    }
    format!(" {}", pc.pretty)
}

// ---------------------------------------------------------------------------
// Repository set lookups

/// `getRepoName` of a package's repository.
fn repo_name(ctx: &MessageContext<'_>, origin: Origin) -> String {
    match origin {
        Origin::Root => "root package repo".to_owned(),
        Origin::Platform => "platform repo".to_owned(),
        Origin::Locked => "lock repo".to_owned(),
        Origin::Repository(i) => match ctx.set.repositories.get(i) {
            Some(Repository::Composer(repo)) => repo.repo_name(),
            Some(Repository::Path(repo)) => repo.repo_name(),
            _ => "unknown repo".to_owned(),
        },
        Origin::Detached | Origin::Result => "unknown repo".to_owned(),
    }
}

/// `RepositorySet::findPackages($name, $constraint, $flags)`: arena
/// indices, repository order.
fn find_packages(
    ctx: &mut MessageContext<'_>,
    name: &str,
    constraint: Option<&Constraint>,
    flags: u8,
) -> Vec<usize> {
    let ignore_stability = flags & ALLOW_UNACCEPTABLE_STABILITIES != 0;
    let from_all_repos = flags & ALLOW_SHADOWED_REPOSITORIES != 0;
    let all_stabilities: BTreeMap<String, i32> = ["stable", "RC", "beta", "alpha", "dev"]
        .iter()
        .map(|s| (s.to_string(), crate::version::stability_rank(s)))
        .collect();
    let empty_flags = BTreeMap::new();
    let already = BTreeMap::new();
    let map = vec![(
        name.to_owned(),
        constraint.cloned().unwrap_or(Constraint::MatchAll),
    )];
    let mut found: Vec<usize> = Vec::new();
    for (i, repository) in ctx.set.repositories.iter().enumerate() {
        let members: Option<&Vec<usize>> = match repository {
            Repository::Root(m) | Repository::Platform(m) | Repository::Locked(m) => Some(m),
            Repository::Path(repo) => Some(&repo.members),
            Repository::Composer(_) => None,
        };
        if from_all_repos {
            // `$repository->findPackages($name, $constraint)`: no stability
            // filter at this stage.
            match repository {
                Repository::Composer(repo) => {
                    if let Ok((_, ids)) = repo.load_packages(
                        &map,
                        &all_stabilities,
                        &empty_flags,
                        &already,
                        Origin::Repository(i),
                        ctx.arena,
                    ) {
                        found.extend(ids);
                    }
                }
                _ => {
                    for &idx in members.expect("array repository") {
                        let p = &ctx.arena[idx];
                        if p.name == name
                            && constraint.is_none_or(|c| {
                                c.matches(&Constraint::new(Op::Eq, p.version.clone()))
                            })
                        {
                            found.push(idx);
                        }
                    }
                }
            }
            continue;
        }
        let (acceptable, stability_flags) = if ignore_stability {
            (&all_stabilities, &empty_flags)
        } else {
            (&ctx.set.acceptable_stabilities, &ctx.set.stability_flags)
        };
        let (names_found, ids) = match repository {
            Repository::Composer(repo) => repo
                .load_packages(
                    &map,
                    acceptable,
                    stability_flags,
                    &already,
                    Origin::Repository(i),
                    ctx.arena,
                )
                .unwrap_or_default(),
            _ => crate::pool::array_repository_load_packages(
                members.expect("array repository"),
                &map,
                acceptable,
                stability_flags,
                &already,
                ctx.arena,
            ),
        };
        found.extend(ids);
        if names_found.iter().any(|n| n == name) {
            break;
        }
    }
    if ignore_stability || !from_all_repos {
        return found;
    }
    found
        .into_iter()
        .filter(|&idx| {
            let p = &ctx.arena[idx];
            is_package_acceptable(
                &ctx.set.acceptable_stabilities,
                &ctx.set.stability_flags,
                &p.names(true),
                p.stability,
            )
        })
        .collect()
}

/// `RepositorySet::getProviders($name)` without the providers API: every
/// repository's loaded packages whose `provide` links target the name
/// (`ArrayRepository::getProviders`), merged by name.
fn providers(ctx: &mut MessageContext<'_>, name: &str) -> Vec<(String, Option<String>)> {
    let mut result: Vec<(String, Option<String>)> = Vec::new();
    for (i, repository) in ctx.set.repositories.iter().enumerate() {
        let candidates: Vec<usize> = match repository {
            Repository::Root(m) | Repository::Platform(m) | Repository::Locked(m) => m.clone(),
            Repository::Path(repo) => repo.members.clone(),
            Repository::Composer(repo) => {
                // `ComposerRepository::getProviders`: the providers API
                // answers alone when the repository declares one.
                if let Ok(Some(api)) = repo.providers_api(name) {
                    for (n, d) in api {
                        match result.iter_mut().find(|(existing, _)| *existing == n) {
                            Some(slot) => slot.1 = d,
                            None => result.push((n, d)),
                        }
                    }
                    continue;
                }
                repo.provider_candidates(Origin::Repository(i), ctx.arena)
                    .unwrap_or_default()
            }
        };
        let mut repo_result: Vec<(String, Option<String>)> = Vec::new();
        for idx in candidates {
            let p = &ctx.arena[idx];
            if repo_result.iter().any(|(n, _)| *n == p.name) {
                continue;
            }
            if p.provides.iter().any(|l| l.target == name) {
                let description = p
                    .raw
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                repo_result.push((p.name.clone(), description));
            }
        }
        // `array_merge` on string keys: first position kept, value replaced.
        for (n, d) in repo_result {
            match result.iter_mut().find(|(existing, _)| *existing == n) {
                Some(slot) => slot.1 = d,
                None => result.push((n, d)),
            }
        }
    }
    result
}

/// `getProvidersList`.
fn providers_list(ctx: &mut MessageContext<'_>, name: &str, max: usize) -> Option<String> {
    let list = providers(ctx, name);
    if list.is_empty() {
        return None;
    }
    let shown: &[(String, Option<String>)] = if list.len() > max + 1 {
        &list[..max]
    } else {
        &list
    };
    let mut text = String::new();
    for (n, description) in shown {
        let d = match description {
            Some(d) if !d.is_empty() => format!(" {}", php_substr(d, 100)),
            _ => String::new(),
        };
        text.push_str(&format!("      - {n}{d}\n"));
    }
    if list.len() > max + 1 {
        text.push_str(&format!("      ... and {} more.\n", list.len() - max));
    }
    Some(text)
}

/// `substr($s, 0, $n)` on bytes (a UTF-8 cut is kept as PHP makes it).
fn php_substr(s: &str, n: usize) -> String {
    let bytes = s.as_bytes();
    if bytes.len() <= n {
        return s.to_owned();
    }
    String::from_utf8_lossy(&bytes[..n]).into_owned()
}

/// `getPlatformPackageVersion`.
fn platform_package_version(ctx: &MessageContext<'_>, name: &str) -> Option<String> {
    let available = ctx.pool.what_provides(ctx.arena, name, None);
    if available.is_empty() {
        return None;
    }
    let idxs: Vec<usize> = available
        .iter()
        .map(|&id| ctx.pool.package_by_id(id))
        .collect();
    let selected = idxs
        .iter()
        .copied()
        .find(|&idx| ctx.arena[idx].origin == Origin::Platform)
        .unwrap_or(idxs[0]);
    let p = &ctx.arena[selected];
    if p.name != name {
        for link in p.provides.iter().chain(p.replaces.iter()) {
            if link.target == name {
                let description = link.kind.description();
                return Some(format!(
                    "{} {}d by {}",
                    link.pretty_constraint,
                    &description[..description.len() - 1],
                    p.pretty_string()
                ));
            }
        }
    }
    let mut version = p.pretty_version.clone();
    let overridden = p
        .raw
        .get("extra")
        .and_then(|e| e.get("config.platform"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    if overridden {
        let description = p
            .raw
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        version.push_str(&format!("; {}", description.replace("Package ", "")));
    }
    Some(version)
}

// ---------------------------------------------------------------------------
// Missing package reasons

/// `Problem::getMissingPackageReason`: `[prefix, reason]`.
fn missing_package_reason(
    ctx: &mut MessageContext<'_>,
    package_name: &str,
    constraint: Option<PrettyConstraint<'_>>,
) -> (String, String) {
    let ctt = constraint_to_text(constraint);
    let c = constraint.map(|pc| pc.constraint);
    if is_platform_package(package_name) {
        if package_name.to_lowercase().starts_with("php") || package_name == "hhvm" {
            let version = platform_package_version(ctx, package_name);
            let msg = format!("- Root composer.json requires {package_name}{ctt} but ");
            if package_name == "hhvm" {
                if !ctx
                    .pool
                    .what_provides(ctx.arena, package_name, None)
                    .is_empty()
                {
                    return (
                        msg,
                        "your HHVM version does not satisfy that requirement.".to_owned(),
                    );
                }
                return (
                    msg,
                    "HHVM was not detected on this machine, make sure it is in your PATH."
                        .to_owned(),
                );
            }
            return match version {
                None => (
                    msg,
                    format!("the {package_name} package is disabled by your platform config. Enable it again with \"composer config platform.{package_name} --unset\"."),
                ),
                Some(v) => (
                    msg,
                    format!("your {package_name} version ({v}) does not satisfy that requirement."),
                ),
            };
        }
        if package_name.to_lowercase().starts_with("ext-") {
            if package_name.contains(' ') {
                return (
                    "- ".to_owned(),
                    format!(
                        "PHP extension {package_name} should be required as {}.",
                        package_name.replace(' ', "-")
                    ),
                );
            }
            let ext = &package_name[4..];
            let msg =
                format!("- Root composer.json requires PHP extension {package_name}{ctt} but ");
            let version = platform_package_version(ctx, package_name);
            if version.is_none() {
                let providers_str = providers_list(ctx, package_name, 5).map(|s| {
                    format!("\n\n      Alternatively you can require one of these packages that provide the extension (or parts of it):\n      <warning>Keep in mind that the suggestions are automated and may not be valid or safe to use</warning>\n{s}")
                }).unwrap_or_default();
                if ctx.loaded_extensions.contains(&package_name.to_lowercase()) {
                    return (
                        msg,
                        format!("the {package_name} package is disabled by your platform config. Enable it again with \"composer config platform.{package_name} --unset\".{providers_str}"),
                    );
                }
                return (
                    msg,
                    format!("it is missing from your system. Install or enable PHP's {ext} extension.{providers_str}"),
                );
            }
            return (
                msg,
                format!(
                    "it has the wrong version installed ({}).",
                    version.expect("checked")
                ),
            );
        }
        if package_name.to_lowercase().starts_with("lib-") {
            if package_name.to_lowercase() == "lib-icu" {
                let error = if ctx.loaded_extensions.contains("ext-intl") {
                    "it has the wrong version installed, try upgrading the intl extension."
                } else {
                    "it is missing from your system, make sure the intl extension is loaded."
                };
                return (
                    format!(
                        "- Root composer.json requires linked library {package_name}{ctt} but "
                    ),
                    error.to_owned(),
                );
            }
            let providers_str = providers_list(ctx, package_name, 5).map(|s| {
                format!("\n\n      Alternatively you can require one of these packages that provide the library (or parts of it):\n      <warning>Keep in mind that the suggestions are automated and may not be valid or safe to use</warning>\n{s}")
            }).unwrap_or_default();
            return (
                format!("- Root composer.json requires linked library {package_name}{ctt} but "),
                format!("it has the wrong version installed or is missing from your system, make sure to load the extension providing it.{providers_str}"),
            );
        }
    }

    let mut locked_package: Option<usize> = None;
    for idx in ctx.request.locked_packages_all() {
        if ctx.arena[idx].name == package_name {
            locked_package = Some(idx);
            if ctx.pool.is_unacceptable_fixed_or_locked(idx) {
                let p = &ctx.arena[idx];
                return (
                    "- ".to_owned(),
                    format!("{} is fixed to {} (lock file version) by a partial update but that version is rejected by your minimum-stability. Make sure you list it as an argument for the update command.", p.pretty_name, p.pretty_version),
                );
            }
            break;
        }
    }

    if let Some(pc) = constraint {
        if let Constraint::Single { op: Op::Eq, .. } = pc.constraint {
            if pc.pretty.starts_with("dev-") && pc.pretty.contains('#') {
                let new_constraint = strip_as_suffix(pc.pretty);
                let alt = new_constraint.replace('#', "+");
                let multi = Constraint::Multi {
                    constraints: vec![
                        Constraint::new(Op::Eq, new_constraint.clone()),
                        Constraint::new(Op::Eq, alt),
                    ],
                    conjunctive: false,
                };
                let packages = find_packages(ctx, package_name, Some(&multi), 0);
                if !packages.is_empty() {
                    return (
                        format!("- Root composer.json requires {package_name}{ctt}, "),
                        format!(
                            "found {}. The # character in branch names is replaced by a + character. Make sure to require it as \"{}\".",
                            package_list(ctx, &packages, c, false),
                            pc.pretty.replace('#', "+")
                        ),
                    );
                }
            }
        }
    }

    let packages = find_packages(ctx, package_name, c, 0);
    if !packages.is_empty() {
        if let Some(root_req) = ctx.set.root_requires.get(package_name) {
            let root_pretty = ctx
                .request
                .pretty_requires
                .get(package_name)
                .cloned()
                .unwrap_or_else(|| root_req.to_string());
            if !packages.iter().any(|&idx| {
                root_req.matches(&Constraint::new(Op::Eq, ctx.arena[idx].version.clone()))
            }) {
                return (
                    format!("- Root composer.json requires {package_name}{ctt}, "),
                    format!(
                        "found {} but {} with your root composer.json require ({root_pretty}).",
                        package_list(ctx, &packages, c, false),
                        if has_multiple_names(ctx, &packages) {
                            "these conflict"
                        } else {
                            "it conflicts"
                        }
                    ),
                );
            }
        }
        let first_names = ctx.arena[packages[0]].names(true);
        for name in first_names {
            if let Some(temp) = ctx.set.temporary_constraints.get(&name) {
                if !packages.iter().any(|&idx| {
                    temp.matches(&Constraint::new(Op::Eq, ctx.arena[idx].version.clone()))
                }) {
                    return (
                        format!("- Root composer.json requires {name}{ctt}, "),
                        format!(
                            "found {} but {} with your temporary update constraint ({name}:{temp}).",
                            package_list(ctx, &packages, c, false),
                            if has_multiple_names(ctx, &packages) {
                                "these conflict"
                            } else {
                                "it conflicts"
                            }
                        ),
                    );
                }
            }
        }
        if let Some(locked) = locked_package {
            let fixed = Constraint::new(Op::Eq, ctx.arena[locked].version.clone());
            if !packages
                .iter()
                .any(|&idx| fixed.matches(&Constraint::new(Op::Eq, ctx.arena[idx].version.clone())))
            {
                return (
                    format!("- Root composer.json requires {package_name}{ctt}, "),
                    format!(
                        "found {} but the package is fixed to {} (lock file version) by a partial update and that version does not match. Make sure you list it as an argument for the update command.",
                        package_list(ctx, &packages, c, false),
                        ctx.arena[locked].pretty_version
                    ),
                );
            }
        }
        if let Some(cst) = c {
            if ctx.pool.is_abandoned_removed(package_name, cst) {
                return (
                    format!("- Root composer.json requires {package_name}{ctt}, "),
                    format!(
                        "found {} but these were not loaded, because they are abandoned and you configured \"policy.abandoned.block\" to true.",
                        package_list(ctx, &packages, c, false)
                    ),
                );
            }
            if ctx.pool.is_security_removed(package_name, cst) {
                let complete = matching_security_advisories(ctx, &packages, package_name);
                let (advisories_list, advisory_ids): (Vec<String>, Vec<String>) = match complete {
                    Some(list) if !list.is_empty() => (
                        list.iter()
                            .map(|a| {
                                let link = a.complete.as_ref().and_then(|c| c.link.clone());
                                match link {
                                    Some(l) if !l.is_empty() => {
                                        format!("<href={}>{}</>", escape_output(&l), a.advisory_id)
                                    }
                                    _ if a.advisory_id.starts_with("PKSA-") => format!(
                                        "<href={}>{}</>",
                                        escape_output(&format!(
                                            "https://packagist.org/security-advisories/{}",
                                            a.advisory_id
                                        )),
                                        a.advisory_id
                                    ),
                                    _ => a.advisory_id.clone(),
                                }
                            })
                            .collect(),
                        list.iter().map(|a| a.advisory_id.clone()).collect(),
                    ),
                    _ => {
                        let ids = ctx.pool.security_advisory_ids(package_name, cst);
                        (
                            ids.iter()
                                .map(|id| {
                                    if id.starts_with("PKSA-") {
                                        format!(
                                            "<href={}>{id}</>",
                                            escape_output(&format!(
                                                "https://packagist.org/security-advisories/{id}"
                                            ))
                                        )
                                    } else {
                                        id.clone()
                                    }
                                })
                                .collect(),
                            ids,
                        )
                    }
                };
                let has_packagist = advisory_ids.iter().all(|id| id.starts_with("PKSA-"));
                let details_hint = if has_packagist {
                    " Go to https://packagist.org/security-advisories/ to find advisory details."
                } else {
                    " Review the advisory details above for more information."
                };
                return (
                    format!("- Root composer.json requires {package_name}{ctt}, "),
                    format!(
                        "found {} but these were not loaded, because they are affected by security advisories (\"{}\").{details_hint} To ignore the advisories, add their IDs to the \"policy.advisories.ignore-id\" config or add the package to \"policy.advisories.ignore\". To turn the feature off entirely, you can set \"policy.advisories.block\" to false.",
                        package_list(ctx, &packages, c, false),
                        advisories_list.join("\", \"")
                    ),
                );
            }
            if ctx.pool.is_filter_list_removed_version(package_name, cst) {
                let filters = ctx.pool.filter_list_entries_text(package_name, cst);
                let ignore_paths = filters
                    .iter()
                    .map(|(l, _)| format!("\"policy.{l}.ignore\""))
                    .collect::<Vec<_>>()
                    .join(" and ");
                let off_paths = filters
                    .iter()
                    .map(|(l, _)| format!("\"policy.{l}.block\""))
                    .collect::<Vec<_>>()
                    .join(" and ");
                return (
                    format!("- Root composer.json requires {package_name}{ctt}, "),
                    format!(
                        "found {} but these were not loaded, because they were {}. To ignore filters for this package, add the package to the {ignore_paths} config. To turn the feature off entirely, you can set {off_paths} to false.",
                        package_list(ctx, &packages, c, false),
                        filters.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                );
            }
        }
        if packages
            .iter()
            .all(|&idx| ctx.arena[idx].origin == Origin::Locked)
        {
            return (
                format!("- Root composer.json requires {package_name}{ctt}, "),
                format!(
                    "found {} in the lock file but not in remote repositories, make sure you avoid updating this package to keep the one from the lock file.",
                    package_list(ctx, &packages, c, false)
                ),
            );
        }
        return (
            format!("- Root composer.json requires {package_name}{ctt}, "),
            format!(
                "found {} but these were not loaded, likely because {} with another require.",
                package_list(ctx, &packages, c, false),
                if has_multiple_names(ctx, &packages) {
                    "they conflict"
                } else {
                    "it conflicts"
                }
            ),
        );
    }

    let packages = find_packages(ctx, package_name, c, ALLOW_UNACCEPTABLE_STABILITIES);
    if !packages.is_empty() {
        let all_repos = find_packages(ctx, package_name, c, ALLOW_SHADOWED_REPOSITORIES);
        if !all_repos.is_empty() {
            return check_for_lower_prio_repo(
                ctx,
                package_name,
                &packages,
                &all_repos,
                "minimum-stability",
                constraint,
            );
        }
        return (
            format!("- Root composer.json requires {package_name}{ctt}, "),
            format!(
                "found {} but {} not match your minimum-stability.",
                package_list(ctx, &packages, c, false),
                if has_multiple_names(ctx, &packages) {
                    "these do"
                } else {
                    "it does"
                }
            ),
        );
    }

    let packages = find_packages(ctx, package_name, None, ALLOW_UNACCEPTABLE_STABILITIES);
    if !packages.is_empty() {
        let all_repos = find_packages(ctx, package_name, c, ALLOW_SHADOWED_REPOSITORIES);
        if !all_repos.is_empty() {
            return check_for_lower_prio_repo(
                ctx,
                package_name,
                &packages,
                &all_repos,
                "constraint",
                constraint,
            );
        }
        let mut suffix = String::new();
        if let Some(pc) = constraint {
            if let Constraint::Single { version, .. } = pc.constraint {
                if version == "dev-master" {
                    for &idx in &packages {
                        let v = &ctx.arena[idx].version;
                        if v == "dev-default" || v == "dev-main" {
                            suffix = format!(
                                " Perhaps dev-master was renamed to {}?",
                                ctx.arena[idx].pretty_version
                            );
                            break;
                        }
                    }
                }
            }
        }
        if ctx.arena[packages[0]].origin == Origin::Root {
            suffix =
                " See https://getcomposer.org/dep-on-root for details and assistance.".to_owned();
        }
        return (
            format!("- Root composer.json requires {package_name}{ctt}, "),
            format!(
                "found {} but {} not match the constraint.{suffix}",
                package_list(ctx, &packages, c, false),
                if has_multiple_names(ctx, &packages) {
                    "these do"
                } else {
                    "it does"
                }
            ),
        );
    }

    let legal = |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '-');
    if !package_name.chars().all(legal) {
        let illegal: String = package_name.chars().filter(|c| !legal(*c)).collect();
        return (
            format!("- Root composer.json requires {package_name}, it "),
            format!(
                "could not be found, it looks like its name is invalid, \"{illegal}\" is not allowed in package names."
            ),
        );
    }

    if let Some(providers_str) = providers_list(ctx, package_name, 15) {
        return (
            format!("- Root composer.json requires {package_name}{ctt}, it "),
            format!(
                "could not be found in any version, but the following packages provide it:\n{providers_str}      Consider requiring one of these to satisfy the {package_name} requirement."
            ),
        );
    }

    (
        format!("- Root composer.json requires {package_name}, it "),
        "could not be found in any version, there may be a typo in the package name.".to_owned(),
    )
}

/// `Preg::replace('{ +as +([^,\s|]+)$}', '', $pretty)`.
fn strip_as_suffix(pretty: &str) -> String {
    let trimmed = pretty.trim_end();
    if let Some(pos) = trimmed.rfind(" as ") {
        let after = &trimmed[pos + 4..];
        let after = after.trim_start();
        if !after.is_empty() && !after.contains([',', ' ', '|']) {
            return trimmed[..pos].trim_end().to_owned();
        }
    }
    pretty.to_owned()
}

/// `OutputFormatter::escape` for `<href=…>`: a backslash before `<`.
fn escape_output(text: &str) -> String {
    text.replace('<', "\\<")
}

/// `RepositorySet::getMatchingSecurityAdvisories($packages, false, true)`
/// for one name: the complete advisories, `None` when a repository fails
/// (the reference would throw; the ids recorded by the filter are used).
fn matching_security_advisories(
    ctx: &mut MessageContext<'_>,
    packages: &[usize],
    package_name: &str,
) -> Option<Vec<Advisory>> {
    let map = crate::pool_filters::constraints_by_name(packages, ctx.arena);
    let mut unreachable = Vec::new();
    let all = crate::pool_filters::security_advisories_for_constraints(
        &ctx.set.repositories,
        &map,
        false,
        true,
        &mut unreachable,
    )
    .ok()?;
    all.into_iter()
        .find(|(n, _)| n == package_name)
        .map(|(_, list)| list)
}

/// `computeCheckForLowerPrioRepo`.
fn check_for_lower_prio_repo(
    ctx: &mut MessageContext<'_>,
    package_name: &str,
    higher: &[usize],
    all_repos: &[usize],
    reason: &str,
    constraint: Option<PrettyConstraint<'_>>,
) -> (String, String) {
    let ctt = constraint_to_text(constraint);
    let c = constraint.map(|pc| pc.constraint);
    let mut next_repo: Option<Origin> = None;
    let mut next_repo_packages: Vec<usize> = Vec::new();
    for &idx in all_repos {
        let origin = ctx.arena[idx].origin;
        match next_repo {
            None => {
                next_repo = Some(origin);
                next_repo_packages.push(idx);
            }
            Some(o) if o == origin => next_repo_packages.push(idx),
            Some(_) => break,
        }
    }
    let next_repo = next_repo.expect("non-empty");
    if let Some(&top) = higher.first() {
        if ctx.arena[top].origin == Origin::Root {
            let p = &ctx.arena[top];
            return (
                format!("- Root composer.json requires {package_name}{ctt}, it is "),
                format!(
                    "satisfiable by {} from {} but {} {} is the root package and cannot be modified. See https://getcomposer.org/dep-on-root for details and assistance.",
                    package_list(ctx, &next_repo_packages, c, false),
                    repo_name(ctx, next_repo),
                    p.pretty_name,
                    p.pretty_version
                ),
            );
        }
    }
    if next_repo == Origin::Locked {
        let singular = higher.len() == 1;
        let mut suggestion = format!(
            "Make sure you either fix the {reason} or avoid updating this package to keep the one present in the lock file ({}).",
            package_list(ctx, &next_repo_packages, c, false)
        );
        let first = &ctx.arena[next_repo_packages[0]];
        if first.dist.as_ref().is_some_and(|d| d.kind == "path") {
            let symlink_off = first
                .raw
                .get("transport-options")
                .and_then(|t| t.get("symlink"))
                .and_then(serde_json::Value::as_bool)
                == Some(false);
            if !symlink_off {
                suggestion = format!("Make sure you fix the {reason} as packages installed from symlinked path repos are updated even in partial updates and the one from the lock file can thus not be used.");
            }
        }
        return (
            format!("- Root composer.json requires {package_name}{ctt}, "),
            format!(
                "found {} but {} not match your {reason} and {} therefore not installable. {suggestion}",
                package_list(ctx, higher, c, false),
                if singular { "it does" } else { "these do" },
                if singular { "is" } else { "are" }
            ),
        );
    }
    let higher_repo = repo_name(ctx, ctx.arena[higher[0]].origin);
    (
        format!("- Root composer.json requires {package_name}{ctt}, it is "),
        format!(
            "satisfiable by {} from {} but {} from {higher_repo} has higher repository priority. The packages from the higher priority repository do not match your {reason} and are therefore not installable. That repository is canonical so the lower priority repo's packages are not installable. See https://getcomposer.org/repoprio for details and assistance.",
            package_list(ctx, &next_repo_packages, c, false),
            repo_name(ctx, next_repo),
            package_list(ctx, higher, c, false)
        ),
    )
}

/// `getMissingLockedPackageReason`.
fn missing_locked_package_reason(ctx: &MessageContext<'_>, idx: usize) -> (String, String) {
    let p = &ctx.arena[idx];
    let constraint = Constraint::new(Op::Eq, p.version.clone());
    let prefix = format!(
        "- Package {} {} (in the lock file) ",
        p.name, p.pretty_version
    );
    if ctx
        .pool
        .is_filter_list_removed_version(&p.name, &constraint)
    {
        let filters = ctx.pool.filter_list_entries_text(&p.name, &constraint);
        let ignore_paths = filters
            .iter()
            .map(|(l, _)| format!("\"policy.{l}.ignore\""))
            .collect::<Vec<_>>()
            .join(" and ");
        let off_paths = filters
            .iter()
            .map(|(l, _)| format!("\"policy.{l}.block\""))
            .collect::<Vec<_>>()
            .join(" and ");
        return (
            prefix,
            format!(
                "was not loaded, because it was {}. To ignore filters for this package, add the package to the {ignore_paths} config. To turn the feature off entirely, you can set {off_paths} to false.",
                filters.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(", ")
            ),
        );
    }
    (
        prefix,
        "was not loaded (filter list removed locked package must have version removed from pool)."
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_strings_compare_as_numbers() {
        assert_eq!(php_string_cmp("1234", "98"), Ordering::Greater);
        assert_eq!(php_string_cmp("-12", "3"), Ordering::Less);
        assert_eq!(php_string_cmp("12-34", "98"), Ordering::Less); // strcmp
        assert_eq!(
            php_string_cmp("acme/a-1.0.0.0", "acme/b-1.0.0.0"),
            Ordering::Less
        );
        assert_eq!(php_string_cmp("1e3", "999"), Ordering::Greater);
    }

    #[test]
    fn output_styles_are_stripped_but_urls_kept() {
        assert_eq!(
            strip_output_styles("see <https://getcomposer.org/doc/04-schema.md#minimum-stability> for <info>x</info> <warning>y</warning> <href=https://a>PKSA-1</>"),
            "see <https://getcomposer.org/doc/04-schema.md#minimum-stability> for x y PKSA-1"
        );
    }

    #[test]
    fn version_lists_condense_per_major() {
        let versions: Vec<(String, String)> = [
            "1.0.0.0", "1.1.0.0", "1.2.0.0", "1.3.0.0", "1.4.0.0", "2.0.0.0",
        ]
        .iter()
        .map(|v| (v.to_string(), v.trim_end_matches(".0").to_string()))
        .collect();
        assert_eq!(
            condense_version_list(&versions, 4, 16),
            vec!["1", "...", "1.4", "2"]
        );
        assert_eq!(
            condense_version_list(&versions[..4], 4, 16),
            vec!["1", "1.1", "1.2", "1.3"]
        );
    }

    #[test]
    fn exact_constraints_get_their_expansions() {
        let c = Constraint::new(Op::Eq, "1.2.0.0");
        assert_eq!(
            constraint_to_text(Some(PrettyConstraint {
                constraint: &c,
                pretty: "1.2"
            })),
            " 1.2 (exact version match: 1.2, 1.2.0 or 1.2.0.0)"
        );
        assert_eq!(
            constraint_to_text(Some(PrettyConstraint {
                constraint: &c,
                pretty: "v1.2"
            })),
            " v1.2 (exact version match)"
        );
        let caret = Constraint::new(Op::Ge, "1.0.0.0-dev");
        assert_eq!(
            constraint_to_text(Some(PrettyConstraint {
                constraint: &caret,
                pretty: "^1.0"
            })),
            " ^1.0"
        );
    }
}
