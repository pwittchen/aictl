//! Settings-pane Tauri commands.
//!
//! The Settings overlay in the webview reads and writes through these
//! handlers; everything they touch lives in `~/.aictl/config` or the
//! system keyring (via `aictl_core::keys`). The CLI's slash commands
//! (`/config`, `/keys`, `/security`, `/memory`) target the same keys, so
//! changes round-trip between the two binaries.
//!
//! Whitelisting which config keys are settable from the desktop is
//! deliberate: the Settings UI exposes a small surface — workspace,
//! provider/model, security flags, memory mode, auto-compact threshold,
//! LLM timeout — and the catch-all `config_set` command refuses anything
//! outside that set. A future "Advanced" tab can grow the list.
//! Workspace / provider-model edits go through their dedicated commands
//! (`set_workspace`, `set_active_model`) rather than this generic path.

use aictl_core::config::{self, AICTL_WORKING_DIR_DESKTOP};
use aictl_core::keys::{self, ClearOutcome, KeyLocation, LockOutcome, SetOutcome, UnlockOutcome};
use aictl_core::tools::BUILTIN_TOOLS;
use serde::{Deserialize, Serialize};

/// Names that the desktop Settings UI is allowed to read/write through
/// the generic [`config_get`] / [`config_set`] / [`config_unset`]
/// handlers. Workspace + provider/model live behind their own commands.
const ALLOWED_CONFIG_KEYS: &[&str] = &[
    "AICTL_AUTO_COMPACT_THRESHOLD",
    "AICTL_LLM_TIMEOUT",
    "AICTL_MAX_ITERATIONS",
    "AICTL_MEMORY",
    "AICTL_SECURITY",
    "AICTL_SECURITY_INJECTION_GUARD",
    "AICTL_SECURITY_AUDIT_LOG",
    "AICTL_SECURITY_REDACTION",
    "AICTL_SECURITY_REDACTION_LOCAL",
    "AICTL_SECURITY_CWD_RESTRICT",
    "AICTL_SECURITY_BLOCK_SUBSHELL",
    "AICTL_SECURITY_DISABLED_TOOLS",
    "AICTL_SECURITY_SHELL_ALLOWED",
    "AICTL_SECURITY_SHELL_BLOCKED",
    "AICTL_SECURITY_BLOCKED_PATHS",
    "AICTL_SECURITY_ALLOWED_PATHS",
    "AICTL_SECURITY_BLOCKED_ENV",
    "AICTL_SECURITY_SHELL_TIMEOUT",
    "AICTL_SECURITY_MAX_WRITE",
    "AICTL_REDACTION_DETECTORS",
    "AICTL_REDACTION_EXTRA_PATTERNS",
    "AICTL_REDACTION_ALLOW",
    "AICTL_REDACTION_NER",
    "AICTL_TOOLS_ENABLED",
    "AICTL_TOOL_APPROVAL",
    "AICTL_PROMPT_FALLBACK",
    "AICTL_STREAMING",
    "AICTL_BEHAVIOR",
    "AICTL_CLIENT_HOST",
    "AICTL_MCP_ENABLED",
    "AICTL_MCP_DENY_SERVERS",
    "AICTL_PLUGINS_ENABLED",
    "AICTL_DESKTOP_THEME",
    "AICTL_DESKTOP_DENSITY",
    "AICTL_DESKTOP_NOTIFICATIONS",
    "LLM_OLLAMA_HOST",
];

#[derive(Serialize)]
pub struct ConfigEntry {
    pub key: &'static str,
    pub value: Option<String>,
}

#[tauri::command]
pub fn config_dump() -> Vec<ConfigEntry> {
    ALLOWED_CONFIG_KEYS
        .iter()
        .map(|k| ConfigEntry {
            key: k,
            value: config::config_get(k),
        })
        .collect()
}

#[derive(Deserialize)]
pub struct ConfigGetArgs {
    pub key: String,
}

#[tauri::command]
pub fn config_value(args: ConfigGetArgs) -> Result<Option<String>, String> {
    if !is_allowed(&args.key) {
        return Err(format!("config key '{}' is not user-settable", args.key));
    }
    Ok(config::config_get(&args.key))
}

