//! The TUF metadata model shared by the Retrovert publisher and updater.
//!
//! A Retrovert channel is a self-contained TUF repository with consistent
//! snapshots, Ed25519 keys for every role, and one key per role at threshold 1.
//! This crate owns the wire types, the canonical-JSON signing rule, the expiry
//! policy, and the channel's on-disk naming; deciding *what* to publish belongs
//! to the publisher.

pub mod canonical;
pub mod error;
pub mod key;
pub mod metadata;
pub mod policy;
pub mod repository;

pub use error::{Error, Result};
pub use key::{KeyPair, KeyVal, PublicKey};
pub use metadata::{
    MetaFile, Role, RoleKeys, RoleName, Root, Signature, Signed, Snapshot, TargetFile, Targets,
    Timestamp,
};
pub use repository::{Channel, published_names};
