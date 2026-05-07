//! Linux systemd backend.
//!
//! Uses systemd user timers for scheduling. Creates .timer and .service files
//! in ~/.config/systemd/user/ and manages them via systemctl --user.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use tokio::process::Command;

use super::{Backend, BackendKind, BoxFuture};
use crate::SkdlrConfig;
use crate::error::{Error, Result};
use crate::models::{Run, Schedule, ScheduleKind};
use crate::validation::validate_schedule;

/// Systemd backend for Linux.
#[derive(Debug)]
pub struct SystemdBackend {
    /// Prefix for service/timer names.
    service_prefix: String,
    /// Path to systemd user directory.
    user_dir: PathBuf,
    /// Configuration for executor wrapping.
    config: SkdlrConfig,
}

impl SystemdBackend {
    /// Creates a new systemd backend.
    pub fn new(config: &SkdlrConfig) -> Self {
        let user_dir = dirs::config_dir()
            .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("systemd/user");

        Self {
            service_prefix: config.service_prefix.clone(),
            user_dir,
            config: config.clone(),
        }
    }

    /// Returns the service unit name for a schedule.
    fn service_name(&self, schedule: &Schedule) -> String {
        format!("{}-{}.service", self.service_prefix, schedule.name)
    }

    /// Returns the timer unit name for a schedule.
    fn timer_name(&self, schedule: &Schedule) -> String {
        format!("{}-{}.timer", self.service_prefix, schedule.name)
    }

    /// Returns the path to the service file.
    fn service_path(&self, schedule: &Schedule) -> PathBuf {
        self.user_dir.join(self.service_name(schedule))
    }

    /// Returns the path to the timer file.
    fn timer_path(&self, schedule: &Schedule) -> PathBuf {
        self.user_dir.join(self.timer_name(schedule))
    }

    /// Generates the service unit file content.
    fn generate_service(&self, schedule: &Schedule) -> String {
        let exec_start = match super::render_exec_start(schedule, &self.config) {
            Ok(cmd) => cmd,
            Err(e) => {
                log::error!("Failed to render wrapped command: {}", e);
                // Fallback to direct execution
                format!("ExecStart=/bin/sh -c '{}'", schedule.command.replace('\'', "'\\''"))
            }
        };

        let mut content = format!(
            "[Unit]\n\
             Description=skdlr: {}\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             {}\n",
            schedule.description.as_deref().unwrap_or(&schedule.name),
            exec_start,
        );

        if let Some(workdir) = &schedule.workdir {
            content.push_str(&format!("WorkingDirectory={}\n", workdir));
        }

        for (key, value) in &schedule.env {
            content.push_str(&format!("Environment=\"{}={}\"\n", key, value));
        }

        content
    }

    /// Generates the timer unit file content.
    fn generate_timer(&self, schedule: &Schedule) -> Result<String> {
        match &schedule.kind {
            ScheduleKind::Recurring { cron_expr } => {
                // Convert cron expression to systemd OnCalendar format
                let on_calendar = cron_to_oncalendar(cron_expr)?;

                Ok(format!(
                    "[Unit]\n\
                     Description=Timer for skdlr: {}\n\
                     Requires={}\n\
                     \n\
                     [Timer]\n\
                     OnCalendar={}\n\
                     Persistent=true\n\
                     \n\
                     [Install]\n\
                     WantedBy=timers.target\n",
                    schedule.name,
                    self.service_name(schedule),
                    on_calendar,
                ))
            }
            ScheduleKind::OneOff { run_at } => {
                // Convert timestamp to systemd OnCalendar format
                // Format: YYYY-MM-DD HH:MM:SS
                let on_calendar = run_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();

                Ok(format!(
                    "[Unit]\n\
                     Description=One-off timer for skdlr: {}\n\
                     Requires={}\n\
                     \n\
                     [Timer]\n\
                     OnCalendar={}\n\
                     Persistent=false\n\
                     \n\
                     [Install]\n\
                     WantedBy=timers.target\n",
                    schedule.name,
                    self.service_name(schedule),
                    on_calendar,
                ))
            }
        }
    }

    /// Runs systemctl with the given arguments.
    async fn systemctl(&self, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .await?;

        Ok(output)
    }

    /// Reloads the systemd daemon.
    async fn daemon_reload(&self) -> Result<()> {
        self.systemctl(&["daemon-reload"]).await?;
        Ok(())
    }
}

