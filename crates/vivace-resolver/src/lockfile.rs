//! Écriture du lock : port d'`ArrayDumper::dump` (depuis les métadonnées
//! brutes et le modèle, avec les normalisations d'`ArrayLoader`), de
//! `Locker::lockPackages` et de `Locker::setLockData` ; encodage JsonFile.

use crate::package::Package;
use crate::root::RootAlias;
use crate::version::DEFAULT_BRANCH_ALIAS;
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::sync::OnceLock;

/// `empty()` PHP.
pub fn php_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => true,
        Some(Value::String(s)) => s.is_empty() || s == "0",
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Bool(true)) => false,
    }
}

/// `is_array` PHP sur du JSON décodé en tableau associatif.
fn is_array(v: &Value) -> bool {
    matches!(v, Value::Array(_) | Value::Object(_))
}

/// Comparaison PHP 8 de deux chaînes (`sort`/`strcmp` : les chaînes
/// numériques se comparent en nombres, les autres octet à octet).
pub fn php_compare_strings(a: &str, b: &str) -> Ordering {
    fn numeric(s: &str) -> Option<f64> {
        let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
        let t = t.trim_end_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
        if t.is_empty() {
            return None;
        }
        let bytes = t.as_bytes();
        let ok = bytes
            .iter()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'));
        if !ok || !bytes[0].is_ascii_digit() && !matches!(bytes[0], b'.' | b'+' | b'-') {
            return None;
        }
        t.parse::<f64>().ok()
    }
    match (numeric(a), numeric(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => a.as_bytes().cmp(b.as_bytes()),
    }
}

/// Tri par insertion stable (zend_insert_sort pour n ≤ 16 ; au-delà
/// zend_sort devient hybride et un ordre non total peut différer).
fn insertion_sort<T>(items: &mut [T], cmp: impl Fn(&T, &T) -> Ordering) {
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 && cmp(&items[j - 1], &items[j]) == Ordering::Greater {
            items.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// `Package::getTargetDir()`.
fn normalize_target_dir(dir: &str) -> String {
    static RE: OnceLock<pcre2::bytes::Regex> = OnceLock::new();
    let re = crate::version::regex(
        &RE,
        r"(?:^|[\\/]+)\.\.?(?:[\\/]+|$)(?:\.\.?(?:[\\/]+|$))*",
        false,
    );
    let mut out = String::new();
    let mut last = 0;
    for m in re.find_iter(dir.as_bytes()).flatten() {
        out.push_str(&dir[last..m.start()]);
        out.push('/');
        last = m.end();
    }
    out.push_str(&dir[last..]);
    out.trim_start_matches('/').to_owned()
}

fn ksort(map: &Map<String, Value>) -> Map<String, Value> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    insertion_sort(&mut entries, |(a, _), (b, _)| php_compare_strings(a, b));
    entries
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// `ArrayLoader` : `new \DateTime($time, UTC)` puis `format(DATE_RFC3339)`.
/// Les formes reconnues : horodatage entier, ISO 8601 avec ou sans fuseau,
/// `Y-m-d H:i:s`, `Y-m-d`. Une forme inconnue est ignorée (exception avalée).
pub fn release_date(time: &Value) -> Option<String> {
    let text = match time {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if php_empty(Some(time)) {
        return None;
    }
    if text.bytes().all(|c| c.is_ascii_digit()) {
        let ts: i64 = text.parse().ok()?;
        return Some(format_rfc3339(civil_from_unix(ts), "+00:00"));
    }
    parse_datetime(&text)
}

/// (année, mois, jour, heure, minute, seconde).
type Civil = (i64, u32, u32, u32, u32, u32);

fn civil_from_unix(ts: i64) -> Civil {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Algorithme de Howard Hinnant (days → civil).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        ((secs % 3600) / 60) as u32,
        (secs % 60) as u32,
    )
}

fn format_rfc3339(c: Civil, offset: &str) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{offset}",
        c.0, c.1, c.2, c.3, c.4, c.5
    )
}

fn parse_datetime(text: &str) -> Option<String> {
    let t = text.trim();
    let b = t.as_bytes();
    let num = |i: usize, n: usize| -> Option<u32> {
        if b.len() < i + n || !b[i..i + n].iter().all(u8::is_ascii_digit) {
            return None;
        }
        t[i..i + n].parse().ok()
    };
    let year = num(0, 4)? as i64;
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let month = num(5, 2)?;
    let day = num(8, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut pos = 10;
    let (mut h, mut mi, mut s) = (0, 0, 0);
    if let Some(&sep) = b.get(pos) {
        if sep != b'T' && sep != b' ' {
            return None;
        }
        pos += 1;
        h = num(pos, 2)?;
        if b.get(pos + 2) != Some(&b':') {
            return None;
        }
        mi = num(pos + 3, 2)?;
        pos += 5;
        if b.get(pos) == Some(&b':') {
            s = num(pos + 1, 2)?;
            pos += 3;
        }
        if h > 23 || mi > 59 || s > 60 {
            return None;
        }
        // Fraction de seconde ignorée par le format de sortie.
        if b.get(pos) == Some(&b'.') {
            pos += 1;
            while b.get(pos).is_some_and(u8::is_ascii_digit) {
                pos += 1;
            }
        }
    }
    let rest = t[pos..].trim_start();
    let offset = match rest {
        "" => "+00:00".to_owned(),
        "Z" | "z" | "UTC" | "GMT" => "+00:00".to_owned(),
        r if (r.starts_with('+') || r.starts_with('-')) && r.len() >= 3 && r.len() <= 6 => {
            let sign = &r[..1];
            let digits: String = r[1..].chars().filter(|c| *c != ':').collect();
            if !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let (oh, om) = match digits.len() {
                2 => (digits.parse::<u32>().ok()?, 0),
                4 => (
                    digits[..2].parse::<u32>().ok()?,
                    digits[2..].parse::<u32>().ok()?,
                ),
                _ => return None,
            };
            format!("{sign}{oh:02}:{om:02}")
        }
        _ => return None,
    };
    Some(format_rfc3339((year, month, day, h, mi, s), &offset))
}

/// `ArrayDumper::dump($package)` pour un paquet non racine.
pub fn dump_package(p: &Package) -> Map<String, Value> {
    let raw = p.raw.as_object().cloned().unwrap_or_default();
    let mut data = Map::new();
    data.insert("name".into(), Value::String(p.pretty_name.clone()));
    data.insert("version".into(), Value::String(p.pretty_version.clone()));
    data.insert(
        "version_normalized".into(),
        Value::String(p.version.clone()),
    );
    if let Some(td) = raw.get("target-dir").filter(|v| !v.is_null()) {
        // `Package::getTargetDir()` : segments `.`/`..` et slashes de tête
        // retirés.
        let text = match td {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        data.insert(
            "target-dir".into(),
            Value::String(normalize_target_dir(&text)),
        );
    }
    if let Some(src) = &p.source {
        let mut s = Map::new();
        s.insert("type".into(), Value::String(src.kind.clone()));
        s.insert("url".into(), Value::String(src.url.clone()));
        if let Some(r) = &src.reference {
            s.insert("reference".into(), Value::String(r.clone()));
        }
        if let Some(m) = raw.get("source").and_then(|s| s.get("mirrors")) {
            if !php_empty(Some(m)) {
                s.insert("mirrors".into(), m.clone());
            }
        }
        data.insert("source".into(), Value::Object(s));
    }
    if let Some(dist) = &p.dist {
        let mut d = Map::new();
        d.insert("type".into(), Value::String(dist.kind.clone()));
        // `setDistUrl('')` range null.
        d.insert(
            "url".into(),
            if dist.url.is_empty() {
                Value::Null
            } else {
                Value::String(dist.url.clone())
            },
        );
        if let Some(r) = &dist.reference {
            d.insert("reference".into(), Value::String(r.clone()));
        }
        if let Some(shasum) = raw.get("dist").and_then(|s| s.get("shasum")) {
            if !shasum.is_null() {
                d.insert("shasum".into(), shasum.clone());
            }
        }
        if let Some(m) = raw.get("dist").and_then(|s| s.get("mirrors")) {
            if !php_empty(Some(m)) {
                d.insert("mirrors".into(), m.clone());
            }
        }
        data.insert("dist".into(), Value::Object(d));
    }
    for (key, links) in [
        ("require", &p.requires),
        ("conflict", &p.conflicts),
        ("provide", &p.provides),
        ("replace", &p.replaces),
        ("require-dev", &p.dev_requires),
    ] {
        if links.is_empty() {
            continue;
        }
        let mut m = Map::new();
        for l in links.iter() {
            m.insert(l.target.clone(), Value::String(l.pretty_constraint.clone()));
        }
        data.insert(key.into(), Value::Object(ksort(&m)));
    }
    if let Some(suggest) = raw.get("suggest").filter(|v| is_array(v)) {
        let self_version = |v: &Value| match v {
            Value::String(s) if s.trim() == "self.version" => {
                Value::String(p.pretty_version.clone())
            }
            other => other.clone(),
        };
        let value = match suggest {
            Value::Object(o) => {
                let m: Map<String, Value> = o
                    .iter()
                    .map(|(k, v)| (k.clone(), self_version(v)))
                    .collect();
                Value::Object(ksort(&m))
            }
            Value::Array(a) => Value::Array(a.iter().map(self_version).collect()),
            _ => Value::Null,
        };
        if !php_empty(Some(&value)) {
            data.insert("suggest".into(), value);
        }
    }
    if let Some(time) = raw.get("time").and_then(release_date) {
        data.insert("time".into(), Value::String(time));
    }
    if p.is_default_branch {
        data.insert("default-branch".into(), Value::Bool(true));
    }
    // dumpValues : bin, type, extra, installation-source, autoload,
    // autoload-dev, notification-url, include-path, php-ext.
    if let Some(bin) = raw.get("bin").filter(|v| !v.is_null()) {
        // Tableau PHP : les clés d'un objet sont conservées.
        let ltrim = |v: Value| match v {
            Value::String(s) => Value::String(s.trim_start_matches('/').to_owned()),
            other => other,
        };
        let value = match bin {
            Value::Array(a) => Value::Array(a.iter().cloned().map(ltrim).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), ltrim(v.clone())))
                    .collect(),
            ),
            other => Value::Array(vec![ltrim(other.clone())]),
        };
        if !php_empty(Some(&value)) {
            data.insert("bin".into(), value);
        }
    }
    // `getType()` : `$this->type ?: 'library'`.
    data.insert(
        "type".into(),
        Value::String(if p.package_type.is_empty() || p.package_type == "0" {
            "library".to_owned()
        } else {
            p.package_type.clone()
        }),
    );
    if let Some(extra) = raw.get("extra") {
        if is_array(extra) && !php_empty(Some(extra)) {
            data.insert("extra".into(), extra.clone());
        }
    }
    if let Some(v) = raw.get("installation-source").filter(|v| !v.is_null()) {
        data.insert("installation-source".into(), v.clone());
    }
    for key in ["autoload", "autoload-dev"] {
        if let Some(v) = raw.get(key) {
            if is_array(v) && !php_empty(Some(v)) {
                data.insert(key.into(), v.clone());
            }
        }
    }
    if let Some(v) = raw.get("notification-url") {
        if !php_empty(Some(v)) {
            data.insert("notification-url".into(), v.clone());
        }
    }
    if let Some(v) = raw.get("include-path") {
        if is_array(v) && !php_empty(Some(v)) {
            data.insert("include-path".into(), v.clone());
        }
    }
    if let Some(v) = raw.get("php-ext") {
        if is_array(v) && !php_empty(Some(v)) {
            data.insert("php-ext".into(), v.clone());
        }
    }
    // CompletePackage.
    let mut archive = Map::new();
    if let Some(name) = raw.get("archive").and_then(|a| a.get("name")) {
        if !php_empty(Some(name)) {
            archive.insert("name".into(), name.clone());
        }
    }
    if let Some(exclude) = raw.get("archive").and_then(|a| a.get("exclude")) {
        if !php_empty(Some(exclude)) {
            archive.insert("exclude".into(), exclude.clone());
        }
    }
    if !archive.is_empty() {
        data.insert("archive".into(), Value::Object(archive));
    }
    if let Some(scripts) = raw.get("scripts").filter(|v| is_array(v)) {
        // `(array) $listeners` : un tableau garde ses clés, un scalaire est
        // enveloppé, null devient vide.
        let cast = |listeners: &Value| match listeners {
            Value::Array(_) | Value::Object(_) => listeners.clone(),
            Value::Null => Value::Array(Vec::new()),
            other => Value::Array(vec![other.clone()]),
        };
        let value = match scripts {
            Value::Object(o) => {
                Value::Object(o.iter().map(|(k, v)| (k.clone(), cast(v))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(cast).collect()),
            _ => Value::Null,
        };
        if !php_empty(Some(&value)) {
            data.insert("scripts".into(), value);
        }
    }
    if let Some(license) = raw.get("license") {
        if !php_empty(Some(license)) {
            let list = match license {
                Value::Array(a) => Value::Array(a.clone()),
                Value::Object(o) => Value::Object(o.clone()),
                other => Value::Array(vec![other.clone()]),
            };
            data.insert("license".into(), list);
        }
    }
    if let Some(authors) = raw.get("authors") {
        if is_array(authors) && !php_empty(Some(authors)) {
            data.insert("authors".into(), authors.clone());
        }
    }
    if let Some(Value::String(d)) = raw.get("description") {
        if !d.is_empty() && d != "0" {
            data.insert("description".into(), Value::String(d.clone()));
        }
    }
    if let Some(Value::String(h)) = raw.get("homepage") {
        if !h.is_empty() && h != "0" {
            data.insert("homepage".into(), Value::String(h.clone()));
        }
    }
    if let Some(keywords) = raw.get("keywords") {
        if is_array(keywords) && !php_empty(Some(keywords)) {
            let list: Vec<Value> = match keywords {
                Value::Array(a) => a.clone(),
                Value::Object(o) => o.values().cloned().collect(),
                _ => Vec::new(),
            };
            let mut strings: Vec<String> = list
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(true) => "1".to_owned(),
                    Value::Bool(false) | Value::Null => String::new(),
                    Value::Number(n) => match n.as_f64() {
                        Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 => {
                            format!("{}", f as i64)
                        }
                        _ => n.to_string(),
                    },
                    other => other.to_string(),
                })
                .collect();
            // `sort()` : la comparaison PHP n'est pas un ordre total sur des
            // chaînes mixtes ; un tri par insertion (celui de zend_sort en
            // dessous de 17 éléments) ne suppose rien et ne panique pas.
            insertion_sort(&mut strings, |a, b| php_compare_strings(a, b));
            data.insert(
                "keywords".into(),
                Value::Array(strings.into_iter().map(Value::String).collect()),
            );
        }
    }
    if let Some(support) = raw.get("support") {
        if is_array(support) && !php_empty(Some(support)) {
            data.insert("support".into(), support.clone());
        }
    }
    if let Some(funding) = raw.get("funding") {
        if is_array(funding) && !php_empty(Some(funding)) {
            data.insert("funding".into(), funding.clone());
        }
    }
    match raw.get("abandoned") {
        Some(Value::String(s)) if !s.is_empty() && s != "0" => {
            data.insert("abandoned".into(), Value::String(s.clone()));
        }
        Some(v) if !php_empty(Some(v)) && !v.is_string() => {
            data.insert("abandoned".into(), Value::Bool(true));
        }
        _ => {}
    }
    // `transport-options` : celles du lock (`loadOptions`) ou posées par le
    // dépôt (`configurePackageTransportOptions`) ; le dépôt a déjà retiré
    // celles des métadonnées.
    if let Some(t) = raw.get("transport-options") {
        if is_array(t) && !php_empty(Some(t)) {
            data.insert("transport-options".into(), t.clone());
        }
    }
    data
}

