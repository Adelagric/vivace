//! Port of `Composer\Policy\PolicyConfig` and the policy classes
//! (docs/reference/policy/*.php): what `config.policy` and the legacy
//! `config.audit` say about the three pool filters (security advisories,
//! filter lists (malware), abandoned packages) after the global/project
//! merge of `Config::merge`, the environment variables (`COMPOSER_POLICY`,
//! `COMPOSER_POLICY_*_BLOCK`, `COMPOSER_NO_BLOCKING`...) and
//! `--no-blocking`.
//!
//! Not ported: custom lists (`policy.<other name>`) and
//! `audit.abandoned`/`policy.*.audit` (audit mode, no effect on blocking);
//! the former are rejected, the latter is ignored.

use serde_json::{Map, Value};

use crate::constraint::{parse_constraints, Constraint};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PolicyError(pub String);

/// A per-package ignore rule (`IgnorePackageRule`).
#[derive(Debug, Clone)]
pub struct IgnorePackageRule {
    pub package_name: String,
    pub constraint: Constraint,
    pub reason: Option<String>,
    pub on_block: bool,
    pub on_audit: bool,
}

/// A per-advisory-id ignore rule (`IgnoreIdRule`).
#[derive(Debug, Clone)]
pub struct IgnoreIdRule {
    pub id: String,
    pub reason: Option<String>,
    pub on_block: bool,
    pub on_audit: bool,
}

/// `IgnoreSeverityRule`.
#[derive(Debug, Clone)]
pub struct IgnoreSeverityRule {
    pub severity: String,
    pub reason: Option<String>,
    pub on_block: bool,
    pub on_audit: bool,
}

/// Per-package rules, in declaration order (PHP array).
pub type IgnoreMap = Vec<(String, Vec<IgnorePackageRule>)>;

#[derive(Debug, Clone)]
pub struct AdvisoriesPolicy {
    pub block: bool,
    pub ignore: IgnoreMap,
    pub ignore_id: Vec<IgnoreIdRule>,
    pub ignore_severity: Vec<IgnoreSeverityRule>,
}

#[derive(Debug, Clone)]
pub struct MalwarePolicy {
    pub block: bool,
    /// `all`, `update` or `install`.
    pub block_scope: String,
    pub ignore: IgnoreMap,
    pub ignore_source: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AbandonedPolicy {
    pub block: bool,
    pub ignore: IgnoreMap,
}

/// `IgnoreUnreachable`: an unreachable repository is ignored (warning) or
/// fatal, per operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IgnoreUnreachable {
    pub audit: bool,
    pub install: bool,
    pub update: bool,
}

impl IgnoreUnreachable {
    pub fn for_block_scope(&self, scope: &str) -> bool {
        if scope == "install" {
            self.install
        } else {
            self.update
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// `policy: false` (or `COMPOSER_POLICY=0`): everything is disabled.
    pub enabled: bool,
    pub advisories: AdvisoriesPolicy,
    pub malware: MalwarePolicy,
    pub abandoned: AbandonedPolicy,
    /// Names of the declared custom lists (not ported).
    pub custom_lists: Vec<String>,
    pub ignore_unreachable: IgnoreUnreachable,
}

const NON_LIST_KEYS: &[&str] = &["ignore-unreachable"];
const BUILTIN_LIST_NAMES: &[&str] = &["advisories", "malware", "abandoned"];

/// `Platform::getBoolEnv`: `0`/`1`/`false`/`true`/`off`/`on`, empty =
/// absent, any other value = error.
pub fn bool_env(name: &str) -> Result<Option<bool>, PolicyError> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => match v.as_str() {
            "1" | "true" | "on" => Ok(Some(true)),
            "0" | "false" | "off" => Ok(Some(false)),
            other => Err(PolicyError(format!(
                "Invalid value for {name}: {other}. Expected 0, 1, false, true, off, or on."
            ))),
        },
        _ => Ok(None),
    }
}

/// The merged value of `config.policy` and `config.audit` (`Config`
/// defaults, then the global config, then the project: `Config::merge`).
#[derive(Debug, Clone)]
pub struct RawPolicyConfig {
    /// `true`, `false` or an object.
    pub policy: Value,
    pub audit: Value,
}

