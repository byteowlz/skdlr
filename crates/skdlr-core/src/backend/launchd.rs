//! macOS launchd backend.
//!
//! Uses launchd for scheduling via plist files in ~/Library/LaunchAgents/.
//!
//! # Run recording
//!
//! launchd executes jobs outside of any skdlr process, so generated plists
//! route the command through the skdlr binary itself:
//!
//! `skdlr __exec <name> -- <program> <args...>`
//!
//! The `__exec` handler records the run lifecycle in `SQLite` (creating the run
//! row if none was dispatched by `skdlr run`, completing it with the actual
//! exit code once the job exits). This gives `skdlr logs` reliable history for
//! both native scheduled runs and manual runs.
//!
//! # Environment
//!
//! `LaunchAgents` run with a stripped environment (no user PATH), so generated
//! plists embed `EnvironmentVariables` with the invoking user's `PATH` (plus
//! any per-schedule variables). Commands can rely on user-installed binaries
//! (e.g. Homebrew tools) resolving at execution time.

use std::path::PathBuf;

use tokio::process::Command;

use super::{Backend, BackendKind, BoxFuture};
use crate::SkdlrConfig;
use crate::error::{Error, Result};
use crate::models::{Run, Schedule};
use crate::validation::validate_schedule;

/// Fallback PATH for generated `LaunchAgents` when the invoking process has none.
const FALLBACK_PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Launchd backend for macOS.
#[derive(Debug)]
pub struct LaunchdBackend {
    /// Prefix for plist labels.
    service_prefix: String,
    /// Path to `LaunchAgents` directory.
    agents_dir: PathBuf,
    /// Configuration for executor wrapping.
    config: SkdlrConfig,
    /// Application paths (config file + metadata database) for run recording.
    paths: Option<crate::paths::AppPaths>,
}

/// Snapshot of `launchctl print` state for a job.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JobPrintState {
    /// Whether launchd reports the job as currently running.
    running: bool,
    /// The last exit code reported by launchd, if any.
    last_exit_code: Option<i32>,
}

impl LaunchdBackend {
    /// Creates a new launchd backend using default application paths.
    pub fn new(config: &SkdlrConfig) -> Self {
        Self::with_paths(config, None)
    }

    /// Creates a new launchd backend bound to explicit application paths.
    ///
    /// Binding paths is important when a custom `--config` is in use: the run
    /// recorder spawned by launchd does not inherit the shell environment and
    /// would otherwise resolve default paths instead of the configured ones.
    pub fn with_paths(config: &SkdlrConfig, paths: Option<&crate::paths::AppPaths>) -> Self {
        let agents_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join("Library/LaunchAgents");

        Self {
            service_prefix: config.service_prefix.clone(),
            agents_dir,
            config: config.clone(),
            paths: paths.cloned(),
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

    /// Returns the path to the skdlr binary used as the run recorder.
    fn recorder_path() -> Result<String> {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.to_str().map(str::to_string))
            .ok_or_else(|| Error::backend("could not determine skdlr binary path".to_string()))
    }

    /// Returns the recorder invocation for `ProgramArguments`.
    ///
    /// The generated argv routes execution through the skdlr recorder so runs
    /// are recorded and completed in `SQLite` regardless of who triggered
    /// them. The configured config file is passed explicitly because launchd
    /// does not inherit the invoking shell's environment or aliases.
    fn recorder_argv(&self, schedule: &Schedule) -> Result<Vec<String>> {
        let mut argv = vec![Self::recorder_path()?];
        if let Some(config_file) = self
            .paths
            .as_ref()
            .map(|paths| paths.config_file.to_string_lossy().into_owned())
        {
            argv.extend(["--config".to_string(), config_file]);
        }
        argv.extend([
            "__exec".to_string(),
            schedule.name.clone(),
            "--".to_string(),
        ]);
        argv.extend(super::render_launchd_args(schedule, &self.config)?);
        Ok(argv)
 }

    /// Returns the environment variables embedded in the generated plist.
    ///
    /// `LaunchAgents` inherit a stripped environment, so the invoking user's
    /// `PATH` is captured at install time (falling back to a sensible default
    /// covering Homebrew and system paths). Per-schedule variables take
    /// precedence over PATH if they redefine it.
    fn plist_env(schedule: &Schedule) -> Vec<(String, String)> {
        let mut env = vec![("PATH".to_string(), fallback_or_current_path())];
        for (key, value) in &schedule.env {
            env.push((key.clone(), value.clone()));
        }
        env
    }

    /// Generates the plist file content.
    fn generate_plist(&self, schedule: &Schedule) -> Result<String> {
        let argv = self.recorder_argv(schedule)?;

        // Convert args to plist XML array
        let args_array: Vec<String> = argv
            .iter()
            .map(|a| format!("        <string>{}</string>", escape_xml(a)))
            .collect();

        let cron_expr = schedule
            .cron_expr()
            .ok_or_else(|| Error::InvalidCron("schedule is not recurring".to_string()))?;
        let schedule_block = cron_to_schedule_block(cron_expr)?;

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
{}
    <key>StandardOutPath</key>
    <string>/tmp/{}.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/{}.err.log</string>
"#,
            self.label(schedule),
            args_array.join("\n"),
            schedule_block,
            self.label(schedule),
            self.label(schedule),
        );

        if let Some(workdir) = &schedule.workdir {
            plist.push_str(&format!(
                "    <key>WorkingDirectory</key>\n    <string>{}</string>\n",
                escape_xml(workdir)
            ));
        }

        let env = Self::plist_env(schedule);
        plist.push_str("    <key>EnvironmentVariables</key>\n    <dict>\n");
        for (key, value) in &env {
            plist.push_str(&format!(
                "        <key>{}</key>\n        <string>{}</string>\n",
                escape_xml(key),
                escape_xml(value)
            ));
        }
        plist.push_str("    </dict>\n");

        plist.push_str("</dict>\n</plist>\n");

        Ok(plist)
    }