#[derive(Deserialize)]
pub struct ConfigSetArgs {
    pub key: String,
    pub value: String,
}

#[tauri::command]
pub fn config_write(args: ConfigSetArgs) -> Result<(), String> {
    if !is_allowed(&args.key) {
        return Err(format!("config key '{}' is not user-settable", args.key));
    }
    if args.value.trim().is_empty() {
        config::config_unset(&args.key);
    } else {
        config::config_set(&args.key, &args.value);
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ConfigUnsetArgs {
    pub key: String,
}

#[tauri::command]
pub fn config_clear(args: ConfigUnsetArgs) -> Result<bool, String> {
    if !is_allowed(&args.key) {
        return Err(format!("config key '{}' is not user-settable", args.key));
    }
    Ok(config::config_unset(&args.key))
}

fn is_allowed(key: &str) -> bool {
    ALLOWED_CONFIG_KEYS.contains(&key) || key == AICTL_WORKING_DIR_DESKTOP
}

/// One row in the keys panel. Mirrors `aictl_core::keys::KEY_NAMES`
/// without ever returning the secret itself — the webview only renders
/// presence + storage location.
#[derive(Serialize)]
pub struct KeyRow {
    pub name: String,
    pub label: &'static str,
    pub location: &'static str,
}

#[derive(Serialize)]
pub struct KeyBackend {
    pub available: bool,
    pub name: &'static str,
}

#[tauri::command]
pub fn keys_status() -> Vec<KeyRow> {
    keys::all_locations()
        .into_iter()
        .map(|(name, loc)| KeyRow {
            name: name.to_string(),
            label: provider_label(name),
            location: location_str(loc),
        })
        .collect()
}

#[tauri::command]
pub fn keys_backend() -> KeyBackend {
    KeyBackend {
        available: keys::backend_available(),
        name: keys::backend_name(),
    }
}

#[derive(Deserialize)]
pub struct KeySetArgs {
    pub name: String,
    pub value: String,
}

/// Persist a key. If the system keyring is available the value lands
/// there; otherwise it falls back to plain `~/.aictl/config`. The CLI's
/// `/keys` lock/unlock flow can later migrate between the two stores.
#[tauri::command]
pub fn keys_set(args: KeySetArgs) -> Result<&'static str, String> {
    if !is_known_key(&args.name) {
        return Err(format!("unknown key '{}'", args.name));
    }
    let trimmed = args.value.trim();
    if trimmed.is_empty() {
        return Err("value is empty — use keys_clear to delete".to_string());
    }
    match keys::set_secret(&args.name, trimmed) {
        SetOutcome::Keyring => Ok("keyring"),
        SetOutcome::Plain => Ok("plain"),
        SetOutcome::Error(reason) => Err(reason),
    }
}

#[derive(Deserialize)]
pub struct KeyClearArgs {
    pub name: String,
}

#[tauri::command]
pub fn keys_clear(args: KeyClearArgs) -> Result<&'static str, String> {
    if !is_known_key(&args.name) {
        return Err(format!("unknown key '{}'", args.name));
    }
    match keys::clear_key(&args.name) {
        ClearOutcome::Cleared => Ok("cleared"),
        ClearOutcome::NotPresent => Ok("not_present"),
        ClearOutcome::Error(reason) => Err(reason),
    }
}

#[derive(Deserialize)]
pub struct KeyLockArgs {
    pub name: String,
}

/// Migrate one key from plain config into the system keyring. Mirrors
/// the CLI's `/keys → lock` action but scoped to a single row.
#[tauri::command]
pub fn keys_lock(args: KeyLockArgs) -> Result<&'static str, String> {
    if !is_known_key(&args.name) {
        return Err(format!("unknown key '{}'", args.name));
    }
    if !keys::backend_available() {
        return Err(format!(
            "system keyring is not available (backend: {})",
            keys::backend_name()
        ));
    }
    match keys::lock_key(&args.name) {
        LockOutcome::Locked => Ok("locked"),
        LockOutcome::AlreadyLocked => Ok("already_locked"),
        LockOutcome::NotInConfig => Ok("not_in_config"),
        LockOutcome::Error(reason) => Err(reason),
    }
}

