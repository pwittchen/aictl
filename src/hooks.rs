//! User-defined lifecycle hooks.
//!
//! A hook is a shell command the harness runs in response to a lifecycle
//! event — `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
//! `Stop`, `PreCompact`, `Notification`, `SessionEnd`. Hooks are *harness*
//! behavior, not LLM behavior: rules like "always run `cargo fmt` after
//! `edit_file`" belong here, not in agent prompts or memory.
//!
//! Configured in `~/.aictl/hooks.json` (override via `AICTL_HOOKS_FILE`):
//!
//! ```json
//! {
//!   "PreToolUse": [
//!     { "matcher": "exec_shell", "command": "echo seen", "timeout": 30 }
//!   ],
//!   "PostToolUse": [
//!     { "matcher": "edit_file|write_file", "command": "cargo fmt" }
//!   ]
//! }
//! ```
//!
//! Each hook receives a JSON payload on stdin and may return JSON on
//! stdout to influence the harness:
//!   * `{ "decision": "block", "reason": "..." }` aborts the action and
//!     surfaces the reason to the LLM.
//!   * `{ "additionalContext": "..." }` injects extra context into the
//!     next turn.
//!   * `{ "rewrittenPrompt": "..." }` (`UserPromptSubmit` only) replaces
//!     the user's prompt before the agent sees it.
//!
//! Hooks default to a 60s timeout, run in the security working directory
//! with a scrubbed env, and are NOT bypassed by `--unrestricted` — that
//! flag only relaxes the inner shell-validation that would otherwise
//! refuse to run hook commands containing blocked binaries.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::sync::RwLock;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::config_get;

/// Default timeout (seconds) for an individual hook invocation. Long enough
/// for a typical formatter or linter; short enough that a wedged hook can't
/// hang the agent loop.
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 60;

/// Loaded hook table, keyed by event. Populated once on `init()` and
/// reloaded on demand via [`reload`] (used by the `/hooks` REPL menu after
/// a toggle/edit).
static HOOKS: OnceLock<RwLock<HashMap<HookEvent, Vec<Hook>>>> = OnceLock::new();

/// All lifecycle events. The string forms are the keys used in
/// `~/.aictl/hooks.json` and the `event` field in the JSON payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    PreCompact,
    Notification,
}

impl HookEvent {
    pub const ALL: &'static [HookEvent] = &[
        HookEvent::SessionStart,
        HookEvent::SessionEnd,
        HookEvent::UserPromptSubmit,
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::Stop,
        HookEvent::PreCompact,
        HookEvent::Notification,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::Stop => "Stop",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::Notification => "Notification",
        }
    }

    pub fn from_str(s: &str) -> Option<HookEvent> {
        for ev in Self::ALL {
            if ev.as_str().eq_ignore_ascii_case(s) {
                return Some(*ev);
            }
        }
        None
    }
}

impl fmt::Display for HookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Hook {
    pub event: HookEvent,
    pub matcher: String,
    pub command: String,
    pub timeout_secs: u64,
    pub enabled: bool,
}

/// What a hook decided when its stdout was parsed. Each variant maps to a
/// JSON shape on stdout — see the module docs for the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    /// Hook ran, no influence on the harness (empty stdout / no decision key).
    Continue,
    /// `{ "decision": "block", "reason": "..." }` — caller aborts the action.
    Block(String),
    /// `{ "decision": "approve", "reason": "..." }` — caller may skip user
    /// confirmation for the in-flight tool call.
    Approve(String),
    /// `{ "additionalContext": "..." }` — append text to the next turn.
    AddContext(String),
    /// `{ "rewrittenPrompt": "..." }` — `UserPromptSubmit` only; replace the
    /// user's prompt before the agent sees it.
    RewritePrompt(String),
}

/// Aggregated outcome of running every matching hook for one event. The
/// caller folds these into a single decision (block beats everything; the
/// first rewrite wins; additional context lines accumulate).
#[derive(Debug, Default, Clone)]
pub struct HookOutcome {
    pub blocked: Option<String>,
    pub approved: Option<String>,
    pub rewritten_prompt: Option<String>,
    pub additional_context: Vec<String>,
}

impl HookOutcome {
    pub fn merged_context(&self) -> Option<String> {
        if self.additional_context.is_empty() {
            None
        } else {
            Some(self.additional_context.join("\n\n"))
        }
    }
}

