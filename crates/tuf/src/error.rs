//! Error type shared by the TUF model.

use std::path::PathBuf;

/// Errors produced while building, signing, or writing TUF metadata.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A key could not be encoded, decoded, or parsed.
    #[error("{context}")]
    Key {
        /// What was being attempted.
        context: &'static str,
        /// Boxed so the pre-1.0 `pkcs8` error types stay out of this crate's
        /// public API while still chaining through [`std::error::Error::source`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A declared Ed25519 public key was not 32 bytes.
    #[error("ed25519 public key must be 32 bytes, got {0}")]
    PublicKeyLength(usize),

    /// The same key was passed twice when signing; TUF forbids duplicate key
    /// IDs in a signatures array.
    #[error("duplicate signing key id {0}")]
    DuplicateKeyId(String),

    /// The operating system random source was unavailable.
    #[error("could not read random bytes: {0}")]
    Random(getrandom::Error),

    /// Date arithmetic on an expiry timestamp failed.
    #[error("expiry arithmetic failed: {0}")]
    Expiry(#[from] jiff::Error),

    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn key(
        context: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Key {
            context,
            source: Box::new(source),
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
