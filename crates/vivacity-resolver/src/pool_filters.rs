//! The filters `PoolBuilder::buildPool` applies to the pool before the
//! optimizer: `SecurityAdvisoryPoolFilter` (security advisories and
//! abandoned packages) then `FilterListPoolFilter` (filter lists, with
//! Packagist's malware list first), driven by
//! [`crate::policy_config::PolicyConfig`].
//!
//! Versions removed by a list are kept in `Pool::filter_list_removed`: the
//! rule generator and the solver need them. Versions removed because of an
//! advisory only serve Composer's explanations (not ported) and are not
//! kept.

use std::collections::BTreeMap;

use pcre2::bytes::Regex;

use crate::constraint::{Constraint, Op};
use crate::package::Package;
use crate::platform::is_platform_package;
use crate::policy_config::{
    advisory_ignore_list_for_block, advisory_ignore_severity_for_block, flat_ignore_for_block,
    IgnoreMap, PolicyConfig,
};
use crate::pool::{Pool, Repository, Request};
use crate::repository::{AdvisoriesByName, Advisory, ComposerRepository, FilterEntry};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct FilterError(pub String);

/// `BasePackage::packageNamesToRegexp` (`{^(?:a|b)$}iD`), `None` without names.
fn package_names_regexp(names: &[String]) -> Option<Regex> {
    if names.is_empty() {
        return None;
    }
    let parts: Vec<String> = names
        .iter()
        .map(|n| crate::version::preg_quote(n).replace("\\*", ".*"))
        .collect();
    pcre2::bytes::RegexBuilder::new()
        .caseless(true)
        .build(&format!("^(?:{})\\z", parts.join("|")))
        .ok()
}

fn matches_regex(re: &Option<Regex>, name: &str) -> bool {
    re.as_ref()
        .is_some_and(|r| r.is_match(name.as_bytes()).unwrap_or(false))
}

/// `name -> MultiConstraint(= v1, = v2, ...)` of the given packages (root
/// aliases excluded), as `getMatchingSecurityAdvisories` and
/// `getMatchingFilterLists` build it.
pub(crate) fn constraints_by_name(
    packages: &[usize],
    arena: &[Package],
) -> Vec<(String, Constraint)> {
    let mut by_name: Vec<(String, Vec<Constraint>)> = Vec::new();
    for &idx in packages {
        let p = &arena[idx];
        if p.alias_of.is_some() && p.root_package_alias {
            continue;
        }
        let c = Constraint::new(Op::Eq, &p.version);
        match by_name.iter_mut().find(|(n, _)| *n == p.name) {
            Some((_, list)) => {
                // `$constraintsByName[$name][$version]`: one per version.
                if !list.iter().any(|existing| existing == &c) {
                    list.push(c);
                }
            }
            None => by_name.push((p.name.clone(), vec![c])),
        }
    }
    by_name
        .into_iter()
        .map(|(n, list)| (n, Constraint::create(list, false)))
        .collect()
}

fn composer_repos(repositories: &[Repository]) -> Vec<&ComposerRepository> {
    repositories
        .iter()
        .filter_map(|r| match r {
            Repository::Composer(c) => Some(c.as_ref()),
            _ => None,
        })
        .collect()
}

/// `RepositorySet::getSecurityAdvisoriesForConstraints`: the advisories of
/// all repositories, merged by name; an unreachable repository is ignored
/// (and reported) or fatal.
pub(crate) fn security_advisories_for_constraints(
    repositories: &[Repository],
    map: &[(String, Constraint)],
    allow_partial: bool,
    ignore_unreachable: bool,
    unreachable: &mut Vec<String>,
) -> Result<AdvisoriesByName, FilterError> {
    let mut all: AdvisoriesByName = Vec::new();
    for repo in composer_repos(repositories) {
        // `RepositorySet::__construct`/`getSecurityAdvisoriesForConstraints`:
        // only a TransportException falls under `ignore-unreachable`.
        let result = repo.has_security_advisories().and_then(|has| {
            if has {
                repo.get_security_advisories(map, allow_partial)
                    .map(|(_, a)| a)
            } else {
                Ok(Vec::new())
            }
        });
        match result {
            Ok(advisories) => {
                for (name, list) in advisories {
                    match all.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, existing)) => existing.extend(list),
                        None => all.push((name, list)),
                    }
                }
            }
            Err(e) if e.is_transport() && ignore_unreachable => unreachable.push(e.0),
            Err(e) => return Err(FilterError(e.0)),
        }
    }
    Ok(all)
}

