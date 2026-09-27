//! QoS rate limiter — a governor-backed token bucket shared across all
//! active download tasks.
//!
//! # Design
//!
//! There is exactly ONE [`QosLimiter`] per [`crate::Engine`]. Every segment
//! worker of every task calls [`acquire`](Self::acquire) with the size of the
//! chunk it just received, so the *aggregate* bandwidth of the whole daemon
//! is capped at the configured target — that's what makes HyprFetch polite on
//! congested links (and what makes it different from per-task throttling).
//!
//! The target rate can be changed at runtime (`PUT /api/qos`). Because
//! `governor`'s `Quota` is immutable once constructed, changing the target
//! atomically swaps in a freshly built limiter under a read-mostly
//! `RwLock`. In-flight `acquire` calls keep using the limiter they grabbed,
//! so a mid-download retune never corrupts accounting — it simply applies to
//! new chunks.
//!
//! # Burst behavior
//!
//! The bucket capacity equals one second of the target rate (governor's
//! `Quota::per_second` semantics): a fresh/retuned bucket starts full, which
//! gives downloads a short ramp-up burst before settling at the target.
//!
//! When disabled (default), all calls to [`acquire`](Self::acquire) are
//! no-ops so the limiter adds zero overhead to the fast path.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultDirectRateLimiter, Quota};

/// A governor limiter paired with the target rate it was built for.
///
/// Keeping the target inside the arc makes `acquire` self-consistent: the
/// burst size used to split large chunks always matches the bucket it is
/// spent against, even if the global setting changes concurrently.
#[derive(Debug)]
struct LimiterSlot {
    target_bps: u64,
    limiter: Arc<DefaultDirectRateLimiter>,
}

/// A token-bucket rate limiter that, when enabled, caps the global
/// aggregate download rate across all segments of all tasks.
///
/// Cheap to clone (`Arc` interior); clones share the same bucket.
#[derive(Debug, Clone)]
pub struct QosLimiter {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    enabled: AtomicBool,
    target_bps: AtomicU64,
    slot: std::sync::RwLock<Option<Arc<LimiterSlot>>>,
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
                target_bps: AtomicU64::new(0),
                slot: std::sync::RwLock::new(None),
            }),
        }
    }

    /// Enable QoS at the given target rate (bytes per second).
    ///
    /// `target_bps == 0` is equivalent to [`disable`](Self::disable) — a
    /// zero-byte budget cannot issue tokens.
    pub fn enable(&self, target_bps: u64) {
        self.inner.target_bps.store(target_bps, Ordering::Relaxed);
        if target_bps == 0 {
            self.inner.enabled.store(false, Ordering::Relaxed);
            *write_slot(&self.inner.slot) = None;
            return;
        }
        let burst = burst_for(target_bps);
        // `Quota::per_second(n)` = n tokens/sec with a burst capacity of n —
        // i.e. one second's worth of bandwidth.
        let quota = Quota::per_second(NonZeroU32::new(burst).expect("burst_for is nonzero"));
        let slot = Arc::new(LimiterSlot {
            target_bps,
            limiter: Arc::new(DefaultDirectRateLimiter::direct(quota)),
        });
        *write_slot(&self.inner.slot) = Some(slot);
        self.inner.enabled.store(true, Ordering::Relaxed);
    }

    /// Disable QoS — full speed allowed. Already-issued budget is not
    /// revoked, but new `acquire` calls return immediately.
    pub fn disable(&self) {
        self.inner.enabled.store(false, Ordering::Relaxed);
        self.inner.target_bps.store(0, Ordering::Relaxed);
        *write_slot(&self.inner.slot) = None;
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

    /// Reserve `bytes` of download budget. Returns once the budget is
    /// available; when disabled, returns immediately.
    ///
    /// Requests larger than the bucket capacity are split into
    /// capacity-sized acquisitions and spread over time by the governor
    /// clock — so arbitrarily large chunks are safe to pass.
    ///
    /// If QoS is disabled *while* an `acquire` is in flight, the call
    /// finishes under the (old) bucket it already grabbed; only new calls
    /// observe the disabled state.
    pub async fn acquire(&self, bytes: u64) {
        if bytes == 0 || !self.is_enabled() {
            return;
        }
        // Grab a snapshot of the current slot. Cloned Arc — the critical
        // section is one clone long, so workers never block each other
        // beyond that.
        let slot = {
            let guard = read_slot(&self.inner.slot);
            guard.as_ref().map(Arc::clone)
        };
        let Some(slot) = slot else {
            return;
        };

        let burst = burst_for(slot.target_bps) as u64;
        let mut remaining = bytes;
        while remaining > 0 && self.is_enabled() {
            let n = remaining.min(burst).min(u32::MAX as u64) as u32;
            let n = NonZeroU32::new(n).expect("n > 0 because remaining > 0");
            // `until_n_ready` sleeps (async, yields the task) until the
            // bucket holds `n` tokens. InsufficientCapacity cannot happen
            // here because `n <= burst == capacity` by construction and the
            // slot is immutable — but treat it defensively rather than spin.
            match slot.limiter.until_n_ready(n).await {
                Ok(_) => {
                    remaining -= n.get() as u64;
                }
                Err(_insufficient) => {
                    // Capacity mismatch (shouldn't happen): back off a tick
                    // and re-evaluate; if the limiter got disabled meanwhile
                    // the loop condition exits.
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    return;
                }
            }
        }
    }
}

