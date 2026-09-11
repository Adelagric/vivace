//! Modèle de paquet du résolveur — port de `Composer\Package\{BasePackage,
//! Package, CompletePackage, AliasPackage, Link}` réduit à ce que le pool et
//! le solveur lisent, plus le JSON brut de la version (pour écrire le lock à
//! l'identique). Les paquets vivent dans une arène (`Vec<Package>`) et se
//! désignent par index, comme Composer par identité d'objet.

use crate::constraint::{Constraint, Op};
use crate::version::parse_stability;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    Require,
    DevRequire,
    Conflict,
    Provide,
    Replace,
}

impl LinkType {
    /// `BasePackage::$supportedLinkTypes[…]['description']`.
    pub fn description(self) -> &'static str {
        match self {
            LinkType::Require => "requires",
            LinkType::DevRequire => "requires (for development)",
            LinkType::Conflict => "conflicts",
            LinkType::Provide => "provides",
            LinkType::Replace => "replaces",
        }
    }

    pub fn json_key(self) -> &'static str {
        match self {
            LinkType::Require => "require",
            LinkType::DevRequire => "require-dev",
            LinkType::Conflict => "conflict",
            LinkType::Provide => "provide",
            LinkType::Replace => "replace",
        }
    }
}

/// `Composer\Package\Link`, plus sa clé dans le tableau PHP qui le porte :
/// la cible en général, le nom nu pour les `lib-*` de la plateforme
/// (`PlatformRepository::addLibrary`), aucune (clé numérique) pour les liens
/// `self.version` ajoutés par `AliasPackage`. `Pool::match` cherche par clé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub key: Option<String>,
    pub source: String,
    pub target: String,
    pub constraint: Constraint,
    pub pretty_constraint: String,
    pub kind: LinkType,
}

impl Link {
    /// Lien indexé par sa cible (cas `ArrayLoader::parseLinks`).
    pub fn new(
        source: &str,
        target: &str,
        constraint: Constraint,
        pretty_constraint: &str,
        kind: LinkType,
    ) -> Link {
        Link {
            key: Some(target.to_owned()),
            source: source.to_owned(),
            target: target.to_owned(),
            constraint,
            pretty_constraint: pretty_constraint.to_owned(),
            kind,
        }
    }
}

/// Tableau PHP de liens : ordre d'insertion, une entrée par clé (la
/// dernière écriture gagne, à la position de la première) ; les liens sans
/// clé sont les entrées à clé numérique d'`array_merge`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Links(pub Vec<Link>);

impl Links {
    pub fn insert(&mut self, link: Link) {
        if link.key.is_some() {
            if let Some(existing) = self.0.iter_mut().find(|l| l.key == link.key) {
                *existing = link;
                return;
            }
        }
        self.0.push(link);
    }
    /// `isset($links[$key])` / `$links[$key]`.
    pub fn get(&self, key: &str) -> Option<&Link> {
        self.0.iter().find(|l| l.key.as_deref() == Some(key))
    }
    /// `isset($links[0])` : au moins une entrée à clé numérique.
    pub fn has_numeric_keys(&self) -> bool {
        self.0.iter().any(|l| l.key.is_none())
    }
    pub fn iter(&self) -> impl Iterator<Item = &Link> {
        self.0.iter()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub kind: String,
    pub url: String,
    pub reference: Option<String>,
}

/// D'où vient un paquet (`getRepository()` chez Composer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Root,
    Platform,
    Locked,
    /// Index du dépôt distant dans la liste des dépôts.
    Repository(usize),
    /// Aucun dépôt : alias racine créé par `PoolBuilder::loadPackage`.
    Detached,
    /// `$resultRepo` d'`extractDevPackages` : paquets du premier solve
    /// rechargés depuis leur dump.
    Result,
}

#[derive(Debug, Clone)]
pub struct Package {
    /// Nom en minuscules (`getName`).
    pub name: String,
    pub pretty_name: String,
    /// Version normalisée (`getVersion`).
    pub version: String,
    pub pretty_version: String,
    pub package_type: String,
    pub stability: &'static str,
    pub is_default_branch: bool,
    pub source: Option<SourceRef>,
    pub dist: Option<SourceRef>,
    pub requires: Links,
    pub dev_requires: Links,
    pub conflicts: Links,
    pub provides: Links,
    pub replaces: Links,
    /// JSON brut de la version (métadonnées p2 ou entrée de lock).
    pub raw: Value,
    pub origin: Origin,
    /// `AliasPackage` : index du paquet aliasé dans l'arène.
    pub alias_of: Option<usize>,
    /// `AliasPackage::isRootPackageAlias`.
    pub root_package_alias: bool,
    /// `AliasPackage::hasSelfVersionRequires`.
    pub has_self_version_requires: bool,
}

