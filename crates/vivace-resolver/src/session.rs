//! Mise en place d'un `update` : ce que `Factory::createComposer` puis
//! `Installer::doUpdate` font avant `createPool` — racine, plateforme,
//! dépôts (config globale + composer.json, mêmes règles de fusion que
//! `Config::merge`), dépôt du lock, `Request`. Réplique de
//! tools/oracle-pool.php côté Rust.

use crate::constraint::{Constraint, Op};
use crate::optimizer::PoolOptimizer;
use crate::package::Package;
use crate::platform::{platform_packages, probe};
use crate::platform_filter::PlatformRequirementFilter;
use crate::policy::DefaultPolicy;
use crate::pool::{OrderedMap, Pool, PoolError, Repository, RepositorySet, Request};
use crate::repository::{locked_repository, ComposerRepository, FileTransport};
use crate::root::RootPackage;
use crate::solver::{SolveError, Solver};
use crate::transaction::LockTransaction;
use crate::version::{parse_stability, regex, stability_rank};
use pcre2::bytes::Regex;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SessionError(pub String);

impl From<PoolError> for SessionError {
    fn from(e: PoolError) -> SessionError {
        SessionError(e.0)
    }
}

/// `Config::$repositories` après fusion : (nom ou index, définition).
#[derive(Debug, Clone, PartialEq)]
pub struct RepoConfig {
    pub key: RepoKey,
    pub definition: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoKey {
    Named(String),
    Indexed(u64),
}

/// `Config::merge` pour la clé `repositories`, appliquée dans l'ordre
/// (défauts, config globale, composer.json).
pub fn merge_repositories(current: &mut Vec<RepoConfig>, new: &Value) {
    static PACKAGIST: OnceLock<Regex> = OnceLock::new();
    let entries: Vec<(RepoKey, Value)> = match new {
        Value::Array(list) => list
            .iter()
            .enumerate()
            .map(|(i, v)| (RepoKey::Indexed(i as u64), v.clone()))
            .collect(),
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let key = match k.parse::<u64>() {
                    Ok(i) if i.to_string() == *k => RepoKey::Indexed(i),
                    _ => RepoKey::Named(k.clone()),
                };
                (key, v.clone())
            })
            .collect(),
        _ => return,
    };
    if entries.is_empty() {
        return;
    }
    current.reverse();
    // `disableRepoByName((string) $name)` : une chaîne numérique retombe
    // sur la clé entière du tableau PHP.
    let disable = |current: &mut Vec<RepoConfig>, name: &str| {
        let key = match name.parse::<u64>() {
            Ok(i) if i.to_string() == name => RepoKey::Indexed(i),
            _ => RepoKey::Named(name.to_owned()),
        };
        if current.iter().any(|r| r.key == key) {
            current.retain(|r| r.key != key);
        } else if name == "packagist" {
            current.retain(|r| r.key != RepoKey::Named("packagist.org".into()));
        }
    };
    for (key, repository) in entries.into_iter().rev() {
        if repository == Value::Bool(false) {
            let name = match &key {
                RepoKey::Named(n) => n.clone(),
                RepoKey::Indexed(i) => i.to_string(),
            };
            disable(current, &name);
            continue;
        }
        if let Some(obj) = repository.as_object() {
            if obj.len() == 1 && obj.values().next() == Some(&Value::Bool(false)) {
                disable(current, obj.keys().next().map(String::as_str).unwrap_or(""));
                continue;
            }
        }
        if repository.get("type").and_then(Value::as_str) == Some("composer") {
            if let Some(url) = repository.get("url").and_then(Value::as_str) {
                let re = regex(
                    &PACKAGIST,
                    r"^https?://(?:[a-z0-9-.]+\.)?packagist.org(/|$)",
                    false,
                );
                if re.is_match(url.as_bytes()).unwrap_or(false) {
                    disable(current, "packagist.org");
                }
            }
        }
        match key {
            RepoKey::Indexed(i) => {
                if current.iter().any(|r| r.key == RepoKey::Indexed(i)) {
                    let next = current
                        .iter()
                        .filter_map(|r| match r.key {
                            RepoKey::Indexed(j) => Some(j + 1),
                            _ => None,
                        })
                        .max()
                        .unwrap_or(0);
                    current.push(RepoConfig {
                        key: RepoKey::Indexed(next),
                        definition: repository,
                    });
                } else {
                    current.push(RepoConfig {
                        key: RepoKey::Indexed(i),
                        definition: repository,
                    });
                }
            }
            RepoKey::Named(name) => {
                let name = if name == "packagist" {
                    "packagist.org".to_owned()
                } else {
                    name
                };
                let key = RepoKey::Named(name);
                if let Some(slot) = current.iter_mut().find(|r| r.key == key) {
                    slot.definition = repository;
                } else {
                    current.push(RepoConfig {
                        key,
                        definition: repository,
                    });
                }
            }
        }
    }
    current.reverse();
}

