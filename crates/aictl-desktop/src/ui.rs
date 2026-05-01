//! `AgentUI` implementation for the desktop webview.
//!
//! Every method on the trait fans out as a [`tauri::Emitter::emit`]
//! call carrying an [`AgentEvent`]. The webview listens on the
//! `agent_event` channel (declared in `lib/ipc.ts`) and renders.
//!
//! Tool approval is the one surface that needs a return value from the
//! webview: `confirm_tool_async` registers a oneshot in
//! [`crate::state::AppState::pending_approvals`], emits a
//! `ToolApprovalRequest` event, and parks until the
//! `tool_approval_response` Tauri command resolves the oneshot.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use aictl_core::llm::TokenUsage;
use aictl_core::tools::ToolCall;
use aictl_core::ui::events::{AgentEvent, SummaryEvent, TokenUsageEvent};
use aictl_core::{AgentUI, ToolApproval};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::state::AppState;

const EVENT_AGENT: &str = "agent_event";
const EVENT_WORKSPACE: &str = "workspace_changed";

/// Install a [`aictl_core::ui::WarningSink`] that forwards every
/// engine-side warning through the same `agent_event` channel as the
/// rest of the UI stream. Called once during [`crate::run`] setup.
pub fn install_warning_sink(app: AppHandle) {
    let sink: aictl_core::ui::WarningSink = Box::new(move |text: &str| {
        emit_agent(
            &app,
            AgentEvent::Warning {
                text: text.to_string(),
            },
        );
    });
    // `set_warning_sink` is set-once. If the desktop process happens to
    // be embedded somewhere that already registered a sink, fall back
    // silently — the engine writes to stderr in that case, which Tauri
    // surfaces in `console.app` / the Xcode log stream.
    let _ = aictl_core::ui::set_warning_sink(sink);
}

/// Push a freshly-changed workspace path to every open window so the
/// title bar / sidebar / Workspace pane resync.
pub fn emit_workspace_changed(app: &AppHandle, path: Option<&str>) {
    #[derive(Serialize, Clone)]
    struct Payload {
        path: Option<String>,
    }
    let _ = app.emit(
        EVENT_WORKSPACE,
        Payload {
            path: path.map(str::to_string),
        },
    );
}

fn emit_agent(app: &AppHandle, event: AgentEvent) {
    let _ = app.emit(EVENT_AGENT, event);
}

/// Tauri-side implementation of [`AgentUI`].
///
/// Holds an `AppHandle` (so it can `emit` from any thread) plus a
/// reference to the shared [`AppState`] for tool-approval routing. The
/// struct itself is `Send + Sync` — the engine drives it from a tokio
/// task spawned by `commands::chat::send_message`.
pub struct DesktopUI {
    app: AppHandle,
    state: Arc<AppState>,
}

impl DesktopUI {
    pub fn new(app: AppHandle, state: Arc<AppState>) -> Self {
        Self { app, state }
    }

    /// Convenience for chat-side error pathways — used by `chat::run_turn`
    /// when the agent loop itself returns an `AictlError` rather than
    /// emitting through the trait.
    pub fn emit_error(&self, text: &str) {
        emit_agent(
            &self.app,
            AgentEvent::Error {
                text: text.to_string(),
            },
        );
    }

    pub fn emit_warning(&self, text: &str) {
        emit_agent(
            &self.app,
            AgentEvent::Warning {
                text: text.to_string(),
            },
        );
    }
}

impl AgentUI for DesktopUI {
    fn start_spinner(&self, msg: &str) {
        emit_agent(
            &self.app,
            AgentEvent::SpinnerStart {
                message: msg.to_string(),
            },
        );
    }

    fn stop_spinner(&self) {
        emit_agent(&self.app, AgentEvent::SpinnerStop);
    }

    fn show_reasoning(&self, text: &str) {
        emit_agent(
            &self.app,
            AgentEvent::Reasoning {
                text: text.to_string(),
            },
        );
    }

    fn show_auto_tool(&self, tool_call: &ToolCall) {
        emit_agent(
            &self.app,
            AgentEvent::ToolAuto {
                tool: tool_call.name.clone(),
                input: tool_call.input.clone(),
            },
        );
    }

    fn show_tool_result(&self, result: &str) {
        emit_agent(
            &self.app,
            AgentEvent::ToolResult {
                text: result.to_string(),
            },
        );
    }

    /// Synchronous fallback. The engine never calls this directly — the
    /// agent loop always goes through `confirm_tool_async`. We deny if
    /// the sync path is somehow reached so a routing regression never
    /// silently auto-approves a tool call.
    fn confirm_tool(&self, _tool_call: &ToolCall) -> ToolApproval {
        self.warn(
            "AgentUI::confirm_tool called synchronously — desktop expects confirm_tool_async; \
             denying as a safety default.",
        );
        ToolApproval::Deny
    }

    fn confirm_tool_async<'a>(
        &'a self,
        tool_call: &'a ToolCall,
    ) -> Pin<Box<dyn Future<Output = ToolApproval> + Send + 'a>> {
        let id = self.state.next_approval_id();
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.state.pending_approvals.lock() {
            map.insert(id, tx);
        }
        emit_agent(
            &self.app,
            AgentEvent::ToolApprovalRequest {
                id,
                tool: tool_call.name.clone(),
                input: tool_call.input.clone(),
            },
        );
        Box::pin(async move { rx.await.unwrap_or(ToolApproval::Deny) })
    }

    fn show_answer(&self, text: &str) {
        emit_agent(
            &self.app,
            AgentEvent::Answer {
                text: text.to_string(),
            },
        );
    }

    fn show_error(&self, text: &str) {
        emit_agent(
            &self.app,
            AgentEvent::Error {
                text: text.to_string(),
            },
        );
    }

    fn stream_begin(&self) {
        emit_agent(&self.app, AgentEvent::StreamBegin);
    }

    fn stream_chunk(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        emit_agent(
            &self.app,
            AgentEvent::StreamChunk {
                text: text.to_string(),
            },
        );
    }

    fn stream_suspend(&self) {
        emit_agent(&self.app, AgentEvent::StreamSuspend);
    }

    fn stream_end(&self) {
        emit_agent(&self.app, AgentEvent::StreamEnd);
    }

    fn show_token_usage(
        &self,
        usage: &TokenUsage,
        model: &str,
        final_answer: bool,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
        memory: &str,
    ) {
        emit_agent(
            &self.app,
            AgentEvent::TokenUsage(TokenUsageEvent::from_usage(
                usage,
                model,
                final_answer,
                tool_calls,
                elapsed,
                context_pct,
                memory,
            )),
        );
    }

    fn show_summary(
        &self,
        usage: &TokenUsage,
        model: &str,
        llm_calls: u32,
        tool_calls: u32,
        elapsed: Duration,
        context_pct: u8,
    ) {
        emit_agent(
            &self.app,
            AgentEvent::Summary(SummaryEvent::from_usage(
                usage,
                model,
                llm_calls,
                tool_calls,
                elapsed,
                context_pct,
            )),
        );
    }

    fn warn(&self, text: &str) {
        emit_agent(
            &self.app,
            AgentEvent::Warning {
                text: text.to_string(),
            },
        );
    }
}
