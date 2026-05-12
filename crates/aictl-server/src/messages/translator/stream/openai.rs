//! Translate the OpenAI `/v1/chat/completions` SSE stream into the
//! Anthropic Messages event sequence.
//!
//! OpenAI's stream is a sequence of `data: {…}` JSON chunks ending in
//! `data: [DONE]`. Each chunk carries `choices[0].delta` which may
//! contain a `content` fragment, a `tool_calls[]` partial, or a
//! `finish_reason`. Anthropic's stream is structured: a `message_start`
//! event opens the response, each text or tool_use content block is
//! framed by `content_block_start` / `content_block_delta` /
//! `content_block_stop`, then `message_delta` carries the stop reason
//! and `message_stop` closes the response.
//!
//! The state machine in [`translate`] tracks per-content-block index
//! and per-tool-call buffering so multiple interleaved tool calls
//! survive the round-trip.

use std::collections::HashMap;

use axum::body::Body;
use bytes::Bytes;
use futures::Stream;
use serde::Deserialize;

use crate::messages::translator::stream;

/// Wrap a `Stream<Item = Result<Bytes, _>>` of OpenAI SSE bytes into an
/// axum body that emits Anthropic-shaped SSE events.
pub fn translate<S>(stream_in: S, request_id: &str, model: &str) -> Body
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let request_id = request_id.to_string();
    let model = model.to_string();
    let s = async_stream::stream! {
        let mut sse = stream::sse_reader(stream_in);
        let mut state = StreamState::new();
        yield Ok::<Bytes, std::io::Error>(stream::emit("message_start", &stream::message_start(&request_id, &model)));

        while let Some(line) = sse.next().await {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    yield Ok(stream::emit("error", &stream::error_event(&format!("upstream read error: {e}"))));
                    return;
                }
            };
            if line.is_empty() {
                continue;
            }
            if line == "[DONE]" {
                break;
            }
            let chunk: OpenAiStreamChunk = match serde_json::from_str(&line) {
                Ok(c) => c,
                Err(_) => continue, // ignore keep-alive / unparseable lines
            };
            for event in state.consume(chunk) {
                yield Ok(event);
            }
        }
        for event in state.finalize() {
            yield Ok(event);
        }
    };
    Body::from_stream(s)
}

/// State carried across SSE chunks while translating OpenAI deltas to
/// Anthropic events.
struct StreamState {
    /// Index of the next content block to open. Anthropic indexes
    /// content blocks 0..N in emission order.
    next_index: u32,
    /// Whether a text content block is currently open and which index
    /// it has.
    text_block: Option<u32>,
    /// Maps an OpenAI `tool_calls[i].index` to the Anthropic content
    /// block index assigned to it.
    tool_index_map: HashMap<u32, u32>,
    /// Stop reason captured from the finish_reason field on the final
    /// chunk.
    stop_reason: &'static str,
    /// Output tokens from the trailing `usage` chunk.
    output_tokens: u64,
}

impl StreamState {
    fn new() -> Self {
        Self {
            next_index: 0,
            text_block: None,
            tool_index_map: HashMap::new(),
            stop_reason: "end_turn",
            output_tokens: 0,
        }
    }

    fn consume(&mut self, chunk: OpenAiStreamChunk) -> Vec<Bytes> {
        let mut out = Vec::new();
        if let Some(usage) = chunk.usage
            && usage.completion_tokens > 0
        {
            self.output_tokens = usage.completion_tokens;
        }
        let Some(choice) = chunk.choices.into_iter().next() else {
            return out;
        };
        if let Some(finish) = &choice.finish_reason {
            self.stop_reason =
                super::super::openai_family::stop_reason_from_openai(Some(finish.as_str()));
        }

        let delta = choice.delta;

        // --- Text content -----------------------------------------------
        if let Some(text) = delta.content
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
                &stream::content_block_delta_text(idx, &text),
            ));
        }

        // --- Tool calls -------------------------------------------------
        for tc in delta.tool_calls {
            let oi = tc.index;
            // First-time tool call at this index — close any open text
            // block, then open a tool_use block.
            if !self.tool_index_map.contains_key(&oi) {
                if let Some(text_idx) = self.text_block.take() {
                    out.push(stream::emit(
                        "content_block_stop",
                        &stream::content_block_stop(text_idx),
                    ));
                }
                let assigned = self.next_index;
                self.next_index += 1;
                self.tool_index_map.insert(oi, assigned);
                let id = tc.id.as_deref().unwrap_or("");
                let name = tc
                    .function
                    .as_ref()
                    .and_then(|f| f.name.as_deref())
                    .unwrap_or("");
                out.push(stream::emit(
                    "content_block_start",
                    &stream::content_block_start_tool_use(assigned, id, name),
                ));
            }
            let assigned = *self.tool_index_map.get(&oi).unwrap();
            if let Some(f) = tc.function
                && let Some(args) = f.arguments
                && !args.is_empty()
            {
                out.push(stream::emit(
                    "content_block_delta",
                    &stream::content_block_delta_input_json(assigned, &args),
                ));
            }
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
        // Close every tool_use block we opened, in the order Anthropic
        // assigned them.
        let mut tool_indices: Vec<u32> = self.tool_index_map.values().copied().collect();
        tool_indices.sort_unstable();
        for idx in tool_indices {
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

// --- OpenAI streaming shape -------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiStreamUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    #[serde(default)]
    delta: OpenAiStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiStreamToolCall>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiStreamToolFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamUsage {
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_emits_text_block_then_stop() {
        let mut s = StreamState::new();
        let chunk: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices": [{
                "delta": {"content": "hi"},
            }],
        }))
        .unwrap();
        let events = s.consume(chunk);
        let texts: Vec<String> = events
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert!(texts[0].contains("content_block_start"));
        assert!(texts[1].contains("text_delta"));

        let chunk2: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices": [{"delta": {}, "finish_reason": "stop"}],
        }))
        .unwrap();
        let _ = s.consume(chunk2);
        let tail = s.finalize();
        let tail_str: Vec<String> = tail
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert!(tail_str.iter().any(|s| s.contains("content_block_stop")));
        assert!(tail_str.iter().any(|s| s.contains("message_delta")));
        assert!(tail_str.iter().any(|s| s.contains("message_stop")));
    }

    #[test]
    fn state_emits_tool_use_blocks() {
        let mut s = StreamState::new();
        // First chunk: tool_call id + name appear.
        let c1: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "t1",
                        "function": {"name": "calc", "arguments": "{\"x\":"},
                    }],
                },
            }],
        }))
        .unwrap();
        let e1 = s.consume(c1);
        let s1: Vec<String> = e1
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert!(s1.iter().any(|x| x.contains("tool_use")));
        assert!(s1.iter().any(|x| x.contains("input_json_delta")));

        // Second chunk: more arguments fragment.
        let c2: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": "2}"},
                    }],
                },
            }],
        }))
        .unwrap();
        let e2 = s.consume(c2);
        let s2: Vec<String> = e2
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert!(s2.iter().any(|x| x.contains("input_json_delta")));

        // Final chunk with tool_calls finish reason.
        let c3: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
        }))
        .unwrap();
        let _ = s.consume(c3);
        let tail = s.finalize();
        let tail_str: Vec<String> = tail
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect();
        assert!(tail_str.iter().any(|s| s.contains("tool_use")));
        assert!(
            tail_str
                .iter()
                .any(|s| s.contains("\"stop_reason\":\"tool_use\""))
        );
    }
}
