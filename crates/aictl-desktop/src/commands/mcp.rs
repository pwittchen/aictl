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
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    /// Remote-only: empty for stdio entries.
    pub url: String,
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
            let (transport, command, args, url, state, state_detail, tool_count) = match summary {
                Some(s) => {
                    let (state, detail) = match &s.state {
                        mcp::ServerState::Ready => ("ready", None),
                        mcp::ServerState::Failed(r) => ("failed", Some(r.clone())),
                        mcp::ServerState::Disabled => ("disabled", None),
                    };
                    (
                        s.transport.as_str().to_string(),
                        s.command.clone(),
                        s.args.clone(),
                        s.url.clone(),
                        state.to_string(),
                        detail,
                        s.tools.len(),
                    )
                }
                None => (
                    "stdio".to_string(),
                    String::new(),
                    vec![],
                    String::new(),
                    "unknown".to_string(),
                    None,
                    0,
                ),
            };
            McpServerRow {
                name: name.clone(),
                transport,
                command,
                args,
                url,
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
    /// `"stdio"` (default), `"http"`, or `"sse"`. Validated against the
    /// same set the parser accepts.
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Remote-only: dispatch URL.
    #[serde(default)]
    pub url: String,
    /// Remote-only: extra HTTP headers (Authorization, etc.).
    #[serde(default)]
    pub headers: HashMap<String, String>,
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
    let transport_raw = args
        .transport
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("stdio")
        .to_string();
    match transport_raw.as_str() {
        "stdio" | "http" | "sse" => {}
        other => return Err(format!("unknown transport '{other}'")),
    }
    let is_remote = transport_raw != "stdio";
    let command = args.command.trim().to_string();
    let url = args.url.trim().to_string();
    if is_remote {
        if url.is_empty() {
            return Err("url is empty".to_string());
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("url must start with http:// or https://".to_string());
        }
    } else if command.is_empty() {
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
    if is_remote {
        entry.insert("transport".into(), Value::String(transport_raw));
        entry.insert("url".into(), Value::String(url));
        if !args.headers.is_empty() {
            let header_map: Map<String, Value> = args
                .headers
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            entry.insert("headers".into(), Value::Object(header_map));
        }
    } else {
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

/// Tear down the live MCP catalogue and rebuild it from config. The
/// composer's MCP toggle calls this right after flipping
/// `AICTL_MCP_ENABLED` so a disable closes every spawned child and an
/// enable spawns the configured servers without an app restart.
#[tauri::command]
pub async fn mcp_reload() {
    mcp::reload().await;
}

#[derive(Deserialize)]
pub struct McpNameArgs {
    pub name: String,
}

/// Delete a server entry from `mcp.json`. Removes the key under
/// `mcpServers` and writes the document back; everything else (other
/// servers, top-level keys) is preserved. The running process keeps its
/// in-memory catalogue until the next restart or `mcp_reload`.
#[tauri::command]
pub fn mcp_delete(args: McpNameArgs) -> Result<(), String> {
    let path = mcp_config_path();
    if !path.exists() {
        return Err("mcp.json does not exist".to_string());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read mcp.json: {e}"))?;
    let mut doc: Value = serde_json::from_str(&raw).map_err(|e| format!("parse mcp.json: {e}"))?;
    let map = doc
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "missing 'mcpServers' object".to_string())?;
    if map.remove(&args.name).is_none() {
        return Err(format!("server '{}' not found", args.name));
    }
    let serialized = serde_json::to_string_pretty(&doc).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// One tool surfaced by a running MCP server. The View modal shows the
/// list so the user can see what they're connected to without opening
/// the JSON config.
#[derive(Serialize)]
pub struct McpToolRow {
    pub name: String,
    pub description: String,
}

/// Full picture of a single server — what the View modal renders. Pulls
/// the on-disk entry (transport, command, args, env, url, headers,
/// timeout, enabled) and merges the runtime view (state, tools).
#[derive(Serialize)]
pub struct McpServerDetails {
    pub name: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout_secs: Option<u64>,
    pub enabled: bool,
    pub state: String,
    pub state_detail: Option<String>,
    pub tools: Vec<McpToolRow>,
    pub config_path: String,
}

/// Read the on-disk entry plus the runtime catalogue for one server.
/// Errors when the file or the named entry is missing — the View
/// button is only rendered once the row exists, so this is paranoid.
#[tauri::command]
pub fn mcp_details(args: McpNameArgs) -> Result<McpServerDetails, String> {
    let path = mcp_config_path();
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read mcp.json: {e}"))?;
    let doc: Value = serde_json::from_str(&raw).map_err(|e| format!("parse mcp.json: {e}"))?;
    let entry = doc
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|m| m.get(&args.name))
        .and_then(Value::as_object)
        .ok_or_else(|| format!("server '{}' not found", args.name))?;

    let transport = entry
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("stdio")
        .to_string();
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let args_vec = entry
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env = entry
        .get("env")
        .and_then(Value::as_object)
        .map(|m| {
            let mut pairs: Vec<(String, String)> = m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs
        })
        .unwrap_or_default();
    let url = entry
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let headers = entry
        .get("headers")
        .and_then(Value::as_object)
        .map(|m| {
            let mut pairs: Vec<(String, String)> = m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs
        })
        .unwrap_or_default();
    let timeout_secs = entry.get("timeout_secs").and_then(Value::as_u64);
    let enabled = entry
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let runtime = mcp::list().into_iter().find(|s| s.name == args.name);
    let (state, state_detail, tools) = match runtime {
        Some(s) => {
            let (state, detail) = match &s.state {
                mcp::ServerState::Ready => ("ready".to_string(), None),
                mcp::ServerState::Failed(r) => ("failed".to_string(), Some(r.clone())),
                mcp::ServerState::Disabled => ("disabled".to_string(), None),
            };
            let tools = s
                .tools
                .iter()
                .map(|t| McpToolRow {
                    name: t.name.clone(),
                    description: t.description.clone(),
                })
                .collect();
            (state, detail, tools)
        }
        None => ("unknown".to_string(), None, Vec::new()),
    };

    Ok(McpServerDetails {
        name: args.name,
        transport,
        command,
        args: args_vec,
        env,
        url,
        headers,
        timeout_secs,
        enabled,
        state,
        state_detail,
        tools,
        config_path: path.display().to_string(),
    })
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
