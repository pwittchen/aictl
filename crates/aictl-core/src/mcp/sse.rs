//! Legacy HTTP+SSE transport for MCP.
//!
//! This is the older two-channel transport that predates streamable-HTTP:
//!
//!   * Client opens a long-lived `GET` to the SSE URL with
//!     `Accept: text/event-stream` and reads server frames from the body.
//!   * The server's first frame is an `event: endpoint` line whose
//!     `data:` payload is the POST URL the client should write to. (Some
//!     servers send a relative path; we resolve it against the SSE URL.)
//!   * Every subsequent `event: message` frame carries a JSON-RPC
//!     response; the client matches it back to a pending request via the
//!     `id` field.
//!   * Client requests go out as HTTP `POST` to the endpoint URL.
//!
//! Implementation mirrors `StdioClient`: a background task reads the SSE
//! stream and routes responses to per-id oneshot channels; a `Mutex`
//! around the endpoint URL synchronizes the "wait for endpoint" handshake.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use super::config::ServerConfig;
use super::protocol::{CallToolResult, JsonRpcMessage, RawTool, ToolsListResult};
use super::transport::Transport;

/// HTTP+SSE MCP client. Owns the long-lived SSE stream task and the
/// reqwest client used for outbound POSTs. Cheap to clone fields stay
/// behind `Arc<Mutex<…>>` so the reader and writer halves don't fight
/// over ownership.
pub struct SseClient {
    next_id: AtomicI64,
    client: reqwest::Client,
    sse_url: String,
    headers: HeaderMap,
    endpoint: Arc<Mutex<Option<String>>>,
    endpoint_ready: Arc<Notify>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcMessage>>>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    rpc_timeout: Duration,
}

impl SseClient {
    /// Build the client and start the background SSE reader. Returns
    /// `Err` if the initial `GET` fails or a header is malformed.
    pub async fn spawn(cfg: &ServerConfig) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        for (k, v) in &cfg.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| format!("invalid header name '{k}': {e}"))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| format!("invalid header value for '{k}': {e}"))?;
            headers.insert(name, value);
        }
        // SSE servers expect this Accept header.
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );

        // No global timeout on the client — the SSE GET is intentionally
        // long-lived. Per-call POSTs wrap themselves in `tokio::time::timeout`.
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("build http client: {e}"))?;

        let sse_url = cfg.url.clone();
        let resp = client
            .get(&sse_url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| format!("sse connect: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("sse connect: http {}", resp.status()));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !content_type.contains("text/event-stream") {
            return Err(format!(
                "sse connect: expected text/event-stream, got '{content_type}'"
            ));
        }

        let endpoint: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let endpoint_ready: Arc<Notify> = Arc::new(Notify::new());
        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let endpoint_for_reader = endpoint.clone();
        let endpoint_ready_for_reader = endpoint_ready.clone();
        let pending_for_reader = pending.clone();
        let base_url = sse_url.clone();
        let reader_task = tokio::spawn(async move {
            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else {
                    break;
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n")) {
                    let raw_frame = buffer[..end].to_string();
                    let advance = if buffer[end..].starts_with("\r\n\r\n") {
                        4
                    } else {
                        2
                    };
                    buffer.drain(..end + advance);
                    handle_sse_frame(
                        &raw_frame,
                        &base_url,
                        &endpoint_for_reader,
                        &endpoint_ready_for_reader,
                        &pending_for_reader,
                    )
                    .await;
                }
            }
            // Stream closed — wake any waiters so they can fail rather
            // than block forever.
            endpoint_ready_for_reader.notify_waiters();
        });

        Ok(Self {
            next_id: AtomicI64::new(1),
            client,
            sse_url,
            headers,
            endpoint,
            endpoint_ready,
            pending,
            reader_task: Mutex::new(Some(reader_task)),
            rpc_timeout: cfg.timeout,
        })
    }

    /// Wait until the server announces a POST endpoint URL on the SSE
    /// channel, then return it. Times out under `startup_timeout` so a
    /// silent server doesn't hang init.
    async fn wait_for_endpoint(&self, startup_timeout: Duration) -> Result<String, String> {
        if let Some(url) = self.endpoint.lock().await.clone() {
            return Ok(url);
        }
        let notified = self.endpoint_ready.notified();
        match tokio::time::timeout(startup_timeout, notified).await {
            Ok(()) => {}
            Err(_) => {
                return Err(format!(
                    "sse endpoint not announced within {}s",
                    startup_timeout.as_secs()
                ));
            }
        }
        self.endpoint
            .lock()
            .await
            .clone()
            .ok_or_else(|| "sse endpoint announce failed".to_string())
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value, String> {
        crate::security::validate_mcp_url(&self.sse_url)?;

        let endpoint = self
            .endpoint
            .lock()
            .await
            .clone()
            .ok_or_else(|| "sse endpoint not yet announced".to_string())?;

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
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let post = self
            .client
            .post(&endpoint)
            .headers(self.headers.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send();

        let resp = match tokio::time::timeout(self.rpc_timeout, post).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                self.pending.lock().await.remove(&id);
                return Err(format!("sse post: {e}"));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "rpc '{method}' post timed out after {}s",
                    self.rpc_timeout.as_secs()
                ));
            }
        };
        if !resp.status().is_success() {
            self.pending.lock().await.remove(&id);
            return Err(format!("sse post: http {}", resp.status()));
        }
        // Per the SSE transport spec, the POST itself just acks (often
        // 202 Accepted). The actual response arrives over the SSE
        // channel and is delivered through the oneshot.
        let response = match tokio::time::timeout(self.rpc_timeout, rx).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err("sse stream closed before response".to_string());
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "rpc '{method}' response timed out after {}s",
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
        // Wait for the server's endpoint announcement before issuing any
        // RPC — otherwise we have nowhere to POST to.
        let _endpoint = self.wait_for_endpoint(startup_timeout).await?;
        let init = async {
            self.send_request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "aictl", "version": crate::VERSION }
                }),
            )
            .await
        };
        match tokio::time::timeout(startup_timeout, init).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(format!(
                    "initialize timed out after {}s",
                    startup_timeout.as_secs()
                ));
            }
        }
        // Best-effort `notifications/initialized`.
        if let Some(endpoint) = self.endpoint.lock().await.clone() {
            let notif = json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            });
            let body = serde_json::to_string(&notif).unwrap_or_default();
            let _ = self
                .client
                .post(&endpoint)
                .headers(self.headers.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await;
        }
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<RawTool>, String> {
        let result = self.send_request("tools/list", json!({})).await?;
        let parsed: ToolsListResult =
            serde_json::from_value(result).map_err(|e| format!("decode tools/list: {e}"))?;
        Ok(parsed.tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult, String> {
        let result = self
            .send_request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        serde_json::from_value::<CallToolResult>(result)
            .map_err(|e| format!("decode tools/call: {e}"))
    }

    pub async fn shutdown(&self) {
        if let Some(handle) = self.reader_task.lock().await.take() {
            handle.abort();
        }
    }
}

impl Drop for SseClient {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.reader_task.try_lock()
            && let Some(h) = handle.take()
        {
            h.abort();
        }
    }
}