/// Configuration fusionnée utile au résolveur.
#[derive(Debug, Clone, Default)]
pub struct MergedConfig {
    pub repositories: Vec<RepoConfig>,
    /// `config.platform` (la dernière définition remplace la précédente).
    pub platform: Map<String, Value>,
}

impl MergedConfig {
    /// Défauts + `COMPOSER_HOME/config.json` + composer.json, comme
    /// `Factory::createConfig` puis `Config::merge($localConfig)`.
    pub fn load(
        manifest: &Value,
        composer_home: Option<&Path>,
    ) -> Result<MergedConfig, SessionError> {
        let mut cfg = MergedConfig {
            repositories: vec![RepoConfig {
                key: RepoKey::Named("packagist.org".into()),
                definition: serde_json::json!({"type": "composer", "url": "https://repo.packagist.org"}),
            }],
            platform: Map::new(),
        };
        if let Some(home) = composer_home {
            let global = home.join("config.json");
            if global.is_file() {
                let text = std::fs::read_to_string(&global)
                    .map_err(|e| SessionError(format!("{}: {e}", global.display())))?;
                let v: Value = serde_json::from_str(&text)
                    .map_err(|e| SessionError(format!("{}: {e}", global.display())))?;
                cfg.merge(&v);
            }
        }
        cfg.merge(manifest);
        Ok(cfg)
    }

    fn merge(&mut self, config: &Value) {
        // `$this->config['platform'] = $val` : toute valeur remplace la
        // précédente (`[]` ou `null` la vident).
        if let Some(platform) = config.get("config").and_then(|c| c.get("platform")) {
            self.platform = platform.as_object().cloned().unwrap_or_default();
        }
        if let Some(repos) = config.get("repositories") {
            merge_repositories(&mut self.repositories, repos);
        }
    }
}

/// Résultat d'un `solve` : la transaction et de quoi le comparer à Composer.
pub struct SolveReport {
    pub transaction: LockTransaction,
    /// Littéraux décidés, dans l'ordre.
    pub decisions: Vec<i64>,
    /// `getRuleSetSize()`.
    pub rules: usize,
    /// Règles apprises (conflits rencontrés).
    pub learned: usize,
}

/// Tout ce que `Installer::doUpdate` a en main juste avant `createPool`.
pub struct UpdateSession {
    pub arena: Vec<Package>,
    pub root: RootPackage,
    /// Index d'arène de la racine figée (requires vidés) et de son alias.
    pub fixed_root: usize,
    pub fixed_root_alias: Option<usize>,
    pub platform: Vec<usize>,
    pub locked: Option<Vec<usize>>,
    pub set: RepositorySet,
    pub request: Request,
    pub config: MergedConfig,
    pub dev_mode: bool,
}

impl UpdateSession {
    pub fn prepare(
        project_dir: &Path,
        composer_home: Option<&Path>,
        dev_mode: bool,
    ) -> Result<UpdateSession, SessionError> {
        // Pas encore de mise à jour partielle par cette entrée (R3).
        let partial_update = false;
        let manifest_path = project_dir.join("composer.json");
        let manifest_text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| SessionError(format!("{}: {e}", manifest_path.display())))?;
        let manifest: Value = serde_json::from_str(&manifest_text)
            .map_err(|e| SessionError(format!("{}: {e}", manifest_path.display())))?;
        let config = MergedConfig::load(&manifest, composer_home)?;
        let root = RootPackage::load(&manifest, project_dir).map_err(|e| SessionError(e.0))?;
        let probed = probe().map_err(|e| SessionError(e.0))?;
        let platform_pkgs =
            platform_packages(&probed, &config.platform).map_err(|e| SessionError(e.0))?;

        let mut arena: Vec<Package> = Vec::new();

        // `$fixedRootPackage = clone $package; setRequires([]); setDevRequires([])`.
        let mut fixed = root.package.clone();
        fixed.requires = Default::default();
        fixed.dev_requires = Default::default();
        arena.push(fixed);
        let fixed_root = 0;
        let fixed_root_alias = root.branch_alias.as_ref().map(|(normalized, pretty)| {
            let a = arena[fixed_root].alias(fixed_root, normalized, pretty);
            arena.push(a);
            arena.len() - 1
        });
        let root_members: Vec<usize> = fixed_root_alias
            .into_iter()
            .chain(std::iter::once(fixed_root))
            .collect();

