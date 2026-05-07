//! skdlr CLI - Cross-platform task scheduler.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use skdlr_core::backend::{Backend, BackendKind, create_backend};
use skdlr_core::models::{Schedule, ScheduleKind, ScheduleStatus};
use skdlr_core::paths::AppPaths;
use skdlr_core::validation::{
    validate_cron_expression, validate_run_at, validate_schedule, validate_schedule_name,
};
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
        Command::Show(cmd) => handle_show(&storage, &cmd),
        Command::Edit(cmd) => handle_edit(&storage, backend.as_ref(), cmd).await,
        Command::Remove(cmd) => handle_remove(&storage, backend.as_ref(), cmd).await,
        Command::Enable(cmd) => handle_enable(&storage, backend.as_ref(), cmd).await,
        Command::Disable(cmd) => handle_disable(&storage, backend.as_ref(), cmd).await,
        Command::Run(cmd) => handle_run(&storage, backend.as_ref(), cmd).await,
        Command::Logs(cmd) => handle_logs(&storage, backend.as_ref(), &cmd).await,
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

    /// Schedule timing. Supports natural language like "every day at 9am", "hourly",
    /// "every monday at 8am", "weekly", or cron format "0 8 * * *". Mutually exclusive with --at.
    #[arg(short, long, conflicts_with = "at")]
    schedule: Option<String>,

    /// One-off run at specific time. Supports natural language like "tomorrow 9am",
    /// "in 2 hours", "next monday 14:00", "friday 3pm", or ISO format "2026-02-13 08:00".
    /// Mutually exclusive with --schedule.
    #[arg(long, conflicts_with = "schedule")]
    at: Option<String>,

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

    /// New schedule timing (only for recurring schedules). Supports natural language
    /// like "every day at 9am" or cron format. Mutually exclusive with --at.
    #[arg(short, long, conflicts_with = "at")]
    schedule: Option<String>,

    /// New one-off timestamp (only for one-off schedules). Supports natural language
    /// like "tomorrow 9am" or ISO format. Mutually exclusive with --schedule.
    #[arg(long, conflicts_with = "schedule")]
    at: Option<String>,

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

    // Create schedule based on --schedule (cron) or --at (one-off)
    let mut schedule = match (&cmd.schedule, &cmd.at) {
        (Some(schedule_str), None) => {
            // Recurring schedule - try natural language first, then cron
            let cron_expr = parse_schedule_input(schedule_str)?;
            validate_cron_expression(&cron_expr)?;
            Schedule::new(&cmd.name, &cron_expr, &cmd.command)
        }
        (None, Some(at_str)) => {
            // One-off schedule with timestamp
            let run_at = parse_datetime_input(at_str)?;
            validate_run_at(run_at)?;
            Schedule::new_one_off(&cmd.name, run_at, &cmd.command)
        }
        (None, None) => {
            anyhow::bail!("either --schedule or --at must be specified");
        }
        (Some(_), Some(_)) => {
            // This should be prevented by clap's conflicts_with, but handle it anyway
            anyhow::bail!("--schedule and --at are mutually exclusive");
        }
    };

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

    let schedule_type = if schedule.is_one_off() {
        "one-off"
    } else {
        "recurring"
    };
    println!("Created {} schedule '{}'", schedule_type, cmd.name);
    Ok(())
}

/// Parses a datetime string in various formats, including natural language.
fn parse_datetime_input(input: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    use chrono::{Local, TimeZone};

    let input_lower = input.to_lowercase();
    let input_trimmed = input_lower.trim();

    // Try natural language patterns first
    if let Some(dt) = parse_natural_datetime(input_trimmed) {
        return Ok(dt);
    }

    // Try RFC3339 format (e.g., "2026-02-13T08:00:00Z")
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }

    // Try common formats
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ];

    for fmt in formats {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(input, fmt) {
            // Assume local time, convert to UTC
            if let Some(local) = Local.from_local_datetime(&naive).single() {
                return Ok(local.with_timezone(&chrono::Utc));
            }
        }
    }

    // Try "tomorrow 8am", "monday 14:00", etc. with date and time parts
    if let Some(dt) = parse_date_with_time(input_trimmed) {
        return Ok(dt);
    }

    anyhow::bail!(
        "invalid datetime format: '{}'. Examples:\n  \
         - Natural: 'tomorrow 9am', 'in 2 hours', 'next monday 14:00', 'friday 3pm'\n  \
         - ISO 8601: '2026-02-13 08:00' or '2026-02-13T08:00:00Z'",
        input
    )
}

