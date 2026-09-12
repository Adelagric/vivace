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
    /// Plusieurs URL d'un coup (un lot de `loadAsyncPackages`, que Composer
    /// télécharge en parallèle) ; résultats dans l'ordre des URL. Par défaut
    /// séquentiel.
    fn fetch_many(&self, urls: &[String]) -> Vec<Result<Option<Vec<u8>>, RepoError>> {
        urls.iter().map(|u| self.fetch(u)).collect()
    }
}

/// Récupération réseau fournie par l'appelant (`https://`), `Ok(None)` sur
/// 404.
pub type HttpFetch = std::sync::Arc<dyn Fn(&str) -> Result<Option<Vec<u8>>, String> + Send + Sync>;
/// Variante par lot : toutes les URL en parallèle, résultats dans l'ordre.
pub type HttpFetchMany =
    std::sync::Arc<dyn Fn(&[String]) -> Vec<Result<Option<Vec<u8>>, String>> + Send + Sync>;

pub struct HttpTransport {
    pub fetch: HttpFetch,
    pub fetch_many: Option<HttpFetchMany>,
}

impl Transport for HttpTransport {
    fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>, RepoError> {
        (self.fetch)(url).map_err(RepoError)
    }
    fn fetch_many(&self, urls: &[String]) -> Vec<Result<Option<Vec<u8>>, RepoError>> {
        match &self.fetch_many {
            Some(f) => f(urls).into_iter().map(|r| r.map_err(RepoError)).collect(),
            None => urls.iter().map(|u| self.fetch(u)).collect(),
        }
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
    /// `mirrors` de packages.json : `sourceMirrors[type]` et `distMirrors`
    /// (`[{url, preferred}]`).
    source_mirrors: BTreeMap<String, Vec<Value>>,
    dist_mirrors: Vec<Value>,
    /// Dépôt sans `metadata-url` ni providers : toutes les métadonnées
    /// (`packages` + `includes`), dans l'ordre de `loadIncludes`.
    plain: Option<Vec<Value>>,
}

pub struct ComposerRepository {
    pub url: String,
    pub base_url: String,
    /// `options` de la définition du dépôt (transport-options des paquets
    /// dont une URL de dist est sous `base_url`).
    pub options: Value,
    packages_json_url: String,
    transport: Box<dyn Transport>,
    /// Chargé au premier `loadPackages`, comme chez Composer.
    root: std::cell::OnceCell<RootData>,
    /// `provider-<name>.json` déjà lus (cache mémoire du run).
    fetched: std::cell::RefCell<BTreeMap<String, Option<std::rc::Rc<Value>>>>,
    /// Dépôt plein : index d'arène de ses paquets une fois chargés
    /// (`getPackages()`), [alias, base] par version aliasée.
    members: std::cell::OnceCell<Vec<usize>>,
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
            options: Value::Object(Map::new()),
            packages_json_url,
            transport,
            root: std::cell::OnceCell::new(),
            fetched: std::cell::RefCell::new(BTreeMap::new()),
            members: std::cell::OnceCell::new(),
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
        if let Some(mirrors) = data.get("mirrors").and_then(Value::as_array) {
            for mirror in mirrors {
                let preferred = !php_empty(mirror.get("preferred"));
                for (key, kind) in [("git-url", "git"), ("hg-url", "hg")] {
                    if let Some(u) = mirror.get(key).filter(|u| !php_empty(Some(u))) {
                        r.source_mirrors
                            .entry(kind.to_owned())
                            .or_default()
                            .push(serde_json::json!({"url": u, "preferred": preferred}));
                    }
                }
                if let Some(u) = mirror.get("dist-url").and_then(Value::as_str) {
                    if !php_empty(Some(&Value::String(u.to_owned()))) {
                        r.dist_mirrors
                            .push(serde_json::json!({"url": self.canonicalize_url(u), "preferred": preferred}));
                    }
                }
            }
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
            // Dépôt « plein » (Satis, `packages.json` statique) : tous les
            // paquets viennent de `packages` et des `includes`
            // (`loadIncludes`), chargés d'un bloc comme `initialize()`.
            r.plain = Some(self.load_includes(&data)?);
        }
        let _ = self.root.set(r);
        Ok(self.root.get().expect("just set"))
    }

