//! vivace-core — manifestes, plateforme, fetch et installation.

pub mod constraint;
pub mod content_hash;
pub mod error;
pub mod lock;
pub mod phpjson;
pub mod platform;
pub mod scope;
pub mod version;

pub use error::{Error, Result};