impl Package {
    pub fn new(pretty_name: &str, version: &str, pretty_version: &str, origin: Origin) -> Package {
        Package {
            name: pretty_name.to_lowercase(),
            pretty_name: pretty_name.to_owned(),
            version: version.to_owned(),
            pretty_version: pretty_version.to_owned(),
            package_type: "library".to_owned(),
            stability: parse_stability(version),
            is_default_branch: false,
            source: None,
            dist: None,
            requires: Links::default(),
            dev_requires: Links::default(),
            conflicts: Links::default(),
            provides: Links::default(),
            replaces: Links::default(),
            raw: Value::Null,
            origin,
            alias_of: None,
            root_package_alias: false,
            has_self_version_requires: false,
        }
    }

    pub fn is_alias(&self) -> bool {
        self.alias_of.is_some()
    }

    pub fn is_dev(&self) -> bool {
        self.stability == "dev"
    }

    /// `getNames()` : nom + cibles des provide et replace.
    pub fn names(&self, provides: bool) -> Vec<String> {
        let mut names: Vec<String> = vec![self.name.clone()];
        if provides {
            for l in self.provides.iter() {
                if !names.contains(&l.target) {
                    names.push(l.target.clone());
                }
            }
        }
        for l in self.replaces.iter() {
            if !names.contains(&l.target) {
                names.push(l.target.clone());
            }
        }
        names
    }

    pub fn unique_name(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }

    pub fn pretty_string(&self) -> String {
        format!("{} {}", self.pretty_name, self.pretty_version)
    }

    pub fn source_reference(&self) -> Option<&str> {
        self.source.as_ref().and_then(|s| s.reference.as_deref())
    }

    pub fn dist_reference(&self) -> Option<&str> {
        self.dist.as_ref().and_then(|s| s.reference.as_deref())
    }

    pub fn links(&self, kind: LinkType) -> &Links {
        match kind {
            LinkType::Require => &self.requires,
            LinkType::DevRequire => &self.dev_requires,
            LinkType::Conflict => &self.conflicts,
            LinkType::Provide => &self.provides,
            LinkType::Replace => &self.replaces,
        }
    }

    /// `AliasPackage::__construct` : copie du paquet avec la version de
    /// l'alias, la stabilité de l'alias, et les liens `self.version` réécrits
    /// (`replaceSelfVersionDependencies`).
    pub fn alias(&self, alias_of: usize, version: &str, pretty_version: &str) -> Package {
        let mut a = self.clone();
        a.version = version.to_owned();
        a.pretty_version = pretty_version.to_owned();
        a.stability = parse_stability(version);
        a.alias_of = Some(alias_of);
        a.root_package_alias = false;
        a.has_self_version_requires = false;
        let pretty = if pretty_version == crate::version::DEFAULT_BRANCH_ALIAS {
            self.pretty_version.clone()
        } else {
            pretty_version.to_owned()
        };
        let rewrite = |links: &Links, kind: LinkType, has_self: &mut bool| -> Links {
            let mut out = Links::default();
            if matches!(
                kind,
                LinkType::Conflict | LinkType::Provide | LinkType::Replace
            ) {
                // `array_merge($links, $newLinks)` : les liens self.version
                // sont ajoutés en plus (clés numériques) — même cible deux fois.
                let mut extra: Vec<Link> = Vec::new();
                for l in links.iter() {
                    out.0.push(l.clone());
                    if l.pretty_constraint == "self.version" {
                        extra.push(Link {
                            key: None,
                            source: l.source.clone(),
                            target: l.target.clone(),
                            constraint: Constraint::new(Op::Eq, version),
                            pretty_constraint: pretty.clone(),
                            kind,
                        });
                    }
                }
                out.0.extend(extra);
            } else {
                for l in links.iter() {
                    if l.pretty_constraint == "self.version" {
                        if kind == LinkType::Require {
                            *has_self = true;
                        }
                        out.0.push(Link {
                            key: l.key.clone(),
                            source: l.source.clone(),
                            target: l.target.clone(),
                            constraint: Constraint::new(Op::Eq, version),
                            pretty_constraint: pretty.clone(),
                            kind,
                        });
                    } else {
                        out.0.push(l.clone());
                    }
                }
            }
            out
        };
        let mut has_self = false;
        a.requires = rewrite(&self.requires, LinkType::Require, &mut has_self);
        a.dev_requires = rewrite(&self.dev_requires, LinkType::DevRequire, &mut has_self);
        a.conflicts = rewrite(&self.conflicts, LinkType::Conflict, &mut has_self);
        a.provides = rewrite(&self.provides, LinkType::Provide, &mut has_self);
        a.replaces = rewrite(&self.replaces, LinkType::Replace, &mut has_self);
        a.has_self_version_requires = has_self;
        a
    }
}
