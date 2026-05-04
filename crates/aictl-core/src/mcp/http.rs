//! Streamable HTTP transport for MCP.
//!
//! Implements the modern MCP "streamable HTTP" transport: every JSON-RPC
//! request is a `POST` to a fixed URL, and the server replies with a
//! response body whose `Content-Type` is either `application/json` (a
//! single JSON-RPC frame) or `text/event-stream` (a sequence of SSE
//! events; we scan for the first `data:` frame whose `id` matches the
//! request and ignore the rest).
//!
//! Compared to stdio there is no long-lived reader task — each request
//! owns its own response stream. This keeps the lifecycle simple and
//! makes per-call timeouts trivial: the whole `send` future is wrapped
//! in `tokio::time::timeout`.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::config::ServerConfig;
use super::protocol::{CallToolResult, JsonRpcMessage, RawTool, ToolsListResult};
use super::transport::Transport;

/// Streamable-HTTP MCP client. Cheap to construct (no network call until
/// `initialize`); all state lives in this struct so multiple in-flight
/// RPCs can share the same `reqwest::Client` connection pool.
pub struct HttpClient {
    next_id: AtomicI64,
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    rpc_timeout: Duration,
}

impl HttpClient {
    /// Build an HTTP client for the given server config. Returns `Err`
    /// when a configured header value is not a valid HTTP header byte
    /// string (matches what `reqwest` would reject at send time, surfaced
    /// here so the failure shows up on startup rather than on first call).
    pub fn new(cfg: &ServerConfig) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        for (k, v) in &cfg.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| format!("invalid header name '{k}': {e}"))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| format!("invalid header value for '{k}': {e}"))?;
            headers.insert(name, value);
        }
        // Streamable-HTTP servers may respond with either JSON or SSE; ask
        // for both up front so the server can pick.
        if !headers.contains_key(reqwest::header::ACCEPT) {
            headers.insert(
                reqwest::header::ACCEPT,
                HeaderValue::from_static("application/json, text/event-stream"),
            );
        }
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(Self {
            next_id: AtomicI64::new(1),
            client,
            url: cfg.url.clone(),
            headers,
            rpc_timeout: cfg.timeout,
        })
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        // Defense in depth: re-validate the URL host before every dispatch.
        // The parser already ran this check at config-load time, but the
        // policy could change between then and now (env vars are mutable),
        // and this is the seam where we reach the network.
        crate::security::validate_mcp_url(&self.url)?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = JsonRpcMessage {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(id)),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
        };
        let body = serde_json::to_string(&payload).map_err(|e| format!("encode: {e}"))?;

        let send = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send();

        let resp = match tokio::time::timeout(self.rpc_timeout, send).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(format!("http: {e}")),
            Err(_) => {
                return Err(format!(
                    "rpc '{method}' timed out after {}s",
                    self.rpc_timeout.as_secs()
                ));
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let snippet = resp.text().await.unwrap_or_default();
            return Err(format!("http {status}: {}", snippet.trim_start()));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        let frame = if content_type.contains("text/event-stream") {
            read_first_sse_frame(resp, id, self.rpc_timeout).await?
        } else {
            // Best-effort JSON path. Anything that is not SSE we treat as
            // JSON — some servers omit Content-Type or reply with
            // `application/json-rpc`.
            let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
            serde_json::from_str::<JsonRpcMessage>(text.trim()).map_err(|e| {
                format!(
                    "decode response: {e} (body: {})",
                    &text[..text.len().min(200)]
                )
            })?
        };

        if let Some(err) = frame.error {
            return Err(format!("server error {}: {}", err.code, err.message));
        }
        frame
            .result
            .ok_or_else(|| "missing 'result' in response".to_string())
    }

    pub async fn initialize(&self, startup_timeout: Duration) -> Result<(), String> {
        let payload = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "aictl", "version": crate::VERSION }
        });
        let send = async {
            // Reuse `request` but with the longer startup timeout. We can't
            // overwrite `self.rpc_timeout` (it's atomic-free), so we build
            // a parallel send here.
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let frame = JsonRpcMessage {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(id)),
                method: Some("initialize".to_string()),
                params: Some(payload),
                result: None,
                error: None,
            };
            let body = serde_json::to_string(&frame).map_err(|e| format!("encode: {e}"))?;
            let resp = self
                .client
                .post(&self.url)
                .headers(self.headers.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|e| format!("http: {e}"))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let snippet = resp.text().await.unwrap_or_default();
                return Err(format!("initialize: http {status}: {}", snippet.trim()));
            }
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            let parsed = if content_type.contains("text/event-stream") {
                read_first_sse_frame(resp, id, startup_timeout).await?
            } else {
                let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
                serde_json::from_str::<JsonRpcMessage>(text.trim())
                    .map_err(|e| format!("decode initialize: {e}"))?
            };
            if let Some(err) = parsed.error {
                return Err(format!("initialize failed: {} ({})", err.message, err.code));
            }
            Ok::<(), String>(())
        };

        match tokio::time::timeout(startup_timeout, send).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(format!(
                    "initialize timed out after {}s",
                    startup_timeout.as_secs()
                ));
            }
        }

        // Send the `notifications/initialized` notification per the spec.
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let body = serde_json::to_string(&notif).map_err(|e| format!("encode: {e}"))?;
        // Best-effort — if the server rejects the notification we still
        // proceed. Some servers don't accept notifications via POST.
        let _ = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await;
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

    /// HTTP transport has no persistent connection to tear down. Kept on
    /// the impl so `mcp::shutdown` can call through without a runtime
    /// type check.
    #[allow(clippy::unused_async)] // matches `Transport::shutdown` signature
    pub async fn shutdown(&self) {
        // No-op.
    }
}