/// `Locker::lockPackages`.
pub fn lock_packages(arena: &[Package], packages: &[usize]) -> Result<Vec<Value>, String> {
    let mut locked: Vec<Map<String, Value>> = Vec::new();
    for &idx in packages {
        let p = &arena[idx];
        if p.is_alias() {
            continue;
        }
        if php_empty(Some(&Value::String(p.pretty_name.clone())))
            || php_empty(Some(&Value::String(p.pretty_version.clone())))
        {
            return Err(format!(
                "Package \"{}\" has no version or name and can not be locked",
                p.pretty_string()
            ));
        }
        let mut spec = dump_package(p);
        spec.shift_remove("version_normalized");
        let time = spec.shift_remove("time");
        if let Some(t) = time {
            spec.insert("time".into(), t);
        }
        spec.shift_remove("installation-source");
        locked.push(spec);
    }
    locked.sort_by(|a, b| {
        let (an, bn) = (
            a["name"].as_str().unwrap_or(""),
            b["name"].as_str().unwrap_or(""),
        );
        an.as_bytes().cmp(bn.as_bytes()).then_with(|| {
            a["version"]
                .as_str()
                .unwrap_or("")
                .as_bytes()
                .cmp(b["version"].as_str().unwrap_or("").as_bytes())
        })
    });
    Ok(locked.into_iter().map(Value::Object).collect())
}

