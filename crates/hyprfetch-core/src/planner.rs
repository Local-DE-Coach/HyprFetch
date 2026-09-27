//! Split a byte range into N segments for parallel download.

use crate::Segment;

/// Split `total_bytes` into `n` contiguous, non-overlapping segments.
///
/// Each segment covers `[start, end)` (end exclusive). The last segment
/// absorbs any remainder so the segments exactly cover the full range.
///
/// Returns an empty Vec if `n == 0` or `total_bytes == 0`.
pub fn split(total_bytes: i64, n: i64) -> Vec<Segment> {
    if n <= 0 || total_bytes <= 0 {
        return Vec::new();
    }
    let n = n.min(total_bytes); // can't have more segments than bytes
    let chunk = total_bytes / n;
    let mut segments = Vec::with_capacity(n as usize);
    let mut offset: i64 = 0;
    for idx in 0..n {
        let size = if idx == n - 1 {
            // last segment absorbs remainder
            total_bytes - offset
        } else {
            chunk
        };
        let start = offset;
        let end = offset + size; // exclusive
        segments.push(Segment {
            idx,
            start_byte: start,
            end_byte: end, // exclusive
            current_byte: start,
        });
        offset = end;
    }
    debug_assert_eq!(offset, total_bytes);
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_evenly() {
        let s = split(1000, 4);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].start_byte, 0);
        assert_eq!(s[0].end_byte, 250);
        assert_eq!(s[1].start_byte, 250);
        assert_eq!(s[1].end_byte, 500);
        assert_eq!(s[3].start_byte, 750);
        assert_eq!(s[3].end_byte, 1000);
        // No overlap
        for i in 0..3 {
            assert_eq!(s[i].end_byte, s[i + 1].start_byte);
        }
        // Total coverage
        let total: i64 = s.iter().map(|s| s.end_byte - s.start_byte).sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn last_segment_absorbs_remainder() {
        let s = split(1000, 3);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].start_byte, 0);
        assert_eq!(s[0].end_byte, 333);
        assert_eq!(s[1].start_byte, 333);
        assert_eq!(s[1].end_byte, 666);
        assert_eq!(s[2].start_byte, 666);
        assert_eq!(s[2].end_byte, 1000); // 334 bytes, absorbs remainder
        let total: i64 = s.iter().map(|s| s.end_byte - s.start_byte).sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn single_segment_covers_all() {
        let s = split(1000, 1);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].start_byte, 0);
        assert_eq!(s[0].end_byte, 1000);
    }

    #[test]
    fn more_segments_than_bytes_caps_at_byte_count() {
        // 5 bytes, 10 requested — only 5 segments (each 1 byte).
        let s = split(5, 10);
        assert_eq!(s.len(), 5);
        for (i, seg) in s.iter().enumerate() {
            assert_eq!(seg.start_byte, i as i64);
            assert_eq!(seg.end_byte, (i + 1) as i64);
        }
    }

    #[test]
    fn zero_total_returns_empty() {
        assert!(split(0, 8).is_empty());
    }

    #[test]
    fn zero_n_returns_empty() {
        assert!(split(1000, 0).is_empty());
    }

    #[test]
    fn segments_start_at_their_offset() {
        // Each segment's current_byte starts equal to start_byte.
        let s = split(1000, 4);
        for seg in &s {
            assert_eq!(seg.current_byte, seg.start_byte);
        }
    }
}
