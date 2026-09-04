//! Backend abstraction for OS-specific schedulers.
//!
//! Each backend implements the `Backend` trait which provides a unified
//! interface for creating, managing, and querying scheduled tasks.

use std::future::Future;
use std::pin::Pin;

use crate::SkdlrConfig;
use crate::error::Result;
use crate::models::{Run, Schedule};

// Platform-specific backend modules
#[cfg(target_os = "linux")]
pub mod systemd;

#[cfg(target_os = "macos")]
pub mod launchd;

#[cfg(target_os = "windows")]
pub mod schtasks;

pub mod internal;

/// Identifies which backend is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Linux systemd timers.
    Systemd,
    /// macOS launchd plists.
    Launchd,
    /// Windows Task Scheduler.
    Schtasks,
    /// Internal scheduler (fallback, runs as daemon).
    Internal,
}

impl BackendKind {
    /// Detects the appropriate backend for the current platform.
    pub fn detect() -> Self {
        #[cfg(target_os = "linux")]
        {
            if systemd::is_available() {
                return Self::Systemd;
            }
        }

        #[cfg(target_os = "macos")]
        {
            return Self::Launchd;
        }

        #[cfg(target_os = "windows")]
        {
            return Self::Schtasks;
        }

        Self::Internal
    }

    /// Returns a human-readable name for this backend.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::Schtasks => "schtasks",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A boxed future type for async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait for OS-specific scheduler backends.
///
/// Each backend must implement this trait to provide scheduling functionality.
/// Backends are responsible for:
/// - Creating timer/service/plist files as needed
/// - Enabling/disabling schedules
/// - Querying run history from native logs (e.g., journalctl)
pub trait Backend: Send + Sync {
    /// Returns the kind of backend.
    fn kind(&self) -> BackendKind;

    /// Installs a schedule into the native scheduler.
    ///
    /// This creates the necessary files (e.g., .timer/.service for systemd)
    /// but does not enable the schedule.
    fn install<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>>;

    /// Removes a schedule from the native scheduler.
    ///
    /// This removes all associated files and disables the schedule if enabled.
    fn uninstall<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>>;

    /// Enables a schedule so it will run at scheduled times.
    fn enable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>>;

    /// Disables a schedule so it will not run.
    fn disable<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<()>>;

    /// Triggers an immediate run of the schedule.
    fn run_now<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<Run>>;

    /// Checks if a schedule is currently running.
    fn is_running<'a>(&'a self, schedule: &'a Schedule) -> BoxFuture<'a, Result<bool>>;

    /// Gets the last N runs from native logs.
    fn get_runs<'a>(
        &'a self,
        schedule: &'a Schedule,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Run>>>;

    /// Reconciles stale `running` run records against the native scheduler.
    ///
    /// Backends that execute jobs outside the skdlr process (e.g. launchd)
    /// implement this to finalize run records using authoritative scheduler
    /// state. Returns the updated runs; the caller persists them. The default
    /// implementation is a no-op (records are completed by the dispatcher).
    fn reconcile_stale_runs<'a>(
        &'a self,
        _schedule: &'a Schedule,
        _stale: &'a [Run],
    ) -> BoxFuture<'a, Result<Vec<Run>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    /// Gets the next scheduled run time.
    fn next_run<'a>(
        &'a self,
        schedule: &'a Schedule,
    ) -> BoxFuture<'a, Result<Option<chrono::DateTime<chrono::Utc>>>>;

    /// Checks if the backend is available on this system.
    fn is_available(&self) -> bool;
}

/// Creates a backend instance for the current platform.
///
/// See [`create_backend_with_paths`] to pass explicit application paths so
/// backends can locate the shared metadata database (required by the launchd
/// run recorder when a custom `--config` is in use).
pub fn create_backend(kind: BackendKind, config: &crate::SkdlrConfig) -> Box<dyn Backend> {
    create_backend_with_paths(kind, config, None)
}

/// Creates a backend instance for the current platform, optionally binding it
/// to explicit application paths.
pub fn create_backend_with_paths(
    kind: BackendKind,
    config: &crate::SkdlrConfig,
    paths: Option<&crate::paths::AppPaths>,
) -> Box<dyn Backend> {
    match kind {
        #[cfg(target_os = "linux")]
        BackendKind::Systemd => Box::new(systemd::SystemdBackend::new(config)),

        #[cfg(target_os = "macos")]
        BackendKind::Launchd => {
            Box::new(launchd::LaunchdBackend::with_paths(config, paths))
        }

        #[cfg(target_os = "windows")]
        BackendKind::Schtasks => Box::new(schtasks::SchtasksBackend::new(config)),

        BackendKind::Internal => Box::new(internal::InternalBackend::new(config)),

        // Fallback for non-matching platforms
        #[allow(unreachable_patterns)]
        _ => Box::new(internal::InternalBackend::new(config)),
    }
}

