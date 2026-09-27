//! Engine event bus — broadcast fan-out of download lifecycle events.
//!
//! The engine publishes three event kinds on a `tokio::sync::broadcast`
//! channel; the API layer forwards them verbatim to WebSocket clients, so
//! the UI never has to poll:
//!
//! | event           | payload                                          | source            |
//! |-----------------|--------------------------------------------------|-------------------|
//! | `task:progress` | `downloaded_bytes, total_bytes, speed_bps`       | task coordinator  |
//! | `task:state`    | `state, error`                                   | task coordinator  |
//! | `global:speed`  | `speed_bps, active_tasks`                        | speed aggregator  |
//!
//! `global:speed` is computed by a background aggregator task that watches
//! `task:progress` deltas once per second and emits the aggregate rate of
//! all active tasks. It goes quiet (one final zero event) when the daemon
//! is idle, so subscribers aren't spammed while nothing is downloading.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::broadcast;

/// How often the aggregator samples progress deltas and emits `global:speed`.
pub const SPEED_TICK: Duration = Duration::from_secs(1);

/// One engine event, serialized as JSON for WebSocket clients.
#[derive(Debug, Clone, Serialize)]
pub struct EngineEvent {
    /// Event kind: `task:progress`, `task:state`, or `global:speed`.
    pub event: &'static str,
    /// Owning task id, if the event is task-scoped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Event-specific fields, flattened into the JSON object.
    #[serde(flatten)]
    pub payload: serde_json::Value,
    /// Unix timestamp (milliseconds).
    pub ts: i64,
}

impl EngineEvent {
    pub fn task_state(
        task_id: &str,
        state: hyprfetch_db::schema::TaskState,
        error: Option<&str>,
    ) -> Self {
        Self {
            event: "task:state",
            task_id: Some(task_id.to_string()),
            payload: serde_json::json!({
                "state": state.as_str(),
                "error": error,
            }),
            ts: now_ms(),
        }
    }

    pub fn task_progress(
        task_id: &str,
        downloaded_bytes: i64,
        total_bytes: Option<i64>,
        speed_bps: u64,
    ) -> Self {
        Self {
            event: "task:progress",
            task_id: Some(task_id.to_string()),
            payload: serde_json::json!({
                "downloaded_bytes": downloaded_bytes,
                "total_bytes": total_bytes,
                "speed_bps": speed_bps,
            }),
            ts: now_ms(),
        }
    }

    pub fn global_speed(speed_bps: u64, active_tasks: usize) -> Self {
        Self {
            event: "global:speed",
            task_id: None,
            payload: serde_json::json!({
                "speed_bps": speed_bps,
                "active_tasks": active_tasks,
            }),
            ts: now_ms(),
        }
    }
}

/// Cloneable handle to the engine's broadcast bus. `emit` never blocks and
/// silently drops events when there are no subscribers (or slow ones lag).
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<EngineEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// Create a bus with capacity for 1024 queued events per subscriber.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    /// Register a new subscriber. Each subscriber gets an independent
    /// replay-free stream starting "now" (lagged subscribers skip missed
    /// events rather than stall).
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.tx.subscribe()
    }

    /// Publish an event. No-op when nobody is listening.
    pub fn emit(&self, ev: EngineEvent) {
        // A send error just means "no active subscribers" — never a fault.
        let _ = self.tx.send(ev);
    }
}

