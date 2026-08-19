//! Errors the facade reports to its caller.

use std::path::PathBuf;

/// Errors produced constructing or querying an updater.
///
/// Nothing a check or an apply fails with reaches here — those run on the
/// updater's own thread and are reported through [`super::StatusSnapshot`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The channel could not be opened.
    #[error(transparent)]
    Channel(#[from] crate::channel::Error),

    /// The check record could not be read.
    #[error(transparent)]
    Check(#[from] crate::check::Error),

    /// The install root could not be read or written.
    #[error(transparent)]
    Apply(#[from] crate::apply::Error),

    /// A base URL that must be https was not. Artifacts are digest-verified,
    /// but a plaintext fetch still tells every observer what this client runs
    /// and hands a middlebox free corruption retries.
    #[error("{0} is not an https url")]
    Insecure(String),

    /// The trust state sits inside the download cache. A cache is evictable
    /// by definition, and a floor that can be evicted is not a floor.
    #[error("trust state dir {trust} must not sit inside cache dir {cache}")]
    TrustStateInCache {
        /// The configured trust-state directory.
        trust: PathBuf,
        /// The configured cache directory.
        cache: PathBuf,
    },
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, Error>;
