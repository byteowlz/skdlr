# skdlr

A glossary for the domain language used by this project. Add terms only after
their meaning has been resolved; delete this guidance when the first term is
added.

`CONTEXT.md` is a glossary, not a specification, implementation guide, or
scratchpad. Include only project-specific domain concepts, not general
programming terms.

## Language

**Schedule**:
The unit of work: a name, a command to run, a trigger (cron or one-off), and
execution options (workdir, env, retries, tenant). The core thing the user
creates.

**ScheduleKind**:
The trigger type of a schedule — `Recurring` (cron expression) or `OneOff`
(absolute `run_at` timestamp). A schedule has exactly one kind.

**ScheduleStatus**:
The runnable state of a schedule — `Enabled`, `Paused`, `Disabled`. Pause is
time-bound (`paused_until`); disable is indefinite.

**BackendKind**:
The OS scheduler abstraction — `Systemd` (Linux), `Launchd` (macOS),
`Schtasks` (Windows), or `Internal` (portable fallback daemon). Auto-detected
when unset.

**Run**:
An execution attempt of a schedule, recording start/stop time, exit code, and
status. One schedule can have many runs over its lifetime.

**JobInstance**:
A unit of queued/claimed work in the retry model, tracking state (`Pending`,
`Claimed`, `Succeeded`, `Dead`, …), attempt count vs `max_attempts`, and the
claiming worker. Drives retries and the dead-letter queue.

**Tenant**:
A scoping namespace for schedules in multi-tenant setups. `DEFAULT_TENANT_ID`
is the single-user default.

**Internal backend**:
The cross-platform fallback scheduler that runs an on-machine daemon polling the
SQLite store, used when no native scheduler is available.

**Executor wrapper**:
An optional wrapper binary all scheduled commands run through (e.g. a sandbox).
Enforced in Octo mode via `SKDLR_OCTO_MODE`; placeholders `{name}`, `{workdir}`,
`{command}` are substituted into `wrapper_args`.

**Agent context (AGENT_CTX)**:
A snapshot of agent environment/context captured at schedule creation (metadata
only).

**service_prefix**:
Prefix used to name generated systemd/launchd/schtasks service objects.