/// Renders a schedule's command with optional executor wrapper.
///
/// Returns (program, args) where `args` includes the scheduled command
/// as the final element(s).
///
/// Supports placeholders in `wrapper_args`:
/// - `{name}` - Schedule name
/// - `{workdir}` - Working directory
///
/// If `SKDLR_OCTO_MODE` is set and no wrapper is configured,
/// this returns an error.
pub fn render_wrapped_command(
    schedule: &Schedule,
    config: &SkdlrConfig,
) -> Result<(String, Vec<String>)> {
    // Validate executor config
    config.executor.validate()?;

    if let Some(wrapper) = &config.executor.wrapper {
        // Expand placeholders in wrapper args
        let mut args: Vec<String> = config
            .executor
            .wrapper_args
            .iter()
            .map(|arg| {
                arg.replace("{name}", &schedule.name)
                    .replace("{workdir}", schedule.workdir.as_deref().unwrap_or("."))
            })
            .collect();

        // Add delimiter and then the original command
        if wrapper_args_has_delimiter(&config.executor.wrapper_args) {
            // Wrapper already has delimiter, just append command
            args.push(schedule.command.clone());
        } else {
            // Add delimiter to separate wrapper args from command
            args.push("--".to_string());
            args.push(schedule.command.clone());
        }

        Ok((wrapper.clone(), args))
    } else {
        // No wrapper configured - use direct execution
        // Note: In Octo mode (SKDLR_OCTO_MODE), this should have been caught by validate()
        Ok((
            "/bin/sh".to_string(),
            vec!["-c".to_string(), format!("exec {}", schedule.command)],
        ))
    }
}

/// Checks if wrapper args contain a delimiter like "--".
fn wrapper_args_has_delimiter(args: &[String]) -> bool {
    args.iter().any(|a| a == "--" || a.starts_with("--"))
}

/// Renders a schedule's command as a single string for systemd `ExecStart`.
/// This is a convenience wrapper around `render_wrapped_command`.
///
/// For systemd `ExecStart`, we need to be careful about quoting:
/// - When using /bin/sh -c 'script', the 'script' is passed as a single argument
/// - The command and its arguments should be properly shell-escaped within that script
/// - We use exec to replace the shell process with the actual command
pub fn render_exec_start(schedule: &Schedule, config: &SkdlrConfig) -> Result<String> {
    let (program, args) = render_wrapped_command(schedule, config)?;

    // If the program is already /bin/sh, we're using shell execution
    // The args will be: ["-c", "actual_script"]
    // We need to build: /bin/sh -c 'actual_script'
    // Where actual_script is the args after -c, properly escaped

    if program == "/bin/sh" && args.first().map(String::as_str) == Some("-c") {
        // The args are usually ["-c", "script_content"], but wrapper configurations
        // may append additional tokens. Preserve the entire script payload.
        let script_content = if args.len() <= 1 {
            String::new()
        } else {
            args[1..].join(" ")
        };
        let escaped = script_content.replace('\'', "'\\''");
        return Ok(format!("ExecStart=/bin/sh -c '{escaped}'"));
    }

    // For other programs, build: /bin/sh -c 'program args...'
    // Simple shell escaping for single quotes
    let escaped_program = program.replace('\'', "'\\''");
    let escaped_args = args
        .iter()
        .map(|a| a.replace('\'', "'\\''"))
        .collect::<Vec<_>>()
        .join(" ");

    Ok(format!(
        "ExecStart=/bin/sh -c '{escaped_program} {escaped_args}'"
    ))
}

/// Renders arguments for launchd `ProgramArguments`.
pub fn render_launchd_args(schedule: &Schedule, config: &SkdlrConfig) -> Result<Vec<String>> {
    let (program, args) = render_wrapped_command(schedule, config)?;
    let mut full_args = vec![program];
    full_args.extend(args);
    Ok(full_args)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn render_exec_start_direct_shell_exec() {
        let schedule = Schedule::new("test", "* * * * *", "/tmp/run-me.sh");
        let config = SkdlrConfig::default();

        let rendered = render_exec_start(&schedule, &config).unwrap();
        assert_eq!(rendered, "ExecStart=/bin/sh -c 'exec /tmp/run-me.sh'");
    }

    #[test]
    fn render_exec_start_preserves_shell_payload_tokens() {
        let schedule = Schedule::new("test", "* * * * *", "/tmp/run-me.sh");
        let mut config = SkdlrConfig::default();
        config.executor.wrapper = Some("/bin/sh".to_string());
        config.executor.wrapper_args = vec!["-c".to_string(), "exec".to_string()];

        let rendered = render_exec_start(&schedule, &config).unwrap();
        assert_eq!(rendered, "ExecStart=/bin/sh -c 'exec -- /tmp/run-me.sh'");
    }
}
