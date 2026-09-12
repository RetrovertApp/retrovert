//! An in-process HTTP server for the live-transfer suite.
//!
//! Hermetic by construction: it binds an ephemeral loopback port, serves
//! fixture bodies held in memory, and records every request so a test can
//! assert what the transport actually asked for — including whether a resumed
//! transfer sent a `Range` and got a 206 back.
//!
//! A body can be served in throttled pieces, which is what gives a test time to
//! catch a transfer mid-flight and pause or preempt it.

// Each test binary compiles this module separately and uses a different subset.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A body the server serves, and how fast.
#[derive(Clone)]
pub struct Body {
    bytes: Arc<Vec<u8>>,
    piece: usize,
    delay: Duration,
    header_delay: Duration,
    etag: String,
}

impl Body {
    /// A body written in one go.
    pub fn instant(bytes: Vec<u8>, etag: &str) -> Self {
        Self {
            piece: bytes.len().max(1),
            bytes: Arc::new(bytes),
            delay: Duration::ZERO,
            header_delay: Duration::ZERO,
            etag: etag.to_string(),
        }
    }

    /// A body written `piece` bytes at a time, pausing `delay` between pieces.
    pub fn throttled(bytes: Vec<u8>, etag: &str, piece: usize, delay: Duration) -> Self {
        Self {
            bytes: Arc::new(bytes),
            piece: piece.max(1),
            delay,
            header_delay: Duration::ZERO,
            etag: etag.to_string(),
        }
    }

    /// Hold back the response head for `header_delay` before serving as usual.
    ///
    /// A transfer is claimed by a worker — and so reads as in flight — before
    /// the transport has a response to hand back. This widens that window on
    /// demand, so a test can land a request against a transfer the queue has
    /// no handle for yet.
    pub fn stalling(mut self, header_delay: Duration) -> Self {
        self.header_delay = header_delay;
        self
    }
}

/// One request the server answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The request method.
    pub method: String,
    /// The path requested.
    pub path: String,
    /// The first byte a `Range` header asked for, when it sent one.
    pub range_from: Option<u64>,
    /// The status the server answered with.
    pub status: u16,
}

struct State {
    routes: HashMap<String, Body>,
    log: Mutex<Vec<Record>>,
    running: AtomicBool,
}

/// A loopback HTTP server serving a fixed set of bodies.
pub struct FixtureServer {
    addr: SocketAddr,
    state: Arc<State>,
    accept: Option<JoinHandle<()>>,
}

impl FixtureServer {
    /// Start a server serving `routes`, keyed by request path.
    pub fn start(routes: HashMap<String, Body>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port to bind");
        let addr = listener.local_addr().expect("the bound address");
        let state = Arc::new(State {
            routes,
            log: Mutex::new(Vec::new()),
            running: AtomicBool::new(true),
        });
        let accept_state = Arc::clone(&state);
        let accept = thread::spawn(move || accept_loop(&listener, &accept_state));
        Self {
            addr,
            state,
            accept: Some(accept),
        }
    }

    /// The absolute URL of `path` on this server.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// Every request answered so far, oldest first.
    pub fn records(&self) -> Vec<Record> {
        self.state.log.lock().expect("an unpoisoned log").clone()
    }

    /// How many requests have been answered for `path`.
    pub fn request_count(&self, path: &str) -> usize {
        self.records().iter().filter(|r| r.path == path).count()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.state.running.store(false, Ordering::Release);
        // Unblock the accept call so the loop can see the flag.
        if let Ok(stream) = TcpStream::connect(self.addr) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

fn accept_loop(listener: &TcpListener, state: &Arc<State>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    while let Ok((stream, _)) = listener.accept() {
        if !state.running.load(Ordering::Acquire) {
            break;
        }
        let connection_state = Arc::clone(state);
        connections.push(thread::spawn(move || serve(stream, &connection_state)));
        connections.retain(|handle| !handle.is_finished());
    }
    for handle in connections {
        let _ = handle.join();
    }
}

fn serve(mut stream: TcpStream, state: &Arc<State>) {
    let _ = stream.set_nodelay(true);
    // A client that stops reading must not wedge this thread forever.
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Some(request) = read_request(&stream) else {
        return;
    };

    let Some(body) = state.routes.get(route_of(&request.path)) else {
        record(state, &request, 404);
        let _ = stream.write_all(head(404, "Not Found", 0, None, None).as_bytes());
        return;
    };

    // Before anything is written back, so the client is still waiting on the
    // response head rather than on the body.
    if !body.header_delay.is_zero() {
        thread::sleep(body.header_delay);
    }

    let total = u64::try_from(body.bytes.len()).expect("a fixture body fits in a u64");
    // A stale `If-Range` means the partial bytes are no longer valid, so the
    // whole body is served instead of a splice.
    let outdated_range = request
        .if_range
        .as_ref()
        .is_some_and(|tag| tag != &body.etag);
    let from = match request.range_from {
        Some(from) if !outdated_range && from <= total => from,
        _ => 0,
    };

    if request.method == "HEAD" {
        record(state, &request, 200);
        let _ = stream.write_all(head(200, "OK", total, Some(&body.etag), None).as_bytes());
        return;
    }

    let (status, reason, content_range) = if request.range_from.is_some() && from > 0 {
        (
            206,
            "Partial Content",
            Some(format!("bytes {from}-{}/{total}", total.saturating_sub(1))),
        )
    } else {
        (200, "OK", None)
    };
    record(state, &request, status);

    let rest = &body.bytes[usize::try_from(from).unwrap_or(0)..];
    let header = head(
        status,
        reason,
        u64::try_from(rest.len()).expect("a fixture body fits in a u64"),
        Some(&body.etag),
        content_range.as_deref(),
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    for piece in rest.chunks(body.piece) {
        if stream.write_all(piece).is_err() || stream.flush().is_err() {
            return;
        }
        if !body.delay.is_zero() {
            thread::sleep(body.delay);
        }
    }
}

struct Request {
    method: String,
    path: String,
    range_from: Option<u64>,
    if_range: Option<String>,
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let mut parts = start.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut range_from = None;
    let mut if_range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("range") {
            range_from = parse_range_from(value);
        } else if name.eq_ignore_ascii_case("if-range") {
            if_range = Some(value.to_string());
        }
    }
    Some(Request {
        method,
        path,
        range_from,
        if_range,
    })
}

/// The route a request target names, with any query dropped: a cache-busting
/// key varies per request and would otherwise miss every route.
fn route_of(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

/// The first byte of a `bytes=N-` range, the only form the transport sends.
fn parse_range_from(value: &str) -> Option<u64> {
    value
        .strip_prefix("bytes=")?
        .split('-')
        .next()?
        .parse()
        .ok()
}

fn head(
    status: u16,
    reason: &str,
    content_length: u64,
    etag: Option<&str>,
    content_range: Option<&str>,
) -> String {
    let etag = etag.map_or(String::new(), |etag| format!("ETag: {etag}\r\n"));
    let content_range =
        content_range.map_or(String::new(), |range| format!("Content-Range: {range}\r\n"));
    format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {content_length}\r\n\
         Accept-Ranges: bytes\r\n\
         Connection: close\r\n\
         {etag}{content_range}\r\n"
    )
}

fn record(state: &Arc<State>, request: &Request, status: u16) {
    state.log.lock().expect("an unpoisoned log").push(Record {
        method: request.method.clone(),
        path: request.path.clone(),
        range_from: request.range_from,
        status,
    });
}
