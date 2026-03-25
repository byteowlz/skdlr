//! skdlr-api — HTTP API server for schedule CRUD, run history, and job management.
//!
//! Provides tenant-scoped REST endpoints for managing schedules, viewing run history,
//! pausing/resuming schedules, and inspecting job instances.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use skdlr_core::SkdlrConfig;
use skdlr_core::models::{
    DEFAULT_TENANT_ID, JobInstance, JobState, Run, Schedule, ScheduleKind, ScheduleStatus,
};
use skdlr_core::paths::AppPaths;
use skdlr_core::storage::Storage;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    storage: Arc<Mutex<Storage>>,
}

impl AppState {
    fn db(&self) -> std::sync::MutexGuard<'_, Storage> {
        self.storage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// ── CLI ───────────────────────────────────────────────────────────────────────

/// skdlr API server
#[derive(Debug, Parser)]
#[command(name = "skdlr-api", about = "HTTP API server for skdlr")]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    /// Config file path
    #[arg(long)]
    config: Option<String>,
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    if let Err(err) = try_main() {
        let _ = writeln!(io::stderr(), "error: {err:#}");
        std::process::exit(1);
    }
}

#[tokio::main]
async fn try_main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let paths = AppPaths::discover(None)?;
    let _config = if let Some(config_path) = &cli.config {
        SkdlrConfig::load_from_path(std::path::Path::new(config_path))?
    } else {
        SkdlrConfig::load(&paths, false)?
    };

    let storage = Arc::new(Mutex::new(Storage::open(&paths.db_path)?));
    let state = AppState { storage };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Health
        .route("/health", get(health))
        // Schedules (tenant-scoped)
        .route(
            "/api/v1/tenants/{tenant_id}/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/tenants/{tenant_id}/schedules/{name}",
            get(get_schedule).put(update_schedule).delete(delete_schedule),
        )
        .route(
            "/api/v1/tenants/{tenant_id}/schedules/{name}/pause",
            axum::routing::post(pause_schedule),
        )
        .route(
            "/api/v1/tenants/{tenant_id}/schedules/{name}/resume",
            axum::routing::post(resume_schedule),
        )
        // Runs
        .route(
            "/api/v1/tenants/{tenant_id}/schedules/{name}/runs",
            get(list_runs),
        )
        // Job instances
        .route(
            "/api/v1/tenants/{tenant_id}/schedules/{name}/jobs",
            get(list_jobs),
        )
        .route(
            "/api/v1/tenants/{tenant_id}/jobs/dead-letter",
            get(list_dead_letter),
        )
        // Default tenant shortcuts
        .route(
            "/api/v1/schedules",
            get(list_schedules_default).post(create_schedule_default),
        )
        .route(
            "/api/v1/schedules/{name}",
            get(get_schedule_default)
                .put(update_schedule_default)
                .delete(delete_schedule_default),
        )
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", cli.bind, cli.port).parse()?;
    tracing::info!("Starting API server on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

#[derive(Deserialize)]
struct CreateScheduleRequest {
    name: String,
    command: String,
    /// Cron expression for recurring schedules.
    cron_expr: Option<String>,
    /// ISO 8601 timestamp for one-off schedules.
    run_at: Option<DateTime<Utc>>,
    description: Option<String>,
    workdir: Option<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
    #[serde(default)]
    max_retries: u32,
    #[serde(default = "default_retry_delay")]
    retry_delay_secs: u64,
}

fn default_retry_delay() -> u64 {
    30
}

#[derive(Deserialize)]
struct UpdateScheduleRequest {
    command: Option<String>,
    cron_expr: Option<String>,
    run_at: Option<DateTime<Utc>>,
    description: Option<String>,
    workdir: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
    max_retries: Option<u32>,
    retry_delay_secs: Option<u64>,
}

#[derive(Deserialize)]
struct PauseRequest {
    /// Pause until this timestamp. If not set, pauses indefinitely (disables).
    until: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct PaginationQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

#[derive(Serialize)]
struct ScheduleResponse {
    id: String,
    tenant_id: String,
    name: String,
    description: Option<String>,
    kind: String,
    cron_expr: Option<String>,
    run_at: Option<DateTime<Utc>>,
    command: String,
    workdir: Option<String>,
    env: std::collections::HashMap<String, String>,
    status: String,
    user: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    paused_until: Option<DateTime<Utc>>,
    max_retries: u32,
    retry_delay_secs: u64,
}

impl From<Schedule> for ScheduleResponse {
    fn from(s: Schedule) -> Self {
        let (kind, cron_expr, run_at) = match &s.kind {
            ScheduleKind::Recurring { cron_expr } => {
                ("recurring".to_string(), Some(cron_expr.clone()), None)
            }
            ScheduleKind::OneOff { run_at } => ("one_off".to_string(), None, Some(*run_at)),
        };
        Self {
            id: s.id.to_string(),
            tenant_id: s.tenant_id,
            name: s.name,
            description: s.description,
            kind,
            cron_expr,
            run_at,
            command: s.command,
            workdir: s.workdir,
            env: s.env,
            status: s.status.to_string(),
            user: s.user,
            created_at: s.created_at,
            updated_at: s.updated_at,
            paused_until: s.paused_until,
            max_retries: s.max_retries,
            retry_delay_secs: s.retry_delay_secs,
        }
    }
}

#[derive(Serialize)]
struct RunResponse {
    id: String,
    schedule_id: String,
    started_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
    status: String,
    manual: bool,
    error: Option<String>,
}

impl From<Run> for RunResponse {
    fn from(r: Run) -> Self {
        Self {
            id: r.id.to_string(),
            schedule_id: r.schedule_id.to_string(),
            started_at: r.started_at,
            completed_at: r.completed_at,
            exit_code: r.exit_code,
            status: r.status.to_string(),
            manual: r.manual,
            error: r.error,
        }
    }
}

#[derive(Serialize)]
struct JobResponse {
    id: String,
    schedule_id: String,
    tenant_id: String,
    state: String,
    scheduled_at: DateTime<Utc>,
    claimed_by: Option<String>,
    attempt: u32,
    max_attempts: u32,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<JobInstance> for JobResponse {
    fn from(j: JobInstance) -> Self {
        Self {
            id: j.id.to_string(),
            schedule_id: j.schedule_id.to_string(),
            tenant_id: j.tenant_id,
            state: j.state.to_string(),
            scheduled_at: j.scheduled_at,
            claimed_by: j.claimed_by,
            attempt: j.attempt,
            max_attempts: j.max_attempts,
            last_error: j.last_error,
            created_at: j.created_at,
            updated_at: j.updated_at,
        }
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

type ApiResult<T> = std::result::Result<T, ApiError>;

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.0, Json(ErrorResponse { error: self.1 })).into_response()
    }
}

impl From<skdlr_core::Error> for ApiError {
    fn from(e: skdlr_core::Error) -> Self {
        match &e {
            skdlr_core::Error::ScheduleNotFound(_) => Self(StatusCode::NOT_FOUND, e.to_string()),
            skdlr_core::Error::ScheduleExists(_) => Self(StatusCode::CONFLICT, e.to_string()),
            skdlr_core::Error::Validation(_) | skdlr_core::Error::InvalidCron(_) => {
                Self(StatusCode::BAD_REQUEST, e.to_string())
            }
            _ => Self(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

// ── Schedule CRUD (tenant-scoped) ─────────────────────────────────────────────

async fn list_schedules(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
) -> ApiResult<Json<Vec<ScheduleResponse>>> {
    let schedules = state.db().list_schedules_for_tenant(&tenant_id)?;
    Ok(Json(schedules.into_iter().map(Into::into).collect()))
}

async fn create_schedule(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    Json(req): Json<CreateScheduleRequest>,
) -> ApiResult<(StatusCode, Json<ScheduleResponse>)> {
    // Check for duplicate
    if state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &req.name)?
        .is_some()
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!(
                "schedule '{}' already exists for tenant '{tenant_id}'",
                req.name
            ),
        ));
    }

    let schedule = match (req.cron_expr, req.run_at) {
        (Some(cron_expr), None) => Schedule::new(&req.name, &cron_expr, &req.command),
        (None, Some(run_at)) => Schedule::new_one_off(&req.name, run_at, &req.command),
        (Some(_), Some(_)) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "specify either cron_expr or run_at, not both".to_string(),
            ));
        }
        (None, None) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "must specify either cron_expr or run_at".to_string(),
            ));
        }
    };

    let schedule = schedule
        .with_tenant(&tenant_id)
        .with_retries(req.max_retries, req.retry_delay_secs);

    let schedule = if let Some(desc) = req.description {
        schedule.with_description(desc)
    } else {
        schedule
    };

    let mut schedule = if let Some(workdir) = req.workdir {
        schedule.with_workdir(workdir)
    } else {
        schedule
    };

    schedule.env = req.env;

    state.db().save_schedule(&schedule)?;
    Ok((StatusCode::CREATED, Json(schedule.into())))
}

