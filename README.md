# skdlr

Cross-platform task scheduler with native OS integration.

skdlr provides a unified interface for scheduling recurring tasks across different operating systems, using native schedulers (systemd, launchd, Windows Task Scheduler) when available, with a fallback internal scheduler.

## Features

- Native OS integration: systemd (Linux), launchd (macOS), schtasks (Windows)
- Fallback internal scheduler for portable operation
- Cron-style scheduling expressions
- SQLite-based schedule storage
- CLI, TUI, and library interfaces
- MCP server for AI agent integration
- HTTP API for dashboard integration (e.g., octo)

## Installation

```bash
cargo install --path crates/skdlr-cli
```

Or build from source:

```bash
cargo build --release
```

## Quick Start

```bash
# Add a schedule
skdlr add backup --schedule "0 2 * * *" --command "restic backup ~"

# List schedules
skdlr list

# Enable/disable
skdlr disable backup
skdlr enable backup

# Trigger immediate run
skdlr run backup

# View execution history
skdlr logs backup

# Check status
skdlr status
skdlr backend
skdlr doctor
```

## CLI Reference

```
skdlr add <name> --schedule <cron> --command <cmd>   Add a schedule
skdlr list                                            List all schedules
skdlr show <name>                                     Show schedule details
skdlr edit <name> [options]                           Update a schedule
skdlr remove <name>                                   Delete a schedule
skdlr enable <name>                                   Enable a schedule
skdlr disable <name>                                  Disable a schedule
skdlr run <name>                                      Trigger immediate run
skdlr logs <name>                                     View execution history
skdlr status                                          Overview of all schedules
skdlr next                                            Show upcoming runs
skdlr backend                                         Show active backend
skdlr doctor                                          Health check
skdlr completions <shell>                             Generate shell completions
```

## Configuration

Config file: `$XDG_CONFIG_HOME/skdlr/config.toml`

```toml
# Backend (auto-detected if not set)
# backend = "systemd"  # or "launchd", "schtasks", "internal"

# Prefix for generated service/timer names
service_prefix = "skdlr"

# Default working directory
default_workdir = "~"

[internal]
# Check interval for internal scheduler (seconds)
check_interval_secs = 60
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         skdlr                               │
├─────────────────────────────────────────────────────────────┤
│  CLI / TUI / MCP Server / Library                           │
├─────────────────────────────────────────────────────────────┤
│                    Backend Abstraction                      │
├───────────┬───────────┬──────────────┬──────────────────────┤
│  systemd  │  launchd  │  schtasks    │  internal            │
│  (Linux)  │  (macOS)  │  (Windows)   │  (fallback)          │
└───────────┴───────────┴──────────────┴──────────────────────┘
```

## Crate Structure

```
crates/
├── skdlr-core/    # Core library (backend trait, models, storage)
├── skdlr-cli/     # Command-line interface
├── skdlr-tui/     # Terminal user interface (ratatui)
├── skdlr-mcp/     # MCP server for AI agents
└── skdlr-api/     # HTTP API server (axum)
```

## Integration

### As a Library

```rust
use skdlr_core::{Storage, Schedule, backend};

// Open storage
let storage = Storage::open(Path::new("skdlr.db"))?;

// Create and save a schedule
let schedule = Schedule::new("backup", "0 2 * * *", "restic backup ~");
storage.save_schedule(&schedule)?;

// Get backend and install
let backend = backend::create_backend(backend::BackendKind::detect(), &config);
backend.install(&schedule).await?;
backend.enable(&schedule).await?;
```

### With octo

skdlr-api provides REST endpoints for the octo dashboard to manage schedules.

### With byt

```bash
byt schedule add "catalog-refresh" --schedule "0 */6 * * *" --command "byt catalog refresh"
byt schedule list
```

## Development

```bash
just build          # Build all crates
just test           # Run tests
just check-all      # Format, lint, test
just install-all    # Install all binaries
```

## License

MIT
