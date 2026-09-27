//! Type-safe schema constants and enums shared between the DB and the engine.
//!
//! Kept in a separate module so `hyprfetch-core` can depend on it without
//! pulling in the full DB layer.

use std::fmt;

/// Lifecycle state of a task.
///
/// Matches the `state` column in the `tasks` table.
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
    /// Failed — see `error_message` on the task row.
    Error,
    /// Removed by user. Kept briefly for UI feedback.
    Removed,
}

impl TaskState {
    /// All variants in canonical order.
    pub const ALL: &'static [Self] = &[
        Self::Queued,
        Self::Downloading,
        Self::Paused,
        Self::Complete,
        Self::Error,
        Self::Removed,
    ];

    /// Convert to the string used in the DB column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Removed => "removed",
        }
    }

    /// Parse from the DB string. Returns `None` if unknown.
    ///
    /// Named `from_db_str` to avoid colliding with `std::str::FromStr`.
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "complete" => Self::Complete,
            "error" => Self::Error,
            "removed" => Self::Removed,
            _ => return None,
        })
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-segment state.
///
/// Matches the `state` column in the `segments` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegmentState {
    /// Not yet started.
    Pending,
    /// Actively downloading.
    Downloading,
    /// Paused mid-flight.
    Paused,
    /// Reached `end_byte`.
    Complete,
    /// Failed — see error_message.
    Error,
}

impl SegmentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Error => "error",
        }
    }

    /// Parse from the DB string.
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "complete" => Self::Complete,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

impl fmt::Display for SegmentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// QoS override per task. `None` means use the global setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QosOverride {
    /// Use whatever the global QoS setting is.
    Auto,
    /// Force QoS on for this task, regardless of global setting.
    ForceOn,
    /// Force QoS off for this task, regardless of global setting.
    ForceOff,
}

impl QosOverride {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ForceOn => "force_on",
            Self::ForceOff => "force_off",
        }
    }

    /// Parse from the DB string.
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "force_on" => Self::ForceOn,
            "force_off" => Self::ForceOff,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_state_roundtrips_via_db_string() {
        for s in TaskState::ALL {
            let s2 = TaskState::from_db_str(s.as_str()).unwrap();
            assert_eq!(*s, s2);
        }
    }

    #[test]
    fn segment_state_roundtrips() {
        for s in [
            SegmentState::Pending,
            SegmentState::Downloading,
            SegmentState::Paused,
            SegmentState::Complete,
            SegmentState::Error,
        ] {
            let s2 = SegmentState::from_db_str(s.as_str()).unwrap();
            assert_eq!(s, s2);
        }
    }

    #[test]
    fn qos_override_roundtrips() {
        for s in [
            QosOverride::Auto,
            QosOverride::ForceOn,
            QosOverride::ForceOff,
        ] {
            let s2 = QosOverride::from_db_str(s.as_str()).unwrap();
            assert_eq!(s, s2);
        }
    }

    #[test]
    fn unknown_state_returns_none() {
        assert!(TaskState::from_db_str("invalid").is_none());
        assert!(SegmentState::from_db_str("invalid").is_none());
        assert!(QosOverride::from_db_str("invalid").is_none());
    }
}
