//! What a consumer hands the updater at construction.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use crate::queue::WorkerHook;

/// Where one channel's metadata and its artifacts are served from.
///
/// Two base URLs rather than one: the signed metadata is continuously replaced
/// in place, while a release set's artifacts sit with the release that
/// published them and never move again.
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// The signed metadata and the manifest it carries as a target. HTTPS
    /// only, because the verification time is this host's `Date` header.
    pub metadata_base_url: String,
    /// What the manifest's artifact paths resolve against. HTTPS only:
    /// artifacts are digest-verified, but a plaintext fetch still tells every
    /// observer what this client runs.
    pub artifact_base_url: String,
}

/// How the updater places the threads it owns.
#[derive(Clone)]
pub struct WorkerConfig {
    /// Workers draining transfers, and so transfers in flight at once.
    pub count: NonZeroUsize,
    /// Run at the top of every worker thread, given that worker's index. The
    /// consumer uses it for affinity, scheduling priority, or a debugger name.
    pub on_worker_start: Option<WorkerHook>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            count: NonZeroUsize::MIN,
            on_worker_start: None,
        }
    }
}

impl std::fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerConfig")
            .field("count", &self.count)
            .field("on_worker_start", &self.on_worker_start.is_some())
            .finish()
    }
}

/// Everything one updater is configured with.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    /// Generations are published beneath `install_root/generations/`, leaving
    /// the root itself to whatever else the consumer keeps there.
    pub install_root: PathBuf,
    /// Digest-keyed download entries and their resume sidecars. Evictable: a
    /// janitor that clears it costs the next update its bytes and nothing
    /// else.
    pub cache_dir: PathBuf,
    /// Trust state and the check record. Must survive upgrades and power loss,
    /// and must not sit inside any cache.
    pub trust_state_dir: PathBuf,
    /// The channel to follow.
    pub channel: ChannelConfig,
    /// The TUF root metadata this binary was built with, which is the only
    /// thing a channel's chain is ever accepted against.
    pub embedded_root: Vec<u8>,
    /// The consumer's opaque target selector. `None` takes only the artifacts
    /// that name no target at all.
    pub target: Option<String>,
    /// The shortest time between two checks.
    pub check_interval: Duration,
    /// Whether a successful check flows straight into an apply.
    pub auto_apply: bool,
    /// Worker count and placement.
    pub workers: WorkerConfig,
}
