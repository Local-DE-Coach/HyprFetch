//! Download engine: task lifecycle, segmented downloads, resume, QoS.
//!
//! The engine is the heart of HyprFetch. It owns the task table, spawns
//! segment workers, aggregates progress, and persists to the DB.

#![forbid(unsafe_code)]

pub mod categories;
pub mod engine;
pub mod events;
pub mod http_client;
pub mod planner;
pub mod qos;
pub mod segment;
pub mod ssrf;
pub mod task;
pub mod update;

pub use categories::{
    category_for_filename, ensure_all_dirs, ext_for_content_type, override_key, sanitize_filename,
    sniff_filename, CATEGORIES, SET_CATEGORIZE, SET_DOWNLOAD_DIR,
};
pub use engine::{Engine, EngineError};
pub use events::{EngineEvent, EventBus};
pub use http_client::{ExtraHeaders, HttpClient, HttpError, ProbeResult, DEFAULT_USER_AGENT};
pub use planner::split as split_segments;
pub use qos::QosLimiter;
pub use segment::{open_target_file, Segment, SegmentEvent, SegmentWorker, SegmentWorkerError};
pub use ssrf::{check_url, is_private_ip, private_range_name, SsrfPolicy, UrlSafetyError};
pub use task::{TaskId, TaskState};
pub use update::{ApplyResult, UpdateCheck, UpdateConfig, UpdateError};
