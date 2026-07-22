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
                format!(
                    "ExecStart=/bin/sh -c '{}'",
                    schedule.command.replace('\'', "'\\''")
                )
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
                    .and_then(|raw| {
                        chrono::DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%z").ok()
                    })
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

            runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
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
            // Ask systemd for an epoch value so named local timezone strings
            // (CEST, PST, etc.) never fall through to a UTC cron calculation.
            let output = self
                .systemctl(&[
                    "show",
                    &timer,
                    "--property=NextElapseUSecRealtime",
                    "--value",
                    "--timestamp=unix",
                ])
                .await?;

            if !output.status.success() {
                // Fall back to schedule-based calculation if systemctl fails
                return Ok(next_from_schedule(&schedule.kind));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let value = stdout.trim();
            if value.is_empty() || value == "n/a" {
                return Ok(next_from_schedule(&schedule.kind));
            }
            if let Some(timestamp) = parse_systemd_unix_timestamp(value) {
                return Ok(Some(timestamp));
            }

            // Fall back only when an older systemd cannot emit unix timestamps.
            Ok(next_from_schedule(&schedule.kind))
        })
    }

    fn is_available(&self) -> bool {
        is_available()
    }
}

fn parse_systemd_unix_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let seconds = value.strip_prefix('@').unwrap_or(value);
    let whole_seconds = seconds.split('.').next()?.parse::<i64>().ok()?;
    DateTime::from_timestamp(whole_seconds, 0)
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

    let minute = cron_field_to_systemd(parts[0], 0)?;
    let hour = cron_field_to_systemd(parts[1], 0)?;
    let day = cron_field_to_systemd(parts[2], 1)?;
    let month = cron_field_to_systemd(parts[3], 1)?;
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
    calendar.push_str(&month);
    calendar.push('-');
    calendar.push_str(&day);
    calendar.push(' ');

    // Time part: Hour:Minute:00
    calendar.push_str(&hour);
    calendar.push(':');
    calendar.push_str(&minute);
    calendar.push_str(":00");

    Ok(calendar)
}

/// systemd calendar repetition uses `start/step`, while cron commonly uses
/// `*/step`. Time fields start at zero; month/day fields start at one.
fn cron_field_to_systemd(field: &str, wildcard_start: u8) -> Result<String> {
    let Some(step) = field.strip_prefix("*/") else {
        return Ok(field.to_string());
    };
    let parsed = step
        .parse::<u32>()
        .map_err(|_| Error::InvalidCron(format!("invalid step value: {field}")))?;
    if parsed == 0 {
        return Err(Error::InvalidCron(format!(
            "step must be greater than zero: {field}"
        )));
    }
    Ok(format!("{wildcard_start}/{parsed}"))
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
    fn systemd_unix_timestamp_is_timezone_independent() {
        assert_eq!(
            parse_systemd_unix_timestamp("@1784715420")
                .expect("timestamp")
                .timestamp(),
            1_784_715_420
        );
        assert_eq!(
            parse_systemd_unix_timestamp("@1784715420.123456")
                .expect("fractional timestamp")
                .timestamp(),
            1_784_715_420
        );
        assert!(parse_systemd_unix_timestamp("Wed 2026-07-22 CEST").is_none());
    }

    #[test]
    fn test_cron_to_oncalendar() {
        // Daily at 8am
        assert_eq!(cron_to_oncalendar("0 8 * * *").unwrap(), "*-*-* 8:0:00");

        // Every Monday at midnight
        assert_eq!(cron_to_oncalendar("0 0 * * 1").unwrap(), "Mon *-*-* 0:0:00");

        // First of every month at 2:30am
        assert_eq!(cron_to_oncalendar("30 2 1 * *").unwrap(), "*-*-1 2:30:00");

        // Every six hours at minute 17. systemd uses start/step, not */step.
        assert_eq!(
            cron_to_oncalendar("17 */6 * * *").unwrap(),
            "*-*-* 0/6:17:00"
        );

        // Every fifteen minutes.
        assert_eq!(
            cron_to_oncalendar("*/15 * * * *").unwrap(),
            "*-*-* *:0/15:00"
        );

        // Every other month/day uses one as the wildcard range origin.
        assert_eq!(
            cron_to_oncalendar("0 0 */2 */3 *").unwrap(),
            "*-1/3-1/2 0:0:00"
        );

        assert!(cron_to_oncalendar("0 */0 * * *").is_err());
    }
}
