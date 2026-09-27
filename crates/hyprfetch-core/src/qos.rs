//! QoS rate limiter — token bucket shared across all active download segments.
//!
//! Stub. Real implementation lands in `feature/qos-rate-limiter`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A token-bucket rate limiter that, when enabled, caps the global
/// aggregate download rate across all segments.
///
/// When disabled (default), all calls to [`acquire`](Self::acquire) are no-ops
/// so the limiter adds zero overhead to the fast path.
#[derive(Debug, Clone)]
pub struct QosLimiter {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    enabled: AtomicBool,
    target_bps: std::sync::atomic::AtomicU64,
}

impl Default for QosLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl QosLimiter {
    /// Construct a disabled limiter (QoS off).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: AtomicBool::new(false),
                target_bps: std::sync::atomic::AtomicU64::new(0),
            }),
        }
    }

    /// Enable QoS at a given target rate (bytes per second).
    pub fn enable(&self, target_bps: u64) {
        self.inner.target_bps.store(target_bps, Ordering::Relaxed);
        self.inner.enabled.store(true, Ordering::Relaxed);
    }

    /// Disable QoS — full speed allowed.
    pub fn disable(&self) {
        self.inner.enabled.store(false, Ordering::Relaxed);
    }

    /// Returns `true` if QoS is currently capping downloads.
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Current target rate (bytes/sec). 0 if disabled.
    pub fn target_bps(&self) -> u64 {
        if self.is_enabled() {
            self.inner.target_bps.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Reserve `bytes` of download budget. When disabled, returns immediately.
    ///
    /// Stub for now — real implementation will sleep the appropriate amount
    /// using `governor`'s clock-aware token bucket. Returned future will
    /// yield when budget is available.
    pub async fn acquire(&self, _bytes: u64) {
        // No-op in stub. Real implementation pending.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let l = QosLimiter::new();
        assert!(!l.is_enabled());
        assert_eq!(l.target_bps(), 0);
    }

    #[test]
    fn enable_sets_target() {
        let l = QosLimiter::new();
        l.enable(5_242_880); // 5 MB/s
        assert!(l.is_enabled());
        assert_eq!(l.target_bps(), 5_242_880);
    }

    #[test]
    fn disable_clears() {
        let l = QosLimiter::new();
        l.enable(1_000_000);
        assert!(l.is_enabled());
        l.disable();
        assert!(!l.is_enabled());
        assert_eq!(l.target_bps(), 0);
    }

    #[tokio::test]
    async fn acquire_is_noop_when_disabled() {
        let l = QosLimiter::new();
        // Should complete instantly (no blocking).
        let start = std::time::Instant::now();
        l.acquire(1_000_000).await;
        assert!(start.elapsed() < std::time::Duration::from_millis(10));
    }

    #[test]
    fn clone_shares_state() {
        let l = QosLimiter::new();
        let l2 = l.clone();
        l.enable(2_000_000);
        assert!(l2.is_enabled());
        assert_eq!(l2.target_bps(), 2_000_000);
    }
}
