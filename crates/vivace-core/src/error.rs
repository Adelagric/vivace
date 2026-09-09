use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("I/O error at {path}: {source}")]
    Io {
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

    #[error("invalid zip archive for {dest}: {source}")]
    Zip {
        dest: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },

    #[error("hostile archive refused for {dest}: {reason}")]
    HostileArchive { dest: PathBuf, reason: String },

    #[error("HTTP failure for {url}: {message}")]
    Http { url: String, message: String },

    #[error("dist checksum mismatch for {name}: expected sha1 {expected}, got {actual}")]
    ShasumMismatch {
        name: String,
        expected: String,
        actual: String,
    },

    #[error("cannot encode non-finite float ({0}) as JSON (PHP json_encode would fail too)")]
    NonFiniteFloat(f64),
}

impl Error {
    pub fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
        move |source| Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub fn zip(dest: &Path) -> impl FnOnce(zip::result::ZipError) -> Error + '_ {
        move |source| Error::Zip {
            dest: dest.to_path_buf(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