        let mut platform: Vec<usize> = Vec::new();
        for p in platform_pkgs {
            arena.push(p);
            platform.push(arena.len() - 1);
        }

        // `Locker::isLocked()` = fichier présent et `isset($data['packages'])` ;
        // un lock illisible est ignoré pour une mise à jour complète
        // (`doUpdate` avale la ParsingException), fatal pour une partielle.
        let lock_path = project_dir.join("composer.lock");
        let lock: Option<Value> = if lock_path.is_file() {
            let text = std::fs::read_to_string(&lock_path)
                .map_err(|e| SessionError(format!("{}: {e}", lock_path.display())))?;
            match serde_json::from_str::<Value>(&text) {
                Ok(v) if v.get("packages").is_some_and(|p| !p.is_null()) => Some(v),
                Ok(_) => None,
                Err(e) => {
                    if partial_update {
                        return Err(SessionError(format!(
                            "\"{}\" does not contain valid JSON\n{e}",
                            lock_path.display()
                        )));
                    }
                    None
                }
            }
        } else {
            None
        };
        let locked = match &lock {
            Some(v) => Some(locked_repository(v, &mut arena).map_err(|e| SessionError(e.0))?),
            None => None,
        };

        // `$stabilityFlags[$package->getName()] = STABILITIES[parseStability($package->getVersion())]`
        // : la version vue est celle de l'alias racine s'il existe.
        let mut stability_flags = root.stability_flags.clone();
        let root_version = match fixed_root_alias {
            Some(a) => arena[a].version.clone(),
            None => arena[fixed_root].version.clone(),
        };
        stability_flags.insert(
            root.package.name.clone(),
            stability_rank(parse_stability(&root_version)),
        );

        // `createRepositorySet(forUpdate)` et `requirePackagesForUpdate(…, true)`
        // prennent toujours require + require-dev, `--no-dev` ou pas ; et avec
        // un alias racine, `$this->package` est le RootAliasPackage, dont les
        // liens `self.version` visent la version de l'alias.
        let mut root_requires: OrderedMap<Constraint> = OrderedMap::default();
        let requires = match &root.branch_alias {
            Some((normalized, pretty)) => {
                let aliased = root.package.alias(0, normalized, pretty);
                let mut all = aliased.requires.clone();
                for l in aliased.dev_requires.iter() {
                    all.insert(l.clone());
                }
                all
            }
            None => root.all_requires(),
        };
        for link in requires.iter() {
            root_requires.insert(&link.target, link.constraint.clone());
        }

        let mut set = RepositorySet::new(
            &root.minimum_stability,
            stability_flags,
            &root.aliases,
            root.references.clone(),
            root_requires,
            Default::default(),
        );
        set.add_repository(Repository::Root(root_members));
        set.add_repository(Repository::Platform(platform.clone()));
        for repo in &config.repositories {
            set.add_repository(open_repository(repo)?);
        }
        if let Some(ids) = &locked {
            set.add_repository(Repository::Locked(ids.clone()));
        }

        let mut request = Request::new(locked.clone());
        if let Some(a) = fixed_root_alias {
            request.fix_package(a);
        }
        request.fix_package(fixed_root);
        for &p in &platform {
            let provided = arena[fixed_root]
                .provides
                .get(&arena[p].name)
                .map(|l| l.constraint.clone());
            let provided_here = provided
                .is_some_and(|c| c.matches(&Constraint::new(Op::Eq, arena[p].version.clone())));
            if !provided_here {
                request.fix_package(p);
            }
        }
        for link in requires.iter() {
            request.require_name(&link.target, Some(link.constraint.clone()))?;
        }

