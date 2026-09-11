//! Dépôts vus par le pool : `ComposerRepository` v2 (`metadata-url`,
//! fichiers p2 minifiés, `~dev`), le dépôt du lock (`LockArrayRepository`),
//! la racine et la plateforme (listes de paquets déjà chargés). Port de
//! docs/reference/resolver/ComposerRepository.php (chemin v2 uniquement :
//! `providers-url`/`provider-includes` v1 → refus).

use crate::constraint::Constraint;
use crate::loader::{self, branch_alias, expand_minified_owned};
use crate::package::{Origin, Package};
use crate::platform::is_platform_package;
use crate::version::{normalize, parse_stability, regex, stability_rank, DEFAULT_BRANCH_ALIAS};
use pcre2::bytes::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RepoError(pub String);

/// Récupération d'une URL : `Ok(None)` = 404 (paquet inconnu, toléré par
/// Composer en HTTP).
pub trait Transport {
    fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, RepoError>;
}

/// Récupération réseau fournie par l'appelant (`https://`), `Ok(None)` sur
/// 404.
pub type HttpFetch = std::sync::Arc<dyn Fn(&str) -> Result<Option<Vec<u8>>, String> + Send + Sync>;

pub struct HttpTransport(pub HttpFetch);

impl Transport for HttpTransport {
    fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, RepoError> {
        (self.0)(url).map_err(RepoError)
    }
}

/// `file://` : un fichier absent est fatal, comme chez Composer.
pub struct FileTransport;

impl Transport for FileTransport {
    fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, RepoError> {
        let path = url
            .strip_prefix("file://")
            .ok_or_else(|| RepoError(format!("unsupported url scheme: {url}")))?;
        std::fs::read(path).map(Some).map_err(|e| {
            RepoError(format!(
                "The \"{url}\" file could not be downloaded: Failed to open stream: {e}"
            ))
        })
    }
}

/// `StabilityFilter::isPackageAcceptable`.
pub fn is_package_acceptable(
    acceptable: &BTreeMap<String, i32>,
    flags: &BTreeMap<String, i32>,
    names: &[String],
    stability: &str,
) -> bool {
    for name in names {
        if let Some(flag) = flags.get(name) {
            if stability_rank(stability) <= *flag {
                return true;
            }
        } else if acceptable.contains_key(stability) {
            return true;
        }
    }
    false
}

/// `BasePackage::packageNameToRegexp`.
fn package_name_regexp(pattern: &str) -> Regex {
    let quoted = crate::version::preg_quote(pattern).replace("\\*", ".*");
    pcre2::bytes::RegexBuilder::new()
        .caseless(true)
        .build(&format!("^{quoted}$"))
        .unwrap_or_else(|e| panic!("pattern {pattern}: {e}"))
}

/// `loadRootServerFile` : ce que packages.json apporte.
#[derive(Debug, Default)]
struct RootData {
    lazy_providers_url: Option<String>,
    notify_url: Option<String>,
    has_available_package_list: bool,
    available_packages: BTreeSet<String>,
    available_patterns: Vec<Regex>,
    /// `partialPackagesByName` : paquets en ligne de packages.json, par nom
    /// (ordre d'apparition).
    partial_packages: Vec<(String, Vec<Value>)>,
}

pub struct ComposerRepository {
    pub url: String,
    pub base_url: String,
    packages_json_url: String,
    transport: Box<dyn Transport>,
    /// Chargé au premier `loadPackages`, comme chez Composer.
    root: std::cell::OnceCell<RootData>,
    /// `provider-<name>.json` déjà lus (cache mémoire du run).
    fetched: std::cell::RefCell<BTreeMap<String, Option<std::rc::Rc<Value>>>>,
}

/// `empty()` PHP sur une valeur JSON.
fn php_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => true,
        Some(Value::String(s)) => s.is_empty() || s == "0",
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Bool(true)) => false,
    }
}

