//! What one poll of an updater reports.

use crate::check::{Plan, Record};

/// What an updater is doing right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Phase {
    /// Nothing is in flight.
    #[default]
    Idle,
    /// Asking the channel what it offers.
    Checking,
    /// Acquiring a plan and publishing it as a generation.
    Applying,
}

/// An updater's state as of one poll, owned by the caller that asked.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    /// What the updater is doing.
    pub phase: Phase,
    /// How far the plan being applied has got, in `0.0..=1.0`, measured
    /// against what it weighs. `0.0` when nothing is being applied.
    pub progress: f32,
    /// The last check this client recorded, which outlives the process.
    pub last_check: Option<Record>,
    /// A plan the channel offered that nothing has applied yet.
    pub available: Option<Plan>,
    /// Why the last check or apply did not get where it was going. Never
    /// fatal: the installed generation is untouched either way.
    pub last_error: Option<String>,
}
