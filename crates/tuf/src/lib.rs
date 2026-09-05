//! The TUF metadata model shared by the Retrovert publisher and updater.
//!
//! A Retrovert channel is a self-contained TUF repository with consistent
//! snapshots and Ed25519 keys for every role. The online roles carry one key
//! each at threshold 1; `root` is authorized per channel and may hold several
//! keys at a threshold above one, which is what lets a root be held by more
//! than one person.
//! This crate owns the wire types, the canonical-JSON signing rule, the expiry
//! policy, and the channel's on-disk naming; deciding *what* to publish belongs
//! to the publisher.

pub mod canonical;
pub mod error;
pub mod key;
pub mod manifest;
pub mod metadata;
pub mod policy;
pub mod repository;

pub use error::{Error, Result};
pub use key::{KeyPair, KeyVal, PublicKey};
pub use manifest::{Artifact, Manifest};
pub use metadata::{
    MetaFile, Role, RoleAuthorization, RoleKeys, RoleName, Root, Signature, Signed, Snapshot,
    TargetFile, Targets, Timestamp,
};
pub use repository::{Channel, published_names, target_published_name, versioned_name};
