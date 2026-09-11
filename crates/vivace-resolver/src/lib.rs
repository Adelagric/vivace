//! vivace-resolver — port de la résolution de Composer 2.10.3 : versions et
//! contraintes (composer/semver), métadonnées Packagist v2, construction du
//! pool, puis le solveur. Chaque module est un port de la source vendorée
//! dans docs/reference/resolver/, vérifié par un oracle contre le phar.

pub mod constraint;
pub mod decisions;
pub mod intervals;
pub mod loader;
pub mod optimizer;
pub mod package;
pub mod phpver;
pub mod platform;
pub mod platform_filter;
pub mod policy;
pub mod pool;
pub mod repository;
pub mod root;
pub mod rule;
pub mod rules_gen;
pub mod session;
pub mod solver;
pub mod transaction;
pub mod version;
pub mod watch;
