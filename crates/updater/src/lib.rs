//! Resolves a signed update channel into a verified installed generation.
//!
//! The channel authenticates a content-agnostic manifest, and the artifacts it
//! lists are opaque here: this crate acquires them and publishes the generation
//! beneath a caller-owned installation root. What the artifacts are — playback
//! plugins, Replay databases, anything else a manifest names — and when a
//! generation goes live are the consumer's, decided after acquisition.
//!
//! [`Updater`] is the whole of the surface. The channel's source, its clock,
//! and the transfer queue are seams this crate swaps in its own tests and a
//! consumer never holds.

mod apply;
mod channel;
mod check;
mod policy;
mod queue;
mod transport;
mod updater;

#[cfg(test)]
mod testing;

pub use retrovert_tuf::manifest::Artifact;

pub use apply::{Error as ApplyError, Generation, Installed};
pub use channel::Error as ChannelError;
pub use check::{Conclusion, Error as CheckError, Plan, Record};
pub use queue::{Failure, Priority};
pub use transport::Failure as TransferFailure;
pub use updater::{
    ChannelConfig, Error, Phase, Result, StatusSnapshot, Updater, UpdaterConfig, WorkerConfig,
    WorkerHook,
};