    /// Opens a short-lived storage connection for run recording.
    fn open_storage(&self) -> Result<crate::Storage> {
        let path = self
            .paths
            .as_ref()
            .map(|paths| paths.db_path.clone())
            .or_else(|| crate::paths::AppPaths::discover(None).ok().map(|p| p.db_path))
            .ok_or_else(|| {
                Error::backend("storage path unavailable; cannot record runs".to_string())
            })?;
        crate::Storage::open(&path)
    }

    /// Runs launchctl with the given arguments.
    async fn launchctl(&self, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new("launchctl").args(args).output().await?;

        Ok(output)
    }

    /// Returns the current user's UID (for `launchctl print gui/<uid>/...`).
    async fn uid(&self) -> Result<String> {
        let output = Command::new("id").arg("-u").output().await?;
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if uid.is_empty() {
            return Err(Error::backend("could not determine user uid".to_string()));
        }
        Ok(uid)
    }

    /// Queries `launchctl print gui/<uid>/<label>` for the job state.
    ///
    /// Returns `Ok(None)` when the job is not loaded with launchd.
    async fn print_state(&self, schedule: &Schedule) -> Result<Option<JobPrintState>> {
        let uid = self.uid().await?;
        let target = format!("gui/{uid}/{}", self.label(schedule));
        let output = self.launchctl(&["print", &target]).await?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut running = false;
        let mut last_exit_code = None;

        for line in stdout.lines() {
            let line = line.trim();
            if let Some(state) = line.strip_prefix("state = ") {
                running = state == "running";
            } else if let Some(code) = line.strip_prefix("last exit code = ") {
                last_exit_code = code.trim().parse::<i32>().ok();
            }
        }

        Ok(Some(JobPrintState {
            running,
            last_exit_code,
        }))
    }

    fn plist_path_str(&self, schedule: &Schedule) -> Result<String> {
        let path = self.plist_path(schedule);
        path.to_str()
            .map(str::to_string)
            .ok_or_else(|| Error::backend("plist path is not valid UTF-8".to_string()))
    }
}