/// Chemin d'une URL (sans schéma, hôte, requête ni fragment).
fn url_path(url: &str) -> &str {
    let rest = match url.find("://") {
        Some(i) => {
            let after = &url[i + 3..];
            match after.find('/') {
                Some(j) => &after[j..],
                None => "",
            }
        }
        None => url,
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    &rest[..end]
}

impl ComposerRepository {
    /// Constructeur (sans lecture : `loadRootServerFile` est paresseux).
    pub fn open(url: &str, transport: Box<dyn Transport>) -> Result<ComposerRepository, RepoError> {
        static SCHEME: OnceLock<Regex> = OnceLock::new();
        static PACKAGIST: OnceLock<Regex> = OnceLock::new();
        static BASE: OnceLock<Regex> = OnceLock::new();
        let mut url = url.to_owned();
        if !regex(&SCHEME, r"^[\w.]+\??://", false)
            .is_match(url.as_bytes())
            .unwrap_or(false)
        {
            match std::fs::canonicalize(&url) {
                Ok(p) => url = format!("file://{}", p.to_string_lossy()),
                Err(_) => url = format!("http://{url}"),
            }
        }
        url = url.trim_end_matches('/').to_owned();
        if let Some(rest) = url.strip_prefix("https?") {
            url = format!("https{rest}");
        }
        if let Ok(Some(caps)) = regex(&PACKAGIST, r"^(?P<proto>https?)://packagist\.org/?$", true)
            .captures(url.as_bytes())
        {
            url = format!("{}://repo.packagist.org", crate::version::group(&caps, 1));
        }
        let base_re = regex(&BASE, r"(?:/[^/\\]+\.json)?(?:[?#].*)?$", false);
        let base_url = match base_re.find(url.as_bytes()).ok().flatten() {
            Some(m) => url[..m.start()].trim_end_matches('/').to_owned(),
            None => url.clone(),
        };
        // `getPackagesJsonUrl` : `.json` cherché dans le chemin seulement.
        let packages_json_url = if url_path(&url).contains(".json") {
            url.clone()
        } else {
            format!("{url}/packages.json")
        };
        Ok(ComposerRepository {
            url,
            base_url,
            packages_json_url,
            transport,
            root: std::cell::OnceCell::new(),
            fetched: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    /// `loadRootServerFile`, une fois.
    fn root_data(&self) -> Result<&RootData, RepoError> {
        if let Some(r) = self.root.get() {
            return Ok(r);
        }
        let bytes = self
            .transport
            .fetch(&self.packages_json_url)?
            .ok_or_else(|| RepoError(format!("{} not found", self.packages_json_url)))?;
        let data: Value = serde_json::from_slice(&bytes)
            .map_err(|e| RepoError(format!("{}: invalid JSON: {e}", self.packages_json_url)))?;
        let non_empty = |k: &str| !php_empty(data.get(k));
        let mut r = RootData::default();
        if non_empty("notify-batch") {
            r.notify_url = data["notify-batch"]
                .as_str()
                .map(|s| self.canonicalize_url(s));
        } else if non_empty("notify") {
            r.notify_url = data["notify"].as_str().map(|s| self.canonicalize_url(s));
        }
        let mut has_providers = false;
        let mut has_partial = false;
        if non_empty("providers-lazy-url") {
            r.lazy_providers_url = data["providers-lazy-url"]
                .as_str()
                .map(|s| self.canonicalize_url(s));
            has_providers = true;
            has_partial = non_empty("packages") && data["packages"].is_object();
        }
        if non_empty("metadata-url") {
            r.lazy_providers_url = data["metadata-url"]
                .as_str()
                .map(|s| self.canonicalize_url(s));
            has_partial = non_empty("packages") && data["packages"].is_object();
            if non_empty("available-packages") {
                for p in data["available-packages"].as_array().into_iter().flatten() {
                    if let Some(s) = p.as_str() {
                        r.available_packages.insert(s.to_lowercase());
                    }
                }
                r.has_available_package_list = true;
            }
            if non_empty("available-package-patterns") {
                for p in data["available-package-patterns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(s) = p.as_str() {
                        r.available_patterns.push(package_name_regexp(s));
                    }
                }
                r.has_available_package_list = true;
            }
        } else if non_empty("providers-url")
            || non_empty("providers")
            || non_empty("providers-includes")
            || has_providers
        {
            return Err(RepoError(format!(
                "{}: Composer v1 repository protocol (providers) is not supported by vivace",
                self.url
            )));
        }
        if has_partial {
            // `initializePartialPackages` : indexés par le `name` de chaque
            // version, pas par la clé du tableau.
            for (_, versions) in data["packages"].as_object().into_iter().flatten() {
                let list: Vec<&Value> = match versions {
                    Value::Array(a) => a.iter().collect(),
                    Value::Object(o) => o.values().collect(),
                    _ => Vec::new(),
                };
                for v in list {
                    let name = v
                        .get("name")
                        .map(|n| match n {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default()
                        .to_lowercase();
                    match r.partial_packages.iter_mut().find(|(n, _)| *n == name) {
                        Some(slot) => slot.1.push(v.clone()),
                        None => r.partial_packages.push((name, vec![v.clone()])),
                    }
                }
            }
        } else if r.lazy_providers_url.is_none() {
            return Err(RepoError(format!(
                "{}: no metadata-url in packages.json (only v2 repositories are supported)",
                self.url
            )));
        }
        let _ = self.root.set(r);
        Ok(self.root.get().expect("just set"))
    }

    pub fn notify_url(&self) -> Result<Option<String>, RepoError> {
        Ok(self.root_data()?.notify_url.clone())
    }

    pub fn lazy_providers_url(&self) -> Result<Option<String>, RepoError> {
        Ok(self.root_data()?.lazy_providers_url.clone())
    }

    /// `canonicalizeUrl`.
    fn canonicalize_url(&self, url: &str) -> String {
        static RE: OnceLock<Regex> = OnceLock::new();
        if let Some(rest) = url.strip_prefix('/') {
            let re = regex(&RE, r"^[^:]++://[^/]*+", false);
            if let Ok(Some(m)) = re.find(self.url.as_bytes()) {
                return format!("{}/{}", &self.url[..m.end()], rest);
            }
            return self.url.clone();
        }
        url.to_owned()
    }

    /// `lazyProvidersRepoContains`.
    fn contains(root: &RootData, name: &str) -> bool {
        if root.available_packages.contains(name) {
            return true;
        }
        root.available_patterns
            .iter()
            .any(|re| re.is_match(name.as_bytes()).unwrap_or(false))
    }

    /// `startCachedAsyncDownload` : le JSON du fichier p2 d'un nom (avec
    /// `~dev`), None si 404 ou sans la clé attendue.
    fn provider(
        &self,
        file_name: &str,
        package_name: &str,
    ) -> Result<Option<std::rc::Rc<Value>>, RepoError> {
        let key = file_name.to_lowercase();
        if let Some(v) = self.fetched.borrow().get(&key) {
            return Ok(v.clone());
        }
        let Some(template) = &self.root_data()?.lazy_providers_url else {
            return Err(RepoError("startCachedAsyncDownload only supports v2 protocol composer repos with a metadata-url".into()));
        };
        let url = template.replace("%package%", &key);
        let value = match self.transport.fetch(&url)? {
            None => None,
            Some(bytes) => {
                let v: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| RepoError(format!("{url}: invalid JSON: {e}")))?;
                let has = v
                    .get("packages")
                    .and_then(|p| p.get(package_name))
                    .is_some()
                    || v.get("security-advisories").is_some()
                    || v.get("filter").is_some();
                if has {
                    Some(std::rc::Rc::new(v))
                } else {
                    None
                }
            }
        };
        self.fetched.borrow_mut().insert(key, value.clone());
        Ok(value)
    }

    /// `isVersionAcceptable`.
    fn is_version_acceptable(
        constraint: Option<&Constraint>,
        name: &str,
        version_data: &Map<String, Value>,
        acceptable: &BTreeMap<String, i32>,
        flags: &BTreeMap<String, i32>,
    ) -> bool {
        let mut versions: Vec<String> = Vec::new();
        if let Some(v) = version_data
            .get("version_normalized")
            .and_then(Value::as_str)
        {
            versions.push(v.to_owned());
        }
        if let Some(alias) = branch_alias(version_data) {
            versions.push(alias);
        }
        let names = vec![name.to_owned()];
        for v in &versions {
            if !is_package_acceptable(acceptable, flags, &names, parse_stability(v)) {
                continue;
            }
            if let Some(c) = constraint {
                if !c.matches_version(v) {
                    continue;
                }
            }
            return true;
        }
        false
    }

    /// `whatProvides` restreint aux paquets en ligne de packages.json :
    /// versions du nom, dédoublonnées par `uid`, filtrées par stabilité, et
    /// chargées en lot — [base, alias] par version (`$result[$uid]`,
    /// `$result[$uid.'-alias']`).
    #[allow(clippy::too_many_arguments)]
    fn what_provides_partial(
        &self,
        root: &RootData,
        name: &str,
        acceptable: &BTreeMap<String, i32>,
        flags: &BTreeMap<String, i32>,
        already_loaded: &BTreeMap<String, BTreeSet<String>>,
        origin: Origin,
        arena: &mut Vec<Package>,
    ) -> Result<Vec<usize>, RepoError> {
        let Some((_, versions)) = root.partial_packages.iter().find(|(n, _)| n == name) else {
            return Ok(Vec::new());
        };
        let mut to_load: Vec<(String, Value)> = Vec::new();
        for v in versions {
            let mut data = v.as_object().cloned().unwrap_or_default();
            let normalized_name = data
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if normalized_name != name {
                continue;
            }
            let uid = data
                .get("uid")
                .map(|u| match u {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if to_load.iter().any(|(u, _)| *u == uid) {
                continue;
            }
            Self::fill_version_normalized(&mut data)?;
            let normalized = data
                .get("version_normalized")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if already_loaded
                .get(name)
                .is_some_and(|s| s.contains(&normalized))
            {
                continue;
            }
            if Self::is_version_acceptable(None, &normalized_name, &data, acceptable, flags) {
                to_load.push((uid, Value::Object(data)));
            }
        }
        let mut out = Vec::new();
        for (_, config) in &to_load {
            let config = Self::with_notification_url(config, root);
            let (package, alias) =
                loader::load(&config, origin, true).map_err(|e| RepoError(e.0))?;
            let idx = arena.len();
            arena.push(package);
            out.push(idx);
            if let Some((normalized, pretty)) = alias {
                let a = arena[idx].alias(idx, &normalized, &pretty);
                arena.push(a);
                out.push(arena.len() - 1);
            }
        }
        Ok(out)
    }

    /// `createPackages` : `$data['notification-url'] ??= $this->notifyUrl`.
    fn add_notification_url(obj: &mut Map<String, Value>, root: &RootData) {
        if !obj.contains_key("notification-url") {
            obj.insert(
                "notification-url".into(),
                match &root.notify_url {
                    Some(u) => Value::String(u.clone()),
                    None => Value::Null,
                },
            );
        }
    }

    fn with_notification_url(config: &Value, root: &RootData) -> Value {
        let mut config = config.clone();
        if let Some(obj) = config.as_object_mut() {
            Self::add_notification_url(obj, root);
        }
        config
    }

    /// `version_normalized` absent ou égal à l'alias de branche par défaut
    /// → recalculé depuis `version`.
    fn fill_version_normalized(data: &mut Map<String, Value>) -> Result<(), RepoError> {
        let pretty = data
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        match data.get("version_normalized").and_then(Value::as_str) {
            None => {
                let n = normalize(&pretty, None).map_err(|e| RepoError(e.0))?;
                data.insert("version_normalized".into(), Value::String(n));
            }
            Some(v) if v == DEFAULT_BRANCH_ALIAS => {
                let n = normalize(&pretty, None).map_err(|e| RepoError(e.0))?;
                data.insert("version_normalized".into(), Value::String(n));
            }
            _ => {}
        }
        Ok(())
    }

    /// `loadPackages` : paquets en ligne d'abord (`whatProvides`), puis le
    /// chemin v2 (`loadAsyncPackages`). Rend `(namesFound, ids)` ;
    /// `already_loaded` : name → versions normalisées déjà dans le pool pour
    /// ce dépôt.
    pub fn load_packages(
        &self,
        package_name_map: &[(String, Constraint)],
        acceptable: &BTreeMap<String, i32>,
        flags: &BTreeMap<String, i32>,
        already_loaded: &BTreeMap<String, BTreeSet<String>>,
        origin: Origin,
        arena: &mut Vec<Package>,
    ) -> Result<(Vec<String>, Vec<usize>), RepoError> {
        let root = self.root_data()?;
        let mut map: Vec<(String, Constraint)> = package_name_map.to_vec();
        let mut packages: Vec<usize> = Vec::new();
        let mut names_found: Vec<String> = Vec::new();

        if !root.partial_packages.is_empty() {
            let mut rest: Vec<(String, Constraint)> = Vec::new();
            for (name, constraint) in map {
                if !root.partial_packages.iter().any(|(n, _)| *n == name) {
                    rest.push((name, constraint));
                    continue;
                }
                let candidates = self.what_provides_partial(
                    root,
                    &name,
                    acceptable,
                    flags,
                    already_loaded,
                    origin,
                    arena,
                )?;
                let mut matches: Vec<usize> = Vec::new();
                for &c in &candidates {
                    if !names_found.contains(&name) {
                        names_found.push(name.clone());
                    }
                    let all = matches!(constraint, Constraint::MatchAll);
                    if all || constraint.matches_version(&arena[c].version) {
                        if !matches.contains(&c) {
                            matches.push(c);
                        }
                        if let Some(base) = arena[c].alias_of {
                            if !matches.contains(&base) {
                                matches.push(base);
                            }
                        }
                    }
                }
                for &c in &candidates {
                    if let Some(base) = arena[c].alias_of {
                        if matches.contains(&base) && !matches.contains(&c) {
                            matches.push(c);
                        }
                    }
                }
                packages.extend(matches);
            }
            map = rest;
        }

        if root.lazy_providers_url.is_none() || map.is_empty() {
            return Ok((names_found, packages));
        }
        if root.has_available_package_list {
            map.retain(|(name, _)| Self::contains(root, &name.to_lowercase()));
        }
        // `$packageNames[$name.'~dev'] = $constraint` (ajouté à la fin) ;
        // dev seul → le nom nu est retiré.
        let only_dev = acceptable.len() == 1 && acceptable.contains_key("dev") && flags.is_empty();
        let mut names: Vec<(String, Constraint)> = Vec::new();
        let mut dev_names: Vec<(String, Constraint)> = Vec::new();
        for (name, c) in &map {
            if is_package_acceptable(acceptable, flags, std::slice::from_ref(name), "dev") {
                dev_names.push((format!("{name}~dev"), c.clone()));
            }
            if !only_dev {
                names.push((name.clone(), c.clone()));
            }
        }
        names.extend(dev_names);

        for (name, constraint) in &names {
            let name = name.to_lowercase();
            let real_name = name.strip_suffix("~dev").unwrap_or(&name).to_owned();
            if is_platform_package(&real_name) || real_name == "__root__" {
                continue;
            }
            let Some(response) = self.provider(&name, &real_name)? else {
                continue;
            };
            let versions: Vec<Value> =
                match response.get("packages").and_then(|p| p.get(&real_name)) {
                    Some(Value::Array(a)) => a.clone(),
                    Some(Value::Object(o)) => o.values().cloned().collect(),
                    _ => continue,
                };
            let versions: Vec<Value> =
                if response.get("minified").and_then(Value::as_str) == Some("composer/2.0") {
                    expand_minified_owned(versions)
                } else {
                    versions
                };
            if !names_found.contains(&real_name) {
                names_found.push(real_name.clone());
            }
            let mut to_load: Vec<Value> = Vec::new();
            for v in versions {
                let mut data = match v {
                    Value::Object(o) => o,
                    _ => Map::new(),
                };
                Self::fill_version_normalized(&mut data)?;
                let normalized = data
                    .get("version_normalized")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if already_loaded
                    .get(&real_name)
                    .is_some_and(|s| s.contains(&normalized))
                {
                    continue;
                }
                if Self::is_version_acceptable(
                    Some(constraint),
                    &real_name,
                    &data,
                    acceptable,
                    flags,
                ) {
                    Self::add_notification_url(&mut data, root);
                    to_load.push(Value::Object(data));
                }
            }
            let ids =
                loader::load_packages(&to_load, origin, arena, true).map_err(|e| RepoError(e.0))?;
            packages.extend(ids);
        }
        Ok((names_found, packages))
    }
}

/// `Locker::getLockedRepository(true)` : paquets du lock (+ dev) puis les
/// alias racine (`aliases`), chaque alias avant son paquet.
pub fn locked_repository(lock: &Value, arena: &mut Vec<Package>) -> Result<Vec<usize>, RepoError> {
    let mut configs: Vec<Value> = lock
        .get("packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match lock.get("packages-dev").and_then(Value::as_array) {
        Some(dev) => configs.extend(dev.iter().cloned()),
        None => {
            return Err(RepoError(
                "The lock file does not contain require-dev information, run install with the --no-dev option or delete it and run composer update to generate a new lock file.".into(),
            ))
        }
    }
    if configs.is_empty() {
        return Ok(Vec::new());
    }
    let ids = loader::load_packages(&configs, Origin::Locked, arena, false)
        .map_err(|e| RepoError(e.0))?;
    let mut out = ids.clone();
    // `$packageByName[$name] = $package` : pour un alias, les deux noms
    // pointent (alias → dernier écrit gagne : le paquet de base).
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    for id in &ids {
        by_name.insert(arena[*id].name.clone(), *id);
        if let Some(base) = arena[*id].alias_of {
            by_name.insert(arena[base].name.clone(), base);
        }
    }
    for alias in lock
        .get("aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(pkg), Some(alias_normalized), Some(alias_pretty)) = (
            alias.get("package").and_then(Value::as_str),
            alias.get("alias_normalized").and_then(Value::as_str),
            alias.get("alias").and_then(Value::as_str),
        ) else {
            continue;
        };
        if let Some(&base) = by_name.get(pkg) {
            let mut a = arena[base].alias(base, alias_normalized, alias_pretty);
            a.root_package_alias = true;
            arena.push(a);
            out.push(arena.len() - 1);
        }
    }
    Ok(out)
}
