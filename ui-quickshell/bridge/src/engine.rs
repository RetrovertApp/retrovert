//! The audio worker that lives inside the shell process.
//!
//! One thread owns the [`PlayerBackend`], the cpal stream, the playlist and a private set of
//! [`ScopeTrace`]s. Every loop it decodes as much audio as wall-clock time has passed, so
//! the song advances at a steady rate rather than in the device's callback-sized bursts, and
//! the ring in front of the device only absorbs jitter. Sixty times a second it captures a
//! [`VizSnapshot`], refills its traces and meter and copies them into the shared [`Front`]
//! under a lock held only for that copy. The render thread reads the front the same way,
//! so neither side ever waits on decoding.
//!
//! The pacing matters for the scopes: a decoder exposes the samples it mixed last, so the
//! picture only moves when the decoder does. Rendered in callback-sized bursts it moved
//! about twenty-four times a second; paced, every capture sees fresh samples.
//!
//! Everything that is not per frame, the library rows, the playing song's metadata and the
//! queue, is published as JSON [`Docs`] behind a second lock, so a large library never sits
//! between the render thread and its frame. The library is described one file per loop, only
//! while the ring is comfortably ahead of the device, because the decoders that read the
//! metadata live on this thread and load only once.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use playlist_engine::{Engine as Playlist, ItemSpec, Mode};
use retrovert_host::service::{MetadataValue, TrackMetadata};
use retrovert_host::session::StreamFormat;
use retrovert_host::visualization::{VisualizationConfig, VizSnapshot, MAX_SCOPE_CHANNELS};
use retrovert_library::Entry;
use retrovert_player::{PlaybackBackend, PlaybackStatus, PlayerBackend};
use retrovert_present::{ScopePoint, ScopeTrace, VuMeter};
use serde::Serialize;

/// The rate the device is opened at and the decoders render to.
pub const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const MAX_FRAMES: u32 = 4_096;
/// How much audio the worker keeps decoded ahead of the device.
const BUFFERED_SAMPLES: usize = (SAMPLE_RATE as usize / 5) * CHANNELS as usize;
/// Below this the ring is refilled as fast as possible: at mount, and after a stall.
const LOW_WATER_SAMPLES: usize = BUFFERED_SAMPLES / 4;
/// Library files are described only while the ring holds at least this much.
const SCAN_WATER_SAMPLES: usize = BUFFERED_SAMPLES / 2;
const WORKER_SLEEP: Duration = Duration::from_millis(2);
/// How long a closing session waits for the worker to leave a decoder call before giving up
/// on it: a plugin blocked in its own I/O must not hold the window open.
const QUIT_GRACE: Duration = Duration::from_millis(500);
const CAPTURE_INTERVAL: Duration = Duration::from_micros(16_667);
/// How much of full scale a VU level falls per capture when the channel goes quiet.
const VU_FALL_PER_FRAME: f32 = 0.06;
/// A scan republishes the library after this many new rows, or this long, whichever first.
const SCAN_PUBLISH_ROWS: usize = 100;
const SCAN_PUBLISH_INTERVAL: Duration = Duration::from_millis(400);
/// "Previous" inside the first seconds of a song goes to the song before; later it restarts.
const RESTART_AFTER_MS: i64 = 3_000;
/// Points reserved per channel, comfortably above one pixel per point at any sane width.
pub const TRACE_CAPACITY: usize = 1024;

/// What the UI is told about playback, as the C ABI spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    /// Nothing mounted.
    Idle = 0,
    /// A song is decoding.
    Playing = 1,
    /// The song reached its end and nothing follows it.
    Finished = 2,
    /// The last open or render failed; see the error text.
    Error = 3,
    /// A song is mounted and held.
    Paused = 4,
}

/// What happens when a song ends, as the C ABI spells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum LoopMode {
    /// The queue plays through once.
    #[default]
    Off = 0,
    /// The song restarts.
    One = 1,
    /// The queue starts over.
    All = 2,
}

impl LoopMode {
    /// The mode a raw discriminant names; anything unknown is `Off`.
    pub fn from_u32(raw: u32) -> Self {
        match raw {
            1 => Self::One,
            2 => Self::All,
            _ => Self::Off,
        }
    }
}

/// Which JSON document [`Docs`] is asked for, as the C ABI spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Document {
    /// The library: root, decoders, and one row per song.
    Library = 0,
    /// The playing song's metadata.
    Track = 1,
    /// What follows the playing song.
    Queue = 2,
}

impl Document {
    /// The document a raw discriminant names, if any.
    pub fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Library),
            1 => Some(Self::Track),
            2 => Some(Self::Queue),
            _ => None,
        }
    }
}