pub struct LockInput<'a> {
    pub content_hash: &'a str,
    pub packages: Vec<Value>,
    pub packages_dev: Option<Vec<Value>>,
    pub platform: Map<String, Value>,
    pub platform_dev: Map<String, Value>,
    pub aliases: &'a [RootAlias],
    pub minimum_stability: &'a str,
    pub stability_flags: &'a std::collections::BTreeMap<String, i32>,
    pub prefer_stable: bool,
    pub prefer_lowest: bool,
    pub platform_overrides: &'a Map<String, Value>,
}

/// `Locker::setLockData` : les données du lock (avant écriture).
pub fn lock_data(input: LockInput<'_>) -> Value {
    let aliases: Vec<Value> = input
        .aliases
        .iter()
        .map(|a| {
            let version =
                if ["dev-master", "dev-trunk", "dev-default"].contains(&a.version.as_str()) {
                    DEFAULT_BRANCH_ALIAS.to_owned()
                } else {
                    a.version.clone()
                };
            let mut m = Map::new();
            m.insert("package".into(), Value::String(a.package.clone()));
            m.insert("version".into(), Value::String(version));
            m.insert("alias".into(), Value::String(a.alias.clone()));
            m.insert(
                "alias_normalized".into(),
                Value::String(a.alias_normalized.clone()),
            );
            Value::Object(m)
        })
        .collect();
    let mut lock = Map::new();
    lock.insert(
        "_readme".into(),
        Value::Array(vec![
            Value::String("This file locks the dependencies of your project to a known state".into()),
            Value::String(
                "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies".into(),
            ),
            Value::String("This file is @generated automatically".into()),
        ]),
    );
    lock.insert(
        "content-hash".into(),
        Value::String(input.content_hash.to_owned()),
    );
    lock.insert("packages".into(), Value::Array(input.packages));
    lock.insert(
        "packages-dev".into(),
        match input.packages_dev {
            Some(p) => Value::Array(p),
            None => Value::Null,
        },
    );
    lock.insert("aliases".into(), Value::Array(aliases));
    lock.insert(
        "minimum-stability".into(),
        Value::String(input.minimum_stability.to_owned()),
    );
    // fixupJsonDataType : ksort + `{}` si vide.
    let mut flags = Map::new();
    for (k, v) in input.stability_flags {
        flags.insert(k.clone(), Value::Number((*v).into()));
    }
    let object_or_stdclass = |m: Map<String, Value>| -> Value {
        if m.is_empty() {
            vivace_core::phpjson::empty_stdclass()
        } else {
            Value::Object(m)
        }
    };
    lock.insert("stability-flags".into(), object_or_stdclass(ksort(&flags)));
    lock.insert("prefer-stable".into(), Value::Bool(input.prefer_stable));
    lock.insert("prefer-lowest".into(), Value::Bool(input.prefer_lowest));
    lock.insert("platform".into(), object_or_stdclass(input.platform));
    lock.insert(
        "platform-dev".into(),
        object_or_stdclass(input.platform_dev),
    );
    if !input.platform_overrides.is_empty() {
        lock.insert(
            "platform-overrides".into(),
            Value::Object(input.platform_overrides.clone()),
        );
    }
    lock.insert(
        "plugin-api-version".into(),
        Value::String(crate::platform::PLUGIN_API_VERSION.to_owned()),
    );
    Value::Object(lock)
}

