//! The HTTP transport the rest of the crate is built on.
//!
//! Two ways to fetch. [`Transport::download`] is for artifacts: cached under
//! the digest they are expected to have, resumable across interruptions, and
//! pausable. [`Transport::get_bounded`] is for update metadata, which is small,
//! mutable, and must never be cached or resumed.

mod cache;
mod digest;
mod download;
mod error;
mod meta;

use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use ureq::Agent;

pub use cache::Cache;
pub use digest::ArtifactDigest;
pub use download::{Chunk, Download, Failure, Snapshot, Status};
pub use error::{Error, Result};

const USER_AGENT: &str = concat!("retrovert-updater/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A streaming artifact GET gets no global timeout — a large file on a slow
/// link is not an error. Everything else is small enough to bound.
const SHORT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: u32 = 10;

/// The agents every request from one transport goes through.
///
/// Each timeout profile exists in a plaintext-tolerant and a TLS-only form,
/// chosen per request by the URL's own scheme: a request that starts on
/// `https` must not be walked onto plaintext by a redirect, while an
/// explicitly plaintext URL — a test fixture, an opted-in local mirror — keeps
/// working.
struct Agents {
    streaming: Agent,
    streaming_tls: Agent,
    short: Agent,
    /// Refuses a plaintext hop outright, redirects included.
    tls_only: Agent,
}

impl Agents {
    fn new() -> Self {
        Self {
            streaming: agent(None, false),
            streaming_tls: agent(None, true),
            short: agent(Some(SHORT_TIMEOUT), false),
            tls_only: agent(Some(SHORT_TIMEOUT), true),
        }
    }

    /// The artifact-streaming agent for `url`, holding it to TLS when it
    /// starts there.
    fn streaming_for(&self, url: &str) -> &Agent {
        if is_tls(url) {
            &self.streaming_tls
        } else {
            &self.streaming
        }
    }

    /// The bounded-fetch agent for `url`, holding it to TLS when it starts
    /// there.
    fn short_for(&self, url: &str) -> &Agent {
        if is_tls(url) {
            &self.tls_only
        } else {
            &self.short
        }
    }
}

fn is_tls(url: &str) -> bool {
    url.starts_with("https://")
}

fn agent(global_timeout: Option<Duration>, tls_only: bool) -> Agent {
    ureq::Agent::config_builder()
        .max_redirects(MAX_REDIRECTS)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(global_timeout)
        .https_only(tls_only)
        // Status is inspected here, so a 4xx is a response rather than an error.
        .http_status_as_error(false)
        .build()
        .into()
}

/// A response small enough to hold in memory.
#[derive(Debug, Clone)]
pub struct BoundedResponse {
    /// The response body.
    pub body: Vec<u8>,
    /// The `Date` header verbatim, when the server sent one. The update check
    /// takes its verification time from here rather than the local clock.
    pub date: Option<String>,
}

/// HTTP transport over an instance-owned cache directory.
pub struct Transport {
    cache: Cache,
    agents: Agents,
}

impl Transport {
    /// A transport caching under `cache_dir`.
    #[must_use]
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache: Cache::new(cache_dir),
            agents: Agents::new(),
        }
    }

    /// The cache this transport downloads into.
    #[must_use]
    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Start a transfer into the cache entry for `digest`.
    ///
    /// A complete entry is served from disk without a request.
    pub fn download(
        &self,
        url: &str,
        digest: &ArtifactDigest,
        allow_resume: bool,
    ) -> Result<Download> {
        Download::start(&self.agents, self.cache.path_for(digest), url, allow_resume)
    }

    /// Fetch at most `limit` bytes into memory, cache-busted and unresumable.
    ///
    /// Fails with [`Error::TooLarge`] rather than reading a body past `limit`.
    /// An `https` URL stays on TLS across redirects.
    pub fn get_bounded(&self, url: &str, limit: usize) -> Result<BoundedResponse> {
        bounded(self.agents.short_for(url), url, limit)
    }

    /// As [`Transport::get_bounded`], but refusing to speak plaintext to
    /// anyone, including whoever a redirect points at.
    ///
    /// For callers that trust the answer on the strength of the transport that
    /// carried it rather than on a signature over it.
    pub fn get_bounded_over_tls(&self, url: &str, limit: usize) -> Result<BoundedResponse> {
        bounded(&self.agents.tls_only, url, limit)
    }
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

fn bounded(agent: &Agent, url: &str, limit: usize) -> Result<BoundedResponse> {
    let response = agent
        .get(url)
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .call()
        .map_err(|e| Error::request(url, e))?;

    let status = response.status().as_u16();
    if !is_success(status) {
        return Err(Error::Status {
            url: url.to_string(),
            status,
        });
    }
    let date = header(&response, "date");
    let body = read_bounded(&mut response.into_body().into_reader(), limit)
        .map_err(|e| Error::request(url, e))?
        .ok_or_else(|| Error::TooLarge {
            url: url.to_string(),
            limit,
        })?;
    Ok(BoundedResponse { body, date })
}

fn url_size(agent: &Agent, url: &str) -> Option<u64> {
    let response = agent.head(url).call().ok()?;
    if !is_success(response.status().as_u16()) {
        return None;
    }
    header(&response, "content-length")?.parse().ok()
}

fn header<T>(response: &ureq::http::Response<T>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_string)
}

/// Read the whole source, or `None` if it holds more than `limit` bytes.
fn read_bounded(reader: &mut impl Read, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut body = Vec::new();
    reader.take(cap).read_to_end(&mut body)?;
    Ok((body.len() <= limit).then_some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with(name: &str, value: &str) -> ureq::http::Response<()> {
        ureq::http::Response::builder()
            .header(name, value)
            .body(())
            .unwrap()
    }

    #[test]
    fn a_header_is_read_by_any_case_of_its_name() {
        let response = response_with("Date", "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(
            header(&response, "date").as_deref(),
            Some("Sun, 06 Nov 1994 08:49:37 GMT")
        );
        assert_eq!(header(&response, "etag"), None);
    }

    #[test]
    fn an_https_url_is_held_to_tls_and_a_plaintext_one_is_not() {
        assert!(is_tls("https://host/artifact"));
        assert!(!is_tls("http://host/artifact"));
        // Scheme selection is what keeps an https transfer off plaintext
        // redirect targets; anything else falls to the permissive agents and
        // fails on its own merits.
        assert!(!is_tls("ftp://host/artifact"));
    }

    #[test]
    fn a_body_at_the_limit_is_read_and_one_past_it_is_not() {
        assert_eq!(
            read_bounded(&mut b"12345".as_slice(), 5).unwrap(),
            Some(b"12345".to_vec())
        );
        assert_eq!(read_bounded(&mut b"123456".as_slice(), 5).unwrap(), None);
        assert_eq!(
            read_bounded(&mut b"".as_slice(), 0).unwrap(),
            Some(Vec::new())
        );
        assert_eq!(read_bounded(&mut b"1".as_slice(), 0).unwrap(), None);
    }
}
