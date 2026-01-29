//! macOS launchd backend.
//!
//! Uses launchd for scheduling via plist files in ~/Library/LaunchAgents/.

use std::path::PathBuf;

use tokio::process::Command;

use super::{Backend, BackendKind, BoxFuture};
use crate::SkdlrConfig;
use crate::error::{Error, Result};
use crate::models::{Run, Schedule};
use crate::validation::validate_schedule;

/// Launchd backend for macOS.
pub struct LaunchdBackend {
    /// Prefix for plist labels.
    service_prefix: String,
    /// Path to LaunchAgents directory.
    agents_dir: PathBuf,
    /// Configuration for executor wrapping.
    config: SkdlrConfig,
}

impl LaunchdBackend {
    /// Creates a new launchd backend.
    pub fn new(config: &SkdlrConfig) -> Self {
        let agents_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join("Library/LaunchAgents");

        Self {
            service_prefix: config.service_prefix.clone(),
            agents_dir,
            config: config.clone(),
        }
    }

    /// Returns the plist label for a schedule.
    fn label(&self, schedule: &Schedule) -> String {
        format!("com.byteowlz.{}.{}", self.service_prefix, schedule.name)
    }

    /// Returns the path to the plist file.
    fn plist_path(&self, schedule: &Schedule) -> PathBuf {
        self.agents_dir
            .join(format!("{}.plist", self.label(schedule)))
    }

    /// Generates the plist file content.
    fn generate_plist(&self, schedule: &Schedule) -> Result<String> {
        // Get program arguments from wrapper
        let program_args = super::render_launchd_args(schedule, &self.config)?;

        // Convert args to plist XML array
        let args_array: Vec<String> = program_args
            .iter()
            .map(|a| format!("        <string>{}</string>", escape_xml(a)))
            .collect();

        let cron_expr = schedule
            .cron_expr()
            .ok_or_else(|| Error::InvalidCron("schedule is not recurring".to_string()))?;
        let calendar_interval = cron_to_calendar_interval(cron_expr)?;

        let mut plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{}</string>
    <key>ProgramArguments</key>
    <array>
{}
    </array>
    <key>StartCalendarInterval</key>
    {}
    <key>StandardOutPath</key>
    <string>/tmp/{}.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/{}.err.log</string>
"#,
            self.label(schedule),
            args_array.join("\n"),
            calendar_interval,
            self.label(schedule),
            self.label(schedule),
        );

        if let Some(workdir) = &schedule.workdir {
            plist.push_str(&format!(
                "    <key>WorkingDirectory</key>\n    <string>{}</string>\n",
                escape_xml(workdir)
            ));
        }

        if !schedule.env.is_empty() {
            plist.push_str("    <key>EnvironmentVariables</key>\n    <dict>\n");
            for (key, value) in &schedule.env {
                plist.push_str(&format!(
                    "        <key>{}</key>\n        <string>{}</string>\n",
                    escape_xml(key),
                    escape_xml(value)
                ));
            }
            plist.push_str("    </dict>\n");
        }

        plist.push_str("</dict>\n</plist>\n");

        Ok(plist)
    }

    /// Runs launchctl with the given arguments.
    async fn launchctl(&self, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new("launchctl").args(args).output().await?;

        Ok(output)
    }

    fn plist_path_str(&self, schedule: &Schedule) -> Result<String> {
        let path = self.plist_path(schedule);
        path.to_str()
            .map(str::to_string)
            .ok_or_else(|| Error::backend("plist path is not valid UTF-8".to_string()))
    }
}

impl Backend for LaunchdBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Launchd
    }

    fn install<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            validate_schedule(schedule)?;

            // Ensure directory exists
            tokio::fs::create_dir_all(&self.agents_dir).await?;

            // Write plist file
            let plist_content = self.generate_plist(schedule)?;
            tokio::fs::write(self.plist_path(schedule), plist_content).await?;

            Ok(())
        })
    }

    fn uninstall<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Unload first (ignore errors if not loaded)
            let _ = self.disable(schedule).await;

            // Remove plist file
            let _ = tokio::fs::remove_file(self.plist_path(schedule)).await;

            Ok(())
        })
    }

    fn enable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let plist = self.plist_path_str(schedule)?;
            let output = self.launchctl(&["load", "-w", &plist]).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to load plist: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn disable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let plist = self.plist_path_str(schedule)?;
            let output = self.launchctl(&["unload", "-w", &plist]).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to unload plist: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(())
        })
    }

    fn run_now<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<Run>> {
        Box::pin(async move {
            let label = self.label(schedule);
            let output = self.launchctl(&["start", &label]).await?;

            if !output.status.success() {
                return Err(Error::backend(format!(
                    "failed to start job: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            Ok(Run::new(schedule.id, true))
        })
    }

    fn is_running<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let label = self.label(schedule);
            let output = self.launchctl(&["list", &label]).await?;

            Ok(output.status.success())
        })
    }

    fn get_runs<'a>(
        &'a self,
        _schedule: &'a Schedule,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move {
            // launchd doesn't provide run history
            // Would need to parse log files
            Ok(Vec::new())
        })
    }

    fn next_run<'a>(
        &'a self,
        _schedule: &'a Schedule,
    ) -> BoxFuture<'a, Result<Option<chrono::DateTime<chrono::Utc>>>> {
        Box::pin(async move {
            // launchd doesn't expose next run time easily
            // Would need to calculate from cron expression
            Ok(None)
        })
    }

    fn is_available(&self) -> bool {
        cfg!(target_os = "macos")
    }
}

/// Converts a cron expression to launchd StartCalendarInterval format.
fn cron_to_calendar_interval(cron_expr: &str) -> Result<String> {
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

    let mut dict = String::from("<dict>\n");

    if minute != "*" {
        dict.push_str(&format!(
            "        <key>Minute</key>\n        <integer>{}</integer>\n",
            minute
        ));
    }
    if hour != "*" {
        dict.push_str(&format!(
            "        <key>Hour</key>\n        <integer>{}</integer>\n",
            hour
        ));
    }
    if day != "*" {
        dict.push_str(&format!(
            "        <key>Day</key>\n        <integer>{}</integer>\n",
            day
        ));
    }
    if month != "*" {
        dict.push_str(&format!(
            "        <key>Month</key>\n        <integer>{}</integer>\n",
            month
        ));
    }
    if dow != "*" {
        // launchd uses 0=Sunday, same as cron
        dict.push_str(&format!(
            "        <key>Weekday</key>\n        <integer>{}</integer>\n",
            dow
        ));
    }

    dict.push_str("    </dict>");

    Ok(dict)
}

/// Escapes special XML characters.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_to_calendar_interval() {
        let result = cron_to_calendar_interval("0 8 * * *").unwrap();
        assert!(result.contains("<key>Minute</key>"));
        assert!(result.contains("<integer>0</integer>"));
        assert!(result.contains("<key>Hour</key>"));
        assert!(result.contains("<integer>8</integer>"));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a < b"), "a &lt; b");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
    }
}
