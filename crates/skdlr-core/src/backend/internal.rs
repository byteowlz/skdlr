//! Internal scheduler backend.
//!
//! A fallback scheduler that runs as a daemon process, checking schedules
//! at regular intervals. Used when native OS schedulers are unavailable.

use chrono::{DateTime, Utc};
use tokio::process::Command as TokioCommand;

use super::{Backend, BackendKind, BoxFuture};
use crate::SkdlrConfig;
use crate::error::Result;
use crate::models::{Run, Schedule, ScheduleKind, ScheduleStatus};
use crate::validation::validate_schedule;

/// Internal scheduler backend.
///
/// This backend runs as part of the skdlr daemon and checks schedules
/// at regular intervals, executing commands when their cron expressions match.
#[derive(Debug)]
pub struct InternalBackend {
    /// Check interval in seconds.
    check_interval_secs: u64,
    /// Configuration for executor wrapping.
    config: SkdlrConfig,
}

impl InternalBackend {
    /// Creates a new internal backend.
    pub fn new(config: &SkdlrConfig) -> Self {
        Self {
            check_interval_secs: config.internal.check_interval_secs,
            config: config.clone(),
        }
    }

    /// Runs a command and returns the Run result.
    async fn execute_command(&self, schedule: &Schedule) -> Run {
        let mut run = Run::new(schedule.id, false);

        // Get wrapped command if configured
        let (program, mut args) = match super::render_wrapped_command(schedule, &self.config) {
            Ok(cmd) => cmd,
            Err(e) => {
                tracing::error!("Failed to render wrapped command: {}", e);
                // Fallback to direct execution
                let (shell, shell_args) = shell_command_owned();
                (shell, shell_args)
            }
        };

        let mut cmd = TokioCommand::new(program);
        cmd.args(&mut args);

        if let Some(workdir) = &schedule.workdir {
            cmd.current_dir(workdir);
        }

        for (key, value) in &schedule.env {
            cmd.env(key, value);
        }

        match cmd.output().await {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(-1);
                run.complete(exit_code);
            }
            Err(e) => {
                run.fail(e.to_string());
            }
        }

        run
    }

    /// Calculates the next run time based on schedule kind.
    fn next_run_time(kind: &ScheduleKind) -> Option<DateTime<Utc>> {
        match kind {
            ScheduleKind::Recurring { cron_expr } => Self::next_from_cron(cron_expr),
            ScheduleKind::OneOff { run_at } => {
                // Return run_at if it's in the future
                let now = Utc::now();
                if *run_at > now { Some(*run_at) } else { None }
            }
        }
    }

    /// Returns true if a schedule should execute at `now`.
    ///
    /// Recurring schedules are due when their next cron occurrence falls
    /// within the check window. One-off schedules are due once their time has
    /// arrived (`now >= run_at`) and stay due until executed, so a run missed
    /// while the daemon was down is still caught up rather than dropped.
    fn is_due(kind: &ScheduleKind, now: DateTime<Utc>, window_secs: i64) -> bool {
        match kind {
            ScheduleKind::Recurring { cron_expr } => Self::next_from_cron(cron_expr)
                .is_some_and(|next| (next - now).num_seconds().abs() <= window_secs),
            ScheduleKind::OneOff { run_at } => now >= *run_at,
        }
    }

    /// Calculates the next run time from a cron expression.
    fn next_from_cron(cron_expr: &str) -> Option<DateTime<Utc>> {
        use cron::Schedule as CronSchedule;
        use std::str::FromStr;

        // Add seconds field if not present (cron crate expects 6 or 7 fields)
        let expr = if cron_expr.split_whitespace().count() == 5 {
            format!("0 {}", cron_expr)
        } else {
            cron_expr.to_string()
        };

        CronSchedule::from_str(&expr)
            .ok()
            .and_then(|sched| sched.upcoming(Utc).next())
    }

    /// Starts the scheduler loop (call this in daemon mode).
    pub async fn run_scheduler(&self, storage: &crate::Storage) -> Result<()> {
        loop {
            let schedules = storage.list_schedules()?;

            for schedule in schedules {
                self.maybe_execute(storage, &schedule).await;
            }

            tokio::time::sleep(std::time::Duration::from_secs(self.check_interval_secs)).await;
        }
    }

    /// Checks if a schedule is due and executes it.
    async fn maybe_execute(&self, storage: &crate::Storage, schedule: &Schedule) {
        if schedule.status != ScheduleStatus::Enabled {
            return;
        }

        if !Self::is_due(&schedule.kind, Utc::now(), self.check_interval_secs as i64) {
            return;
        }

        tracing::info!("Executing schedule: {}", schedule.name);
        let run = self.execute_command(schedule).await;
        if let Err(e) = storage.save_run(&run) {
            tracing::error!("Failed to save run: {}", e);
        }

        if schedule.is_one_off() {
            Self::disable_one_off(storage, schedule);
        }
    }

    /// Disables a one-off schedule after execution.
    fn disable_one_off(storage: &crate::Storage, schedule: &Schedule) {
        let mut updated = schedule.clone();
        updated.status = ScheduleStatus::Disabled;
        updated.updated_at = Utc::now();
        if let Err(e) = storage.save_schedule(&updated) {
            tracing::error!("Failed to disable one-off schedule after execution: {}", e);
        }
    }
}

