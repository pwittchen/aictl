//! Streaming infrastructure shared across providers.
//!
//! Two responsibilities:
//!
//! 1. The [`StreamState`] suspend-buffer used by the agent loop's [`TokenSink`]
//!    to hide the `<tool name="…">` XML prefix from the user once a tool call
//!    starts emerging from the stream.
//!
//! 2. A small SSE line splitter ([`SseLines`]) consumed by every
//!    OpenAI-compatible provider (`openai`, `grok`, `mistral`, `deepseek`,
//!    `kimi`, `zai`) and Anthropic. Gemini also uses it (it serves SSE on the
//!    `streamGenerateContent?alt=sse` endpoint). Ollama uses raw JSONL and has
//!    its own splitter.
//!
//! [`TokenSink`]: super::TokenSink

/// Exact prefix the LLM must emit to start a tool call. Anything that is a
/// prefix of this literal is held back from the UI until we see whether it
/// completes the prefix (→ suspend) or diverges (→ flush + resume).
pub const TOOL_PREFIX: &str = "<tool name=\"";

/// State machine for the streaming UI. Lives inside the [`super::TokenSink`]
/// closure built by `run_agent_turn`, accumulating every delta the provider
/// emits. The closure asks `accept` what (if anything) to forward to the UI
/// and whether the tool-prefix has been definitively matched.
#[derive(Default)]
pub struct StreamState {
    /// Every delta concatenated, in order. The agent loop ignores the
    /// provider's returned full string in favor of this — the two are
    /// supposed to match, but having a single source of truth avoids
    /// any provider-side discrepancy.
    pub full: String,
    /// Tail of `full` we have not yet decided on. Either flushed (when it
    /// can no longer be a prefix of [`TOOL_PREFIX`]) or held until we see
    /// either a full match or a divergence.
    pending: String,
    /// Once true, [`TOOL_PREFIX`] has matched — drop further deltas from
    /// the user-visible stream. The full text is still recorded in `full`
    /// so `parse_tool_call` can act on it after the stream ends.
    pub suspended: bool,
}

/// Outcome of feeding one delta into [`StreamState::accept`].
#[derive(Debug, Clone)]
pub struct Accepted {
    /// Bytes ready for the UI. Empty when the state machine is still
    /// holding the tail to decide tool-prefix vs. ordinary prose.
    pub emit: String,
    /// `true` exactly once — on the delta that completed the match against
    /// [`TOOL_PREFIX`]. The agent loop uses this to signal the UI to flush
    /// any buffered word-wrap tail and show a "preparing tool call…"
    /// spinner during the (otherwise invisible) tool-XML stream.
    pub became_suspended: bool,
}

impl StreamState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one delta from the provider. Returns the bytes (if any) the UI
    /// should display now, plus whether this delta was the one that flipped
    /// the suspend flag.
    ///
    /// Algorithm:
    /// - Append to `full` unconditionally.
    /// - If already suspended, return empty — provider may keep streaming
    ///   (tool args, closing tag) but the user has already been hidden from it.
    /// - Otherwise grow `pending` and split it: the leading bytes that are
    ///   guaranteed to NOT be part of [`TOOL_PREFIX`] are flushed; the
    ///   trailing bytes that could still be a prefix of it are kept.
    /// - If the kept tail equals [`TOOL_PREFIX`] (or starts with it), set
    ///   `suspended = true` and drop everything after the prefix start.
    pub fn accept(&mut self, delta: &str) -> Accepted {
        self.full.push_str(delta);
        if self.suspended {
            return Accepted {
                emit: String::new(),
                became_suspended: false,
            };
        }

        self.pending.push_str(delta);
        let (flush, keep, matched) = split_pending(&self.pending);
        self.pending = keep;
        if matched {
            self.suspended = true;
            // Anything we held back (the partial prefix) is now confirmed
            // tool-call markup — discard it from the UI buffer.
            self.pending.clear();
            return Accepted {
                emit: flush,
                became_suspended: true,
            };
        }
        Accepted {
            emit: flush,
            became_suspended: false,
        }
    }

    /// Flush whatever is still pending. Called once the stream ends; if a
    /// tool call was never matched, the held tail is just ordinary text and
    /// the UI deserves to see it. Currently unused in production (the agent
    /// loop relies on the provider's full assembled string instead) but kept
    /// for callers that want to consume the suspend buffer in isolation, and
    /// covered by tests so future use doesn't bit-rot.
    #[allow(dead_code)]
    pub fn flush(&mut self) -> String {
        if self.suspended {
            self.pending.clear();
            return String::new();
        }
        std::mem::take(&mut self.pending)
    }
}

