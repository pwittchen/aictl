//! SSE event emitters and provider-specific bridges to Anthropic's
//! event sequence.
//!
//! Anthropic's streaming response shape is structured:
//!
//! ```text
//! event: message_start
//! data: {"type":"message_start","message":{"id":..,"type":"message","role":"assistant","content":[],"model":..,"usage":{..}}}
//!
//! event: content_block_start
//! data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
//!
//! event: content_block_delta
//! data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}
//! ...
//! event: content_block_stop
//! data: {"type":"content_block_stop","index":0}
//!
//! event: message_delta
//! data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":42}}
//!
//! event: message_stop
//! data: {"type":"message_stop"}
//! ```
//!
//! Each provider's native shape gets a dedicated state machine that
//! emits this sequence. [`emit`] is the byte-formatter every bridge
//! uses, so the wire format is consistent.

use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use serde_json::Value;

pub mod gemini;
pub mod ollama;
pub mod openai;

/// Read a `Stream<Item = Result<Bytes, _>>` and yield the body of each
/// `data: ` line. Strips the `data: ` prefix and skips non-`data:`
/// framing lines (comments, `event:` hints, blanks). Shared by every
/// provider whose stream uses standard SSE framing (OpenAI and
/// Gemini's `alt=sse` mode).
pub struct SseLineReader<S> {
    inner: S,
    buf: String,
}

impl<S> SseLineReader<S> {
    #[must_use]
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: String::new(),
        }
    }
}

impl<S> SseLineReader<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    pub async fn next(&mut self) -> Option<Result<String, reqwest::Error>> {
        loop {
            if let Some(idx) = self.buf.find('\n') {
                let line = self.buf[..idx].trim_end_matches('\r').to_string();
                self.buf.drain(..=idx);
                if let Some(rest) = line.strip_prefix("data:") {
                    return Some(Ok(rest.trim().to_string()));
                }
                continue;
            }
            match self.inner.next().await {
                Some(Ok(b)) => {
                    self.buf.push_str(&String::from_utf8_lossy(&b));
                }
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    if self.buf.trim().is_empty() {
                        return None;
                    }
                    let line = std::mem::take(&mut self.buf);
                    if let Some(rest) = line.strip_prefix("data:") {
                        return Some(Ok(rest.trim().to_string()));
                    }
                    return None;
                }
            }
        }
    }
}

#[must_use]
pub fn sse_reader<S>(inner: S) -> SseLineReader<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    SseLineReader::new(inner)
}

/// Format a single Anthropic SSE event as the bytes that go on the
/// wire (`event: NAME\ndata: JSON\n\n`).
#[must_use]
pub fn emit(event: &str, data: &Value) -> Bytes {
    let payload = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    Bytes::from(format!("event: {event}\ndata: {payload}\n\n"))
}

/// Build the `message_start` payload.
#[must_use]
pub fn message_start(request_id: &str, model: &str) -> Value {
    serde_json::json!({
        "type": "message_start",
        "message": {
            "id": format!("msg_{request_id}"),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {"input_tokens": 0, "output_tokens": 0},
        },
    })
}

#[must_use]
pub fn content_block_start_text(index: u32) -> Value {
    serde_json::json!({
        "type": "content_block_start",
        "index": index,
        "content_block": {"type": "text", "text": ""},
    })
}

#[must_use]
pub fn content_block_start_tool_use(index: u32, id: &str, name: &str) -> Value {
    serde_json::json!({
        "type": "content_block_start",
        "index": index,
        "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}},
    })
}

#[must_use]
pub fn content_block_delta_text(index: u32, text: &str) -> Value {
    serde_json::json!({
        "type": "content_block_delta",
        "index": index,
        "delta": {"type": "text_delta", "text": text},
    })
}

#[must_use]
pub fn content_block_delta_input_json(index: u32, partial: &str) -> Value {
    serde_json::json!({
        "type": "content_block_delta",
        "index": index,
        "delta": {"type": "input_json_delta", "partial_json": partial},
    })
}

#[must_use]
pub fn content_block_stop(index: u32) -> Value {
    serde_json::json!({"type": "content_block_stop", "index": index})
}

#[must_use]
pub fn message_delta(stop_reason: &str, output_tokens: u64) -> Value {
    serde_json::json!({
        "type": "message_delta",
        "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
        "usage": {"output_tokens": output_tokens},
    })
}

#[must_use]
pub fn message_stop() -> Value {
    serde_json::json!({"type": "message_stop"})
}

#[must_use]
pub fn error_event(message: &str) -> Value {
    serde_json::json!({
        "type": "error",
        "error": {"type": "api_error", "message": message},
    })
}
