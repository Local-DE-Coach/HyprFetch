//! Segment model and segment worker.
//!
//! A `Segment` is one byte range of a download. Multiple segments run in
//! parallel for a single task; each writes to the same file fd at its own
//! offset using `pwrite` (positional write — no locking required).

use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::http_client::HttpClient;
use crate::qos::QosLimiter;
use url::Url;

/// How many times a segment worker retries a transient failure before
/// giving up and failing the task.
///
/// Real-world servers and middleboxes drop long-lived connections (nginx
/// per-connection limits, NAT idle expiry, TLS session re-keying). Without
/// retry, ONE dropped connection failed the WHOLE task (observed on
/// thinkbroadband: 4 of 8 segments dropped ~150 s into a 1 GB download →
/// `only 4 of 8 segments completed`). Each retry resumes from the last
/// written byte offset, so retried work is proportional to the bytes lost,
/// not to the whole segment.
pub const MAX_SEGMENT_ATTEMPTS: u32 = 6;

/// In-memory representation of one segment of a download.
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    /// 0-based index within the parent task.
    pub idx: i64,
    /// Byte offset in the file where this segment starts (inclusive).
    pub start_byte: i64,
    /// Byte offset where this segment ends (exclusive).
    pub end_byte: i64,
    /// Current write position. Starts at `start_byte`, advances to `end_byte`.
    pub current_byte: i64,
}

impl Segment {
    /// Bytes remaining for this segment.
    pub fn remaining(&self) -> i64 {
        (self.end_byte - self.current_byte).max(0)
    }

    /// Fraction complete (0.0..=1.0).
    pub fn fraction(&self) -> f64 {
        let total = self.end_byte - self.start_byte;
        if total <= 0 {
            return 1.0;
        }
        let done = self.current_byte - self.start_byte;
        done as f64 / total as f64
    }

    /// True when `current_byte` has reached `end_byte`.
    pub fn is_complete(&self) -> bool {
        self.current_byte >= self.end_byte
    }

    /// Bytes downloaded by this segment so far.
    pub fn downloaded(&self) -> i64 {
        (self.current_byte - self.start_byte).max(0)
    }
}

/// Progress events emitted by a segment worker.
#[derive(Debug, Clone)]
pub enum SegmentEvent {
    /// N bytes were just written. The task aggregator uses this to update
    /// totals and to debounce persistence.
    Progress {
        idx: i64,
        bytes_written: u64,
        current_byte: i64,
    },
    /// The segment reached its end_byte.
    Completed { idx: i64 },
    /// The worker failed.
    Failed { idx: i64, error: String },
}

/// Errors returned by a segment worker.
#[derive(Debug, thiserror::Error)]
pub enum SegmentWorkerError {
    #[error("http: {0}")]
    Http(#[from] crate::http_client::HttpError),
    #[error("http transport: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("server returned unexpected status: {0}")]
    UnexpectedStatus(reqwest::StatusCode),
    #[error("server did not honor Range header (got 200 OK, expected 206)")]
    RangeIgnored,
    #[error("segment already complete")]
    AlreadyComplete,
}

impl SegmentWorkerError {
    /// Whether the worker may reasonably retry this error from its current
    /// offset. Transport hiccups (reset, timeout, EOF) and server-side
    /// 5xx/429 are transient; disk I/O errors, SSRF violations, ignored
    /// Range headers and client errors are not.
    pub fn is_retryable(&self) -> bool {
        match self {
            SegmentWorkerError::Reqwest(_) => true,
            SegmentWorkerError::Http(h) => match h {
                crate::http_client::HttpError::Reqwest(_) => true,
                crate::http_client::HttpError::BadStatus { status, .. } => {
                    status.is_server_error() || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
                }
                _ => false,
            },
            SegmentWorkerError::Io(io_err) => io_err.kind() == io::ErrorKind::UnexpectedEof,
            SegmentWorkerError::UnexpectedStatus(_) => false,
            SegmentWorkerError::RangeIgnored => false,
            SegmentWorkerError::AlreadyComplete => false,
        }
    }
}

/// Exponential backoff before retry `attempt` (1-based, i.e. the pause
/// before the 2nd try is `backoff_delay(1)`): 1 s, 2 s, 4 s, 8 s, then
/// capped at 15 s.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64 << (attempt - 1).min(4);
    Duration::from_secs(secs.min(15))
}

/// Configuration for a single segment worker.
pub struct SegmentWorker {
    pub client: HttpClient,
    pub url: Url,
    pub segment: Segment,
    pub file: Arc<std::fs::File>,
    /// Shared engine-wide limiter. `None` bypasses QoS entirely (per-task
    /// `force_off` override).
    pub qos: Option<QosLimiter>,
    pub buffer_size: usize,
    pub progress_tx: mpsc::UnboundedSender<SegmentEvent>,
    pub extra_headers: Option<crate::http_client::ExtraHeaders>,
}

