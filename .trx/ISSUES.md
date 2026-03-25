# Issues

## Open

### [skdlr-e785] Container-native multi-user scheduler mode (central schedule authority + per-user execution) (P1, epic)
Context
- Current Linux default favors systemd backend and per-user systemd timers.
- For Docker/K8s multi-user runtime, systemd assumptions are a poor fit.
- We want to keep strong per-user isolation while ensuring reliable schedule triggering.

...


