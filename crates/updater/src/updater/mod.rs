//! The one type a consumer holds, closing over everything the crate does.
//!
//! Configure it with an install root, the two directories it keeps state in, a
//! channel, and the root this binary trusts; then drive it from a frame loop.
//! Checking and applying run on a thread the updater owns, so neither
//! [`Updater::tick`] nor anything else here blocks the caller; what they are
//! doing is read back through [`Updater::poll`].
//!
//! The channel's swappable source, its clock, and the transfer queue stay
//! behind this type. A consumer configures them and never holds one.

mod config;
mod error;
mod schedule;
mod status;

#[cfg(test)]
mod driving;

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::apply::{Applier, Completions, Generation, InstallRoot};
use crate::channel::{Channel, TrustStore};
use crate::check::{CheckLog, Checker, Outcome, Plan, Record};
use crate::policy;
use crate::queue::{Config as QueueConfig, Priority, TransferQueue};
use crate::transport::{ArtifactDigest, Cache, Transport};

pub use crate::queue::WorkerHook;
pub use config::{ChannelConfig, UpdaterConfig, WorkerConfig};
pub use error::{Error, Result};
pub use status::{Phase, StatusSnapshot};

use schedule::Schedule;

/// Generations are published one level down, leaving the install root itself
/// to whatever else the consumer keeps there.
const GENERATIONS: &str = "generations";

/// Resolves a channel into installed generations, on its own threads.
#[derive(Debug)]
pub struct Updater {
    inner: Arc<Inner>,
    /// The check-or-apply thread, at most one at a time.
    job: Mutex<Option<JoinHandle<()>>>,
}

impl Updater {
    /// An updater following the channel `config` names.
    ///
    /// Nothing is fetched here: the first check runs on the first
    /// [`Updater::tick`] or [`Updater::check_now`].
    pub fn new(config: UpdaterConfig) -> Result<Self> {
        // Its own transport: the queue owns the one artifacts stream through,
        // and metadata is small, uncached, and unresumable. They share only the
        // cache directory, which is a path rather than state.
        let metadata = Arc::new(Transport::new(&config.cache_dir));
        let channel = Channel::https(
            &metadata,
            &config.channel.metadata_base_url,
            config.embedded_root.clone(),
            TrustStore::new(&config.trust_state_dir),
        )?;
        Ok(Self::over(config, channel))
    }

    fn over(config: UpdaterConfig, channel: Channel) -> Self {
        let queue = Arc::new(TransferQueue::with_config(
            Transport::new(&config.cache_dir),
            QueueConfig {
                workers: config.workers.count.get(),
                on_worker_start: config.workers.on_worker_start,
            },
        ));
        let applier = Applier::new(
            Arc::clone(&queue),
            InstallRoot::new(config.install_root.join(GENERATIONS)),
            &config.channel.artifact_base_url,
        );
        let checker = Checker::new(
            channel,
            config.target,
            Cache::new(&config.cache_dir),
            CheckLog::new(&config.trust_state_dir),
        );

        let status = Status {
            last_check: checker.log().last().ok().flatten(),
            ..Status::default()
        };
        Self {
            inner: Arc::new(Inner {
                completions: applier.completions(),
                checker: Mutex::new(checker),
                applier,
                queue,
                cache: Cache::new(&config.cache_dir),
                auto_apply: config.auto_apply,
                schedule: Mutex::new(Schedule::new(config.check_interval)),
                status: Mutex::new(status),
            }),
            job: Mutex::new(None),
        }
    }

    /// Check the channel if the interval has passed, and report whether one
    /// started.
    ///
    /// Meant for a frame loop: calling it every frame costs a clock read and a
    /// comparison until a check is due.
    pub fn tick(&self) -> bool {
        let now = now_ms();
        if !lock(&self.inner.schedule).due(now) {
            return false;
        }
        self.begin_check(now, Priority::Background)
    }

    /// Check the channel now, whatever the interval says, and report whether
    /// one started.
    ///
    /// For a user asking and for a consumer's first-run escalation. Returns
    /// `false` while a check or an apply is already running.
    pub fn check_now(&self, priority: Priority) -> bool {
        self.begin_check(now_ms(), priority)
    }

    /// Acquire `plan` and publish it as a generation, and report whether the
    /// apply started.
    ///
    /// Returns `false` while a check or an apply is already running. The
    /// generation is offered through [`Updater::next_completed`] once it has
    /// been published in full.
    pub fn apply(&self, plan: &Plan, priority: Priority) -> bool {
        self.start(Job::Apply {
            plan: plan.clone(),
            priority,
        })
    }

