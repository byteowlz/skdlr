//! SQLite storage for schedule metadata.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::models::{Run, RunStatus, Schedule, ScheduleKind, ScheduleStatus};
use crate::validation::validate_schedule;

/// SQLite-based storage for schedules and runs.
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Opens or creates a storage database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let storage = Self { conn };
        storage.init_schema()?;
        Ok(storage)
    }

    /// Opens an in-memory database (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let storage = Self { conn };
        storage.init_schema()?;
        Ok(storage)
    }

    /// Initializes the database schema.
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS schedules (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                description TEXT,
                cron_expr TEXT,
                run_at TEXT,
                command TEXT NOT NULL,
                workdir TEXT,
                env TEXT,
                status TEXT NOT NULL DEFAULT 'enabled',
                user TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                paused_until TEXT,
                backend_id TEXT
            );

            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                schedule_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                exit_code INTEGER,
                status TEXT NOT NULL,
                manual INTEGER NOT NULL DEFAULT 0,
                log_path TEXT,
                error TEXT,
                FOREIGN KEY (schedule_id) REFERENCES schedules(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_runs_schedule_id ON runs(schedule_id);
            CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at);
            CREATE INDEX IF NOT EXISTS idx_schedules_name ON schedules(name);
            ",
        )?;
        // Migrate existing databases: add run_at column if not exists
        let _ = self
            .conn
            .execute("ALTER TABLE schedules ADD COLUMN run_at TEXT", []);
        Ok(())
    }

    /// Saves a schedule (insert or update).
    pub fn save_schedule(&self, schedule: &Schedule) -> Result<()> {
        validate_schedule(schedule)?;
        let env_json = serde_json::to_string(&schedule.env)
            .map_err(|e| Error::Parse(format!("failed to serialize env: {e}")))?;

        // Extract cron_expr and run_at from the schedule kind
        let (cron_expr, run_at) = match &schedule.kind {
            ScheduleKind::Recurring { cron_expr } => (Some(cron_expr.clone()), None),
            ScheduleKind::OneOff { run_at } => (None, Some(run_at.to_rfc3339())),
        };

        self.conn.execute(
            "INSERT INTO schedules (id, name, description, cron_expr, run_at, command, workdir, env, status, user, created_at, updated_at, paused_until, backend_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                cron_expr = excluded.cron_expr,
                run_at = excluded.run_at,
                command = excluded.command,
                workdir = excluded.workdir,
                env = excluded.env,
                status = excluded.status,
                user = excluded.user,
                updated_at = excluded.updated_at,
                paused_until = excluded.paused_until,
                backend_id = excluded.backend_id",
            params![
                schedule.id.to_string(),
                &schedule.name,
                &schedule.description,
                cron_expr,
                run_at,
                &schedule.command,
                &schedule.workdir,
                env_json,
                schedule.status.to_string(),
                &schedule.user,
                schedule.created_at.to_rfc3339(),
                schedule.updated_at.to_rfc3339(),
                schedule.paused_until.map(|t| t.to_rfc3339()),
                &schedule.backend_id,
            ],
        )?;
        Ok(())
    }

    /// Gets a schedule by ID.
    pub fn get_schedule(&self, id: &Uuid) -> Result<Option<Schedule>> {
        self.conn
            .query_row(
                "SELECT id, name, description, cron_expr, run_at, command, workdir, env, status, user, created_at, updated_at, paused_until, backend_id
                 FROM schedules WHERE id = ?1",
                params![id.to_string()],
                |row| self.row_to_schedule(row),
            )
            .optional()
            .map_err(Error::from)
    }

    /// Gets a schedule by name.
    pub fn get_schedule_by_name(&self, name: &str) -> Result<Option<Schedule>> {
        self.conn
            .query_row(
                "SELECT id, name, description, cron_expr, run_at, command, workdir, env, status, user, created_at, updated_at, paused_until, backend_id
                 FROM schedules WHERE name = ?1",
                params![name],
                |row| self.row_to_schedule(row),
            )
            .optional()
            .map_err(Error::from)
    }

    /// Lists all schedules.
    pub fn list_schedules(&self) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, cron_expr, run_at, command, workdir, env, status, user, created_at, updated_at, paused_until, backend_id
             FROM schedules ORDER BY name",
        )?;

        let schedules = stmt
            .query_map([], |row| self.row_to_schedule(row))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(schedules)
    }

    /// Deletes a schedule by ID.
    pub fn delete_schedule(&self, id: &Uuid) -> Result<bool> {
        let count = self.conn.execute(
            "DELETE FROM schedules WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(count > 0)
    }

    /// Saves a run.
    pub fn save_run(&self, run: &Run) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, schedule_id, started_at, completed_at, exit_code, status, manual, log_path, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                completed_at = excluded.completed_at,
                exit_code = excluded.exit_code,
                status = excluded.status,
                log_path = excluded.log_path,
                error = excluded.error",
            params![
                run.id.to_string(),
                run.schedule_id.to_string(),
                run.started_at.to_rfc3339(),
                run.completed_at.map(|t| t.to_rfc3339()),
                run.exit_code,
                run.status.to_string(),
                run.manual,
                run.log_path,
                run.error,
            ],
        )?;
        Ok(())
    }

    /// Gets runs for a schedule.
    pub fn get_runs(&self, schedule_id: &Uuid, limit: usize) -> Result<Vec<Run>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, schedule_id, started_at, completed_at, exit_code, status, manual, log_path, error
             FROM runs WHERE schedule_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;

        let runs = stmt
            .query_map(params![schedule_id.to_string(), limit as i64], |row| {
                self.row_to_run(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(runs)
    }

    fn row_to_schedule(&self, row: &rusqlite::Row) -> rusqlite::Result<Schedule> {
        let id: String = row.get(0)?;
        let cron_expr: Option<String> = row.get(3)?;
        let run_at: Option<String> = row.get(4)?;
        let env_json: String = row.get(7)?;
        let status_str: String = row.get(8)?;
        let created_at: String = row.get(10)?;
        let updated_at: String = row.get(11)?;
        let paused_until: Option<String> = row.get(12)?;

        // Determine the schedule kind from cron_expr or run_at
        let kind = if let Some(cron) = cron_expr {
            ScheduleKind::Recurring { cron_expr: cron }
        } else if let Some(run_at_str) = run_at {
            let run_at_dt = parse_datetime(&run_at_str, 4)?;
            ScheduleKind::OneOff { run_at: run_at_dt }
        } else {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "schedule must have either cron_expr or run_at",
                )),
            ));
        };

        let schedule = Schedule {
            id: parse_uuid(&id, 0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            kind,
            command: row.get(5)?,
            workdir: row.get(6)?,
            env: parse_env(&env_json, 7)?,
            status: match status_str.as_str() {
                "disabled" => ScheduleStatus::Disabled,
                "paused" => ScheduleStatus::Paused,
                _ => ScheduleStatus::Enabled,
            },
            user: row.get(9)?,
            created_at: parse_datetime(&created_at, 10)?,
            updated_at: parse_datetime(&updated_at, 11)?,
            paused_until: paused_until
                .map(|value| parse_datetime(&value, 12))
                .transpose()?,
            backend_id: row.get(13)?,
        };

        // Skip validation for loaded schedules since run_at may now be in the past
        // The validation is for new schedules only

        Ok(schedule)
    }

    fn row_to_run(&self, row: &rusqlite::Row) -> rusqlite::Result<Run> {
        let id: String = row.get(0)?;
        let schedule_id: String = row.get(1)?;
        let started_at: String = row.get(2)?;
        let completed_at: Option<String> = row.get(3)?;
        let status_str: String = row.get(5)?;

        Ok(Run {
            id: parse_uuid(&id, 0)?,
            schedule_id: parse_uuid(&schedule_id, 1)?,
            started_at: parse_datetime(&started_at, 2)?,
            completed_at: completed_at
                .map(|value| parse_datetime(&value, 3))
                .transpose()?,
            exit_code: row.get(4)?,
            status: match status_str.as_str() {
                "succeeded" => RunStatus::Succeeded,
                "failed" => RunStatus::Failed,
                "cancelled" => RunStatus::Cancelled,
                _ => RunStatus::Running,
            },
            manual: row.get(6)?,
            log_path: row.get(7)?,
            error: row.get(8)?,
        })
    }
}

