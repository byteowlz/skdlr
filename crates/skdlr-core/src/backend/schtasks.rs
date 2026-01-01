//! Windows Task Scheduler backend.
//!
//! Uses schtasks.exe CLI for scheduling tasks.

use tokio::process::Command;

use super::{Backend, BackendKind, BoxFuture};
use crate::SkdlrConfig;
use crate::error::{Error, Result};
use crate::models::{Run, Schedule};
use crate::validation::validate_schedule;

/// Windows Task Scheduler backend.
pub struct SchtasksBackend {
    /// Prefix for task names.
    service_prefix: String,
}

impl SchtasksBackend {
    /// Creates a new schtasks backend.
    pub fn new(config: &SkdlrConfig) -> Self {
        Self {
            service_prefix: config.service_prefix.clone(),
        }
    }

    /// Returns the task name for a schedule.
    fn task_name(&self, schedule: &Schedule) -> String {
        format!("{}\\{}", self.service_prefix, schedule.name)
    }

    /// Runs schtasks with the given arguments.
    async fn schtasks(&self, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new("schtasks").args(args).output().await?;

        Ok(output)
    }

    /// Converts a cron expression to schtasks schedule parameters.
    fn cron_to_schtasks_args(&self, cron_expr: &str) -> Result<Vec<String>> {
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

        let mut args = Vec::new();

        // Determine schedule type based on cron pattern
        if dow != "*" && day == "*" && month == "*" {
            // Weekly schedule
            args.push("/SC".to_string());
            args.push("WEEKLY".to_string());
            args.push("/D".to_string());
            args.push(dow_to_schtasks(dow));
        } else if day != "*" && month == "*" && dow == "*" {
            // Monthly schedule
            args.push("/SC".to_string());
            args.push("MONTHLY".to_string());
            args.push("/D".to_string());
            args.push(day.to_string());
        } else if day == "*" && month == "*" && dow == "*" {
            // Daily schedule
            args.push("/SC".to_string());
            args.push("DAILY".to_string());
        } else {
            // Complex pattern - use ONCE and note limitation
            return Err(Error::InvalidCron(
                "complex cron patterns not fully supported on Windows".to_string(),
            ));
        }

        // Set time
        if hour != "*" && minute != "*" {
            args.push("/ST".to_string());
            args.push(format!("{:0>2}:{:0>2}", hour, minute));
        }

        Ok(args)
    }
}

impl Backend for SchtasksBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Schtasks
    }

    fn install<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            validate_schedule(schedule)?;

            let task_name = self.task_name(schedule);
            let mut args = vec![
                "/CREATE".to_string(),
                "/TN".to_string(),
                task_name,
                "/TR".to_string(),
                schedule.command.clone(),
                "/F".to_string(), // Force overwrite
            ];

            // Add schedule parameters
            let schedule_args = self.cron_to_schtasks_args(&schedule.cron_expr)?;
            args.extend(schedule_args);

            let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let output = self.schtasks(&args_refs).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to create task: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn uninstall<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let task_name = self.task_name(schedule);
            let output = self.schtasks(&["/DELETE", "/TN", &task_name, "/F"]).await?;

            if !output.status.success() {
                // Ignore "not found" errors
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("does not exist") {
                    return Err(Error::backend(format!("failed to delete task: {}", stderr)));
                }
            }

            Ok(())
        })
    }

    fn enable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let task_name = self.task_name(schedule);
            let output = self
                .schtasks(&["/CHANGE", "/TN", &task_name, "/ENABLE"])
                .await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to enable task: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn disable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let task_name = self.task_name(schedule);
            let output = self
                .schtasks(&["/CHANGE", "/TN", &task_name, "/DISABLE"])
                .await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to disable task: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn run_now<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<Run>> {
        Box::pin(async move {
            let task_name = self.task_name(schedule);
            let output = self.schtasks(&["/RUN", "/TN", &task_name]).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to run task: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(Run::new(schedule.id, true))
        })
    }

    fn is_running<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let task_name = self.task_name(schedule);
            let output = self
                .schtasks(&["/QUERY", "/TN", &task_name, "/V", "/FO", "LIST"])
                .await?;

            if !output.status.success() {
                return Ok(false);
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.contains("Running"))
        })
    }

    fn get_runs<'a>(
        &'a self,
        _schedule: &'a Schedule,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move {
            // schtasks /QUERY can show last run time but not full history
            // Would need to parse Windows Event Log
            Ok(Vec::new())
        })
    }

    fn next_run<'a>(
        &'a self,
        schedule: &'a Schedule,
    ) -> BoxFuture<'a, Result<Option<chrono::DateTime<chrono::Utc>>>> {
        Box::pin(async move {
            let task_name = self.task_name(schedule);
            let output = self
                .schtasks(&["/QUERY", "/TN", &task_name, "/V", "/FO", "LIST"])
                .await?;

            if !output.status.success() {
                return Ok(None);
            }

            // TODO: Parse "Next Run Time:" from output
            Ok(None)
        })
    }

    fn is_available(&self) -> bool {
        cfg!(target_os = "windows")
    }
}

/// Converts cron day-of-week to schtasks format.
fn dow_to_schtasks(dow: &str) -> String {
    match dow {
        "0" | "7" => "SUN".to_string(),
        "1" => "MON".to_string(),
        "2" => "TUE".to_string(),
        "3" => "WED".to_string(),
        "4" => "THU".to_string(),
        "5" => "FRI".to_string(),
        "6" => "SAT".to_string(),
        "*" => "*".to_string(),
        // Handle ranges like "1-5"
        other => other.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dow_to_schtasks() {
        assert_eq!(dow_to_schtasks("0"), "SUN");
        assert_eq!(dow_to_schtasks("1"), "MON");
        assert_eq!(dow_to_schtasks("5"), "FRI");
    }
}