    /// Pause every transfer in flight, keeping its partial file resumable.
    pub fn pause(&self) {
        self.inner.queue.pause_all();
    }

    /// Resume every transfer this updater paused.
    pub fn resume(&self) {
        self.inner.queue.resume_all();
    }

    /// Cancel every transfer in flight, failing the apply that queued them.
    ///
    /// The installed generation is not touched: a cancelled apply leaves the
    /// consumer on whatever it was already running.
    pub fn cancel(&self) {
        self.inner.queue.cancel_all();
    }

    /// Where the updater stands, as an owned snapshot.
    #[must_use]
    pub fn poll(&self) -> StatusSnapshot {
        let status = self.inner.status();
        StatusSnapshot {
            phase: status.phase,
            progress: self.inner.progress(&status),
            last_check: status.last_check.clone(),
            available: status.available.clone(),
            last_error: status.last_error.clone(),
        }
    }

    /// The next generation this updater has published and not yet reported.
    ///
    /// Once only per generation, so a consumer polling every frame acts on one
    /// exactly once. The record is this process's: a restart offers the
    /// generation it then applies again, which is what a consumer that has
    /// just started needs to hear.
    #[must_use]
    pub fn next_completed(&self) -> Option<Generation> {
        self.inner.completions.next_completed()
    }

    /// Every generation the install root holds, in no particular order.
    pub fn generations(&self) -> Result<Vec<Generation>> {
        Ok(self.inner.applier.install_root().generations()?)
    }

    /// Remove every generation but the ones `live` names, returning what was
    /// removed.
    ///
    /// Which generations are live is the consumer's: this crate publishes them
    /// and never decides when one stops being used.
    pub fn retain(&self, live: &[String]) -> Result<Vec<String>> {
        Ok(self.inner.applier.install_root().retain(live)?)
    }

    /// The directory generations are published into.
    #[must_use]
    pub fn generations_dir(&self) -> &Path {
        self.inner.applier.install_root().root()
    }

    #[cfg(test)]
    fn run_panicking_job(&self) -> bool {
        self.start(Job::Panic)
    }

    fn begin_check(&self, now_ms: i64, priority: Priority) -> bool {
        if !self.start(Job::Check { priority }) {
            return false;
        }
        lock(&self.inner.schedule).attempted(now_ms);
        true
    }

    /// Put `job` on the updater's thread, unless one is already running.
    fn start(&self, job: Job) -> bool {
        let mut handle = lock(&self.job);
        {
            let mut status = self.inner.status();
            if status.phase != Phase::Idle {
                return false;
            }
            status.phase = job.phase();
        }
        // The previous job set Idle as its last act, so this joins a thread
        // that is already on its way out.
        if let Some(finished) = handle.take() {
            let _ = finished.join();
        }

        let inner = Arc::clone(&self.inner);
        match thread::Builder::new()
            .name("retrovert-updater".to_string())
            .spawn(move || inner.run(job))
        {
            Ok(spawned) => {
                *handle = Some(spawned);
                true
            }
            Err(e) => {
                let mut status = self.inner.status();
                status.phase = Phase::Idle;
                status.last_error = Some(e.to_string());
                false
            }
        }
    }
}

impl Drop for Updater {
    /// Cancel what is in flight and wait the updater's thread out.
    ///
    /// A check already talking to the host is not interruptible; it is bounded
    /// by the transport's own timeout.
    fn drop(&mut self) {
        self.inner.queue.cancel_all();
        if let Some(handle) = lock(&self.job).take() {
            let _ = handle.join();
        }
    }
}

/// What the updater's thread was asked to do.
enum Job {
    Check {
        priority: Priority,
    },
    Apply {
        plan: Plan,
        priority: Priority,
    },
    /// Stands in for a job that unwinds, so the suite can prove the thread's
    /// panic guard.
    #[cfg(test)]
    Panic,
}

impl Job {
    fn phase(&self) -> Phase {
        match self {
            Self::Check { .. } => Phase::Checking,
            Self::Apply { .. } => Phase::Applying,
            #[cfg(test)]
            Self::Panic => Phase::Checking,
        }
    }
}

/// Everything the caller and the updater's thread share.
struct Inner {
    checker: Mutex<Checker>,
    applier: Applier,
    completions: Completions,
    queue: Arc<TransferQueue>,
    cache: Cache,
    auto_apply: bool,
    schedule: Mutex<Schedule>,
    status: Mutex<Status>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("applier", &self.applier)
            .field("auto_apply", &self.auto_apply)
            .finish_non_exhaustive()
    }
}

