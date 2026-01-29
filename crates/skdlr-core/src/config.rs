//! Configuration types and loading for skdlr.

use std::path::Path;

use anyhow::Result;
use config::{Config, Environment, File, FileFormat};
use serde::{Deserialize, Serialize};

use crate::backend::BackendKind;
use crate::env_prefix;
use crate::paths::{AppPaths, expand_str_path, write_default_config};

/// Main skdlr configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkdlrConfig {
    /// Backend to use (auto-detected if not set).
    pub backend: Option<String>,

    /// Prefix for service/timer names.
    pub service_prefix: String,

    /// Default working directory for schedules.
    pub default_workdir: Option<String>,

    /// Logging configuration.
    pub logging: LoggingConfig,

    /// Internal backend configuration.
    pub internal: InternalConfig,

    /// Executor wrapper configuration.
    pub executor: ExecutorConfig,
}

impl SkdlrConfig {
    /// Load configuration from file and environment.
    pub fn load(paths: &AppPaths, dry_run: bool) -> Result<Self> {
        if !paths.config_file.exists() {
            if dry_run {
                log::info!(
                    "dry-run: would create default config at {}",
                    paths.config_file.display()
                );
            } else {
                write_default_config(&paths.config_file)?;
            }
        }

        Self::load_from_path(&paths.config_file)
    }

    /// Load configuration from a specific path.
    pub fn load_from_path(config_file: &Path) -> Result<Self> {
        let env_prefix = env_prefix();
        let built = Config::builder()
            .set_default("service_prefix", "skdlr")?
            .set_default("logging.level", "info")?
            .set_default("internal.check_interval_secs", 60_i64)?
            .add_source(
                File::from(config_file)
                    .format(FileFormat::Toml)
                    .required(false),
            )
            .add_source(Environment::with_prefix(&env_prefix).separator("__"))
            .build()?;

        let mut config: SkdlrConfig = built.try_deserialize()?;

        if let Some(ref workdir) = config.default_workdir {
            let expanded = expand_str_path(workdir)?;
            config.default_workdir = Some(expanded.display().to_string());
        }

        Ok(config)
    }

    /// Returns the configured backend kind, or auto-detects.
    pub fn backend_kind(&self) -> BackendKind {
        match self.backend.as_deref() {
            Some("systemd") => BackendKind::Systemd,
            Some("launchd") => BackendKind::Launchd,
            Some("schtasks") => BackendKind::Schtasks,
            Some("internal") => BackendKind::Internal,
            _ => BackendKind::detect(),
        }
    }
}

impl Default for SkdlrConfig {
    fn default() -> Self {
        Self {
            backend: None,
            service_prefix: "skdlr".to_string(),
            default_workdir: None,
            logging: LoggingConfig::default(),
            internal: InternalConfig::default(),
            executor: ExecutorConfig::default(),
        }
    }
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error).
    pub level: String,
    /// Optional log file path.
    pub file: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file: None,
        }
    }
}

/// Internal backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InternalConfig {
    /// Check interval in seconds.
    pub check_interval_secs: u64,
}

impl Default for InternalConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 60,
        }
    }
}

/// Executor wrapper configuration for running scheduled commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutorConfig {
    /// Wrapper binary to run all commands through (e.g., "octo-sandbox").
    pub wrapper: Option<String>,

    /// Arguments passed to wrapper before the scheduled command.
    /// Supports placeholders: {name}, {workdir}, {command}
    #[serde(default)]
    pub wrapper_args: Vec<String>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            wrapper: None,
            wrapper_args: Vec::new(),
        }
    }
}

impl ExecutorConfig {
    /// Checks if wrapper is enabled and required (in Octo mode).
    pub fn is_required(&self) -> bool {
        std::env::var("SKDLR_OCTO_MODE").is_ok()
    }

    /// Returns an error if wrapper is required but not configured.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.is_required() && self.wrapper.is_none() {
            return Err(crate::error::Error::config(
                "Executor wrapper is required in Octo mode (SKDLR_OCTO_MODE is set), \
                but no wrapper is configured. Add [executor] section with 'wrapper' in config.",
            ));
        }
        Ok(())
    }
}
