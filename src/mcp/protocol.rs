//! MCP (Model Context Protocol) wire types.
//!
//! Only the subset Phase 1 needs: `initialize`, `tools/list`, `tools/call`,
//! and `shutdown`. Resources and prompts are deferred to Phase 3.
//!
//! These mirror the JSON-RPC payloads the spec defines but stay deliberately
//! lenient — extra unknown fields from the server are ignored, and we use
//! `serde_json::Value` for tool input schemas because they are user-supplied
//! JSON Schema documents that vary per server.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 envelope used both ways on the wire.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcMessage {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Result of `tools/list`. `tools` is the only field we consume.
#[derive(Debug, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<RawTool>,
}

/// One entry as the server reports it.
#[derive(Debug, Deserialize, Clone)]
pub struct RawTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

/// Result of `tools/call`. The MCP spec splits the response into a typed
/// content array (text, image, resource…); for v1 we concatenate text blocks
/// and ignore the rest.
#[derive(Debug, Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    /// Catch-all for image/resource/etc. variants. Phase 1 only renders
    /// text blocks; non-text content is ignored. Documented as a future
    /// extension point in `.claude/plans/mcp-support.md`.
    #[serde(other)]
    Other,
}
