# Runtime Modes

skdlr supports two runtime modes depending on your deployment:

## Host Mode (Single-User)

Default mode. Uses native OS schedulers (systemd, launchd, schtasks) or the internal scheduler.

```bash
# CLI manages schedules directly
skdlr add "backup" --schedule "0 2 * * *" --command "restic backup ~"
skdlr list
```

All schedules belong to the `default` tenant. No service daemon needed — the OS scheduler handles execution.

## Container Mode (Multi-User)

Central schedule authority with per-user execution isolation. Requires running `skdlr-service` as a daemon.

### Architecture

```
┌─────────────────────────────┐
│        skdlr-service        │  ← Central scheduler daemon
│  (polls schedules, claims   │
│   jobs, dispatches via       │
│   Dispatcher trait)          │
├─────────────────────────────┤
│        skdlr-api            │  ← HTTP API for schedule management
│  (tenant-scoped CRUD)       │
├─────────────────────────────┤
│     LocalDispatcher         │  ← Executes commands on host
│  (or future HTTP/container  │
│   dispatchers)              │
└─────────────────────────────┘
```

### Starting the Service

```bash
# Start the scheduler daemon
skdlr-service --poll-interval 10 --lease-duration 300

# Start the API server
skdlr-api --port 3000
```

### Multi-Tenant API

Each tenant has isolated schedules:

```bash
# Create schedule for tenant "alice"
curl -X POST http://localhost:3000/api/v1/tenants/alice/schedules \
  -H "Content-Type: application/json" \
  -d '{"name":"backup","command":"restic backup ~","cron_expr":"0 2 * * *"}'

# List alice's schedules
curl http://localhost:3000/api/v1/tenants/alice/schedules

# Default tenant shortcuts (single-user)
curl http://localhost:3000/api/v1/schedules
```

### Job Lifecycle

```
Queued → Running → Succeeded
                 → Failed → Retrying → Running → ...
                                      → DeadLetter
```

1. **Queued**: Scheduler enqueues job instances when cron fires
2. **Running**: Worker claims job with lease (atomic via SQLite)
3. **Succeeded/Failed**: Worker reports result
4. **Retrying**: Failed jobs with remaining retries wait for backoff
5. **DeadLetter**: All retries exhausted

### Lease & Recovery

- Workers hold a lease on claimed jobs (default: 5 minutes)
- The scheduler periodically checks for expired leases
- Stuck jobs (lease expired) are re-queued automatically
- Workers must renew leases for long-running jobs

### Retry Configuration

```bash
# Via CLI
skdlr add "flaky-job" --schedule "0 * * * *" --command "curl ..." \
  --max-retries 3 --retry-delay 30

# Via API
curl -X POST http://localhost:3000/api/v1/schedules \
  -d '{"name":"flaky-job","command":"curl ...","cron_expr":"0 * * * *","max_retries":3,"retry_delay_secs":30}'
```

Backoff is exponential: `base_delay * 2^(attempt-1)`, capped at 1 hour.

## Migration from Host to Container Mode

1. **Keep existing database** — the schema auto-migrates (adds `tenant_id`, `job_instances` table)
2. Existing schedules get `tenant_id = "default"`
3. Start `skdlr-service` daemon
4. Optionally start `skdlr-api` for HTTP management
5. Existing CLI commands continue to work against the default tenant

### Configuration

Config at `$XDG_CONFIG_HOME/skdlr/config.toml`:

```toml
# Backend preference
# backend = "internal"  # Use internal scheduler for container mode

# Service prefix for generated names
service_prefix = "skdlr"

# Internal scheduler settings
[internal]
check_interval_secs = 60

# Executor wrapper (for sandboxed execution)
[executor]
# wrapper = "/usr/bin/sandbox-exec"
# wrapper_args = ["--user", "{name}", "--workdir", "{workdir}"]
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `SKDLR_BACKEND` | Override backend selection |
| `SKDLR_OCTO_MODE` | Require executor wrapper |
| `SKDLR_SERVICE_PREFIX` | Override service name prefix |

## Dispatcher Architecture

The `Dispatcher` trait abstracts how jobs are executed:

- **`LocalDispatcher`**: Runs commands directly via `tokio::process`
- **Future**: HTTP dispatcher for per-user runner agents
- **Future**: Container dispatcher via Docker/Podman API

Custom dispatchers implement the `Dispatcher` trait:

```rust
pub trait Dispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        schedule: &'a Schedule,
        instance: &'a JobInstance,
    ) -> DispatchFuture<'a>;

    fn name(&self) -> &'static str;
}
```
