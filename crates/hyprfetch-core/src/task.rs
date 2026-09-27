//! Task model — the in-memory representation of a download.
//!
//! Stub: full state machine + persistence wiring lands in later PRs.

use std::fmt;

/// Opaque, sortable, unique task ID. Uses UUID v7 so creation order is
/// lexicographically sortable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct TaskId(uuid::Uuid);

impl TaskId {
    /// Generate a new v7 UUID-based ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// Created, waiting for a worker slot.
    Queued,
    /// Actively downloading.
    Downloading,
    /// User-paused or paused by QoS.
    Paused,
    /// Finished successfully.
    Complete,
    /// Failed — see `error` field on Task.
    Error,
    /// Removed by user. Kept in memory briefly for UI feedback.
    Removed,
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Downloading => write!(f, "downloading"),
            Self::Paused => write!(f, "paused"),
            Self::Complete => write!(f, "complete"),
            Self::Error => write!(f, "error"),
            Self::Removed => write!(f, "removed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_serializes_as_string() {
        let id = TaskId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.starts_with('"') && json.ends_with('"'));
    }

    #[test]
    fn task_state_roundtrips() {
        for s in [
            TaskState::Queued,
            TaskState::Downloading,
            TaskState::Paused,
            TaskState::Complete,
            TaskState::Error,
            TaskState::Removed,
        ] {
            let json = serde_json::to_string(&s).unwrap();
            let back: TaskState = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn task_state_display_matches_serde() {
        for s in [
            TaskState::Queued,
            TaskState::Downloading,
            TaskState::Paused,
            TaskState::Complete,
            TaskState::Error,
            TaskState::Removed,
        ] {
            let json = serde_json::to_string(&s).unwrap();
            let json_inner = json.trim_matches('"');
            assert_eq!(s.to_string(), json_inner);
        }
    }
}