/// Split `pending` into `(safe_to_flush, keep, matched_prefix)`.
///
/// `matched_prefix == true` means [`TOOL_PREFIX`] appears somewhere in
/// `pending`; everything before that occurrence is flushed (with trailing
/// whitespace trimmed so model-emitted `\n\n` before the tool tag doesn't
/// show as blank lines above the tool call) and the prefix itself is
/// consumed (so it never reaches the UI).
///
/// When the prefix is not yet present, `keep` is the longest suffix of
/// `pending` that could still grow into the prefix (length 0..=PREFIX_LEN-1),
/// extended backwards over any trailing whitespace so a tool tag arriving in
/// the next chunk discards that whitespace too. `safe_to_flush` is the rest.
/// A short pending buffer that is itself a prefix of [`TOOL_PREFIX`] (or
/// trailing whitespace) is held entirely.
fn split_pending(pending: &str) -> (String, String, bool) {
    if let Some(idx) = pending.find(TOOL_PREFIX) {
        let flush = pending[..idx].trim_end().to_string();
        return (flush, String::new(), true);
    }
    // Find the longest suffix of `pending` that is a (proper) prefix of
    // TOOL_PREFIX. We can hold at most PREFIX_LEN-1 bytes.
    let prefix_len = TOOL_PREFIX.len();
    let max_keep = prefix_len.saturating_sub(1).min(pending.len());
    let mut keep_start = pending.len();
    // Walk from the longest possible kept suffix down to 1; the first one
    // that is a prefix of TOOL_PREFIX is what we keep.
    for keep_len in (1..=max_keep).rev() {
        // Only consider suffixes that begin on a UTF-8 char boundary so
        // slicing is safe.
        let split = pending.len() - keep_len;
        if !pending.is_char_boundary(split) {
            continue;
        }
        let suffix = &pending[split..];
        if TOOL_PREFIX.starts_with(suffix) {
            keep_start = split;
            break;
        }
    }
    // Extend the kept suffix backwards over trailing ASCII whitespace so a
    // tool tag in the next chunk drops that whitespace cleanly. If no tool
    // tag follows, the held whitespace is flushed alongside the next chunk's
    // content.
    let bytes = pending.as_bytes();
    while keep_start > 0 && bytes[keep_start - 1].is_ascii_whitespace() {
        keep_start -= 1;
    }
    let flush = pending[..keep_start].to_string();
    let keep = pending[keep_start..].to_string();
    (flush, keep, false)
}

/// Drive an `OpenAI`-compatible SSE stream: forward `choices[0].delta.content`
/// to `on_token`, accumulate the full text, and extract a `usage` block from
/// whichever event carries it.
///
/// Used by `openai`, `grok`, `mistral`, `deepseek`, `kimi`, and `zai`. Each
/// provider passes its own `parse_usage` closure to handle the tiny shape
/// variations (e.g. `OpenAI`'s nested `prompt_tokens_details.cached_tokens`,
/// `DeepSeek`'s `prompt_cache_hit_tokens`, Kimi's nested cache field).
///
/// `on_token` receives **delta** strings — never the cumulative buffer.
/// Returns the assembled full text plus the parsed usage. Usage defaults to
/// `TokenUsage::default()` when no event in the stream carries one.
pub async fn drive_openai_compatible_stream<F>(
    response: reqwest::Response,
    on_token: &super::TokenSink,
    mut parse_usage: F,
) -> Result<(String, super::TokenUsage), Box<dyn std::error::Error>>
where
    F: FnMut(&serde_json::Value) -> Option<super::TokenUsage>,
{
    use futures_util::StreamExt;

    let mut bytes = response.bytes_stream();
    let mut sse = SseLines::new();
    let mut full = String::new();
    let mut usage = super::TokenUsage::default();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk?;
        for line in sse.push(&chunk) {
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            // Delta content
            if let Some(delta) = v["choices"][0]["delta"]["content"].as_str()
                && !delta.is_empty()
            {
                full.push_str(delta);
                on_token(delta);
            }
            // Usage may arrive in this event (typically the final one when
            // `stream_options.include_usage = true` is set).
            if let Some(u) = parse_usage(&v) {
                usage = u;
            }
        }
    }

    Ok((full, usage))
}

/// Splits a stream of bytes into Server-Sent-Event lines.
///
/// Each call to [`SseLines::push`] appends new bytes and yields any complete
/// lines (terminated by `\n`, optional trailing `\r` stripped). Incomplete
/// trailing data is buffered until the next push (or [`SseLines::take_remaining`]).
#[derive(Default)]
pub struct SseLines {
    buf: Vec<u8>,
}

