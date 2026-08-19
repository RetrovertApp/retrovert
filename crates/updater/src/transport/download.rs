//! One transfer: resume from a `.meta` sidecar, pause, cancel, and progress.
//!
//! A [`Download`] is `Sync`: [`Download::read_chunk`] mutates transfer state
//! behind a `Mutex`, while progress, pause and cancel ride lock-free atomics
//! and never block behind a network read.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use ureq::Agent;

use super::error::{Error, Result};
use super::meta::{self, Meta};
use super::{Agents, cache, header, url_size};

/// Only 206 makes a resumed body splice onto the partial file.
const PARTIAL_CONTENT: u16 = 206;
/// `progress_total` when the server did not say how long the body is.
const UNKNOWN_TOTAL: u64 = u64::MAX;

/// Where a transfer stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    /// Set up, no request issued yet.
    Pending = 0,
    /// Reading the response body.
    Downloading = 1,
    /// Stopped at the caller's request, resumable.
    Paused = 2,
    /// Every byte is on disk.
    Complete = 3,
    /// Stopped at the caller's request, partial bytes discarded.
    Cancelled = 4,
    /// Stopped on an error; [`Snapshot::failure`] says which.
    Failed = 5,
}

impl Status {
    fn from_repr(value: u8) -> Self {
        match value {
            0 => Self::Pending,
            1 => Self::Downloading,
            2 => Self::Paused,
            3 => Self::Complete,
            4 => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}

/// Why a transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Failure {
    /// The server answered with a 4xx or 5xx status.
    Server = 1,
    /// A resume was answered with a full body instead of a 206.
    ResumeRestart = 2,
    /// The server sent an HTML page where a binary artifact was expected.
    HtmlPage = 3,
    /// The connection failed, or a read or write did.
    Transfer = 4,
}

impl Failure {
    fn from_repr(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Server),
            2 => Some(Self::ResumeRestart),
            3 => Some(Self::HtmlPage),
            4 => Some(Self::Transfer),
            _ => None,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Server => "server returned an error",
            Self::ResumeRestart => "resume failed, restarting",
            Self::HtmlPage => "server returned an error page instead of the file",
            Self::Transfer => "transfer failed",
        };
        f.write_str(text)
    }
}

/// A transfer's state at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// Bytes on disk, resumed bytes included.
    pub downloaded: u64,
    /// The completed length, when the server reported one.
    pub total: Option<u64>,
    /// Where the transfer stands.
    pub status: Status,
    /// Set once `status` is [`Status::Failed`].
    pub failure: Option<Failure>,
    /// Whether retrying the same URL is pointless.
    pub no_retry: bool,
}

/// Outcome of one [`Download::read_chunk`] poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chunk {
    /// This many bytes were written into the caller's buffer.
    Read(usize),
    /// No bytes this poll: the transfer is complete or paused.
    Idle,
    /// The transfer has failed or been cancelled.
    Failed,
}

/// Mutable transfer state, touched only by `read_chunk` behind the `Mutex`.
struct Transfer {
    url: String,
    path: PathBuf,
    file_size: Option<u64>,
    bytes_written: u64,
    resume_offset: u64,
    etag: String,
    started: bool,
    is_cached: bool,
    reader: Option<Box<dyn Read + Send>>,
    file: Option<File>,
    cached_file: Option<File>,
}

/// A single transfer, driven by repeated [`Download::read_chunk`] calls.
pub struct Download {
    agent: Agent,
    progress_bytes: AtomicU64,
    progress_total: AtomicU64,
    status: AtomicU8,
    failure: AtomicU8,
    no_retry: AtomicBool,
    cancel: AtomicBool,
    pause: AtomicBool,
    xfer: Mutex<Transfer>,
}

/// What a partial file on disk allows a transfer to skip.
struct Resume {
    offset: u64,
    file_size: u64,
    etag: String,
}

/// How a transfer starts, decided before the [`Download`] exists.
enum Setup {
    Cached { file: File, size: u64 },
    Resume { file: File, resume: Resume },
    Fresh { file: File, file_size: Option<u64> },
}

impl Download {
    pub(super) fn start(
        agents: &Agents,
        path: PathBuf,
        url: &str,
        allow_resume: bool,
    ) -> Result<Self> {
        if url.is_empty() {
            return Err(Error::EmptyUrl);
        }
        let setup = prepare(agents, &path, url, allow_resume)?;
        Ok(Self::from_setup(agents.streaming.clone(), path, url, setup))
    }

