//! Blocking HTTP behind a small trait, so resolution logic is testable offline.
//!
//! Per ADR-0004 all network I/O is synchronous: `ureq` with rustls, no async
//! runtime.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

/// The largest body [`UreqTransport::get_bytes`] reads into memory. Manifests
/// are a few megabytes; ureq's own default of 10 MB is too tight for Bundles.
const MAX_BODY_BYTES: u64 = 512 * 1024 * 1024;

/// How long a streamed download may take in total. Large Bundles on slow
/// links need far more than the 30 seconds allowed for small documents.
const STREAM_TIMEOUT: Duration = Duration::from_mins(30);

/// An HTTP failure. Response bodies are never included, because some of the
/// documents AE3 fetches carry fields that must not be echoed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HttpError {
    #[error("{url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url}: {message}")]
    Transport { url: String, message: String },
    /// The response arrived, but the writer it was streamed into failed, for
    /// example because the disk is full or the download was cancelled.
    #[error("{url}: writing the response failed: {message}")]
    Sink { url: String, message: String },
}

impl HttpError {
    /// Whether trying the same request again may succeed: connection failures,
    /// timeouts, 408, 429 and 5xx. A 403/404 or a failing writer is permanent.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Status { status, .. } => matches!(status, 408 | 429 | 500..=599),
            Self::Transport { .. } => true,
            Self::Sink { .. } => false,
        }
    }
}

/// The body of a Range GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeResponse {
    /// The requested bytes. Shorter than requested when the resource ends
    /// before `end_inclusive`.
    pub bytes: Vec<u8>,
    /// The full length of the resource, when the server reported it (from
    /// `Content-Range`, or `Content-Length` when the server ignored Range).
    pub total_len: Option<u64>,
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

    /// GETs `url` and streams its body into `sink`, returning the number of
    /// bytes written. The default buffers the whole body via
    /// [`Transport::get_bytes`]; real transports stream.
    fn get_to_writer(&self, url: &str, sink: &mut dyn Write) -> Result<u64, HttpError> {
        let bytes = self.get_bytes(url)?;
        sink.write_all(&bytes).map_err(|e| sink_error(url, &e))?;
        Ok(bytes.len() as u64)
    }

    /// GETs bytes `start..=end_inclusive` of `url` with a Range request.
    /// Servers that ignore Range (HTTP 200) are handled by reading only as far
    /// as needed and slicing.
    fn get_range(
        &self,
        url: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<RangeResponse, HttpError>;

    /// Checks whether `url` exists with a 1-byte Range request, without
    /// downloading the body. Returns `Ok(false)` for 403 and 404, which is how
    /// the CDN answers for paths that were never published.
    fn exists(&self, url: &str) -> Result<bool, HttpError>;
}

impl<T: Transport + ?Sized> Transport for &T {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        (**self).get_bytes(url)
    }
    fn get_text(&self, url: &str) -> Result<String, HttpError> {
        (**self).get_text(url)
    }
    fn get_to_writer(&self, url: &str, sink: &mut dyn Write) -> Result<u64, HttpError> {
        (**self).get_to_writer(url, sink)
    }
    fn get_range(&self, url: &str, start: u64, end: u64) -> Result<RangeResponse, HttpError> {
        (**self).get_range(url, start, end)
    }
    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        (**self).exists(url)
    }
}

impl<T: Transport + ?Sized> Transport for Arc<T> {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        (**self).get_bytes(url)
    }
    fn get_text(&self, url: &str) -> Result<String, HttpError> {
        (**self).get_text(url)
    }
    fn get_to_writer(&self, url: &str, sink: &mut dyn Write) -> Result<u64, HttpError> {
        (**self).get_to_writer(url, sink)
    }
    fn get_range(&self, url: &str, start: u64, end: u64) -> Result<RangeResponse, HttpError> {
        (**self).get_range(url, start, end)
    }
    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        (**self).exists(url)
    }
}

/// The production [`Transport`]: a shared `ureq` agent. It is `Send + Sync`,
/// so one instance serves every download thread.
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

fn transport_error(url: &str, err: &dyn std::fmt::Display) -> HttpError {
    HttpError::Transport {
        url: url.to_owned(),
        message: err.to_string(),
    }
}

pub(crate) fn sink_error(url: &str, err: &io::Error) -> HttpError {
    HttpError::Sink {
        url: url.to_owned(),
        message: err.to_string(),
    }
}

fn status_error(url: &str, status: u16) -> HttpError {
    HttpError::Status {
        url: url.to_owned(),
        status,
    }
}

