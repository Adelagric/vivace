//! vivace-core — manifestes, plateforme, fetch et installation.

pub mod binproxy;
pub mod clone;
pub mod constraint;
pub mod content_hash;
pub mod error;
pub mod extract;
pub mod fetch;
pub mod installer;
pub mod installers;
pub mod layout;
pub mod lock;
pub mod pathutil;
pub mod phpjson;
pub mod platform;
pub mod root_version;
pub mod runtime_stub;
pub mod scope;
pub mod state;
pub mod store;
pub mod version;

pub use error::{Error, Result};