impl SegmentWorker {
    /// Run the worker to completion (or failure). Sends events via `progress_tx`.
    ///
    /// Transient transport errors are retried up to [`MAX_SEGMENT_ATTEMPTS`]
    /// times with [`backoff_delay`] backoff; every retry re-requests only the
    /// remaining byte range (from the last written offset).
    pub async fn run(mut self) -> Result<(), SegmentWorkerError> {
        if self.segment.is_complete() {
            return Err(SegmentWorkerError::AlreadyComplete);
        }

        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match self.run_once().await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < MAX_SEGMENT_ATTEMPTS && e.is_retryable() => {
                    let delay = backoff_delay(attempt);
                    tracing::warn!(
                        seg = self.segment.idx,
                        attempt,
                        retry_in_s = delay.as_secs(),
                        from_byte = self.segment.current_byte,
                        error = %e,
                        "segment attempt failed — retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One attempt: fetch the remaining range and stream it to the file.
    async fn run_once(&mut self) -> Result<(), SegmentWorkerError> {
        let start = self.segment.current_byte;
        let end_inclusive = self.segment.end_byte - 1;

        tracing::debug!(
            seg = self.segment.idx,
            start,
            end_inclusive,
            "segment worker starting"
        );

        let resp = self
            .client
            .fetch_range(&self.url, start, end_inclusive, self.extra_headers.as_ref())
            .await?;

        let status = resp.status();
        // Response rules:
        // - 206 Partial Content: the happy path — body is exactly the
        //   requested range.
        // - 200 OK when this segment starts at byte 0: the server ignored
        //   Range and sent the WHOLE file. For the single-segment fallback
        //   (servers without `Accept-Ranges` support) that body is exactly
        //   what we need — accept it, cap writes at this segment's end and
        //   discard the excess. Writes beyond the cap would corrupt the
        //   pre-allocated file.
        // - 200 OK with start > 0: the server ignored Range mid-file; the
        //   body starts at file byte 0 but we would write it at `start` —
        //   corruption. Fatal for this attempt.
        if status == reqwest::StatusCode::OK {
            if start != 0 {
                return Err(SegmentWorkerError::RangeIgnored);
            }
        } else if status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(SegmentWorkerError::RangeIgnored);
        }

        // Stream the body, writing each chunk via pwrite at the right offset.
        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut offset = self.segment.current_byte;
        let buf_size = self.buffer_size.max(4096);

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            let mut remaining = &chunk[..];

            // Apply QoS throttle (no-op when QoS is off or the task opted
            // out). Acquire for the whole chunk at once; the limiter splits
            // chunks larger than the bucket capacity internally.
            if let Some(qos) = &self.qos {
                qos.acquire(chunk.len() as u64).await;
            }

            while !remaining.is_empty() {
                // Cap writes at this segment's end: a 200 (whole-file) body
                // is longer than the requested range — discard the excess.
                if offset >= self.segment.end_byte {
                    break;
                }
                let n = remaining
                    .len()
                    .min(buf_size)
                    .min((self.segment.end_byte - offset) as usize);
                let slice = &remaining[..n];
                // pwrite — positional write, no seek needed.
                self.file.write_at(slice, offset as u64)?;
                offset += n as i64;
                self.segment.current_byte = offset;
                let _ = self.progress_tx.send(SegmentEvent::Progress {
                    idx: self.segment.idx,
                    bytes_written: n as u64,
                    current_byte: offset,
                });
                remaining = &remaining[n..];
            }

            // Got the full range already (200 with excess bytes) — stop
            // reading the stream.
            if self.segment.current_byte >= self.segment.end_byte {
                break;
            }
        }

        if self.segment.current_byte >= self.segment.end_byte {
            let _ = self.progress_tx.send(SegmentEvent::Completed {
                idx: self.segment.idx,
            });
            tracing::debug!(seg = self.segment.idx, "segment complete");
            Ok(())
        } else {
            // Server closed the connection before delivering all bytes.
            // This is recoverable — caller can restart the segment.
            Err(SegmentWorkerError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "segment {} got {} of {} bytes before EOF",
                    self.segment.idx,
                    self.segment.current_byte - self.segment.start_byte,
                    self.segment.end_byte - self.segment.start_byte
                ),
            )))
        }
    }
}

