# Issues

## Open

### [skdlr-vq7g.4] Add durability test matrix (crash, contention, replay) (P1, task)
Integration tests for crash recovery, WAL contention, lease expiry recovery, and transition invariants.

### [skdlr-vq7g.3] Implement step executor with replay-safe resume (P1, feature)
Add step-based execution API that resumes from last completed step and never replays committed side effects.

### [skdlr-vq7g.2] Implement atomic lease claim/renew/requeue semantics (P1, task)
Harden worker leasing with atomic claims, renewals, expiry recovery, and deterministic retry backoff.

### [skdlr-vq7g.1] Design durable state machine + checkpoint schema (P1, task)
Define SQLite schema changes for job_instances + job_steps/checkpoints with idempotency keys and transition invariants.

### [skdlr-vq7g] Durable Execution v1 (SQLite, step checkpoints, lease-safe workers) (P1, epic)
Adopt an Absurd-inspired durable execution model in skdlr while staying SQLite-first per user/workspace DB.\n\nReference\n- ../external-repos/absurd @ 3120ce9\n\nGoals\n- Multi-step job execution with durable checkpoints.\n- Lease-safe worker claims and watchdog recovery hardened for production.\n- Idempotent step transitions and replay-safe retries.\n- Keep architecture compatible with optional future Postgres backend without requiring it now.\n\nScope\n1) Data model\n   - job_instances durable state machine\n   - step checkpoint table with idempotency keys\n2) Scheduler/runtime\n   - atomic claim+lease semantics\n   - lease renewal, stuck-job requeue, deterministic retry backoff\n3) APIs/UX\n   - expose per-step status and resume point in CLI/API/TUI\n4) Verification\n   - crash/restart recovery tests\n   - contention tests on SQLite WAL + busy_timeout\n   - invariant checks for state transitions\n\nDefinition of done\n- jobs can resume from last successful step after crash\n- no duplicate side effects when retries happen (idempotency enforced)\n- watchdog recovers expired leases without manual intervention\n- docs updated with durable execution semantics and operational guidance

### [skdlr-vq7g.5] Document durable execution model and operator guidance (P2, task)
Update docs with state model, retry/idempotency semantics, and operational tuning (WAL, busy_timeout, watchdog intervals).

## Closed

- [skdlr-rz76] systemd service files missing ExecStart= prefix (closed 2026-05-06)
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
