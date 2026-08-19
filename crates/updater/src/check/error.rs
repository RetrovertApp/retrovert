//! Errors a check reports to its caller.

use std::path::PathBuf;

/// Errors produced while checking a channel.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The channel did not authenticate, so there is nothing to plan against.
    #[error(transparent)]
    Channel(#[from] crate::channel::Error),

    /// The check record could not be read or written.
    #[error("{path}: {source}")]
    Record {
        /// The record file involved.
        path: PathBuf,
        /// Boxed so the store and serialization error types stay out of this
        /// crate's public API while still chaining through
        /// [`std::error::Error::source`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl Error {
    pub(super) fn record(
        path: impl Into<PathBuf>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Record {
            path: path.into(),
            source: Box::new(source),
        }
    }
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, Error>;
