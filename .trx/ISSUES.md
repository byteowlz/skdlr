# Issues

## Open

### [skdlr-e785.9] Tests: multi-tenant isolation, duplicate-delivery idempotency, retry/lease behavior (P1, task)

### [skdlr-e785.7] API: implement schedule CRUD + run history + pause/resume endpoints with tenant scoping (P1, task)

### [skdlr-e785.6] Dispatcher: add runner dispatch interface (transport-agnostic, no direct host exec in multi-user mode) (P1, task)

### [skdlr-e785.5] Scheduler core: implement retry/backoff/dead-letter semantics with idempotency keys (P1, task)

### [skdlr-e785.4] Scheduler core: implement atomic claim + lease renewal + stuck-job recovery (P1, task)

### [skdlr-e785.3] Schema: add job_instances lease/retry table for queued/running/succeeded/failed states (P1, task)

### [skdlr-e785.2] Schema: add tenant_id and change unique key to (tenant_id, name) (P1, task)

### [skdlr-e785.1] Build skdlr-service daemon that continuously runs internal scheduler loop (P1, task)

### [skdlr-e785] Container-native multi-user scheduler mode (central schedule authority + per-user execution) (P1, epic)
Context
- Current Linux default favors systemd backend and per-user systemd timers.
- For Docker/K8s multi-user runtime, systemd assumptions are a poor fit.
- We want to keep strong per-user isolation while ensuring reliable schedule triggering.

...


### [skdlr-e785.8] Docs: runtime-mode guide (host-systemd vs container mode) and migration notes (P2, task)