    fn from_setup(agent: Agent, path: PathBuf, url: &str, setup: Setup) -> Self {
        let mut xfer = Transfer {
            url: url.to_string(),
            path,
            file_size: None,
            bytes_written: 0,
            resume_offset: 0,
            etag: String::new(),
            started: false,
            is_cached: false,
            reader: None,
            file: None,
            cached_file: None,
        };
        let (status, downloaded, total) = match setup {
            Setup::Cached { file, size } => {
                xfer.cached_file = Some(file);
                xfer.file_size = Some(size);
                xfer.is_cached = true;
                xfer.started = true;
                (Status::Complete, size, Some(size))
            }
            Setup::Resume { file, resume } => {
                xfer.file = Some(file);
                xfer.resume_offset = resume.offset;
                xfer.file_size = Some(resume.file_size);
                xfer.etag = resume.etag;
                (Status::Pending, resume.offset, Some(resume.file_size))
            }
            Setup::Fresh { file, file_size } => {
                xfer.file = Some(file);
                xfer.file_size = file_size;
                (Status::Pending, 0, file_size)
            }
        };
        Self {
            agent,
            progress_bytes: AtomicU64::new(downloaded),
            progress_total: AtomicU64::new(total.unwrap_or(UNKNOWN_TOTAL)),
            status: AtomicU8::new(status as u8),
            failure: AtomicU8::new(0),
            no_retry: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            pause: AtomicBool::new(false),
            xfer: Mutex::new(xfer),
        }
    }