/// `Auditor::needsCompleteAdvisoryLoad`: partial advisories and an ignore
/// rule that is not a `PKSA-` identifier.
fn needs_complete_advisory_load(
    advisories: &AdvisoriesByName,
    ignore_list: &[(String, Option<String>)],
) -> bool {
    if advisories.is_empty() {
        return false;
    }
    if advisories
        .iter()
        .all(|(_, list)| list.iter().all(|a| a.complete.is_some()))
    {
        return false;
    }
    ignore_list.iter().any(|(id, _)| !id.starts_with("PKSA-"))
}

/// `Auditor::processAdvisories`: what remains after the ignore rules.
fn process_advisories(
    all: AdvisoriesByName,
    ignore_list: &[(String, Option<String>)],
    ignored_severities: &[(String, Option<String>)],
) -> AdvisoriesByName {
    if ignore_list.is_empty() && ignored_severities.is_empty() {
        return all;
    }
    let ignored = |key: &str| ignore_list.iter().any(|(k, _)| k == key);
    let mut out: AdvisoriesByName = Vec::new();
    for (package, list) in all {
        for advisory in list {
            let mut active = true;
            if ignored(&package) || ignored(&advisory.advisory_id) {
                active = false;
            }
            if let Some(c) = &advisory.complete {
                if c.severity
                    .as_ref()
                    .is_some_and(|s| ignored_severities.iter().any(|(k, _)| k == s))
                {
                    active = false;
                }
                if c.cve.as_ref().is_some_and(|cve| ignored(cve)) {
                    active = false;
                }
                if c.source_remote_ids.iter().any(|id| ignored(id)) {
                    active = false;
                }
            }
            if active {
                match out.iter_mut().find(|(n, _)| *n == package) {
                    Some((_, v)) => v.push(advisory),
                    None => out.push((package.clone(), vec![advisory])),
                }
            }
        }
    }
    out
}

/// `isAbandoned()` of a complete package: `abandoned` true or a replacement
/// name.
fn is_abandoned(p: &Package) -> bool {
    match p.raw.get("abandoned") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => !s.is_empty(),
        _ => false,
    }
}

/// `SecurityAdvisoryPoolFilter::filter`: removes abandoned packages (if
/// `abandoned.block`) and non-dev versions covered by an advisory.
pub fn security_advisory_filter(
    pool: Pool,
    arena: &[Package],
    repositories: &[Repository],
    request: &Request,
    policy: &PolicyConfig,
    warnings: &mut Vec<String>,
) -> Result<Pool, FilterError> {
    if !policy.advisories.block {
        return Ok(pool);
    }
    let ignore_list = advisory_ignore_list_for_block(&policy.advisories);
    let ignore_unreachable = policy.ignore_unreachable.update;
    let candidates: Vec<usize> = pool
        .packages
        .iter()
        .copied()
        .filter(|&idx| {
            let p = &arena[idx];
            !matches!(p.origin, crate::package::Origin::Root)
                && !is_platform_package(&p.name)
                && !request.is_locked_package(idx)
        })
        .collect();
    let map = constraints_by_name(&candidates, arena);
    let mut unreachable = Vec::new();
    let mut all = security_advisories_for_constraints(
        repositories,
        &map,
        true,
        ignore_unreachable,
        &mut unreachable,
    )?;
    if needs_complete_advisory_load(&all, &ignore_list) {
        unreachable.clear();
        all = security_advisories_for_constraints(
            repositories,
            &map,
            false,
            ignore_unreachable,
            &mut unreachable,
        )?;
    }
    if ignore_unreachable && !unreachable.is_empty() {
        warnings.push("Security advisory data could not be fetched from some repositories (ignored per policy.ignore-unreachable); matches may be incomplete:".into());
        for r in &unreachable {
            warnings.push(format!("  - {r}"));
        }
    }
    let advisory_map = process_advisories(
        all,
        &ignore_list,
        &advisory_ignore_severity_for_block(&policy.advisories),
    );
    let abandoned_ignore: Vec<String> = flat_ignore_for_block(&policy.abandoned.ignore)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let abandoned_re = package_names_regexp(&abandoned_ignore);
    let mut kept: Vec<usize> = Vec::with_capacity(pool.packages.len());
    let mut security_removed: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut abandoned_removed: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for &idx in &pool.packages {
        let p = &arena[idx];
        if policy.abandoned.block && is_abandoned(p) && !matches_regex(&abandoned_re, &p.name) {
            for name in p.names(false) {
                abandoned_removed
                    .entry(name)
                    .or_default()
                    .insert(p.version.clone(), p.pretty_version.clone());
            }
            continue;
        }
        let matching = matching_advisories(p, &advisory_map);
        if !matching.is_empty() {
            let ids: Vec<String> = matching.iter().map(|a| a.advisory_id.clone()).collect();
            for name in p.names(false) {
                security_removed
                    .entry(name)
                    .or_default()
                    .insert(p.version.clone(), ids.clone());
            }
            continue;
        }
        kept.push(idx);
    }
    if kept.len() == pool.packages.len() {
        return Ok(pool);
    }
    let mut out = pool.with_packages(kept, arena);
    out.security_removed = security_removed;
    out.abandoned_removed = abandoned_removed;
    Ok(out)
}

