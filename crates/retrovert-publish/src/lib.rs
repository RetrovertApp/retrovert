//! Publish-side operations for Retrovert release channels.
//!
//! The binary is a thin shell over this library so the same code paths the CLI
//! runs are directly testable.

pub mod error;
pub mod init;
pub mod keys;
pub mod publish;
pub mod workspace;

pub use error::{Error, Result};
pub use init::{InitReport, KeySet, init};
pub use keys::KeyStore;
pub use publish::{PublishReport, publish};
pub use workspace::Workspace;
