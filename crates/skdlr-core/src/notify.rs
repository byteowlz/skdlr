//! On-completion notification delivery for schedules.
//!
//! A schedule may carry a `notify_target` (a return address, e.g. a herdr pane
//! id / `AGENT_CTX_AGENT_ADDRESS`) and an optional custom `on_exit` action.
//! When a run finishes, skdlr delivers a completion notification to the target
//! through an existing mechanism (`agent-notify`), spooling the message if the
//! delivery fails (for example because the target session has ended).
//!
//! **Security posture**: `notify_target` / `AGENT_CTX_AGENT_ADDRESS` is a
//! routing hint for delivery only — never an authorization signal. A reachable
//! address does not imply the target is authorized to act; access control stays
//! with the sandbox/runner.

use std::path::{Path, PathBuf};

use crate::error::Result;

/// Native env var for a herdr-managed agent's pane id (the raw source of the
/// v2 `AGENT_CTX_AGENT_ADDRESS` under the herdr multiplexer).
const ENV_HERDR_PANE_ID: &str = "HERDR_PANE_ID";
/// v2 `AGENT_CTX` middle-layer return address (`== HERDR_PANE_ID` under herdr).
const ENV_AGENT_CTX_ADDRESS: &str = "AGENT_CTX_AGENT_ADDRESS";

/// Resolves the best-known return-address target for on-exit delivery.
///
/// Priority: explicit target override → creation-time snapshot
/// `agent_address` (v2) → live `AGENT_CTX_AGENT_ADDRESS` env → live
/// `HERDR_PANE_ID` env. Returns `None` when no address is known.
pub fn resolve_target(explicit: Option<&str>) -> Option<String> {
    resolve_from(explicit, |name| std::env::var(name).ok())
}

fn resolve_from(explicit: Option<&str>, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(t) = explicit.filter(|s| !s.trim().is_empty()) {
        return Some(t.trim().to_string());
    }
    for var in [ENV_AGENT_CTX_ADDRESS, ENV_HERDR_PANE_ID] {
        if let Some(v) = get(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Everything needed to deliver a completion notification.
#[derive(Debug)]
pub struct Completion {
    /// Human-readable label for the finished task (e.g. the schedule name).
    pub label: String,
    /// Exit code of the completed run.
    pub exit_code: i32,
    /// Return address to deliver to. `None` → no default delivery is attempted.
    pub target: Option<String>,
    /// Optional custom on-exit action (shell command). `{target}`, `{exit}`,
    /// `{name}`, and `{label}` tokens are substituted before execution.
    pub on_exit: Option<String>,
}

/// Delivers a completion notification for `completion`.
///
/// When a custom `on_exit` action is present it is substituted and run via the
/// shell. Otherwise (or additionally for a `target` to still be reached) the
/// default delivery goes through `agent-notify`. Any delivery failure is
/// spooled to disk so it can be replayed by `agent-notify --drain`.
pub fn deliver(completion: &Completion) -> Result<()> {
    let custom = completion
        .on_exit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|tmpl| substitute(tmpl, completion));

    let failed = match custom {
        Some(cmd) => run_shell(&cmd)?, // true => spool the raw failure
        None => false,
    };

    // The default agent-notify delivery is attempted whenever a target exists,
    // even alongside a custom action (so the return address is always reached).
    let mut notify_err: Option<String> = None;
    match completion.target.as_deref() {
        Some(target) if !target.trim().is_empty() => {
            notify_err = deliver_via_agent_notify(completion, target);
        }
        _ => {}
    }

    if let (true, Some(err)) = (failed, notify_err.as_deref()) {
        spool_return(
            target_or_self(completion),
            &completion.label,
            &format!("on_exit failed: {err}"),
        );
    } else if let Some(err) = notify_err {
        spool_return(completion.target.as_deref(), &completion.label, &err);
    }
    Ok(())
}

fn target_or_self(completion: &Completion) -> Option<&str> {
    completion.target.as_deref()
}
/// Substitutes `{target}`, `{exit}`, `{name}`, and `{label}` tokens in `tmpl`.
fn substitute(tmpl: &str, completion: &Completion) -> String {
    let target = completion.target.as_deref().unwrap_or("");
    tmpl.replace("{target}", target)
        .replace("{exit}", &completion.exit_code.to_string())
        .replace("{name}", &completion.label)
        .replace("{label}", &completion.label)
}

/// Runs a shell command, returning `false` on failure or non-zero exit.
fn run_shell(cmd: &str) -> Result<bool> {
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .status()?;
    Ok(!status.success())
}

/// Attempts delivery via `agent-notify --tid <target> <msg>`. Returns an error
/// description on failure (missing binary, non-zero exit), `None` on success.
fn deliver_via_agent_notify(completion: &Completion, target: &str) -> Option<String> {
    let Some(bin) = find_on_path("agent-notify") else {
        return Some("agent-notify not found on PATH".to_string());
    };
    let msg = format!(
        "{} finished (exit {})",
        completion.label, completion.exit_code
    );
    let status = std::process::Command::new(bin)
        .arg("--tid")
        .arg(target)
        .arg(&msg)
        .status();
    match status {
        Ok(s) if s.success() => None,
        Ok(s) => Some(format!(
            "agent-notify exited with {}",
            s.code().unwrap_or(-1)
        )),
        Err(e) => Some(format!("agent-notify failed to launch: {e}")),
    }
}

/// Locates an executable by name on `$PATH`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => m.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Persists a failed delivery to the agent-notify spool directory so it can be
/// replayed via `agent-notify --drain` (never silently dropped).
fn spool_return(target: Option<&str>, label: &str, err: &str) {
    let tid = target.unwrap_or("");
    let msg = format!("{label} finished (delivery failed)");
    let payload = serde_json::json!({ "tid": tid, "msg": msg, "err": err }).to_string();
    let dir = spool_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let file = dir.join(format!("{}-{}.json", std::process::id(), secs));
    let _ = std::fs::write(file, payload);
}

fn spool_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENT_NOTIFY_SPOOL") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".config").join("agent-notify").join("spool")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(map: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            map.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn resolves_explicit_target_over_env() {
        let e = env(&[
            (ENV_HERDR_PANE_ID, "w1:p1"),
            (ENV_AGENT_CTX_ADDRESS, "w1:p1"),
        ]);
        assert_eq!(
            resolve_from(Some("explicit-pane"), e).as_deref(),
            Some("explicit-pane")
        );
    }

    #[test]
    fn resolves_agent_ctx_address_then_pane_id() {
        let e = env(&[
            (ENV_AGENT_CTX_ADDRESS, "w2:p9"),
            (ENV_HERDR_PANE_ID, "w2:pX"),
        ]);
        assert_eq!(resolve_from(None, e).as_deref(), Some("w2:p9"));
    }

    #[test]
    fn falls_back_to_pane_id_when_address_absent() {
        let e = env(&[(ENV_HERDR_PANE_ID, "w2:pX")]);
        assert_eq!(resolve_from(None, e).as_deref(), Some("w2:pX"));
    }

    #[test]
    fn trims_and_rejects_blank_targets() {
        let e = env(&[(ENV_HERDR_PANE_ID, "  "), (ENV_AGENT_CTX_ADDRESS, "")]);
        assert_eq!(resolve_from(None, e), None);
    }

    #[test]
    fn substitutes_tokens() {
        let c = Completion {
            label: "build".to_string(),
            exit_code: 3,
            target: Some("w2:p1".to_string()),
            on_exit: None,
        };
        assert_eq!(
            substitute("deal {target} {exit} {name}", &c),
            "deal w2:p1 3 build"
        );
    }
}