async fn get_schedule(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
) -> ApiResult<Json<ScheduleResponse>> {
    let schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;
    Ok(Json(schedule.into()))
}

async fn update_schedule(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
    Json(req): Json<UpdateScheduleRequest>,
) -> ApiResult<Json<ScheduleResponse>> {
    let mut schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;

    if let Some(command) = req.command {
        schedule.command = command;
    }
    if let Some(cron_expr) = req.cron_expr {
        schedule.kind = ScheduleKind::Recurring { cron_expr };
    }
    if let Some(run_at) = req.run_at {
        schedule.kind = ScheduleKind::OneOff { run_at };
    }
    if let Some(description) = req.description {
        schedule.description = Some(description);
    }
    if let Some(workdir) = req.workdir {
        schedule.workdir = Some(workdir);
    }
    if let Some(env) = req.env {
        schedule.env = env;
    }
    if let Some(max_retries) = req.max_retries {
        schedule.max_retries = max_retries;
    }
    if let Some(retry_delay_secs) = req.retry_delay_secs {
        schedule.retry_delay_secs = retry_delay_secs;
    }

    schedule.updated_at = Utc::now();
    state.db().save_schedule(&schedule)?;
    Ok(Json(schedule.into()))
}

async fn delete_schedule(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;
    state.db().delete_schedule(&schedule.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pause_schedule(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
    Json(req): Json<PauseRequest>,
) -> ApiResult<Json<ScheduleResponse>> {
    let mut schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;

    if let Some(until) = req.until {
        schedule.status = ScheduleStatus::Paused;
        schedule.paused_until = Some(until);
    } else {
        schedule.status = ScheduleStatus::Disabled;
        schedule.paused_until = None;
    }
    schedule.updated_at = Utc::now();

    state.db().save_schedule(&schedule)?;
    Ok(Json(schedule.into()))
}

async fn resume_schedule(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
) -> ApiResult<Json<ScheduleResponse>> {
    let mut schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;

    schedule.status = ScheduleStatus::Enabled;
    schedule.paused_until = None;
    schedule.updated_at = Utc::now();

    state.db().save_schedule(&schedule)?;
    Ok(Json(schedule.into()))
}

// ── Runs ──────────────────────────────────────────────────────────────────────

async fn list_runs(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<Vec<RunResponse>>> {
    let schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;
    let runs = state.db().get_runs(&schedule.id, pagination.limit)?;
    Ok(Json(runs.into_iter().map(Into::into).collect()))
}

// ── Job instances ─────────────────────────────────────────────────────────────

async fn list_jobs(
    State(state): State<AppState>,
    Path((tenant_id, name)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<Vec<JobResponse>>> {
    let schedule = state
        .db()
        .get_schedule_by_name_for_tenant(&tenant_id, &name)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("schedule '{name}' not found"),
            )
        })?;
    let jobs = state
        .db()
        .get_job_instances(&schedule.id, pagination.limit)?;
    Ok(Json(jobs.into_iter().map(Into::into).collect()))
}

async fn list_dead_letter(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> ApiResult<Json<Vec<JobResponse>>> {
    let jobs = state
        .db()
        .get_dead_letter_jobs(&tenant_id, pagination.limit)?;
    Ok(Json(jobs.into_iter().map(Into::into).collect()))
}

// ── Default tenant shortcuts ──────────────────────────────────────────────────

async fn list_schedules_default(state: State<AppState>) -> ApiResult<Json<Vec<ScheduleResponse>>> {
    list_schedules(state, Path(DEFAULT_TENANT_ID.to_string())).await
}

async fn create_schedule_default(
    state: State<AppState>,
    body: Json<CreateScheduleRequest>,
) -> ApiResult<(StatusCode, Json<ScheduleResponse>)> {
    create_schedule(state, Path(DEFAULT_TENANT_ID.to_string()), body).await
}

async fn get_schedule_default(
    state: State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ScheduleResponse>> {
    get_schedule(state, Path((DEFAULT_TENANT_ID.to_string(), name))).await
}

async fn update_schedule_default(
    state: State<AppState>,
    Path(name): Path<String>,
    body: Json<UpdateScheduleRequest>,
) -> ApiResult<Json<ScheduleResponse>> {
    update_schedule(state, Path((DEFAULT_TENANT_ID.to_string(), name)), body).await
}

async fn delete_schedule_default(
    state: State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    delete_schedule(state, Path((DEFAULT_TENANT_ID.to_string(), name))).await
}
