//! Dispatcher trait for transport-agnostic job execution.
//!
//! The dispatcher is responsible for executing jobs on behalf of the scheduler.
//! In single-user host mode, this runs commands directly. In multi-user/container
//! mode, it delegates to per-user runners via a transport layer (HTTP, gRPC, etc.).

use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use crate::models::{JobInstance, Schedule};

/// Result of a dispatched job execution.
#[derive(Debug, Clone)]
pub struct DispatchResult {
    /// Exit code from the executed command.
    pub exit_code: i32,

    /// Standard output (truncated if large).
    pub stdout: Option<String>,

    /// Standard error (truncated if large).
    pub stderr: Option<String>,

    /// Error message if dispatch itself failed (not the command).
    pub dispatch_error: Option<String>,
}

impl DispatchResult {
    /// Creates a successful dispatch result.
    pub fn success(exit_code: i32) -> Self {
        Self {
            exit_code,
            stdout: None,
            stderr: None,
            dispatch_error: None,
        }
    }

    /// Creates a failed dispatch result (dispatch itself failed).
    pub fn dispatch_failed(error: impl Into<String>) -> Self {
        Self {
            exit_code: -1,
            stdout: None,
            stderr: None,
            dispatch_error: Some(error.into()),
        }
    }

    /// Returns true if the command executed successfully (exit code 0).
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && self.dispatch_error.is_none()
    }

    /// Returns the error message (dispatch error or stderr).
    pub fn error_message(&self) -> Option<String> {
        if let Some(err) = &self.dispatch_error {
            return Some(err.clone());
        }
        if self.exit_code != 0 {
            return self.stderr.clone();
        }
        None
    }
}

/// A boxed future for async dispatch.
pub type DispatchFuture<'a> = Pin<Box<dyn Future<Output = Result<DispatchResult>> + Send + 'a>>;

/// Transport-agnostic dispatcher for job execution.
///
/// Implementations determine HOW jobs are executed:
/// - `LocalDispatcher`: Direct process execution on the host
/// - Future: HTTP-based dispatch to per-user runner agents
/// - Future: Container-based dispatch via container runtime
pub trait Dispatcher: Send + Sync {
    /// Dispatches a job for execution.
    ///
    /// The dispatcher receives the schedule (for command, workdir, env) and
    /// the job instance (for tracking). It returns a `DispatchResult` with
    /// the exit code and optional output.
    fn dispatch<'a>(
        &'a self,
        schedule: &'a Schedule,
        instance: &'a JobInstance,
    ) -> DispatchFuture<'a>;

    /// Returns a human-readable name for this dispatcher type.
    fn name(&self) -> &'static str;

    /// Checks if this dispatcher is healthy and ready to accept work.
    fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + '_>> {
        Box::pin(async { Ok(true) })
    }
}

/// Local process dispatcher — executes commands directly on the host.
#[derive(Debug)]
pub struct LocalDispatcher {
    config: crate::SkdlrConfig,
}

impl LocalDispatcher {
    /// Creates a new local dispatcher.
    pub fn new(config: &crate::SkdlrConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }
}

impl Dispatcher for LocalDispatcher {
    fn dispatch<'a>(
        &'a self,
        schedule: &'a Schedule,
        _instance: &'a JobInstance,
    ) -> DispatchFuture<'a> {
        Box::pin(async move {
            let (program, args) =
                match crate::backend::render_wrapped_command(schedule, &self.config) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        return Ok(DispatchResult::dispatch_failed(format!(
                            "failed to render command: {e}"
                        )));
                    }
                };

            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&args);

            if let Some(workdir) = &schedule.workdir {
                cmd.current_dir(workdir);
            }

            for (key, value) in &schedule.env {
                cmd.env(key, value);
            }

            match cmd.output().await {
                Ok(output) => {
                    let exit_code = output.status.code().unwrap_or(-1);
                    let stdout = if output.stdout.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(&output.stdout).into_owned())
                    };
                    let stderr = if output.stderr.is_empty() {
                        None
                    } else {
                        Some(String::from_utf8_lossy(&output.stderr).into_owned())
                    };

                    Ok(DispatchResult {
                        exit_code,
                        stdout,
                        stderr,
                        dispatch_error: None,
                    })
                }
                Err(e) => Ok(DispatchResult::dispatch_failed(format!(
                    "failed to execute command: {e}"
                ))),
            }
        })
    }

    fn name(&self) -> &'static str {
        "local"
    }
}
