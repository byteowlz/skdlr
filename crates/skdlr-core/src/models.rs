//! Core data models for skdlr.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The kind of schedule: recurring (cron) or one-off (timestamp).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleKind {
    /// Recurring schedule using a cron expression.
    Recurring {
        /// Cron expression (e.g., "0 8 * * *" for daily at 8am).
        cron_expr: String,
    },
    /// One-off schedule that runs at a specific timestamp.
    OneOff {
        /// The timestamp when the task should run.
        run_at: DateTime<Utc>,
    },
}

impl ScheduleKind {
    /// Creates a recurring schedule kind from a cron expression.
    pub fn recurring(cron_expr: impl Into<String>) -> Self {
        Self::Recurring {
            cron_expr: cron_expr.into(),
        }
    }

    /// Creates a one-off schedule kind for a specific timestamp.
    pub fn one_off(run_at: DateTime<Utc>) -> Self {
        Self::OneOff { run_at }
    }

    /// Returns the cron expression if this is a recurring schedule.
    pub fn cron_expr(&self) -> Option<&str> {
        match self {
            Self::Recurring { cron_expr } => Some(cron_expr),
            Self::OneOff { .. } => None,
        }
    }

    /// Returns the `run_at` timestamp if this is a one-off schedule.
    pub fn run_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Recurring { .. } => None,
            Self::OneOff { run_at } => Some(*run_at),
        }
    }

    /// Returns true if this is a one-off schedule.
    pub fn is_one_off(&self) -> bool {
        matches!(self, Self::OneOff { .. })
    }

    /// Returns true if this is a recurring schedule.
    pub fn is_recurring(&self) -> bool {
        matches!(self, Self::Recurring { .. })
    }

    /// Returns a display string for this schedule kind.
    pub fn display(&self) -> String {
        match self {
            Self::Recurring { cron_expr } => cron_expr.clone(),
            Self::OneOff { run_at } => format!("@{}", run_at.format("%Y-%m-%d %H:%M:%S UTC")),
        }
    }
}

impl std::fmt::Display for ScheduleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display())
    }
}

/// Default tenant ID for single-user mode.
pub const DEFAULT_TENANT_ID: &str = "default";

/// A scheduled task definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique identifier.
    pub id: Uuid,

    /// Tenant identifier for multi-user isolation.
    #[serde(default = "default_tenant_id")]
    pub tenant_id: String,

    /// Human-readable name (used as identifier in CLI).
    pub name: String,

    /// Optional description.
    pub description: Option<String>,

    /// The schedule timing (cron expression or one-off timestamp).
    pub kind: ScheduleKind,

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

    /// Maximum number of retries on failure (0 = no retry).
    #[serde(default)]
    pub max_retries: u32,

    /// Backoff base delay in seconds between retries.
    #[serde(default = "default_retry_delay_secs")]
    pub retry_delay_secs: u64,
}

fn default_tenant_id() -> String {
    DEFAULT_TENANT_ID.to_string()
}

fn default_retry_delay_secs() -> u64 {
    30
}

