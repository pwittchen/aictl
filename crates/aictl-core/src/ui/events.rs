//! Serialized agent events for non-terminal frontends.
//!
//! `aictl-cli` renders directly through synchronous [`crate::AgentUI`]
//! method calls. Frontends that don't share a stack frame with the agent
//! loop — `aictl-desktop` (Tauri webview), a future `aictl-server` web
//! UI, in-process tests — need a value-typed event stream they can
//! serialize, queue, and replay. [`AgentEvent`] is that stream: a single
//! enum that mirrors every notification the engine emits and is
//! `serde::Serialize` so frontends can ship it across an IPC boundary
//! verbatim.
//!
//! The terminal frontends do not consume this enum (they don't need
//! serialization) — its purpose is to give richer frontends a stable
//! data shape without forcing the engine to depend on a transport.

use serde::Serialize;

/// A single notification from a running agent turn.
///
/// `kind` is the discriminator so JavaScript / TypeScript consumers can
/// `switch` on a string tag; the inner fields are inlined as siblings to
/// `kind` (serde's adjacent tagging would add a wrapper that the desktop
/// webview doesn't need).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Spinner started — frontend may render an indeterminate progress
    /// indicator with the supplied phrase.
    SpinnerStart { message: String },
    /// Spinner stopped — frontend should hide its progress indicator.
    SpinnerStop,
    /// Reasoning / "thinking" trace from the model. Streamed before any
    /// tool call resolves.
    Reasoning { text: String },
    /// Streaming response is about to begin (after the spinner stops).
    StreamBegin,
    /// Incremental token chunk from the model.
    StreamChunk { text: String },
    /// Stream paused because the next token sequence is part of a tool
    /// call (the `<tool>` prefix matched). The frontend should hide the
    /// in-flight stream surface and render a "preparing tool" affordance
    /// until [`AgentEvent::ToolAuto`] / [`AgentEvent::ToolApprovalRequest`]
    /// arrives.
    StreamSuspend,
    /// Stream finished — the agent has either produced a final answer or
    /// is about to dispatch a tool.
    StreamEnd,
    /// A tool call dispatched without an approval prompt (because the
    /// session is in `--auto` mode or a hook pre-approved it).
    ToolAuto { tool: String, input: String },
    /// Approval requested for a tool call. The frontend renders an
    /// approve/deny modal and responds via the corresponding command
    /// (e.g. `tool_approval_response` in `aictl-desktop`). Until that
    /// response arrives the agent loop is blocked on the oneshot.
    ToolApprovalRequest {
        id: u64,
        tool: String,
        input: String,
    },
    /// Output of a completed tool call. Already-truncated and sanitized
    /// per the security policy.
    ToolResult { text: String },
    /// Final assistant answer for this turn.
    Answer { text: String },
    /// Fatal error from the agent loop.
    Error { text: String },
    /// Non-fatal warning emitted by the engine (redaction notes,
    /// hook timeouts, etc.).
    Warning { text: String },
    /// Per-iteration token-usage / cost snapshot.
    TokenUsage(TokenUsageEvent),
    /// Per-turn summary delivered after the final answer.
    Summary(SummaryEvent),
    /// Long-running progress indicator started.
    ProgressBegin {
        id: u64,
        label: String,
        total: Option<u64>,
    },
    /// Progress indicator advanced.
    ProgressUpdate {
        id: u64,
        current: u64,
        message: Option<String>,
    },
    /// Progress indicator finished.
    ProgressEnd { id: u64, message: Option<String> },
}

/// Wire shape of a [`crate::llm::TokenUsage`] reading. Avoids leaking the
/// engine's token-usage struct (which is not `Serialize`) across the IPC
/// boundary while keeping the per-call accounting that frontends want.
#[derive(Debug, Clone, Serialize)]
pub struct TokenUsageEvent {
    pub model: String,
    pub final_answer: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub tool_calls: u32,
    pub elapsed_ms: u64,
    pub context_pct: u8,
    pub memory: String,
}

/// Wire shape of a turn-final summary. Mirrors the args of
/// [`crate::AgentUI::show_summary`] so frontends can format their own
/// banner without depending on terminal helpers.
#[derive(Debug, Clone, Serialize)]
pub struct SummaryEvent {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub llm_calls: u32,
    pub tool_calls: u32,
    pub elapsed_ms: u64,
    pub context_pct: u8,
}

impl TokenUsageEvent {
    /// Build a [`TokenUsageEvent`] from the engine's token-usage struct
    /// plus the surrounding agent-turn metadata. Centralized here so
    /// every frontend that wants to emit the event uses the same shape.
    #[must_use]
    pub fn from_usage(
        usage: &crate::llm::TokenUsage,
        model: &str,
        final_answer: bool,
        tool_calls: u32,
        elapsed: std::time::Duration,
        context_pct: u8,
        memory: &str,
    ) -> Self {
        Self {
            model: model.to_string(),
            final_answer,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            tool_calls,
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            context_pct,
            memory: memory.to_string(),
        }
    }
}

impl SummaryEvent {
    /// Build a [`SummaryEvent`] from the same arguments
    /// [`crate::AgentUI::show_summary`] receives.
    #[must_use]
    pub fn from_usage(
        usage: &crate::llm::TokenUsage,
        model: &str,
        llm_calls: u32,
        tool_calls: u32,
        elapsed: std::time::Duration,
        context_pct: u8,
    ) -> Self {
        Self {
            model: model.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            llm_calls,
            tool_calls,
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            context_pct,
        }
    }
}