/// `getMatchingAdvisories`: never for a dev version; on each of the
/// `getNames(false)` (name + `replace`).
fn matching_advisories<'a>(p: &Package, advisory_map: &'a AdvisoriesByName) -> Vec<&'a Advisory> {
    if p.is_dev() {
        return Vec::new();
    }
    let constraint = Constraint::new(Op::Eq, &p.version);
    let mut out = Vec::new();
    for name in p.names(false) {
        let Some((_, list)) = advisory_map.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        for a in list {
            if a.affected_versions.matches(&constraint) {
                out.push(a);
            }
        }
    }
    out
}

/// `FilterListPoolFilter::filter` in `update` or `install` scope: the
/// versions flagged by an active list; locked versions (or versions
/// identical to a lock version) are judged against the `install`-scoped
/// lists.
pub fn filter_list_filter(
    pool: Pool,
    arena: &[Package],
    repositories: &[Repository],
    request: &Request,
    policy: &PolicyConfig,
    block_scope: &str,
    warnings: &mut Vec<String>,
) -> Result<Pool, FilterError> {
    // A custom list with no source and no repository advertising it is
    // inert in Composer; otherwise it requires a provider that is not
    // ported.
    if !policy.custom_lists.is_empty() {
        for repo in composer_repos(repositories) {
            if let Ok(lists) = repo.get_filter_lists() {
                if let Some(l) = lists.iter().find(|l| policy.custom_lists.contains(l)) {
                    return Err(FilterError(format!(
                        "custom policy list \"{l}\" is not supported by vivacity yet"
                    )));
                }
            }
        }
    }
    let check_locked_against_install = block_scope == "update";
    let configured: Vec<String> = if policy.malware_blocks(block_scope) {
        vec!["malware".to_owned()]
    } else {
        Vec::new()
    };
    let install_lists: Vec<String> =
        if check_locked_against_install && policy.malware_blocks("install") {
            vec!["malware".to_owned()]
        } else {
            Vec::new()
        };
    let mut union: Vec<String> = configured.clone();
    for l in &install_lists {
        if !union.contains(l) {
            union.push(l.clone());
        }
    }
    if union.is_empty() {
        return Ok(pool);
    }
    let mut ignore_unreachable = policy.ignore_unreachable.for_block_scope(block_scope);
    if check_locked_against_install {
        ignore_unreachable = ignore_unreachable && policy.ignore_unreachable.install;
    }
    let filterable: Vec<usize> = pool
        .packages
        .iter()
        .copied()
        .filter(|&idx| {
            let p = &arena[idx];
            !matches!(p.origin, crate::package::Origin::Root) && !is_platform_package(&p.name)
        })
        .collect();
    let map = constraints_by_name(&filterable, arena);
    // `FilterListProviderSet::getMatchingFilterLists`.
    let mut by_list: Vec<(String, Vec<FilterEntry>)> = Vec::new();
    let mut unreachable = Vec::new();
    for repo in composer_repos(repositories) {
        // `FilterListProviderSet`: `hasFilter()` (packages.json) and
        // `getFilter()` alike only surface their TransportException under
        // `ignore-unreachable`.
        let provider_lists = match repo.get_filter_lists() {
            Ok(l) => l,
            Err(e) if e.is_transport() && ignore_unreachable => {
                unreachable.push(e.0);
                continue;
            }
            Err(e) => return Err(FilterError(e.0)),
        };
        let relevant: Vec<String> = union
            .iter()
            .filter(|l| provider_lists.contains(l))
            .cloned()
            .collect();
        if relevant.is_empty() {
            continue;
        }
        match repo.get_filter(&map, &relevant) {
            Ok(filter) => {
                for (list, entries) in filter {
                    if !union.contains(&list) || !provider_lists.contains(&list) {
                        continue;
                    }
                    for entry in entries {
                        let Some((_, wanted)) = map.iter().find(|(n, _)| *n == entry.package_name)
                        else {
                            continue;
                        };
                        if !entry.constraint.matches(wanted) {
                            continue;
                        }
                        match by_list.iter_mut().find(|(l, _)| *l == list) {
                            Some((_, v)) => v.push(entry),
                            None => by_list.push((list.clone(), vec![entry])),
                        }
                    }
                }
            }
            Err(e) if e.is_transport() && ignore_unreachable => unreachable.push(e.0),
            Err(e) => return Err(FilterError(e.0)),
        }
    }
    by_list.sort_by(|(a, _), (b, _)| a.cmp(b));
    if std::env::var_os("VIVACITY_TRACE").is_some() {
        eprintln!(
            "trace: filter lists       {union:?} → {} entries ({} names queried)",
            by_list.iter().map(|(_, e)| e.len()).sum::<usize>(),
            map.len()
        );
    }
    if !unreachable.is_empty() {
        warnings.push("Filter list data could not be fetched from some sources (ignored per policy.ignore-unreachable); matches may be incomplete:".into());
        for r in &unreachable {
            warnings.push(format!("  - {r}"));
        }
    }
    // `$filterListMap[$packageName][$listName][] = $entry`.
    let mut filter_map: BTreeMap<String, Vec<(String, Vec<FilterEntry>)>> = BTreeMap::new();
    for (list, entries) in &by_list {
        for e in entries {
            let lists = filter_map.entry(e.package_name.clone()).or_default();
            match lists.iter_mut().find(|(l, _)| l == list) {
                Some((_, v)) => v.push(e.clone()),
                None => lists.push((list.clone(), vec![e.clone()])),
            }
        }
    }
    if filter_map.is_empty() {
        return Ok(pool);
    }
    let locked_versions: BTreeMap<String, Vec<String>> = if check_locked_against_install {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for idx in request.locked_repository.iter().flatten() {
            let p = &arena[*idx];
            m.entry(p.name.clone()).or_default().push(p.version.clone());
        }
        m
    } else {
        BTreeMap::new()
    };
    let mut kept: Vec<usize> = Vec::with_capacity(pool.packages.len());
    let mut removed: crate::pool::FilterListRemoved = pool.filter_list_removed.clone();
    for &idx in &pool.packages {
        let p = &arena[idx];
        if matches!(p.origin, crate::package::Origin::Root) || is_platform_package(&p.name) {
            kept.push(idx);
            continue;
        }
        let locked_equivalent = check_locked_against_install
            && (request.is_locked_package(idx)
                || locked_versions
                    .get(&p.name)
                    .is_some_and(|v| v.contains(&p.version)));
        let (lists, scope) = if locked_equivalent {
            (&install_lists, "install")
        } else {
            (&configured, block_scope)
        };
        let matching = matching_entries(p, &filter_map, lists, policy, scope);
        if matching.is_empty() {
            kept.push(idx);
            continue;
        }
        for name in p.names(false) {
            let versions = removed.entry(name).or_default();
            match versions.iter_mut().find(|(v, _)| *v == p.version) {
                Some((_, e)) => *e = matching.clone(),
                None => versions.push((p.version.clone(), matching.clone())),
            }
        }
    }
    let mut out = pool.with_packages(kept, arena);
    out.filter_list_removed = removed;
    Ok(out)
}

