//! Error types for skdlr.

use thiserror::Error;

/// Result type alias using skdlr's Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in skdlr.
#[derive(Error, Debug)]
pub enum Error {
    /// Schedule not found.
    #[error("schedule not found: {0}")]
    ScheduleNotFound(String),

    /// Schedule already exists.
    #[error("schedule already exists: {0}")]
    ScheduleExists(String),

    /// Invalid cron expression.
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),

    /// Backend error.
    #[error("backend error: {0}")]
    Backend(String),

    /// Storage error.
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// Command execution error.
    #[error("command failed: {0}")]
    Command(String),

    /// Backend not available.
    #[error("backend not available: {0}")]
    BackendUnavailable(String),

    /// Permission denied.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Parse error.
    #[error("parse error: {0}")]
    Parse(String),

    /// Validation error.
    #[error("validation error: {0}")]
    Validation(String),
}

impl Error {
    /// Creates a backend error.
    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend(msg.into())
    }

    /// Creates a config error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Creates a command error.
    pub fn command(msg: impl Into<String>) -> Self {
        Self::Command(msg.into())
    }
}