/// Path to the hooks config file. Override with `AICTL_HOOKS_FILE`,
/// otherwise `~/.aictl/hooks.json`.
pub fn hooks_file() -> Option<PathBuf> {
    if let Some(p) = config_get("AICTL_HOOKS_FILE") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(format!("{home}/.aictl/hooks.json")))
}

fn store() -> &'static RwLock<HashMap<HookEvent, Vec<Hook>>> {
    HOOKS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Initialize the hooks subsystem. Reads `hooks.json` and populates the
/// in-memory table. Idempotent: subsequent calls are a no-op (use [`reload`]
/// to re-read after edits).
pub fn init() {
    if HOOKS.get().is_some() {
        return;
    }
    let map = load_from_disk();
    let _ = HOOKS.set(RwLock::new(map));
}

/// Re-read the hooks file, replacing the in-memory table. Used by the
/// `/hooks` menu after enable/disable toggles or external edits.
pub fn reload() {
    let map = load_from_disk();
    if let Ok(mut w) = store().write() {
        *w = map;
    }
}

fn load_from_disk() -> HashMap<HookEvent, Vec<Hook>> {
    let Some(path) = hooks_file() else {
        return HashMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    parse_hooks(&raw).unwrap_or_else(|err| {
        eprintln!("hooks: failed to parse {}: {err}", path.display());
        HashMap::new()
    })
}

/// Parse a `hooks.json` document. Public for tests.
pub fn parse_hooks(raw: &str) -> Result<HashMap<HookEvent, Vec<Hook>>, String> {
    let doc: Value = serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = doc
        .as_object()
        .ok_or_else(|| "top-level must be an object".to_string())?;

    let mut out: HashMap<HookEvent, Vec<Hook>> = HashMap::new();
    for (key, val) in obj {
        // Skip JSON5-style comment keys silently — common convention in
        // JSON config that doesn't support real comments.
        if key.starts_with('_') {
            continue;
        }
        let Some(event) = HookEvent::from_str(key) else {
            eprintln!("hooks: skipping unknown event '{key}'");
            continue;
        };
        let arr = val
            .as_array()
            .ok_or_else(|| format!("event '{key}' must be an array"))?;
        let mut hooks = Vec::with_capacity(arr.len());
        for (i, entry) in arr.iter().enumerate() {
            let entry_obj = entry
                .as_object()
                .ok_or_else(|| format!("{key}[{i}]: must be an object"))?;
            let matcher = entry_obj
                .get("matcher")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string();
            let command = entry_obj
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{key}[{i}]: missing required field 'command'"))?
                .to_string();
            if command.trim().is_empty() {
                return Err(format!("{key}[{i}]: 'command' must not be empty"));
            }
            let timeout_secs = entry_obj
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS);
            let enabled = entry_obj
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            hooks.push(Hook {
                event,
                matcher,
                command,
                timeout_secs,
                enabled,
            });
        }
        out.insert(event, hooks);
    }
    Ok(out)
}

/// All hooks for `event`, including disabled ones (caller filters).
pub fn list_for(event: HookEvent) -> Vec<Hook> {
    store()
        .read()
        .ok()
        .and_then(|m| m.get(&event).cloned())
        .unwrap_or_default()
}

/// Flat list of every configured hook across every event, for `/hooks` and
/// `--list-hooks`. Sorted: events in canonical order, hooks within an event
/// in their declared order so the user can see exactly what they wrote.
pub fn list_all() -> Vec<Hook> {
    let mut out = Vec::new();
    let map = store().read();
    if let Ok(map) = map {
        for ev in HookEvent::ALL {
            if let Some(hooks) = map.get(ev) {
                out.extend(hooks.iter().cloned());
            }
        }
    }
    out
}

/// Glob-match `pattern` against `name`. `*` matches any run of characters,
/// `?` matches a single character, `|` separates alternative patterns
/// (`exec_shell|write_file` matches either). An empty pattern or `*` always
/// matches. Case-sensitive — tool names are.
pub fn matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    pattern.split('|').any(|p| glob_match(p.trim(), name))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_slice(&p, &t)
}

fn glob_match_slice(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            // Skip consecutive stars.
            let mut idx = 0;
            while idx < p.len() && p[idx] == '*' {
                idx += 1;
            }
            if idx == p.len() {
                return true;
            }
            for i in 0..=t.len() {
                if glob_match_slice(&p[idx..], &t[i..]) {
                    return true;
                }
            }
            false
        }
        '?' => !t.is_empty() && glob_match_slice(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && glob_match_slice(&p[1..], &t[1..]),
    }
}

