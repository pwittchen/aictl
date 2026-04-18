//! Scripted mock LLM provider for integration tests.
//!
//! The agent loop in `run::run_agent_turn` dispatches by `Provider` enum; the
//! `Mock` variant (cfg-gated to `#[cfg(test)]`) routes here. Tests construct a
//! [`MockGuard`], push a sequence of scripted responses, and drive the real
//! agent loop — which parses `<tool>` tags, executes tools, handles denials,
//! guards duplicate calls, and loops up to `max_iterations` — without ever
//! touching a network.
//!
//! A process-wide `Mutex<()>` serializes tests that rely on the mock state, so
//! parallel `cargo test` execution is safe. The guard resets both the mock
//! script/call log and the session-wide tool-call history on construction so
//! each test starts from a clean slate.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::Message;
use crate::llm::{TokenSink, TokenUsage};

/// Outcome of a single scripted LLM call: either `Ok((response_text, usage))`
/// or `Err(reason)` to simulate a provider failure.
pub(crate) type MockResult = Result<(String, TokenUsage), String>;

#[derive(Default)]
struct MockState {
    scripts: VecDeque<MockResult>,
    /// Captured messages slice per call, in order. Tests assert over this to
    /// verify system prompt, tool-result injection, and conversation history
    /// shape without scraping the UI.
    calls: Vec<Vec<Message>>,
}

static MOCK_STATE: Mutex<MockState> = Mutex::new(MockState {
    scripts: VecDeque::new(),
    calls: Vec::new(),
});

/// Process-wide lock held by [`MockGuard`] so only one mock-using test can be
/// driving the agent loop at a time. Without it, `cargo test`'s parallel
/// runner would interleave script pops across tests.
static MOCK_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard held for the duration of a mock-using test.
///
/// Construction acquires the process-wide mock lock, clears the scripted
/// response queue / call log, and clears the tool-call history so duplicate
/// detection from prior tests doesn't leak in. On drop the lock releases and
/// the next waiting test can proceed.
pub(crate) struct MockGuard {
    _guard: MutexGuard<'static, ()>,
}

// The `&self` receivers on the push/inspect methods below look unused, but they
// tie method calls to the guard's lifetime — you can't push to the mock queue
// without first acquiring the guard, which serializes access across tests.
#[allow(clippy::unused_self)]
impl MockGuard {
    pub fn new() -> Self {
        let guard = MOCK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        {
            let mut st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
            st.scripts.clear();
            st.calls.clear();
        }
        crate::tools::clear_call_history();
        Self { _guard: guard }
    }

    /// Queue a scripted response with default placeholder token usage.
    pub fn push_response(&self, response: impl Into<String>) {
        self.push_response_with_usage(
            response,
            TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                ..TokenUsage::default()
            },
        );
    }

    /// Queue a scripted response with explicit token usage.
    pub fn push_response_with_usage(&self, response: impl Into<String>, usage: TokenUsage) {
        let mut st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        st.scripts.push_back(Ok((response.into(), usage)));
    }

    /// Queue a scripted provider error.
    #[allow(dead_code)]
    pub fn push_error(&self, err: impl Into<String>) {
        let mut st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        st.scripts.push_back(Err(err.into()));
    }

    /// Snapshot of the `messages` slices the mock saw, one entry per LLM call.
    pub fn calls(&self) -> Vec<Vec<Message>> {
        let st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        st.calls.clone()
    }

    /// Number of LLM calls the mock has received so far.
    pub fn call_count(&self) -> usize {
        let st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        st.calls.len()
    }
}

/// Scripted provider entry point. Matches the signature of real providers
/// (`&str, &[Message], Option<TokenSink>`) so `run_agent_turn`'s dispatch arm
/// can call it identically. A single-chunk stream is emitted to the sink when
/// one is provided so the streaming code path is exercised too.
///
/// The function is `async` to match the real provider signatures even though
/// it never awaits — the agent loop wraps each provider call in
/// `tokio::time::timeout`, which requires a future.
#[allow(clippy::unused_async)]
pub async fn call_mock(
    _model: &str,
    messages: &[Message],
    on_token: Option<TokenSink>,
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    let response = {
        let mut st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        st.calls.push(messages.to_vec());
        st.scripts
            .pop_front()
            .ok_or_else(|| -> Box<dyn std::error::Error> {
                "mock script exhausted — no more scripted responses queued".into()
            })?
    };
    let (content, usage) = response.map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    if let Some(sink) = on_token {
        sink(&content);
    }
    Ok((content, usage))
}
