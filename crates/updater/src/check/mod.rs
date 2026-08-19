//! Asking a channel what it offers, and deciding what would have to be
//! fetched — without fetching any of it.
//!
//! This is the first half of the two-phase update. It authenticates the
//! channel, keeps the artifacts that apply to this consumer, and prices them
//! against the download cache, so a caller on a metered connection decides
//! whether to go ahead before a single artifact byte moves.

mod error;
mod plan;
mod record;

use jiff::Timestamp;

pub use error::{Error, Result};
pub use plan::{Plan, applies_to};
pub use record::{CheckLog, Conclusion, MAX_ENTRIES, Record};

use crate::channel::{Attempt, Channel};
use crate::transport::Cache;

/// What one check concluded.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Nothing was verified and no trust state moved.
    Skipped(Skipped),
    /// The channel names the generation already installed.
    UpToDate {
        /// The generation both the channel and this client are on.
        generation_id: String,
    },
    /// The channel names a generation this client does not have.
    ///
    /// A plan covering no artifacts is still an update: the generation the
    /// consumer is on advances even when a release set carries nothing for its
    /// target, and applying it costs no bytes.
    UpdateAvailable(Plan),
}

/// Why a check did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Skipped {
    /// The network could not be asked what time it is — offline, or a host
    /// that served no `Date`. Verifying against the local clock is not an
    /// option, so nothing ran and the caller keeps the generation it has.
    NoNetworkTime,
}

/// Checks one channel on behalf of one consumer.
pub struct Checker {
    channel: Channel,
    target: Option<String>,
    cache: Cache,
    log: CheckLog,
}

impl std::fmt::Debug for Checker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checker")
            .field("target", &self.target)
            .field("log", &self.log.dir())
            .finish_non_exhaustive()
    }
}

impl Checker {
    /// Check `channel` for a consumer whose target is `target`, answering
    /// "already cached" from `cache` and recording every attempt in `log`.
    ///
    /// `target` is opaque: it means whatever the channel's publisher means by
    /// it, and `None` takes only the artifacts that name no target at all.
    #[must_use]
    pub fn new(channel: Channel, target: Option<String>, cache: Cache, log: CheckLog) -> Self {
        Self {
            channel,
            target,
            cache,
            log,
        }
    }

    /// The record every check appends to.
    #[must_use]
    pub fn log(&self) -> &CheckLog {
        &self.log
    }

    /// Ask the channel what it offers, given that `installed` is the generation
    /// this consumer already has.
    ///
    /// Reads the channel's metadata and its manifest and nothing else: what a
    /// plan says would be downloaded is decided from that manifest and the
    /// cache alone.
    pub fn check(&mut self, installed: Option<&str>) -> Result<Outcome> {
        let outcome = self.resolve(installed);
        let appended = self.log.append(&record_of(&outcome));
        match outcome {
            Ok(outcome) => appended.map(|()| outcome),
            // A check that failed is the more useful of the two failures, and
            // the one the caller asked a question about.
            Err(e) => Err(e),
        }
    }

    fn resolve(&mut self, installed: Option<&str>) -> Result<Outcome> {
        let authenticated = match self.channel.authenticate()? {
            Attempt::Skipped => return Ok(Outcome::Skipped(Skipped::NoNetworkTime)),
            Attempt::Authenticated(authenticated) => authenticated,
        };

        if installed == Some(authenticated.generation_id.as_str()) {
            return Ok(Outcome::UpToDate {
                generation_id: authenticated.generation_id,
            });
        }
        Ok(Outcome::UpdateAvailable(Plan::select(
            authenticated.generation_id,
            &authenticated.manifest,
            self.target.as_deref(),
            &self.cache,
        )))
    }
}

fn record_of(outcome: &Result<Outcome>) -> Record {
    let (conclusion, error) = match outcome {
        Ok(Outcome::Skipped(_)) => (Conclusion::Skipped, None),
        Ok(Outcome::UpToDate { .. }) => (Conclusion::UpToDate, None),
        Ok(Outcome::UpdateAvailable(_)) => (Conclusion::UpdateAvailable, None),
        Err(e) => (Conclusion::Failed, Some(e.to_string())),
    };
    Record {
        at: Timestamp::now(),
        conclusion,
        error,
    }
}