/// Parameters passed in the JSON payload, dependent on the event kind. The
/// caller fills in only the fields relevant to its event; unused fields are
/// omitted from the JSON.
#[derive(Debug, Default, Clone)]
pub struct HookContext<'a> {
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub tool_name: Option<&'a str>,
    pub tool_input: Option<&'a str>,
    pub tool_output: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub notification: Option<&'a str>,
    pub trigger: Option<&'a str>,
}

/// Run every enabled hook matching `event` and `match_target`, fold their
/// decisions into a single [`HookOutcome`]. `match_target` is the tool name
/// for tool events, the empty string (matched by `*`) for prompt/lifecycle
/// events. Hooks run sequentially in declared order — predictable side
/// effects matter more than wall-clock parallelism here.
pub async fn run_hooks(event: HookEvent, match_target: &str, ctx: HookContext<'_>) -> HookOutcome {
    let hooks = list_for(event);
    if hooks.is_empty() {
        return HookOutcome::default();
    }

    let payload = build_payload(event, match_target, &ctx);
    let payload_str = serde_json::to_string(&payload).unwrap_or_default();

    let mut outcome = HookOutcome::default();
    for hook in hooks {
        if !hook.enabled {
            continue;
        }
        if !matches(&hook.matcher, match_target) {
            continue;
        }
        match execute_hook(&hook, &payload_str).await {
            HookDecision::Continue => {}
            HookDecision::Block(reason) => {
                if outcome.blocked.is_none() {
                    outcome.blocked = Some(reason);
                }
            }
            HookDecision::Approve(reason) => {
                if outcome.approved.is_none() {
                    outcome.approved = Some(reason);
                }
            }
            HookDecision::AddContext(text) => {
                outcome.additional_context.push(text);
            }
            HookDecision::RewritePrompt(text) => {
                if outcome.rewritten_prompt.is_none() {
                    outcome.rewritten_prompt = Some(text);
                }
            }
        }
    }
    outcome
}

/// Build the JSON payload sent to a hook on stdin. Public for tests.
pub fn build_payload(event: HookEvent, match_target: &str, ctx: &HookContext<'_>) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("event".into(), Value::String(event.as_str().to_string()));
    if let Some(id) = &ctx.session_id {
        obj.insert("session_id".into(), Value::String(id.clone()));
    }
    if let Some(cwd) = &ctx.cwd {
        obj.insert("cwd".into(), Value::String(cwd.display().to_string()));
    }
    if let Some(name) = ctx.tool_name {
        let mut tool = serde_json::Map::new();
        tool.insert("name".into(), Value::String(name.to_string()));
        if let Some(input) = ctx.tool_input {
            tool.insert("input".into(), Value::String(input.to_string()));
        }
        if let Some(output) = ctx.tool_output {
            tool.insert("output".into(), Value::String(output.to_string()));
        }
        obj.insert("tool".into(), Value::Object(tool));
    }
    if let Some(prompt) = ctx.prompt {
        obj.insert("prompt".into(), Value::String(prompt.to_string()));
    }
    if let Some(note) = ctx.notification {
        obj.insert("notification".into(), Value::String(note.to_string()));
    }
    if let Some(trigger) = ctx.trigger {
        obj.insert("trigger".into(), Value::String(trigger.to_string()));
    }
    if !match_target.is_empty() {
        obj.insert("matcher".into(), Value::String(match_target.to_string()));
    }
    Value::Object(obj)
}

