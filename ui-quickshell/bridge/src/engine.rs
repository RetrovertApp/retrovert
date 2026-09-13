//! The audio worker that lives inside the shell process.
//!
//! One thread owns the [`PlayerBackend`], the cpal stream and a private set of
//! [`ScopeTrace`]s. Every loop it decodes as much audio as wall-clock time has passed, so
//! the song advances at a steady rate rather than in the device's callback-sized bursts, and
//! the ring in front of the device only absorbs jitter. Sixty times a second it captures a
//! [`VizSnapshot`], refills its traces and copies their points into the shared [`Front`]
//! under a lock held only for that copy. The render thread reads the front the same way,
//! so neither side ever waits on decoding.
//!
//! The pacing matters for the scopes: a decoder exposes the samples it mixed last, so the
//! picture only moves when the decoder does. Rendered in callback-sized bursts it moved
//! about twenty-four times a second; paced, every capture sees fresh samples.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use retrovert_host::session::StreamFormat;
use retrovert_host::visualization::{VisualizationConfig, VizSnapshot};
use retrovert_player::{PlaybackBackend, PlaybackStatus, PlayerBackend};
use retrovert_present::{ScopePoint, ScopeTrace};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const MAX_FRAMES: u32 = 4_096;
/// How much audio the worker keeps decoded ahead of the device.
const BUFFERED_SAMPLES: usize = (SAMPLE_RATE as usize / 5) * CHANNELS as usize;
/// Below this the ring is refilled as fast as possible: at mount, and after a stall.
const LOW_WATER_SAMPLES: usize = BUFFERED_SAMPLES / 4;
const WORKER_SLEEP: Duration = Duration::from_millis(2);
const CAPTURE_INTERVAL: Duration = Duration::from_micros(16_667);
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
    /// The song reached its end.
    Finished = 2,
    /// The last open or render failed; see the error text.
    Error = 3,
}

/// The state shared between the worker and the render thread.
pub struct Front {
    /// One polyline per scope channel of the mounted song.
    pub traces: Vec<ScopeTrace>,
    /// Bumped once per capture, so a reader can tell a new frame from a repeat.
    pub frame: u64,
    /// Playback state.
    pub status: Status,
    /// Delivered position in milliseconds.
    pub position_ms: i64,
    /// The decoder that took the song, or empty.
    pub plugin: String,
    /// The last failure, or empty.
    pub error: String,
}

impl Front {
    fn new() -> Self {
        Self {
            traces: Vec::new(),
            frame: 0,
            status: Status::Idle,
            position_ms: 0,
            plugin: String::new(),
            error: String::new(),
        }
    }
}

/// Locks a poisoned mutex anyway: the data is plain values a panic cannot leave torn.
pub fn lock(front: &Mutex<Front>) -> MutexGuard<'_, Front> {
    match front.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

enum Command {
    Open(PathBuf),
    Stop,
    Quit,
}

/// Handle to the worker thread, owned by the UI session.
pub struct Engine {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    /// The state the render thread reads.
    pub front: Arc<Mutex<Front>>,
}

impl Engine {
    /// Loads every plugin under `plugin_dir` and starts the worker. Setup cadence.
    pub fn start(plugin_dir: &Path) -> Self {
        let front = Arc::new(Mutex::new(Front::new()));
        let (commands, receiver) = channel();
        let paths = plugin_paths(plugin_dir);
        log::info!(
            "{} plugin libraries under {}",
            paths.len(),
            plugin_dir.display()
        );
        let worker_front = Arc::clone(&front);
        let thread = std::thread::Builder::new()
            .name("audio-worker".to_string())
            .spawn(move || worker_main(&paths, &worker_front, &receiver))
            .ok();
        if thread.is_none() {
            lock(&front).error = "could not start the audio worker".to_string();
        }
        Self {
            commands,
            thread,
            front,
        }
    }

    /// Mounts `path` on the worker; the outcome lands in the front's status.
    pub fn open(&self, path: PathBuf) {
        let _ = self.commands.send(Command::Open(path));
    }

