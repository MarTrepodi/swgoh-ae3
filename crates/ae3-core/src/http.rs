//! Blocking HTTP behind a small trait, so resolution logic is testable offline.
//!
//! Per ADR-0004 all network I/O is synchronous: `ureq` with rustls, no async
//! runtime.

use std::time::Duration;

use thiserror::Error;

/// The largest body [`UreqTransport::get_bytes`] reads into memory. Manifests
/// are a few megabytes; ureq's own default of 10 MB is too tight for Bundles.
const MAX_BODY_BYTES: u64 = 512 * 1024 * 1024;

/// An HTTP failure. Response bodies are never included, because some of the
/// documents AE3 fetches carry fields that must not be echoed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HttpError {
    #[error("{url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url}: {message}")]
    Transport { url: String, message: String },
}

/// The HTTP operations AE3 needs.
pub trait Transport {
    /// GETs `url` and returns its body; any non-2xx status is an error.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError>;

    /// GETs `url` and returns its body as UTF-8 text.
    fn get_text(&self, url: &str) -> Result<String, HttpError> {
        String::from_utf8(self.get_bytes(url)?).map_err(|_| HttpError::Transport {
            url: url.to_owned(),
            message: "response is not valid UTF-8".to_owned(),
        })
    }

    /// Checks whether `url` exists with a 1-byte Range request, without
    /// downloading the body. Returns `Ok(false)` for 403 and 404, which is how
    /// the CDN answers for paths that were never published.
    fn exists(&self, url: &str) -> Result<bool, HttpError>;
}

/// The production [`Transport`]: a shared `ureq` agent.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    #[must_use]
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("ae3/", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn transport_error(url: &str, err: &ureq::Error) -> HttpError {
    HttpError::Transport {
        url: url.to_owned(),
        message: err.to_string(),
    }
}

impl Transport for UreqTransport {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| transport_error(url, &e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(HttpError::Status {
                url: url.to_owned(),
                status: status.as_u16(),
            });
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|e| transport_error(url, &e))
    }

    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        // The body is dropped unread: a server that ignores Range would
        // otherwise stream the whole file.
        let response = self
            .agent
            .get(url)
            .header("Range", "bytes=0-0")
            .call()
            .map_err(|e| transport_error(url, &e))?;
        match response.status().as_u16() {
            200 | 206 => Ok(true),
            403 | 404 => Ok(false),
            status => Err(HttpError::Status {
                url: url.to_owned(),
                status,
            }),
        }
    }
}