/// Copies `reader` into `sink`, telling read failures (the network) apart
/// from write failures (the sink).
fn copy_body(url: &str, reader: &mut dyn Read, sink: &mut dyn Write) -> Result<u64, HttpError> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(transport_error(url, &e)),
        };
        sink.write_all(&buf[..n]).map_err(|e| sink_error(url, &e))?;
        total += n as u64;
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
            return Err(status_error(url, status.as_u16()));
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|e| transport_error(url, &e))
    }

    fn get_to_writer(&self, url: &str, sink: &mut dyn Write) -> Result<u64, HttpError> {
        let mut response = self
            .agent
            .get(url)
            .config()
            .timeout_global(Some(STREAM_TIMEOUT))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build()
            .call()
            .map_err(|e| transport_error(url, &e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(url, status.as_u16()));
        }
        copy_body(url, &mut response.body_mut().as_reader(), sink)
    }

    fn get_range(
        &self,
        url: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<RangeResponse, HttpError> {
        if end_inclusive < start {
            return Err(transport_error(
                url,
                &format_args!("invalid byte range {start}-{end_inclusive}"),
            ));
        }
        let wanted = (end_inclusive - start).saturating_add(1);
        if wanted > MAX_BODY_BYTES {
            return Err(transport_error(
                url,
                &format_args!("byte range of {wanted} bytes is too large to hold in memory"),
            ));
        }
        let mut response = self
            .agent
            .get(url)
            .header("Range", format!("bytes={start}-{end_inclusive}"))
            .call()
            .map_err(|e| transport_error(url, &e))?;
        match response.status().as_u16() {
            206 => {
                let (range_start, total_len) = response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range)
                    .ok_or_else(|| {
                        transport_error(url, &"206 response without a valid Content-Range")
                    })?;
                if range_start != start {
                    return Err(transport_error(
                        url,
                        &format_args!("asked for byte {start} but the server sent {range_start}"),
                    ));
                }
                let mut bytes = Vec::new();
                response
                    .body_mut()
                    .as_reader()
                    .take(wanted)
                    .read_to_end(&mut bytes)
                    .map_err(|e| transport_error(url, &e))?;
                Ok(RangeResponse { bytes, total_len })
            }
            200 => {
                // The server ignored Range: read only up to the window's end
                // and drop the rest of the connection unread.
                let total_len = response.body().content_length();
                let mut bytes = Vec::new();
                response
                    .body_mut()
                    .as_reader()
                    .take(end_inclusive.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|e| transport_error(url, &e))?;
                let skip = usize::try_from(start)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                bytes.drain(..skip);
                Ok(RangeResponse { bytes, total_len })
            }
            status => Err(status_error(url, status)),
        }
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
            status => Err(status_error(url, status)),
        }
    }
}

/// Parses a `Content-Range: bytes <start>-<end>/<total|*>` header into the
/// first byte's offset and the total length, if known.
#[must_use]
pub fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let (first, last) = range.trim().split_once('-')?;
    let first: u64 = first.trim().parse().ok()?;
    let last: u64 = last.trim().parse().ok()?;
    if last < first {
        return None;
    }
    let total = match total.trim() {
        "*" => None,
        n => Some(n.parse().ok()?),
    };
    Some((first, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_range() {
        assert_eq!(parse_content_range("bytes 0-0/1234"), Some((0, Some(1234))));
        assert_eq!(parse_content_range("bytes 10-19/*"), Some((10, None)));
        assert_eq!(
            parse_content_range(" bytes  5 - 9 / 10 "),
            Some((5, Some(10)))
        );
        for bad in [
            "",
            "bytes */100",
            "items 0-1/2",
            "bytes 9-5/10",
            "bytes 0-1",
        ] {
            assert_eq!(parse_content_range(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn transient_errors() {
        let status = |s| status_error("u", s);
        assert!(status(503).is_transient());
        assert!(status(429).is_transient());
        assert!(!status(404).is_transient());
        assert!(!status(403).is_transient());
        assert!(transport_error("u", &"reset").is_transient());
        assert!(!sink_error("u", &io::Error::other("disk full")).is_transient());
    }

    #[test]
    fn copy_body_separates_read_and_write_failures() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut data: &[u8] = b"hello";
        let err = copy_body("u", &mut data, &mut FailingWriter).unwrap_err();
        assert!(matches!(err, HttpError::Sink { .. }));

        let mut data: &[u8] = b"hello";
        let mut out = Vec::new();
        assert_eq!(copy_body("u", &mut data, &mut out), Ok(5));
        assert_eq!(out, b"hello");
    }
}
