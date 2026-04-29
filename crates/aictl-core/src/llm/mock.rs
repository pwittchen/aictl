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
use crate::error::AictlError;
use crate::llm::{TokenSink, TokenUsage};

/// Env var for spawned-binary tests (out-of-crate `tests/`): path to a file
/// where each non-empty line is one scripted response. Consumed lazily on the
/// first `call_mock` invocation that finds `MOCK_STATE.scripts` empty, so
/// in-crate `MockGuard`-driven tests are unaffected.
pub const MOCK_RESPONSES_FILE_ENV: &str = "AICTL_MOCK_RESPONSES_FILE";

/// Marker separator between multi-line responses in the env-driven response
/// file. A response in the file is everything between markers (or between the
/// start/end of file and a marker); newlines inside a response are preserved.
/// Tests can use multi-line responses by placing `---` on its own line.
const MOCK_FILE_SEPARATOR: &str = "---";

/// Outcome of a single scripted LLM call: either `Ok((response_text, usage))`
/// or `Err(reason)` to simulate a provider failure.
type MockResult = Result<(String, TokenUsage), String>;

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
///
/// Compiled into release builds too — `cargo test` from the CLI crate
/// builds the engine in non-test mode, so the test helper has to live
/// outside `#[cfg(test)]`. The struct is inert when nothing is calling
/// `call_mock`.
pub struct MockGuard {
    _guard: MutexGuard<'static, ()>,
}

impl Default for MockGuard {
    fn default() -> Self {
        Self::new()
    }
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

/// Parse a `MOCK_FILE_SEPARATOR`-delimited response file into a list of
/// scripted responses. Responses are the text blocks between separator lines
/// (or between start/end of file and a separator). Leading/trailing whitespace
/// is trimmed per-response and empty responses are dropped so a trailing
/// newline in the file doesn't script an extra empty reply.
fn parse_mock_file(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if line.trim() == MOCK_FILE_SEPARATOR {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                out.push(trimmed);
            }
            current.clear();
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        out.push(trimmed);
    }
    out
}

/// Lazily populate `MOCK_STATE.scripts` from the env-driven response file if
/// it's currently empty. No-op when the env var is unset (the in-crate
/// `MockGuard` tests path) or when the file cannot be read. Responses already
/// queued by `MockGuard::push_response` take precedence; this only fills in
/// on first-miss for spawned-binary tests.
fn load_scripts_from_env(state: &mut MockState) {
    if !state.scripts.is_empty() {
        return;
    }
    let Ok(path) = std::env::var(MOCK_RESPONSES_FILE_ENV) else {
        return;
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    for response in parse_mock_file(&content) {
        state.scripts.push_back(Ok((
            response,
            TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                ..TokenUsage::default()
            },
        )));
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
) -> Result<(String, TokenUsage), AictlError> {
    let response = {
        let mut st = MOCK_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        st.calls.push(messages.to_vec());
        load_scripts_from_env(&mut st);
        st.scripts.pop_front().ok_or_else(|| {
            AictlError::Other(
                "mock script exhausted — no more scripted responses queued".to_string(),
            )
        })?
    };
    let (content, usage) = response.map_err(AictlError::Other)?;
    if let Some(sink) = on_token {
        sink(&content);
    }
    Ok((content, usage))
}