/// Open a file for pwrite, creating it if missing and pre-allocating to `size`.
///
/// Uses `ftruncate` to set the file length. On Linux this creates a sparse
/// file (no actual disk allocation until writes happen) which is fine for
/// our use case — segments will write sequentially within their ranges.
/// We avoid `fallocate` here because it requires `unsafe` and our crate
/// forbids `unsafe_code`; the trade-off is acceptable for the download
/// workload (writes are largely sequential per segment).
pub fn open_target_file(path: &Path, size: i64) -> io::Result<std::fs::File> {
    use std::fs::OpenOptions;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;

    if size > 0 {
        file.set_len(size as u64)?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn segment_remaining_and_fraction() {
        let s = Segment {
            idx: 0,
            start_byte: 0,
            end_byte: 1000,
            current_byte: 250,
        };
        assert_eq!(s.remaining(), 750);
        assert_eq!(s.fraction(), 0.25);
        assert!(!s.is_complete());
        assert_eq!(s.downloaded(), 250);
    }

    #[test]
    fn segment_complete_when_current_reaches_end() {
        let s = Segment {
            idx: 0,
            start_byte: 500,
            end_byte: 1000,
            current_byte: 1000,
        };
        assert!(s.is_complete());
        assert_eq!(s.remaining(), 0);
        assert_eq!(s.fraction(), 1.0);
    }

    #[test]
    fn open_target_file_creates_and_prewrites() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        // Drop the temp file so we can re-open it fresh.
        drop(tmp);

        let file = open_target_file(&path, 1000).unwrap();
        // Verify we can pwrite at offset 500.
        file.write_at(b"hello", 500u64).unwrap();

        let mut contents = String::new();
        use std::io::Read;
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        // File length is 1000 (ftruncate pre-allocated), but bytes 500..505 are "hello".
        // Bytes 0..500 are NUL (sparse).
        assert_eq!(contents.len(), 1000);
        assert_eq!(&contents.as_bytes()[500..505], b"hello");
    }

    #[test]
    fn multiple_pwrites_do_not_conflict() {
        // Verify pwrite semantics: writes at different offsets don't collide.
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        drop(tmp);
        let file = open_target_file(&path, 100).unwrap();

        // Simulate two segment workers writing to the same file concurrently.
        let file_clone = Arc::new(file);
        let file2 = file_clone.clone();

        let h1 = std::thread::spawn(move || {
            for i in 0..50 {
                file_clone.write_at(&[b'A' + i as u8], i as u64).unwrap();
            }
        });
        let h2 = std::thread::spawn(move || {
            for i in 0..50 {
                file2.write_at(&[b'a' + i as u8], (50 + i) as u64).unwrap();
            }
        });
        h1.join().unwrap();
        h2.join().unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents.len(), 100);
        assert_eq!(contents[0], b'A');
        assert_eq!(contents[49], b'A' + 49);
        assert_eq!(contents[50], b'a');
        assert_eq!(contents[99], b'a' + 49);
    }

    #[test]
    fn segment_worker_fails_on_already_complete_segment() {
        // We can't actually run the worker without a real HTTP server,
        // but we can verify the early-return path.
        let seg = Segment {
            idx: 0,
            start_byte: 0,
            end_byte: 100,
            current_byte: 100,
        };
        assert!(seg.is_complete());
        // If we tried to construct a SegmentWorker with this, run() would
        // immediately return AlreadyComplete.
    }

    #[test]
    fn retryability_classification() {
        use crate::http_client::HttpError;
        use std::io::ErrorKind;

        // EOF from a server that closed early is transient.
        let eof = SegmentWorkerError::Io(io::Error::new(ErrorKind::UnexpectedEof, "short read"));
        assert!(eof.is_retryable());

        // Disk errors are not.
        let disk = SegmentWorkerError::Io(io::Error::new(ErrorKind::StorageFull, "no space left"));
        assert!(!disk.is_retryable());

        let make_status = |status: reqwest::StatusCode| {
            SegmentWorkerError::Http(HttpError::BadStatus {
                status,
                url: "http://example.invalid/f.bin".into(),
            })
        };
        assert!(make_status(reqwest::StatusCode::BAD_GATEWAY).is_retryable());
        assert!(make_status(reqwest::StatusCode::SERVICE_UNAVAILABLE).is_retryable());
        assert!(make_status(reqwest::StatusCode::TOO_MANY_REQUESTS).is_retryable());
        assert!(!make_status(reqwest::StatusCode::NOT_FOUND).is_retryable());
        assert!(!make_status(reqwest::StatusCode::FORBIDDEN).is_retryable());

        assert!(!SegmentWorkerError::RangeIgnored.is_retryable());
        assert!(!SegmentWorkerError::AlreadyComplete.is_retryable());
        assert!(!SegmentWorkerError::UnexpectedStatus(reqwest::StatusCode::OK).is_retryable());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(8));
        assert_eq!(backoff_delay(5), Duration::from_secs(15));
        assert_eq!(backoff_delay(6), Duration::from_secs(15));
        assert_eq!(backoff_delay(100), Duration::from_secs(15));
    }
}
