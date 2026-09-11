//! vivace-resolver — port de la résolution de Composer 2.10.3 : versions et
//! contraintes (composer/semver), métadonnées Packagist v2, construction du
//! pool, puis le solveur. Chaque module est un port de la source vendorée
//! dans docs/reference/resolver/, vérifié par un oracle contre le phar.

pub mod constraint;
pub mod intervals;
pub mod phpver;
pub mod version;