/// The tracker position of the last capture, or `has == false` for a decoder without one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackerPosition {
    /// Whether the decoder reported a position.
    pub has: bool,
    /// Order-list index.
    pub order: u32,
    /// Pattern number.
    pub pattern: u32,
    /// Row within the pattern.
    pub row: u32,
}

/// The per-frame state shared between the worker and the render thread.
pub struct Front {
    /// One polyline per scope channel of the mounted song.
    pub traces: Vec<ScopeTrace>,
    /// Decayed VU levels, one per scope channel, in 0..=1.
    pub vu: [f32; MAX_SCOPE_CHANNELS as usize],
    /// How many of `vu` are populated.
    pub vu_len: usize,
    /// Bumped once per capture, so a reader can tell a new frame from a repeat.
    pub frame: u64,
    /// Playback state.
    pub status: Status,
    /// Delivered position in milliseconds.
    pub position_ms: i64,
    /// Length of the playing subsong in milliseconds, 0 when the decoder gave none.
    pub duration_ms: i64,
    /// The playing subsong, and how many the song holds.
    pub subsong: u32,
    /// See `subsong`.
    pub subsong_count: u32,
    /// Where the tracker is, when the decoder says.
    pub position: TrackerPosition,
    /// Output gain, 0..=1.
    pub volume: f32,
    /// What happens when the song ends.
    pub loop_mode: LoopMode,
    /// Library rows described so far, and how many the walk found.
    pub library_scanned: u32,
    /// See `library_scanned`.
    pub library_total: u32,
    /// Bumped whenever the matching document in [`Docs`] changes.
    pub library_rev: u64,
    /// See `library_rev`.
    pub track_rev: u64,
    /// See `library_rev`.
    pub queue_rev: u64,
    /// The decoder that took the song, or empty.
    pub plugin: String,
    /// The last failure, or empty.
    pub error: String,
}

impl Front {
    fn new() -> Self {
        Self {
            traces: Vec::new(),
            vu: [0.0; MAX_SCOPE_CHANNELS as usize],
            vu_len: 0,
            frame: 0,
            status: Status::Idle,
            position_ms: 0,
            duration_ms: 0,
            subsong: 0,
            subsong_count: 0,
            position: TrackerPosition::default(),
            volume: 1.0,
            loop_mode: LoopMode::Off,
            library_scanned: 0,
            library_total: 0,
            library_rev: 0,
            track_rev: 0,
            queue_rev: 0,
            plugin: String::new(),
            error: String::new(),
        }
    }
}

/// The JSON documents the UI reads at its own pace: none is per frame, and each may be large.
#[derive(Default)]
pub struct Docs {
    /// See [`Document::Library`].
    pub library: String,
    /// See [`Document::Track`].
    pub track: String,
    /// See [`Document::Queue`].
    pub queue: String,
}

impl Docs {
    /// The text of one document.
    pub fn get(&self, which: Document) -> &str {
        match which {
            Document::Library => &self.library,
            Document::Track => &self.track,
            Document::Queue => &self.queue,
        }
    }
}

/// Locks a poisoned mutex anyway: the data is plain values a panic cannot leave torn.
pub fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// What the UI asks of the worker.
pub enum Command {
    /// Play one file now, as a queue of its own.
    Open(PathBuf),
    /// Play library rows in the given order, starting at the given index into it.
    PlayRows(Vec<u32>, usize),
    /// Drop the current song and the queue.
    Stop,
    /// Hold or resume.
    TogglePause,
    /// Move the playhead to a millisecond position.
    Seek(i64),
    /// Skip to what follows.
    Next,
    /// Restart the song, or early in it go to what came before.
    Previous,
    /// Open another subsong of the playing song.
    SetSubsong(u32),
    /// Decide what happens when a song ends.
    SetLoop(LoopMode),
    /// Output gain, clamped to 0..=1.
    SetVolume(f32),
    /// Keep the playing song and drop everything queued after it.
    ClearQueue,
    /// Walk a directory and describe every file a decoder claims.
    ScanLibrary(PathBuf),
    /// The walk of `ScanLibrary` finished; sent by its helper thread.
    Walked(PathBuf, Vec<PathBuf>),
    /// End the worker.
    Quit,
}

/// Handle to the worker thread, owned by the UI session.
pub struct Engine {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    /// Signalled (by disconnecting) when the worker returns.
    exited: Receiver<()>,
    /// The per-frame state the render thread reads.
    pub front: Arc<Mutex<Front>>,
    /// The documents the UI reads when their revision moves.
    pub docs: Arc<Mutex<Docs>>,
}