    /// `loadIncludes($data)` : métadonnées de `packages` (par nom, par
    /// version) puis des fichiers `includes`, récursivement.
    fn load_includes(&self, data: &Value) -> Result<Vec<Value>, RepoError> {
        let mut out = Vec::new();
        let has_packages = data.get("packages").is_some();
        let has_includes = data.get("includes").is_some();
        if !has_packages && !has_includes {
            for (_, pkg) in data.as_object().into_iter().flatten() {
                if let Some(Value::Array(versions)) = pkg.get("versions") {
                    out.extend(versions.iter().cloned());
                } else if let Some(Value::Object(versions)) = pkg.get("versions") {
                    out.extend(versions.values().cloned());
                }
            }
            return Ok(out);
        }
        if let Some(packages) = data.get("packages").and_then(Value::as_object) {
            for (_, versions) in packages {
                match versions {
                    Value::Array(a) => out.extend(a.iter().cloned()),
                    Value::Object(o) => out.extend(o.values().cloned()),
                    _ => {}
                }
            }
        }
        if let Some(includes) = data.get("includes").and_then(Value::as_object) {
            for (include, _) in includes {
                let url = self.canonicalize_url(include);
                let url = if url.contains("://") {
                    url
                } else {
                    format!("{}/{}", self.base_url, url.trim_start_matches('/'))
                };
                let bytes = self
                    .transport
                    .fetch(&url)?
                    .ok_or_else(|| RepoError(format!("{url} not found")))?;
                let included: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| RepoError(format!("{url}: invalid JSON: {e}")))?;
                out.extend(self.load_includes(&included)?);
            }
        }
        Ok(out)
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
        let fetched = self.transport.fetch(&url)?;
        let value = Self::parse_provider(&url, package_name, fetched)?;
        self.fetched.borrow_mut().insert(key, value.clone());
        Ok(value)
    }

    fn parse_provider(
        url: &str,
        package_name: &str,
        fetched: Option<Vec<u8>>,
    ) -> Result<Option<std::rc::Rc<Value>>, RepoError> {
        Ok(match fetched {
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
        })
    }

    /// Les fichiers d'un lot pas encore en cache, téléchargés d'un coup
    /// (`loadAsyncPackages` lance toutes les promesses avant d'attendre).
    fn prefetch(&self, names: &[(String, String)]) -> Result<(), RepoError> {
        let Some(template) = self.root_data()?.lazy_providers_url.clone() else {
            return Ok(());
        };
        let mut todo: Vec<(String, String, String)> = Vec::new();
        {
            let cache = self.fetched.borrow();
            for (file_name, package_name) in names {
                let key = file_name.to_lowercase();
                if cache.contains_key(&key) || todo.iter().any(|(k, _, _)| *k == key) {
                    continue;
                }
                let url = template.replace("%package%", &key);
                todo.push((key, package_name.clone(), url));
            }
        }
        if todo.len() < 2 {
            return Ok(());
        }
        let urls: Vec<String> = todo.iter().map(|(_, _, u)| u.clone()).collect();
        let results = self.transport.fetch_many(&urls);
        let mut cache = self.fetched.borrow_mut();
        for ((key, package_name, url), result) in todo.into_iter().zip(results) {
            let value = Self::parse_provider(&url, &package_name, result?)?;
            cache.insert(key, value);
        }
        Ok(())
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
            let (mut package, alias) =
                loader::load(&config, origin, true).map_err(|e| RepoError(e.0))?;
            self.configure_package(root, &mut package);
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

    /// Suite de `createPackages` : `setSourceMirrors` (par type),
    /// `setDistMirrors` (toujours, écrase ceux des métadonnées),
    /// `configurePackageTransportOptions` (les `options` du dépôt si une
    /// URL de dist est sous `baseUrl`) ; et les `transport-options` des
    /// métadonnées ne sont pas chargées (`loadOptions` faux).
    fn configure_package(&self, root: &RootData, p: &mut Package) {
        let Some(obj) = p.raw.as_object_mut() else {
            return;
        };
        obj.shift_remove("transport-options");
        if let Some(src) = &p.source {
            if let Some(mirrors) = root.source_mirrors.get(&src.kind) {
                if let Some(Value::Object(s)) = obj.get_mut("source") {
                    s.insert("mirrors".into(), Value::Array(mirrors.clone()));
                }
            }
        }
        if let Some(Value::Object(d)) = obj.get_mut("dist") {
            if root.dist_mirrors.is_empty() {
                d.shift_remove("mirrors");
            } else {
                d.insert("mirrors".into(), Value::Array(root.dist_mirrors.clone()));
            }
        }
        if let Some(dist) = &p.dist {
            let urls = dist_urls(
                dist,
                &root.dist_mirrors,
                &p.name,
                &p.version,
                &p.pretty_version,
            );
            if urls.iter().any(|u| u.starts_with(&self.base_url)) {
                let empty = self.options.as_object().is_some_and(Map::is_empty)
                    || self.options.as_array().is_some_and(Vec::is_empty);
                if !empty {
                    obj.insert("transport-options".into(), self.options.clone());
                }
            }
        }
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
        if let Some(plain) = &root.plain {
            // `parent::loadPackages` (ArrayRepository) sur `getPackages()`.
            if self.members.get().is_none() {
                let configs: Vec<Value> = plain
                    .iter()
                    .map(|c| Self::with_notification_url(c, root))
                    .collect();
                let ids = loader::load_packages(&configs, origin, arena, true)
                    .map_err(|e| RepoError(e.0))?;
                for &id in &ids {
                    let mut p = std::mem::replace(&mut arena[id], Package::new("", "", "", origin));
                    self.configure_package(root, &mut p);
                    arena[id] = p;
                }
                let _ = self.members.set(ids);
            }
            let members = self.members.get().expect("just set");
            return Ok(crate::pool::array_repository_load_packages(
                members,
                package_name_map,
                acceptable,
                flags,
                already_loaded,
                arena,
            ));
        }
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

        let wanted: Vec<(String, String)> = names
            .iter()
            .map(|(n, _)| n.to_lowercase())
            .filter(|n| {
                let real = n.strip_suffix("~dev").unwrap_or(n);
                !is_platform_package(real) && real != "__root__"
            })
            .map(|n| {
                let real = n.strip_suffix("~dev").unwrap_or(&n).to_owned();
                (n.clone(), real)
            })
            .collect();
        self.prefetch(&wanted)?;

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
            for &id in &ids {
                let base = arena[id].alias_of.unwrap_or(id);
                let mut p = std::mem::replace(&mut arena[base], Package::new("", "", "", origin));
                self.configure_package(root, &mut p);
                arena[base] = p;
                if base != id {
                    let mut a = std::mem::replace(&mut arena[id], Package::new("", "", "", origin));
                    self.configure_package(root, &mut a);
                    arena[id] = a;
                }
            }
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

/// `ComposerMirror::processUrl`.
fn process_mirror_url(
    mirror_url: &str,
    name: &str,
    version: &str,
    reference: Option<&str>,
    kind: &str,
    pretty_version: &str,
) -> String {
    static HEX: OnceLock<Regex> = OnceLock::new();
    let reference = reference.map(|r| {
        if r.is_empty() {
            String::new()
        } else if regex(&HEX, r"^([a-f0-9]*|%reference%)$", false)
            .is_match(r.as_bytes())
            .unwrap_or(false)
        {
            r.to_owned()
        } else {
            vivace_core::content_hash::md5_hex(r.as_bytes())
        }
    });
    let version = if version.contains('/') {
        vivace_core::content_hash::md5_hex(version.as_bytes())
    } else {
        version.to_owned()
    };
    mirror_url
        .replace("%package%", name)
        .replace("%version%", &version)
        .replace("%reference%", reference.as_deref().unwrap_or(""))
        .replace("%type%", kind)
        .replace("%prettyVersion%", pretty_version)
}

/// `Package::getDistUrls` : l'URL (placeholders traités) puis les miroirs,
/// les préférés devant.
fn dist_urls(
    dist: &crate::package::SourceRef,
    mirrors: &[Value],
    name: &str,
    version: &str,
    pretty: &str,
) -> Vec<String> {
    if dist.url.is_empty() {
        return Vec::new();
    }
    let url = if dist.url.contains('%') {
        process_mirror_url(
            &dist.url,
            name,
            version,
            dist.reference.as_deref(),
            &dist.kind,
            pretty,
        )
    } else {
        dist.url.clone()
    };
    let mut urls = vec![url];
    for m in mirrors {
        let Some(mu) = m.get("url").and_then(Value::as_str) else {
            continue;
        };
        let mirror_url = process_mirror_url(
            mu,
            name,
            version,
            dist.reference.as_deref(),
            &dist.kind,
            pretty,
        );
        if !urls.contains(&mirror_url) {
            if m.get("preferred") == Some(&Value::Bool(true)) {
                urls.insert(0, mirror_url);
            } else {
                urls.push(mirror_url);
            }
        }
    }
    urls
}