impl Default for RawPolicyConfig {
    fn default() -> Self {
        RawPolicyConfig {
            policy: Value::Bool(true),
            audit: serde_json::json!({"ignore": [], "abandoned": "fail"}),
        }
    }
}

impl RawPolicyConfig {
    /// `Config::merge` for the `audit` and `policy` keys of a `config`.
    pub fn merge(&mut self, config: &Map<String, Value>) {
        if let Some(val) = config.get("audit") {
            let current_ignores = self
                .audit
                .get("ignore")
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let mut merged = php_array_merge(&self.audit, val);
            let incoming = val
                .get("ignore")
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            if let Value::Object(m) = &mut merged {
                m.insert(
                    "ignore".into(),
                    php_array_merge(&current_ignores, &incoming),
                );
            }
            self.audit = merged;
        }
        if let Some(val) = config.get("policy") {
            let val = if val == &Value::Bool(true) {
                Value::Object(Map::new())
            } else {
                val.clone()
            };
            match val {
                Value::Bool(false) => self.policy = Value::Bool(false),
                Value::Object(lists) => {
                    let mut current = match &self.policy {
                        Value::Object(m) => m.clone(),
                        _ => Map::new(),
                    };
                    for (list_name, list_config) in lists {
                        if NON_LIST_KEYS.contains(&list_name.as_str()) {
                            current.insert(list_name, list_config);
                            continue;
                        }
                        let list_config = if list_config == Value::Bool(true) {
                            Value::Object(Map::new())
                        } else {
                            list_config
                        };
                        let existing = match current.get(&list_name) {
                            Some(Value::Bool(true)) => Some(Value::Object(Map::new())),
                            Some(v) => Some(v.clone()),
                            None => None,
                        };
                        let merged = match (&existing, &list_config) {
                            (_, Value::Bool(false)) => Value::Bool(false),
                            (None | Some(Value::Bool(false)), _) => list_config.clone(),
                            (Some(Value::Object(e)), Value::Object(i)) => {
                                let mut m = php_array_merge(
                                    &Value::Object(e.clone()),
                                    &Value::Object(i.clone()),
                                );
                                for inner in
                                    ["ignore", "ignore-id", "ignore-severity", "ignore-source"]
                                {
                                    if let (Some(ei), Some(ii)) = (e.get(inner), i.get(inner)) {
                                        if is_php_array(ei) && is_php_array(ii) {
                                            if let Value::Object(mm) = &mut m {
                                                mm.insert(inner.into(), php_array_merge(ei, ii));
                                            }
                                        }
                                    }
                                }
                                m
                            }
                            _ => list_config.clone(),
                        };
                        current.insert(list_name, merged);
                    }
                    self.policy = Value::Object(current);
                }
                _ => {}
            }
        }
    }

    /// `Config::get('policy')`: `COMPOSER_POLICY=0` disables, `=1`
    /// re-enables a `false`.
    pub fn effective_policy(&self) -> Result<Value, PolicyError> {
        Ok(match bool_env("COMPOSER_POLICY")? {
            Some(false) => Value::Bool(false),
            Some(true) if self.policy == Value::Bool(false) => Value::Bool(true),
            _ => self.policy.clone(),
        })
    }
}

fn is_php_array(v: &Value) -> bool {
    matches!(v, Value::Array(_) | Value::Object(_))
}

