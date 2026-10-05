# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Added

- `[api]` config section with an exact-origin CORS allowlist (never `*`).
- Config generation via `schemars` (`just generate-config` / `validate-config`),
  so `examples/config.toml` and `examples/config.schema.json` are derived from
  `SkdlrConfig` and can't drift.
- `scripts/drift-check.sh` and the `scripts/` guardrail layout for the
  byteowlz rust-workspace standard.
- `docs/adr/` guidance, `CONTEXT.md` domain glossary, `clippy.toml`, and
  `.githooks/` (pre-commit/pre-push/schema-guard, `just setup-hooks`).
- Pinned `rmcp-macros` in lockstep with `rmcp` in the workspace manifest.

### Fixed

- API CORS changed from `allow_origin(Any)` to a configurable exact-origin
  allowlist, per the byteowlz API guardrail.
- Resolved pre-existing `clippy -D warnings` failures in `skdlr-core` and
  `skdlr-cli`.