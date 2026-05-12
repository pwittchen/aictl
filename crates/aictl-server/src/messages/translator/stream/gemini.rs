//! Translate the Gemini `streamGenerateContent?alt=sse` stream into
//! the Anthropic Messages event sequence.
//!
//! Gemini SSE chunks look like:
//! ```text
//! data: {"candidates":[{"content":{"parts":[{"text":"Hel"}]}}], ...}
//! data: {"candidates":[{"content":{"parts":[{"text":"lo"}]}}], ...}
//! data: {"candidates":[{"finishReason":"STOP"}], "usageMetadata":{...}}
//! ```
//!
//! Parts can be `{"text": "..."}` or `{"functionCall": {"name", "args"}}`.
//! Function calls in Gemini arrive whole (not as JSON-arg deltas like
//! OpenAI), so we open + emit-arg + close in one shot for each.

use axum::body::Body;
use bytes::Bytes;
use futures::Stream;
use serde::Deserialize;
use serde_json::Value;

use crate::messages::translator::stream;

pub fn translate<S>(stream_in: S, request_id: &str, model: &str) -> Body
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let request_id = request_id.to_string();
    let model = model.to_string();
    let s = async_stream::stream! {
        let mut sse = stream::sse_reader(stream_in);
        let mut state = GeminiStreamState::new();
        yield Ok::<Bytes, std::io::Error>(stream::emit("message_start", &stream::message_start(&request_id, &model)));

        while let Some(line) = sse.next().await {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    yield Ok(stream::emit("error", &stream::error_event(&format!("upstream read error: {e}"))));
                    return;
                }
            };
            if line.is_empty() || line == "[DONE]" {
                if line == "[DONE]" {
                    break;
                }
                continue;
            }
            let chunk: GeminiStreamChunk = match serde_json::from_str(&line) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for event in state.consume(chunk, &request_id) {
                yield Ok(event);
            }
        }
        for event in state.finalize() {
            yield Ok(event);
        }
    };
    Body::from_stream(s)
}

struct GeminiStreamState {
    next_index: u32,
    text_block: Option<u32>,
    tool_indices: Vec<u32>,
    stop_reason: &'static str,
    output_tokens: u64,
}

impl GeminiStreamState {
    fn new() -> Self {
        Self {
            next_index: 0,
            text_block: None,
            tool_indices: Vec::new(),
            stop_reason: "end_turn",
            output_tokens: 0,
        }
    }

    fn consume(&mut self, chunk: GeminiStreamChunk, request_id: &str) -> Vec<Bytes> {
        let mut out = Vec::new();
        if let Some(u) = chunk.usage_metadata
            && u.candidates_token_count > 0
        {
            self.output_tokens = u.candidates_token_count;
        }
        let Some(cand) = chunk.candidates.into_iter().next() else {
            return out;
        };
        for part in cand.content.parts {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                let idx = if let Some(i) = self.text_block {
                    i
                } else {
                    let i = self.next_index;
                    self.next_index += 1;
                    self.text_block = Some(i);
                    out.push(stream::emit(
                        "content_block_start",
                        &stream::content_block_start_text(i),
                    ));
                    i
                };
                out.push(stream::emit(
                    "content_block_delta",
                    &stream::content_block_delta_text(idx, text),
                ));
            }
            if let Some(call) = part.get("functionCall") {
                if let Some(text_idx) = self.text_block.take() {
                    out.push(stream::emit(
                        "content_block_stop",
                        &stream::content_block_stop(text_idx),
                    ));
                }
                let name = call.get("name").and_then(Value::as_str).unwrap_or("");
                let args_str = call.get("args").map_or_else(
                    || "{}".into(),
                    |v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into()),
                );
                let idx = self.next_index;
                self.next_index += 1;
                let id = format!("call_{request_id}_{idx}");
                out.push(stream::emit(
                    "content_block_start",
                    &stream::content_block_start_tool_use(idx, &id, name),
                ));
                if !args_str.is_empty() {
                    out.push(stream::emit(
                        "content_block_delta",
                        &stream::content_block_delta_input_json(idx, &args_str),
                    ));
                }
                self.tool_indices.push(idx);
                self.stop_reason = "tool_use";
            }
        }
        if let Some(finish) = cand.finish_reason {
            self.stop_reason = match finish.as_str() {
                "MAX_TOKENS" => "max_tokens",
                "STOP" => {
                    if self.stop_reason == "tool_use" {
                        "tool_use"
                    } else {
                        "end_turn"
                    }
                }
                _ => self.stop_reason,
            };
        }
        out
    }

    fn finalize(&mut self) -> Vec<Bytes> {
        let mut out = Vec::new();
        if let Some(idx) = self.text_block.take() {
            out.push(stream::emit(
                "content_block_stop",
                &stream::content_block_stop(idx),
            ));
        }
        for idx in std::mem::take(&mut self.tool_indices) {
            out.push(stream::emit(
                "content_block_stop",
                &stream::content_block_stop(idx),
            ));
        }
        out.push(stream::emit(
            "message_delta",
            &stream::message_delta(self.stop_reason, self.output_tokens),
        ));
        out.push(stream::emit("message_stop", &stream::message_stop()));
        out
    }
}

#[derive(Debug, Deserialize)]
struct GeminiStreamChunk {
    #[serde(default)]
    candidates: Vec<GeminiStreamCandidate>,
    #[serde(default)]
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiStreamUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiStreamCandidate {
    #[serde(default)]
    content: GeminiStreamContent,
    #[serde(default)]
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GeminiStreamContent {
    #[serde(default)]
    parts: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct GeminiStreamUsage {
    #[serde(default)]
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: u64,
}
