//! MCP-server pane Tauri commands.
//!
//! Reads `~/.aictl/mcp.json` (or `AICTL_MCP_CONFIG`) and surfaces a list
//! the Settings UI can render. Toggling an entry rewrites the file with
//! `enabled: true|false`; the change picks up on the next process launch
//! (`mcp::init` only runs once). The `mcp_create` handler appends a new
//! entry to the same document so the desktop can author servers without
//! the user editing JSON by hand.

use std::collections::HashMap;
use std::path::PathBuf;

use aictl_core::mcp;
use aictl_core::mcp::config::{config_path as mcp_config_path, is_valid_name};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One row in the MCP panel — same fields the CLI's `/mcp` menu shows
/// plus the on-disk `enabled` flag so the toggle reflects file state
/// (not just whatever ran for this process).
#[derive(Serialize)]
pub struct McpServerRow {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub state: String,
    pub state_detail: Option<String>,
    pub tool_count: usize,
}

#[derive(Serialize)]
pub struct McpStatus {
    pub enabled: bool,
    pub config_path: String,
    pub config_exists: bool,
    pub servers: Vec<McpServerRow>,
}

#[tauri::command]
pub fn mcp_status() -> McpStatus {
    let path = mcp_config_path();
    let config_exists = path.exists();
    let on_disk = read_enabled_map(&path).unwrap_or_default();
    let runtime: HashMap<String, mcp::ServerSummary> = mcp::list()
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();

    let mut rows: Vec<McpServerRow> = on_disk
        .keys()
        .map(|name| {
            let enabled = *on_disk.get(name).unwrap_or(&true);
            let summary = runtime.get(name);
            let (command, args, state, state_detail, tool_count) = match summary {
                Some(s) => {
                    let (state, detail) = match &s.state {
                        mcp::ServerState::Ready => ("ready", None),
                        mcp::ServerState::Failed(r) => ("failed", Some(r.clone())),
                        mcp::ServerState::Disabled => ("disabled", None),
                    };
                    (
                        s.command.clone(),
                        s.args.clone(),
                        state.to_string(),
                        detail,
                        s.tools.len(),
                    )
                }
                None => (String::new(), vec![], "unknown".to_string(), None, 0),
            };
            McpServerRow {
                name: name.clone(),
                command,
                args,
                enabled,
                state,
                state_detail,
                tool_count,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    McpStatus {
        enabled: mcp::enabled(),
        config_path: path.display().to_string(),
        config_exists,
        servers: rows,
    }
}

#[derive(Deserialize)]
pub struct McpToggleArgs {
    pub name: String,
    pub enabled: bool,
}

/// Flip a server's `enabled` flag in `mcp.json`. Round-trips the JSON
/// document so unrelated keys (`env`, `args`, `timeout_secs`) survive
/// the rewrite.
#[tauri::command]
pub fn mcp_toggle(args: McpToggleArgs) -> Result<bool, String> {
    let path = mcp_config_path();
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read mcp.json: {e}"))?;
    let mut doc: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse mcp.json: {e}"))?;
    let map = doc
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "missing 'mcpServers' object".to_string())?;
    let entry = map
        .get_mut(&args.name)
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| format!("server '{}' not found", args.name))?;
    entry.insert("enabled".into(), serde_json::Value::Bool(args.enabled));
    let serialized = serde_json::to_string_pretty(&doc).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("write: {e}"))?;
    Ok(args.enabled)
}

#[derive(Deserialize)]
pub struct McpCreateArgs {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub timeout_secs: Option<u64>,
    /// `true` once the user has confirmed they want to clobber an
    /// existing server of the same name.
    #[serde(default)]
    pub overwrite: bool,
}

/// Append a new server entry to `mcp.json`. Creates the file with an
/// empty `mcpServers` object if it didn't exist yet — same shape
/// `mcp::config::parse` expects on first read. Refuses to overwrite an
/// existing entry unless the caller passes `overwrite: true`; the
/// dialog confirms in JS before retrying.
#[tauri::command]
pub fn mcp_create(args: McpCreateArgs) -> Result<(), String> {
    let name = args.name.trim().to_string();
    if !is_valid_name(&name) {
        return Err("invalid name — use only letters, numbers, underscore, or dash".to_string());
    }
    let command = args.command.trim().to_string();
    if command.is_empty() {
        return Err("command is empty".to_string());
    }
    if let Some(t) = args.timeout_secs
        && t == 0
    {
        return Err("timeout must be greater than zero".to_string());
    }

    let path = mcp_config_path();
    let mut doc: Value = if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("read mcp.json: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("parse mcp.json: {e}"))?
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut root = Map::new();
        root.insert("mcpServers".into(), Value::Object(Map::new()));
        Value::Object(root)
    };

    let map = doc
        .as_object_mut()
        .and_then(|root| {
            root.entry("mcpServers")
                .or_insert_with(|| Value::Object(Map::new()));
            root.get_mut("mcpServers")
                .and_then(serde_json::Value::as_object_mut)
        })
        .ok_or_else(|| "mcp.json root must be an object".to_string())?;

    if map.contains_key(&name) && !args.overwrite {
        return Err(format!("server '{name}' already exists"));
    }

    let mut entry = Map::new();
    entry.insert("command".into(), Value::String(command));
    if !args.args.is_empty() {
        entry.insert(
            "args".into(),
            Value::Array(args.args.into_iter().map(Value::String).collect()),
        );
    }
    if !args.env.is_empty() {
        let env_map: Map<String, Value> = args
            .env
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        entry.insert("env".into(), Value::Object(env_map));
    }
    entry.insert("enabled".into(), Value::Bool(true));
    if let Some(t) = args.timeout_secs {
        entry.insert("timeout_secs".into(), Value::Number(t.into()));
    }

    map.insert(name, Value::Object(entry));

    let serialized = serde_json::to_string_pretty(&doc).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

fn read_enabled_map(path: &PathBuf) -> Option<HashMap<String, bool>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let map = v.get("mcpServers")?.as_object()?;
    let mut out = HashMap::new();
    for (name, entry) in map {
        let enabled = entry
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        out.insert(name.clone(), enabled);
    }
    Some(out)
}
