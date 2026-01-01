//! Core data models for skdlr.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A scheduled task definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique identifier.
    pub id: Uuid,

    /// Human-readable name (used as identifier in CLI).
    pub name: String,

    /// Optional description.
    pub description: Option<String>,

    /// Cron expression (e.g., "0 8 * * *" for daily at 8am).
    pub cron_expr: String,

    /// Command to execute.
    pub command: String,

    /// Working directory for command execution.
    pub workdir: Option<String>,

    /// Environment variables to set.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Current status.
    pub status: ScheduleStatus,

    /// User who owns this schedule (for multi-user mode).
    pub user: Option<String>,

    /// When this schedule was created.
    pub created_at: DateTime<Utc>,

    /// When this schedule was last modified.
    pub updated_at: DateTime<Utc>,

    /// Pause until this time (if paused temporarily).
    pub paused_until: Option<DateTime<Utc>>,

    /// Backend-specific identifier (e.g., systemd timer name).
    pub backend_id: Option<String>,
}

impl Schedule {
    /// Creates a new schedule with the given name, cron expression, and command.
    pub fn new(
        name: impl Into<String>,
        cron_expr: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: None,
            cron_expr: cron_expr.into(),
            command: command.into(),
            workdir: None,
            env: std::collections::HashMap::new(),
            status: ScheduleStatus::Enabled,
            user: None,
            created_at: now,
            updated_at: now,
            paused_until: None,
            backend_id: None,
        }
    }

    /// Sets the working directory.
    pub fn with_workdir(mut self, workdir: impl Into<String>) -> Self {
        self.workdir = Some(workdir.into());
        self
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Returns the service/timer name for this schedule.
    pub fn service_name(&self, prefix: &str) -> String {
        format!("{}-{}", prefix, self.name)
    }
}

/// Status of a schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// Schedule is active and will run at scheduled times.
    Enabled,

    /// Schedule is disabled and will not run.
    Disabled,

    /// Schedule is temporarily paused until a specific time.
    Paused,
}

impl std::fmt::Display for ScheduleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enabled => write!(f, "enabled"),
            Self::Disabled => write!(f, "disabled"),
            Self::Paused => write!(f, "paused"),
        }
    }
}

/// A single execution of a schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Unique identifier.
    pub id: Uuid,

    /// The schedule this run belongs to.
    pub schedule_id: Uuid,

    /// When the run started.
    pub started_at: DateTime<Utc>,

    /// When the run completed (if finished).
    pub completed_at: Option<DateTime<Utc>>,

    /// Exit code (if completed).
    pub exit_code: Option<i32>,

    /// Current status.
    pub status: RunStatus,

    /// Whether this was a manual run.
    pub manual: bool,

    /// Path to log file (if any).
    pub log_path: Option<String>,

    /// Error message (if failed).
    pub error: Option<String>,
}

impl Run {
    /// Creates a new run for the given schedule.
    pub fn new(schedule_id: Uuid, manual: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            schedule_id,
            started_at: Utc::now(),
            completed_at: None,
            exit_code: None,
            status: RunStatus::Running,
            manual,
            log_path: None,
            error: None,
        }
    }

    /// Marks the run as completed successfully.
    pub fn complete(&mut self, exit_code: i32) {
        self.completed_at = Some(Utc::now());
        self.exit_code = Some(exit_code);
        self.status = if exit_code == 0 {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        };
    }

    /// Marks the run as failed with an error.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.completed_at = Some(Utc::now());
        self.status = RunStatus::Failed;
        self.error = Some(error.into());
    }
}

/// Status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is currently executing.
    Running,

    /// Run completed successfully (exit code 0).
    Succeeded,

    /// Run failed (non-zero exit code or error).
    Failed,

    /// Run was cancelled.
    Cancelled,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Succeeded => write!(f, "succeeded"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_creation() {
        let schedule = Schedule::new("backup", "0 2 * * *", "restic backup ~")
            .with_workdir("/home/user")
            .with_description("Daily backup");

        assert_eq!(schedule.name, "backup");
        assert_eq!(schedule.cron_expr, "0 2 * * *");
        assert_eq!(schedule.command, "restic backup ~");
        assert_eq!(schedule.workdir, Some("/home/user".to_string()));
        assert_eq!(schedule.status, ScheduleStatus::Enabled);
    }

    #[test]
    fn test_service_name() {
        let schedule = Schedule::new("my-task", "0 * * * *", "echo hello");
        assert_eq!(schedule.service_name("skdlr"), "skdlr-my-task");
    }

    #[test]
    fn test_run_lifecycle() {
        let schedule = Schedule::new("test", "* * * * *", "echo test");
        let mut run = Run::new(schedule.id, false);

        assert_eq!(run.status, RunStatus::Running);
        assert!(run.completed_at.is_none());

        run.complete(0);
        assert_eq!(run.status, RunStatus::Succeeded);
        assert!(run.completed_at.is_some());
        assert_eq!(run.exit_code, Some(0));
    }
}