/// Read the first SSE `data:` frame whose `id` matches `target_id` and
/// decode it as a JSON-RPC envelope. Other frames (heartbeats, comments,
/// notifications without an id) are ignored.
async fn read_first_sse_frame(
    resp: reqwest::Response,
    target_id: i64,
    timeout: Duration,
) -> Result<JsonRpcMessage, String> {
    let read = async {
        let mut stream = resp.bytes_stream();
        let mut pending = String::new();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("sse stream: {e}"))?;
            pending.push_str(&String::from_utf8_lossy(&chunk));
            // SSE frames are separated by a blank line. Process as many
            // complete frames as the buffer holds, leave the tail for
            // the next chunk.
            while let Some(end) = pending.find("\n\n").or_else(|| pending.find("\r\n\r\n")) {
                let frame = &pending[..end];
                buf.clear();
                for line in frame.lines() {
                    if let Some(rest) = line.strip_prefix("data:") {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(rest.trim_start());
                    }
                }
                let advance = if pending[end..].starts_with("\r\n\r\n") {
                    4
                } else {
                    2
                };
                let drop_to = end + advance;
                pending.drain(..drop_to);
                if buf.is_empty() {
                    continue;
                }
                if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(&buf)
                    && msg.id.as_ref().and_then(serde_json::Value::as_i64) == Some(target_id)
                {
                    return Ok(msg);
                }
            }
        }
        Err::<JsonRpcMessage, String>("sse stream ended before matching response".to_string())
    };
    match tokio::time::timeout(timeout, read).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "sse response timed out after {}s",
            timeout.as_secs()
        )),
    }
}

impl Transport for HttpClient {
    fn initialize(&self, startup_timeout: Duration) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { Self::initialize(self, startup_timeout).await })
    }

    fn list_tools(&self) -> BoxFuture<'_, Result<Vec<RawTool>, String>> {
        Box::pin(async move { Self::list_tools(self).await })
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> BoxFuture<'_, Result<CallToolResult, String>> {
        let name = name.to_string();
        Box::pin(async move { Self::call_tool(self, &name, arguments).await })
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { Self::shutdown(self).await })
    }
}