/// Parses natural language datetime expressions.
fn parse_natural_datetime(input: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{Duration, Local, TimeZone, Timelike};

    let now = Local::now();

    // Handle "in X minutes/hours/days"
    if let Some(rest) = input.strip_prefix("in ")
        && let Some(dt) = parse_relative_duration(rest, now)
    {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    // Handle "X minutes/hours/days from now"
    if let Some(rest) = input.strip_suffix(" from now")
        && let Some(dt) = parse_relative_duration(rest, now)
    {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    // Handle simple keywords without time
    match input {
        "now" => return Some(now.with_timezone(&chrono::Utc)),
        "midnight" | "tonight" => {
            let tomorrow = now.date_naive() + Duration::days(1);
            let dt = tomorrow.and_hms_opt(0, 0, 0)?;
            return Local
                .from_local_datetime(&dt)
                .single()
                .map(|d| d.with_timezone(&chrono::Utc));
        }
        "noon" => {
            let mut target = now.date_naive().and_hms_opt(12, 0, 0)?;
            if now.hour() >= 12 {
                target = (now.date_naive() + Duration::days(1)).and_hms_opt(12, 0, 0)?;
            }
            return Local
                .from_local_datetime(&target)
                .single()
                .map(|d| d.with_timezone(&chrono::Utc));
        }
        _ => {}
    }

    None
}

/// Parses relative duration like "2 hours", "30 minutes", "1 day"
fn parse_relative_duration(
    input: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::Duration;

    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let amount: i64 = parts[0].parse().ok()?;
    let unit = parts[1].trim_end_matches('s'); // Handle plural

    let duration = match unit {
        "minute" | "min" => Duration::minutes(amount),
        "hour" | "hr" => Duration::hours(amount),
        "day" => Duration::days(amount),
        "week" => Duration::weeks(amount),
        _ => return None,
    };

    Some(now + duration)
}

/// Parses date with optional time like "tomorrow 9am", "monday 14:00", "next friday 3pm"
fn parse_date_with_time(input: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{Datelike, Duration, Local, NaiveTime, TimeZone};

    let now = Local::now();
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.is_empty() {
        return None;
    }

    // Check for "next" prefix
    let (has_next, date_parts) = if parts[0] == "next" {
        (true, &parts[1..])
    } else {
        (false, &parts[..])
    };

    if date_parts.is_empty() {
        return None;
    }

    // Parse the date part
    let (target_date, time_idx) = match date_parts[0] {
        "today" => (now.date_naive(), 1),
        "tomorrow" => (now.date_naive() + Duration::days(1), 1),
        day_name => {
            if let Some(weekday) = parse_weekday(day_name) {
                let days_ahead = days_until_weekday(now.weekday(), weekday, has_next);
                (now.date_naive() + Duration::days(days_ahead), 1)
            } else {
                return None;
            }
        }
    };

    // Parse the time part (if provided)
    let time = if time_idx < date_parts.len() {
        parse_time_string(date_parts[time_idx])?
    } else {
        // Default to 9:00 AM if no time specified
        NaiveTime::from_hms_opt(9, 0, 0)?
    };

    let naive_dt = target_date.and_time(time);
    Local
        .from_local_datetime(&naive_dt)
        .single()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Parses weekday names
fn parse_weekday(s: &str) -> Option<chrono::Weekday> {
    use chrono::Weekday;
    match s {
        "monday" | "mon" => Some(Weekday::Mon),
        "tuesday" | "tue" | "tues" => Some(Weekday::Tue),
        "wednesday" | "wed" => Some(Weekday::Wed),
        "thursday" | "thu" | "thur" | "thurs" => Some(Weekday::Thu),
        "friday" | "fri" => Some(Weekday::Fri),
        "saturday" | "sat" => Some(Weekday::Sat),
        "sunday" | "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Calculates days until a target weekday
fn days_until_weekday(current: chrono::Weekday, target: chrono::Weekday, next_week: bool) -> i64 {
    let current_num = current.num_days_from_monday() as i64;
    let target_num = target.num_days_from_monday() as i64;

    let mut days = target_num - current_num;

    if days <= 0 || next_week {
        days += 7;
    }

    days
}

/// Parses time strings like "9am", "14:00", "3:30pm", "9:00"
fn parse_time_string(s: &str) -> Option<chrono::NaiveTime> {
    use chrono::NaiveTime;

    let s = s.to_lowercase();

    // Check for am/pm suffix
    let (time_part, is_pm) = if s.ends_with("am") {
        (&s[..s.len() - 2], false)
    } else if s.ends_with("pm") {
        (&s[..s.len() - 2], true)
    } else {
        (s.as_str(), false)
    };

    // Parse hour and optional minute
    let (hour, minute) = if time_part.contains(':') {
        let parts: Vec<&str> = time_part.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        (parts[0].parse::<u32>().ok()?, parts[1].parse::<u32>().ok()?)
    } else {
        (time_part.parse::<u32>().ok()?, 0)
    };

    // Adjust for PM
    let hour = if is_pm && hour < 12 {
        hour + 12
    } else if !is_pm && hour == 12 && s.ends_with("am") {
        0
    } else {
        hour
    };

    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// Parses a schedule string into a cron expression.
/// Supports natural language like "every day at 9am", "hourly", "every monday at 8am".
fn parse_schedule_input(input: &str) -> Result<String> {
    let input_lower = input.to_lowercase();
    let input_trimmed = input_lower.trim();

    // Try natural language first
    if let Some(cron) = parse_natural_schedule(input_trimmed) {
        return Ok(cron);
    }

    // Check if it looks like a valid cron expression (5 space-separated fields)
    if input.split_whitespace().count() == 5 {
        return Ok(input.to_string());
    }

    anyhow::bail!(
        "invalid schedule format: '{}'. Examples:\n  \
         - Natural: 'every day at 9am', 'hourly', 'every monday at 14:00', 'weekly on friday'\n  \
         - Cron: '0 9 * * *' (minute hour day month weekday)",
        input
    )
}

/// Parses natural language schedule expressions into cron format.
fn parse_natural_schedule(input: &str) -> Option<String> {
    parse_keyword_schedule(input)
        .or_else(|| parse_every_prefix(input))
        .or_else(|| parse_daily_prefix(input))
        .or_else(|| parse_weekly_prefix(input))
        .or_else(|| parse_day_group_schedule(input))
        .or_else(|| parse_bare_weekday(input))
}

/// Matches simple keyword schedules (hourly, daily, midnight, etc.).
fn parse_keyword_schedule(input: &str) -> Option<String> {
    match input {
        "hourly" | "every hour" => Some("0 * * * *".to_string()),
        "daily" | "every day" => Some("0 9 * * *".to_string()),
        "weekly" | "every week" => Some("0 9 * * 1".to_string()),
        "monthly" | "every month" => Some("0 9 1 * *".to_string()),
        "yearly" | "annually" | "every year" => Some("0 9 1 1 *".to_string()),
        "midnight" | "every midnight" | "daily at midnight" => Some("0 0 * * *".to_string()),
        "noon" | "every noon" | "daily at noon" => Some("0 12 * * *".to_string()),
        "weekdays" => Some("0 9 * * 1-5".to_string()),
        "weekends" => Some("0 9 * * 0,6".to_string()),
        _ => None,
    }
}

/// Matches "every ..." patterns (interval, day, weekday, month).
fn parse_every_prefix(input: &str) -> Option<String> {
    let rest = input.strip_prefix("every ")?;

    if let Some(cron) = parse_every_interval(rest) {
        return Some(cron);
    }

    // "every day at TIME" / "every day TIME"
    if let Some(time) = rest
        .strip_prefix("day at ")
        .or_else(|| rest.strip_prefix("day "))
        .and_then(parse_time_string)
    {
        return Some(format!("{} {} * * *", time.format("%M"), time.format("%H")));
    }

    if let Some(cron) = parse_every_weekday(rest) {
        return Some(cron);
    }

    parse_every_month(rest)
}

/// Matches "daily at TIME" / "daily TIME".
fn parse_daily_prefix(input: &str) -> Option<String> {
    let rest = input
        .strip_prefix("daily at ")
        .or_else(|| input.strip_prefix("daily "))?;
    let time = parse_time_string(rest)?;
    Some(format!("{} {} * * *", time.format("%M"), time.format("%H")))
}

/// Matches "weekly on ..." / "weekly ...".
fn parse_weekly_prefix(input: &str) -> Option<String> {
    let rest = input
        .strip_prefix("weekly on ")
        .or_else(|| input.strip_prefix("weekly "))?;
    parse_weekly_on(rest)
}

/// Matches "weekdays/weekends [at] TIME".
fn parse_day_group_schedule(input: &str) -> Option<String> {
    let (rest, days) = if let Some(r) = input
        .strip_prefix("weekdays at ")
        .or_else(|| input.strip_prefix("weekdays "))
    {
        (r, "1-5")
    } else if let Some(r) = input
        .strip_prefix("weekends at ")
        .or_else(|| input.strip_prefix("weekends "))
    {
        (r, "0,6")
    } else {
        return None;
    };
    let time = parse_time_string(rest)?;
    Some(format!(
        "{} {} * * {days}",
        time.format("%M"),
        time.format("%H")
    ))
}

/// Matches bare weekday names with optional time: "monday 9am" / "monday at 9am".
fn parse_bare_weekday(input: &str) -> Option<String> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let dow = weekday_to_cron(parts.first()?)?;

    let default_time = chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(chrono::NaiveTime::MIN);

    let time = if parts.len() > 1 {
        let time_str = if parts.len() > 2 && parts[1] == "at" {
            parts[2]
        } else {
            parts[1]
        };
        parse_time_string(time_str).unwrap_or(default_time)
    } else {
        default_time
    };

    Some(format!(
        "{} {} * * {}",
        time.format("%M"),
        time.format("%H"),
        dow
    ))
}

/// Parses "N minutes" or "N hours" into cron
fn parse_every_interval(input: &str) -> Option<String> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let amount: u32 = parts[0].parse().ok()?;
    let unit = parts[1].trim_end_matches('s');

    match unit {
        "minute" | "min" if amount > 0 => Some(format!("*/{amount} * * * *")),
        "hour" | "hr" if amount > 0 => Some(format!("0 */{amount} * * *")),
        _ => None,
    }
}

/// Parses "monday [at TIME]", "tuesday 9am", etc.
fn parse_every_weekday(input: &str) -> Option<String> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let dow = weekday_to_cron(parts[0])?;

    let time = if parts.len() > 1 {
        let time_str = if parts.len() > 2 && parts[1] == "at" {
            parts[2]
        } else {
            parts[1]
        };
        parse_time_string(time_str)?
    } else {
        chrono::NaiveTime::from_hms_opt(9, 0, 0)?
    };

    Some(format!(
        "{} {} * * {}",
        time.format("%M"),
        time.format("%H"),
        dow
    ))
}

/// Parses "month on DAY [at TIME]" or "month at TIME"
fn parse_every_month(input: &str) -> Option<String> {
    let input = input.strip_prefix("month ")?.trim();

    // "on the 1st at 9am" or "on 15 at 9am"
    if let Some(rest) = input.strip_prefix("on the ") {
        return parse_month_day_time(rest);
    }
    if let Some(rest) = input.strip_prefix("on ") {
        return parse_month_day_time(rest);
    }

    // "at TIME" - default to 1st of month
    if let Some(rest) = input.strip_prefix("at ")
        && let Some(time) = parse_time_string(rest)
    {
        return Some(format!("{} {} 1 * *", time.format("%M"), time.format("%H")));
    }

    None
}

/// Parses "15 at 9am" or "1st at noon"
fn parse_month_day_time(input: &str) -> Option<String> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    // Parse day, stripping ordinal suffixes
    let day_str = parts[0]
        .trim_end_matches("st")
        .trim_end_matches("nd")
        .trim_end_matches("rd")
        .trim_end_matches("th");
    let day: u32 = day_str.parse().ok()?;

    if !(1..=31).contains(&day) {
        return None;
    }

    let time = if parts.len() > 1 {
        let time_str = if parts.len() > 2 && parts[1] == "at" {
            parts[2]
        } else {
            parts[1]
        };
        parse_time_string(time_str)?
    } else {
        chrono::NaiveTime::from_hms_opt(9, 0, 0)?
    };

    Some(format!(
        "{} {} {} * *",
        time.format("%M"),
        time.format("%H"),
        day
    ))
}

/// Parses "friday at 9am" or "friday 9am" for weekly schedules
fn parse_weekly_on(input: &str) -> Option<String> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let dow = weekday_to_cron(parts[0])?;

    let time = if parts.len() > 1 {
        let time_str = if parts.len() > 2 && parts[1] == "at" {
            parts[2]
        } else {
            parts[1]
        };
        parse_time_string(time_str)?
    } else {
        chrono::NaiveTime::from_hms_opt(9, 0, 0)?
    };

    Some(format!(
        "{} {} * * {}",
        time.format("%M"),
        time.format("%H"),
        dow
    ))
}

/// Converts weekday name to cron day-of-week number
fn weekday_to_cron(s: &str) -> Option<&'static str> {
    match s {
        "sunday" | "sun" => Some("0"),
        "monday" | "mon" => Some("1"),
        "tuesday" | "tue" | "tues" => Some("2"),
        "wednesday" | "wed" => Some("3"),
        "thursday" | "thu" | "thur" | "thurs" => Some("4"),
        "friday" | "fri" => Some("5"),
        "saturday" | "sat" => Some("6"),
        _ => None,
    }
}