/// `FilterListAuditor::matchingEntries` for the `block` operation: the
/// entries of the active lists covering the version, unless ignored.
fn matching_entries(
    p: &Package,
    filter_map: &BTreeMap<String, Vec<(String, Vec<FilterEntry>)>>,
    active_lists: &[String],
    policy: &PolicyConfig,
    _scope: &str,
) -> Vec<FilterEntry> {
    if filter_map.is_empty() || active_lists.is_empty() {
        return Vec::new();
    }
    let malware_active = active_lists.iter().any(|l| l == "malware");
    let ignore_source = &policy.malware.ignore_source;
    let ignore_map: &IgnoreMap = &policy.malware.ignore;
    let ignored_names: Vec<String> = flat_ignore_for_block(ignore_map)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let ignored_re = package_names_regexp(&ignored_names);
    let constraint = Constraint::new(Op::Eq, &p.version);
    let mut out = Vec::new();
    for name in p.names(false) {
        let Some(lists) = filter_map.get(&name) else {
            continue;
        };
        for (list, entries) in lists {
            if !active_lists.contains(list) {
                continue;
            }
            // `applyMalwareIgnoreSource`: the entries of an ignored source.
            let entries: Vec<&FilterEntry> = entries
                .iter()
                .filter(|e| {
                    !(list == "malware"
                        && malware_active
                        && e.source.as_ref().is_some_and(|s| ignore_source.contains(s)))
                })
                .collect();
            if matches_regex(&ignored_re, &name) && list == "malware" {
                // `isPackageIgnored`: a rule whose pattern and constraint
                // cover the version dismisses the list.
                let ignored = ignore_map.iter().any(|(_, rules)| {
                    rules.iter().any(|r| {
                        r.on_block
                            && matches_regex(
                                &package_names_regexp(std::slice::from_ref(&r.package_name)),
                                &name,
                            )
                            && r.constraint.matches(&constraint)
                    })
                });
                if ignored {
                    continue;
                }
            }
            for e in entries {
                if e.constraint.matches(&constraint) {
                    out.push(e.clone());
                }
            }
        }
    }
    out
}

