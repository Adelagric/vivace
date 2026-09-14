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

/// Erreur de dépôt : de transport (`TransportException` chez Composer —
/// réseau, fichier absent, 404 là où il est fatal) ou de données (JSON,
/// contrainte, forme d'une réponse) ; seules les premières relèvent de
/// `ignore-unreachable`.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RepoError(pub String, pub RepoErrorKind);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoErrorKind {
    Transport,
    Data,
}

impl RepoError {
    pub fn data(message: impl Into<String>) -> RepoError {
        RepoError(message.into(), RepoErrorKind::Data)
    }
    pub fn transport(message: impl Into<String>) -> RepoError {
        RepoError(message.into(), RepoErrorKind::Transport)
    }
    pub fn is_transport(&self) -> bool {
        self.1 == RepoErrorKind::Transport
    }
}

/// Récupération d'une URL : `Ok(None)` = 404 (paquet inconnu, toléré par
/// Composer en HTTP).
/// Résultat d'une récupération conditionnelle (`If-Modified-Since`).
#[derive(Debug, Clone)]
pub enum Fetched {
    /// 304 : le cache est bon.
    NotModified,
    /// 404 : paquet inconnu (toléré par Composer en HTTP).
    NotFound,
    Body {
        bytes: Vec<u8>,
        /// En-tête `Last-Modified` de la réponse.
        last_modified: Option<String>,
    },
}

/// Une requête : URL et `If-Modified-Since` éventuel (la valeur
/// `last-modified` du fichier en cache).
pub type Request = (String, Option<String>);

pub trait Transport {
    fn fetch(&self, url: &str, if_modified_since: Option<&str>) -> Result<Fetched, RepoError>;
    /// POST `application/x-www-form-urlencoded` (l'API des avis de
    /// sécurité de Packagist) ; le corps est déjà encodé. Par défaut
    /// refusé.
    fn post_form(&self, url: &str, _body: &str) -> Result<Fetched, RepoError> {
        Err(RepoError::transport(format!(
            "POST {url}: not supported by this transport"
        )))
    }
    /// Plusieurs requêtes d'un coup (un lot de `loadAsyncPackages`, que
    /// Composer télécharge en parallèle) ; résultats dans l'ordre. Par
    /// défaut séquentiel.
    fn fetch_many(&self, requests: &[Request]) -> Vec<Result<Fetched, RepoError>> {
        requests
            .iter()
            .map(|(u, ims)| self.fetch(u, ims.as_deref()))
            .collect()
    }
}

/// Récupération réseau fournie par l'appelant (`https://`), `Ok(None)` sur
/// 404.
pub type HttpFetch =
    std::sync::Arc<dyn Fn(&str, Option<&str>) -> Result<Fetched, String> + Send + Sync>;
/// Variante par lot : toutes les requêtes en parallèle, résultats dans l'ordre.
pub type HttpFetchMany =
    std::sync::Arc<dyn Fn(&[Request]) -> Vec<Result<Fetched, String>> + Send + Sync>;

/// POST d'un formulaire encodé ; `Ok(None)` sur 404.
pub type HttpPost = std::sync::Arc<dyn Fn(&str, &str) -> Result<Fetched, String> + Send + Sync>;
/// Les trois fermetures réseau d'un appelant : GET conditionnel, lot, POST.
pub type HttpTransports = (HttpFetch, Option<HttpFetchMany>, Option<HttpPost>);

pub struct HttpTransport {
    pub fetch: HttpFetch,
    pub fetch_many: Option<HttpFetchMany>,
    pub post: Option<HttpPost>,
}

impl Transport for HttpTransport {
    fn fetch(&self, url: &str, if_modified_since: Option<&str>) -> Result<Fetched, RepoError> {
        (self.fetch)(url, if_modified_since).map_err(RepoError::transport)
    }
    fn post_form(&self, url: &str, body: &str) -> Result<Fetched, RepoError> {
        match &self.post {
            Some(p) => p(url, body).map_err(RepoError::transport),
            None => Err(RepoError::transport(format!(
                "POST {url}: no transport for it"
            ))),
        }
    }
    fn fetch_many(&self, requests: &[Request]) -> Vec<Result<Fetched, RepoError>> {
        match &self.fetch_many {
            Some(f) => f(requests)
                .into_iter()
                .map(|r| r.map_err(RepoError::transport))
                .collect(),
            None => requests
                .iter()
                .map(|(u, ims)| self.fetch(u, ims.as_deref()))
                .collect(),
        }
    }
}

/// `file://` : un fichier absent est fatal, comme chez Composer ; pas de
/// `Last-Modified`, donc jamais de 304.
pub struct FileTransport;