/// `ValidatingArrayLoader::validatePackage` (appelé sur chaque paquet
/// retenu par le solveur) : refuse les noms invalides ou réservés, les
/// URL/références commençant par `-` (injection d'arguments) et les `bin`
/// avec `..`.
pub fn validate_package(p: &Package) -> Result<(), String> {
    static DASH: OnceLock<pcre2::bytes::Regex> = OnceLock::new();
    static DOTDOT: OnceLock<pcre2::bytes::Regex> = OnceLock::new();
    if matches!(p.origin, crate::package::Origin::Root) {
        return Ok(());
    }
    if let Some(err) = package_naming_error(&p.name) {
        return Err(format!(
            "Invalid package found during dependency resolution, aborting: {err}"
        ));
    }
    let dash = crate::version::regex(&DASH, r"^\s*-", false);
    let fields: [(&str, Option<&str>); 4] = [
        ("source.url", p.source.as_ref().map(|s| s.url.as_str())),
        (
            "source.reference",
            p.source.as_ref().and_then(|s| s.reference.as_deref()),
        ),
        (
            "dist.url",
            p.dist
                .as_ref()
                .map(|d| d.url.as_str())
                .filter(|u| !u.is_empty()),
        ),
        (
            "dist.reference",
            p.dist.as_ref().and_then(|d| d.reference.as_deref()),
        ),
    ];
    for (field, value) in fields {
        if let Some(v) = value {
            if dash.is_match(v.as_bytes()).unwrap_or(false) {
                return Err(format!(
                    "{} has an invalid {field}, it must not start with a \"-\": {v}",
                    p.name
                ));
            }
        }
    }
    let dotdot = crate::version::regex(&DOTDOT, r"(?:^|[\\/])\.\.(?:[\\/]|$)", false);
    if let Some(bins) = p.raw.get("bin") {
        let list: Vec<String> = match bins {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            Value::Object(o) => o
                .values()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        for bin in list {
            if dotdot.is_match(bin.as_bytes()).unwrap_or(false) {
                return Err(format!(
                    "{} has an invalid bin {bin}, it must not contain \"..\" path segments",
                    p.name
                ));
            }
        }
    }
    Ok(())
}

/// `ValidatingArrayLoader::hasPackageNamingError($name)` (hors liens).
fn package_naming_error(name: &str) -> Option<String> {
    static NAME: OnceLock<pcre2::bytes::Regex> = OnceLock::new();
    if crate::platform::is_platform_package(name) {
        return None;
    }
    let re = crate::version::regex(
        &NAME,
        r"^[a-z0-9](?:[_.-]?[a-z0-9]++)*+/[a-z0-9](?:(?:[_.]|-{1,2})?[a-z0-9]++)*+\z",
        true,
    );
    if !re.is_match(name.as_bytes()).unwrap_or(false) {
        return Some(format!(
            "{name} is invalid, it should have a vendor name, a forward slash, and a package name. The vendor and package name can be words separated by -, . or _. The complete name should match \"^[a-z0-9]([_.-]?[a-z0-9]+)*/[a-z0-9](([_.]?|-{{0,2}})[a-z0-9]+)*$\"."
        ));
    }
    const RESERVED: &[&str] = &[
        "nul", "con", "prn", "aux", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let lower = name.to_lowercase();
    let mut bits = lower.splitn(2, '/');
    let vendor = bits.next().unwrap_or("");
    let package = bits.next().unwrap_or("");
    if RESERVED.contains(&vendor) || RESERVED.contains(&package) {
        return Some(format!(
            "{name} is reserved, package and vendor names can not match any of: {}.",
            RESERVED.join(", ")
        ));
    }
    if name.ends_with(".json") {
        return Some(format!(
            "{name} is invalid, package names can not end in .json, consider renaming it or perhaps using a -json suffix instead."
        ));
    }
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        return Some(format!(
            "{name} is invalid, it should not contain uppercase characters. We suggest using {} instead.",
            name.to_lowercase()
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_reformatted_like_datetime() {
        assert_eq!(
            release_date(&Value::String("2020-11-13T09:40:50+00:00".into())).as_deref(),
            Some("2020-11-13T09:40:50+00:00")
        );
        assert_eq!(
            release_date(&Value::String("2020-11-13 09:40:50".into())).as_deref(),
            Some("2020-11-13T09:40:50+00:00")
        );
        assert_eq!(
            release_date(&Value::String("2020-11-13T09:40:50Z".into())).as_deref(),
            Some("2020-11-13T09:40:50+00:00")
        );
        assert_eq!(
            release_date(&Value::String("2020-11-13T09:40:50+0200".into())).as_deref(),
            Some("2020-11-13T09:40:50+02:00")
        );
        assert_eq!(
            release_date(&Value::String("2020-11-13".into())).as_deref(),
            Some("2020-11-13T00:00:00+00:00")
        );
        assert_eq!(
            release_date(&Value::String("1605260450".into())).as_deref(),
            Some("2020-11-13T09:40:50+00:00")
        );
        assert_eq!(
            release_date(&Value::String("2020-11-13 09:40:50 UTC".into())).as_deref(),
            Some("2020-11-13T09:40:50+00:00")
        );
        assert_eq!(
            release_date(&Value::String("2020-11-13 09:40:50 +0100".into())).as_deref(),
            Some("2020-11-13T09:40:50+01:00")
        );
        assert_eq!(normalize_target_dir("../foo/./bar/"), "foo/bar/");
        assert_eq!(normalize_target_dir("/Foo"), "Foo");
        assert_eq!(release_date(&Value::String("yesterday".into())), None);
        assert_eq!(release_date(&Value::String(String::new())), None);
    }

    #[test]
    fn validates_packages_like_composer() {
        use crate::package::{Origin, Package, SourceRef};
        let mut p = Package::new("acme/lib", "1.0.0.0", "1.0.0", Origin::Repository(0));
        assert!(validate_package(&p).is_ok());
        p.dist = Some(SourceRef {
            kind: "zip".into(),
            url: " -evil".into(),
            reference: None,
        });
        assert!(validate_package(&p).unwrap_err().contains("dist.url"));
        p.dist = None;
        p.raw = serde_json::json!({"bin": ["../x"]});
        assert!(validate_package(&p).unwrap_err().contains("bin"));
        assert!(package_naming_error("Acme/Lib").is_some());
        assert!(package_naming_error("acme/lib.json").is_some());
        assert!(package_naming_error("con/lib").is_some());
        assert!(package_naming_error("acme/lib--x").is_none());
        assert!(package_naming_error("php").is_none());
    }

    #[test]
    fn php_string_order() {
        assert_eq!(php_compare_strings("10", "9"), Ordering::Greater);
        assert_eq!(php_compare_strings("a", "b"), Ordering::Less);
        assert_eq!(php_compare_strings("Zend", "apc"), Ordering::Less);
    }
}
