//! Stdio JSON-RPC transport for MCP.
//!
//! Spawns the configured child process with a scrubbed env (the server's
//! own `env` overlay is applied on top), pipes JSON-RPC over its stdin /
//! stdout, and exposes the four operations the dispatch layer needs:
//! `initialize`, `tools/list`, `tools/call`, and `shutdown`.
//!
//! The wire format is line-delimited JSON-RPC: each request and response
//! is a single JSON object on its own line. That's the dialect every
//! reference MCP server (filesystem, git, github) implements today; the
//! more elaborate `Content-Length:` framing the spec also describes is
//! still rare in deployed servers and not implemented here.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::config::ServerConfig;
use super::protocol::{CallToolResult, JsonRpcMessage, RawTool, ToolsListResult};

/// One stdio-backed MCP client. Owns the child process and the reader/writer
/// halves of its stdio. All RPCs are funnelled through [`Self::request`] which
/// matches responses to in-flight requests via the JSON-RPC `id` field.
pub struct StdioClient {
    next_id: AtomicI64,
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcMessage>>>>,
    child: Mutex<Option<Child>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    rpc_timeout: Duration,
}

impl StdioClient {
    /// Spawn the child and start a background reader task that fans incoming
    /// JSON-RPC responses out to the per-request oneshot channels.
    ///
    /// `async` even though no `await` is needed here — `tokio::process::Command`
    /// requires a tokio runtime context to spawn, and keeping the function
    /// async lets callers chain it with the rest of the init pipeline naturally.
    #[allow(clippy::unused_async)]
    pub async fn spawn(cfg: &ServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args);
        // Start from the same scrubbed env every other tool subprocess uses,
        // then layer the entry's own `env` on top. Server tokens go through
        // here, so the keyring substitution in config::parse already handled
        // their resolution.
        cmd.env_clear();
        for (k, v) in crate::security::scrubbed_env() {
            cmd.env(k, v);
        }
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn '{}': {e}", cfg.command))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "child stdin not available".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "child stdout not available".to_string())?;
        // Drain stderr so the child doesn't block on a full pipe; we do not
        // surface its content unless the user asks via `/mcp show`.
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
            });
        }

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = pending.clone();
        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break, // EOF or transport error
                    Ok(_) => {}
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(trimmed) else {
                    // Servers occasionally print non-JSON banners on startup;
                    // ignore anything that isn't a valid envelope.
                    continue;
                };
                let Some(id) = msg.id.as_ref().and_then(serde_json::Value::as_i64) else {
                    // Notification (no id) — Phase 1 ignores these.
                    continue;
                };
                let mut map = pending_for_reader.lock().await;
                if let Some(tx) = map.remove(&id) {
                    let _ = tx.send(msg);
                }
            }
        });

        Ok(Self {
            next_id: AtomicI64::new(1),
            stdin: Mutex::new(stdin),
            pending,
            child: Mutex::new(Some(child)),
            reader_task: Mutex::new(Some(reader_task)),
            rpc_timeout: cfg.timeout,
        })
    }

    /// Send a JSON-RPC request and await the matching response. Returns
    /// `Err` on timeout, transport error, or when the server replies with an
    /// `error` envelope.
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = JsonRpcMessage {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(id)),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }
        let raw = serde_json::to_string(&payload).map_err(|e| format!("encode: {e}"))?;
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(raw.as_bytes())
                .await
                .map_err(|e| format!("write: {e}"))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| format!("write: {e}"))?;
            stdin.flush().await.map_err(|e| format!("flush: {e}"))?;
        }
        let response = match tokio::time::timeout(self.rpc_timeout, rx).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err("server closed connection".to_string());
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "rpc '{method}' timed out after {}s",
                    self.rpc_timeout.as_secs()
                ));
            }
        };
        if let Some(err) = response.error {
            return Err(format!("server error {}: {}", err.code, err.message));
        }
        response
            .result
            .ok_or_else(|| "missing 'result' in response".to_string())
    }

    pub async fn initialize(&self, startup_timeout: Duration) -> Result<(), String> {
        // Use the startup timeout for the first RPC by temporarily
        // overriding the per-call deadline. Two-step: write the request,
        // wait on the oneshot ourselves with the longer timeout.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = JsonRpcMessage {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(id)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "aictl", "version": crate::VERSION }
            })),
            result: None,
            error: None,
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }
        let raw = serde_json::to_string(&payload).map_err(|e| format!("encode: {e}"))?;
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(raw.as_bytes())
                .await
                .map_err(|e| format!("write: {e}"))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| format!("write: {e}"))?;
            stdin.flush().await.map_err(|e| format!("flush: {e}"))?;
        }
        let response = match tokio::time::timeout(startup_timeout, rx).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err("server closed connection during initialize".to_string());
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "initialize timed out after {}s",
                    startup_timeout.as_secs()
                ));
            }
        };
        if let Some(err) = response.error {
            return Err(format!("initialize failed: {} ({})", err.message, err.code));
        }
        // Send the `notifications/initialized` notification per the spec.
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let raw = serde_json::to_string(&notif).map_err(|e| format!("encode: {e}"))?;
        let mut stdin = self.stdin.lock().await;
        let _ = stdin.write_all(raw.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.flush().await;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<RawTool>, String> {
        let result = self.request("tools/list", json!({})).await?;
        let parsed: ToolsListResult =
            serde_json::from_value(result).map_err(|e| format!("decode tools/list: {e}"))?;
        Ok(parsed.tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult, String> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        serde_json::from_value::<CallToolResult>(result)
            .map_err(|e| format!("decode tools/call: {e}"))
    }

    pub async fn shutdown(&self) {
        // Best-effort: send the shutdown notification then close stdin so
        // the child sees EOF. The reader task ends when stdout closes.
        let _ = self.request("shutdown", json!({})).await.ok();
        // Drop stdin so EOF flows through to the child.
        // (Replacing it would need a re-take; instead we leave it open and
        // rely on kill_on_drop when the StdioClient is dropped.)
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        }
        if let Some(handle) = self.reader_task.lock().await.take() {
            handle.abort();
        }
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        // The Child has `kill_on_drop(true)`, so explicit cleanup is
        // mostly redundant — but we abort the reader task here so it
        // doesn't outlive the client.
        if let Ok(mut handle) = self.reader_task.try_lock()
            && let Some(h) = handle.take()
        {
            h.abort();
        }
    }
}