impl SseLines {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `chunk` and return any lines that completed. UTF-8 decoding is
    /// lossy on a per-line basis (invalid sequences become `?`), which is
    /// fine: every provider we deal with sends valid UTF-8 SSE.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop(); // drop \n
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        out
    }

    /// Final buffered partial line, if any. Most servers terminate the stream
    /// with `\n\n` so this is usually empty.
    pub fn take_remaining(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let line: Vec<u8> = std::mem::take(&mut self.buf);
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_match() -> &'static str {
        TOOL_PREFIX
    }

    #[test]
    fn pure_prose_is_flushed_immediately() {
        let mut s = StreamState::new();
        let r = s.accept("hello world");
        assert_eq!(r.emit, "hello world");
        assert!(!r.became_suspended);
        assert!(!s.suspended);
    }

    #[test]
    fn prefix_in_one_chunk_suspends() {
        let mut s = StreamState::new();
        let r = s.accept(&format!("here we go {}", full_match()));
        // Trailing whitespace before the tool tag is dropped so the UI
        // doesn't show a hanging space (or blank line) above the call.
        assert_eq!(r.emit, "here we go");
        assert!(r.became_suspended);
        assert!(s.suspended);
    }

    #[test]
    fn trailing_newlines_before_tool_are_trimmed() {
        let mut s = StreamState::new();
        let r = s.accept(&format!("I'll do this.\n\n{}", full_match()));
        assert_eq!(r.emit, "I'll do this.");
        assert!(r.became_suspended);
    }

    #[test]
    fn trailing_whitespace_held_then_flushed_with_next_chunk() {
        let mut s = StreamState::new();
        let r1 = s.accept("hello\n\n");
        // Trailing newlines are buffered, not flushed yet — we don't know
        // whether a tool tag is about to follow.
        assert_eq!(r1.emit, "hello");
        let r2 = s.accept("more");
        // Non-tool continuation: the held whitespace flushes with the
        // new content.
        assert_eq!(r2.emit, "\n\nmore");
        assert!(!s.suspended);
    }

    #[test]
    fn trailing_whitespace_dropped_when_tool_follows_in_next_chunk() {
        let mut s = StreamState::new();
        let r1 = s.accept("hello\n\n");
        assert_eq!(r1.emit, "hello");
        let r2 = s.accept(full_match());
        assert_eq!(r2.emit, "");
        assert!(r2.became_suspended);
    }

    #[test]
    fn prefix_split_across_chunks_suspends_eventually() {
        let mut s = StreamState::new();
        // Feed the prefix one byte at a time. None of the partial prefixes
        // should be flushed, and the final byte completes the match.
        let mut suspended_at = None;
        for (i, c) in full_match().chars().enumerate() {
            let r = s.accept(&c.to_string());
            assert_eq!(r.emit, "");
            if r.became_suspended {
                suspended_at = Some(i);
                break;
            }
        }
        assert_eq!(suspended_at, Some(full_match().chars().count() - 1));
        assert!(s.suspended);
    }

    #[test]
    fn diverging_prefix_is_flushed() {
        let mut s = StreamState::new();
        // Looks like the start of <tool name=" ... but diverges
        let r1 = s.accept("<tool ");
        assert_eq!(r1.emit, "");
        assert!(!r1.became_suspended);
        let r2 = s.accept("name='x'>");
        // Now <tool name=' ... is held until we know it diverges from
        // <tool name=" — at the apostrophe vs. quote we know it diverges.
        // The full pending is "<tool name='x'>" — TOOL_PREFIX (<tool name=")
        // is not a substring, and no suffix is a prefix of TOOL_PREFIX.
        assert_eq!(format!("{}{}", r1.emit, r2.emit), "<tool name='x'>");
        assert!(!s.suspended);
    }

    #[test]
    fn flush_returns_pending_when_not_suspended() {
        let mut s = StreamState::new();
        // End mid-prefix
        let r = s.accept("<tool nam");
        assert_eq!(r.emit, "");
        let leftover = s.flush();
        assert_eq!(leftover, "<tool nam");
    }

    #[test]
    fn flush_drops_pending_when_suspended() {
        let mut s = StreamState::new();
        let _ = s.accept(full_match());
        let leftover = s.flush();
        assert_eq!(leftover, "");
    }

    #[test]
    fn full_text_always_records_everything() {
        let mut s = StreamState::new();
        let _ = s.accept("here ");
        let _ = s.accept(full_match());
        let _ = s.accept("read_file\">a.txt</tool>");
        // The user only saw "here " — but `full` carries the whole response
        // for parse_tool_call.
        assert!(s.full.starts_with("here "));
        assert!(s.full.contains(TOOL_PREFIX));
        assert!(s.full.ends_with("</tool>"));
    }

    // --- SSE splitter ---

    #[test]
    fn sse_splits_lines_lf_only() {
        let mut s = SseLines::new();
        let lines = s.push(b"a\nbb\ncc\n");
        assert_eq!(lines, vec!["a", "bb", "cc"]);
    }

    #[test]
    fn sse_splits_lines_crlf() {
        let mut s = SseLines::new();
        let lines = s.push(b"data: 1\r\ndata: 2\r\n\r\n");
        assert_eq!(lines, vec!["data: 1", "data: 2", ""]);
    }

    #[test]
    fn sse_buffers_partial_line_across_pushes() {
        let mut s = SseLines::new();
        let l1 = s.push(b"par");
        assert!(l1.is_empty());
        let l2 = s.push(b"tial\nrest");
        assert_eq!(l2, vec!["partial"]);
        let l3 = s.push(b"\n");
        assert_eq!(l3, vec!["rest"]);
    }

    #[test]
    fn sse_take_remaining_returns_unterminated_tail() {
        let mut s = SseLines::new();
        let _ = s.push(b"complete\nhalf");
        assert_eq!(s.take_remaining().as_deref(), Some("half"));
        assert_eq!(s.take_remaining(), None);
    }
}
