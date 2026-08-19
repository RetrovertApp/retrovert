//! The persisted record of every check this client has run.
//!
//! It outlives the process, because the question it answers — "what has this
//! installation been doing?" — is asked days later, by a human holding a
//! support ticket and no logs. It lives with the caller's durable state and
//! never in a cache, which a janitor is free to clear.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sigstore_tuf::{FileStore, MetadataStore};

use super::error::{Error, Result};

/// Where the record is kept, relative to the state directory.
const RECORD_FILE: &str = "checks.json";

/// How many checks are kept. A client checking on an interval forever must not
/// grow a file without bound, and the oldest entries are the ones a support
/// question is least likely to be about.
pub(super) const MAX_ENTRIES: usize = 64;

/// What a check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Conclusion {
    /// The check did not run.
    Skipped,
    /// The channel names the generation already installed.
    UpToDate,
    /// The channel names a generation this client does not have.
    UpdateAvailable,
    /// The check failed; the record's error says how.
    Failed,
}

/// One check, as it reads back weeks later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// When the check ran, by the local clock — the only clock a skipped check
    /// has. Nothing verifies against it.
    pub at: Timestamp,
    /// What the check concluded.
    pub conclusion: Conclusion,
    /// How it failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A directory holding one consumer's check record.
#[derive(Debug, Clone)]
pub struct CheckLog {
    dir: PathBuf,
}

impl CheckLog {
    /// Keep the record under `dir`, which is created when first written to.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory the record lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every kept check, oldest first.
    pub fn entries(&self) -> Result<Vec<Record>> {
        let path = self.path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::record(path, e)),
        };
        serde_json::from_slice(&bytes).map_err(|e| Error::record(path, e))
    }

    /// The most recent check, if this client has ever run one.
    pub fn last(&self) -> Result<Option<Record>> {
        Ok(self.entries()?.pop())
    }

    /// Append `record`, dropping the oldest entries past [`MAX_ENTRIES`].
    ///
    /// A record that cannot be read back is replaced rather than raised: a
    /// logbook is an observation of the check, and must never be what stops one
    /// from being recorded again.
    pub fn append(&self, record: &Record) -> Result<()> {
        let mut entries = self.entries().unwrap_or_default();
        entries.push(record.clone());
        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }

        let bytes = serde_json::to_vec(&entries).map_err(|e| Error::record(self.path(), e))?;
        // Through the blob writer the trust state uses, so a crash mid-write
        // cannot leave a truncated record behind.
        FileStore::new(&self.dir)
            .store(RECORD_FILE, &bytes)
            .map_err(|e| Error::record(self.path(), e))
    }

    fn path(&self) -> PathBuf {
        self.dir.join(RECORD_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(at: &str, conclusion: Conclusion) -> Record {
        Record {
            at: at.parse().unwrap(),
            conclusion,
            error: None,
        }
    }

    #[test]
    fn a_client_that_has_never_checked_has_an_empty_record() {
        let dir = tempfile::tempdir().unwrap();
        let log = CheckLog::new(dir.path().join("state"));

        assert_eq!(log.entries().unwrap(), Vec::new());
        assert_eq!(log.last().unwrap(), None);
    }

    #[test]
    fn checks_accumulate_oldest_first_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let log = CheckLog::new(dir.path());

        let first = record("2026-08-15T12:00:00Z", Conclusion::Skipped);
        let second = record("2026-08-15T13:00:00Z", Conclusion::UpdateAvailable);
        log.append(&first).unwrap();
        log.append(&second).unwrap();

        let reopened = CheckLog::new(dir.path());
        assert_eq!(reopened.entries().unwrap(), vec![first, second.clone()]);
        assert_eq!(reopened.last().unwrap(), Some(second));
    }

    #[test]
    fn a_failure_keeps_its_message() {
        let dir = tempfile::tempdir().unwrap();
        let log = CheckLog::new(dir.path());
        let failed = Record {
            error: Some("channel did not authenticate".to_string()),
            ..record("2026-08-15T12:00:00Z", Conclusion::Failed)
        };

        log.append(&failed).unwrap();
        assert_eq!(CheckLog::new(dir.path()).last().unwrap(), Some(failed));
    }

    #[test]
    fn the_record_keeps_the_newest_entries_and_drops_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let log = CheckLog::new(dir.path());
        let at = |minute: i64| Timestamp::from_second(minute * 60).unwrap();

        let overflow = 5;
        for minute in 0..MAX_ENTRIES + overflow {
            log.append(&Record {
                at: at(i64::try_from(minute).unwrap()),
                conclusion: Conclusion::UpToDate,
                error: None,
            })
            .unwrap();
        }

        let entries = log.entries().unwrap();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].at, at(i64::try_from(overflow).unwrap()));
        assert_eq!(
            entries[MAX_ENTRIES - 1].at,
            at(i64::try_from(MAX_ENTRIES + overflow - 1).unwrap())
        );
    }

    #[test]
    fn a_corrupt_record_is_raised_to_a_reader_and_replaced_by_the_next_check() {
        let dir = tempfile::tempdir().unwrap();
        let log = CheckLog::new(dir.path());
        std::fs::write(dir.path().join(RECORD_FILE), b"{ not json").unwrap();

        let err = log.entries().unwrap_err();
        assert!(matches!(err, Error::Record { .. }), "{err}");

        let next = record("2026-08-15T12:00:00Z", Conclusion::UpToDate);
        log.append(&next).unwrap();
        assert_eq!(log.entries().unwrap(), vec![next]);
    }
}
