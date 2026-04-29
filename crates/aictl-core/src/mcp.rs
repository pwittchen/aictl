//! Model Context Protocol (MCP) client.
//!
//! MCP is a JSON-RPC protocol for exposing tools, resources, and prompts from
//! external servers to an LLM agent. Phase 1 covers the stdio transport and
//! tools only — see `.claude/plans/mcp-support.md` for the full roadmap.
//!
//! Master switch: `AICTL_MCP_ENABLED` (default `false`) — MCP servers are
//! third-party code and must be opted in deliberately, matching the plugin
//! gate. Server entries live in `~/.aictl/mcp.json` (override via
//! `AICTL_MCP_CONFIG`) in a shape compatible with Claude Desktop.
//!
//! Once enabled, [`init`] spawns each configured stdio server, completes the
//! `initialize` handshake, calls `tools/list`, and stores the merged catalogue.
//! Tools are exposed to the agent loop as `mcp__<server>__<tool>` and
//! dispatched via [`call_tool`] from `tools.rs::execute_tool`.

pub mod config;
pub mod protocol;
pub mod stdio;

use std::sync::Arc;
use std::sync::OnceLock;

use serde_json::Value;

use crate::config::config_get;
use protocol::RawTool;
use stdio::StdioClient;

/// Returns whether the MCP subsystem is opted in. Default `false`.
pub fn enabled() -> bool {
    matches!(config_get("AICTL_MCP_ENABLED").as_deref(), Some(v) if v != "false" && v != "0")
}

/// Lifetime state of one configured server.
#[derive(Debug, Clone)]
pub enum ServerState {
    Ready,
    Failed(String),
    Disabled,
}

/// Snapshot of one MCP tool, keyed by qualified name.
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// One configured server. The `client` is `None` when the server is disabled,
/// failed to spawn, or failed its handshake — every other field is still
/// populated so `/mcp` and `--list-mcp` can render meaningful output.
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub state: ServerState,
    pub tools: Vec<McpTool>,
    client: Option<Arc<StdioClient>>,
}

static SERVERS: OnceLock<Vec<McpServer>> = OnceLock::new();

fn servers() -> &'static [McpServer] {
    SERVERS.get().map_or(&[], Vec::as_slice)
}

/// Initialize the MCP catalogue. Idempotent: subsequent calls are no-ops.
///
/// Loads the config, spawns every enabled server in parallel, completes each
/// handshake under the startup-timeout, lists tools, and stores the merged
/// catalogue. Per-server failures are recorded in `ServerState::Failed` and
/// do not abort startup — MCP is always best-effort.
///
/// `only`, when `Some`, restricts startup to a single server name — every
/// other configured server is force-disabled for this process. Mirrors the
/// `--mcp-server <name>` CLI flag without persisting the disable list to
/// `~/.aictl/config`.
pub async fn init_with(only: Option<&str>) {
    if SERVERS.get().is_some() {
        return;
    }
    if !enabled() {
        let _ = SERVERS.set(Vec::new());
        return;
    }
    let entries = match config::load() {
        Ok(e) => e,
        Err(reason) => {
            eprintln!("mcp: config error — {reason}");
            let _ = SERVERS.set(Vec::new());
            return;
        }
    };
    let disabled = config::disabled_set();
    let startup_timeout = config::startup_timeout();
    let only = only.map(str::to_string);

    let futures = entries.into_iter().map(|cfg| {
        let disabled = disabled.clone();
        let only = only.clone();
        async move {
            let only_skip = only.as_ref().is_some_and(|n| n != &cfg.name);
            let force_disabled =
                !cfg.enabled || only_skip || disabled.iter().any(|d| d == &cfg.name);
            if force_disabled {
                return McpServer {
                    name: cfg.name,
                    command: cfg.command,
                    args: cfg.args,
                    state: ServerState::Disabled,
                    tools: vec![],
                    client: None,
                };
            }
            spawn_one(cfg, startup_timeout).await
        }
    });
    let collected = futures_util::future::join_all(futures).await;

    let _ = SERVERS.set(collected);
}

async fn spawn_one(cfg: config::ServerConfig, startup_timeout: std::time::Duration) -> McpServer {
    let name = cfg.name.clone();
    let command = cfg.command.clone();
    let args = cfg.args.clone();
    match StdioClient::spawn(&cfg).await {
        Ok(client) => match client.initialize(startup_timeout).await {
            Ok(()) => match client.list_tools().await {
                Ok(raws) => {
                    let tools = raws
                        .into_iter()
                        .map(
                            |RawTool {
                                 name: tname,
                                 description,
                                 input_schema,
                             }| McpTool {
                                name: tname,
                                description,
                                input_schema,
                            },
                        )
                        .collect();
                    McpServer {
                        name,
                        command,
                        args,
                        state: ServerState::Ready,
                        tools,
                        client: Some(Arc::new(client)),
                    }
                }
                Err(reason) => McpServer {
                    name,
                    command,
                    args,
                    state: ServerState::Failed(format!("tools/list: {reason}")),
                    tools: vec![],
                    client: None,
                },
            },
            Err(reason) => McpServer {
                name,
                command,
                args,
                state: ServerState::Failed(format!("initialize: {reason}")),
                tools: vec![],
                client: None,
            },
        },
        Err(reason) => McpServer {
            name,
            command,
            args,
            state: ServerState::Failed(format!("spawn: {reason}")),
            tools: vec![],
            client: None,
        },
    }
}

