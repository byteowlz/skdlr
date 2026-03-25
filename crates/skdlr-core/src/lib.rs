//! skdlr-core: Cross-platform task scheduler core library.
//!
//! This crate provides:
//! - Backend trait for abstracting OS schedulers
//! - Schedule and Run models
//! - SQLite-based metadata storage
//! - Platform-specific backend implementations

pub mod backend;
pub mod config;
pub mod dispatcher;
pub mod error;
pub mod models;
pub mod paths;
pub mod scheduler;
pub mod storage;
pub mod validation;

pub use config::SkdlrConfig;
pub use dispatcher::{DispatchResult, Dispatcher, LocalDispatcher};
pub use error::{Error, Result};
pub use models::{
    DEFAULT_TENANT_ID, JobInstance, JobState, Run, RunStatus, Schedule, ScheduleKind,
    ScheduleStatus,
};
pub use scheduler::{Scheduler, SchedulerConfig};
pub use storage::Storage;

/// Application name used for config directories and environment prefix.
pub const APP_NAME: &str = "skdlr";

/// Returns the environment variable prefix for this application.
pub fn env_prefix() -> String {
    APP_NAME.to_ascii_uppercase()
}

/// Detects and returns the appropriate backend for the current platform.
pub fn detect_backend() -> backend::BackendKind {
    backend::BackendKind::detect()
}