impl Engine {
    /// Loads every plugin under `plugin_dir` and starts the worker. Setup cadence.
    pub fn start(plugin_dir: &Path) -> Self {
        let front = Arc::new(Mutex::new(Front::new()));
        let docs = Arc::new(Mutex::new(Docs::default()));
        let (commands, receiver) = channel();
        let paths = plugin_paths(plugin_dir);
        log::info!(
            "{} plugin libraries under {}",
            paths.len(),
            plugin_dir.display()
        );
        let worker_front = Arc::clone(&front);
        let worker_docs = Arc::clone(&docs);
        let worker_commands = commands.clone();
        let (exit_tx, exited) = channel::<()>();
        let thread = std::thread::Builder::new()
            .name("audio-worker".to_string())
            .spawn(move || {
                // Dropped when the worker returns, which disconnects `exited`.
                let _exit = exit_tx;
                Worker::new(&paths, worker_front, worker_docs, worker_commands).run(&receiver);
            })
            .ok();
        if thread.is_none() {
            lock(&front).error = "could not start the audio worker".to_string();
        }
        Self {
            commands,
            thread,
            exited,
            front,
            docs,
        }
    }

    /// Sends one command; a worker that has gone drops it.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        let Some(thread) = self.thread.take() else {
            return;
        };
        // The worker answers Quit within one loop unless a decoder has it. A decoder stuck in
        // its own child process would otherwise hold the whole shell past Ctrl-C; the thread
        // is left to die with the process instead.
        match self.exited.recv_timeout(QUIT_GRACE) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                log::warn!("the audio worker is stuck in a decoder; leaving it behind");
            }
        }
    }
}

/// Every shared library under `dir`, recursively, in a stable order.
fn plugin_paths(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "so") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

/// The decoder names the rail lists: `hively_playback.so` is "hively".
fn plugin_labels(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .map(|stem| {
            stem.trim_start_matches("lib")
                .trim_end_matches("_playback")
                .to_owned()
        })
        .collect()
}

fn start_output(ring: &Arc<Mutex<VecDeque<f32>>>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let config = cpal::StreamConfig {
        channels: CHANNELS,
        sample_rate: cpal::SampleRate(SAMPLE_RATE),
        buffer_size: cpal::BufferSize::Default,
    };
    let ring = Arc::clone(ring);
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut ring = lock(&ring);
                for sample in out.iter_mut() {
                    *sample = ring.pop_front().unwrap_or(0.0);
                }
            },
            |e| log::error!("audio output: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

/// A queued song: its file and, when it came from the library, its row.
#[derive(Clone, Debug)]
struct Track {
    path: PathBuf,
    row: Option<u32>,
}

/// The library as the worker holds it while and after describing it.
struct Library {
    root: PathBuf,
    plugins: Vec<String>,
    pending: VecDeque<PathBuf>,
    entries: Vec<Entry>,
    /// Rows added since the last publish, and when that was.
    unpublished: usize,
    published_at: Instant,
}

#[derive(Serialize)]
struct LibraryDoc<'a> {
    root: &'a Path,
    plugins: &'a [String],
    scanned: usize,
    total: usize,
    rows: &'a [Entry],
}

#[derive(Serialize)]
struct SubsongDoc<'a> {
    index: u32,
    name: &'a str,
    length_ms: u32,
}

#[derive(Serialize)]
struct TrackDoc<'a> {
    title: &'a str,
    composer: &'a str,
    format: &'a str,
    plugin: &'a str,
    channels: usize,
    samples: Vec<&'a str>,
    instruments: Vec<&'a str>,
    subsongs: Vec<SubsongDoc<'a>>,
    subsong: u32,
    year: u32,
    file: &'a str,
    path: &'a Path,
    size: u64,
    row: Option<u32>,
}

#[derive(Serialize)]
struct QueueItemDoc<'a> {
    row: Option<u32>,
    title: &'a str,
    composer: &'a str,
    format: &'a str,
    duration_ms: u32,
}

#[derive(Serialize)]
struct QueueDoc<'a> {
    items: Vec<QueueItemDoc<'a>>,
    total_ms: u64,
}

/// The song currently mounted and what its decoder said about it.
struct Playing {
    track: Track,
    metadata: TrackMetadata,
    entry: Entry,
    subsong: u32,
    /// The visualization slot sized for this song.
    snapshot: VizSnapshot,
}

struct Worker {
    backend: PlayerBackend,
    ring: Arc<Mutex<VecDeque<f32>>>,
    _stream: Option<cpal::Stream>,
    front: Arc<Mutex<Front>>,
    docs: Arc<Mutex<Docs>>,
    commands: Sender<Command>,
    playlist: Playlist<Track>,
    playing: Option<Playing>,
    library: Library,
    traces: Vec<ScopeTrace>,
    vu: VuMeter,
    pacer: Pacer,
    next_capture: Instant,
    frame: u64,
    volume: f32,
    loop_mode: LoopMode,
}

