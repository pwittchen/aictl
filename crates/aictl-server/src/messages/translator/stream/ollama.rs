//! Translate Ollama's `/api/chat` NDJSON stream into the Anthropic
//! Messages event sequence.
//!
//! Ollama emits one JSON object per line, terminated by an object with
//! `done: true`. Each chunk's `message.content` is a delta string (or
//! empty); `message.tool_calls[]` appear only on the final chunk for
//! tool-capable models. `done_reason` carries the stop reason
//! (`"length"` → `max_tokens`, anything else → `end_turn`).

use axum::body::Body;
use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;

use crate::messages::translator::ollama::{OllamaResponse, OllamaToolCall};
use crate::messages::translator::stream;

pub fn translate<S>(stream_in: S, request_id: &str, model: &str) -> Body
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let request_id = request_id.to_string();
    let model = model.to_string();
    let s = async_stream::stream! {
        let mut reader = NdjsonReader::new(stream_in);
        let mut state = OllamaStreamState::new();
        yield Ok::<Bytes, std::io::Error>(stream::emit("message_start", &stream::message_start(&request_id, &model)));

        while let Some(line) = reader.next().await {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    yield Ok(stream::emit("error", &stream::error_event(&format!("upstream read error: {e}"))));
                    return;
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let chunk: OllamaResponse = match serde_json::from_str(trimmed) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for event in state.consume(&chunk, &request_id) {
                yield Ok(event);
            }
            if chunk.done {
                break;
            }
        }
        for event in state.finalize() {
            yield Ok(event);
        }
    };
    Body::from_stream(s)
}

struct OllamaStreamState {
    next_index: u32,
    text_block: Option<u32>,
    tool_indices: Vec<u32>,
    stop_reason: &'static str,
    output_tokens: u64,
}

impl OllamaStreamState {
    fn new() -> Self {
        Self {
            next_index: 0,
            text_block: None,
            tool_indices: Vec::new(),
            stop_reason: "end_turn",
            output_tokens: 0,
        }
    }

    fn consume(&mut self, chunk: &OllamaResponse, request_id: &str) -> Vec<Bytes> {
        let mut out = Vec::new();
        if chunk.eval_count > 0 {
            self.output_tokens = chunk.eval_count;
        }
        let Some(msg) = chunk.message.as_ref() else {
            return out;
        };
        if !msg.content.is_empty() {
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
                &stream::content_block_delta_text(idx, &msg.content),
            ));
        }
        for (i, tc) in msg.tool_calls.iter().enumerate() {
            out.extend(self.open_tool_call(tc, i, request_id));
        }
        if chunk.done {
            self.stop_reason = match chunk.done_reason.as_deref() {
                Some("length") => "max_tokens",
                _ => {
                    if self.tool_indices.is_empty() {
                        "end_turn"
                    } else {
                        "tool_use"
                    }
                }
            };
        }
        out
    }

    fn open_tool_call(&mut self, tc: &OllamaToolCall, i: usize, request_id: &str) -> Vec<Bytes> {
        let mut out = Vec::new();
        if let Some(text_idx) = self.text_block.take() {
            out.push(stream::emit(
                "content_block_stop",
                &stream::content_block_stop(text_idx),
            ));
        }
        let idx = self.next_index;
        self.next_index += 1;
        let id = tc
            .id
            .clone()
            .unwrap_or_else(|| format!("call_{request_id}_{i}"));
        out.push(stream::emit(
            "content_block_start",
            &stream::content_block_start_tool_use(idx, &id, &tc.function.name),
        ));
        let args_str =
            serde_json::to_string(&tc.function.arguments).unwrap_or_else(|_| "{}".into());
        if !args_str.is_empty() {
            out.push(stream::emit(
                "content_block_delta",
                &stream::content_block_delta_input_json(idx, &args_str),
            ));
        }
        self.tool_indices.push(idx);
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

/// Read a `Stream<Item = Result<Bytes, _>>` of NDJSON bytes and yield
/// one line per `next()` call. Strips trailing `\r` and skips blank
/// lines.
struct NdjsonReader<S> {
    inner: S,
    buf: String,
    eof: bool,
}

impl<S> NdjsonReader<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            buf: String::new(),
            eof: false,
        }
    }
}

impl<S> NdjsonReader<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    async fn next(&mut self) -> Option<Result<String, reqwest::Error>> {
        loop {
            if let Some(idx) = self.buf.find('\n') {
                let line = self.buf[..idx].trim_end_matches('\r').to_string();
                self.buf.drain(..=idx);
                return Some(Ok(line));
            }
            if self.eof {
                if self.buf.trim().is_empty() {
                    return None;
                }
                return Some(Ok(std::mem::take(&mut self.buf)));
            }
            match self.inner.next().await {
                Some(Ok(b)) => self.buf.push_str(&String::from_utf8_lossy(&b)),
                Some(Err(e)) => return Some(Err(e)),
                None => self.eof = true,
            }
        }
    }
}
