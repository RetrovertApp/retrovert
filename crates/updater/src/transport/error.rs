//! Errors the transport reports to its caller.

use std::path::PathBuf;

/// Errors produced while setting up or performing a transfer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The request never produced a response.
    #[error("{url}: {source}")]
    Request {
        /// The URL requested.
        url: String,
        /// Boxed so `ureq`'s error type stays out of this crate's public API
        /// while still chaining through [`std::error::Error::source`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The server answered with a non-success status.
    #[error("{url}: server returned status {status}")]
    Status {
        /// The URL requested.
        url: String,
        /// The status code received.
        status: u16,
    },

    /// The response body was larger than the caller allowed.
    #[error("{url}: response is larger than the {limit} byte limit")]
    TooLarge {
        /// The URL requested.
        url: String,
        /// The limit that was exceeded, in bytes.
        limit: usize,
    },

    /// The free-space gate refused the transfer.
    #[error("{path}: needs {required} bytes, {available} available")]
    DiskSpace {
        /// Where the transfer would have been written.
        path: PathBuf,
        /// Bytes the transfer needs, headroom included.
        required: u64,
        /// Bytes the filesystem reports free.
        available: u64,
    },

    /// A digest was not 64 hex characters.
    #[error("not a sha-256 digest: {0}")]
    Digest(String),

    /// A transfer was asked for with an empty URL.
    #[error("empty url")]
    EmptyUrl,
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn request(
        url: &str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Request {
            url: url.to_string(),
            source: Box::new(source),
        }
    }
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, Error>;