/// Background task: watch `task:progress` events, and once per tick emit
/// `global:speed` with the aggregate bytes/sec across all active tasks.
///
/// Exits when the bus is dropped (all senders gone).
pub async fn run_speed_aggregator(
    bus: EventBus,
    mut rx: broadcast::Receiver<EngineEvent>,
    tick: Duration,
) {
    use broadcast::error::RecvError;
    use tokio::time::MissedTickBehavior;

    // task_id → most recent downloaded_bytes reported by its coordinator.
    let mut totals: HashMap<String, i64> = HashMap::new();
    // Snapshot of `totals` as of the previous tick (for delta math).
    let mut prev: HashMap<String, i64> = HashMap::new();
    let mut last_emitted_speed: u64 = 0;

    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let speed: i64 = totals
                    .iter()
                    .map(|(id, &now)| (now - prev.get(id).copied().unwrap_or(0)).max(0))
                    .sum();
                let speed = speed.max(0) as u64;
                let active = totals.len();
                // Emit while anything is active, plus one final zero-speed
                // event on the active → idle transition so UIs settle cleanly.
                if active > 0 || last_emitted_speed > 0 {
                    bus.emit(EngineEvent::global_speed(speed, active));
                }
                last_emitted_speed = speed;
                prev = totals.clone();
            }
            ev = rx.recv() => match ev {
                Ok(ev) => match ev.event {
                    "task:progress" => {
                        if let (Some(id), Some(bytes)) = (
                            ev.task_id,
                            ev.payload.get("downloaded_bytes").and_then(|v| v.as_i64()),
                        ) {
                            totals.insert(id, bytes);
                        }
                    }
                    "task:state" => {
                        let state = ev.payload.get("state").and_then(|v| v.as_str());
                        if matches!(state, Some("complete") | Some("error") | Some("removed")) {
                            if let Some(id) = ev.task_id {
                                totals.remove(&id);
                            }
                        }
                    }
                    _ => {}
                },
                Err(RecvError::Lagged(_)) => continue, // skip missed, keep going
                Err(RecvError::Closed) => break,       // engine dropped the bus
            },
        }
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn emit_without_subscribers_is_noop() {
        let bus = EventBus::new();
        bus.emit(EngineEvent::global_speed(1000, 1)); // must not panic
    }

    #[tokio::test]
    async fn subscribers_receive_emitted_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.emit(EngineEvent::global_speed(42, 2));
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.event, "global:speed");
        assert_eq!(ev.payload["speed_bps"], 42);
        assert_eq!(ev.payload["active_tasks"], 2);
    }

    #[tokio::test]
    async fn each_subscriber_gets_its_own_copy() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.emit(EngineEvent::task_state(
            "t1",
            hyprfetch_db::schema::TaskState::Downloading,
            None,
        ));
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn event_serializes_with_expected_shape() {
        let ev = EngineEvent::task_progress("abc", 500, Some(1000), 100);
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""event":"task:progress""#));
        assert!(json.contains(r#""task_id":"abc""#));
        assert!(json.contains(r#""downloaded_bytes":500"#));
        assert!(json.contains(r#""total_bytes":1000"#));
        assert!(json.contains(r#""speed_bps":100"#));
    }

    #[tokio::test]
    async fn aggregator_emits_zero_speed_on_idle_transition() {
        // Start aggregator, feed one progress event, then a terminal state —
        // the aggregator should emit global:speed (non-zero at least once
        // while active) and eventually a final 0 when the task completes.
        let bus = EventBus::new();
        let mut out = bus.subscribe();
        let agg_bus = bus.clone();
        let rx = bus.subscribe();
        tokio::spawn(async move {
            run_speed_aggregator(agg_bus, rx, Duration::from_millis(50)).await;
        });

        bus.emit(EngineEvent::task_progress("t1", 100, Some(1000), 0));
        tokio::time::sleep(Duration::from_millis(80)).await;
        bus.emit(EngineEvent::task_progress("t1", 600, Some(1000), 0));
        tokio::time::sleep(Duration::from_millis(80)).await;
        bus.emit(EngineEvent::task_state(
            "t1",
            hyprfetch_db::schema::TaskState::Complete,
            None,
        ));

        // Collect events for a short window.
        let start = Instant::now();
        let mut saw_speed = false;
        let mut saw_zero_after_active = false;
        while start.elapsed() < Duration::from_secs(3) {
            let ev = tokio::time::timeout(Duration::from_millis(300), out.recv()).await;
            let Ok(Ok(ev)) = ev else { break };
            if ev.event == "global:speed" {
                let bps = ev.payload["speed_bps"].as_u64().unwrap_or(0);
                if bps > 0 {
                    saw_speed = true;
                }
                if bps == 0 && saw_speed {
                    saw_zero_after_active = true;
                }
            }
        }
        assert!(saw_speed, "aggregator never emitted a non-zero speed");
        assert!(
            saw_zero_after_active,
            "aggregator never emitted the final idle zero-speed event"
        );
    }
}
