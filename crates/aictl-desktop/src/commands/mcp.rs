//! MCP-server pane Tauri commands.
//!
//! Reads `~/.aictl/mcp.json` (or `AICTL_MCP_CONFIG`) and surfaces a list
//! the Settings UI can render. Toggling an entry rewrites the file with
//! `enabled: true|false`; the change picks up on the next process launch
//! (`mcp::init` only runs once).

use std::collections::HashMap;
use std::path::PathBuf;

use aictl_core::mcp;
use aictl_core::mcp::config::config_path as mcp_config_path;
use serde::{Deserialize, Serialize};

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
