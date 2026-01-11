# skdlr - Cross-Platform Task Scheduler

## Overview

skdlr is a lean, cross-platform task scheduler that abstracts over native OS scheduling mechanisms (systemd, launchd, Windows Task Scheduler) with a unified CLI and TUI interface.

**Core responsibility**: Time-based task scheduling with native OS integration.

**What it owns**:
- Schedule management (create, update, delete, pause, resume)
- Cross-platform backend abstraction
- Schedule metadata storage (SQLite)
- TUI for schedule management

**What it depends on**:
- Native OS schedulers (systemd, launchd, schtasks)
- SQLite for metadata

**What it exposes**:
- `skdlr` CLI binary
- `skdlr-tui` TUI binary
- `skdlr-mcp` MCP server for AI agent integration
- `skdlr-core` library for embedding (used by octo, byt)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         skdlr                                │
├─────────────────────────────────────────────────────────────┤
│  CLI / TUI / MCP Server / Library                           │
├─────────────────────────────────────────────────────────────┤
│                    Backend Abstraction                       │
├───────────┬───────────┬──────────────┬──────────────────────┤
│  systemd  │  launchd  │  schtasks    │  internal            │
│  (Linux)  │  (macOS)  │  (Windows)   │  (fallback/portable) │
└───────────┴───────────┴──────────────┴──────────────────────┘
```

### Backend Selection

Backends are compile-time gated:
- Linux: `systemd` backend (falls back to `internal` if systemd unavailable)
- macOS: `launchd` backend
- Windows: `schtasks` backend
- All: `internal` backend available as fallback (runs as daemon)

### User Modes

- **Single-user**: Default mode, schedules run as current user
- **Multi-user**: Each user has their own schedules (separate processes/services)

---

## Crate Structure

```
crates/
├── skdlr-core/       # Core library (backend trait, models, storage)
│   └── src/
│       ├── lib.rs
│       ├── models.rs       # Schedule, Run, Status types
│       ├── storage.rs      # SQLite metadata storage
│       ├── backend/
│       │   ├── mod.rs      # Backend trait
│       │   ├── systemd.rs  # Linux systemd (cfg-gated)
│       │   ├── launchd.rs  # macOS launchd (cfg-gated)
│       │   ├── schtasks.rs # Windows Task Scheduler (cfg-gated)
│       │   └── internal.rs # Fallback daemon scheduler
│       └── cron.rs         # Cron expression parsing
├── skdlr-cli/        # CLI binary
├── skdlr-tui/        # TUI binary
├── skdlr-mcp/        # MCP server for AI agents
└── skdlr-api/        # HTTP API server (for octo integration)
```

---

## CLI Interface

```bash
# Schedule management
skdlr add "backup" --schedule "0 2 * * *" --command "restic backup ~"
skdlr add "analyze" --schedule "0 8 * * *" --command "opencode -p 'Review'" --workdir ~/projects/myapp
skdlr list                    # List all schedules
skdlr show backup             # Show schedule details
skdlr edit backup --schedule "0 3 * * *"  # Update schedule
skdlr remove backup           # Delete schedule

# Enable/disable
skdlr enable backup
skdlr disable backup
skdlr pause backup --until "2026-01-15"   # Pause until date

# Manual execution
skdlr run backup              # Trigger immediate run
skdlr run backup --dry-run    # Show what would run

# History and logs
skdlr logs backup             # View execution history
skdlr logs backup --last 10   # Last 10 runs
skdlr logs backup --follow    # Follow live output

# Status
skdlr status                  # Overview of all schedules
skdlr next                    # Show upcoming runs

# Backend info
skdlr backend                 # Show active backend
skdlr doctor                  # Health check

# TUI
skdlr-tui                     # Launch interactive TUI
```

---

## Configuration

Config at `$XDG_CONFIG_HOME/skdlr/config.toml`:

```toml
# Backend preference (auto-detected if not set)
# backend = "systemd"  # or "launchd", "schtasks", "internal"

# Default working directory for schedules
default_workdir = "~"

# Prefix for generated service/timer names
service_prefix = "skdlr"

# Internal backend settings (used when native unavailable)
[internal]
check_interval_secs = 60
```

---

## Core Principles

- **Native first**: Use OS schedulers when available for reliability
- **Lean**: Time-based scheduling only, no file watchers or webhooks
- **Composable**: Library mode for embedding in octo/byt
- **Platform-gated**: Only compile code for current platform

---

## Rust Workflow

- Platform-specific code uses `#[cfg(target_os = "...")]` attributes
- Backend implementations in separate files, conditionally compiled
- Run `cargo check` to verify current platform compiles
- Run `cargo test` for platform-agnostic tests
- Cross-platform testing via CI matrix

---

## Agent Coordination with mailz

When working on skdlr, use mailz for coordination if multiple agents are active:

```bash
# At session start - check for messages
mailz-cli inbox

# Before editing shared files
mailz-cli reserve crates/skdlr-core/src/backend/mod.rs --ttl 1800 --reason "Refactoring backend trait"

# When you need input from another agent or human
mailz-cli send <recipient> "Need review" --body "Please review the backend trait changes"

# At session end
mailz-cli release crates/skdlr-core/src/backend/mod.rs
```

---

## Issue Tracking (trx)

```bash
trx ready              # Show unblocked issues
trx create "Title" -t task -p 2   # Create issue (types: bug/feature/task/epic/chore, priority: 0-4)
trx update <id> --status in_progress
trx close <id> -r "Done"
trx sync               # Commit .trx/ changes
```

Priorities: 0=critical, 1=high, 2=medium, 3=low, 4=backlog

## Memory System (byt/mmry)

```bash
byt memory add "Important decision or learning"
byt memory search "query"
```

---

## Integration Points

### octo Integration

octo uses skdlr-core as a library to:
- Display schedules in dashboard
- Create/edit/delete schedules via web UI
- Show execution history

### byt Integration

byt wraps skdlr CLI for cross-repo scheduling:
```bash
byt schedule add "catalog-refresh" --schedule "0 */6 * * *" --command "byt catalog refresh"
byt schedule list
```

---

## Releases & Distribution

Uses GitHub Actions for automated releases. See `.github/workflows/release.yml`.

```bash
git tag v1.0.0 && git push --tags
```

Builds for:
- Linux x86_64 (ubuntu-latest)
- macOS x86_64 and ARM64 (macos-14)
- Windows x86_64 (windows-latest)
