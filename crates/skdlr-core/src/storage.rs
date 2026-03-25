//! SQLite-based storage for schedule metadata and job instances.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::models::{
    DEFAULT_TENANT_ID, JobInstance, JobState, Run, RunStatus, Schedule, ScheduleKind,
    ScheduleStatus,
};
use crate::validation::validate_schedule;

/// SQLite-based storage for schedules, runs, and job instances.
#[derive(Debug)]
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Opens or creates a storage database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
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

    /// Returns a reference to the inner connection (for transactions).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Initializes the database schema.
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS schedules (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL DEFAULT 'default',
                name TEXT NOT NULL,
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
                backend_id TEXT,
                max_retries INTEGER NOT NULL DEFAULT 0,
                retry_delay_secs INTEGER NOT NULL DEFAULT 30
            );

            -- Unique key is now (tenant_id, name) for multi-tenant isolation
            CREATE UNIQUE INDEX IF NOT EXISTS idx_schedules_tenant_name
                ON schedules(tenant_id, name);

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

            -- Job instances: lease/retry table for queued/running/succeeded/failed states
            CREATE TABLE IF NOT EXISTS job_instances (
                id TEXT PRIMARY KEY,
                schedule_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued',
                scheduled_at TEXT NOT NULL,
                claimed_at TEXT,
                claimed_by TEXT,
                lease_expires_at TEXT,
                started_at TEXT,
                completed_at TEXT,
                exit_code INTEGER,
                attempt INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 1,
                next_retry_at TEXT,
                last_error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (schedule_id) REFERENCES schedules(id) ON DELETE CASCADE
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_job_instances_idempotency
                ON job_instances(idempotency_key);
            CREATE INDEX IF NOT EXISTS idx_job_instances_state
                ON job_instances(state, scheduled_at);
            CREATE INDEX IF NOT EXISTS idx_job_instances_tenant
                ON job_instances(tenant_id, state);
            CREATE INDEX IF NOT EXISTS idx_job_instances_lease
                ON job_instances(state, lease_expires_at);
            CREATE INDEX IF NOT EXISTS idx_job_instances_schedule
                ON job_instances(schedule_id, scheduled_at);
            ",
        )?;

        // Migration: add new columns to existing databases
        self.migrate()?;
        Ok(())
    }

    /// Runs schema migrations for existing databases.
    fn migrate(&self) -> Result<()> {
        // Add run_at column if not exists
        let _ = self
            .conn
            .execute("ALTER TABLE schedules ADD COLUMN run_at TEXT", []);
        // Add tenant_id column if not exists
        let _ = self.conn.execute(
            "ALTER TABLE schedules ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'default'",
            [],
        );
        // Add retry columns
        let _ = self.conn.execute(
            "ALTER TABLE schedules ADD COLUMN max_retries INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE schedules ADD COLUMN retry_delay_secs INTEGER NOT NULL DEFAULT 30",
            [],
        );
        // Drop old unique index on name only (if exists) — replaced by (tenant_id, name)
        let _ = self
            .conn
            .execute("DROP INDEX IF EXISTS idx_schedules_name", []);
        Ok(())
    }

    // ── Schedule CRUD ─────────────────────────────────────────────────────────

    /// Saves a schedule (insert or update).
    pub fn save_schedule(&self, schedule: &Schedule) -> Result<()> {
        validate_schedule(schedule)?;
        let env_json = serde_json::to_string(&schedule.env)
            .map_err(|e| Error::Parse(format!("failed to serialize env: {e}")))?;

        let (cron_expr, run_at) = match &schedule.kind {
            ScheduleKind::Recurring { cron_expr } => (Some(cron_expr.clone()), None),
            ScheduleKind::OneOff { run_at } => (None, Some(run_at.to_rfc3339())),
        };

        self.conn.execute(
            "INSERT INTO schedules (id, tenant_id, name, description, cron_expr, run_at, command,
                workdir, env, status, user, created_at, updated_at, paused_until, backend_id,
                max_retries, retry_delay_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                tenant_id = excluded.tenant_id,
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
                backend_id = excluded.backend_id,
                max_retries = excluded.max_retries,
                retry_delay_secs = excluded.retry_delay_secs",
            params![
                schedule.id.to_string(),
                &schedule.tenant_id,
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
                schedule.max_retries,
                schedule.retry_delay_secs as i64,
            ],
        )?;
        Ok(())
    }

    /// Gets a schedule by ID.
    pub fn get_schedule(&self, id: &Uuid) -> Result<Option<Schedule>> {
        self.conn
            .query_row(
                "SELECT id, tenant_id, name, description, cron_expr, run_at, command, workdir,
                    env, status, user, created_at, updated_at, paused_until, backend_id,
                    max_retries, retry_delay_secs
                 FROM schedules WHERE id = ?1",
                params![id.to_string()],
                Self::row_to_schedule,
            )
            .optional()
            .map_err(Error::from)
    }

    /// Gets a schedule by name within a tenant.
    pub fn get_schedule_by_name(&self, name: &str) -> Result<Option<Schedule>> {
        self.get_schedule_by_name_for_tenant(DEFAULT_TENANT_ID, name)
    }

    /// Gets a schedule by name within a specific tenant.
    pub fn get_schedule_by_name_for_tenant(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<Schedule>> {
        self.conn
            .query_row(
                "SELECT id, tenant_id, name, description, cron_expr, run_at, command, workdir,
                    env, status, user, created_at, updated_at, paused_until, backend_id,
                    max_retries, retry_delay_secs
                 FROM schedules WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
                Self::row_to_schedule,
            )
            .optional()
            .map_err(Error::from)
    }

    /// Lists all schedules (default tenant).
    pub fn list_schedules(&self) -> Result<Vec<Schedule>> {
        self.list_schedules_for_tenant(DEFAULT_TENANT_ID)
    }

    /// Lists all schedules for a specific tenant.
    pub fn list_schedules_for_tenant(&self, tenant_id: &str) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tenant_id, name, description, cron_expr, run_at, command, workdir,
                env, status, user, created_at, updated_at, paused_until, backend_id,
                max_retries, retry_delay_secs
             FROM schedules WHERE tenant_id = ?1 ORDER BY name",
        )?;

        let schedules = stmt
            .query_map(params![tenant_id], Self::row_to_schedule)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(schedules)
    }

    /// Lists all schedules across all tenants.
    pub fn list_all_schedules(&self) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tenant_id, name, description, cron_expr, run_at, command, workdir,
                env, status, user, created_at, updated_at, paused_until, backend_id,
                max_retries, retry_delay_secs
             FROM schedules ORDER BY tenant_id, name",
        )?;

        let schedules = stmt
            .query_map([], Self::row_to_schedule)?
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

    // ── Run CRUD ──────────────────────────────────────────────────────────────

    /// Saves a run.
    pub fn save_run(&self, run: &Run) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, schedule_id, started_at, completed_at, exit_code, status,
                manual, log_path, error)
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
            "SELECT id, schedule_id, started_at, completed_at, exit_code, status, manual,
                log_path, error
             FROM runs WHERE schedule_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;

        let runs = stmt
            .query_map(params![schedule_id.to_string(), limit as i64], |row| {
                Self::row_to_run(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(runs)
    }

    // ── Job Instance CRUD ─────────────────────────────────────────────────────

    /// Enqueues a new job instance (idempotent via `idempotency_key`).
    ///
    /// Returns `Ok(Some(instance))` if created, `Ok(None)` if duplicate.
    pub fn enqueue_job(&self, instance: &JobInstance) -> Result<Option<JobInstance>> {
        let result = self.conn.execute(
            "INSERT OR IGNORE INTO job_instances (id, schedule_id, tenant_id, idempotency_key,
                state, scheduled_at, claimed_at, claimed_by, lease_expires_at, started_at,
                completed_at, exit_code, attempt, max_attempts, next_retry_at, last_error,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18)",
            params![
                instance.id.to_string(),
                instance.schedule_id.to_string(),
                &instance.tenant_id,
                &instance.idempotency_key,
                instance.state.to_string(),
                instance.scheduled_at.to_rfc3339(),
                instance.claimed_at.map(|t| t.to_rfc3339()),
                &instance.claimed_by,
                instance.lease_expires_at.map(|t| t.to_rfc3339()),
                instance.started_at.map(|t| t.to_rfc3339()),
                instance.completed_at.map(|t| t.to_rfc3339()),
                instance.exit_code,
                instance.attempt,
                instance.max_attempts,
                instance.next_retry_at.map(|t| t.to_rfc3339()),
                &instance.last_error,
                instance.created_at.to_rfc3339(),
                instance.updated_at.to_rfc3339(),
            ],
        )?;

        if result == 0 {
            Ok(None) // Duplicate idempotency key
        } else {
            Ok(Some(instance.clone()))
        }
    }

    /// Atomically claims a queued job instance.
    ///
    /// Uses `UPDATE ... WHERE state = 'queued'` for atomic claim semantics.
    /// Returns `Ok(Some(instance))` if claimed, `Ok(None)` if already claimed.
    pub fn claim_job(
        &self,
        worker_id: &str,
        lease_duration: chrono::Duration,
    ) -> Result<Option<JobInstance>> {
        let now = Utc::now();
        let lease_expires = now + lease_duration;

        // Atomically claim the oldest queued job (or a retrying job whose retry time has passed)
        let count = self.conn.execute(
            "UPDATE job_instances
             SET state = 'running',
                 claimed_at = ?1,
                 claimed_by = ?2,
                 lease_expires_at = ?3,
                 started_at = ?1,
                 updated_at = ?1
             WHERE id = (
                 SELECT id FROM job_instances
                 WHERE (state = 'queued' AND scheduled_at <= ?1)
                    OR (state = 'retrying' AND next_retry_at <= ?1)
                 ORDER BY scheduled_at ASC
                 LIMIT 1
             ) AND (state = 'queued' OR state = 'retrying')",
            params![now.to_rfc3339(), worker_id, lease_expires.to_rfc3339(),],
        )?;

        if count == 0 {
            return Ok(None);
        }

        // Fetch the claimed job
        let instance = self
            .conn
            .query_row(
                "SELECT id, schedule_id, tenant_id, idempotency_key, state, scheduled_at,
                    claimed_at, claimed_by, lease_expires_at, started_at, completed_at,
                    exit_code, attempt, max_attempts, next_retry_at, last_error,
                    created_at, updated_at
                 FROM job_instances WHERE claimed_by = ?1 AND state = 'running'
                 ORDER BY claimed_at DESC LIMIT 1",
                params![worker_id],
                Self::row_to_job_instance,
            )
            .optional()?;

        Ok(instance)
    }

    /// Renews the lease on a running job instance.
    pub fn renew_lease(
        &self,
        instance_id: &Uuid,
        worker_id: &str,
        lease_duration: chrono::Duration,
    ) -> Result<bool> {
        let now = Utc::now();
        let new_expires = now + lease_duration;

        let count = self.conn.execute(
            "UPDATE job_instances
             SET lease_expires_at = ?1, updated_at = ?2
             WHERE id = ?3 AND claimed_by = ?4 AND state = 'running'",
            params![
                new_expires.to_rfc3339(),
                now.to_rfc3339(),
                instance_id.to_string(),
                worker_id,
            ],
        )?;

        Ok(count > 0)
    }

    /// Marks a job instance as succeeded.
    pub fn complete_job(&self, instance_id: &Uuid, exit_code: i32) -> Result<bool> {
        let now = Utc::now();
        let state = if exit_code == 0 {
            "succeeded"
        } else {
            "failed"
        };

        let count = self.conn.execute(
            "UPDATE job_instances
             SET state = ?1, completed_at = ?2, exit_code = ?3, updated_at = ?2
             WHERE id = ?4 AND state = 'running'",
            params![state, now.to_rfc3339(), exit_code, instance_id.to_string(),],
        )?;

        Ok(count > 0)
    }

    /// Marks a job instance as failed, scheduling a retry if possible.
    pub fn fail_job(
        &self,
        instance_id: &Uuid,
        error: &str,
        retry_delay_secs: u64,
    ) -> Result<JobState> {
        let now = Utc::now();

        // Get current state
        let (attempt, max_attempts): (u32, u32) = self.conn.query_row(
            "SELECT attempt, max_attempts FROM job_instances WHERE id = ?1",
            params![instance_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let next_attempt = attempt + 1;
        let new_state = if next_attempt < max_attempts {
            JobState::Retrying
        } else {
            JobState::DeadLetter
        };

        let next_retry_at = if new_state == JobState::Retrying {
            let delay = retry_delay_secs * 2u64.saturating_pow(attempt);
            let capped = delay.min(3600);
            Some(now + chrono::Duration::seconds(capped as i64))
        } else {
            None
        };

        self.conn.execute(
            "UPDATE job_instances
             SET state = ?1, last_error = ?2, attempt = ?3, next_retry_at = ?4,
                 completed_at = ?5, updated_at = ?5,
                 claimed_at = NULL, claimed_by = NULL, lease_expires_at = NULL
             WHERE id = ?6 AND state = 'running'",
            params![
                new_state.to_string(),
                error,
                next_attempt,
                next_retry_at.map(|t| t.to_rfc3339()),
                now.to_rfc3339(),
                instance_id.to_string(),
            ],
        )?;

        Ok(new_state)
    }

    /// Recovers stuck jobs whose leases have expired.
    ///
    /// Returns the number of recovered jobs.
    pub fn recover_stuck_jobs(&self) -> Result<usize> {
        let now = Utc::now();

        let count = self.conn.execute(
            "UPDATE job_instances
             SET state = 'queued',
                 claimed_at = NULL,
                 claimed_by = NULL,
                 lease_expires_at = NULL,
                 started_at = NULL,
                 last_error = 'lease expired - recovered by stuck job recovery',
                 updated_at = ?1
             WHERE state = 'running' AND lease_expires_at < ?1",
            params![now.to_rfc3339()],
        )?;

        Ok(count)
    }

    /// Gets job instances for a schedule.
    pub fn get_job_instances(&self, schedule_id: &Uuid, limit: usize) -> Result<Vec<JobInstance>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, schedule_id, tenant_id, idempotency_key, state, scheduled_at,
                claimed_at, claimed_by, lease_expires_at, started_at, completed_at,
                exit_code, attempt, max_attempts, next_retry_at, last_error,
                created_at, updated_at
             FROM job_instances WHERE schedule_id = ?1
             ORDER BY scheduled_at DESC LIMIT ?2",
        )?;

        let instances = stmt
            .query_map(
                params![schedule_id.to_string(), limit as i64],
                Self::row_to_job_instance,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(instances)
    }

    /// Gets job instances by state for a tenant.
    pub fn get_job_instances_by_state(
        &self,
        tenant_id: &str,
        state: JobState,
        limit: usize,
    ) -> Result<Vec<JobInstance>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, schedule_id, tenant_id, idempotency_key, state, scheduled_at,
                claimed_at, claimed_by, lease_expires_at, started_at, completed_at,
                exit_code, attempt, max_attempts, next_retry_at, last_error,
                created_at, updated_at
             FROM job_instances WHERE tenant_id = ?1 AND state = ?2
             ORDER BY scheduled_at DESC LIMIT ?3",
        )?;

        let instances = stmt
            .query_map(
                params![tenant_id, state.to_string(), limit as i64],
                Self::row_to_job_instance,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(instances)
    }

    /// Gets dead-lettered job instances for a tenant.
    pub fn get_dead_letter_jobs(&self, tenant_id: &str, limit: usize) -> Result<Vec<JobInstance>> {
        self.get_job_instances_by_state(tenant_id, JobState::DeadLetter, limit)
    }

    /// Counts pending (queued + retrying) jobs.
    pub fn count_pending_jobs(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM job_instances WHERE state IN ('queued', 'retrying')",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    // ── Row mappers ───────────────────────────────────────────────────────────

    fn row_to_schedule(row: &rusqlite::Row) -> rusqlite::Result<Schedule> {
        let id: String = row.get(0)?;
        let tenant_id: String = row.get(1)?;
        let cron_expr: Option<String> = row.get(4)?;
        let run_at: Option<String> = row.get(5)?;
        let env_json: String = row.get(8)?;
        let status_str: String = row.get(9)?;
        let created_at: String = row.get(11)?;
        let updated_at: String = row.get(12)?;
        let paused_until: Option<String> = row.get(13)?;
        let max_retries: u32 = row.get::<_, Option<u32>>(15)?.unwrap_or(0);
        let retry_delay_secs: u64 = row.get::<_, Option<i64>>(16)?.unwrap_or(30) as u64;

        let kind = if let Some(cron) = cron_expr {
            ScheduleKind::Recurring { cron_expr: cron }
        } else if let Some(run_at_str) = run_at {
            let run_at_dt = parse_datetime(&run_at_str, 5)?;
            ScheduleKind::OneOff { run_at: run_at_dt }
        } else {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "schedule must have either cron_expr or run_at",
                )),
            ));
        };

        Ok(Schedule {
            id: parse_uuid(&id, 0)?,
            tenant_id,
            name: row.get(2)?,
            description: row.get(3)?,
            kind,
            command: row.get(6)?,
            workdir: row.get(7)?,
            env: parse_env(&env_json, 8)?,
            status: match status_str.as_str() {
                "disabled" => ScheduleStatus::Disabled,
                "paused" => ScheduleStatus::Paused,
                _ => ScheduleStatus::Enabled,
            },
            user: row.get(10)?,
            created_at: parse_datetime(&created_at, 11)?,
            updated_at: parse_datetime(&updated_at, 12)?,
            paused_until: paused_until
                .map(|value| parse_datetime(&value, 13))
                .transpose()?,
            backend_id: row.get(14)?,
            max_retries,
            retry_delay_secs,
        })
    }

    fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<Run> {
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

    fn row_to_job_instance(row: &rusqlite::Row) -> rusqlite::Result<JobInstance> {
        let id: String = row.get(0)?;
        let schedule_id: String = row.get(1)?;
        let tenant_id: String = row.get(2)?;
        let idempotency_key: String = row.get(3)?;
        let state_str: String = row.get(4)?;
        let scheduled_at: String = row.get(5)?;
        let claimed_at: Option<String> = row.get(6)?;
        let claimed_by: Option<String> = row.get(7)?;
        let lease_expires_at: Option<String> = row.get(8)?;
        let started_at: Option<String> = row.get(9)?;
        let completed_at: Option<String> = row.get(10)?;
        let exit_code: Option<i32> = row.get(11)?;
        let attempt: u32 = row.get(12)?;
        let max_attempts: u32 = row.get(13)?;
        let next_retry_at: Option<String> = row.get(14)?;
        let last_error: Option<String> = row.get(15)?;
        let created_at: String = row.get(16)?;
        let updated_at: String = row.get(17)?;

        let state = match state_str.as_str() {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "succeeded" => JobState::Succeeded,
            "failed" => JobState::Failed,
            "retrying" => JobState::Retrying,
            "dead_letter" => JobState::DeadLetter,
            "cancelled" => JobState::Cancelled,
            _ => JobState::Queued,
        };

        Ok(JobInstance {
            id: parse_uuid(&id, 0)?,
            schedule_id: parse_uuid(&schedule_id, 1)?,
            tenant_id,
            idempotency_key,
            state,
            scheduled_at: parse_datetime(&scheduled_at, 5)?,
            claimed_at: claimed_at.map(|v| parse_datetime(&v, 6)).transpose()?,
            claimed_by,
            lease_expires_at: lease_expires_at
                .map(|v| parse_datetime(&v, 8))
                .transpose()?,
            started_at: started_at.map(|v| parse_datetime(&v, 9)).transpose()?,
            completed_at: completed_at.map(|v| parse_datetime(&v, 10)).transpose()?,
            exit_code,
            attempt,
            max_attempts,
            next_retry_at: next_retry_at.map(|v| parse_datetime(&v, 14)).transpose()?,
            last_error,
            created_at: parse_datetime(&created_at, 16)?,
            updated_at: parse_datetime(&updated_at, 17)?,
        })
    }
}