/// Parse one fully-buffered SSE frame, then dispatch:
///
///   * `event: endpoint` — record the announced POST URL and wake any
///     `wait_for_endpoint` futures.
///   * everything else — try to decode the `data:` payload as a JSON-RPC
///     envelope and route it to the matching `pending` oneshot.
async fn handle_sse_frame(
    raw: &str,
    base_url: &str,
    endpoint: &Mutex<Option<String>>,
    endpoint_ready: &Notify,
    pending: &Mutex<HashMap<i64, oneshot::Sender<JsonRpcMessage>>>,
) {
    let mut event_type = String::from("message");
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                event_type = trimmed.to_string();
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() {
        return;
    }
    if event_type.as_str() == "endpoint" {
        let resolved = resolve_endpoint(base_url, data.trim());
        let mut slot = endpoint.lock().await;
        *slot = Some(resolved);
        drop(slot);
        endpoint_ready.notify_waiters();
        return;
    }
    let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(&data) else {
        return;
    };
    let Some(id) = msg.id.as_ref().and_then(serde_json::Value::as_i64) else {
        return;
    };
    let mut map = pending.lock().await;
    if let Some(tx) = map.remove(&id) {
        let _ = tx.send(msg);
    }
}

/// Resolve the endpoint URL the server announces. Servers are allowed to
/// send a fully qualified URL or a relative path; in the latter case we
/// resolve against the SSE URL's origin so we don't drift to a different
/// host without going through the security gate again.
fn resolve_endpoint(base_url: &str, announced: &str) -> String {
    if announced.starts_with("http://") || announced.starts_with("https://") {
        return announced.to_string();
    }
    let Some((scheme, rest)) = base_url.split_once("://") else {
        return announced.to_string();
    };
    let host = rest.split('/').next().unwrap_or("");
    if announced.starts_with('/') {
        format!("{scheme}://{host}{announced}")
    } else {
        format!("{scheme}://{host}/{announced}")
    }
}

impl Transport for SseClient {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_endpoint_absolute_passthrough() {
        let url = resolve_endpoint(
            "https://x.example.com/sse",
            "https://other.example.com/post",
        );
        assert_eq!(url, "https://other.example.com/post");
    }

    #[test]
    fn resolve_endpoint_root_relative() {
        let url = resolve_endpoint("https://x.example.com/sse", "/messages");
        assert_eq!(url, "https://x.example.com/messages");
    }

    #[test]
    fn resolve_endpoint_bare_path() {
        let url = resolve_endpoint("https://x.example.com:8443/sse", "messages");
        assert_eq!(url, "https://x.example.com:8443/messages");
    }
}