fn parse_uuid(value: &str, column: usize) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn parse_datetime(value: &str, column: usize) -> rusqlite::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
}

fn parse_env(
    value: &str,
    column: usize,
) -> rusqlite::Result<std::collections::HashMap<String, String>> {
    serde_json::from_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_schedule_crud() {
        let storage = Storage::in_memory().unwrap();

        let schedule = Schedule::new("test", "0 * * * *", "echo hello");
        storage.save_schedule(&schedule).unwrap();

        let loaded = storage.get_schedule(&schedule.id).unwrap().unwrap();
        assert_eq!(loaded.name, "test");
        assert_eq!(loaded.cron_expr(), Some("0 * * * *"));
        assert!(!loaded.is_one_off());

        let by_name = storage.get_schedule_by_name("test").unwrap().unwrap();
        assert_eq!(by_name.id, schedule.id);

        let all = storage.list_schedules().unwrap();
        assert_eq!(all.len(), 1);

        storage.delete_schedule(&schedule.id).unwrap();
        assert!(storage.get_schedule(&schedule.id).unwrap().is_none());
    }

    #[test]
    fn test_one_off_schedule_crud() {
        let storage = Storage::in_memory().unwrap();

        let run_at = Utc::now() + chrono::Duration::hours(1);
        let schedule = Schedule::new_one_off("one-time-test", run_at, "echo hello");
        storage.save_schedule(&schedule).unwrap();

        let loaded = storage.get_schedule(&schedule.id).unwrap().unwrap();
        assert_eq!(loaded.name, "one-time-test");
        assert!(loaded.is_one_off());
        assert!(loaded.cron_expr().is_none());
        // Compare timestamps with some tolerance (truncation to seconds)
        let loaded_run_at = loaded.run_at().unwrap();
        assert!((loaded_run_at - run_at).num_seconds().abs() < 2);

        storage.delete_schedule(&schedule.id).unwrap();
        assert!(storage.get_schedule(&schedule.id).unwrap().is_none());
    }

    #[test]
    fn test_runs() {
        let storage = Storage::in_memory().unwrap();

        let schedule = Schedule::new("test", "0 * * * *", "echo hello");
        storage.save_schedule(&schedule).unwrap();

        let mut run = Run::new(schedule.id, false);
        run.complete(0);
        storage.save_run(&run).unwrap();

        let runs = storage.get_runs(&schedule.id, 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Succeeded);
    }
}
