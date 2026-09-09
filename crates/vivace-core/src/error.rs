use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid JSON in {context}: {source}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("cannot encode non-finite float ({0}) as JSON (PHP json_encode would fail too)")]
    NonFiniteFloat(f64),
}

pub type Result<T> = std::result::Result<T, Error>;
