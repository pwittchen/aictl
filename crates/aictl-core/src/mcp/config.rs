//! MCP server config parser.
//!
//! Reads `~/.aictl/mcp.json` (override via `AICTL_MCP_CONFIG`) in a shape
//! compatible with Claude Desktop. Stdio entries:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "<name>": {
//!       "command": "...",
//!       "args": [...],
//!       "env": { "K": "V" },
//!       "enabled": true,
//!       "timeout_secs": 30
//!     }
//!   }
//! }
//! ```
//!
//! Remote entries opt in via the `transport` field — `"http"` (the modern
//! Streamable HTTP transport) or `"sse"` (the legacy HTTP+SSE transport)
//! — and supply a `url` plus optional `headers`:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "remote-name": {
//!       "transport": "http",
//!       "url": "https://mcp.example.com/v1",
//!       "headers": { "Authorization": "Bearer ${keyring:MCP_TOKEN}" },
//!       "enabled": true,
//!       "timeout_secs": 30
//!     }
//!   }
//! }
//! ```
//!
//! Values inside `env` and `headers` may use `${keyring:NAME}` to pull a
//! secret from `keys::get_secret(NAME)` rather than checking the literal
//! string in. Remote entries pass through `security::validate_mcp_url` at
//! parse time so the hostname allow/deny gate (`AICTL_MCP_ALLOW_HOSTS` /
//! `AICTL_MCP_DENY_HOSTS`) rejects a server before it is ever contacted.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::config_get;
use crate::keys;
use crate::security;

/// Default per-call RPC timeout when neither the entry nor `AICTL_MCP_TIMEOUT`
/// supplies one.
const DEFAULT_RPC_TIMEOUT_SECS: u64 = 30;
/// Default `initialize` handshake timeout. Distinct from the per-call RPC
/// timeout because the first probe also has to wait for the child process to
/// spawn and start serving.
const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 10;

/// Wire transport for one MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Spawn a child process and pipe JSON-RPC over its stdio.
    Stdio,
    /// "Streamable HTTP" — POST JSON-RPC frames to a URL, read either a
    /// JSON or SSE response body.
    Http,
    /// Legacy HTTP+SSE — long-lived SSE GET to receive server frames,
    /// short-lived POSTs to the endpoint URL the server announces on the
    /// first SSE event.
    Sse,
}

impl Transport {
    /// Stable on-disk label, lowercase. Mirrors the strings the parser
    /// accepts in `mcp.json`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Http => "http",
            Transport::Sse => "sse",
        }
    }

    /// `true` when this transport reaches outside the local machine — used
    /// by callers (config parser, UI summaries) that need to differentiate
    /// process-spawn vs network-call semantics.
    #[must_use]
    pub fn is_remote(self) -> bool {
        matches!(self, Transport::Http | Transport::Sse)
    }
}

/// One server entry, fully resolved (keyring secrets substituted, env merged,
/// remote URLs validated against the security policy).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub transport: Transport,
    /// Stdio-only: executable path / name (PATH-resolved).
    pub command: String,
    /// Stdio-only: argv tail.
    pub args: Vec<String>,
    /// Stdio-only: env overlay (on top of the scrubbed default).
    pub env: HashMap<String, String>,
    /// Remote-only: dispatch URL.
    pub url: String,
    /// Remote-only: extra HTTP headers (Authorization, etc.).
    pub headers: HashMap<String, String>,
    pub enabled: bool,
    pub timeout: Duration,
}

/// Validate a server name with the same rule used for agents/skills/plugins:
/// alphanumeric + underscore + dash.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Resolve the effective config path. Override via `AICTL_MCP_CONFIG`.
pub fn config_path() -> PathBuf {
    if let Some(p) = config_get("AICTL_MCP_CONFIG") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/mcp.json"))
}

