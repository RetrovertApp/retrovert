//! Fixtures the crate's own suites are written against.
//!
//! They live inside the crate because the seams they stand in for — the
//! channel's source and its clock — are internal, and a suite that reached
//! them from outside would be reaching for something no consumer can hold.

pub mod fixture_channel;
pub mod fixture_server;
