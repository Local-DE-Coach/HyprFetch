//! Process resource usage of THIS app only (v0.4.6).
//!
//! Reads the kernel's own accounting for the current PID — no external
//! tools, no system-wide metrics, so the numbers are exactly "what this
//! app uses":
//! - `/proc/self/status` → VmRSS (current RAM), VmHWM (peak RAM), Threads
//! - `/proc/self/stat`   → utime + stime (CPU ticks, USER_HZ = 100 per
//!   proc(5)) → CPU% sampled between two calls, normalized to all cores
//!   (100% = every core fully busy).

use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;

/// Per proc(5), `/proc/*/stat` CPU times are always reported in USER_HZ
/// clock ticks, which is fixed at 100 on Linux regardless of CONFIG_HZ.
const USER_HZ: f64 = 100.0;

/// Last CPU sample, kept between successive requests so the percent is a
/// real delta instead of a guess.
#[derive(Clone, Copy)]
struct CpuSample {
    at: Instant,
    ticks: u64,
}

pub struct UsageTracker {
    last_cpu: Mutex<Option<CpuSample>>,
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self {
            last_cpu: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    /// Resident memory of this process right now, in bytes.
    pub rss_bytes: u64,
    /// Peak resident memory of this process, in bytes.
    pub peak_rss_bytes: u64,
    /// CPU usage of this process, percent of ALL cores (100 = every core
    /// fully busy). Computed over the window since the previous sample;
    /// the first call reports the average since process start.
    pub cpu_percent: f64,
    /// Live OS threads of this process.
    pub threads: u32,
    /// Process uptime in seconds (mirrors /api/server for convenience).
    pub uptime_secs: u64,
}

/// Parse `/proc/self/status` fields we care about (all in kB except Threads).
fn read_status() -> (Option<u64>, Option<u64>, Option<u32>) {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return (None, None, None);
    };
    let mut rss: Option<u64> = None;
    let mut hwm: Option<u64> = None;
    let mut threads: Option<u32> = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("VmRSS:") {
            rss = v.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some(v) = line.strip_prefix("VmHWM:") {
            hwm = v.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some(v) = line.strip_prefix("Threads:") {
            threads = v.trim().parse().ok();
        }
    }
    // kB → bytes.
    (rss.map(|k| k * 1024), hwm.map(|k| k * 1024), threads)
}

/// Sum of utime + stime ticks from `/proc/self/stat` (fields 14 + 15).
/// Commas in the comm field are handled by parsing after the last `)`.
fn read_cpu_ticks() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = text.rsplit(')').next()?;
    // `rest` starts with a space then the state char; fields from here are
    // ppid(1) … so utime is field 12 and stime field 13 in this tail.
    let mut fields = rest.split_whitespace();
    let mut utime = None;
    let mut stime = None;
    for (i, f) in fields.by_ref().enumerate() {
        if i == 12 {
            utime = f.parse::<u64>().ok();
        } else if i == 13 {
            stime = f.parse::<u64>().ok();
            break;
        }
    }
    Some(utime? + stime?)
}

fn core_count() -> f64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0)
}

impl UsageTracker {
    /// Take one sample. `uptime_secs` comes from the caller (AppState).
    pub fn sample(&self, uptime_secs: u64) -> Usage {
        let (rss_bytes, peak_rss_bytes, threads) = read_status();
        let ticks = read_cpu_ticks().unwrap_or(0);
        let cores = core_count();

        // CPU% since the previous sample (or since start on first call).
        let cpu_percent = {
            let mut last = self.last_cpu.lock().unwrap_or_else(|e| e.into_inner());
            let pct = match *last {
                Some(prev) if ticks >= prev.ticks => {
                    let wall = prev.at.elapsed().as_secs_f64();
                    if wall > 0.05 {
                        ((ticks - prev.ticks) as f64 / USER_HZ) / wall * 100.0 / cores
                    } else {
                        // Window too small to be meaningful — keep it simple.
                        0.0
                    }
                }
                Some(_) => 0.0, // ticks went backwards (shouldn't happen)
                None => {
                    // First sample: average since process start.
                    let started = Instant::now() - std::time::Duration::from_secs(uptime_secs);
                    let wall = started.elapsed().as_secs_f64();
                    if wall > 0.5 {
                        (ticks as f64 / USER_HZ) / wall * 100.0 / cores
                    } else {
                        0.0
                    }
                }
            };
            *last = Some(CpuSample {
                at: Instant::now(),
                ticks,
            });
            pct.clamp(0.0, 100.0)
        };

        Usage {
            rss_bytes: rss_bytes.unwrap_or(0),
            peak_rss_bytes: peak_rss_bytes.unwrap_or(rss_bytes.unwrap_or(0)),
            cpu_percent,
            threads: threads.unwrap_or(0),
            uptime_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_fields_parse_on_linux() {
        let (rss, _hwm, threads) = read_status();
        let rss = rss.expect("VmRSS must exist on Linux CI");
        assert!(rss > 0, "rss must be positive");
        assert!(threads.unwrap_or(0) > 0);
    }

    #[test]
    fn cpu_ticks_parse_on_linux() {
        let t = read_cpu_ticks().expect("self stat must exist on Linux CI");
        assert!(t > 0, "the test binary has used some CPU");
    }

    #[test]
    fn tracker_reports_sane_numbers() {
        let tracker = UsageTracker::default();
        let u1 = tracker.sample(1);
        assert!(u1.rss_bytes > 0);
        assert!(u1.cpu_percent >= 0.0 && u1.cpu_percent <= 100.0);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let u2 = tracker.sample(1);
        assert!(u2.rss_bytes > 0);
        assert!(u2.cpu_percent >= 0.0 && u2.cpu_percent <= 100.0);
    }
}