/// Composer's text for a removed locked package:
/// `getFilterListEntryForPackageVersion` + the
/// `RULE_LOCKED_FILTER_LIST_REMOVED` problem of `Problem::getPrettyString`.
pub fn locked_removed_problem_text(pool: &Pool, p: &Package) -> String {
    let mut lists: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(versions) = pool.filter_list_removed.get(&p.name) {
        for (v, entries) in versions {
            if *v != p.version {
                continue;
            }
            for e in entries {
                let source = e
                    .source
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" reported by {s}"))
                    .unwrap_or_default();
                let url = e
                    .url
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" (see {s})"))
                    .unwrap_or_default();
                let reason = e
                    .reason
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" reason: {s}"))
                    .unwrap_or_default();
                let text = format!("{source}{url}{reason}");
                match lists.iter_mut().find(|(l, _)| *l == e.list_name) {
                    Some((_, v)) => v.push(text),
                    None => lists.push((e.list_name.clone(), vec![text])),
                }
            }
        }
    }
    let filters: Vec<String> = lists
        .iter()
        .map(|(l, entries)| {
            let action = if l == "malware" {
                "flagged as "
            } else {
                "filtered by "
            };
            format!("{action}{l}{}", entries.join(", "))
        })
        .collect();
    let ignore_paths: Vec<String> = lists
        .iter()
        .map(|(l, _)| format!("\"policy.{l}.ignore\""))
        .collect();
    let off_paths: Vec<String> = lists
        .iter()
        .map(|(l, _)| format!("\"policy.{l}.block\""))
        .collect();
    format!(
        "- Package {} {} (in the lock file) was not loaded, because it was {}. To ignore filters for this package, add the package to the {} config. To turn the feature off entirely, you can set {} to false.",
        p.name,
        p.pretty_version,
        filters.join(", "),
        ignore_paths.join(" and "),
        off_paths.join(" and ")
    )
}