impl Worker {
    fn new(
        plugins: &[PathBuf],
        front: Arc<Mutex<Front>>,
        docs: Arc<Mutex<Docs>>,
        commands: Sender<Command>,
    ) -> Self {
        let backend = PlayerBackend::new(
            plugins,
            VisualizationConfig::default(),
            StreamFormat {
                sample_rate: SAMPLE_RATE,
                channels: u32::from(CHANNELS),
            },
            MAX_FRAMES,
        );
        // Setup cadence: sized for the target depth plus one full render, so `top_up` never grows it.
        let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(
            BUFFERED_SAMPLES + MAX_FRAMES as usize * CHANNELS as usize,
        )));
        // Built here so the stream lives and dies on this thread.
        let stream = match start_output(&ring) {
            Ok(stream) => Some(stream),
            Err(message) => {
                log::warn!("no audio output ({message}); decoding continues silently");
                None
            }
        };
        let mut worker = Self {
            backend,
            ring,
            _stream: stream,
            front,
            docs,
            commands,
            playlist: Playlist::new(),
            playing: None,
            library: Library {
                root: PathBuf::new(),
                plugins: plugin_labels(plugins),
                pending: VecDeque::new(),
                entries: Vec::new(),
                unpublished: 0,
                published_at: Instant::now(),
            },
            traces: Vec::new(),
            vu: VuMeter::new(MAX_SCOPE_CHANNELS as usize, VU_FALL_PER_FRAME),
            pacer: Pacer::new(),
            next_capture: Instant::now(),
            frame: 0,
            volume: 1.0,
            loop_mode: LoopMode::Off,
        };
        worker.publish_library();
        worker.publish_track();
        worker.publish_queue();
        worker
    }

    fn run(mut self, commands: &Receiver<Command>) {
        loop {
            match commands.try_recv() {
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => return,
                Ok(command) => self.handle(command),
                Err(TryRecvError::Empty) => {}
            }

            if let Some(item) = self.playlist.poll() {
                let track = item.payload.clone();
                self.mount(track, 0);
            }

            if self.backend.status() == PlaybackStatus::Finished {
                self.song_ended();
            }

            if let Err(e) = top_up(&mut self.backend, &self.ring, &mut self.pacer, self.volume) {
                self.unmount();
                let mut f = lock(&self.front);
                f.status = Status::Error;
                f.error = format!("render failed: {e}");
                self.playlist.on_failed();
            }

            let now = Instant::now();
            if now >= self.next_capture {
                // Keeps the phase, so the sleep granularity does not stretch the interval; a
                // long stall resets rather than replaying missed captures.
                self.next_capture += CAPTURE_INTERVAL;
                if self.next_capture + CAPTURE_INTERVAL < now {
                    self.next_capture = now + CAPTURE_INTERVAL;
                }
                self.capture();
            }

            self.scan_step();
            std::thread::sleep(WORKER_SLEEP);
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Open(path) => {
                let row = self
                    .library
                    .entries
                    .iter()
                    .find(|e| e.path == path)
                    .map(|e| e.id);
                self.start_queue(vec![Track { path, row }], 0);
            }
            Command::PlayRows(rows, start) => self.play_rows(&rows, start),
            Command::Stop => {
                self.playlist.stop();
                self.unmount();
                lock(&self.front).status = Status::Idle;
                self.publish_queue();
            }
            Command::TogglePause => self.toggle_pause(),
            Command::Seek(ms) => self.seek(ms),
            Command::Next => self.playlist.next(),
            Command::Previous => self.previous(),
            Command::SetSubsong(index) => self.set_subsong(index),
            Command::SetLoop(mode) => {
                self.loop_mode = mode;
                self.playlist.set_mode(playlist_mode(mode));
                lock(&self.front).loop_mode = mode;
                self.publish_queue();
            }
            Command::SetVolume(volume) => {
                if volume.is_finite() {
                    self.volume = volume.clamp(0.0, 1.0);
                }
                lock(&self.front).volume = self.volume;
            }
            Command::ClearQueue => self.clear_queue(),
            Command::ScanLibrary(root) => self.scan_library(root),
            Command::Walked(root, files) => self.walked(&root, files),
            Command::Quit => {}
        }
    }

    fn play_rows(&mut self, rows: &[u32], start: usize) {
        let tracks: Vec<Track> = rows
            .iter()
            .filter_map(|&row| self.library.entries.get(row as usize))
            .map(|entry| Track {
                path: entry.path.clone(),
                row: Some(entry.id),
            })
            .collect();
        if !tracks.is_empty() {
            let last = tracks.len() - 1;
            self.start_queue(tracks, start.min(last));
        }
    }

    fn toggle_pause(&mut self) {
        let Some(playing) = self.backend.toggle_pause() else {
            return;
        };
        lock(&self.front).status = if playing {
            Status::Playing
        } else {
            Status::Paused
        };
        if !playing {
            // A held song must not leave the ring's tail playing on.
            clear(&self.ring);
        }
    }

    fn seek(&mut self, ms: i64) {
        let Some(reached) = self.backend.seek(ms.max(0)) else {
            return;
        };
        clear(&self.ring);
        let mut f = lock(&self.front);
        f.position_ms = reached;
        if f.status == Status::Finished {
            f.status = Status::Playing;
        }
    }

    fn previous(&mut self) {
        let position = self.backend.position_ms().unwrap_or(0);
        if position > RESTART_AFTER_MS || !self.has_previous() {
            if self.backend.seek(0).is_some() {
                clear(&self.ring);
                lock(&self.front).position_ms = 0;
            }
        } else {
            self.playlist.previous();
        }
    }

    fn set_subsong(&mut self, index: u32) {
        let Some(playing) = self.playing.as_ref() else {
            return;
        };
        let count = u32::try_from(playing.metadata.subsongs.len()).unwrap_or(u32::MAX);
        if index < count && index != playing.subsong {
            let track = playing.track.clone();
            self.mount(track, index);
        }
    }

    fn clear_queue(&mut self) {
        if let Some(playlist) = self.playlist.playing_playlist() {
            let current = self.playlist.current_item_id();
            let ids: Vec<_> = self
                .playlist
                .playlist(playlist)
                .map(|p| p.items().iter().map(playlist_engine::Item::id).collect())
                .unwrap_or_default();
            let mut past_current = current.is_none();
            for id in ids {
                if past_current {
                    self.playlist.remove_item(playlist, id);
                }
                past_current |= Some(id) == current;
            }
        }
        self.publish_queue();
    }

    fn scan_library(&mut self, root: PathBuf) {
        let commands = self.commands.clone();
        let walk_root = root.clone();
        self.library.root = root;
        self.library.pending.clear();
        self.library.entries.clear();
        self.publish_library();
        // The walk touches no decoder, so it need not wait on this thread.
        let spawned = std::thread::Builder::new()
            .name("library-walk".to_string())
            .spawn(move || {
                let files = retrovert_library::walk(&walk_root);
                let _ = commands.send(Command::Walked(walk_root, files));
            });
        if spawned.is_err() {
            log::warn!("could not start the library walk");
        }
    }

    fn walked(&mut self, root: &Path, files: Vec<PathBuf>) {
        if root != self.library.root {
            return;
        }
        self.library.pending = files
            .into_iter()
            .filter(|path| !self.claimed(path).is_empty())
            .collect();
        log::info!(
            "library: {} playable files under {}",
            self.library.pending.len(),
            root.display()
        );
        self.publish_library();
    }

    /// The extension, or the `mod.name` style prefix, some decoder claims `path` by; empty
    /// when none does. Doubles as the row's format label.
    fn claimed(&self, path: &Path) -> String {
        let extension = retrovert_library::extension(path);
        if !extension.is_empty() && self.backend.can_handle_extension(&extension) {
            return extension;
        }
        let prefix = retrovert_library::prefix(path);
        if !prefix.is_empty() && self.backend.can_handle_extension(&prefix) {
            return prefix;
        }
        String::new()
    }

    fn has_previous(&self) -> bool {
        let Some(playlist) = self.playlist.playing_playlist() else {
            return false;
        };
        let Some(current) = self.playlist.current_item_id() else {
            return false;
        };
        self.playlist
            .playlist(playlist)
            .and_then(|p| p.items().iter().position(|i| i.id() == current))
            .is_some_and(|index| index > 0)
    }

    /// Replaces the queue with `tracks` and starts at `start`. The playlist engine hands the
    /// item back through `poll` on the next loop, which is where the mount happens.
    fn start_queue(&mut self, tracks: Vec<Track>, start: usize) {
        self.playlist.stop();
        let Some(id) = self.playlist.create(None) else {
            return;
        };
        let mut start_item = None;
        for (index, track) in tracks.into_iter().enumerate() {
            let entry = track
                .row
                .and_then(|row| self.library.entries.get(row as usize));
            let mut spec = ItemSpec::new(track.path.to_string_lossy().into_owned(), track);
            if let Some(entry) = entry {
                spec.title.clone_from(&entry.title);
                spec.artist.clone_from(&entry.composer);
                spec.duration_ms = entry.duration_ms;
            }
            let item = self.playlist.append(id, spec);
            if index == start {
                start_item = item;
            }
        }
        self.playlist.set_mode(playlist_mode(self.loop_mode));
        self.playlist.start(id, start_item);
        self.publish_queue();
    }

    /// Mounts `track` at `subsong`, publishes what the decoder said, or records the failure.
    fn mount(&mut self, track: Track, subsong: u32) {
        clear(&self.ring);
        self.playing = None;
        let subsong_index = u16::try_from(subsong).unwrap_or(0);
        match self.backend.mount(&track.path, subsong_index) {
            Ok(()) => {
                // Setup cadence: the one place the per-song buffers are sized.
                let layout = self.backend.visualization_layout().cloned();
                let snapshot = layout
                    .as_ref()
                    .and_then(|layout| layout.new_snapshot().ok());
                let channels = layout.as_ref().map_or(0, |l| l.scope_channels.len());
                let Some(snapshot) = snapshot else {
                    self.fail(&format!(
                        "could not play {}: the decoder published no visualization layout",
                        track.path.display()
                    ));
                    return;
                };
                let metadata = match self.backend.probe_metadata(&track.path) {
                    Ok(Some(metadata)) => metadata,
                    Ok(None) => TrackMetadata::default(),
                    Err(e) => {
                        log::warn!("no metadata for {}: {e}", track.path.display());
                        TrackMetadata::default()
                    }
                };
                let format = self.claimed(&track.path);
                let entry = Entry::describe(
                    track.row.unwrap_or(u32::MAX),
                    &self.library.root,
                    &track.path,
                    &format,
                    Some(&metadata),
                );
                self.traces = (0..channels)
                    .map(|_| ScopeTrace::new(TRACE_CAPACITY))
                    .collect();
                self.vu.clear();
                let duration_ms = metadata
                    .subsongs
                    .get(subsong as usize)
                    .and_then(|s| s.length_seconds)
                    .map_or(entry.duration_ms, retrovert_library::seconds_to_ms);
                {
                    let mut f = lock(&self.front);
                    f.traces = (0..channels)
                        .map(|_| ScopeTrace::new(TRACE_CAPACITY))
                        .collect();
                    f.vu_len = 0;
                    f.status = Status::Playing;
                    f.plugin = self.backend.plugin_name().to_string();
                    f.error.clear();
                    f.position_ms = 0;
                    f.duration_ms = i64::from(duration_ms);
                    f.subsong = subsong;
                    f.subsong_count = u32::try_from(metadata.subsongs.len()).unwrap_or(u32::MAX);
                    f.position = TrackerPosition::default();
                    log::info!("playing {} via {}", track.path.display(), f.plugin);
                }
                self.playing = Some(Playing {
                    track,
                    metadata,
                    entry,
                    subsong,
                    snapshot,
                });
                self.publish_track();
                self.publish_queue();
            }
            Err(e) => {
                self.fail(&format!("could not play {}: {e}", track.path.display()));
            }
        }
    }

    fn fail(&mut self, message: &str) {
        log::error!("{message}");
        self.unmount();
        {
            let mut f = lock(&self.front);
            f.status = Status::Error;
            message.clone_into(&mut f.error);
        }
        self.playlist.on_failed();
        self.publish_track();
        self.publish_queue();
    }

    /// Drops the mounted song and its buffers; the front's status is the caller's to set.
    fn unmount(&mut self) {
        self.backend.close();
        self.playing = None;
        self.traces.clear();
        self.vu.clear();
        clear(&self.ring);
        let mut f = lock(&self.front);
        f.traces.clear();
        f.vu_len = 0;
        f.position = TrackerPosition::default();
    }

    /// The decoder reached the end: restart, advance, or come to rest.
    fn song_ended(&mut self) {
        if self.loop_mode == LoopMode::One && self.backend.seek(0).is_some() {
            clear(&self.ring);
            lock(&self.front).position_ms = 0;
            return;
        }
        self.playlist.on_finished();
        if self.playlist.peek_next().is_some() {
            // The next loop's poll mounts it; nothing to show in between.
            return;
        }
        self.playlist.stop();
        self.unmount();
        lock(&self.front).status = Status::Finished;
        self.publish_queue();
    }

    /// Captures one frame into the traces and meter and copies them to the front.
    fn capture(&mut self) {
        let Some(playing) = self.playing.as_mut() else {
            return;
        };
        if self.backend.capture(&mut playing.snapshot).is_err() {
            return;
        }
        for (channel, trace) in self.traces.iter_mut().enumerate() {
            trace.fill_from(&playing.snapshot, channel);
        }
        self.vu.tick_from(&playing.snapshot);
        self.frame = self.frame.wrapping_add(1);
        let position = self.backend.position_ms().unwrap_or(0);
        let tracker = playing
            .snapshot
            .position
            .map_or(TrackerPosition::default(), |p| TrackerPosition {
                has: true,
                order: p.order,
                pattern: p.pattern,
                row: p.row,
            });
        // Held for a memcpy per channel and nothing else.
        let mut f = lock(&self.front);
        for (dst, src) in f.traces.iter_mut().zip(self.traces.iter()) {
            dst.copy_from(src);
        }
        let levels = self.vu.levels();
        f.vu[..levels.len()].copy_from_slice(levels);
        f.vu_len = levels.len();
        f.frame = self.frame;
        f.position_ms = position;
        f.position = tracker;
    }

    /// Describes one pending library file, when the ring can afford the time.
    fn scan_step(&mut self) {
        if self.library.pending.is_empty() {
            return;
        }
        let buffered = lock(&self.ring).len();
        if self.playing.is_some() && buffered < SCAN_WATER_SAMPLES {
            return;
        }
        let Some(path) = self.library.pending.pop_front() else {
            return;
        };
        let metadata = match self.backend.probe_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) => {
                log::debug!("library: skipping {}: {e}", path.display());
                None
            }
        };
        if metadata.is_none() {
            // Claimed by extension, refused by every decoder: not a song.
            self.library_progress();
            return;
        }
        let id = u32::try_from(self.library.entries.len()).unwrap_or(u32::MAX);
        let format = self.claimed(&path);
        let entry = Entry::describe(id, &self.library.root, &path, &format, metadata.as_ref());
        self.library.entries.push(entry);
        self.library.unpublished += 1;
        self.library_progress();
    }

    fn library_progress(&mut self) {
        let done = self.library.pending.is_empty();
        if done
            || self.library.unpublished >= SCAN_PUBLISH_ROWS
            || self.library.published_at.elapsed() >= SCAN_PUBLISH_INTERVAL
        {
            self.publish_library();
            if done {
                log::info!("library: {} songs described", self.library.entries.len());
                // The playing song may now be a row.
                self.publish_track();
            }
        } else {
            let mut f = lock(&self.front);
            f.library_scanned = u32::try_from(self.library.entries.len()).unwrap_or(u32::MAX);
            f.library_total =
                u32::try_from(self.library.entries.len() + self.library.pending.len())
                    .unwrap_or(u32::MAX);
        }
    }

    fn publish_library(&mut self) {
        let scanned = self.library.entries.len();
        let total = scanned + self.library.pending.len();
        let doc = serde_json::to_string(&LibraryDoc {
            root: &self.library.root,
            plugins: &self.library.plugins,
            scanned,
            total,
            rows: &self.library.entries,
        })
        .unwrap_or_default();
        lock(&self.docs).library = doc;
        self.library.unpublished = 0;
        self.library.published_at = Instant::now();
        let mut f = lock(&self.front);
        f.library_scanned = u32::try_from(scanned).unwrap_or(u32::MAX);
        f.library_total = u32::try_from(total).unwrap_or(u32::MAX);
        f.library_rev = f.library_rev.wrapping_add(1);
    }

    fn publish_track(&mut self) {
        let doc = match self.playing.as_ref() {
            Some(playing) => {
                let row = playing.track.row.or_else(|| {
                    self.library
                        .entries
                        .iter()
                        .find(|e| e.path == playing.track.path)
                        .map(|e| e.id)
                });
                let channels = self
                    .backend
                    .visualization_layout()
                    .map_or(0, |l| l.scope_channels.len());
                let file = playing
                    .track
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let format = text_tag(&playing.metadata, "song_type")
                    .filter(|s| !s.trim().is_empty())
                    .map_or_else(|| playing.entry.format.clone(), |s| s.trim().to_owned());
                serde_json::to_string(&TrackDoc {
                    title: &playing.entry.title,
                    composer: &playing.entry.composer,
                    format: &format,
                    plugin: self.backend.plugin_name(),
                    channels,
                    samples: playing
                        .metadata
                        .samples
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect(),
                    instruments: playing
                        .metadata
                        .instruments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect(),
                    subsongs: playing
                        .metadata
                        .subsongs
                        .iter()
                        .map(|s| SubsongDoc {
                            index: s.index,
                            name: &s.name.text,
                            length_ms: s.length_seconds.map_or(0, retrovert_library::seconds_to_ms),
                        })
                        .collect(),
                    subsong: playing.subsong,
                    year: playing.entry.year,
                    file: &file,
                    path: &playing.track.path,
                    size: playing.entry.size,
                    row,
                })
                .unwrap_or_default()
            }
            None => String::new(),
        };
        lock(&self.docs).track = doc;
        let mut f = lock(&self.front);
        f.track_rev = f.track_rev.wrapping_add(1);
    }

    fn publish_queue(&mut self) {
        let doc = self.playlist.playing_playlist().and_then(|id| {
            let playlist = self.playlist.playlist(id)?;
            let current = self.playlist.current_item_id();
            let items = playlist.items();
            let from = current
                .and_then(|c| items.iter().position(|i| i.id() == c))
                .map_or(0, |i| i + 1);
            let after = &items[from.min(items.len())..];
            let doc = QueueDoc {
                items: after
                    .iter()
                    .map(|item| {
                        let entry = item
                            .payload
                            .row
                            .and_then(|row| self.library.entries.get(row as usize));
                        QueueItemDoc {
                            row: item.payload.row,
                            title: if item.title.is_empty() {
                                item.identity.rsplit('/').next().unwrap_or("")
                            } else {
                                &item.title
                            },
                            composer: &item.artist,
                            format: entry.map_or("", |e| e.format.as_str()),
                            duration_ms: item.duration_ms,
                        }
                    })
                    .collect(),
                total_ms: after.iter().map(|i| u64::from(i.duration_ms)).sum(),
            };
            serde_json::to_string(&doc).ok()
        });
        lock(&self.docs).queue = doc.unwrap_or_default();
        let mut f = lock(&self.front);
        f.queue_rev = f.queue_rev.wrapping_add(1);
    }
}

