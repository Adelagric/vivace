//! vivace-autoload — génération d'autoloader compatible Composer 2.10.3.

pub mod classmap;
pub mod generator;
pub mod natsort;
pub mod pathutil;
pub mod sorter;
pub mod templates;

pub use generator::{
    dump, AutoloadError, ClassmapCacheConfig, DumpOptions, DumpReport, PlatformCheckMode,
};
