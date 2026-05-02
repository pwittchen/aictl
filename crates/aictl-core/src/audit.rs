//! Tool-call audit log.
//!
//! Appends one JSON object per line to `~/.aictl/audit/<session-id>`
//! whenever a tool is invoked during an interactive session. The file
//! name mirrors the corresponding session file under `~/.aictl/sessions/`
//! so a reviewer can read both together. Enabled by default; toggled via
//! `AICTL_SECURITY_AUDIT_LOG` in `~/.aictl/config`. Skipped for
//! incognito sessions and (by default) single-shot (`--message`) mode —
//! there is no session id to key the file by. Single-shot runs can opt
//! in with `--audit-file <PATH>` to write the same per-line JSON log to
//! an explicit path.
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
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::config::config_get_scoped;
use crate::security::redaction::{Match, RedactionDirection, RedactionMode, RedactionSource};
use crate::session;
use crate::tools::ToolCall;

/// Explicit audit-file path, set by `--audit-file <PATH>`. When present,
/// it takes priority over the per-session `~/.aictl/audit/<session-id>`
/// path and lets single-shot runs (which have no session id) capture an
/// audit trail.
static AUDIT_FILE_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Set an explicit audit-log path for this process. Idempotent: the
/// first call wins, subsequent calls are ignored. Creates parent
/// directories on demand so the writer in [`log_tool`] / [`log_redaction`]
/// can append immediately.
pub fn set_file_override(path: &Path) {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }
    let _ = AUDIT_FILE_OVERRIDE.set(path.to_path_buf());
}

fn override_path() -> Option<&'static Path> {
    AUDIT_FILE_OVERRIDE.get().map(PathBuf::as_path)
}

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

/// Returns whether audit logging is enabled. Default: on.
/// Configurable via `AICTL_SECURITY_AUDIT_LOG` (accepts `false` / `0`)
/// in `~/.aictl/config`. The server-side process additionally honors
/// `AICTL_SERVER_SECURITY_AUDIT_LOG` so the proxy's audit posture can
/// differ from the CLI's. An explicit `--audit-file` override
/// force-enables the subsystem so the flag does what its name suggests
/// even if config has it switched off.
pub fn enabled() -> bool {
    if override_path().is_some() {
        return true;
    }
    config_get_scoped(
        "AICTL_SERVER_SECURITY_AUDIT_LOG",
        "AICTL_SECURITY_AUDIT_LOG",
    )
    .is_none_or(|v| v != "false" && v != "0")
}

/// `~/.aictl/audit/`, creating it on first access.
pub fn audit_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(format!("{home}/.aictl/audit"));
    fs::create_dir_all(&p).ok()?;
    Some(p)
}

fn audit_file() -> Option<PathBuf> {
    if let Some(path) = override_path() {
        return Some(path.to_path_buf());
    }
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
        eprintln!(
            "[audit-debug] log_tool({}) skipped: audit disabled",
            tool_call.name
        );
        return;
    }
    if session::is_incognito() {
        eprintln!(
            "[audit-debug] log_tool({}) skipped: incognito",
            tool_call.name
        );
        return;
    }
    let Some(path) = audit_file() else {
        eprintln!(
            "[audit-debug] log_tool({}) skipped: no audit_file (current_id={:?}, override={:?}, audit_dir={:?})",
            tool_call.name,
            session::current_id(),
            override_path(),
            audit_dir(),
        );
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

/// Record a redaction or block event. Mirrors [`log_tool`] in shape:
/// one JSON line per event appended to `~/.aictl/audit/<session-id>`.
/// No-ops outside a session (single-shot `--message` runs), in
/// incognito mode, or when audit logging is disabled.
///
/// The per-match `snippet` embedded in the log is the placeholder plus
/// a few characters of surrounding context — never the original secret.
pub fn log_redaction(
    direction: RedactionDirection,
    source: RedactionSource,
    mode: RedactionMode,
    text: &str,
    matches: &[Match],
) {
    if !enabled() {
        eprintln!("[audit-debug] log_redaction skipped: audit disabled");
        return;
    }
    if session::is_incognito() {
        eprintln!("[audit-debug] log_redaction skipped: incognito");
        return;
    }
    let Some(path) = audit_file() else {
        eprintln!(
            "[audit-debug] log_redaction skipped: no audit_file (current_id={:?}, override={:?}, audit_dir={:?})",
            session::current_id(),
            override_path(),
            audit_dir(),
        );
        return;
    };
    if matches.is_empty() {
        return;
    }
    eprintln!(
        "[audit-debug] log_redaction writing to {} ({} matches)",
        path.display(),
        matches.len()
    );

    let mode_str = match mode {
        RedactionMode::Off => "off",
        RedactionMode::Redact => "redact",
        RedactionMode::Block => "block",
    };

    let mut match_entries = Vec::with_capacity(matches.len());
    for m in matches {
        let placeholder = m.kind.placeholder();
        let ctx_start = backward_boundary(text, m.range.start.saturating_sub(12));
        let ctx_end = forward_boundary(text, (m.range.end + 12).min(text.len()));
        let before = &text[ctx_start..m.range.start];
        let after = &text[m.range.end..ctx_end];
        let snippet = format!("…{before}[REDACTED:{placeholder}]{after}…");
        match_entries.push(json!({
            "kind": placeholder,
            "range": [m.range.start, m.range.end],
            "confidence": m.confidence,
            "snippet": truncate(&snippet, 120),
        }));
    }

    let entry = json!({
        "timestamp": timestamp(),
        "event": "redaction",
        "mode": mode_str,
        "direction": direction.as_str(),
        "source": source.as_str(),
        "matches": match_entries,
    });

    let Ok(line) = serde_json::to_string(&entry) else {
        return;
    };

    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

fn backward_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn forward_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
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
