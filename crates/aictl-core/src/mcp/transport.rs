//! `Transport` trait shared by the stdio, HTTP, and SSE MCP clients.
//!
//! All three transports speak the same JSON-RPC 2.0 dialect with the same
//! handshake (`initialize` → `notifications/initialized`), the same
//! `tools/list` and `tools/call` ops, and the same shutdown semantics —
//! they only differ in how bytes get from client to server. This trait
//! pulls the dispatch surface out of `mcp.rs` so the call-site doesn't
//! care which transport a given server is using.
//!
//! We use `BoxFuture` from `futures-util` (already a workspace dep) rather
//! than `async fn` in trait so the trait stays object-safe — `mcp::McpServer`
//! holds an `Arc<dyn Transport>` and routes calls through it without
//! knowing the concrete type.

use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::Value;

use super::protocol::{CallToolResult, RawTool};

/// One MCP wire client. Implementations:
///
///   * [`super::stdio::StdioClient`] — spawn a child process, JSON-RPC
///     over its stdin/stdout.
///   * [`super::http::HttpClient`] — POST JSON-RPC frames to a URL,
///     accept JSON or SSE response bodies.
///   * [`super::sse::SseClient`] — long-lived SSE GET for server frames,
///     POSTs for client frames (legacy MCP HTTP+SSE transport).
pub trait Transport: Send + Sync {
    /// Complete the JSON-RPC `initialize` handshake. The startup timeout
    /// is passed in because the first message also has to wait for the
    /// underlying transport to become reachable (process spawn, TCP
    /// connect) and so warrants more headroom than a steady-state RPC.
    fn initialize(&self, startup_timeout: Duration) -> BoxFuture<'_, Result<(), String>>;

    /// Fetch the server's tool catalogue.
    fn list_tools(&self) -> BoxFuture<'_, Result<Vec<RawTool>, String>>;

    /// Invoke one tool by bare name.
    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> BoxFuture<'_, Result<CallToolResult, String>>;

    /// Best-effort cleanup. Called from `mcp::shutdown` on every exit
    /// path so child processes / open sockets do not outlive aictl.
    fn shutdown(&self) -> BoxFuture<'_, ()>;
}