#[derive(Deserialize)]
pub struct KeyUnlockArgs {
    pub name: String,
}

/// Migrate one key from the system keyring back into plain config.
/// Mirrors the CLI's `/keys → unlock` action but scoped to a single row.
#[tauri::command]
pub fn keys_unlock(args: KeyUnlockArgs) -> Result<&'static str, String> {
    if !is_known_key(&args.name) {
        return Err(format!("unknown key '{}'", args.name));
    }
    if !keys::backend_available() {
        return Err(format!(
            "system keyring is not available (backend: {})",
            keys::backend_name()
        ));
    }
    match keys::unlock_key(&args.name) {
        UnlockOutcome::Unlocked => Ok("unlocked"),
        UnlockOutcome::AlreadyUnlocked => Ok("already_unlocked"),
        UnlockOutcome::NotInKeyring => Ok("not_in_keyring"),
        UnlockOutcome::Error(reason) => Err(reason),
    }
}

fn is_known_key(name: &str) -> bool {
    keys::KEY_NAMES.contains(&name)
}

fn provider_label(name: &str) -> &'static str {
    match name {
        "LLM_ANTHROPIC_API_KEY" => "Anthropic",
        "LLM_OPENAI_API_KEY" => "OpenAI",
        "LLM_GEMINI_API_KEY" => "Gemini",
        "LLM_GROK_API_KEY" => "Grok",
        "LLM_MISTRAL_API_KEY" => "Mistral",
        "LLM_DEEPSEEK_API_KEY" => "DeepSeek",
        "LLM_KIMI_API_KEY" => "Kimi",
        "LLM_ZAI_API_KEY" => "Z.ai",
        "FIRECRAWL_API_KEY" => "Firecrawl",
        "AICTL_CLIENT_MASTER_KEY" => "aictl-server master key",
        _ => "",
    }
}

fn location_str(loc: KeyLocation) -> &'static str {
    match loc {
        KeyLocation::None => "unset",
        KeyLocation::Config => "plain",
        KeyLocation::Keyring => "keyring",
        KeyLocation::Both => "both",
    }
}

/// One row in the tools panel — a built-in tool plus whether the user
/// has individually disabled it via `AICTL_SECURITY_DISABLED_TOOLS`.
#[derive(Serialize)]
pub struct ToolRow {
    pub name: &'static str,
    pub description: &'static str,
    pub disabled: bool,
}

#[tauri::command]
pub fn tools_list() -> Vec<ToolRow> {
    let disabled: Vec<String> = parse_csv(
        config::config_get("AICTL_SECURITY_DISABLED_TOOLS")
            .as_deref()
            .unwrap_or(""),
    );
    BUILTIN_TOOLS
        .iter()
        .map(|(name, description)| ToolRow {
            name,
            description,
            disabled: disabled.iter().any(|d| d == name),
        })
        .collect()
}

#[derive(Deserialize)]
pub struct ToolToggleArgs {
    pub name: String,
    pub disabled: bool,
}

/// Flip one tool's entry in the comma-separated
/// `AICTL_SECURITY_DISABLED_TOOLS` list. The CLI's `/security` reads the
/// same key, so the change round-trips between the two binaries.
#[tauri::command]
pub fn tool_set_disabled(args: ToolToggleArgs) -> Result<bool, String> {
    if !BUILTIN_TOOLS.iter().any(|(n, _)| *n == args.name) {
        return Err(format!("unknown tool '{}'", args.name));
    }
    let mut current: Vec<String> = parse_csv(
        config::config_get("AICTL_SECURITY_DISABLED_TOOLS")
            .as_deref()
            .unwrap_or(""),
    );
    let was_disabled = current.iter().any(|n| n == &args.name);
    if args.disabled && !was_disabled {
        current.push(args.name.clone());
    } else if !args.disabled && was_disabled {
        current.retain(|n| n != &args.name);
    } else {
        return Ok(args.disabled);
    }
    if current.is_empty() {
        config::config_unset("AICTL_SECURITY_DISABLED_TOOLS");
    } else {
        config::config_set("AICTL_SECURITY_DISABLED_TOOLS", &current.join(","));
    }
    Ok(args.disabled)
}

fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