impl Transport for FileTransport {
    fn fetch(&self, url: &str, _if_modified_since: Option<&str>) -> Result<Fetched, RepoError> {
        let path = url
            .strip_prefix("file://")
            .ok_or_else(|| RepoError::transport(format!("unsupported url scheme: {url}")))?;
        std::fs::read(path)
            .map(|bytes| Fetched::Body {
                bytes,
                last_modified: None,
            })
            .map_err(|e| {
                RepoError::transport(format!(
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
    /// Dépôt au protocole v1 (`providers-url`…) : refusé à la résolution.
    v1_protocol: bool,
    /// `security-advisories` de packages.json : `metadata`, `api-url`.
    security_advisories: Option<AdvisoryConfig>,
    /// `filter` de packages.json (`ComposerRepositoryFilterInformation`).
    filter: Option<FilterInfo>,
}

#[derive(Debug, Clone)]
pub struct AdvisoryConfig {
    pub metadata: bool,
    pub api_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FilterInfo {
    pub metadata: bool,
    /// Listes annoncées et activées, noms réservés exclus.
    pub lists: Vec<String>,
    pub summary_url: Option<String>,
    pub api_url: Option<String>,
}

/// Un avis de sécurité tel que Composer le charge : partiel (`advisoryId`,
/// `affectedVersions`) ou complet (avec `title`, `sources`, `reportedAt`).
#[derive(Debug, Clone)]
pub struct Advisory {
    pub package_name: String,
    pub advisory_id: String,
    pub affected_versions: Constraint,
    /// `SecurityAdvisory` : cve, sévérité, `remoteId` des sources.
    pub complete: Option<CompleteAdvisory>,
}

#[derive(Debug, Clone, Default)]
pub struct CompleteAdvisory {
    pub cve: Option<String>,
    pub severity: Option<String>,
    pub source_remote_ids: Vec<String>,
}

/// `PartialSecurityAdvisory::create` : contrainte analysée avec ses deux
/// replis, complet si `title`, `sources` et `reportedAt` sont là.
pub fn advisory_from_data(package_name: &str, data: &Value) -> Option<Advisory> {
    let affected = data.get("affectedVersions")?.as_str()?.to_owned();
    let advisory_id = data.get("advisoryId")?.as_str()?.to_owned();
    let constraint = match crate::constraint::parse_constraints(&affected) {
        Ok(c) => c.constraint,
        Err(_) => {
            static HEAD: OnceLock<Regex> = OnceLock::new();
            let re = regex(&HEAD, r"(^[>=<^~]*[\d.]+).*", false);
            let head = re
                .captures(affected.as_bytes())
                .ok()
                .flatten()
                .map(|c| crate::version::group(&c, 1).to_owned())
                .unwrap_or_default();
            match crate::constraint::parse_constraints(&head) {
                Ok(c) => c.constraint,
                Err(_) => Constraint::new(crate::constraint::Op::Eq, "0.0.0-invalid-version"),
            }
        }
    };
    let complete = if data.get("title").is_some_and(|v| !v.is_null())
        && data.get("sources").is_some_and(|v| !v.is_null())
        && data.get("reportedAt").is_some_and(|v| !v.is_null())
    {
        Some(CompleteAdvisory {
            cve: data.get("cve").and_then(Value::as_str).map(str::to_owned),
            severity: data
                .get("severity")
                .and_then(Value::as_str)
                .map(str::to_owned),
            source_remote_ids: data
                .get("sources")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.get("remoteId").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        })
    } else {
        None
    };
    Some(Advisory {
        package_name: package_name.to_owned(),
        advisory_id,
        affected_versions: constraint,
        complete,
    })
}

/// Avis par nom de paquet (`[name => [advisory…]]`).
pub type AdvisoriesByName = Vec<(String, Vec<Advisory>)>;
/// Entrées de liste par nom de liste.
pub type FilterEntriesByList = Vec<(String, Vec<FilterEntry>)>;
/// Résumé des listes : liste → (nom, contrainte).
type FilterSummary = Vec<(String, Vec<(String, String)>)>;

/// `FilterListEntry` : une version signalée par une liste.
#[derive(Debug, Clone)]
pub struct FilterEntry {
    pub package_name: String,
    pub constraint: Constraint,
    pub list_name: String,
    pub url: Option<String>,
    pub reason: Option<String>,
    pub id: Option<String>,
    pub source: Option<String>,
}

/// `FilterListEntryBuilder::build` : entrées par liste, restreintes aux
/// noms demandés et aux versions qui les concernent.
fn build_filter_entries(
    raw_by_list: &Value,
    map: &[(String, Constraint)],
    default_package: Option<&str>,
) -> Result<FilterEntriesByList, RepoError> {
    let mut result: FilterEntriesByList = Vec::new();
    let Some(lists) = raw_by_list.as_object() else {
        return Ok(result);
    };
    for (list_name, entries) in lists {
        let Some(entries) = entries.as_array() else {
            continue;
        };
        for data in entries {
            let Some(obj) = data.as_object() else {
                continue;
            };
            let Some(constraint) = obj.get("constraint").and_then(Value::as_str) else {
                continue;
            };
            let package = match obj.get("package").and_then(Value::as_str) {
                Some(p) => p.to_owned(),
                None => match default_package {
                    Some(d) => d.to_owned(),
                    None => continue,
                },
            };
            let parsed = crate::constraint::parse_constraints(constraint)
                .map_err(|e| RepoError::data(e.to_string()))?
                .constraint;
            let Some((_, wanted)) = map.iter().find(|(n, _)| *n == package) else {
                continue;
            };
            if !parsed.matches(wanted) {
                continue;
            }
            let entry = FilterEntry {
                package_name: package,
                constraint: parsed,
                list_name: list_name.clone(),
                url: obj.get("url").and_then(Value::as_str).map(str::to_owned),
                reason: obj.get("reason").and_then(Value::as_str).map(str::to_owned),
                id: obj.get("id").and_then(Value::as_str).map(str::to_owned),
                source: obj.get("source").and_then(Value::as_str).map(str::to_owned),
            };
            match result.iter_mut().find(|(l, _)| l == list_name) {
                Some((_, v)) => v.push(entry),
                None => result.push((list_name.clone(), vec![entry])),
            }
        }
    }
    Ok(result)
}

/// `PolicyConfig::RESERVED_NAMES` + `FUTURE_RESERVED_NAMES` : des noms
/// de liste qu'un dépôt ne peut pas annoncer ; le préfixe `ignore` est
/// réservé aussi.
const RESERVED_LIST_NAMES: &[&str] = &[
    "advisories",
    "abandoned",
    "package",
    "packages",
    "license",
    "licence",
    "licenses",
    "licences",
    "support",
    "maintenance",
    "security",
    "minimum-release-age",
];

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
    /// Cache des métadonnées au format de Composer (`cache-repo-dir`).
    pub cache: Option<crate::metacache::MetadataCache>,
    /// Le dépôt a déjà été signalé en mode dégradé (réseau en panne, cache
    /// utilisé) : un seul avertissement.
    degraded: std::cell::Cell<bool>,
    /// Option `filter` de la définition du dépôt : `None` = `false` (aucune
    /// liste), sinon les listes désactivées.
    pub user_filter: Option<Vec<String>>,
    /// `freshMetadataUrls` : un fichier de métadonnées a été chargé dans
    /// ce processus (les chemins `summary-url`/`api-url` des listes sont
    /// alors ignorés).
    fresh_metadata: std::cell::Cell<bool>,
    /// `FilterRepository` (`only` / `exclude` de la définition) : les noms
    /// que ce dépôt sert ; les chemins des avis et des listes s'y limitent.
    name_filter: Option<NameFilter>,
}

/// `only` (autoriser) ou `exclude` (refuser) une liste de motifs.
pub struct NameFilter {
    regex: Regex,
    only: bool,
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
            cache: None,
            degraded: std::cell::Cell::new(false),
            user_filter: Some(Vec::new()),
            fresh_metadata: std::cell::Cell::new(false),
            name_filter: None,
        })
    }

    /// `FilterRepository::__construct` : `only` ou `exclude` (pas les deux).
    pub fn set_name_filter(
        &mut self,
        only: Option<&Value>,
        exclude: Option<&Value>,
    ) -> Result<(), RepoError> {
        let patterns = |v: &Value, key: &str| -> Result<Vec<String>, RepoError> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .ok_or_else(|| {
                    RepoError::data(format!(
                        "\"{key}\" key for repository {} should be an array",
                        self.repo_name()
                    ))
                })
        };
        if only.is_some() && exclude.is_some() {
            return Err(RepoError::data(format!(
                "Only one of \"only\" and \"exclude\" can be specified for repository {}",
                self.repo_name()
            )));
        }
        let (list, is_only) = match (only, exclude) {
            (Some(o), _) => (patterns(o, "only")?, true),
            (_, Some(e)) => (patterns(e, "exclude")?, false),
            _ => return Ok(()),
        };
        let parts: Vec<String> = list
            .iter()
            .map(|n| crate::version::preg_quote(n).replace("\\*", ".*"))
            .collect();
        let regex = pcre2::bytes::RegexBuilder::new()
            .caseless(true)
            .build(&format!("^(?:{})\\z", parts.join("|")))
            .map_err(|e| RepoError::data(e.to_string()))?;
        self.name_filter = Some(NameFilter {
            regex,
            only: is_only,
        });
        Ok(())
    }

    /// `FilterRepository::isAllowed`.
    fn is_allowed(&self, name: &str) -> bool {
        match &self.name_filter {
            None => true,
            Some(f) => {
                let hit = f.regex.is_match(name.as_bytes()).unwrap_or(false);
                if f.only {
                    hit
                } else {
                    !hit
                }
            }
        }
    }

    /// `parseUserFilterConfig` de l'option `filter` du dépôt.
    pub fn set_user_filter(&mut self, raw: Option<&Value>) -> Result<(), RepoError> {
        self.user_filter = match raw {
            Some(Value::Bool(false)) => None,
            None | Some(Value::Null) | Some(Value::Bool(true)) => Some(Vec::new()),
            Some(Value::Object(m)) => {
                let mut disabled = Vec::new();
                for (list, v) in m {
                    if list.is_empty() {
                        return Err(RepoError::data(
                            "Repository \"filter\" keys must be non-empty list-name strings.",
                        ));
                    }
                    match v {
                        Value::Bool(true) => {}
                        Value::Bool(false) => disabled.push(list.clone()),
                        other => {
                            return Err(RepoError::data(format!(
                                "Repository \"filter\" entry for \"{list}\" must be a boolean; got {other}."
                            )))
                        }
                    }
                }
                Some(disabled)
            }
            Some(_) => {
                return Err(RepoError::data(
                    "Repository \"filter\" must be a boolean or an object mapping advertised list names to false.",
                ))
            }
        };
        Ok(())
    }

    /// `loadRootServerFile`, une fois.
    fn root_data(&self) -> Result<&RootData, RepoError> {
        self.root_data_max_age(None)
    }

    /// `loadRootServerFile($rootMaxAge)` : avec un âge maximal, un
    /// packages.json en cache plus récent est pris sans requête (les
    /// chemins des avis et des listes passent 600 s).
    fn root_data_max_age(&self, max_age: Option<u64>) -> Result<&RootData, RepoError> {
        if let Some(r) = self.root.get() {
            return Ok(r);
        }
        let fresh_enough = max_age.is_some_and(|max| {
            self.cache
                .as_ref()
                .and_then(|c| c.age("packages.json"))
                .is_some_and(|age| age <= max)
        });
        let data: Value = if fresh_enough {
            self.cached("packages.json")
                .map(|(v, _)| v)
                .ok_or_else(|| {
                    RepoError::transport(format!("{} not found", self.packages_json_url))
                })?
        } else {
            self.fetch_cached(&self.packages_json_url, "packages.json")?
                .ok_or_else(|| {
                    RepoError::transport(format!("{} not found", self.packages_json_url))
                })?
        };
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
            if let Some(sa) = data.get("security-advisories").and_then(Value::as_object) {
                let api_url = sa
                    .get("api-url")
                    .and_then(Value::as_str)
                    .map(|u| self.canonicalize_url(u));
                if api_url.is_none() && !r.has_available_package_list {
                    return Err(RepoError::data(format!(
                        "Invalid security advisory configuration on {}: If the repository does not provide a security-advisories.api-url then available-packages or available-package-patterns are required to be provided for performance reason.",
                        self.repo_name()
                    )));
                }
                r.security_advisories = Some(AdvisoryConfig {
                    metadata: !php_empty(sa.get("metadata")),
                    api_url,
                });
            }
            if let Some(f) = data.get("filter").and_then(Value::as_object) {
                let mut lists = Vec::new();
                if let Some(ls) = f.get("lists").and_then(Value::as_object) {
                    for (name, cfg) in ls {
                        if cfg
                            .as_object()
                            .is_some_and(|c| !php_empty(c.get("enabled")))
                            && !RESERVED_LIST_NAMES.contains(&name.as_str())
                            && !name.starts_with("ignore")
                        {
                            lists.push(name.clone());
                        }
                    }
                }
                let url_of = |k: &str| {
                    f.get(k)
                        .and_then(Value::as_str)
                        .filter(|u| !u.is_empty())
                        .map(|u| self.canonicalize_url(u))
                };
                r.filter = Some(FilterInfo {
                    metadata: !php_empty(f.get("metadata")),
                    lists,
                    summary_url: url_of("summary-url"),
                    api_url: url_of("api-url"),
                });
            }
        } else if non_empty("providers-url")
            || non_empty("providers")
            || non_empty("providers-includes")
            || has_providers
        {
            // Le protocole v1 n'est pas porté pour la résolution ; ses
            // packages.json restent lisibles pour ce qu'ils déclarent (avis,
            // listes), comme Composer le fait.
            r.v1_protocol = true;
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
                let included = self
                    .fetch_cached(&url, include)?
                    .ok_or_else(|| RepoError::transport(format!("{url} not found")))?;
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

    /// Lecture du cache : (JSON décodé, `last-modified`).
    fn cached(&self, cache_key: &str) -> Option<(Value, Option<String>)> {
        let bytes = self.cache.as_ref()?.read(cache_key)?;
        let v: Value = serde_json::from_slice(&bytes).ok()?;
        let lm = v
            .get("last-modified")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Some((v, lm))
    }

    /// `asyncFetchFile` + `Cache` : après la réponse, ce que Composer
    /// garde — 304 → le cache ; 404 → rien (pas écrit) ; 200 → le JSON,
    /// ré-encodé avec `last-modified` s'il y a l'en-tête, écrit tel quel
    /// sinon. Une erreur de transport avec un cache daté → mode dégradé.
    fn settle(
        &self,
        url: &str,
        cache_key: &str,
        cached: Option<(Value, Option<String>)>,
        result: Result<Fetched, RepoError>,
    ) -> Result<Option<Value>, RepoError> {
        // `fetchFile` (packages.json, includes) encode avec les flags 0,
        // `asyncFetchFile` (fichiers de paquets) sans échappement.
        let escaped = !cache_key.starts_with("provider-");
        match result {
            Ok(Fetched::NotModified) => Ok(cached.map(|(v, _)| v)),
            Ok(Fetched::NotFound) => Ok(None),
            Ok(Fetched::Body {
                bytes,
                last_modified,
            }) => {
                let data: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| RepoError::data(format!("{url}: invalid JSON: {e}")))?;
                if let Some(cache) = &self.cache {
                    match &last_modified {
                        Some(lm) => {
                            if let Some(encoded) =
                                crate::metacache::MetadataCache::with_last_modified(
                                    &data, lm, escaped,
                                )
                            {
                                cache.write(cache_key, &encoded);
                            }
                        }
                        None => cache.write(cache_key, &bytes),
                    }
                }
                Ok(Some(data))
            }
            Err(e) => {
                if let Some((v, Some(_))) = cached {
                    if !self.degraded.replace(true) {
                        eprintln!(
                            "Warning: {} could not be fully loaded ({}), package information was loaded from the local cache and may be out of date",
                            self.url, e.0
                        );
                    }
                    return Ok(Some(v));
                }
                Err(e)
            }
        }
    }

    /// Un fichier du dépôt, via le cache conditionnel.
    fn fetch_cached(&self, url: &str, cache_key: &str) -> Result<Option<Value>, RepoError> {
        let cached = self.cached(cache_key);
        let ims = cached.as_ref().and_then(|(_, lm)| lm.clone());
        let result = self.transport.fetch(url, ims.as_deref());
        self.settle(url, cache_key, cached, result)
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
            return Err(RepoError::data("startCachedAsyncDownload only supports v2 protocol composer repos with a metadata-url"));
        };
        let url = template.replace("%package%", &key);
        let cache_key = crate::metacache::MetadataCache::provider_key(&key);
        let data = self.fetch_cached(&url, &cache_key)?;
        self.fresh_metadata.set(true);
        let value = Self::parse_provider(package_name, data);
        self.fetched.borrow_mut().insert(key, value.clone());
        Ok(value)
    }