fn parse_uuid(value: &str, column: usize) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn parse_datetime(value: &str, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
        assert_eq!(loaded.tenant_id, DEFAULT_TENANT_ID);
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
    fn test_tenant_isolation() {
        let storage = Storage::in_memory().unwrap();

        let s1 = Schedule::new("backup", "0 * * * *", "echo a").with_tenant("tenant-a");
        let s2 = Schedule::new("backup", "0 * * * *", "echo b").with_tenant("tenant-b");

        storage.save_schedule(&s1).unwrap();
        storage.save_schedule(&s2).unwrap();

        let a_schedules = storage.list_schedules_for_tenant("tenant-a").unwrap();
        assert_eq!(a_schedules.len(), 1);
        assert_eq!(a_schedules[0].command, "echo a");

        let b_schedules = storage.list_schedules_for_tenant("tenant-b").unwrap();
        assert_eq!(b_schedules.len(), 1);
        assert_eq!(b_schedules[0].command, "echo b");

        // Same name in different tenants should both exist
        let all = storage.list_all_schedules().unwrap();
        assert_eq!(all.len(), 2);

        // Name lookup is tenant-scoped
        let a_backup = storage
            .get_schedule_by_name_for_tenant("tenant-a", "backup")
            .unwrap()
            .unwrap();
        assert_eq!(a_backup.command, "echo a");
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

    #[test]
    fn test_job_instance_lifecycle() {
        let storage = Storage::in_memory().unwrap();

        let schedule = Schedule::new("test", "0 * * * *", "echo hello").with_retries(2, 30);
        storage.save_schedule(&schedule).unwrap();

        // Enqueue
        let scheduled_at = Utc::now();
        let instance = JobInstance::new(&schedule, scheduled_at);
        let result = storage.enqueue_job(&instance).unwrap();
        assert!(result.is_some());

        // Idempotency: second enqueue should return None
        let dup = storage.enqueue_job(&instance).unwrap();
        assert!(dup.is_none());

        // Claim
        let claimed = storage
            .claim_job("worker-1", chrono::Duration::minutes(5))
            .unwrap();
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert_eq!(claimed.state, JobState::Running);
        assert_eq!(claimed.claimed_by, Some("worker-1".to_string()));

        // Complete
        let ok = storage.complete_job(&claimed.id, 0).unwrap();
        assert!(ok);

        let instances = storage.get_job_instances(&schedule.id, 10).unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].state, JobState::Succeeded);
    }

    #[test]
    fn test_job_retry_and_dead_letter() {
        let storage = Storage::in_memory().unwrap();

        let schedule = Schedule::new("retry-test", "0 * * * *", "echo fail").with_retries(2, 10);
        storage.save_schedule(&schedule).unwrap();

        let scheduled_at = Utc::now();
        let instance = JobInstance::new(&schedule, scheduled_at);
        storage.enqueue_job(&instance).unwrap();

        // Claim and fail attempt 1
        let claimed = storage
            .claim_job("worker-1", chrono::Duration::minutes(5))
            .unwrap()
            .unwrap();
        let state = storage.fail_job(&claimed.id, "error 1", 10).unwrap();
        assert_eq!(state, JobState::Retrying);

        // Claim and fail attempt 2
        // Simulate retry time passing by directly updating next_retry_at
        storage
            .conn
            .execute(
                "UPDATE job_instances SET next_retry_at = ?1 WHERE id = ?2",
                params![
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
                    claimed.id.to_string(),
                ],
            )
            .unwrap();

        let claimed2 = storage
            .claim_job("worker-1", chrono::Duration::minutes(5))
            .unwrap()
            .unwrap();
        let state = storage.fail_job(&claimed2.id, "error 2", 10).unwrap();
        assert_eq!(state, JobState::Retrying);

        // Claim and fail attempt 3 — should dead-letter
        storage
            .conn
            .execute(
                "UPDATE job_instances SET next_retry_at = ?1 WHERE id = ?2",
                params![
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
                    claimed.id.to_string(),
                ],
            )
            .unwrap();

        let claimed3 = storage
            .claim_job("worker-1", chrono::Duration::minutes(5))
            .unwrap()
            .unwrap();
        let state = storage.fail_job(&claimed3.id, "error 3", 10).unwrap();
        assert_eq!(state, JobState::DeadLetter);

        // Should be in dead letter queue
        let dead = storage.get_dead_letter_jobs(DEFAULT_TENANT_ID, 10).unwrap();
        assert_eq!(dead.len(), 1);
    }

    #[test]
    fn test_stuck_job_recovery() {
        let storage = Storage::in_memory().unwrap();

        let schedule = Schedule::new("stuck-test", "0 * * * *", "echo hello");
        storage.save_schedule(&schedule).unwrap();

        let scheduled_at = Utc::now();
        let instance = JobInstance::new(&schedule, scheduled_at);
        storage.enqueue_job(&instance).unwrap();

        // Claim with very short lease
        let claimed = storage
            .claim_job("worker-dead", chrono::Duration::seconds(0))
            .unwrap()
            .unwrap();

        // Set lease to past
        storage
            .conn
            .execute(
                "UPDATE job_instances SET lease_expires_at = ?1 WHERE id = ?2",
                params![
                    (Utc::now() - chrono::Duration::seconds(60)).to_rfc3339(),
                    claimed.id.to_string(),
                ],
            )
            .unwrap();

        // Recover
        let recovered = storage.recover_stuck_jobs().unwrap();
        assert_eq!(recovered, 1);

        // Should be queued again
        let instances = storage.get_job_instances(&schedule.id, 10).unwrap();
        assert_eq!(instances[0].state, JobState::Queued);
    }
}