    /// Where the bytes are being written.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.lock().path.clone()
    }

    /// Where the transfer stands.
    #[must_use]
    pub fn status(&self) -> Status {
        Status::from_repr(self.status.load(Ordering::SeqCst))
    }

    /// The transfer's state at this instant.
    #[must_use]
    pub fn progress(&self) -> Snapshot {
        let total = self.progress_total.load(Ordering::SeqCst);
        Snapshot {
            downloaded: self.progress_bytes.load(Ordering::SeqCst),
            total: (total != UNKNOWN_TOTAL).then_some(total),
            status: self.status(),
            failure: Failure::from_repr(self.failure.load(Ordering::SeqCst)),
            no_retry: self.no_retry.load(Ordering::SeqCst),
        }
    }

    /// Stop at the next poll, keeping the partial file resumable.
    pub fn request_pause(&self) {
        self.pause.store(true, Ordering::SeqCst);
    }

    /// Stop at the next poll and discard the partial file.
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Advance the transfer by at most one read into `buf`.
    ///
    /// One call performs at most one network read, which is the caller's
    /// polling granularity. An empty `buf` is a caller error and reports
    /// [`Chunk::Failed`] without touching the transfer.
    pub fn read_chunk(&self, buf: &mut [u8]) -> Chunk {
        if buf.is_empty() {
            return Chunk::Failed;
        }
        let mut xfer = self.lock();

        if xfer.is_cached {
            let Some(file) = xfer.cached_file.as_mut() else {
                return Chunk::Idle;
            };
            return match file.read(buf) {
                Ok(0) | Err(_) => {
                    xfer.cached_file = None;
                    Chunk::Idle
                }
                Ok(n) => Chunk::Read(n),
            };
        }

        match self.status() {
            Status::Failed | Status::Cancelled => return Chunk::Failed,
            Status::Complete | Status::Paused => return Chunk::Idle,
            Status::Pending | Status::Downloading => {}
        }

        if self.cancel.load(Ordering::SeqCst) {
            discard(&mut xfer);
            self.status.store(Status::Cancelled as u8, Ordering::SeqCst);
            return Chunk::Failed;
        }
        if self.pause.load(Ordering::SeqCst) {
            if let Some(file) = xfer.file.as_mut() {
                let _ = file.flush();
            }
            save_meta(&xfer);
            self.status.store(Status::Paused as u8, Ordering::SeqCst);
            return Chunk::Idle;
        }

        if !xfer.started && !self.start_transfer(&mut xfer) {
            return Chunk::Failed;
        }

        let read = match xfer.reader.as_mut() {
            Some(reader) => reader.read(buf),
            None => return Chunk::Failed,
        };
        match read {
            Ok(0) => {
                self.finalize_complete(&mut xfer);
                Chunk::Idle
            }
            Ok(n) => {
                let write_failed = xfer
                    .file
                    .as_mut()
                    .is_some_and(|file| file.write_all(&buf[..n]).is_err());
                if write_failed {
                    xfer.file = None;
                    self.fail(Failure::Transfer, false);
                    return Chunk::Failed;
                }
                xfer.bytes_written += n as u64;
                self.progress_bytes
                    .store(xfer.resume_offset + xfer.bytes_written, Ordering::SeqCst);
                Chunk::Read(n)
            }
            Err(_) => {
                xfer.file = None;
                self.fail(Failure::Transfer, false);
                Chunk::Failed
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Transfer> {
        // A prior panic left the transfer state usable, so recover.
        self.xfer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fail(&self, failure: Failure, no_retry: bool) {
        self.failure.store(failure as u8, Ordering::SeqCst);
        self.no_retry.store(no_retry, Ordering::SeqCst);
        self.status.store(Status::Failed as u8, Ordering::SeqCst);
    }

    /// Returns `false` after setting a terminal failed state.
    fn start_transfer(&self, xfer: &mut Transfer) -> bool {
        let mut request = self.agent.get(&xfer.url);
        if xfer.resume_offset > 0 {
            request = request.header("Range", &format!("bytes={}-", xfer.resume_offset));
            if !xfer.etag.is_empty() {
                request = request.header("If-Range", &xfer.etag);
            }
        }
        let Ok(response) = request.call() else {
            xfer.file = None;
            self.fail(Failure::Transfer, false);
            return false;
        };

        let status = response.status().as_u16();
        let content_type = header(&response, "content-type").unwrap_or_default();
        let etag = header(&response, "etag").unwrap_or_default();
        if !etag.is_empty() {
            xfer.etag = etag;
        }

        if status >= 400 {
            discard(xfer);
            self.fail(Failure::Server, (400..500).contains(&status));
            return false;
        }
        if !resume_response_is_valid(xfer.resume_offset, status) {
            discard(xfer);
            self.fail(Failure::ResumeRestart, false);
            return false;
        }
        if is_content_type_mismatch(&content_type, &xfer.url) {
            discard(xfer);
            self.fail(Failure::HtmlPage, true);
            return false;
        }

        xfer.reader = Some(Box::new(response.into_body().into_reader()));
        xfer.started = true;
        self.status
            .store(Status::Downloading as u8, Ordering::SeqCst);
        true
    }

    fn finalize_complete(&self, xfer: &mut Transfer) {
        xfer.file = None;
        let total = xfer.resume_offset + xfer.bytes_written;
        let _ = meta::write(
            &xfer.path,
            &Meta {
                file_size: total,
                bytes_written: total,
                etag: meta::pack_etag(&xfer.etag),
            },
        );
        self.progress_bytes.store(total, Ordering::SeqCst);
        self.progress_total.store(total, Ordering::SeqCst);
        self.status.store(Status::Complete as u8, Ordering::SeqCst);
    }
}

/// Close and delete the partial file and its sidecar.
fn discard(xfer: &mut Transfer) {
    xfer.file = None;
    let _ = fs::remove_file(&xfer.path);
    meta::delete(&xfer.path);
}

fn save_meta(xfer: &Transfer) {
    // A body of unknown length cannot be validated on resume, so it gets no
    // sidecar and restarts instead.
    let Some(file_size) = xfer.file_size else {
        return;
    };
    if xfer.bytes_written == 0 {
        return;
    }
    let _ = meta::write(
        &xfer.path,
        &Meta {
            file_size,
            bytes_written: xfer.resume_offset + xfer.bytes_written,
            etag: meta::pack_etag(&xfer.etag),
        },
    );
}

impl Drop for Download {
    /// An in-flight transfer leaves a resumable sidecar; a failure that never
    /// wrote a byte leaves nothing behind.
    fn drop(&mut self) {
        let status = self.status();
        let xfer = self.lock();
        if xfer.is_cached {
            return;
        }
        if status == Status::Downloading && xfer.bytes_written > 0 {
            save_meta(&xfer);
        } else if status == Status::Failed && xfer.bytes_written == 0 {
            let _ = fs::remove_file(&xfer.path);
            meta::delete(&xfer.path);
        }
    }
}

fn prepare(agents: &Agents, path: &Path, url: &str, allow_resume: bool) -> Result<Setup> {
    if let Some(cached) = open_cached(path) {
        return Ok(cached);
    }
    if allow_resume {
        if let Some(resumed) = open_resume(path) {
            return Ok(resumed);
        }
        meta::delete(path);
        let _ = fs::remove_file(path);
    }
    start_fresh(agents, path, url)
}

fn open_cached(path: &Path) -> Option<Setup> {
    if !cache::is_complete_at(path) {
        return None;
    }
    let file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    Some(Setup::Cached { file, size })
}

fn open_resume(path: &Path) -> Option<Setup> {
    let resume = plan_resume(path)?;
    let file = OpenOptions::new().append(true).open(path).ok()?;
    Some(Setup::Resume { file, resume })
}

fn start_fresh(agents: &Agents, path: &Path, url: &str) -> Result<Setup> {
    let file_size = url_size(&agents.short, url);
    check_disk_space(path, file_size)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty() && !parent.is_dir())
    {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let file = File::create(path).map_err(|e| Error::io(path, e))?;
    Ok(Setup::Fresh { file, file_size })
}

/// What the partial file and its sidecar agree can be skipped, if anything.
fn plan_resume(path: &Path) -> Option<Resume> {
    let meta = meta::read(path)?;
    let on_disk = fs::metadata(path).ok()?.len();
    if on_disk != meta.bytes_written || meta.bytes_written == 0 {
        return None;
    }
    // Complete or inconsistent: restart rather than resume past the end.
    if meta.bytes_written >= meta.file_size {
        return None;
    }
    check_disk_space(path, Some(meta.file_size - meta.bytes_written)).ok()?;
    Some(Resume {
        offset: meta.bytes_written,
        file_size: meta.file_size,
        etag: meta::unpack_etag(&meta.etag),
    })
}

/// Always unknown: `std` exposes no free-space query, and this crate forbids
/// the `unsafe` a syscall would need.
fn available_disk_space(_path: &Path) -> Option<u64> {
    None
}

/// Unknown free space, or an unknown transfer length, allows the transfer.
fn check_disk_space(path: &Path, required: Option<u64>) -> Result<()> {
    let (Some(required), Some(available)) = (required, available_disk_space(path)) else {
        return Ok(());
    };
    if has_enough_disk_space(required, available) {
        return Ok(());
    }
    Err(Error::DiskSpace {
        path: path.to_path_buf(),
        required,
        available,
    })
}

/// Size plus 10% headroom, minimum 1 MiB.
fn has_enough_disk_space(required: u64, available: u64) -> bool {
    if required == 0 {
        return true;
    }
    let headroom = (required / 10).max(1024 * 1024);
    available >= required.saturating_add(headroom)
}

/// A fresh transfer accepts any success status; a resume needs 206.
fn resume_response_is_valid(resume_offset: u64, status: u16) -> bool {
    resume_offset == 0 || status == PARTIAL_CONTENT
}

/// `text/html` for a non-HTML URL is a likely error page.
fn is_content_type_mismatch(content_type: &str, url: &str) -> bool {
    if !content_type
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("text/html"))
    {
        return false;
    }
    !["html", "htm", "xhtml"]
        .iter()
        .any(|ext| url_has_extension(url, ext))
}

fn url_has_extension(url: &str, ext: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        // A leading dot (".html") is a hidden file, not an extension.
        Some(0) | None => false,
        Some(i) => name[i + 1..].eq_ignore_ascii_case(ext),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_partial(path: &Path, bytes: &[u8], file_size: u64, etag: &str) {
        fs::write(path, bytes).unwrap();
        meta::write(
            path,
            &Meta {
                file_size,
                bytes_written: bytes.len() as u64,
                etag: meta::pack_etag(etag),
            },
        );
    }

    fn transfer(path: PathBuf, file_size: Option<u64>, bytes_written: u64) -> Transfer {
        Transfer {
            url: "https://example.invalid/a".to_string(),
            path,
            file_size,
            bytes_written,
            resume_offset: 0,
            etag: String::new(),
            started: true,
            is_cached: false,
            reader: None,
            file: None,
            cached_file: None,
        }
    }

    #[test]
    fn a_resume_needs_partial_content() {
        assert!(resume_response_is_valid(0, 200));
        assert!(resume_response_is_valid(0, 206));
        assert!(resume_response_is_valid(0, 416));
        assert!(resume_response_is_valid(1024, 206));
        assert!(!resume_response_is_valid(1024, 200));
        assert!(!resume_response_is_valid(1024, 416));
        assert!(!resume_response_is_valid(1024, 204));
        assert!(!resume_response_is_valid(1024, 302));
    }

    #[test]
    fn only_a_transfer_of_known_length_leaves_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"twelve bytes").unwrap();

        save_meta(&transfer(path.clone(), None, 12));
        assert!(meta::read(&path).is_none());

        // Nothing written yet is nothing to resume from, length known or not.
        save_meta(&transfer(path.clone(), Some(1000), 0));
        assert!(meta::read(&path).is_none());

        save_meta(&transfer(path.clone(), Some(1000), 12));
        let meta = meta::read(&path).unwrap();
        assert_eq!(meta.file_size, 1000);
        assert_eq!(meta.bytes_written, 12);
    }

    #[test]
    fn html_for_a_binary_url_is_a_mismatch() {
        assert!(is_content_type_mismatch(
            "text/html; charset=utf-8",
            "https://x/plugin.zip"
        ));
        assert!(!is_content_type_mismatch("", "https://x/plugin.zip"));
        assert!(!is_content_type_mismatch(
            "application/zip",
            "https://x/plugin.zip"
        ));
        assert!(!is_content_type_mismatch(
            "text/html",
            "https://x/index.html"
        ));
        assert!(!is_content_type_mismatch("text/html", "https://x/page.HTM"));
        assert!(!is_content_type_mismatch(
            "text/html",
            "https://x/index.html?v=2"
        ));
        // Shorter than "text/html": no panic on the 9-byte prefix.
        assert!(!is_content_type_mismatch(
            "text/htm",
            "https://x/plugin.zip"
        ));
        // A bare ".html" segment is a hidden file, not an extension.
        assert!(is_content_type_mismatch("text/html", "https://x/.html"));
    }

    #[test]
    fn the_disk_gate_wants_ten_percent_or_a_megabyte() {
        assert!(has_enough_disk_space(0, 0));
        // 100 bytes needs 100 + max(10, 1 MiB).
        assert!(has_enough_disk_space(100, 2 * 1024 * 1024));
        assert!(!has_enough_disk_space(100, 1024));
        // 100 MiB needs 110 MiB: the 10% dominates the 1 MiB floor.
        let hundred_mib = 100 * 1024 * 1024;
        assert!(has_enough_disk_space(hundred_mib, 111 * 1024 * 1024));
        assert!(!has_enough_disk_space(hundred_mib, 105 * 1024 * 1024));
    }

    #[test]
    fn a_partial_file_resumes_from_its_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        write_partial(&path, b"twelve bytes", 1000, "\"v1\"");

        let resume = plan_resume(&path).unwrap();
        assert_eq!(resume.offset, 12);
        assert_eq!(resume.file_size, 1000);
        assert_eq!(resume.etag, "\"v1\"");
    }

    #[test]
    fn a_partial_file_that_disagrees_with_its_sidecar_does_not_resume() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");

        // No sidecar at all.
        fs::write(&path, b"twelve bytes").unwrap();
        assert!(plan_resume(&path).is_none());

        // Sidecar disagrees with the bytes on disk.
        write_partial(&path, b"twelve bytes", 1000, "");
        fs::write(&path, b"four").unwrap();
        assert!(plan_resume(&path).is_none());

        // Nothing written yet.
        write_partial(&path, b"", 1000, "");
        assert!(plan_resume(&path).is_none());

        // Claims more bytes than the file is long.
        write_partial(&path, b"twelve bytes", 4, "");
        assert!(plan_resume(&path).is_none());
    }

    #[test]
    fn a_complete_entry_is_served_from_disk_without_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        write_partial(&path, b"cached bytes", 12, "\"v1\"");

        let agents = Agents::new();
        let download = Download::start(&agents, path, "https://example.invalid/a", true).unwrap();
        assert_eq!(download.status(), Status::Complete);

        let snapshot = download.progress();
        assert_eq!(snapshot.downloaded, 12);
        assert_eq!(snapshot.total, Some(12));

        let mut buf = [0u8; 32];
        assert_eq!(download.read_chunk(&mut buf), Chunk::Read(12));
        assert_eq!(&buf[..12], b"cached bytes");
        assert_eq!(download.read_chunk(&mut buf), Chunk::Idle);
    }

    #[test]
    fn a_resumable_entry_starts_pending_at_its_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        write_partial(&path, b"twelve bytes", 1000, "\"v1\"");

        let agents = Agents::new();
        let download =
            Download::start(&agents, path.clone(), "https://example.invalid/a", true).unwrap();
        assert_eq!(download.status(), Status::Pending);

        let snapshot = download.progress();
        assert_eq!(snapshot.downloaded, 12);
        assert_eq!(snapshot.total, Some(1000));
        assert_eq!(download.path(), path);

        // Dropping mid-transfer must leave the partial file and its sidecar.
        drop(download);
        assert_eq!(fs::read(&path).unwrap(), b"twelve bytes");
        assert!(meta::read(&path).is_some());
    }

    #[test]
    fn an_empty_url_is_refused() {
        let agents = Agents::new();
        let dir = tempfile::tempdir().unwrap();
        let error = Download::start(&agents, dir.path().join("a"), "", true)
            .err()
            .unwrap();
        assert!(matches!(error, Error::EmptyUrl));
    }
}