/// Bucket capacity in bytes: one second of the target rate, clamped to
/// `u32::MAX` (governor quotas are `u32`-based).
fn burst_for(target_bps: u64) -> u32 {
    target_bps.min(u32::MAX as u64) as u32
}

fn read_slot(
    slot: &std::sync::RwLock<Option<Arc<LimiterSlot>>>,
) -> std::sync::RwLockReadGuard<'_, Option<Arc<LimiterSlot>>> {
    slot.read().unwrap_or_else(|e| e.into_inner())
}

fn write_slot(
    slot: &std::sync::RwLock<Option<Arc<LimiterSlot>>>,
) -> std::sync::RwLockWriteGuard<'_, Option<Arc<LimiterSlot>>> {
    slot.write().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    #[test]
    fn enable_zero_disables() {
        let l = QosLimiter::new();
        l.enable(1000);
        assert!(l.is_enabled());
        l.enable(0);
        assert!(!l.is_enabled());
        assert_eq!(l.target_bps(), 0);
    }

    #[tokio::test]
    async fn acquire_is_noop_when_disabled() {
        let l = QosLimiter::new();
        // Should complete instantly (no blocking).
        let start = Instant::now();
        l.acquire(1_000_000).await;
        assert!(start.elapsed() < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn acquire_zero_is_noop_when_enabled() {
        let l = QosLimiter::new();
        l.enable(1_000);
        let start = Instant::now();
        l.acquire(0).await;
        assert!(start.elapsed() < Duration::from_millis(10));
    }

    #[test]
    fn clone_shares_state() {
        let l = QosLimiter::new();
        let l2 = l.clone();
        l.enable(2_000_000);
        assert!(l2.is_enabled());
        assert_eq!(l2.target_bps(), 2_000_000);
    }

    #[tokio::test]
    async fn acquire_throttles_to_target_rate() {
        // 10 KB/s budget. First 10 KB come from the full bucket instantly;
        // the next 15 KB must arrive at ~10 KB/s ⇒ ~1.5s of waiting.
        let l = QosLimiter::new();
        l.enable(10_000);
        let start = Instant::now();
        l.acquire(25_000).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(1_200),
            "expected ≥1.2s of throttling, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(6_000),
            "throttling ran away: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn acquire_splits_requests_larger_than_capacity() {
        // 600 B/s ⇒ capacity 600. A 1_500-byte acquire must be split into
        // 600 + 600 + 300 and stretch over ~1.5s.
        let l = QosLimiter::new();
        l.enable(600);
        let start = Instant::now();
        l.acquire(1_500).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(1_000),
            "expected ≥1.0s, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_acquires_share_one_budget() {
        // Two workers pulling 2_000 bytes each against a 2_000 B/s bucket:
        // combined they need (4_000 − 2_000) / 2_000 ≈ 1s. If each worker
        // had its OWN bucket this would finish instantly.
        let l = QosLimiter::new();
        l.enable(2_000);
        let start = Instant::now();
        let l2 = l.clone();
        let (r1, r2) = tokio::join!(async move { l.acquire(2_000).await }, async move {
            l2.acquire(2_000).await
        });
        let _ = (r1, r2);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(800),
            "expected shared budget to serialize workers (≥0.8s), got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn reenabling_swaps_in_a_fresh_full_bucket() {
        let l = QosLimiter::new();
        l.enable(1_000);
        l.acquire(1_000).await; // drains the small bucket
                                // Retune to a big rate: the new bucket starts full, so a large
                                // acquire completes quickly without waiting for the old bucket.
        l.enable(4_000_000);
        let start = Instant::now();
        l.acquire(1_000_000).await;
        assert!(
            start.elapsed() < Duration::from_millis(1_000),
            "retuned bucket should have fresh budget, took {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn disable_makes_new_acquires_instant() {
        let l = QosLimiter::new();
        l.enable(1_000);
        l.acquire(500).await;
        l.disable();
        let start = Instant::now();
        l.acquire(10_000_000).await;
        assert!(
            start.elapsed() < Duration::from_millis(10),
            "disabled limiter must not block, took {:?}",
            start.elapsed()
        );
    }
}
