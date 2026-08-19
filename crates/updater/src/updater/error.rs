//! Errors the facade reports to its caller.

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
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, Error>;
