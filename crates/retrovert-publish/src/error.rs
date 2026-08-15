//! Error type for the publish CLI.

use std::path::PathBuf;

/// Errors produced by publish-side commands.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A metadata or key operation failed.
    #[error(transparent)]
    Tuf(#[from] retrovert_tuf::Error),

    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// `init` was pointed at a directory that already has contents.
    #[error("{0} is not empty; pass --force to re-initialize it and replace its keys")]
    NotEmpty(PathBuf),

    /// A private-key directory already exists as a symlink, which would place
    /// the keys somewhere this tool cannot vouch for.
    #[error("{0} is a symlink; private keys must live in a real directory")]
    KeyDirIsSymlink(PathBuf),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