impl Backend for SystemdBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Systemd
    }

    fn install<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            validate_schedule(schedule)?;

            // Ensure user directory exists
            tokio::fs::create_dir_all(&self.user_dir).await?;

            // Write service file
            let service_content = self.generate_service(schedule);
            tokio::fs::write(self.service_path(schedule), service_content).await?;

            // Write timer file
            let timer_content = self.generate_timer(schedule)?;
            tokio::fs::write(self.timer_path(schedule), timer_content).await?;

            // Reload daemon
            self.daemon_reload().await?;

            Ok(())
        })
    }

    fn uninstall<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Disable first (ignore errors if not enabled)
            let _ = self.disable(schedule).await;

            // Remove files
            let _ = tokio::fs::remove_file(self.service_path(schedule)).await;
            let _ = tokio::fs::remove_file(self.timer_path(schedule)).await;

            // Reload daemon
            self.daemon_reload().await?;

            Ok(())
        })
    }

    fn enable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let timer = self.timer_name(schedule);
            let output = self.systemctl(&["enable", "--now", &timer]).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to enable timer: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn disable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let timer = self.timer_name(schedule);
            let output = self.systemctl(&["disable", "--now", &timer]).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to disable timer: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn run_now<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<Run>> {
        Box::pin(async move {
            let service = self.service_name(schedule);
            let mut run = Run::new(schedule.id, true);

            let output = self.systemctl(&["start", &service]).await?;

            if !output.status.success() {
                run.fail(format!(
                    "failed to start service: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
                return Ok(run);
            }

            // Wait for the service to complete (poll is-active)
            // Oneshot services become inactive once done
            let max_wait = std::time::Duration::from_secs(3600); // 1 hour max
            let poll_interval = std::time::Duration::from_millis(500);
            let start = std::time::Instant::now();

            loop {
                tokio::time::sleep(poll_interval).await;

                let status = self.systemctl(&["is-active", &service]).await?;
                let status_str = String::from_utf8_lossy(&status.stdout).trim().to_string();

                match status_str.as_str() {
                    "active" | "activating" => {
                        // Still running, continue waiting
                        if start.elapsed() > max_wait {
                            // Timeout - return as still running
                            return Ok(run);
                        }
                    }
                    "inactive" => {
                        // Completed - get the exit code from service status
                        let result = self
                            .systemctl(&["show", &service, "--property=ExecMainStatus"])
                            .await?;
                        let result_str = String::from_utf8_lossy(&result.stdout);

                        let exit_code = result_str
                            .trim()
                            .strip_prefix("ExecMainStatus=")
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(0);

                        run.complete(exit_code);
                        return Ok(run);
                    }
                    "failed" => {
                        // Get error info
                        let result = self
                            .systemctl(&["show", &service, "--property=ExecMainStatus"])
                            .await?;
                        let result_str = String::from_utf8_lossy(&result.stdout);

                        let exit_code = result_str
                            .trim()
                            .strip_prefix("ExecMainStatus=")
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(1);

                        run.complete(exit_code);
                        return Ok(run);
                    }
                    _ => {
                        // Unknown state (could be "unknown" if service doesn't exist)
                        run.fail(format!("unexpected service state: {}", status_str));
                        return Ok(run);
                    }
                }
            }
        })
    }

    fn is_running<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let service = self.service_name(schedule);
            let output = self.systemctl(&["is-active", &service]).await?;

            Ok(output.status.success())
        })
    }

    fn get_runs<'a>(
        &'a self,
        schedule: &'a Schedule,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move {
            // Parse journal output for this service into Run records.
            // We pair "Starting skdlr:" with the next "Finished skdlr:" / "Failed" entry.
            let service = self.service_name(schedule);
            let output = Command::new("journalctl")
                .args([
                    "--user",
                    "-u",
                    &service,
                    "--output=short-iso",
                    "--no-pager",
                    "-n",
                    &(limit.saturating_mul(8)).to_string(),
                ])
                .output()
                .await?;

            if !output.status.success() {
                return Ok(Vec::new());
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut runs: Vec<Run> = Vec::new();
            let mut pending_start: Option<chrono::DateTime<Utc>> = None;

            for line in stdout.lines() {
                let ts = line
                    .split_whitespace()
                    .next()
                    .and_then(|raw| chrono::DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%z").ok())
                    .map(|dt| dt.with_timezone(&Utc));

                let is_systemd_line = line.contains(" systemd[");

                if is_systemd_line && line.contains("Starting skdlr:") {
                    pending_start = ts;
                    continue;
                }

                if is_systemd_line && line.contains("Finished skdlr:") {
                    let mut run = Run::new(schedule.id, false);
                    if let Some(started_at) = pending_start.take().or(ts) {
                        run.started_at = started_at;
                    }
                    run.completed_at = ts.or(Some(run.started_at));
                    run.exit_code = Some(0);
                    run.status = crate::models::RunStatus::Succeeded;
                    runs.push(run);
                    if runs.len() >= limit {
                        break;
                    }
                    continue;
                }

                if is_systemd_line
                    && (line.contains("Failed with result")
                        || line.contains("Failed to start skdlr:")
                        || line.contains("Main process exited"))
                {
                    let mut run = Run::new(schedule.id, false);
                    if let Some(started_at) = pending_start.take().or(ts) {
                        run.started_at = started_at;
                    }
                    run.completed_at = ts.or(Some(run.started_at));
                    run.exit_code = Some(1);
                    run.status = crate::models::RunStatus::Failed;
                    run.error = Some(line.to_string());
                    runs.push(run);
                    if runs.len() >= limit {
                        break;
                    }
                }
            }

            runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
            runs.truncate(limit);
            Ok(runs)
        })
    }

    fn next_run<'a>(
        &'a self,
        schedule: &'a Schedule,
    ) -> BoxFuture<'a, Result<Option<DateTime<Utc>>>> {
        Box::pin(async move {
            let timer = self.timer_name(schedule);
            let output = self
                .systemctl(&["show", &timer, "--property=NextElapseUSecRealtime"])
                .await?;

            if !output.status.success() {
                // Fall back to schedule-based calculation if systemctl fails
                return Ok(next_from_schedule(&schedule.kind));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse "NextElapseUSecRealtime=<timestamp>" format
            // Example: "NextElapseUSecRealtime=Wed 2026-01-15 22:00:00 UTC"
            if let Some(value) = stdout.trim().strip_prefix("NextElapseUSecRealtime=") {
                if value.is_empty() || value == "n/a" {
                    // Timer not active, fall back to schedule-based calculation
                    return Ok(next_from_schedule(&schedule.kind));
                }

                // Try parsing the systemd timestamp format
                // Format: "Day YYYY-MM-DD HH:MM:SS TZ" or epoch microseconds
                if let Ok(usec) = value.parse::<u64>() {
                    // It's in microseconds since epoch
                    let secs = (usec / 1_000_000) as i64;
                    if let Some(dt) = DateTime::from_timestamp(secs, 0) {
                        return Ok(Some(dt));
                    }
                }

                // Try parsing as human-readable format (e.g., "Wed 2026-01-15 22:00:00 UTC")
                // Skip the day name if present
                let date_str = if value.contains(' ') {
                    // Skip first word if it looks like a day name
                    let parts: Vec<&str> = value.splitn(2, ' ').collect();
                    if parts.len() == 2 && parts[0].len() <= 3 {
                        parts[1]
                    } else {
                        value
                    }
                } else {
                    value
                };

                // Parse "YYYY-MM-DD HH:MM:SS TZ" format
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
                    date_str.trim_end_matches(" UTC").trim_end_matches(" Local"),
                    "%Y-%m-%d %H:%M:%S",
                ) {
                    return Ok(Some(dt.and_utc()));
                }
            }

            // Fall back to schedule-based calculation if parsing fails
            Ok(next_from_schedule(&schedule.kind))
        })
    }

    fn is_available(&self) -> bool {
        is_available()
    }
}

