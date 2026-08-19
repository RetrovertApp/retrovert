//! Resolves a signed update channel into a verified installed generation.
//!
//! The channel authenticates a content-agnostic manifest, and the artifacts it
//! lists are opaque here: this crate acquires them and publishes the generation
//! beneath a caller-owned installation root. What the artifacts are — playback
//! plugins, Replay databases, anything else a manifest names — and when a
//! generation goes live are the consumer's, decided after acquisition.

pub mod channel;
pub mod policy;
pub mod queue;
pub mod transport;
