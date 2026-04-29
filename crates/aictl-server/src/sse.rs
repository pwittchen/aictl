//! SSE framing helpers.
//!
//! The streaming gateway emits OpenAI-compatible
//! `data: {"choices":[{"delta":...}]}` frames followed by a final
//! `data: [DONE]`. Axum's `Sse<Stream>` body accepts any
//! `TryStream<Item = Result<Event, _>>`; we convert the engine's
//! `TokenSink` callbacks into a channel and shape each chunk into an
//! `Event::default().data(json)`.

use axum::response::sse::Event;
use serde::Serialize;

/// Serialize a streaming chunk as a single SSE `data:` event. Returns
/// the JSON-encoded payload wrapped in an `Event`.
pub fn data_event<T: Serialize>(value: &T) -> Result<Event, serde_json::Error> {
    let json = serde_json::to_string(value)?;
    Ok(Event::default().data(json))
}

/// The terminal frame OpenAI clients expect after the final delta.
#[must_use]
pub fn done_event() -> Event {
    Event::default().data("[DONE]")
}

/// An error frame the gateway emits when an upstream provider errors
/// out mid-stream. Mirrors the OpenAI-style envelope so SDKs that
/// already handle their `error` events keep working.
#[must_use]
pub fn error_event(code: &str, message: &str) -> Event {
    let payload = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
        }
    });
    Event::default().data(payload.to_string())
}
