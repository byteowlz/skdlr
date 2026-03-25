//! skdlr-service — Scheduler daemon that continuously runs the internal scheduler loop.
//!
//! This binary is the central schedule authority in container/multi-user mode.
//! It polls for due schedules, enqueues job instances, and dispatches execution
//! via the configured dispatcher (local process, HTTP runner, etc.).

use std::io::Write;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use tokio::sync::watch;

use skdlr_core::paths::AppPaths;
use skdlr_core::{LocalDispatcher, Scheduler, SchedulerConfig, SkdlrConfig, Storage};

/// skdlr scheduler daemon
#[derive(Parser, Debug)]
#[command(name = "skdlr-service", about = "skdlr scheduler daemon")]
struct Cli {
    /// Poll interval in seconds
    #[arg(long, default_value = "10")]
    poll_interval: u64,

    /// Lease duration in seconds for claimed jobs
    #[arg(long, default_value = "300")]
    lease_duration: u64,

    /// Stuck job check interval in seconds
    #[arg(long, default_value = "60")]
    stuck_check_interval: u64,

    /// Worker ID (auto-generated if not set)
    #[arg(long)]
    worker_id: Option<String>,

    /// Config file path (uses default if not set)
    #[arg(long)]
    config: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        let _ = writeln!(std::io::stderr(), "error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load config
    let paths = AppPaths::discover(None)?;
    let config = if let Some(config_path) = &cli.config {
        SkdlrConfig::load_from_path(std::path::Path::new(config_path))?
    } else {
        SkdlrConfig::load(&paths, false)?
    };

    // Open storage
    let storage = Arc::new(Mutex::new(Storage::open(&paths.db_path)?));

    // Create dispatcher (local for now, configurable later)
    let dispatcher = Arc::new(LocalDispatcher::new(&config));

    // Build scheduler config
    let scheduler_config = SchedulerConfig {
        poll_interval_secs: cli.poll_interval,
        lease_duration_secs: cli.lease_duration,
        stuck_check_interval_secs: cli.stuck_check_interval,
        worker_id: cli
            .worker_id
            .unwrap_or_else(|| format!("worker-{}", uuid::Uuid::new_v4())),
    };

    // Create scheduler
    let scheduler = Scheduler::new(storage, dispatcher, scheduler_config);

    // Set up shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Run the async runtime
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // Listen for SIGINT/SIGTERM
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::error!(error = %e, "Failed to install signal handler");
                return;
            }
            tracing::info!("Received shutdown signal");
            let _ = shutdown_tx_clone.send(true);
        });

        scheduler.run(shutdown_rx).await
    })?;

    Ok(())
}
