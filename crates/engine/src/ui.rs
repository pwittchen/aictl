//! Engine-side UI surface.
//!
//! This module owns the [`AgentUI`] trait that frontends implement, plus
//! the supporting value types ([`ToolApproval`], [`ProgressHandle`],
//! [`WarningSink`]) and the global warning sink. Concrete frontend impls
//! (terminal `PlainUI`/`InteractiveUI`, future `HttpUI`/`DesktopUI`) live
//! in their respective frontend crates and depend only on this trait —
//! the engine itself never names a terminal type.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use crate::llm::TokenUsage;
use crate::tools::ToolCall;

/// Result of a tool-call confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApproval {
    Allow,
    Deny,
    AutoAccept,
}

/// Frontend-supplied progress backend. The engine passes the handle to
/// long-running helpers (model downloads, etc.) and they call back via
/// [`AgentUI::progress_update`] / [`AgentUI::progress_end`]. Frontend
/// implementations can hold any state they need behind this trait —
/// `indicatif::ProgressBar`, an HTTP push channel, a no-op stub.
pub trait ProgressBackend: Send + Sync {
    fn update(&self, current: u64, message: Option<&str>);
    fn finish(&self, final_message: Option<&str>);
}

/// Opaque handle threaded between [`AgentUI::progress_begin`] /
/// `progress_update` / `progress_end`. Holds an optional
/// [`ProgressBackend`] — `None` means "no visible progress" (single-shot,
/// piped, etc.).
pub struct ProgressHandle(Option<Box<dyn ProgressBackend>>);

impl ProgressHandle {
    #[must_use]
    pub fn noop() -> Self {
        Self(None)
    }

    #[must_use]
    pub fn from_backend(backend: Box<dyn ProgressBackend>) -> Self {
        Self(Some(backend))
    }

    pub(crate) fn backend(&self) -> Option<&dyn ProgressBackend> {
        self.0.as_deref()
    }
}

// --- Engine-side warning sink --------------------------------------
//
// A handful of runtime call sites (notably the feature-gated NER
// inference loader in `security/redaction/ner.rs`) need to emit a
// one-shot warning from deep inside a redaction pass — far away from
// any `&dyn AgentUI` reference. Threading a UI through every detector
// would balloon signatures for what is fundamentally an engine-internal
// concern, so the engine exposes a global sink that the active frontend
// installs once at startup.
//
// The sink is set-once (not replaceable) to keep the install path
// simple: the first frontend that asks wins. If no sink is set the
// fallback writes to stderr, matching pre-refactor behavior.

/// Boxed warning callback installed via [`set_warning_sink`] and consumed
/// by [`warn_global`].
pub type WarningSink = Box<dyn Fn(&str) + Send + Sync>;

static WARN_SINK: OnceLock<WarningSink> = OnceLock::new();

/// Install the global warning sink. The first call wins; subsequent
/// calls return `Err` and leave the existing sink in place.
///
/// # Errors
///
/// Returns the supplied sink unchanged if a sink has already been
/// installed.
pub fn set_warning_sink(sink: WarningSink) -> Result<(), WarningSink> {
    WARN_SINK.set(sink)
}

/// Emit a warning via the installed sink, falling back to stderr.
/// Engine code uses this for non-fatal runtime conditions where no
/// `&dyn AgentUI` is reachable (deep helpers, feature-gated callbacks).
pub fn warn_global(text: &str) {
    if let Some(sink) = WARN_SINK.get() {
        sink(text);
    } else {
        eprintln!("Warning: {text}");
    }
}

// ── AgentUI trait ────────────────────────────────────────────────────

pub trait AgentUI {
    fn start_spinner(&self, msg: &str);
    fn stop_spinner(&self);
    fn show_reasoning(&self, text: &str);
    fn show_auto_tool(&self, tool_call: &ToolCall);
    fn show_tool_result(&self, result: &str);
    fn confirm_tool(&self, tool_call: &ToolCall) -> ToolApproval;
    fn show_answer(&self, text: &str);
    fn show_error(&self, text: &str);
    /// Begin a streamed response — called once just before the first
    /// `stream_chunk` of a turn, after the spinner has been stopped.
    /// Used by the interactive UI to draw the top frame; the plain UI
    /// does nothing.
    fn stream_begin(&self) {}
    /// Forward an incremental delta of text from the LLM stream to the user.
    /// Empty deltas should be tolerated (and ignored). The default impl is a
    /// no-op so providers don't need to special-case UIs that don't render
    /// progressively.
    fn stream_chunk(&self, _text: &str) {}
    /// Called once when the streaming state machine has confirmed the start
    /// of a tool call (the `<tool name="…">` prefix matched). The
    /// interactive UI uses this hook to flush any word-wrap buffer — so the
    /// last word of the reasoning appears immediately instead of hanging
    /// until `stream_end` — and to start a "preparing tool call…" spinner
    /// that fills the otherwise-silent gap while the model streams the
    /// (hidden) tool XML. No-op by default.
    fn stream_suspend(&self) {}
    /// End a streamed response — called once after the stream completes
    /// (whether or not a tool call was detected). Draws the bottom frame
    /// in the interactive UI; no-op in plain UI.
    fn stream_end(&self) {}
    #[allow(clippy::too_many_arguments)]
    fn show_token_usage(
        &self,
        usage: &TokenUsage,
        model: &str,
        final_answer: bool,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
        memory: &str,
    );
    fn show_summary(
        &self,
        usage: &TokenUsage,
        model: &str,
        llm_calls: u32,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
    );

    /// Non-fatal warning. Default writes to stderr; UI implementations
    /// can override to integrate with their own framing.
    fn warn(&self, text: &str) {
        eprintln!("Warning: {text}");
    }

    /// Begin a long-running operation indicator. Default returns a no-op
    /// handle so non-terminal frontends don't render anything.
    fn progress_begin(&self, _label: &str, _total: Option<u64>) -> ProgressHandle {
        ProgressHandle::noop()
    }

    /// Advance a progress indicator. Forwards to the handle's backend
    /// (if any); UI impls only need to override this when they want to
    /// intercept updates without a backend.
    fn progress_update(&self, handle: &ProgressHandle, current: u64, message: Option<&str>) {
        if let Some(b) = handle.backend() {
            b.update(current, message);
        }
    }

    /// Finish a progress indicator. Default forwards to the handle's
    /// backend; consuming the handle prevents accidental reuse.
    fn progress_end(&self, handle: ProgressHandle, final_message: Option<&str>) {
        if let Some(b) = handle.backend() {
            b.finish(final_message);
        }
    }

    /// Future that resolves when the user signals cancellation (Esc key
    /// in the interactive REPL). Default never resolves — used by
    /// single-shot, piped, server, and desktop frontends where there's
    /// no Esc channel.
    fn interruption(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}