/// Returns the PATH to embed in generated plists.
fn fallback_or_current_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| FALLBACK_PATH.to_string())
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
            // Regenerate the plist so re-enabling an existing schedule picks
            // up current definitions (recorder wrapper, PATH, cron changes).
            let plist_content = self.generate_plist(schedule)?;
            tokio::fs::create_dir_all(&self.agents_dir).await?;
            tokio::fs::write(self.plist_path(schedule), plist_content).await?;

            // `launchctl load` fails on an already-loaded label; unload first
            // so the freshly written definition is picked up.
            if self.print_state(schedule).await?.is_some() {
                let _ = self.disable(schedule).await;
            }

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
            // Record the run BEFORE dispatching so the recorder process
            // (`skdlr __exec`) spawned by launchd can adopt and complete it.
            let run = Run::new(schedule.id, true);
            if let Ok(storage) = self.open_storage()
                && let Err(e) = storage.save_run(&run)
            {
                tracing::warn!("failed to persist launchd run record: {e}");
            }

            let label = self.label(schedule);
            let output = self.launchctl(&["start", &label]).await?;

            if !output.status.success() {
                let mut run = run;
                run.fail(format!(
                    "failed to start job: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
                if let Ok(storage) = self.open_storage() {
                    let _ = storage.save_run(&run);
                }
                return Ok(run);
            }

            Ok(run)
        })
    }

    fn is_running<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            Ok(self
                .print_state(schedule)
                .await?
                .is_some_and(|state| state.running))
        })
    }

    fn reconcile_stale_runs<'a>(
        &'a self,
        schedule: &'a Schedule,
        stale: &'a [Run],
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move {
            if stale.is_empty() {
                return Ok(Vec::new());
            }

            let Some(state) = self.print_state(schedule).await? else {
                // Job not loaded with launchd; nothing authoritative to say.
                return Ok(Vec::new());
            };
            if state.running {
                // launchd says the job is genuinely running; leave records alone.
                return Ok(Vec::new());
            }

            // The job has exited according to launchd — finalize the stale
            // records with the authoritative last exit code.
            let mut updated = Vec::new();
            for run in stale {
                let mut run = run.clone();
                match state.last_exit_code {
                    Some(code) => run.complete(code),
                    None => run.fail("job exited per launchd, but no exit code was reported"),
                }
                updated.push(run);
            }
            Ok(updated)
        })
    }

    fn get_runs<'a>(
        &'a self,
        _schedule: &'a Schedule,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move {
            // Run history lives in SQLite, recorded by `skdlr __exec` and
            // reconciled against `launchctl print` state. launchd itself does
            // not expose per-run history beyond the last exit code.
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

/// Maximum number of interval dicts emitted for one schedule.
const MAX_INTERVAL_DICTS: usize = 512;

/// Converts a cron expression into a launchd schedule XML block.
///
/// launchd cannot express cron-style step values (`*/15`) as a single
/// `StartCalendarInterval` dictionary. This expands steps, ranges, and lists
/// into an array of dictionaries; a fully wildcard expression falls back to
/// `StartInterval` of 60 seconds (every minute).
fn cron_to_schedule_block(cron_expr: &str) -> Result<String> {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(Error::InvalidCron(format!(
            "expected 5 fields, got {}",
            parts.len()
        )));
    }

    let minute = expand_cron_field(parts[0], 0, 59)?;
    let hour = expand_cron_field(parts[1], 0, 23)?;
    let day = expand_cron_field(parts[2], 1, 31)?;
    let month = expand_cron_field(parts[3], 1, 12)?;
    let dow = expand_cron_field(parts[4], 0, 7)?;

    // A fully wildcard expression means "every minute": StartCalendarInterval
    // cannot express that, but StartInterval can.
    if minute.is_none()
        && hour.is_none()
        && day.is_none()
        && month.is_none()
        && dow.is_none()
    {
        return Ok("    <key>StartInterval</key>\n    <integer>60</integer>".to_string());
    }

    let combos = cartesian(&[minute, hour, day, month, dow]);
    if combos.is_empty() {
        return Err(Error::InvalidCron(format!(
            "cron expression '{cron_expr}' matches no valid times"
        )));
    }
    if combos.len() > MAX_INTERVAL_DICTS {
        return Err(Error::InvalidCron(format!(
            "cron expression '{cron_expr}' expands to {} intervals, exceeds launchd practical limit of {MAX_INTERVAL_DICTS}",
            combos.len()
        )));
    }

    let keys = ["Minute", "Hour", "Day", "Month", "Weekday"];
    let dicts: Vec<String> = combos
        .iter()
        .map(|fields| {
            let mut dict = String::from("        <dict>\n");
            for (key, value) in keys.iter().zip(fields) {
                if let Some(value) = value {
                    dict.push_str(&format!(
                        "            <key>{key}</key>\n            <integer>{value}</integer>\n"
                    ));
                }
            }
            dict.push_str("        </dict>");
            dict
        })
        .collect();

    if dicts.len() == 1 {
        return Ok(format!(
            "    <key>StartCalendarInterval</key>\n{}",
            dicts[0]
        ));
    }

    Ok(format!(
        "    <key>StartCalendarInterval</key>\n    <array>\n{}\n    </array>",
        dicts.join("\n")
    ))
}