/// Snapshot of all configured servers. Used by `/mcp` and the system-prompt
/// catalog injection.
pub fn list() -> Vec<ServerSummary> {
    servers()
        .iter()
        .map(|s| ServerSummary {
            name: s.name.clone(),
            command: s.command.clone(),
            args: s.args.clone(),
            state: s.state.clone(),
            tools: s.tools.clone(),
        })
        .collect()
}

/// Lightweight projection over `McpServer` for read-only consumers.
#[derive(Clone)]
pub struct ServerSummary {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub state: ServerState,
    pub tools: Vec<McpTool>,
}

/// Total tool count across every Ready server.
pub fn total_tools() -> usize {
    servers().iter().map(|s| s.tools.len()).sum()
}

/// Number of servers in `Failed` state — used by the welcome banner.
pub fn failed_count() -> usize {
    servers()
        .iter()
        .filter(|s| matches!(s.state, ServerState::Failed(_)))
        .count()
}

/// Split a qualified `mcp__<server>__<tool>` name into its parts. Returns
/// `None` if `qualified` doesn't follow the convention.
fn split_qualified(qualified: &str) -> Option<(&str, &str)> {
    let rest = qualified.strip_prefix("mcp__")?;
    let sep = rest.find("__")?;
    let server = &rest[..sep];
    let tool = &rest[sep + 2..];
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// Format a `(server, tool)` pair as the qualified name the agent loop sees.
pub fn qualify(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

/// Lookup that returns the live client + bare tool name for an
/// `mcp__server__tool` qualified name.
fn locate(qualified: &str) -> Option<(Arc<StdioClient>, String)> {
    let (server, tool) = split_qualified(qualified)?;
    let s = servers().iter().find(|s| s.name == server)?;
    if !matches!(s.state, ServerState::Ready) {
        return None;
    }
    let client = s.client.as_ref()?.clone();
    Some((client, tool.to_string()))
}

/// Dispatch a tool call. `body` is the raw text from the `<tool>` tag — for
/// MCP that must be JSON. The result is the concatenation of the server's
/// text content blocks, prefixed with `[mcp error]` when `is_error` is set
/// or any decoding step fails.
pub async fn call_tool(qualified: &str, body: &str) -> String {
    let Some((client, tool)) = locate(qualified) else {
        return format!("[mcp error] tool '{qualified}' not available");
    };
    let body_trimmed = body.trim();
    let arguments: Value = if body_trimmed.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_str(body_trimmed) {
            Ok(v) => v,
            Err(e) => {
                return format!("[mcp error] tool body must be a JSON object: {e}");
            }
        }
    };
    if !arguments.is_object() {
        return "[mcp error] tool body must be a JSON object".to_string();
    }
    match client.call_tool(&tool, arguments).await {
        Ok(result) => {
            let mut out = String::new();
            for block in &result.content {
                if let protocol::ContentBlock::Text { text } = block {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            if out.is_empty() {
                out = "(no output)".to_string();
            }
            if result.is_error {
                format!("[mcp error] {out}")
            } else {
                out
            }
        }
        Err(reason) => format!("[mcp error] {reason}"),
    }
}

/// Best-effort shutdown of every Ready server. Called on exit so child
/// processes do not outlive aictl when the user presses Ctrl+C / `/exit`.
pub async fn shutdown() {
    for s in servers() {
        if let Some(client) = &s.client {
            client.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_qualified_basic() {
        assert_eq!(
            split_qualified("mcp__github__create_issue"),
            Some(("github", "create_issue"))
        );
    }

    #[test]
    fn split_qualified_tool_with_double_underscore() {
        // Tools may contain `__`; `find` returns at the *first* `__`, so the
        // server name must not contain it. Servers using `__` are out of
        // scope for v1; document by example here.
        assert_eq!(split_qualified("mcp__srv__a__b"), Some(("srv", "a__b")));
    }

    #[test]
    fn split_qualified_rejects_short() {
        assert!(split_qualified("mcp__only").is_none());
        assert!(split_qualified("mcp__").is_none());
        assert!(split_qualified("not_mcp__a__b").is_none());
    }

    #[test]
    fn qualify_round_trips() {
        let q = qualify("github", "create_issue");
        assert_eq!(q, "mcp__github__create_issue");
        assert_eq!(split_qualified(&q), Some(("github", "create_issue")));
    }
}
