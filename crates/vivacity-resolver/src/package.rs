//! Package model of the resolver: port of `Composer\Package\{BasePackage,
//! Package, CompletePackage, AliasPackage, Link}` reduced to what the pool
//! and the solver read, plus the raw JSON of the version (to write the lock
//! identically). Packages live in an arena (`Vec<Package>`) and refer to
//! each other by index, as Composer does by object identity.

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
    /// `BasePackage::$supportedLinkTypes[...]['description']`.
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

/// `Composer\Package\Link`, plus its key in the PHP array holding it: the
/// target in general, the bare name for the platform's `lib-*`
/// (`PlatformRepository::addLibrary`), none (numeric key) for the
/// `self.version` links added by `AliasPackage`. `Pool::match` looks up by
/// key.
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
    /// Link keyed by its target (the `ArrayLoader::parseLinks` case).
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

/// PHP array of links: insertion order, one entry per key (the last write
/// wins, at the position of the first); keyless links are the numeric-key
/// entries of `array_merge`.
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
    /// `unset($links[$name])`: removes the entry with that key, if any.
    pub fn remove(&mut self, key: &str) {
        self.0.retain(|l| l.key.as_deref() != Some(key));
    }

    pub fn get(&self, key: &str) -> Option<&Link> {
        self.0.iter().find(|l| l.key.as_deref() == Some(key))
    }
    /// `isset($links[0])`: at least one numeric-key entry.
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

/// Where a package comes from (`getRepository()` in Composer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Root,
    Platform,
    Locked,
    /// Index of the remote repository in the repository list.
    Repository(usize),
    /// No repository: root alias created by `PoolBuilder::loadPackage`.
    Detached,
    /// `$resultRepo` of `extractDevPackages`: packages of the first solve
    /// reloaded from their dump.
    Result,
}

#[derive(Debug, Clone)]
pub struct Package {
    /// Lowercased name (`getName`).
    pub name: String,
    pub pretty_name: String,
    /// Normalized version (`getVersion`).
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
    /// Raw JSON of the version (p2 metadata or lock entry).
    pub raw: Value,
    pub origin: Origin,
    /// `AliasPackage`: arena index of the aliased package.
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

    /// `getNames()`: name + targets of provide and replace.
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

    /// `AliasPackage::__construct`: copy of the package with the alias
    /// version, the alias stability, and the `self.version` links rewritten
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
                // `array_merge($links, $newLinks)`: the self.version links
                // are appended (numeric keys), so the same target twice.
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