/// A per-field expansion: `None` means wildcard, otherwise the concrete values.
type FieldValues = Option<Vec<u32>>;

/// Expands one cron field into concrete values.
///
/// Supports `*`, steps (`*/N`, `A-B/N`), ranges (`A-B`), lists (`A,B,C`),
/// names for months and weekdays, and launchd's 0/7=Sunday convention.
fn expand_cron_field(field: &str, min: u32, max: u32) -> Result<FieldValues> {
    if field == "*" {
        return Ok(None);
    }

    let mut values = std::collections::BTreeSet::new();
    for part in field.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().map_err(|_| invalid_field(field))?;
                if step == 0 {
                    return Err(invalid_field(field));
                }
                (range, step)
            }
            None => (part, 1),
        };

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let parse_bound = |bound: &str, is_dow: bool| -> Result<u32> {
                if is_dow {
                    weekday_number(bound.trim())
                } else if min == 1 && max == 12 {
                    month_number(bound.trim())
                } else {
                    bound
                        .trim()
                        .parse::<u32>()
                        .map_err(|_| invalid_field(field))
                }
            };
            let is_dow = min == 0 && max == 7;
            (parse_bound(a, is_dow)?, parse_bound(b, is_dow)?)
        } else if step == 1 {
            // Single value (name or number)
            let value = if min == 0 && max == 7 {
                weekday_number(range_part.trim())?
            } else if min == 1 && max == 12 {
                month_number(range_part.trim())?
            } else {
                range_part
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| invalid_field(field))?
            };
            if value < min || value > max {
                return Err(invalid_field(field));
            }
            values.insert(value);
            continue;
        } else {
            return Err(invalid_field(field));
        };

        if start > end || end > max || start < min {
            return Err(invalid_field(field));
        }
        let mut value = start;
        loop {
            values.insert(value);
            match value.checked_add(step) {
                Some(next) if next <= end => value = next,
                _ => break,
            }
        }
    }

    if values.is_empty() {
        return Err(invalid_field(field));
    }
    Ok(Some(values.into_iter().collect()))
}

fn invalid_field(field: &str) -> Error {
    Error::InvalidCron(format!("unsupported cron field '{field}'"))
}

/// Maps a month name or number (1-12) to its numeric value.
fn month_number(value: &str) -> Result<u32> {
    const NAMES: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    if let Some(index) = NAMES.iter().position(|name| *name == value.to_lowercase()) {
        return Ok((index + 1) as u32);
    }
    value.parse::<u32>().map_err(|_| invalid_field(value))
}

/// Maps a weekday name or number (0-7, 0 and 7 = Sunday) to launchd's value.
fn weekday_number(value: &str) -> Result<u32> {
    const NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
    if let Some(index) = NAMES.iter().position(|name| *name == value.to_lowercase()) {
        return Ok(index as u32);
    }
    let number = value.parse::<u32>().map_err(|_| invalid_field(value))?;
    Ok(number % 7) // cron 7 == Sunday == launchd 0
}