/// Checks if systemd is available on this system.
pub fn is_available() -> bool {
    std::process::Command::new("systemctl")
        .arg("--user")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Calculates the next run time from a schedule kind.
fn next_from_schedule(kind: &ScheduleKind) -> Option<DateTime<Utc>> {
    match kind {
        ScheduleKind::Recurring { cron_expr } => next_from_cron(cron_expr),
        ScheduleKind::OneOff { run_at } => {
            // Return the run_at time if it's in the future, otherwise None
            let now = Utc::now();
            if *run_at > now { Some(*run_at) } else { None }
        }
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

/// Converts a cron expression to systemd `OnCalendar` format.
///
/// Cron: minute hour day-of-month month day-of-week
/// `OnCalendar`: `DayOfWeek` Year-Month-Day Hour:Minute:Second
fn cron_to_oncalendar(cron_expr: &str) -> Result<String> {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(Error::InvalidCron(format!(
            "expected 5 fields, got {}",
            parts.len()
        )));
    }

    let minute = parts[0];
    let hour = parts[1];
    let day = parts[2];
    let month = parts[3];
    let dow = parts[4];

    // Build OnCalendar string
    // Format: DayOfWeek Year-Month-Day Hour:Minute:Second
    let mut calendar = String::new();

    // Day of week
    if dow != "*" {
        calendar.push_str(&dow_to_systemd(dow));
        calendar.push(' ');
    }

    // Date part: *-Month-Day
    calendar.push_str("*-");
    calendar.push_str(if month == "*" { "*" } else { month });
    calendar.push('-');
    calendar.push_str(if day == "*" { "*" } else { day });
    calendar.push(' ');

    // Time part: Hour:Minute:00
    calendar.push_str(if hour == "*" { "*" } else { hour });
    calendar.push(':');
    calendar.push_str(if minute == "*" { "*" } else { minute });
    calendar.push_str(":00");

    Ok(calendar)
}

/// Converts cron day-of-week to systemd format.
fn dow_to_systemd(dow: &str) -> String {
    // Cron uses 0-6 (Sun-Sat) or names
    // Systemd uses Mon, Tue, Wed, Thu, Fri, Sat, Sun
    match dow {
        "0" | "7" => "Sun".to_string(),
        "1" => "Mon".to_string(),
        "2" => "Tue".to_string(),
        "3" => "Wed".to_string(),
        "4" => "Thu".to_string(),
        "5" => "Fri".to_string(),
        "6" => "Sat".to_string(),
        other => other.to_string(), // Pass through names or patterns
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_to_oncalendar() {
        // Daily at 8am
        assert_eq!(cron_to_oncalendar("0 8 * * *").unwrap(), "*-*-* 8:0:00");

        // Every Monday at midnight
        assert_eq!(cron_to_oncalendar("0 0 * * 1").unwrap(), "Mon *-*-* 0:0:00");

        // First of every month at 2:30am
        assert_eq!(cron_to_oncalendar("30 2 1 * *").unwrap(), "*-*-1 2:30:00");
    }
}
