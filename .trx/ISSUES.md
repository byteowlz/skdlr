# Issues

## Open

### [skdlr-jrby.6] Migrate oqto integration from CLI scraping to skdlr-core library (P1, chore)
Replace oqto's text parsing of `skdlr` CLI output with direct use of `skdlr-core` (preferred) or stabilized `skdlr-api`. This is the prerequisite for every other child of this epic.

## Why

oqto_refactor backend/crates/oqto/src/api/handlers/misc.rs:200-400 implements `exec_skdlr_command()`, `scheduler_overview()`, `scheduler_delete()` etc. by `tokio::process::Command::new(\"skdlr\")` and parsing stdout for "list" and "next" output formats. Problems:
...


### [skdlr-jrby.3] Per-job timeout and resource limits across backends (P1, task)
Add `timeout_secs`, `cpu_quota`, `mem_limit_bytes`, `io_weight` to Schedule and enforce them across all four backends.

## Why

Today no backend enforces a per-job timeout. A runaway job holds a lease until the watchdog reaps it (vq7g.2), and resource starvation has to be solved out-of-band. oqto needs hard caps it can configure per schedule, especially for AI-agent jobs (Pi harness) where prompt loops can spin indefinitely.
...


### [skdlr-jrby.2] runner_selector on Schedule + RemoteDispatcher trait impl (P1, feature)
Let a Schedule target a specific runner (or tag set) and dispatch there over a pluggable transport, instead of always running locally.

## Why

oqto already has remote runners: `RunnerHello` advertisement, runner_id routing, Unix socket / WebSocket transport, planned mTLS (oqto-e067) and SSH bootstrap (oqto-wdkj). skdlr's `Dispatcher` trait (crates/skdlr-core/src/dispatcher.rs:70-95) explicitly anticipates this with comments ("Future: HTTP-based dispatch to per-user runner agents") but only ships `LocalDispatcher`.
...


### [skdlr-jrby.1] Typed SandboxSpec on Schedule + structured wrapper handoff (P1, feature)
Replace the ad-hoc `executor.wrapper` + `SKDLR_OCTO_MODE` env-var contract with a typed, persisted `SandboxSpec` field on `Schedule`, delivered to the wrapper as structured JSON instead of argv string-stuffing.

## Why

oqto's sandbox is layered (bubblewrap/sandbox-exec, oqto-guard FUSE, eavs network proxy) and configured via rich data: deny_read paths, allow_write paths, isolate_network/pid, profile name, per-workspace overrides. None of that fits today's wrapper contract:
...


### [skdlr-jrby] oqto fit: sandboxing, remote runners, embedding surface (P1, epic)
Make skdlr a first-class scheduling backend for oqto, with structured sandbox handoff, remote runner dispatch, and a clean library/API embedding surface (no more CLI text scraping).

## Context

Today oqto integrates with skdlr by shelling out to the `skdlr` CLI and parsing text output (see oqto_refactor backend/crates/oqto/src/api/handlers/misc.rs:200-400). The integration works for basic scheduling but blocks every direction oqto is heading:
...


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

### [skdlr-jrby.5] Live log streaming endpoint for JobInstance (P2, feature)
Stream stdout/stderr from a running JobInstance over SSE/WebSocket so oqto can show live output, not poll for `log_path` after the fact.

## Why

Today Run carries a `log_path` that's populated after the job finishes (when populated at all — the internal backend captures into `output.stdout`/`output.stderr` and where exactly that lands depends on the wrapper). oqto wants live output: its UI streams agent canonical events while jobs run, and a 5-minute backup job should not be a black box until it completes.
...


### [skdlr-jrby.4] Secrets references decoupled from env map (P2, feature)
Add `secrets: Vec<SecretRef>` to Schedule so embedders (oqto + eavs) resolve secrets at exec time instead of skdlr persisting them in plaintext.

## Why

`Schedule.env` is `HashMap<String, String>` stored as plain JSON in SQLite. oqto today injects `EAVS_API_KEY` and provider keys this way — those land in `schedules.env` plaintext, in backups, in `skdlr show` output, in CLI argv if anyone ever templates them. The blast radius is wrong.
...


### [skdlr-vq7g.5] Document durable execution model and operator guidance (P2, task)
Update docs with state model, retry/idempotency semantics, and operational tuning (WAL, busy_timeout, watchdog intervals).

## Closed

- [skdlr-rrtr] systemd timer-triggered runs are not recorded in skdlr run history (closed 2026-05-07)
- [skdlr-c4f5] systemd backend generates broken ExecStart when command includes shell wrapper (closed 2026-05-07)
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