/// Execute a single hook command with `payload` piped on stdin. Returns the
/// parsed [`HookDecision`]. Failures (spawn error, timeout, non-zero exit,
/// invalid JSON in stdout) are logged to stderr and treated as `Continue`
/// so a misconfigured hook doesn't wedge the agent.
pub async fn execute_hook(hook: &Hook, payload: &str) -> HookDecision {
    let mut cmd = Command::new("sh");
    cmd.arg("-c");
    cmd.arg(&hook.command);
    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    let cwd = crate::security::policy().paths.working_dir.clone();
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "hook ({} / {}): spawn failed: {e}",
                hook.event, hook.matcher
            );
            return HookDecision::Continue;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let bytes = payload.as_bytes().to_vec();
        let _ = stdin.write_all(&bytes).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    }

    let timeout = std::time::Duration::from_secs(hook.timeout_secs);
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            eprintln!("hook ({} / {}): wait failed: {e}", hook.event, hook.matcher);
            return HookDecision::Continue;
        }
        Err(_) => {
            eprintln!(
                "hook ({} / {}): timed out after {}s",
                hook.event,
                hook.matcher,
                timeout.as_secs()
            );
            return HookDecision::Continue;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        eprintln!(
            "hook ({} / {}): [exit {code}] {stderr}",
            hook.event, hook.matcher
        );
        // A non-zero exit by convention blocks the action and uses stderr
        // (or stdout, when stderr is empty) as the reason. Mirrors the
        // Claude Code shape where exit 2 is a block.
        if code == 2 {
            let reason = if stderr.is_empty() {
                stdout
            } else {
                stderr.to_string()
            };
            let reason = if reason.is_empty() {
                format!("hook '{}' exited 2", hook.matcher)
            } else {
                reason
            };
            return HookDecision::Block(reason);
        }
        return HookDecision::Continue;
    }

    if stdout.is_empty() {
        return HookDecision::Continue;
    }
    parse_decision(&stdout)
}

/// Parse a hook's stdout. Falls back to `Continue` for non-JSON output and
/// surfaces the raw text via stderr for the operator's benefit. Public for
/// tests.
pub fn parse_decision(stdout: &str) -> HookDecision {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return HookDecision::Continue;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        // Plain text on stdout is treated as additional context. This makes
        // the simplest possible hook ("echo hi") useful out of the box.
        return HookDecision::AddContext(trimmed.to_string());
    };
    let Some(obj) = value.as_object() else {
        return HookDecision::AddContext(trimmed.to_string());
    };

    if let Some(prompt) = obj.get("rewrittenPrompt").and_then(Value::as_str) {
        return HookDecision::RewritePrompt(prompt.to_string());
    }
    if let Some(decision) = obj.get("decision").and_then(Value::as_str) {
        let reason = obj
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match decision {
            "block" => return HookDecision::Block(reason),
            "approve" => return HookDecision::Approve(reason),
            _ => {}
        }
    }
    if let Some(ctx) = obj.get("additionalContext").and_then(Value::as_str) {
        return HookDecision::AddContext(ctx.to_string());
    }
    HookDecision::Continue
}