        Ok(UpdateSession {
            arena,
            root,
            fixed_root,
            fixed_root_alias,
            platform,
            locked,
            set,
            request,
            config,
            dev_mode,
        })
    }

    pub fn create_pool(&mut self) -> Result<Pool, SessionError> {
        Ok(self.set.create_pool(&mut self.request, &mut self.arena)?)
    }

    /// `Installer::createPolicy(true, …)` sans `--prefer-lowest` ni
    /// `--minimal-changes`.
    pub fn policy(&self) -> DefaultPolicy {
        DefaultPolicy::new(self.root.prefer_stable, false, None)
    }

    /// `createPool` avec le PoolOptimizer (sauf `COMPOSER_POOL_OPTIMIZER=0`),
    /// comme `Installer::doUpdate`.
    pub fn create_optimized_pool(
        &mut self,
        policy: &mut DefaultPolicy,
    ) -> Result<Pool, SessionError> {
        let pool = self.create_pool()?;
        if std::env::var("COMPOSER_POOL_OPTIMIZER").as_deref() == Ok("0") {
            return Ok(pool);
        }
        Ok(PoolOptimizer::new().optimize(&self.request, &pool, &self.arena, policy))
    }

    /// `Solver::solve` sur ce pool ; rend la transaction et les décisions
    /// (littéraux du pool, dans l'ordre) avec la taille du jeu de règles.
    pub fn solve(
        &self,
        pool: &Pool,
        policy: &mut DefaultPolicy,
        filter: &PlatformRequirementFilter,
    ) -> Result<SolveReport, SolveError> {
        let mut solver = Solver::new(pool, &self.arena);
        let transaction = solver.solve(&self.request, policy, filter)?;
        let decisions = solver.decisions.queue.iter().map(|d| d.literal).collect();
        Ok(SolveReport {
            learned: solver
                .rules
                .ids_of_type(crate::rule::RuleType::Learned)
                .len(),
            rules: solver.rule_set_size(),
            decisions,
            transaction,
        })
    }
}

/// `RepositoryManager::createRepository` restreint aux dépôts `composer`
/// joignables en `file://` (les autres types arrivent avec R3).
fn open_repository(repo: &RepoConfig) -> Result<Repository, SessionError> {
    let def = &repo.definition;
    let kind = def.get("type").and_then(Value::as_str).ok_or_else(|| {
        SessionError(format!(
            "Repository \"{}\" ({def}) must have a type defined",
            key_string(&repo.key)
        ))
    })?;
    if kind != "composer" {
        return Err(SessionError(format!(
            "repository type \"{kind}\" is not supported by vivace update yet ({})",
            key_string(&repo.key)
        )));
    }
    if def.get("only").is_some() || def.get("exclude").is_some() || def.get("canonical").is_some() {
        return Err(SessionError(format!(
            "repository filters (only/exclude/canonical) are not supported by vivace update yet ({})",
            key_string(&repo.key)
        )));
    }
    let url = def
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| SessionError(format!("repository {} has no url", key_string(&repo.key))))?;
    if !url.starts_with("file://") {
        return Err(SessionError(format!(
            "remote composer repositories are not supported by vivace update yet ({url})"
        )));
    }
    let repo =
        ComposerRepository::open(url, Box::new(FileTransport)).map_err(|e| SessionError(e.0))?;
    Ok(Repository::Composer(Box::new(repo)))
}

fn key_string(key: &RepoKey) -> String {
    match key {
        RepoKey::Named(n) => n.clone(),
        RepoKey::Indexed(i) => i.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(repos: &[RepoConfig]) -> Vec<String> {
        repos.iter().map(|r| key_string(&r.key)).collect()
    }

    #[test]
    fn merges_repositories_like_composer_config() {
        let mut cfg = MergedConfig::load(&json!({}), None).unwrap();
        assert_eq!(names(&cfg.repositories), vec!["packagist.org"]);
        // config globale : snapshot + packagist désactivé.
        cfg.merge(&json!({"repositories": {"snapshot": {"type": "composer", "url": "file:///s"}, "packagist.org": false}}));
        assert_eq!(names(&cfg.repositories), vec!["snapshot"]);
        // composer.json : un dépôt indexé passe devant.
        cfg.merge(&json!({"repositories": [{"type": "composer", "url": "https://packages.drupal.org/8"}]}));
        assert_eq!(names(&cfg.repositories), vec!["0", "snapshot"]);
        // deux indexés alors que 0 existe : 1 prend sa clé, 0 est renuméroté
        // (`$this->repositories[] = …`) ; les nouveaux restent devant.
        cfg.merge(
            &json!({"repositories": [{"type": "vcs", "url": "a"}, {"type": "vcs", "url": "b"}]}),
        );
        assert_eq!(names(&cfg.repositories), vec!["2", "1", "0", "snapshot"]);
        assert_eq!(cfg.repositories[0].definition["url"], "a");
        assert_eq!(cfg.repositories[1].definition["url"], "b");
        assert_eq!(
            cfg.repositories[2].definition["url"],
            "https://packages.drupal.org/8"
        );
    }

    #[test]
    fn packagist_url_disables_default() {
        let mut cfg = MergedConfig::load(&json!({}), None).unwrap();
        cfg.merge(
            &json!({"repositories": [{"type": "composer", "url": "https://repo.packagist.org"}]}),
        );
        assert_eq!(names(&cfg.repositories), vec!["0"]);
        let mut cfg = MergedConfig::load(&json!({}), None).unwrap();
        cfg.merge(&json!({"repositories": [{"packagist": false}]}));
        assert!(cfg.repositories.is_empty());
    }
}
