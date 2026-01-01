//! skdlr CLI - Cross-platform task scheduler.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use skdlr_core::backend::{Backend, BackendKind, create_backend};
use skdlr_core::models::{Schedule, ScheduleStatus};
use skdlr_core::paths::AppPaths;
use skdlr_core::validation::{validate_cron_expression, validate_schedule, validate_schedule_name};
use skdlr_core::{SkdlrConfig, Storage};

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        let _ = writeln!(io::stderr(), "error: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    // Initialize paths and config
    let paths = AppPaths::discover(cli.config.clone())?;
    paths.ensure_directories()?;
    let config = SkdlrConfig::load(&paths, false)?;
    let storage = Storage::open(&paths.db_path)?;
    let backend = create_backend(config.backend_kind(), &config);

    match cli.command {
        Command::Add(cmd) => handle_add(&storage, backend.as_ref(), &config, cmd).await,
        Command::List(cmd) => handle_list(&storage, cmd),
        Command::Show(cmd) => handle_show(&storage, cmd),
        Command::Edit(cmd) => handle_edit(&storage, backend.as_ref(), cmd).await,
        Command::Remove(cmd) => handle_remove(&storage, backend.as_ref(), cmd).await,
        Command::Enable(cmd) => handle_enable(&storage, backend.as_ref(), cmd).await,
        Command::Disable(cmd) => handle_disable(&storage, backend.as_ref(), cmd).await,
        Command::Run(cmd) => handle_run(&storage, backend.as_ref(), cmd).await,
        Command::Logs(cmd) => handle_logs(&storage, cmd),
        Command::Status => handle_status(&storage, backend.as_ref()).await,
        Command::Next => handle_next(&storage, backend.as_ref()).await,
        Command::Backend => handle_backend(backend.as_ref()),
        Command::Doctor => handle_doctor(backend.as_ref()),
        Command::Completions { shell } => handle_completions(shell),
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "skdlr",
    author,
    version,
    about = "Cross-platform task scheduler with native OS integration",
    propagate_version = true
)]
struct Cli {
    /// Override the config file path
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,

    /// Output machine readable JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add a new schedule
    Add(AddCommand),

    /// List all schedules
    List(ListCommand),

    /// Show details of a schedule
    Show(ShowCommand),

    /// Edit an existing schedule
    Edit(EditCommand),

    /// Remove a schedule
    Remove(RemoveCommand),

    /// Enable a schedule
    Enable(NameArg),

    /// Disable a schedule
    Disable(NameArg),

    /// Trigger an immediate run
    Run(RunCommand),

    /// View execution history
    Logs(LogsCommand),

    /// Show status overview
    Status,

    /// Show upcoming runs
    Next,

    /// Show active backend
    Backend,

    /// Health check
    Doctor,

    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Clone, Args)]
struct AddCommand {
    /// Schedule name (used as identifier)
    name: String,

    /// Cron expression (e.g., "0 8 * * *" for daily at 8am)
    #[arg(short, long)]
    schedule: String,

    /// Command to execute
    #[arg(short, long)]
    command: String,

    /// Working directory
    #[arg(short, long)]
    workdir: Option<String>,

    /// Description
    #[arg(short, long)]
    description: Option<String>,

    /// Start enabled (default: true)
    #[arg(long, default_value = "true")]
    enabled: bool,
}

#[derive(Debug, Clone, Args)]
struct ListCommand {
    /// Filter by status
    #[arg(long)]
    status: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct ShowCommand {
    /// Schedule name
    name: String,
}

#[derive(Debug, Clone, Args)]
struct EditCommand {
    /// Schedule name
    name: String,

    /// New cron expression
    #[arg(short, long)]
    schedule: Option<String>,

    /// New command
    #[arg(short, long)]
    command: Option<String>,

    /// New working directory
    #[arg(short, long)]
    workdir: Option<String>,

    /// New description
    #[arg(short, long)]
    description: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct RemoveCommand {
    /// Schedule name
    name: String,

    /// Skip confirmation
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(Debug, Clone, Args)]
struct NameArg {
    /// Schedule name
    name: String,
}

#[derive(Debug, Clone, Args)]
struct RunCommand {
    /// Schedule name
    name: String,

