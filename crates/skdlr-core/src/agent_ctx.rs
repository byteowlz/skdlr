//! `AGENT_CTX` environment contract (v1) capture for skdlr.
//!
//! Implements the consumer side of the cross-tool `AGENT_CTX_*` contract
//! (schemas/agent-context-env, <https://schemas.byteowlz.dev/agent-context-env/>):
//!
//! - Context is **metadata, not security authority** — skdlr never derives
//!   permissions from it. Access control stays with the sandbox/runner.
//! - Read defensively: missing variables never break behavior; malformed or
//!   empty values are treated as absent.
//! - Capture is a creation-time snapshot: a schedule records *who/where*
//!   created it (platform, session, workspace), for linking and audit.

use serde::{Deserialize, Serialize};

/// Environment variable names of the `AGENT_CTX` v1 contract.
pub const ENV_VARS: &[&str] = &[
    "AGENT_CTX_VERSION",
    "AGENT_CTX_PLATFORM_NAME",
    "AGENT_CTX_PLATFORM_VERSION",
    "AGENT_CTX_HARNESS",
    "AGENT_CTX_RUN_MODE",
    "AGENT_CTX_PLATFORM_SESSION_ID",
    "AGENT_CTX_HARNESS_SESSION_ID",
    "AGENT_CTX_WORKSPACE_ID",
    "AGENT_CTX_WORKSPACE_PATH",
    "AGENT_CTX_USER_ID",
    "AGENT_CTX_SESSION_NAME",
    "AGENT_CTX_READABLE_ID",
    "AGENT_CTX_MODEL",
    "AGENT_CTX_REQUEST_ID",
    "AGENT_CTX_CORRELATION_ID",
    "AGENT_CTX_SANDBOX_PROFILE",
];

/// Creation-time snapshot of the `AGENT_CTX_*` environment.
///
/// Every field is optional: the contract requires tools to tolerate entirely
/// missing context. A snapshot is only produced when at least one variable is
/// present, so schedules created outside an AGENT_CTX-aware runner stay clean.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContext {
    /// `AGENT_CTX_VERSION` — contract version.
    pub version: Option<String>,
    /// `AGENT_CTX_PLATFORM_NAME` — platform name (e.g. `oqto`).
    pub platform_name: Option<String>,
    /// `AGENT_CTX_PLATFORM_VERSION` — platform build/version.
    pub platform_version: Option<String>,
    /// `AGENT_CTX_HARNESS` — active harness/runtime (e.g. `pi`).
    pub harness: Option<String>,
    /// `AGENT_CTX_RUN_MODE` — runtime mode (e.g. `runner`).
    pub run_mode: Option<String>,
    /// `AGENT_CTX_PLATFORM_SESSION_ID` — stable platform session id.
    pub platform_session_id: Option<String>,
    /// `AGENT_CTX_HARNESS_SESSION_ID` — harness-native session id.
    pub harness_session_id: Option<String>,
    /// `AGENT_CTX_WORKSPACE_ID` — stable workspace id/hash.
    pub workspace_id: Option<String>,
    /// `AGENT_CTX_WORKSPACE_PATH` — absolute workspace path.
    pub workspace_path: Option<String>,
    /// `AGENT_CTX_USER_ID` — platform user id.
    pub user_id: Option<String>,
    /// `AGENT_CTX_SESSION_NAME` — human-readable label (display-only).
    pub session_name: Option<String>,
    /// `AGENT_CTX_READABLE_ID` — short friendly id for logs/UI.
    pub readable_id: Option<String>,
    /// `AGENT_CTX_MODEL` — active model id.
    pub model: Option<String>,
    /// `AGENT_CTX_REQUEST_ID` — per prompt/action id.
    pub request_id: Option<String>,
    /// `AGENT_CTX_CORRELATION_ID` — cross-service trace id.
    pub correlation_id: Option<String>,
    /// `AGENT_CTX_SANDBOX_PROFILE` — active sandbox profile (observability).
    pub sandbox_profile: Option<String>,
}

impl AgentContext {
    /// Captures the current process environment as an `AGENT_CTX` snapshot.
    ///
    /// Returns `None` when no `AGENT_CTX_*` variable is present.
    pub fn capture() -> Option<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Captures a snapshot through an arbitrary lookup function.
    ///
    /// Empty and whitespace-only values are treated as missing (the contract
    /// requires tolerating malformed input, never hard-failing).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let get = |name: &str| -> Option<String> {
            lookup(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        let ctx = Self {
            version: get("AGENT_CTX_VERSION"),
            platform_name: get("AGENT_CTX_PLATFORM_NAME"),
            platform_version: get("AGENT_CTX_PLATFORM_VERSION"),
            harness: get("AGENT_CTX_HARNESS"),
            run_mode: get("AGENT_CTX_RUN_MODE"),
            platform_session_id: get("AGENT_CTX_PLATFORM_SESSION_ID"),
            harness_session_id: get("AGENT_CTX_HARNESS_SESSION_ID"),
            workspace_id: get("AGENT_CTX_WORKSPACE_ID"),
            workspace_path: get("AGENT_CTX_WORKSPACE_PATH"),
            user_id: get("AGENT_CTX_USER_ID"),
            session_name: get("AGENT_CTX_SESSION_NAME"),
            readable_id: get("AGENT_CTX_READABLE_ID"),
            model: get("AGENT_CTX_MODEL"),
            request_id: get("AGENT_CTX_REQUEST_ID"),
            correlation_id: get("AGENT_CTX_CORRELATION_ID"),
            sandbox_profile: get("AGENT_CTX_SANDBOX_PROFILE"),
        };

        if ctx.is_empty() { None } else { Some(ctx) }
    }