fn handle_list(storage: &Storage, _cmd: ListCommand) -> Result<()> {
    let schedules = storage.list_schedules()?;

    if schedules.is_empty() {
        println!("No schedules configured.");
        return Ok(());
    }

    println!(
        "{:<20} {:<10} {:<6} {:<24} COMMAND",
        "NAME", "STATUS", "TYPE", "SCHEDULE"
    );
    println!("{}", "-".repeat(85));

    for schedule in schedules {
        let cmd_preview: String = schedule.command.chars().take(25).collect();
        let (sched_type, sched_display) = match &schedule.kind {
            ScheduleKind::Recurring { cron_expr } => ("cron", cron_expr.clone()),
            ScheduleKind::OneOff { run_at } => {
                ("once", run_at.format("%Y-%m-%d %H:%M UTC").to_string())
            }
        };
        println!(
            "{:<20} {:<10} {:<6} {:<24} {}",
            schedule.name,
            schedule.status,
            sched_type,
            sched_display,
            if schedule.command.len() > 25 {
                format!("{}...", cmd_preview)
            } else {
                cmd_preview
            }
        );
    }

    Ok(())
}

fn handle_show(storage: &Storage, cmd: &ShowCommand) -> Result<()> {
    let schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    println!("Name:        {}", schedule.name);
    println!("Status:      {}", schedule.status);
    match &schedule.kind {
        ScheduleKind::Recurring { cron_expr } => {
            println!("Type:        recurring (cron)");
            println!("Schedule:    {}", cron_expr);
        }
        ScheduleKind::OneOff { run_at } => {
            println!("Type:        one-off");
            println!("Run at:      {}", run_at.format("%Y-%m-%d %H:%M:%S UTC"));
        }
    }
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

    // Handle schedule timing changes
    if let Some(schedule_str) = cmd.schedule {
        if schedule.is_one_off() {
            anyhow::bail!(
                "cannot set cron expression on a one-off schedule; use --at to change the run time"
            );
        }
        let cron_expr = parse_schedule_input(&schedule_str)?;
        validate_cron_expression(&cron_expr)?;
        schedule.kind = ScheduleKind::Recurring { cron_expr };
        changed = true;
    }
    if let Some(at_str) = cmd.at {
        if !schedule.is_one_off() {
            anyhow::bail!(
                "cannot set run time on a recurring schedule; use --schedule to change the cron expression"
            );
        }
        let run_at = parse_datetime_input(&at_str)?;
        validate_run_at(run_at)?;
        schedule.kind = ScheduleKind::OneOff { run_at };
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

async fn handle_logs(storage: &Storage, backend: &dyn Backend, cmd: &LogsCommand) -> Result<()> {
    let schedule = storage
        .get_schedule_by_name(&cmd.name)?
        .ok_or_else(|| anyhow::anyhow!("schedule '{}' not found", cmd.name))?;

    let mut runs = storage.get_runs(&schedule.id, cmd.last)?;

    // Native schedulers (e.g., systemd timers) can produce executions that are
    // not persisted in the local runs table yet. Fall back to backend logs.
    if runs.len() < cmd.last {
        let backend_runs = backend.get_runs(&schedule, cmd.last).await.unwrap_or_default();
        runs.extend(backend_runs);
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        runs.truncate(cmd.last);
    }

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