/// Returns shell command as owned values (for fallback).
fn shell_command_owned() -> (String, Vec<String>) {
    #[cfg(target_os = "windows")]
    {
        ("cmd.exe".to_string(), vec!["/C".to_string()])
    }

    #[cfg(not(target_os = "windows"))]
    {
        ("sh".to_string(), vec!["-c".to_string()])
    }
}

#[cfg(target_os = "windows")]
fn shell_command() -> (&'static str, [&'static str; 1]) {
    ("cmd.exe", ["/C"])
}

#[cfg(not(target_os = "windows"))]
fn shell_command() -> (&'static str, [&'static str; 1]) {
    ("sh", ["-c"])
}

impl Backend for InternalBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Internal
    }

    fn install<'a>(&'a self, _schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Internal backend doesn't need to install files
            // Schedules are managed in SQLite
            Ok(())
        })
    }

    fn uninstall<'a>(&'a self, _schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Nothing to uninstall for internal backend
            Ok(())
        })
    }

    fn enable<'a>(&'a self, _schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Status is managed in SQLite
            Ok(())
        })
    }

    fn disable<'a>(&'a self, _schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Status is managed in SQLite
            Ok(())
        })
    }

    fn run_now<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<Run>> {
        Box::pin(async move {
            validate_schedule(schedule)?;
            let run = self.execute_command(schedule).await;
            Ok(run)
        })
    }

    fn is_running<'a>(&'a self, _schedule: &'a Schedule) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            // TODO: Track running processes
            Ok(false)
        })
    }

    fn get_runs<'a>(
        &'a self,
        _schedule: &'a Schedule,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move {
            // Runs are stored in SQLite, not here
            Ok(Vec::new())
        })
    }

    fn next_run<'a>(
        &'a self,
        schedule: &'a Schedule,
    ) -> BoxFuture<'a, Result<Option<DateTime<Utc>>>> {
        Box::pin(async move { Ok(Self::next_run_time(&schedule.kind)) })
    }

    fn is_available(&self) -> bool {
        true // Always available as fallback
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn shell_command_uses_cmd() {
        let (shell, args) = shell_command();
        assert_eq!(shell, "cmd.exe");
        assert_eq!(args, ["/C"]);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn shell_command_uses_sh() {
        let (shell, args) = shell_command();
        assert_eq!(shell, "sh");
        assert_eq!(args, ["-c"]);
    }

    #[test]
    fn one_off_is_due_once_time_has_arrived() {
        let now = Utc::now();
        let kind = ScheduleKind::one_off(now - chrono::Duration::minutes(5));
        // Past-due one-offs stay due (catch-up), they are never dropped.
        assert!(InternalBackend::is_due(&kind, now, 60));

        let kind = ScheduleKind::one_off(now + chrono::Duration::minutes(5));
        assert!(!InternalBackend::is_due(&kind, now, 60));
    }

    #[test]
    fn recurring_is_due_within_check_window() {
        let now = Utc::now();
        // Every minute: due with a 60s window…
        let every_minute = ScheduleKind::recurring("* * * * *");
        assert!(InternalBackend::is_due(&every_minute, now, 60));
        // …but a yearly schedule is months away, so never due.
        let yearly = ScheduleKind::recurring("0 0 1 1 *");
        assert!(!InternalBackend::is_due(&yearly, now, 60));
    }
}
