//! vivacity-resolver: port of Composer 2.10.3's dependency resolution: versions
//! and constraints (composer/semver), Packagist v2 metadata, pool
//! construction, then the solver. Each module is a port of the source vendored
//! under docs/reference/resolver/, checked by an oracle against the phar.

pub mod config_source;
pub mod constraint;
pub mod decisions;
pub mod intervals;
pub mod json_manipulator;
pub mod loader;
pub mod lockfile;
pub mod metacache;
pub mod optimizer;
pub mod package;
pub mod phpver;
pub mod platform;
pub mod platform_filter;
pub mod policy;
pub mod policy_config;
pub mod pool;
pub mod pool_filters;
pub mod problem;
pub mod repository;
pub mod root;
pub mod rule;
pub mod rules_gen;
pub mod session;
pub mod solver;
pub mod transaction;
pub mod version;
pub mod version_selector;
pub mod watch;
