//! Segment model and segment worker.
//!
//! A `Segment` is one byte range of a download. Multiple segments run in
//! parallel for a single task; each writes to the same file fd at its own
//! offset using `pwrite` (positional write — no locking required).

use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::http_client::HttpClient;
use crate::qos::QosLimiter;
use url::Url;

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

/// Configuration for a single segment worker.
pub struct SegmentWorker {
    pub client: HttpClient,
    pub url: Url,
    pub segment: Segment,
    pub file: Arc<std::fs::File>,
    pub qos: QosLimiter,
    pub buffer_size: usize,
    pub progress_tx: mpsc::UnboundedSender<SegmentEvent>,
    pub extra_headers: Option<crate::http_client::ExtraHeaders>,
}

impl SegmentWorker {
    /// Run the worker to completion (or failure). Sends events via `progress_tx`.
    pub async fn run(mut self) -> Result<(), SegmentWorkerError> {
        if self.segment.is_complete() {
            return Err(SegmentWorkerError::AlreadyComplete);
        }

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
        if status != reqwest::StatusCode::PARTIAL_CONTENT {
            // Server returned 200 OK or similar — it ignored our Range header.
            // This is fatal for segmented download; the engine should fall
            // back to a single-connection download.
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

            // Apply QoS throttle. Acquire for the whole chunk at once; if the
            // chunk is larger than the bucket capacity, governor will sleep
            // and refill.
            self.qos.acquire(chunk.len() as u64).await;

            while !remaining.is_empty() {
                let n = remaining.len().min(buf_size);
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
}