    /// `getRepoName`.
    pub fn repo_name(&self) -> String {
        format!("composer repo ({})", self.url)
    }

    /// `hasSecurityAdvisories`.
    pub fn has_security_advisories(&self) -> Result<bool, RepoError> {
        Ok(self
            .root_data_max_age(Some(600))?
            .security_advisories
            .as_ref()
            .is_some_and(|c| c.metadata || c.api_url.is_some()))
    }

    /// `getSecurityAdvisories` : les avis par nom pour les contraintes
    /// demandées — chemin métadonnées (fichiers p2, avis partiels) puis
    /// API (POST) pour ce qui reste. `allow_partial` faux = chargement
    /// complet exigé (erreur si un avis embarqué n'est que partiel et
    /// qu'aucune API ne peut le compléter).
    pub fn get_security_advisories(
        &self,
        map: &[(String, Constraint)],
        allow_partial: bool,
    ) -> Result<(Vec<String>, AdvisoriesByName), RepoError> {
        let root = self.root_data_max_age(Some(600))?;
        let Some(config) = &root.security_advisories else {
            return Ok((Vec::new(), Vec::new()));
        };
        let mut map: Vec<(String, Constraint)> = map
            .iter()
            .filter(|(n, _)| self.is_allowed(n))
            .cloned()
            .collect();
        if root.has_available_package_list {
            map.retain(|(n, _)| Self::contains(root, &n.to_lowercase()));
        }
        let mut advisories: AdvisoriesByName = Vec::new();
        let mut names_found: Vec<String> = Vec::new();
        let create = |data: &Value,
                      name: &str,
                      wanted: &Constraint|
         -> Result<Option<Advisory>, RepoError> {
            let Some(adv) = advisory_from_data(name, data) else {
                return Ok(None);
            };
            if !allow_partial && adv.complete.is_none() {
                return Err(RepoError::data(format!(
                    "Advisory for {name} could not be loaded as a full advisory from {}\n{data}",
                    self.repo_name()
                )));
            }
            if !adv.affected_versions.matches(wanted) {
                return Ok(None);
            }
            Ok(Some(adv))
        };
        if config.metadata && (allow_partial || config.api_url.is_none()) {
            let wanted: Vec<(String, String)> = map
                .iter()
                .map(|(n, _)| n.to_lowercase())
                .filter(|n| !is_platform_package(n) && n != "__root__")
                .map(|n| (n.clone(), n))
                .collect();
            self.prefetch(&wanted)?;
            let mut done: Vec<String> = Vec::new();
            for (name, constraint) in &map {
                let name = name.to_lowercase();
                if is_platform_package(&name) || name == "__root__" {
                    continue;
                }
                let Some(response) = self.provider(&name, &name)? else {
                    continue;
                };
                let Some(list) = response
                    .get("security-advisories")
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                names_found.push(name.clone());
                if !list.is_empty() {
                    let mut found = Vec::new();
                    for data in list {
                        if let Some(a) = create(data, &name, constraint)? {
                            found.push(a);
                        }
                    }
                    advisories.push((name.clone(), found));
                }
                done.push(name);
            }
            map.retain(|(n, _)| !done.contains(&n.to_lowercase()));
        }
        if let (Some(api_url), false) = (&config.api_url, map.is_empty()) {
            let body: Vec<String> = map
                .iter()
                .map(|(n, _)| format!("packages%5B%5D={}", urlencode(n)))
                .collect();
            let fetched = self.transport.post_form(api_url, &body.join("&"))?;
            let bytes = match fetched {
                Fetched::Body { bytes, .. } => bytes,
                Fetched::NotFound => {
                    return Err(RepoError::transport(format!(
                        "The \"{api_url}\" file could not be downloaded (HTTP/404)"
                    )))
                }
                Fetched::NotModified => Vec::new(),
            };
            let data: Value = serde_json::from_slice(&bytes)
                .map_err(|e| RepoError::data(format!("{api_url}: {e}")))?;
            let mut warned = false;
            for (name, list) in data
                .get("advisories")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let Some((_, constraint)) = map.iter().find(|(n, _)| n == name) else {
                    if !warned {
                        eprintln!(
                            "{} returned names which were not requested in response to the security-advisories API. {name} was not requested but is present in the response. Requested names were: {}",
                            self.repo_name(),
                            map.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
                        );
                        warned = true;
                    }
                    continue;
                };
                let list = list.as_array().cloned().unwrap_or_default();
                if !list.is_empty() {
                    let mut found = Vec::new();
                    for d in &list {
                        if let Some(a) = create(d, name, constraint)? {
                            found.push(a);
                        }
                    }
                    advisories.push((name.clone(), found));
                }
                names_found.push(name.clone());
            }
        }
        Ok((names_found, advisories))
    }

    /// `hasFilter` / `getFilterLists` : les listes annoncées, moins
    /// celles que l'option `filter` du dépôt désactive.
    pub fn get_filter_lists(&self) -> Result<Vec<String>, RepoError> {
        let Some(disabled) = &self.user_filter else {
            return Ok(Vec::new());
        };
        let root = self.root_data_max_age(Some(600))?;
        // `hasFilter()` : sans `metadata`, le dépôt n'est pas un fournisseur.
        let Some(f) = root.filter.as_ref().filter(|f| f.metadata) else {
            return Ok(Vec::new());
        };
        Ok(f.lists
            .iter()
            .filter(|l| !disabled.contains(l))
            .cloned()
            .collect())
    }

    /// `getFilter` : les entrées de liste pour les contraintes demandées —
    /// API (non portée : erreur), sinon résumé puis fichiers p2 des
    /// candidats, sinon fichiers p2 de tous les noms.
    pub fn get_filter(
        &self,
        map: &[(String, Constraint)],
        configured_lists: &[String],
    ) -> Result<FilterEntriesByList, RepoError> {
        let root = self.root_data_max_age(Some(600))?;
        let mut map: Vec<(String, Constraint)> = map
            .iter()
            .filter(|(n, _)| self.is_allowed(n))
            .cloned()
            .collect();
        if root.has_available_package_list {
            map.retain(|(n, _)| Self::contains(root, &n.to_lowercase()));
        }
        let fresh = self.fresh_metadata.get();
        if let Some(f) = &root.filter {
            if f.api_url.is_some() && !fresh {
                return Err(RepoError::data(format!(
                    "{}: a filter api-url is not supported by vivace yet",
                    self.repo_name()
                )));
            }
            if f.summary_url.is_some() && !fresh {
                let summary = self.load_filter_summary()?;
                let mut candidates: Vec<String> = Vec::new();
                for list in configured_lists {
                    let Some(packages) = summary.iter().find(|(l, _)| l == list) else {
                        continue;
                    };
                    for (package, constraint) in &packages.1 {
                        let Some((_, wanted)) = map.iter().find(|(n, _)| n == package) else {
                            continue;
                        };
                        if !matches!(wanted, Constraint::MatchAll)
                            && !crate::constraint::parse_constraints(constraint)
                                .map_err(|e| RepoError::data(e.to_string()))?
                                .constraint
                                .matches(wanted)
                        {
                            continue;
                        }
                        if !candidates.contains(package) {
                            candidates.push(package.clone());
                        }
                    }
                }
                map.retain(|(n, _)| candidates.contains(n));
            }
        }
        let wanted: Vec<(String, String)> = map
            .iter()
            .map(|(n, _)| n.to_lowercase())
            .filter(|n| !is_platform_package(n) && n != "__root__")
            .map(|n| (n.clone(), n))
            .collect();
        self.prefetch(&wanted)?;
        let mut filter: FilterEntriesByList = Vec::new();
        for (name, _) in &map {
            let name = name.to_lowercase();
            if is_platform_package(&name) || name == "__root__" {
                continue;
            }
            let Some(response) = self.provider(&name, &name)? else {
                continue;
            };
            let Some(raw) = response.get("filter").filter(|v| v.is_object()) else {
                continue;
            };
            for (list, entries) in build_filter_entries(raw, &map, Some(&name))? {
                match filter.iter_mut().find(|(l, _)| *l == list) {
                    Some((_, v)) => v.extend(entries),
                    None => filter.push((list, entries)),
                }
            }
        }
        Ok(filter)
    }

    /// `loadFilterSummary` : `summary.json` (cache `filter-summary.json`,
    /// requête conditionnelle) → liste → nom (minuscule) → contrainte.
    fn load_filter_summary(&self) -> Result<FilterSummary, RepoError> {
        let root = self.root_data_max_age(Some(600))?;
        let Some(url) = root.filter.as_ref().and_then(|f| f.summary_url.clone()) else {
            return Ok(Vec::new());
        };
        let data = self.fetch_cached(&url, "filter-summary.json")?;
        let Some(filter) = data
            .as_ref()
            .and_then(|d| d.get("filter"))
            .and_then(Value::as_object)
        else {
            return Err(RepoError::transport(format!(
                "Filter summary URL {url} returned 404 for {}",
                self.repo_name()
            )));
        };
        let mut summary: FilterSummary = Vec::new();
        for (list, packages) in filter {
            let Some(packages) = packages.as_object() else {
                return Err(RepoError::data(format!(
                    "Invalid filter summary received from {}: list \"{list}\" must map to an object of package => constraint",
                    self.repo_name()
                )));
            };
            let mut entries = Vec::new();
            for (name, constraint) in packages {
                let Some(c) = constraint.as_str() else {
                    return Err(RepoError::data(format!(
                        "Invalid filter summary received from {}: list \"{list}\" entries must be strings",
                        self.repo_name()
                    )));
                };
                entries.push((name.to_lowercase(), c.to_owned()));
            }
            summary.push((list.clone(), entries));
        }
        Ok(summary)
    }

    fn parse_provider(package_name: &str, data: Option<Value>) -> Option<std::rc::Rc<Value>> {
        let v = data?;
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
        let cached: Vec<Option<(Value, Option<String>)>> = todo
            .iter()
            .map(|(key, _, _)| self.cached(&crate::metacache::MetadataCache::provider_key(key)))
            .collect();
        let requests: Vec<Request> = todo
            .iter()
            .zip(&cached)
            .map(|((_, _, url), c)| (url.clone(), c.as_ref().and_then(|(_, lm)| lm.clone())))
            .collect();
        let results = self.transport.fetch_many(&requests);
        self.fresh_metadata.set(true);
        let mut settled = Vec::with_capacity(todo.len());
        for (((key, package_name, url), c), result) in todo.into_iter().zip(cached).zip(results) {
            let cache_key = crate::metacache::MetadataCache::provider_key(&key);
            let data = self.settle(&url, &cache_key, c, result)?;
            settled.push((key, Self::parse_provider(&package_name, data)));
        }
        let mut memo = self.fetched.borrow_mut();
        for (key, value) in settled {
            memo.insert(key, value);
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
                loader::load(&config, origin, true).map_err(|e| RepoError::data(e.0))?;
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
                let n = normalize(&pretty, None).map_err(|e| RepoError::data(e.0))?;
                data.insert("version_normalized".into(), Value::String(n));
            }
            Some(v) if v == DEFAULT_BRANCH_ALIAS => {
                let n = normalize(&pretty, None).map_err(|e| RepoError::data(e.0))?;
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
        if root.v1_protocol {
            return Err(RepoError::data(format!(
                "{}: Composer v1 repository protocol (providers) is not supported by vivace",
                self.url
            )));
        }
        if let Some(plain) = &root.plain {
            // `parent::loadPackages` (ArrayRepository) sur `getPackages()`.
            if self.members.get().is_none() {
                let configs: Vec<Value> = plain
                    .iter()
                    .map(|c| Self::with_notification_url(c, root))
                    .collect();
                let ids = loader::load_packages(&configs, origin, arena, true)
                    .map_err(|e| RepoError::data(e.0))?;
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
            let ids = loader::load_packages(&to_load, origin, arena, true)
                .map_err(|e| RepoError::data(e.0))?;
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
    locked_repository_with(lock, arena, true)
}

/// `Locker::getLockedRepository($withDevReqs)`.
pub fn locked_repository_with(
    lock: &Value,
    arena: &mut Vec<Package>,
    with_dev: bool,
) -> Result<Vec<usize>, RepoError> {
    let mut configs: Vec<Value> = lock
        .get("packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if with_dev {
        match lock.get("packages-dev").and_then(Value::as_array) {
            Some(dev) => configs.extend(dev.iter().cloned()),
            None => {
                return Err(RepoError::data(
                    "The lock file does not contain require-dev information, run install with the --no-dev option or delete it and run composer update to generate a new lock file.",
                ))
            }
        }
    }
    if configs.is_empty() {
        return Ok(Vec::new());
    }
    let ids = loader::load_packages(&configs, Origin::Locked, arena, false)
        .map_err(|e| RepoError::data(e.0))?;
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

/// `http_build_query` : encodage RFC 1738 d'une valeur (`/` → `%2F`).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::cell::RefCell;

    /// Transport factice : répond selon un script et note les requêtes.
    struct Scripted {
        responses: RefCell<Vec<Fetched>>,
        seen: std::rc::Rc<RefCell<Vec<Request>>>,
    }

    impl Transport for Scripted {
        fn fetch(&self, url: &str, ims: Option<&str>) -> Result<Fetched, RepoError> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), ims.map(str::to_owned)));
            let mut responses = self.responses.borrow_mut();
            assert!(
                !responses.is_empty(),
                "unexpected request: {url} (seen: {:?})",
                self.seen.borrow()
            );
            Ok(responses.remove(0))
        }
    }

    fn body(json: &str, lm: Option<&str>) -> Fetched {
        Fetched::Body {
            bytes: json.as_bytes().to_vec(),
            last_modified: lm.map(str::to_owned),
        }
    }

    #[test]
    fn revalidates_from_composer_cache() {
        let tmp = tempfile::tempdir().expect("tmp");
        let cache_dir = tmp.path().join("repo");
        let root = r#"{"packages": [], "metadata-url": "/p2/%package%.json"}"#;
        let provider = r#"{"packages": {"acme/lib": [{"name": "acme/lib", "version": "1.0.0", "version_normalized": "1.0.0.0"}]}}"#;

        // Premier run : 200 avec Last-Modified → écrit dans le cache.
        let t = Scripted {
            responses: RefCell::new(vec![
                body(root, Some("Sat, 12 Sep 2026 10:00:00 GMT")),
                body(provider, Some("Sun, 13 Sep 2026 09:00:00 GMT")),
                Fetched::NotFound,
            ]),
            seen: std::rc::Rc::new(RefCell::new(Vec::new())),
        };
        let mut repo =
            ComposerRepository::open("https://satis.example.org", Box::new(t)).expect("open");
        repo.cache = Some(crate::metacache::MetadataCache::new(&cache_dir, &repo.url));
        let mut arena = Vec::new();
        let (found, ids) = repo
            .load_packages(
                &[("acme/lib".to_owned(), Constraint::MatchAll)],
                &[("stable".to_owned(), 0)].into_iter().collect(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                Origin::Repository(2),
                &mut arena,
            )
            .expect("load");
        assert_eq!(found, vec!["acme/lib"]);
        assert_eq!(ids.len(), 1);
        let dir = cache_dir.join("https---satis.example.org");
        let cached = std::fs::read_to_string(dir.join("provider-acme~lib.json")).expect("cached");
        assert!(
            cached.ends_with(r#""last-modified":"Sun, 13 Sep 2026 09:00:00 GMT"}"#),
            "{cached}"
        );
        assert!(std::fs::read_to_string(dir.join("packages.json"))
            .expect("root cached")
            .contains(r#""metadata-url":"\/p2\/%package%.json""#));

        // Second run : If-Modified-Since envoyé, 304 → servi depuis le cache.
        let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
        let t = Scripted {
            responses: RefCell::new(vec![
                Fetched::NotModified,
                Fetched::NotModified,
                Fetched::NotFound,
            ]),
            seen: seen.clone(),
        };
        let mut repo =
            ComposerRepository::open("https://satis.example.org", Box::new(t)).expect("open");
        repo.cache = Some(crate::metacache::MetadataCache::new(&cache_dir, &repo.url));
        let mut arena = Vec::new();
        let (found, ids) = repo
            .load_packages(
                &[("acme/lib".to_owned(), Constraint::MatchAll)],
                &[("stable".to_owned(), 0)].into_iter().collect(),
                &BTreeMap::new(),
                &BTreeMap::new(),
                Origin::Repository(2),
                &mut arena,
            )
            .expect("load");
        assert_eq!(found, vec!["acme/lib"]);
        assert_eq!(arena[ids[0]].version, "1.0.0.0");
        let seen = seen.borrow();
        assert_eq!(seen[0].1.as_deref(), Some("Sat, 12 Sep 2026 10:00:00 GMT"));
        assert_eq!(seen[1].0, "https://satis.example.org/p2/acme/lib.json");
        assert_eq!(seen[1].1.as_deref(), Some("Sun, 13 Sep 2026 09:00:00 GMT"));
    }
}