/// What a poll reads and the updater's thread writes.
#[derive(Debug, Default)]
struct Status {
    phase: Phase,
    /// The plan being acquired, and so what progress is measured against.
    applying: Option<Plan>,
    available: Option<Plan>,
    last_check: Option<Record>,
    last_error: Option<String>,
    /// The generation this updater has published in this process, which is
    /// what a check is answered against. Absent after a restart, so the first
    /// check offers a plan the apply then resolves without fetching anything.
    installed: Option<String>,
}

impl Inner {
    /// Run one job and settle the phase, whether or not the job unwinds.
    ///
    /// A panic that escaped here would leave the phase reading Checking or
    /// Applying with no thread behind it, and every later tick would refuse to
    /// start against a job that is never coming back.
    fn run(&self, job: Job) {
        let ran = panic::catch_unwind(AssertUnwindSafe(|| match job {
            Job::Check { priority } => self.run_check(priority),
            Job::Apply { plan, priority } => self.run_apply(&plan, priority),
            #[cfg(test)]
            Job::Panic => panic!("a job that panics"),
        }));

        let mut status = self.status();
        if ran.is_err() {
            status.applying = None;
            status.last_error = Some("the check or apply panicked".to_string());
        }
        status.phase = Phase::Idle;
    }

    fn run_check(&self, priority: Priority) {
        let installed = self.status().installed.clone();
        let outcome = lock(&self.checker).check(installed.as_deref());
        let last_check = lock(&self.checker).log().last().ok().flatten();

        {
            let mut status = self.status();
            status.last_check = last_check;
            match &outcome {
                Ok(Outcome::UpdateAvailable(plan)) => {
                    status.available = Some(plan.clone());
                    status.last_error = None;
                }
                Ok(Outcome::UpToDate { generation_id }) => {
                    status.available = None;
                    status.last_error = None;
                    status.installed = Some(generation_id.clone());
                }
                // A skip is not an error: the record carries it, and whatever
                // last went wrong is still the last thing that did.
                Ok(Outcome::Skipped(_)) => {}
                Err(e) => status.last_error = Some(e.to_string()),
            }
        }

        // A skipped check counts against the schedule as a failure: it means
        // the network could not be asked the time, and backing off is the whole
        // point of a backoff while offline.
        let mut schedule = lock(&self.schedule);
        if matches!(
            outcome,
            Ok(Outcome::UpToDate { .. } | Outcome::UpdateAvailable(_))
        ) {
            schedule.succeeded();
        } else {
            schedule.failed();
        }
        drop(schedule);

        if self.auto_apply {
            if let Ok(Outcome::UpdateAvailable(plan)) = &outcome {
                self.run_apply(plan, priority);
            }
        }
    }

    fn run_apply(&self, plan: &Plan, priority: Priority) {
        {
            let mut status = self.status();
            status.phase = Phase::Applying;
            status.applying = Some(plan.clone());
        }
        let applied = self.applier.apply(plan, priority);

        let mut status = self.status();
        status.applying = None;
        match applied {
            Ok(generation) => {
                if status
                    .available
                    .as_ref()
                    .is_some_and(|offered| offered.generation_id == plan.generation_id)
                {
                    status.available = None;
                }
                status.installed = Some(generation.id().to_string());
                status.last_error = None;
            }
            Err(e) => status.last_error = Some(e.to_string()),
        }
    }

    /// How much of the plan in flight is already on disk, as a fraction of
    /// what it weighs.
    ///
    /// Read from the cache rather than from the queue: every artifact an apply
    /// acquires lands there under its own digest, so the bytes present answer
    /// the question without the apply having to report on itself.
    fn progress(&self, status: &Status) -> f32 {
        let Some(plan) = status.applying.as_ref() else {
            return 0.0;
        };
        let mut acquired = 0u64;
        for artifact in &plan.artifacts {
            acquired =
                acquired.saturating_add(self.acquired_bytes(&artifact.sha256, artifact.size));
        }
        policy::progress(as_i64(acquired), as_i64(plan.total_bytes))
    }

    fn acquired_bytes(&self, sha256: &str, size: u64) -> u64 {
        let Ok(digest) = ArtifactDigest::from_hex(sha256) else {
            return 0;
        };
        if self.cache.is_complete(&digest) {
            return size;
        }
        // A partial entry can hold more than the manifest named — the transfer
        // is refused for it later — so it never counts for more than the whole.
        std::fs::metadata(self.cache.path_for(&digest)).map_or(0, |entry| entry.len().min(size))
    }

    fn status(&self) -> MutexGuard<'_, Status> {
        lock(&self.status)
    }
}

/// A prior panic left this state usable, so recover rather than propagate.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn as_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}
