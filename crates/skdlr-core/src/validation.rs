//! Validation helpers for schedules and user input.

use chrono::{DateTime, Utc};

use crate::error::{Error, Result};
use crate::models::{Schedule, ScheduleKind};

/// Validates a schedule name for safe use in backend identifiers and paths.
pub fn validate_schedule_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::Validation(
            "schedule name cannot be empty".to_string(),
        ));
    }

    if name.len() > 64 {
        return Err(Error::Validation(
            "schedule name must be 64 characters or fewer".to_string(),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::Validation(
            "schedule name must contain only ASCII letters, digits, '-' or '_'".to_string(),
        ));
    }

    Ok(())
}

/// Validates a cron expression in 5-field format.
pub fn validate_cron_expression(expr: &str) -> Result<()> {
    let fields = expr.split_whitespace().count();
    if fields != 5 {
        return Err(Error::InvalidCron(format!(
            "expected 5 fields, got {}",
            fields
        )));
    }

    let normalized = format!("0 {}", expr);
    cron::Schedule::from_str(&normalized)
        .map(|_| ())
        .map_err(|e| Error::InvalidCron(e.to_string()))
}

/// Validates a one-off run_at timestamp.
pub fn validate_run_at(run_at: DateTime<Utc>) -> Result<()> {
    let now = Utc::now();
    if run_at <= now {
        return Err(Error::Validation(
            "one-off schedule run_at must be in the future".to_string(),
        ));
    }
    Ok(())
}

/// Validates the schedule kind (cron or one-off timestamp).
pub fn validate_schedule_kind(kind: &ScheduleKind) -> Result<()> {
    match kind {
        ScheduleKind::Recurring { cron_expr } => validate_cron_expression(cron_expr),
        ScheduleKind::OneOff { run_at } => validate_run_at(*run_at),
    }
}

/// Validates schedule fields used in backend files.
pub fn validate_schedule(schedule: &Schedule) -> Result<()> {
    validate_schedule_name(&schedule.name)?;
    validate_schedule_kind(&schedule.kind)?;

    reject_controls("command", &schedule.command)?;

    if let Some(description) = &schedule.description {
        reject_controls("description", description)?;
    }

    if let Some(workdir) = &schedule.workdir {
        reject_controls("workdir", workdir)?;
    }

    for (key, value) in &schedule.env {
        validate_env_key(key)?;
        reject_controls("env value", value)?;
    }

    Ok(())
}

fn reject_controls(field: &str, value: &str) -> Result<()> {
    if value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err(Error::Validation(format!(
            "{field} must not contain NUL or newline characters"
        )));
    }
    Ok(())
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(Error::Validation("env key cannot be empty".to_string()));
    }

    if !key.chars().enumerate().all(|(idx, c)| {
        if idx == 0 {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        }
    }) {
        return Err(Error::Validation(format!(
            "env key '{key}' must be ASCII letters, digits, or '_' and not start with a digit"
        )));
    }

    Ok(())
}

use std::str::FromStr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_schedule_name_accepts_safe_names() {
        validate_schedule_name("backup_daily").unwrap();
        validate_schedule_name("job-1").unwrap();
    }

    #[test]
    fn validate_schedule_name_rejects_bad_chars() {
        assert!(validate_schedule_name("bad/name").is_err());
        assert!(validate_schedule_name("bad name").is_err());
    }

    #[test]
    fn validate_cron_expression_rejects_bad_field_count() {
        assert!(validate_cron_expression("0 0 * *").is_err());
    }

    #[test]
    fn validate_schedule_rejects_control_chars() {
        let mut schedule = Schedule::new("test", "0 * * * *", "echo ok");
        schedule.description = Some("bad\nline".to_string());
        assert!(validate_schedule(&schedule).is_err());
    }
}
