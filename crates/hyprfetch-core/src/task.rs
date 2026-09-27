//! Task model re-exports.
//!
//! The canonical `TaskState` lives in `hyprfetch_db::schema` so it can be
//! used uniformly by the DB layer and the engine. We re-export it here
//! for callers who only want to depend on `hyprfetch-core`.

pub use hyprfetch_db::schema::TaskState;

/// Opaque, sortable, unique task ID. Uses UUID v7 so creation order is
/// lexicographically sortable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub uuid::Uuid);

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

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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
    fn task_state_is_reexported() {
        let _s = TaskState::Queued;
        let _s = TaskState::Downloading;
        let _s = TaskState::Complete;
    }
}