    /// Show what would run without executing
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Args)]
struct LogsCommand {
    /// Schedule name
    name: String,

    /// Number of runs to show
    #[arg(long, default_value = "10")]
    last: usize,
}

async fn handle_add(
    storage: &Storage,
    backend: &dyn Backend,
    _config: &SkdlrConfig,
    cmd: AddCommand,
) -> Result<()> {
    validate_schedule_name(&cmd.name)?;

    // Check if schedule already exists
    if storage.get_schedule_by_name(&cmd.name)?.is_some() {
        anyhow::bail!("schedule '{}' already exists", cmd.name);
    }

    // Validate cron expression
    validate_cron_expression(&cmd.schedule)?;

    // Create schedule
    let mut schedule = Schedule::new(&cmd.name, &cmd.schedule, &cmd.command);
    if let Some(workdir) = cmd.workdir {
        schedule = schedule.with_workdir(workdir);
    }
    if let Some(desc) = cmd.description {
        schedule = schedule.with_description(desc);
    }
    if !cmd.enabled {
        schedule.status = ScheduleStatus::Disabled;
    }

    validate_schedule(&schedule)?;

    // Save to storage
    storage.save_schedule(&schedule)?;

    // Install in backend
    backend.install(&schedule).await?;

    if cmd.enabled {
        backend.enable(&schedule).await?;
    }

    println!("Created schedule '{}'", cmd.name);
    Ok(())
}

fn handle_list(storage: &Storage, _cmd: ListCommand) -> Result<()> {
    let schedules = storage.list_schedules()?;

    if schedules.is_empty() {
        println!("No schedules configured.");
        return Ok(());
    }

    println!("{:<20} {:<10} {:<20} COMMAND", "NAME", "STATUS", "SCHEDULE");
    println!("{}", "-".repeat(70));

    for schedule in schedules {
        let cmd_preview: String = schedule.command.chars().take(30).collect();
        println!(
            "{:<20} {:<10} {:<20} {}",
            schedule.name,
            schedule.status,
            schedule.cron_expr,
            if schedule.command.len() > 30 {
                format!("{}...", cmd_preview)
            } else {
                cmd_preview
            }
        );
    }

    Ok(())
}

fn handle_show(storage: &Storage, cmd: ShowCommand) -> Result<()> {
    let schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    println!("Name:        {}", schedule.name);
    println!("Status:      {}", schedule.status);
    println!("Schedule:    {}", schedule.cron_expr);
    println!("Command:     {}", schedule.command);
    if let Some(workdir) = &schedule.workdir {
        println!("Working dir: {}", workdir);
    }
    if let Some(desc) = &schedule.description {
        println!("Description: {}", desc);
    }
    println!("Created:     {}", schedule.created_at);
    println!("Updated:     {}", schedule.updated_at);

    Ok(())
}

async fn handle_edit(storage: &Storage, backend: &dyn Backend, cmd: EditCommand) -> Result<()> {
    let mut schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    let mut changed = false;

    if let Some(cron) = cmd.schedule {
        validate_cron_expression(&cron)?;
        schedule.cron_expr = cron;
        changed = true;
    }
    if let Some(command) = cmd.command {
        schedule.command = command;
        changed = true;
    }
    if let Some(workdir) = cmd.workdir {
        schedule.workdir = Some(workdir);
        changed = true;
    }
    if let Some(desc) = cmd.description {
        schedule.description = Some(desc);
        changed = true;
    }

    if !changed {
        println!("No changes specified.");
        return Ok(());
    }

    validate_schedule(&schedule)?;
    schedule.updated_at = chrono::Utc::now();
    storage.save_schedule(&schedule)?;

    // Reinstall in backend
    backend.uninstall(&schedule).await?;
    backend.install(&schedule).await?;
    if schedule.status == ScheduleStatus::Enabled {
        backend.enable(&schedule).await?;
    }

    println!("Updated schedule '{}'", cmd.name);
    Ok(())
}

async fn handle_remove(storage: &Storage, backend: &dyn Backend, cmd: RemoveCommand) -> Result<()> {
    let schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    if !cmd.yes {
        print!("Remove schedule '{}'? [y/N] ", cmd.name);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    backend.uninstall(&schedule).await?;
    storage.delete_schedule(&schedule.id)?;

    println!("Removed schedule '{}'", cmd.name);
    Ok(())
}

async fn handle_enable(storage: &Storage, backend: &dyn Backend, cmd: NameArg) -> Result<()> {
    let mut schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    schedule.status = ScheduleStatus::Enabled;
    schedule.updated_at = chrono::Utc::now();
    storage.save_schedule(&schedule)?;

    backend.enable(&schedule).await?;

    println!("Enabled schedule '{}'", cmd.name);
    Ok(())
}

async fn handle_disable(storage: &Storage, backend: &dyn Backend, cmd: NameArg) -> Result<()> {
    let mut schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    schedule.status = ScheduleStatus::Disabled;
    schedule.updated_at = chrono::Utc::now();
    storage.save_schedule(&schedule)?;

    backend.disable(&schedule).await?;

    println!("Disabled schedule '{}'", cmd.name);
    Ok(())
}

async fn handle_run(storage: &Storage, backend: &dyn Backend, cmd: RunCommand) -> Result<()> {
    let schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    if cmd.dry_run {
        println!("Would run: {}", schedule.command);
        if let Some(workdir) = &schedule.workdir {
            println!("In directory: {}", workdir);
        }
        return Ok(());
    }

    println!("Running '{}'...", cmd.name);
    let run = backend.run_now(&schedule).await?;
    storage.save_run(&run)?;

    println!("Started run {}", run.id);
    Ok(())
}

fn handle_logs(storage: &Storage, cmd: LogsCommand) -> Result<()> {
    let schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    let runs = storage.get_runs(&schedule.id, cmd.last)?;

    if runs.is_empty() {
        println!("No runs recorded for '{}'.", cmd.name);
        return Ok(());
    }

    println!(
        "{:<36} {:<20} {:<10} {:<6}",
        "ID", "STARTED", "STATUS", "EXIT"
    );
    println!("{}", "-".repeat(75));

    for run in runs {
        println!(
            "{:<36} {:<20} {:<10} {}",
            run.id,
            run.started_at.format("%Y-%m-%d %H:%M:%S"),
            run.status,
            run.exit_code.map(|c| c.to_string()).unwrap_or_default()
        );
    }

    Ok(())
}

async fn handle_status(storage: &Storage, backend: &dyn Backend) -> Result<()> {
    let schedules = storage.list_schedules()?;

    println!("Backend: {}", backend.kind());
    println!("Schedules: {}", schedules.len());
    println!();

    let enabled = schedules
        .iter()
        .filter(|s| s.status == ScheduleStatus::Enabled)
        .count();
    let disabled = schedules
        .iter()
        .filter(|s| s.status == ScheduleStatus::Disabled)
        .count();

    println!("  Enabled:  {}", enabled);
    println!("  Disabled: {}", disabled);

    Ok(())
}

async fn handle_next(storage: &Storage, backend: &dyn Backend) -> Result<()> {
    let schedules = storage.list_schedules()?;

    let mut upcoming: Vec<(String, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for schedule in schedules
        .iter()
        .filter(|s| s.status == ScheduleStatus::Enabled)
    {
        if let Ok(Some(next)) = backend.next_run(schedule).await {
            upcoming.push((schedule.name.clone(), next));
        }
    }

    upcoming.sort_by_key(|(_, t)| *t);

    if upcoming.is_empty() {
        println!("No upcoming runs.");
        return Ok(());
    }

    println!("{:<20} NEXT RUN", "NAME");
    println!("{}", "-".repeat(50));

    for (name, next) in upcoming.iter().take(10) {
        println!("{:<20} {}", name, next.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    Ok(())
}

fn handle_backend(backend: &dyn Backend) -> Result<()> {
    println!("Active backend: {}", backend.kind());
    println!("Available: {}", backend.is_available());
    Ok(())
}

fn handle_doctor(backend: &dyn Backend) -> Result<()> {
    println!("skdlr health check");
    println!();

    print!("Backend ({})... ", backend.kind());
    if backend.is_available() {
        println!("OK");
    } else {
        println!("UNAVAILABLE");
    }

    // Check detected backend
    let detected = BackendKind::detect();
    println!("Detected backend: {}", detected);

    Ok(())
}

fn handle_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "skdlr", &mut io::stdout());
    Ok(())
}
