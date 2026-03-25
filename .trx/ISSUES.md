# Issues

## Closed

- [skdlr-e785] Container-native multi-user scheduler mode (central schedule authority + per-user execution) (closed 2026-03-25)
- [skdlr-e785.8] Docs: runtime-mode guide (host-systemd vs container mode) and migration notes (closed 2026-03-25)
- [skdlr-e785.9] Tests: multi-tenant isolation, duplicate-delivery idempotency, retry/lease behavior (closed 2026-03-25)
- [skdlr-e785.7] API: implement schedule CRUD + run history + pause/resume endpoints with tenant scoping (closed 2026-03-25)
- [skdlr-e785.6] Dispatcher: add runner dispatch interface (transport-agnostic, no direct host exec in multi-user mode) (closed 2026-03-25)
- [skdlr-e785.1] Build skdlr-service daemon that continuously runs internal scheduler loop (closed 2026-03-25)
- [skdlr-e785.5] Scheduler core: implement retry/backoff/dead-letter semantics with idempotency keys (closed 2026-03-25)
- [skdlr-e785.4] Scheduler core: implement atomic claim + lease renewal + stuck-job recovery (closed 2026-03-25)
- [skdlr-e785.3] Schema: add job_instances lease/retry table for queued/running/succeeded/failed states (closed 2026-03-25)
- [skdlr-e785.2] Schema: add tenant_id and change unique key to (tenant_id, name) (closed 2026-03-25)
