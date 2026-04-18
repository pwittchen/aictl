//! Tool-call audit log.
//!
//! Appends one JSON object per line to `~/.aictl/audit/<session-id>`
//! whenever a tool is invoked during an interactive session. The file
//! name mirrors the corresponding session file under `~/.aictl/sessions/`
//! so a reviewer can read both together. Enabled by default; toggled via
//! `AICTL_SECURITY_AUDIT_LOG` in `~/.aictl/config`. Skipped for
//! single-shot (`--message`) mode and incognito sessions — there is no
//! session id to key the file by.
//!
//! This is an observability channel, not a security restriction: it only
//! records what happened and why. `--unrestricted` leaves it running so
//! the operator can still review what the model did.
//!
//! A single log line includes: timestamp (UTC, ISO-8601 seconds
//! precision), tool name, input (truncated), outcome, and (depending on
//! outcome) a truncated result summary or the denial reason.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::config::config_get;
use crate::session;
use crate::tools::ToolCall;

const MAX_INPUT_LEN: usize = 1_000;
const MAX_RESULT_LEN: usize = 500;

/// Why and how this tool call is being recorded.
#[derive(Clone, Copy)]
pub enum Outcome<'a> {
    Executed { result: &'a str },
    DeniedByPolicy { reason: &'a str },
    DeniedByUser,
    DisabledGlobally,
    DuplicateCall,
}

/// Returns whether audit logging is enabled. Default: on. Configurable via
/// `AICTL_SECURITY_AUDIT_LOG` in `~/.aictl/config` (accepts `false` / `0`).
pub fn enabled() -> bool {
    config_get("AICTL_SECURITY_AUDIT_LOG").is_none_or(|v| v != "false" && v != "0")
}

/// `~/.aictl/audit/`, creating it on first access.
pub fn audit_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(format!("{home}/.aictl/audit"));
    fs::create_dir_all(&p).ok()?;
    Some(p)
}

fn audit_file() -> Option<PathBuf> {
    let id = session::current_id()?;
    Some(audit_dir()?.join(id))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}… (truncated, {} of {} bytes)", &s[..cut], cut, s.len())
}

/// Record one tool-call attempt. No-ops when audit logging is disabled,
/// in incognito mode, or before a session has been initialized
/// (e.g. `--message` single-shot runs).
pub fn log_tool(tool_call: &ToolCall, outcome: Outcome<'_>) {
    if !enabled() {
        return;
    }
    if session::is_incognito() {
        return;
    }
    let Some(path) = audit_file() else {
        return;
    };

    let (outcome_str, mut extra) = match outcome {
        Outcome::Executed { result } => (
            "executed",
            json!({ "result_summary": truncate(result, MAX_RESULT_LEN) }),
        ),
        Outcome::DeniedByPolicy { reason } => ("denied_by_policy", json!({ "reason": reason })),
        Outcome::DeniedByUser => ("denied_by_user", json!({})),
        Outcome::DisabledGlobally => ("disabled", json!({})),
        Outcome::DuplicateCall => ("duplicate", json!({})),
    };

    let Value::Object(ref mut extra_map) = extra else {
        return;
    };
    let mut entry = serde_json::Map::new();
    entry.insert("timestamp".into(), Value::String(timestamp()));
    entry.insert("tool".into(), Value::String(tool_call.name.clone()));
    entry.insert(
        "input".into(),
        Value::String(truncate(&tool_call.input, MAX_INPUT_LEN)),
    );
    entry.insert("outcome".into(), Value::String(outcome_str.into()));
    entry.append(extra_map);

    let Ok(line) = serde_json::to_string(&Value::Object(entry)) else {
        return;
    };

    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = epoch_days_to_ymd(days);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Civil-calendar algorithm (Howard Hinnant). Duplicated from `stats.rs`
/// so this module stays self-contained.
fn epoch_days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days_to_ymd_unix_epoch() {
        assert_eq!(epoch_days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn epoch_days_to_ymd_known_date() {
        assert_eq!(epoch_days_to_ymd(20_556), (2026, 4, 13));
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_over_limit_reports_size() {
        let s = "x".repeat(200);
        let out = truncate(&s, 50);
        assert!(out.starts_with("xxxxx"));
        assert!(out.contains("truncated"));
        assert!(out.contains("200"));
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        // 'é' is two bytes; requesting a cut in the middle of it must
        // walk back to the start of the codepoint.
        let mut s = "a".repeat(9);
        s.push('é'); // bytes 9..10
        s.push_str("bbb");
        let out = truncate(&s, 10); // would land inside 'é'
        // Prefix must end on a char boundary.
        let prefix = out.split_once('…').map(|(a, _)| a).unwrap_or(&out);
        assert!(prefix.is_char_boundary(prefix.len()));
    }
}