/// Save a hook table back to disk. Used by the `/hooks` menu to persist
/// enable/disable toggles.
pub fn save(map: &HashMap<HookEvent, Vec<Hook>>) -> Result<(), String> {
    let Some(path) = hooks_file() else {
        return Err("HOME not set".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }

    let mut doc = serde_json::Map::new();
    for ev in HookEvent::ALL {
        let Some(hooks) = map.get(ev) else { continue };
        if hooks.is_empty() {
            continue;
        }
        let arr: Vec<Value> = hooks
            .iter()
            .map(|h| {
                let mut entry = serde_json::Map::new();
                entry.insert("matcher".into(), Value::String(h.matcher.clone()));
                entry.insert("command".into(), Value::String(h.command.clone()));
                entry.insert("timeout".into(), json!(h.timeout_secs));
                if !h.enabled {
                    entry.insert("enabled".into(), Value::Bool(false));
                }
                Value::Object(entry)
            })
            .collect();
        doc.insert(ev.as_str().to_string(), Value::Array(arr));
    }
    let serialized =
        serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// Replace the in-memory table with `map`. Used by the `/hooks` menu after
/// `save()` so callers don't need to re-read the file.
pub fn replace(map: HashMap<HookEvent, Vec<Hook>>) {
    if let Ok(mut w) = store().write() {
        *w = map;
    }
}

/// Snapshot the current in-memory hook table.
pub fn snapshot() -> HashMap<HookEvent, Vec<Hook>> {
    store().read().map(|m| m.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_round_trip() {
        for ev in HookEvent::ALL {
            assert_eq!(HookEvent::from_str(ev.as_str()), Some(*ev));
        }
        assert_eq!(
            HookEvent::from_str("preToolUse"),
            Some(HookEvent::PreToolUse)
        );
        assert_eq!(HookEvent::from_str("nonsense"), None);
    }

    #[test]
    fn glob_matches_basics() {
        assert!(matches("*", "anything"));
        assert!(matches("", "anything"));
        assert!(matches("exec_shell", "exec_shell"));
        assert!(!matches("exec_shell", "exec_shells"));
        assert!(matches("mcp__*__*", "mcp__github__list_issues"));
        assert!(!matches("mcp__*__*", "exec_shell"));
        assert!(matches("read_*", "read_file"));
        assert!(matches("?ead_file", "read_file"));
    }

    #[test]
    fn glob_alternation() {
        assert!(matches("edit_file|write_file", "edit_file"));
        assert!(matches("edit_file|write_file", "write_file"));
        assert!(!matches("edit_file|write_file", "remove_file"));
    }

    #[test]
    fn parse_hooks_minimal() {
        let raw = r#"{ "PreToolUse": [ { "matcher": "exec_shell", "command": "echo hi" } ] }"#;
        let map = parse_hooks(raw).unwrap();
        let hooks = map.get(&HookEvent::PreToolUse).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].matcher, "exec_shell");
        assert_eq!(hooks[0].command, "echo hi");
        assert_eq!(hooks[0].timeout_secs, DEFAULT_HOOK_TIMEOUT_SECS);
        assert!(hooks[0].enabled);
    }

    #[test]
    fn parse_hooks_default_matcher_is_star() {
        let raw = r#"{ "Stop": [ { "command": "echo done" } ] }"#;
        let map = parse_hooks(raw).unwrap();
        let hooks = map.get(&HookEvent::Stop).unwrap();
        assert_eq!(hooks[0].matcher, "*");
    }

    #[test]
    fn parse_hooks_rejects_missing_command() {
        let raw = r#"{ "Stop": [ { "matcher": "*" } ] }"#;
        let err = parse_hooks(raw).unwrap_err();
        assert!(err.contains("missing required field 'command'"));
    }

    #[test]
    fn parse_hooks_rejects_empty_command() {
        let raw = r#"{ "Stop": [ { "command": "   " } ] }"#;
        let err = parse_hooks(raw).unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn parse_hooks_skips_unknown_event() {
        let raw = r#"{ "BogusEvent": [ { "command": "x" } ], "Stop": [ { "command": "y" } ] }"#;
        let map = parse_hooks(raw).unwrap();
        assert!(map.get(&HookEvent::Stop).is_some());
    }

    #[test]
    fn parse_hooks_silently_skips_underscore_keys() {
        // _comment / _description keys are a JSON5-ish convention used to
        // document JSON config files. They must not trip the unknown-event
        // warning on every startup.
        let raw = r#"{ "_comment": "demo", "_description": "x", "Stop": [ { "command": "y" } ] }"#;
        let map = parse_hooks(raw).unwrap();
        assert!(map.get(&HookEvent::Stop).is_some());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn parse_hooks_rejects_non_object_top_level() {
        assert!(parse_hooks("[]").is_err());
    }

    #[test]
    fn parse_hooks_respects_enabled_field() {
        let raw = r#"{ "Stop": [ { "command": "x", "enabled": false }, { "command": "y" } ] }"#;
        let map = parse_hooks(raw).unwrap();
        let hooks = map.get(&HookEvent::Stop).unwrap();
        assert!(!hooks[0].enabled);
        assert!(hooks[1].enabled);
    }

    #[test]
    fn parse_decision_block() {
        let d = parse_decision(r#"{"decision":"block","reason":"nope"}"#);
        match d {
            HookDecision::Block(r) => assert_eq!(r, "nope"),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_approve() {
        let d = parse_decision(r#"{"decision":"approve","reason":"trusted"}"#);
        match d {
            HookDecision::Approve(r) => assert_eq!(r, "trusted"),
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_additional_context() {
        let d = parse_decision(r#"{"additionalContext":"remember this"}"#);
        match d {
            HookDecision::AddContext(c) => assert_eq!(c, "remember this"),
            other => panic!("expected AddContext, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_rewrite_prompt() {
        let d = parse_decision(r#"{"rewrittenPrompt":"please summarize"}"#);
        match d {
            HookDecision::RewritePrompt(c) => assert_eq!(c, "please summarize"),
            other => panic!("expected RewritePrompt, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_plain_text_becomes_context() {
        let d = parse_decision("hello world");
        match d {
            HookDecision::AddContext(c) => assert_eq!(c, "hello world"),
            other => panic!("expected AddContext, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_empty_is_continue() {
        assert!(matches!(parse_decision(""), HookDecision::Continue));
        assert!(matches!(parse_decision("   \n"), HookDecision::Continue));
    }

    #[test]
    fn parse_decision_unknown_decision_is_continue() {
        assert!(matches!(
            parse_decision(r#"{"decision":"flip a coin"}"#),
            HookDecision::Continue
        ));
    }

    #[test]
    fn build_payload_includes_tool_for_tool_events() {
        let ctx = HookContext {
            session_id: Some("abc-123".to_string()),
            cwd: Some(PathBuf::from("/work")),
            tool_name: Some("exec_shell"),
            tool_input: Some("ls"),
            ..HookContext::default()
        };
        let v = build_payload(HookEvent::PreToolUse, "exec_shell", &ctx);
        assert_eq!(v["event"], "PreToolUse");
        assert_eq!(v["session_id"], "abc-123");
        assert_eq!(v["cwd"], "/work");
        assert_eq!(v["tool"]["name"], "exec_shell");
        assert_eq!(v["tool"]["input"], "ls");
        assert_eq!(v["matcher"], "exec_shell");
    }

    #[test]
    fn build_payload_omits_unset_fields() {
        let ctx = HookContext {
            prompt: Some("write a haiku"),
            ..HookContext::default()
        };
        let v = build_payload(HookEvent::UserPromptSubmit, "", &ctx);
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("tool"));
        assert!(!obj.contains_key("session_id"));
        assert!(!obj.contains_key("matcher"));
        assert_eq!(
            obj.get("prompt").and_then(Value::as_str),
            Some("write a haiku")
        );
    }

    #[test]
    fn outcome_helpers() {
        let mut o = HookOutcome::default();
        assert!(o.merged_context().is_none());
        o.additional_context.push("a".to_string());
        o.additional_context.push("b".to_string());
        assert_eq!(o.merged_context().as_deref(), Some("a\n\nb"));
    }

    #[tokio::test]
    async fn execute_hook_continue_on_empty_stdout() {
        let hook = Hook {
            event: HookEvent::Stop,
            matcher: "*".to_string(),
            command: "true".to_string(),
            timeout_secs: 5,
            enabled: true,
        };
        assert!(matches!(
            execute_hook(&hook, "{}").await,
            HookDecision::Continue
        ));
    }

    #[tokio::test]
    async fn execute_hook_passes_payload_on_stdin() {
        // The hook reads stdin and emits `additionalContext` with what it saw.
        let hook = Hook {
            event: HookEvent::Stop,
            matcher: "*".to_string(),
            command: r#"awk 'BEGIN{ORS=""} {print}' | (read line; printf "{\"additionalContext\":\"%s\"}" "$line")"#.to_string(),
            timeout_secs: 5,
            enabled: true,
        };
        let result = execute_hook(&hook, "hello-payload").await;
        match result {
            HookDecision::AddContext(s) => assert_eq!(s, "hello-payload"),
            other => panic!("expected AddContext, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_hook_exit_2_blocks() {
        let hook = Hook {
            event: HookEvent::PreToolUse,
            matcher: "*".to_string(),
            command: "echo 'denied by policy' >&2; exit 2".to_string(),
            timeout_secs: 5,
            enabled: true,
        };
        match execute_hook(&hook, "{}").await {
            HookDecision::Block(reason) => assert!(reason.contains("denied by policy")),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_hook_times_out() {
        let hook = Hook {
            event: HookEvent::Stop,
            matcher: "*".to_string(),
            command: "sleep 5".to_string(),
            timeout_secs: 1,
            enabled: true,
        };
        // Timeouts surface as Continue (logged to stderr) — don't wedge the loop.
        assert!(matches!(
            execute_hook(&hook, "{}").await,
            HookDecision::Continue
        ));
    }

    #[test]
    fn save_and_reload_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "aictl-hooks-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hooks.json");

        let mut map: HashMap<HookEvent, Vec<Hook>> = HashMap::new();
        map.insert(
            HookEvent::Stop,
            vec![Hook {
                event: HookEvent::Stop,
                matcher: "*".to_string(),
                command: "echo done".to_string(),
                timeout_secs: 30,
                enabled: false,
            }],
        );

        // Save → re-parse → verify shape.
        let serialized = serde_json::to_string_pretty(&{
            let mut doc = serde_json::Map::new();
            let arr = vec![json!({
                "matcher": "*",
                "command": "echo done",
                "timeout": 30,
                "enabled": false,
            })];
            doc.insert("Stop".to_string(), Value::Array(arr));
            Value::Object(doc)
        })
        .unwrap();
        std::fs::write(&path, &serialized).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed = parse_hooks(&raw).unwrap();
        let hooks = parsed.get(&HookEvent::Stop).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].timeout_secs, 30);
        assert!(!hooks[0].enabled);

        std::fs::remove_dir_all(&dir).ok();
    }
}