    /// Returns true when no variable carried a value.
    pub fn is_empty(&self) -> bool {
        !self.iter().any(|(_, value)| value.is_some())
    }

    /// Iterates over `(env var name, value)` pairs, including `None` values.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Option<&str>)> + '_ {
        let items: [(&'static str, Option<&str>); 16] = [
            ("AGENT_CTX_VERSION", self.version.as_deref()),
            ("AGENT_CTX_PLATFORM_NAME", self.platform_name.as_deref()),
            (
                "AGENT_CTX_PLATFORM_VERSION",
                self.platform_version.as_deref(),
            ),
            ("AGENT_CTX_HARNESS", self.harness.as_deref()),
            ("AGENT_CTX_RUN_MODE", self.run_mode.as_deref()),
            (
                "AGENT_CTX_PLATFORM_SESSION_ID",
                self.platform_session_id.as_deref(),
            ),
            (
                "AGENT_CTX_HARNESS_SESSION_ID",
                self.harness_session_id.as_deref(),
            ),
            ("AGENT_CTX_WORKSPACE_ID", self.workspace_id.as_deref()),
            ("AGENT_CTX_WORKSPACE_PATH", self.workspace_path.as_deref()),
            ("AGENT_CTX_USER_ID", self.user_id.as_deref()),
            ("AGENT_CTX_SESSION_NAME", self.session_name.as_deref()),
            ("AGENT_CTX_READABLE_ID", self.readable_id.as_deref()),
            ("AGENT_CTX_MODEL", self.model.as_deref()),
            ("AGENT_CTX_REQUEST_ID", self.request_id.as_deref()),
            ("AGENT_CTX_CORRELATION_ID", self.correlation_id.as_deref()),
            ("AGENT_CTX_SANDBOX_PROFILE", self.sandbox_profile.as_deref()),
        ];
        items.into_iter()
    }

    /// Iterates over the populated `(env var name, value)` pairs.
    pub fn iter_present(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.iter()
            .filter_map(|(name, value)| value.map(|v| (name, v)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup_from<'a>(
        map: &'a HashMap<&'static str, &'static str>,
    ) -> impl for<'s> Fn(&'s str) -> Option<String> + 'a {
        move |name| map.get(name).map(|v| (*v).to_string())
    }

    #[test]
    fn missing_context_yields_none() {
        let empty: HashMap<&str, &str> = HashMap::new();
        assert!(AgentContext::from_lookup(lookup_from(&empty)).is_none());
    }

    #[test]
    fn minimal_starter_set_is_captured() {
        let mut env = HashMap::new();
        env.insert("AGENT_CTX_VERSION", "1");
        env.insert("AGENT_CTX_PLATFORM_NAME", "oqto");
        env.insert("AGENT_CTX_WORKSPACE_ID", "ws_a13f");
        env.insert("AGENT_CTX_USER_ID", "u_123");

        let ctx = AgentContext::from_lookup(lookup_from(&env)).unwrap();
        assert_eq!(ctx.version.as_deref(), Some("1"));
        assert_eq!(ctx.platform_name.as_deref(), Some("oqto"));
        assert_eq!(ctx.workspace_id.as_deref(), Some("ws_a13f"));
        assert_eq!(ctx.user_id.as_deref(), Some("u_123"));
        assert_eq!(ctx.platform_session_id, None);
        assert!(!ctx.is_empty());
    }

    #[test]
    fn empty_and_whitespace_values_are_treated_as_missing() {
        let mut env = HashMap::new();
        env.insert("AGENT_CTX_WORKSPACE_ID", "  ");
        env.insert("AGENT_CTX_USER_ID", "");

        assert!(AgentContext::from_lookup(lookup_from(&env)).is_none());
    }

    #[test]
    fn values_are_trimmed() {
        let mut env = HashMap::new();
        env.insert("AGENT_CTX_WORKSPACE_ID", " ws_a13f ");

        let ctx = AgentContext::from_lookup(lookup_from(&env)).unwrap();
        assert_eq!(ctx.workspace_id.as_deref(), Some("ws_a13f"));
    }

    #[test]
    fn full_snapshot_round_trips_through_serde() {
        let mut env: HashMap<&str, &str> = ENV_VARS.iter().map(|v| (*v, "x")).collect();
        env.insert("AGENT_CTX_VERSION", "1");
        env.insert("AGENT_CTX_WORKSPACE_ID", "ws_1");

        let ctx = AgentContext::from_lookup(lookup_from(&env)).unwrap();
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: AgentContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, parsed);
        assert_eq!(ctx.iter_present().count(), ENV_VARS.len());
    }

    #[test]
    fn env_var_names_are_complete() {
        // Every contract variable must appear in ENV_VARS and in iter().
        let ctx = AgentContext::default();
        let iter_names: Vec<&str> = ctx.iter().map(|(name, _)| name).collect();
        assert_eq!(iter_names.len(), ENV_VARS.len());
        for name in ENV_VARS {
            assert!(iter_names.contains(name), "missing from iter(): {name}");
        }
    }
}
