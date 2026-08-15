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

    /// An existing metadata file in the channel could not be parsed.
    #[error("{path}: {source}")]
    Metadata {
        /// The metadata file being read.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: serde_json::Error,
    },

    /// A parent role does not pin the metadata file publish must supersede.
    #[error("{path}: does not pin {name}")]
    MissingPin {
        /// The parent role's metadata file.
        path: PathBuf,
        /// The unversioned name of the missing pin.
        name: String,
    },

    /// A metadata version counter cannot be advanced. Version numbers this
    /// large never arise from real publishes; the metadata is corrupt.
    #[error("{path}: version {version} cannot be incremented")]
    VersionExhausted {
        /// The metadata file carrying the version.
        path: PathBuf,
        /// The version that could not be advanced.
        version: u64,
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