fn playlist_mode(mode: LoopMode) -> Mode {
    if mode == LoopMode::All {
        Mode::Loop
    } else {
        Mode::Sequential
    }
}

fn text_tag<'a>(metadata: &'a TrackMetadata, key: &str) -> Option<&'a str> {
    metadata.tags.iter().find_map(|tag| match &tag.value {
        MetadataValue::Text(text) if tag.key.eq_ignore_ascii_case(key) => Some(text.text.as_str()),
        MetadataValue::Text(_) | MetadataValue::Number(_) => None,
    })
}

fn clear(ring: &Arc<Mutex<VecDeque<f32>>>) {
    lock(ring).clear();
}

/// Meters decoding to wall-clock time: frames are owed at the sample rate and spent as
/// they are rendered, so the ring stays near its target instead of refilling in bursts.
struct Pacer {
    last: Instant,
    /// Frames earned by elapsed time and not yet rendered.
    owed: f64,
}

impl Pacer {
    fn new() -> Self {
        Self {
            last: Instant::now(),
            owed: 0.0,
        }
    }

    /// Frames the clock allows now, capped at one render so a stall cannot burst.
    fn earn(&mut self) -> u32 {
        let now = Instant::now();
        self.owed += now.duration_since(self.last).as_secs_f64() * f64::from(SAMPLE_RATE);
        self.last = now;
        self.owed = self.owed.min(f64::from(MAX_FRAMES));
        // Truncation is the point: whole frames now, the fraction carries.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = self.owed as u32;
        frames
    }

