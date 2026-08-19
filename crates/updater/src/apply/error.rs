//! Errors an apply reports to its caller.
//!
//! None of them is fatal to the host: an update that fails leaves the
//! installed generation exactly where it was, and the caller keeps running on
//! it.

use std::path::PathBuf;

use crate::queue::Failure;

/// Errors produced while applying a plan or maintaining the install root.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An artifact could not be acquired, or the bytes that arrived were not
    /// the ones the manifest named. The download has been quarantined.
    #[error("artifact {name:?} was not acquired: {source}")]
    Artifact {
        /// The artifact the manifest named.
        name: String,
        /// What the transfer queue refused it for.
        #[source]
        source: Failure,
    },

    /// The transfer was cancelled or removed out from under the apply.
    #[error("artifact {name:?} was cancelled before it landed")]
    Cancelled {
        /// The artifact the manifest named.
        name: String,
    },

    /// A manifest digest did not parse, so nothing could be fetched under it.
    #[error("artifact {name:?} names {digest:?}, which is not a SHA-256 digest")]
    Digest {
        /// The artifact the manifest named.
        name: String,
        /// The digest as the manifest spelled it.
        digest: String,
    },

    /// Two artifacts of one plan want the same place in the generation.
    #[error("artifacts {first:?} and {second:?} both publish to {path:?}")]
    Collision {
        /// The artifact that claimed the path.
        first: String,
        /// The artifact that arrived at the same path.
        second: String,
        /// The path both named, relative to the generation directory.
        path: String,
    },

    /// The install root could not be read or written.
    #[error("{path}: {source}")]
    Install {
        /// The file or directory involved.
        path: PathBuf,
        /// Boxed so the store's error type stays out of this crate's public
        /// API while still chaining through [`std::error::Error::source`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl Error {
    pub(super) fn install(
        path: impl Into<PathBuf>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Install {
            path: path.into(),
            source: Box::new(source),
        }
    }
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, Error>;