    /// Drops the current song.
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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
                let mut ring = match ring.lock() {
                    Ok(ring) => ring,
                    Err(poisoned) => poisoned.into_inner(),
                };
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

fn worker_main(plugins: &[PathBuf], front: &Arc<Mutex<Front>>, commands: &Receiver<Command>) {
    let mut backend = PlayerBackend::new(
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
    let _stream = match start_output(&ring) {
        Ok(stream) => Some(stream),
        Err(message) => {
            log::warn!("no audio output ({message}); decoding continues silently");
            None
        }
    };
    let mut snapshot: Option<VizSnapshot> = None;
    let mut traces: Vec<ScopeTrace> = Vec::new();
    let mut pacer = Pacer::new();
    let mut next_capture = Instant::now();
    let mut frame = 0_u64;

    loop {
        match commands.try_recv() {
            Ok(Command::Open(path)) => {
                clear(&ring);
                match backend.mount(&path, 0) {
                    Ok(()) => {
                        // Setup cadence: the one place the per-song buffers are sized.
                        snapshot = backend
                            .visualization_layout()
                            .and_then(|layout| layout.new_snapshot().ok());
                        let channels = backend
                            .visualization_layout()
                            .map_or(0, |layout| layout.scope_channels.len());
                        traces = (0..channels)
                            .map(|_| ScopeTrace::new(TRACE_CAPACITY))
                            .collect();
                        let mut f = lock(front);
                        f.traces = (0..channels)
                            .map(|_| ScopeTrace::new(TRACE_CAPACITY))
                            .collect();
                        f.status = Status::Playing;
                        f.plugin = backend.plugin_name().to_string();
                        f.error.clear();
                        f.position_ms = 0;
                        log::info!("playing {} via {}", path.display(), f.plugin);
                    }
                    Err(e) => {
                        snapshot = None;
                        let mut f = lock(front);
                        f.status = Status::Error;
                        f.error = format!("could not play {}: {e}", path.display());
                        f.traces.clear();
                        log::error!("{}", f.error);
                    }
                }
            }
            Ok(Command::Stop) => {
                backend.close();
                snapshot = None;
                clear(&ring);
                let mut f = lock(front);
                f.status = Status::Idle;
                f.traces.clear();
            }
            Ok(Command::Quit) | Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        if backend.status() == PlaybackStatus::Finished {
            backend.close();
            snapshot = None;
            lock(front).status = Status::Finished;
        }

        if let Err(e) = top_up(&mut backend, &ring, &mut pacer) {
            backend.close();
            snapshot = None;
            let mut f = lock(front);
            f.status = Status::Error;
            f.error = format!("render failed: {e}");
        }

        let now = Instant::now();
        if now >= next_capture {
            // Keeps the phase, so the sleep granularity does not stretch the interval; a
            // long stall resets rather than replaying missed captures.
            next_capture += CAPTURE_INTERVAL;
            if next_capture + CAPTURE_INTERVAL < now {
                next_capture = now + CAPTURE_INTERVAL;
            }
            if let Some(snapshot) = snapshot.as_mut() {
                frame = publish(&mut backend, snapshot, &mut traces, front, frame);
            }
        }

        std::thread::sleep(WORKER_SLEEP);
    }
}

/// Captures one frame into `traces` and copies it to the front; returns the frame number
/// now published, unchanged if the capture failed.
fn publish(
    backend: &mut PlayerBackend,
    snapshot: &mut VizSnapshot,
    traces: &mut [ScopeTrace],
    front: &Arc<Mutex<Front>>,
    frame: u64,
) -> u64 {
    if backend.capture(snapshot).is_err() {
        return frame;
    }
    for (channel, trace) in traces.iter_mut().enumerate() {
        trace.fill_from(snapshot, channel);
    }
    let frame = frame.wrapping_add(1);
    let position = backend.position_ms().unwrap_or(0);
    // Held for a memcpy per channel and nothing else.
    let mut f = lock(front);
    for (dst, src) in f.traces.iter_mut().zip(traces.iter()) {
        dst.copy_from(src);
    }
    f.frame = frame;
    f.position_ms = position;
    frame
}

fn clear(ring: &Arc<Mutex<VecDeque<f32>>>) {
    match ring.lock() {
        Ok(mut ring) => ring.clear(),
        Err(poisoned) => poisoned.into_inner().clear(),
    }
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
) -> Result<(), retrovert_player::BackendError> {
    let buffered = match ring.lock() {
        Ok(ring) => ring.len(),
        Err(poisoned) => poisoned.into_inner().len(),
    };
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
        let mut ring = match ring.lock() {
            Ok(ring) => ring,
            Err(poisoned) => poisoned.into_inner(),
        };
        ring.extend(samples.iter().copied());
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
