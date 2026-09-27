//! Download engine: task lifecycle, segmented downloads, resume, QoS.
//!
//! Stub for now. Real implementation lands in `feature/segmented-downloader`.

#![forbid(unsafe_code)]

pub mod qos;
pub mod task;

pub use qos::QosLimiter;
pub use task::{TaskId, TaskState};
