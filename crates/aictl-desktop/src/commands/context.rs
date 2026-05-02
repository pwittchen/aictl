//! Context-pane Tauri command.
//!
//! Reports the live state of the active conversation: input-token count
//! against the model's context window, message count against the
//! desktop's transcript cap, and the auto-compact threshold. Mirrors
//! the data the CLI's `/context` slash command prints.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use aictl_core::config::{MAX_MESSAGES, auto_compact_threshold, config_get};
use aictl_core::llm;
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Serialize)]
pub struct ContextStatus {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub last_input_tokens: u64,
    pub last_output_tokens: u64,
    pub context_limit: u64,
    pub messages: usize,
    pub max_messages: usize,
    pub token_pct: u8,
    pub message_pct: u8,
    pub context_pct: u8,
    pub auto_compact_threshold: u8,
    /// Whether the threshold is user-configured (`true`) or the
    /// hard-coded default (`false`). Surfaces in the UI so the user
    /// can tell at a glance whether the value comes from their
    /// override or the engine.
    pub auto_compact_overridden: bool,
}

#[tauri::command]
pub fn context_status(state: State<'_, Arc<AppState>>) -> ContextStatus {
    let provider = config_get("AICTL_PROVIDER");
    let model = config_get("AICTL_MODEL");
    let model_str = model.clone().unwrap_or_default();
    let limit = llm::context_limit(&model_str);
    let last_input_tokens = state.last_input_tokens.load(Ordering::Relaxed);
    let last_output_tokens = state.last_output_tokens.load(Ordering::Relaxed);
    let messages = state.messages.lock().map(|m| m.len()).unwrap_or(0);

    let token_pct = llm::pct(last_input_tokens, limit);
    let message_pct = llm::pct_usize(messages, MAX_MESSAGES);
    let context_pct = token_pct.max(message_pct).min(100);

    ContextStatus {
        model,
        provider,
        last_input_tokens,
        last_output_tokens,
        context_limit: limit,
        messages,
        max_messages: MAX_MESSAGES,
        token_pct,
        message_pct,
        context_pct,
        auto_compact_threshold: auto_compact_threshold(),
        auto_compact_overridden: config_get("AICTL_AUTO_COMPACT_THRESHOLD")
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|v| (1..=100).contains(v))
            .is_some(),
    }
}