/// Cartesian product across the five optional field value lists.
fn cartesian(fields: &[FieldValues; 5]) -> Vec<[Option<u32>; 5]> {
    let mut combos = vec![[None; 5]];
    for (index, field) in fields.iter().enumerate() {
        let mut next = Vec::new();
        for combo in &combos {
            match field {
                Some(values) => {
                    for value in values {
                        let mut combo = *combo;
                        combo[index] = Some(*value);
                        next.push(combo);
                    }
                }
                None => next.push(*combo),
            }
        }
        combos = next;
    }
    combos
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_to_schedule_block_fixed_time() {
        let result = cron_to_schedule_block("0 8 * * *").unwrap();
        assert!(result.contains("<key>StartCalendarInterval</key>"));
        assert!(result.contains("<key>Minute</key>"));
        assert!(result.contains("<integer>0</integer>"));
        assert!(result.contains("<key>Hour</key>"));
        assert!(result.contains("<integer>8</integer>"));
        assert!(!result.contains("<key>Day</key>"));
    }

    #[test]
    fn test_cron_step_expands_to_array_of_dicts() {
        // */15 is invalid as a raw launchd integer; it must expand.
        let result = cron_to_schedule_block("*/15 * * * *").unwrap();
        assert!(result.contains("<key>StartCalendarInterval</key>"));
        assert!(result.contains("<array>"));
        assert!(result.contains("<integer>0</integer>"));
        assert!(result.contains("<integer>15</integer>"));
        assert!(result.contains("<integer>30</integer>"));
        assert!(result.contains("<integer>45</integer>"));
        assert!(!result.contains("*/15"));
        // Four dicts (one per expanded minute value)
        assert_eq!(result.match_indices("<dict>").count(), 4);
    }

    #[test]
    fn test_cron_every_minute_uses_start_interval() {
        let result = cron_to_schedule_block("* * * * *").unwrap();
        assert!(result.contains("<key>StartInterval</key>"));
        assert!(result.contains("<integer>60</integer>"));
        assert!(!result.contains("StartCalendarInterval"));
    }

    #[test]
    fn test_cron_list_and_range() {
        let result = cron_to_schedule_block("5,25 0-2 * * *").unwrap();
        // 3 hours x 2 minutes = 6 dicts
        assert_eq!(result.match_indices("<dict>").count(), 6);
        assert!(result.contains("<integer>25</integer>"));
        assert!(result.contains("<integer>2</integer>"));
    }

    #[test]
    fn test_cron_names_and_sunday_seven() {
        let result = cron_to_schedule_block("0 9 * * MON").unwrap();
        assert!(result.contains("<key>Weekday</key>\n            <integer>1</integer>"));

        let result = cron_to_schedule_block("0 9 * * 7").unwrap();
        assert!(result.contains("<key>Weekday</key>\n            <integer>0</integer>"));

        let result = cron_to_schedule_block("0 9 1 JAN *").unwrap();
        assert!(result.contains("<key>Month</key>\n            <integer>1</integer>"));
    }

    #[test]
    fn test_cron_weekday_with_step() {
        let result = cron_to_schedule_block("*/10 9-17 * * 1-5").unwrap();
        // minutes 0..50 step 10 = 6, hours 9..17 = 9, weekdays 1-5 = 5
        assert_eq!(result.match_indices("<dict>").count(), 6 * 9 * 5);
    }

    #[test]
    fn test_cron_invalid_step_rejected() {
        assert!(cron_to_schedule_block("*/0 * * * *").is_err());
        assert!(cron_to_schedule_block("99 * * * *").is_err());
        assert!(cron_to_schedule_block("* * * *").is_err());
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a < b"), "a &lt; b");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
    }

    #[test]
    fn test_plist_env_includes_path() {
        let schedule = Schedule::new("test", "0 8 * * *", "echo hi");
        let env = LaunchdBackend::plist_env(&schedule);
        assert!(env.iter().any(|(k, _)| k == "PATH"));

        let schedule = schedule.with_env("PATH", "/custom/bin");
        let env = LaunchdBackend::plist_env(&schedule);
        assert!(env.contains(&("PATH".to_string(), "/custom/bin".to_string())));
    }

    #[test]
    fn test_generate_plist_uses_recorder_and_env() {
        let config = SkdlrConfig::default();
        let backend = LaunchdBackend::new(&config);
        let schedule = Schedule::new("test", "*/15 * * * *", "atuin sync")
            .with_workdir("/tmp")
            .with_env("FOO", "bar");

        let plist = backend.generate_plist(&schedule).unwrap();
        assert!(plist.contains("<string>__exec</string>"));
        assert!(plist.contains("<string>test</string>"));
        assert!(plist.contains("<key>PATH</key>"));
        assert!(plist.contains("<string>bar</string>"));
        assert!(plist.contains("<key>WorkingDirectory</key>"));
        assert!(plist.contains("<integer>15</integer>"));
        // Ensure XML is well-formed enough to parse
        plist::Value::from_reader_xml(std::io::Cursor::new(&plist)).unwrap();
    }

    #[test]
    fn test_generate_plist_every_minute() {
        let config = SkdlrConfig::default();
        let backend = LaunchdBackend::new(&config);
        let schedule = Schedule::new("always", "* * * * *", "echo hi");
        let plist = backend.generate_plist(&schedule).unwrap();
        plist::Value::from_reader_xml(std::io::Cursor::new(&plist)).unwrap();
    }
}