/// `array_merge($a, $b)` on decoded arrays: the string keys of `b` replace
/// those of `a` (in place), list entries are appended at the end. A result
/// list without any string key stays a list.
fn php_array_merge(a: &Value, b: &Value) -> Value {
    fn entries(v: &Value) -> Vec<(Option<String>, Value)> {
        match v {
            Value::Array(items) => items.iter().map(|x| (None, x.clone())).collect(),
            Value::Object(m) => m
                .iter()
                .map(|(k, x)| {
                    // A canonical numeric key is a PHP integer.
                    let is_int = k == "0"
                        || (k.starts_with(['1', '2', '3', '4', '5', '6', '7', '8', '9'])
                            && k.bytes().all(|c| c.is_ascii_digit()));
                    (if is_int { None } else { Some(k.clone()) }, x.clone())
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    let mut out: Vec<(Option<String>, Value)> = Vec::new();
    for (k, v) in entries(a).into_iter().chain(entries(b)) {
        match &k {
            Some(key) => match out.iter_mut().find(|(ok, _)| ok.as_deref() == Some(key)) {
                Some(slot) => slot.1 = v,
                None => out.push((k, v)),
            },
            None => out.push((None, v)),
        }
    }
    if out.iter().all(|(k, _)| k.is_none()) {
        return Value::Array(out.into_iter().map(|(_, v)| v).collect());
    }
    let mut m = Map::new();
    let mut idx = 0;
    for (k, v) in out {
        match k {
            Some(key) => {
                m.insert(key, v);
            }
            None => {
                m.insert(idx.to_string(), v);
                idx += 1;
            }
        }
    }
    Value::Object(m)
}

/// `is_int($key)` of a decoded array: list, or canonical numeric key.
fn php_entries(v: &Value) -> Vec<(Option<String>, Value)> {
    match v {
        Value::Array(items) => items.iter().map(|x| (None, x.clone())).collect(),
        Value::Object(m) => m
            .iter()
            .map(|(k, x)| {
                let is_int = k == "0"
                    || (k.starts_with(['1', '2', '3', '4', '5', '6', '7', '8', '9'])
                        && k.bytes().all(|c| c.is_ascii_digit()));
                (if is_int { None } else { Some(k.clone()) }, x.clone())
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn php_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !(s.is_empty() || s == "0"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

fn opt_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

/// `(bool) ($config['x'] ?? $default)`.
fn bool_or(m: &Map<String, Value>, key: &str, default: bool) -> bool {
    m.get(key).map(php_truthy).unwrap_or(default)
}

/// `parseLegacySingleIgnore`.
fn legacy_single(
    key: &Option<String>,
    value: &Value,
) -> Result<(Option<String>, bool, bool), PolicyError> {
    let mut reason = None;
    let mut on_block = true;
    let mut on_audit = true;
    if key.is_none() && value.is_string() {
        // list entry: no reason
    } else if let Some(s) = value.as_str() {
        reason = Some(s.to_owned());
    } else if let Value::Object(v) = value {
        let apply = v.get("apply").and_then(Value::as_str).unwrap_or("all");
        reason = opt_str(v.get("reason"));
        if !["audit", "block", "all"].contains(&apply) {
            return Err(PolicyError(format!(
                "Invalid 'apply' value for '{}': {apply}. Expected 'audit', 'block', or 'all'.",
                key.clone().unwrap_or_default()
            )));
        }
        on_block = apply == "block" || apply == "all";
        on_audit = apply == "audit" || apply == "all";
    }
    Ok((reason, on_block, on_audit))
}

/// `ListPolicyConfig::parseLegacyAuditIgnore`: `audit.ignore` mixes ids
/// and package names (a `/` makes it a name).
fn parse_legacy_audit_ignore(
    config: &Value,
) -> Result<(IgnoreMap, Vec<IgnoreIdRule>), PolicyError> {
    let mut packages: IgnoreMap = Vec::new();
    let mut ids = Vec::new();
    for (key, value) in php_entries(config) {
        let id = match &key {
            Some(k) => k.clone(),
            None => php_to_string(&value),
        };
        let (reason, on_block, on_audit) = legacy_single(&key, &value)?;
        if id.contains('/') {
            push_rule(
                &mut packages,
                IgnorePackageRule {
                    package_name: id,
                    constraint: Constraint::MatchAll,
                    reason,
                    on_block,
                    on_audit,
                },
            );
        } else {
            ids.push(IgnoreIdRule {
                id,
                reason,
                on_block,
                on_audit,
            });
        }
    }
    Ok((packages, ids))
}

/// `parseLegacyIgnoreWithApply` (abandoned packages).
fn parse_legacy_ignore_with_apply(config: &Value) -> Result<IgnoreMap, PolicyError> {
    let mut out: IgnoreMap = Vec::new();
    for (key, value) in php_entries(config) {
        let name = match &key {
            Some(k) => k.clone(),
            None => php_to_string(&value),
        };
        let (reason, on_block, on_audit) = legacy_single(&key, &value)?;
        // `$result[$packageName] = [rule]`: a single rule per name.
        out.retain(|(n, _)| *n != name);
        out.push((
            name.clone(),
            vec![IgnorePackageRule {
                package_name: name,
                constraint: Constraint::MatchAll,
                reason,
                on_block,
                on_audit,
            }],
        ));
    }
    Ok(out)
}

fn php_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(true) => "1".into(),
        _ => String::new(),
    }
}

fn push_rule(map: &mut IgnoreMap, rule: IgnorePackageRule) {
    match map.iter_mut().find(|(n, _)| *n == rule.package_name) {
        Some((_, rules)) => rules.push(rule),
        None => map.push((rule.package_name.clone(), vec![rule])),
    }
}

/// `IgnorePackageRule::parseIgnoreMap`.
fn parse_ignore_map(config: &Value) -> Result<IgnoreMap, PolicyError> {
    let mut rules: IgnoreMap = Vec::new();
    let from_rule_object =
        |name: &str, v: &Map<String, Value>| -> Result<IgnorePackageRule, PolicyError> {
            let constraint = match v.get("constraint") {
                Some(c) => {
                    parse_constraints(&php_to_string(c))
                        .map_err(|e| PolicyError(e.to_string()))?
                        .constraint
                }
                None => Constraint::MatchAll,
            };
            Ok(IgnorePackageRule {
                package_name: name.to_owned(),
                constraint,
                reason: opt_str(v.get("reason")),
                on_block: v.get("on-block").map(php_truthy).unwrap_or(true),
                on_audit: v.get("on-audit").map(php_truthy).unwrap_or(true),
            })
        };
    for (key, value) in php_entries(config) {
        match (&key, &value) {
            (Some(k), Value::Null) => push_rule(
                &mut rules,
                IgnorePackageRule {
                    package_name: k.clone(),
                    constraint: Constraint::MatchAll,
                    reason: None,
                    on_block: true,
                    on_audit: true,
                },
            ),
            (Some(k), Value::String(s)) => push_rule(
                &mut rules,
                IgnorePackageRule {
                    package_name: k.clone(),
                    constraint: Constraint::MatchAll,
                    reason: Some(s.clone()),
                    on_block: true,
                    on_audit: true,
                },
            ),
            (None, Value::String(s)) => push_rule(
                &mut rules,
                IgnorePackageRule {
                    package_name: s.clone(),
                    constraint: Constraint::MatchAll,
                    reason: None,
                    on_block: true,
                    on_audit: true,
                },
            ),
            (Some(k), Value::Array(list)) => {
                // `isset($value[0])` is false for `[]`: an empty rule object.
                if list.is_empty() {
                    let r = from_rule_object(k, &Map::new())?;
                    push_rule(&mut rules, r);
                }
                for rule in list {
                    let Value::Object(o) = rule else {
                        return Err(PolicyError(format!(
                            "Invalid ignore rule for \"{k}\": expected an object, got {}.",
                            php_type(rule)
                        )));
                    };
                    let r = from_rule_object(k, o)?;
                    push_rule(&mut rules, r);
                }
            }
            (Some(k), Value::Object(o)) if o.contains_key("0") => {
                for rule in o.values() {
                    let Value::Object(ro) = rule else {
                        return Err(PolicyError(format!(
                            "Invalid ignore rule for \"{k}\": expected an object, got {}.",
                            php_type(rule)
                        )));
                    };
                    let r = from_rule_object(k, ro)?;
                    push_rule(&mut rules, r);
                }
            }
            (Some(k), Value::Object(o)) => {
                let r = from_rule_object(k, o)?;
                push_rule(&mut rules, r);
            }
            (k, v) => {
                return Err(PolicyError(format!(
                    "Invalid ignore entry at key \"{}\": value of type {} is not a supported shape. Expected null, a reason string, a rule object, or a list of rule objects.",
                    k.clone().unwrap_or_default(),
                    php_type(v)
                )))
            }
        }
    }
    Ok(rules)
}

fn php_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "array",
    }
}

/// `IgnoreIdRule::parseIgnoreIdMap`.
fn parse_ignore_id_map(config: &Value) -> Result<Vec<IgnoreIdRule>, PolicyError> {
    let mut rules: Vec<IgnoreIdRule> = Vec::new();
    let mut set = |rule: IgnoreIdRule| match rules.iter_mut().find(|r| r.id == rule.id) {
        Some(slot) => *slot = rule,
        None => rules.push(rule),
    };
    for (key, value) in php_entries(config) {
        match (&key, &value) {
            (None, Value::String(s)) => set(IgnoreIdRule {
                id: s.clone(),
                reason: None,
                on_block: true,
                on_audit: true,
            }),
            (None, other) => {
                return Err(PolicyError(format!(
                    "Invalid ignore-id entry: expected an advisory ID string, got {}.",
                    php_type(other)
                )))
            }
            (Some(k), Value::Null) => set(IgnoreIdRule {
                id: k.clone(),
                reason: None,
                on_block: true,
                on_audit: true,
            }),
            (Some(k), Value::String(s)) => set(IgnoreIdRule {
                id: k.clone(),
                reason: Some(s.clone()),
                on_block: true,
                on_audit: true,
            }),
            (Some(k), Value::Object(o)) => set(IgnoreIdRule {
                id: k.clone(),
                reason: opt_str(o.get("reason")),
                on_block: o.get("on-block").map(php_truthy).unwrap_or(true),
                on_audit: o.get("on-audit").map(php_truthy).unwrap_or(true),
            }),
            (Some(k), other) => {
                return Err(PolicyError(format!(
                    "Invalid ignore-id entry for \"{k}\": value of type {} is not a supported shape. Expected null, a reason string, or a rule object.",
                    php_type(other)
                )))
            }
        }
    }
    Ok(rules)
}

/// `AdvisoriesPolicyConfig::parseLegacySeverityWithApply`: list of
/// severities, or map severity -> reason / `{apply, reason}`.
fn parse_legacy_ignore_severity(config: &Value) -> Result<Vec<IgnoreSeverityRule>, PolicyError> {
    let mut rules: Vec<IgnoreSeverityRule> = Vec::new();
    for (key, value) in php_entries(config) {
        let severity = match &key {
            Some(k) => k.clone(),
            None => php_to_string(&value),
        };
        let (reason, on_block, on_audit) = legacy_single(&key, &value)?;
        rules.retain(|r| r.severity != severity);
        rules.push(IgnoreSeverityRule {
            severity,
            reason,
            on_block,
            on_audit,
        });
    }
    Ok(rules)
}

/// `IgnoreSeverityRule::parseIgnoreSeverityMap` and the legacy form
/// (`audit.ignore-severity`: list of severities or map severity -> reason).
fn parse_ignore_severity(config: &Value) -> Result<Vec<IgnoreSeverityRule>, PolicyError> {
    let mut rules: Vec<IgnoreSeverityRule> = Vec::new();
    for (key, value) in php_entries(config) {
        let (severity, reason, on_block, on_audit) = match (&key, &value) {
            (None, Value::String(s)) => (s.clone(), None, true, true),
            (Some(k), Value::Null) => (k.clone(), None, true, true),
            (Some(k), Value::String(s)) => (k.clone(), Some(s.clone()), true, true),
            (Some(k), Value::Object(o)) => (
                k.clone(),
                opt_str(o.get("reason")),
                o.get("on-block").map(php_truthy).unwrap_or(true),
                o.get("on-audit").map(php_truthy).unwrap_or(true),
            ),
            (k, other) => {
                return Err(PolicyError(format!(
                "Invalid ignore-severity entry \"{}\": value of type {} is not a supported shape.",
                k.clone().unwrap_or_default(),
                php_type(other)
            )))
            }
        };
        rules.retain(|r| r.severity != severity);
        rules.push(IgnoreSeverityRule {
            severity,
            reason,
            on_block,
            on_audit,
        });
    }
    Ok(rules)
}

impl PolicyConfig {
    /// `PolicyConfig::fromConfig` + the environment variables.
    pub fn from_raw(raw: &RawPolicyConfig) -> Result<PolicyConfig, PolicyError> {
        let policy = raw.effective_policy()?;
        if policy == Value::Bool(false) {
            return Ok(PolicyConfig {
                enabled: false,
                advisories: AdvisoriesPolicy {
                    block: false,
                    ignore: Vec::new(),
                    ignore_id: Vec::new(),
                    ignore_severity: Vec::new(),
                },
                malware: MalwarePolicy {
                    block: false,
                    block_scope: "all".into(),
                    ignore: Vec::new(),
                    ignore_source: Vec::new(),
                },
                abandoned: AbandonedPolicy {
                    block: false,
                    ignore: Vec::new(),
                },
                custom_lists: Vec::new(),
                ignore_unreachable: IgnoreUnreachable {
                    audit: true,
                    install: true,
                    update: true,
                },
            });
        }
        let policy_cfg = match &policy {
            Value::Object(m) => m.clone(),
            _ => Map::new(),
        };
        let audit_cfg = match &raw.audit {
            Value::Object(m) => m.clone(),
            _ => Map::new(),
        };
        let empty_audit = audit_cfg.is_empty();
        let empty = Value::Array(Vec::new());

        // AdvisoriesPolicyConfig::fromRawConfig
        let mut advisories = if !policy_cfg.contains_key("advisories") && !empty_audit {
            let (ignore, ignore_id) =
                parse_legacy_audit_ignore(audit_cfg.get("ignore").unwrap_or(&empty))?;
            AdvisoriesPolicy {
                block: bool_or(&audit_cfg, "block-insecure", true),
                ignore,
                ignore_id,
                ignore_severity: parse_legacy_ignore_severity(
                    audit_cfg.get("ignore-severity").unwrap_or(&empty),
                )?,
            }
        } else {
            match policy_cfg.get("advisories") {
                Some(Value::Bool(false)) => AdvisoriesPolicy {
                    block: false,
                    ignore: Vec::new(),
                    ignore_id: Vec::new(),
                    ignore_severity: Vec::new(),
                },
                other => {
                    let cfg = match other {
                        Some(Value::Object(m)) => m.clone(),
                        _ => Map::new(),
                    };
                    AdvisoriesPolicy {
                        block: bool_or(&cfg, "block", true),
                        ignore: parse_ignore_map(cfg.get("ignore").unwrap_or(&empty))?,
                        ignore_id: parse_ignore_id_map(cfg.get("ignore-id").unwrap_or(&empty))?,
                        ignore_severity: parse_ignore_severity(
                            cfg.get("ignore-severity").unwrap_or(&empty),
                        )?,
                    }
                }
            }
        };
        // MalwarePolicyConfig::fromRawConfig
        let mut malware = match policy_cfg.get("malware") {
            Some(Value::Bool(false)) => MalwarePolicy {
                block: false,
                block_scope: "all".into(),
                ignore: Vec::new(),
                ignore_source: Vec::new(),
            },
            other => {
                let cfg = match other {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                };
                MalwarePolicy {
                    block: bool_or(&cfg, "block", true),
                    block_scope: opt_str(cfg.get("block-scope")).unwrap_or_else(|| "all".into()),
                    ignore: parse_ignore_map(cfg.get("ignore").unwrap_or(&empty))?,
                    ignore_source: cfg
                        .get("ignore-source")
                        .map(|v| {
                            php_entries(v)
                                .into_iter()
                                .map(|(_, x)| php_to_string(&x))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }
        };
        // AbandonedPolicyConfig::fromRawConfig
        let mut abandoned = if !policy_cfg.contains_key("abandoned") && !empty_audit {
            AbandonedPolicy {
                block: bool_or(&audit_cfg, "block-abandoned", false),
                ignore: parse_legacy_ignore_with_apply(
                    audit_cfg.get("ignore-abandoned").unwrap_or(&empty),
                )?,
            }
        } else {
            match policy_cfg.get("abandoned") {
                Some(Value::Bool(false)) => AbandonedPolicy {
                    block: false,
                    ignore: Vec::new(),
                },
                other => {
                    let cfg = match other {
                        Some(Value::Object(m)) => m.clone(),
                        _ => Map::new(),
                    };
                    AbandonedPolicy {
                        block: bool_or(&cfg, "block", false),
                        ignore: parse_ignore_map(cfg.get("ignore").unwrap_or(&empty))?,
                    }
                }
            }
        };
        let custom_lists: Vec<String> = policy_cfg
            .keys()
            .filter(|k| {
                !BUILTIN_LIST_NAMES.contains(&k.as_str()) && !NON_LIST_KEYS.contains(&k.as_str())
            })
            .cloned()
            .collect();
        let ignore_unreachable = if let Some(v) = policy_cfg.get("ignore-unreachable") {
            match v {
                Value::Array(list) => {
                    let has = |s: &str| list.iter().any(|x| x.as_str() == Some(s));
                    IgnoreUnreachable {
                        audit: has("audit"),
                        install: has("install"),
                        update: has("update"),
                    }
                }
                other if php_truthy(other) => IgnoreUnreachable {
                    audit: true,
                    install: true,
                    update: true,
                },
                _ => IgnoreUnreachable {
                    audit: false,
                    install: false,
                    update: false,
                },
            }
        } else if audit_cfg.get("ignore-unreachable").is_some_and(php_truthy) {
            IgnoreUnreachable {
                audit: true,
                install: false,
                update: false,
            }
        } else {
            IgnoreUnreachable {
                audit: false,
                install: true,
                update: true,
            }
        };
        // `COMPOSER_AUDIT_ABANDONED`: no effect on blocking, but an invalid
        // value is fatal in Composer.
        if let Ok(v) = std::env::var("COMPOSER_AUDIT_ABANDONED") {
            if !["ignore", "report", "fail"].contains(&v.as_str()) {
                return Err(PolicyError(format!(
                    "Invalid value for COMPOSER_AUDIT_ABANDONED: {v}. Expected one of ignore, report, fail."
                )));
            }
        }
        if let Some(b) = bool_env("COMPOSER_POLICY_ADVISORIES_BLOCK")? {
            advisories.block = b;
        }
        if let Some(b) = bool_env("COMPOSER_POLICY_MALWARE_BLOCK")? {
            malware.block = b;
        }
        let abandoned_env = match bool_env("COMPOSER_POLICY_ABANDONED_BLOCK")? {
            Some(b) => Some(b),
            None => bool_env("COMPOSER_SECURITY_BLOCKING_ABANDONED")?,
        };
        if let Some(b) = abandoned_env {
            abandoned.block = b;
        }
        Ok(PolicyConfig {
            enabled: true,
            advisories,
            malware,
            abandoned,
            custom_lists,
            ignore_unreachable,
        })
    }

    /// `BaseCommand::createPolicyConfig`: `--no-blocking`,
    /// `--no-security-blocking`, `COMPOSER_NO_BLOCKING`,
    /// `COMPOSER_NO_SECURITY_BLOCKING` -> `withBlockingDisabled`.
    pub fn apply_no_blocking(&mut self, option: bool) -> Result<(), PolicyError> {
        let no_blocking = option
            || bool_env("COMPOSER_NO_BLOCKING")?.unwrap_or(false)
            || bool_env("COMPOSER_NO_SECURITY_BLOCKING")?.unwrap_or(false);
        if no_blocking {
            self.advisories.block = false;
            self.malware.block = false;
            self.abandoned.block = false;
        }
        Ok(())
    }

    /// `ListPolicyConfig::shouldBlock` for the malware list.
    pub fn malware_blocks(&self, scope: &str) -> bool {
        if !self.malware.block {
            return false;
        }
        match self.malware.block_scope.as_str() {
            "all" => true,
            s => s == scope,
        }
    }
}

/// `AdvisoriesPolicyConfig::getIgnoreListForOperation('block')`: ids then
/// names -> reason (a PHP map: `array_key_exists`).
pub fn advisory_ignore_list_for_block(p: &AdvisoriesPolicy) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut set = |key: String, reason: Option<String>| {
        match out.iter_mut().find(|(k, _)| *k == key) {
            // `mergeReason`: the first known reason is kept if the new one
            // is empty.
            Some(slot) => {
                if reason.is_some() {
                    slot.1 = reason;
                }
            }
            None => out.push((key, reason)),
        }
    };
    for r in &p.ignore_id {
        if r.on_block {
            set(r.id.clone(), r.reason.clone());
        }
    }
    for (name, rules) in &p.ignore {
        for r in rules {
            if r.on_block {
                set(name.clone(), r.reason.clone());
            }
        }
    }
    out
}

/// `getIgnoreSeverityForOperation('block')`.
pub fn advisory_ignore_severity_for_block(p: &AdvisoriesPolicy) -> Vec<(String, Option<String>)> {
    p.ignore_severity
        .iter()
        .filter(|r| r.on_block)
        .map(|r| (r.severity.clone(), r.reason.clone()))
        .collect()
}

/// `getFlatIgnoreForOperation('block')` of a list (abandoned, malware):
/// name -> reason.
pub fn flat_ignore_for_block(map: &IgnoreMap) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for (name, rules) in map {
        for r in rules {
            if r.on_block {
                match out.iter_mut().find(|(k, _)| k == name) {
                    Some(slot) => {
                        if r.reason.is_some() {
                            slot.1 = r.reason.clone();
                        }
                    }
                    None => out.push((name.clone(), r.reason.clone())),
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_block_advisories_and_malware_only() {
        let p = PolicyConfig::from_raw(&RawPolicyConfig::default()).expect("policy");
        assert!(p.enabled);
        assert!(p.advisories.block);
        assert!(p.malware.block);
        assert!(!p.abandoned.block);
        assert_eq!(
            p.ignore_unreachable,
            IgnoreUnreachable {
                audit: false,
                install: true,
                update: true
            }
        );
    }

    #[test]
    fn legacy_audit_config_is_the_default_path() {
        let mut raw = RawPolicyConfig::default();
        let cfg: Map<String, Value> = serde_json::from_str(
            r#"{"audit": {"ignore": ["CVE-2024-1", "acme/lib"], "block-insecure": false, "block-abandoned": true, "ignore-abandoned": {"old/pkg": "meh"}}}"#,
        )
        .expect("json");
        raw.merge(&cfg);
        let p = PolicyConfig::from_raw(&raw).expect("policy");
        assert!(!p.advisories.block);
        assert_eq!(p.advisories.ignore_id[0].id, "CVE-2024-1");
        assert_eq!(p.advisories.ignore[0].0, "acme/lib");
        assert!(p.abandoned.block);
        assert_eq!(p.abandoned.ignore[0].1[0].reason.as_deref(), Some("meh"));
    }

    #[test]
    fn policy_merge_global_then_project() {
        let mut raw = RawPolicyConfig::default();
        let global: Map<String, Value> = serde_json::from_str(
            r#"{"policy": {"advisories": {"ignore": {"a/b": null}}, "malware": {"ignore-source": ["x"]}}}"#,
        )
        .expect("json");
        let project: Map<String, Value> = serde_json::from_str(
            r#"{"policy": {"advisories": {"ignore": {"c/d": "why"}, "block": false}, "abandoned": true, "malware": false}}"#,
        )
        .expect("json");
        raw.merge(&global);
        raw.merge(&project);
        let p = PolicyConfig::from_raw(&raw).expect("policy");
        assert!(!p.advisories.block);
        let names: Vec<&str> = p
            .advisories
            .ignore
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(names, ["a/b", "c/d"]);
        assert!(!p.malware.block);
        assert!(!p.abandoned.block);
        assert!(p.custom_lists.is_empty());
    }

    #[test]
    fn audit_ignore_lists_concatenate() {
        let mut raw = RawPolicyConfig::default();
        let g: Map<String, Value> =
            serde_json::from_str(r#"{"audit": {"ignore": ["CVE-1"]}}"#).expect("json");
        let p: Map<String, Value> =
            serde_json::from_str(r#"{"audit": {"ignore": {"CVE-2": "r"}}}"#).expect("json");
        raw.merge(&g);
        raw.merge(&p);
        let cfg = PolicyConfig::from_raw(&raw).expect("policy");
        let ids: Vec<&str> = cfg
            .advisories
            .ignore_id
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids, ["CVE-1", "CVE-2"]);
    }
}