    fn spend(&mut self, frames: u32) {
        self.owed -= f64::from(frames);
    }
}

fn top_up(
    backend: &mut PlayerBackend,
    ring: &Arc<Mutex<VecDeque<f32>>>,
    pacer: &mut Pacer,
    volume: f32,
) -> Result<(), retrovert_player::BackendError> {
    let buffered = lock(ring).len();
    let earned = pacer.earn();
    if buffered >= BUFFERED_SAMPLES {
        // A full ring owes nothing: time spent idle must not turn into a burst later.
        pacer.spend(earned);
        return Ok(());
    }
    let missing_frames = (BUFFERED_SAMPLES - buffered) / CHANNELS as usize;
    let missing = u32::try_from(missing_frames)
        .unwrap_or(MAX_FRAMES)
        .min(MAX_FRAMES);
    let frames = if buffered < LOW_WATER_SAMPLES {
        missing
    } else {
        missing.min(earned)
    };
    if frames == 0 {
        return Ok(());
    }
    pacer.spend(frames.min(earned));
    let samples = backend.render(frames)?;
    if !samples.is_empty() {
        let mut ring = lock(ring);
        ring.extend(samples.iter().map(|s| s * volume));
    }
    Ok(())
}

/// Copies up to `capacity` points of channel `channel` into `out`; 0 for an unknown channel.
pub fn copy_points(front: &Front, channel: usize, out: &mut [ScopePoint]) -> usize {
    let Some(trace) = front.traces.get(channel) else {
        return 0;
    };
    let src = trace.points();
    let n = src.len().min(out.len());
    out[..n].copy_from_slice(&src[..n]);
    n
}