/// Default per-call timeout from `AICTL_MCP_TIMEOUT`.
fn default_rpc_timeout() -> Duration {
    let secs = config_get("AICTL_MCP_TIMEOUT")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_RPC_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Initialize-phase handshake timeout from `AICTL_MCP_STARTUP_TIMEOUT`.
pub fn startup_timeout() -> Duration {
    let secs = config_get("AICTL_MCP_STARTUP_TIMEOUT")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Names listed in `AICTL_MCP_DISABLED` are skipped at init time even if their
/// JSON entry has `enabled: true`.
pub fn disabled_set() -> Vec<String> {
    config_get("AICTL_MCP_DISABLED")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the config file at [`config_path`]. Returns an empty list if the
/// file is missing — that's the "no MCP servers configured" steady state and
/// must not be an error.
pub fn load() -> Result<Vec<ServerConfig>, String> {
    let path = config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    parse(&raw)
}

/// Parse the JSON document. Split out so tests can drive it without a file.
#[allow(clippy::too_many_lines)] // two-arm transport switch reads better inline than as helpers
pub fn parse(raw: &str) -> Result<Vec<ServerConfig>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid mcp.json: {e}"))?;
    let map = v
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "missing 'mcpServers' object".to_string())?;

    let default_timeout = default_rpc_timeout();
    let mut out = Vec::new();
    for (name, entry) in map {
        if !is_valid_name(name) {
            return Err(format!(
                "invalid server name '{name}' (use alphanumeric, underscore, or dash)"
            ));
        }
        let obj = entry
            .as_object()
            .ok_or_else(|| format!("server '{name}': entry must be an object"))?;
        let transport = parse_transport(name, obj)?;
        let enabled = obj
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let timeout = obj
            .get("timeout_secs")
            .and_then(serde_json::Value::as_u64)
            .filter(|v| *v > 0)
            .map_or(default_timeout, Duration::from_secs);

        match transport {
            Transport::Stdio => {
                let command = obj
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("server '{name}': missing 'command'"))?
                    .to_string();
                if command.is_empty() {
                    return Err(format!("server '{name}': empty 'command'"));
                }
                let args = obj
                    .get("args")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let env = obj
                    .get("env")
                    .and_then(serde_json::Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| {
                                v.as_str().map(|raw| (k.clone(), substitute_keyring(raw)))
                            })
                            .collect::<HashMap<String, String>>()
                    })
                    .unwrap_or_default();

                out.push(ServerConfig {
                    name: name.clone(),
                    transport,
                    command,
                    args,
                    env,
                    url: String::new(),
                    headers: HashMap::new(),
                    enabled,
                    timeout,
                });
            }
            Transport::Http | Transport::Sse => {
                let url_raw = obj
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        format!(
                            "server '{name}': missing 'url' for transport '{}'",
                            transport.as_str()
                        )
                    })?;
                let url = substitute_keyring(url_raw);
                if url.is_empty() {
                    return Err(format!("server '{name}': empty 'url'"));
                }
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(format!(
                        "server '{name}': url must start with http:// or https://"
                    ));
                }
                // The hostname allow/deny gate runs once at parse time. The
                // URL is immutable for the process lifetime, so a single
                // check is enough — clients re-validate defensively when
                // they dispatch a request.
                security::validate_mcp_url(&url).map_err(|e| format!("server '{name}': {e}"))?;
                let headers = obj
                    .get("headers")
                    .and_then(serde_json::Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| {
                                v.as_str().map(|raw| (k.clone(), substitute_keyring(raw)))
                            })
                            .collect::<HashMap<String, String>>()
                    })
                    .unwrap_or_default();

                out.push(ServerConfig {
                    name: name.clone(),
                    transport,
                    command: String::new(),
                    args: Vec::new(),
                    env: HashMap::new(),
                    url,
                    headers,
                    enabled,
                    timeout,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn parse_transport(
    name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Transport, String> {
    let Some(raw) = obj.get("transport") else {
        return Ok(Transport::Stdio);
    };
    let raw = raw
        .as_str()
        .ok_or_else(|| format!("server '{name}': 'transport' must be a string"))?;
    match raw {
        "stdio" => Ok(Transport::Stdio),
        "http" | "streamable-http" | "streamable_http" => Ok(Transport::Http),
        "sse" => Ok(Transport::Sse),
        other => Err(format!(
            "server '{name}': unknown transport '{other}' (expected 'stdio', 'http', or 'sse')"
        )),
    }
}

/// Replace `${keyring:NAME}` tokens in a value with the corresponding secret
/// from the system keyring (falling back to the plain config). When the
/// secret is missing we leave the literal token in place — that produces a
/// loud failure at handshake time rather than a silent empty-string.
fn substitute_keyring(raw: &str) -> String {
    let needle = "${keyring:";
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find(needle) {
        out.push_str(&rest[..start]);
        let after = &rest[start + needle.len()..];
        let Some(end) = after.find('}') else {
            // No closing brace — treat as literal text.
            out.push_str(&rest[start..]);
            return out;
        };
        let key = &after[..end];
        if let Some(value) = keys::get_secret(key) {
            out.push_str(&value);
        } else {
            out.push_str(&rest[start..=(start + needle.len() + end)]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_entry() {
        let raw = r#"{"mcpServers": {"fs": {"command": "echo", "args": ["hi"]}}}"#;
        let cfg = parse(raw).unwrap();
        assert_eq!(cfg.len(), 1);
        assert_eq!(cfg[0].name, "fs");
        assert_eq!(cfg[0].transport, Transport::Stdio);
        assert_eq!(cfg[0].command, "echo");
        assert_eq!(cfg[0].args, vec!["hi"]);
        assert!(cfg[0].enabled);
    }

    #[test]
    fn parse_http_entry() {
        let raw = r#"{"mcpServers": {"remote": {"transport": "http", "url": "https://mcp.example.com/v1", "headers": {"Authorization": "Bearer xyz"}}}}"#;
        let cfg = parse(raw).unwrap();
        assert_eq!(cfg.len(), 1);
        assert_eq!(cfg[0].transport, Transport::Http);
        assert_eq!(cfg[0].url, "https://mcp.example.com/v1");
        assert_eq!(
            cfg[0].headers.get("Authorization").map(String::as_str),
            Some("Bearer xyz")
        );
        assert!(cfg[0].command.is_empty());
    }

    #[test]
    fn parse_http_rejects_missing_url() {
        let raw = r#"{"mcpServers": {"remote": {"transport": "http"}}}"#;
        let err = parse(raw).unwrap_err();
        assert!(err.contains("missing 'url'"));
    }

    #[test]
    fn parse_http_rejects_non_http_scheme() {
        let raw = r#"{"mcpServers": {"remote": {"transport": "http", "url": "ftp://x"}}}"#;
        let err = parse(raw).unwrap_err();
        assert!(err.contains("http://"));
    }

    #[test]
    fn parse_rejects_unknown_transport() {
        let raw = r#"{"mcpServers": {"remote": {"transport": "websocket", "url": "https://x"}}}"#;
        let err = parse(raw).unwrap_err();
        assert!(err.contains("unknown transport"));
    }

    #[test]
    fn parse_sse_entry() {
        let raw =
            r#"{"mcpServers": {"r": {"transport": "sse", "url": "https://mcp.example.com/sse"}}}"#;
        let cfg = parse(raw).unwrap();
        assert_eq!(cfg[0].transport, Transport::Sse);
        assert!(cfg[0].transport.is_remote());
    }

    #[test]
    fn parse_rejects_missing_command() {
        let raw = r#"{"mcpServers": {"fs": {"args": []}}}"#;
        let err = parse(raw).unwrap_err();
        assert!(err.contains("missing 'command'"));
    }

    #[test]
    fn parse_rejects_invalid_name() {
        let raw = r#"{"mcpServers": {"bad name!": {"command": "x"}}}"#;
        let err = parse(raw).unwrap_err();
        assert!(err.contains("invalid server name"));
    }

    #[test]
    fn parse_respects_enabled_false() {
        let raw = r#"{"mcpServers": {"fs": {"command": "x", "enabled": false}}}"#;
        let cfg = parse(raw).unwrap();
        assert!(!cfg[0].enabled);
    }

    #[test]
    fn parse_uses_per_entry_timeout() {
        let raw = r#"{"mcpServers": {"fs": {"command": "x", "timeout_secs": 5}}}"#;
        let cfg = parse(raw).unwrap();
        assert_eq!(cfg[0].timeout, Duration::from_secs(5));
    }

    #[test]
    fn parse_returns_empty_when_missing_object() {
        let raw = r#"{}"#;
        assert!(parse(raw).is_err());
    }

    #[test]
    fn parse_sorts_servers_alphabetically() {
        let raw = r#"{"mcpServers": {"zeta": {"command": "x"}, "alpha": {"command": "y"}}}"#;
        let cfg = parse(raw).unwrap();
        assert_eq!(cfg[0].name, "alpha");
        assert_eq!(cfg[1].name, "zeta");
    }

    #[test]
    fn keyring_substitution_leaves_literal_when_secret_missing() {
        // Test environment has no secret named __mcp_test_missing__.
        let s = substitute_keyring("Bearer ${keyring:__mcp_test_missing__}");
        assert!(s.contains("${keyring:__mcp_test_missing__}"));
    }

    #[test]
    fn keyring_substitution_no_token_passthrough() {
        assert_eq!(substitute_keyring("plain value"), "plain value");
    }

    #[test]
    fn keyring_substitution_handles_unterminated_brace() {
        let raw = "Bearer ${keyring:UNCLOSED";
        assert_eq!(substitute_keyring(raw), raw);
    }

    #[test]
    fn name_validation() {
        assert!(is_valid_name("good_name-1"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("bad name"));
        assert!(!is_valid_name("bad/name"));
    }
}