impl Schedule {
    /// Creates a new recurring schedule with the given name, cron expression, and command.
    pub fn new(
        name: impl Into<String>,
        cron_expr: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            tenant_id: DEFAULT_TENANT_ID.to_string(),
            name: name.into(),
            description: None,
            kind: ScheduleKind::recurring(cron_expr),
            command: command.into(),
            workdir: None,
            env: std::collections::HashMap::new(),
            status: ScheduleStatus::Enabled,
            user: None,
            created_at: now,
            updated_at: now,
            paused_until: None,
            backend_id: None,
            max_retries: 0,
            retry_delay_secs: 30,
        }
    }

    /// Creates a new one-off schedule with the given name, timestamp, and command.
    pub fn new_one_off(
        name: impl Into<String>,
        run_at: DateTime<Utc>,
        command: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            tenant_id: DEFAULT_TENANT_ID.to_string(),
            name: name.into(),
            description: None,
            kind: ScheduleKind::one_off(run_at),
            command: command.into(),
            workdir: None,
            env: std::collections::HashMap::new(),
            status: ScheduleStatus::Enabled,
            user: None,
            created_at: now,
            updated_at: now,
            paused_until: None,
            backend_id: None,
            max_retries: 0,
            retry_delay_secs: 30,
        }
    }

    /// Sets the tenant ID.
    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }

    /// Sets retry configuration.
    pub fn with_retries(mut self, max_retries: u32, delay_secs: u64) -> Self {
        self.max_retries = max_retries;
        self.retry_delay_secs = delay_secs;
        self
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

    /// Returns true if this is a one-off schedule.
    pub fn is_one_off(&self) -> bool {
        self.kind.is_one_off()
    }

    /// Returns the cron expression if this is a recurring schedule.
    pub fn cron_expr(&self) -> Option<&str> {
        self.kind.cron_expr()
    }

    /// Returns the `run_at` timestamp if this is a one-off schedule.
    pub fn run_at(&self) -> Option<DateTime<Utc>> {
        self.kind.run_at()
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

/// A job instance represents a single scheduled execution tracked through its lifecycle.
///
/// Job instances provide atomic claim semantics, lease-based ownership,
/// retry tracking, and dead-letter handling for reliable execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInstance {
    /// Unique identifier.
    pub id: Uuid,

    /// The schedule this instance belongs to.
    pub schedule_id: Uuid,

    /// Tenant identifier for multi-user isolation.
    pub tenant_id: String,

    /// Idempotency key to prevent duplicate execution.
    /// Format: `{schedule_id}:{scheduled_at_rfc3339}`
    pub idempotency_key: String,

    /// Current lifecycle state.
    pub state: JobState,

    /// When this instance was scheduled to run.
    pub scheduled_at: DateTime<Utc>,

    /// When this instance was claimed by a worker.
    pub claimed_at: Option<DateTime<Utc>>,

    /// Worker ID that claimed this instance.
    pub claimed_by: Option<String>,

    /// Lease expiry — if now > `lease_expires_at`, the job is considered stuck.
    pub lease_expires_at: Option<DateTime<Utc>>,

    /// When execution started.
    pub started_at: Option<DateTime<Utc>>,

    /// When execution completed (succeeded, failed, or dead-lettered).
    pub completed_at: Option<DateTime<Utc>>,

    /// Exit code (if completed).
    pub exit_code: Option<i32>,

    /// Current retry attempt (0 = first attempt).
    pub attempt: u32,

    /// Maximum retries allowed (copied from schedule at creation).
    pub max_attempts: u32,

    /// When the next retry should be attempted (if retrying).
    pub next_retry_at: Option<DateTime<Utc>>,

    /// Error message from the last attempt.
    pub last_error: Option<String>,

    /// When this record was created.
    pub created_at: DateTime<Utc>,

    /// When this record was last updated.
    pub updated_at: DateTime<Utc>,
}

impl JobInstance {
    /// Creates a new queued job instance for a schedule.
    pub fn new(schedule: &Schedule, scheduled_at: DateTime<Utc>) -> Self {
        let now = Utc::now();
        let idempotency_key = format!("{}:{}", schedule.id, scheduled_at.to_rfc3339());
        Self {
            id: Uuid::new_v4(),
            schedule_id: schedule.id,
            tenant_id: schedule.tenant_id.clone(),
            idempotency_key,
            state: JobState::Queued,
            scheduled_at,
            claimed_at: None,
            claimed_by: None,
            lease_expires_at: None,
            started_at: None,
            completed_at: None,
            exit_code: None,
            attempt: 0,
            max_attempts: schedule.max_retries + 1, // first attempt + retries
            next_retry_at: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Returns true if the lease has expired.
    pub fn is_lease_expired(&self) -> bool {
        self.lease_expires_at
            .is_some_and(|expires| Utc::now() > expires)
    }

    /// Returns true if this instance can be retried.
    pub fn can_retry(&self) -> bool {
        self.attempt < self.max_attempts
    }

    /// Calculate exponential backoff delay for the current attempt.
    pub fn backoff_delay(&self, base_delay_secs: u64) -> chrono::Duration {
        let delay_secs = base_delay_secs * 2u64.saturating_pow(self.attempt.saturating_sub(1));
        // Cap at 1 hour
        let capped = delay_secs.min(3600);
        chrono::Duration::seconds(capped as i64)
    }
}

/// Lifecycle state of a job instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting to be claimed by a worker.
    Queued,

    /// Claimed by a worker, execution in progress.
    Running,

    /// Execution completed successfully.
    Succeeded,

    /// Execution failed, may be retried.
    Failed,

    /// Waiting to be retried after a failure.
    Retrying,

    /// All retry attempts exhausted — moved to dead letter.
    DeadLetter,

    /// Manually cancelled.
    Cancelled,
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Running => write!(f, "running"),
            Self::Succeeded => write!(f, "succeeded"),
            Self::Failed => write!(f, "failed"),
            Self::Retrying => write!(f, "retrying"),
            Self::DeadLetter => write!(f, "dead_letter"),
            Self::Cancelled => write!(f, "cancelled"),
        }
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_creation() {
        let schedule = Schedule::new("backup", "0 2 * * *", "restic backup ~")
            .with_workdir("/home/user")
            .with_description("Daily backup");

        assert_eq!(schedule.name, "backup");
        assert_eq!(schedule.cron_expr(), Some("0 2 * * *"));
        assert_eq!(schedule.command, "restic backup ~");
        assert_eq!(schedule.workdir, Some("/home/user".to_string()));
        assert_eq!(schedule.status, ScheduleStatus::Enabled);
        assert!(!schedule.is_one_off());
    }

    #[test]
    fn test_one_off_schedule_creation() {
        let run_at = Utc::now() + chrono::Duration::hours(1);
        let schedule = Schedule::new_one_off("one-time-backup", run_at, "restic backup ~")
            .with_workdir("/home/user")
            .with_description("One-time backup");

        assert_eq!(schedule.name, "one-time-backup");
        assert_eq!(schedule.run_at(), Some(run_at));
        assert!(schedule.cron_expr().is_none());
        assert!(schedule.is_one_off());
        assert_eq!(schedule.status, ScheduleStatus::Enabled);
    }

    #[test]
    fn test_schedule_kind_display() {
        let recurring = ScheduleKind::recurring("0 8 * * *");
        assert_eq!(recurring.display(), "0 8 * * *");

        let run_at = DateTime::parse_from_rfc3339("2026-02-15T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let one_off = ScheduleKind::one_off(run_at);
        assert_eq!(one_off.display(), "@2026-02-15 08:00:00 UTC");
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
