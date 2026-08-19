//! Errors an authentication attempt reports to its caller.

use std::path::PathBuf;

/// Errors produced while authenticating a channel.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The channel's metadata did not authenticate against the embedded root.
    #[error("channel did not authenticate: {0}")]
    Tuf(#[from] sigstore_tuf::Error),

    /// The authenticated bytes were not a manifest this build can read.
    #[error(transparent)]
    Manifest(#[from] retrovert_tuf::Error),

    /// The channel offered a chain older than one already trusted.
    #[error("channel offers {role} version {offered}, below the trusted floor of {floor}")]
    Rollback {
        /// The role whose version went backwards.
        role: &'static str,
        /// The lowest version trust state accepts.
        floor: u64,
        /// The version the channel served.
        offered: u64,
    },

    /// The channel dated the check earlier than a time this client has
    /// already verified against — the freeze a lied-backwards `Date` header
    /// would otherwise enable.
    #[error(
        "network time went backwards: offered {offered}, \
         already verified against {floor} (unix seconds)"
    )]
    TimeRollback {
        /// The newest verification time trust state has recorded.
        floor: i64,
        /// The earlier time the channel offered.
        offered: i64,
    },

    /// Trust state could not be read or written.
    #[error("{path}: {source}")]
    Trust {
        /// The trust-state file involved.
        path: PathBuf,
        /// Boxed so the store and serialization error types stay out of this
        /// crate's public API while still chaining through
        /// [`std::error::Error::source`].
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A refresh returned success without leaving a complete chain behind.
    #[error("refresh left no trusted {0}")]
    IncompleteRefresh(&'static str),

    /// A `Date` header was asked for from a host that does not authenticate it.
    #[error("{0} is not an https url, so its Date header carries no authentication")]
    Insecure(String),
}

impl Error {
    pub(super) fn trust(
        path: impl Into<PathBuf>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Trust {
            path: path.into(),
            source: Box::new(source),
        }
    }
}

/// Result alias for this module.
pub type Result<T> = std::result::Result<T, Error>;
