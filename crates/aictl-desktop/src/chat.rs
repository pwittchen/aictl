//! Glue between Tauri commands and `aictl_core::run::run_agent_turn`.
//!
//! `commands::chat::send_message` does almost no work itself — it
//! resolves provider/model/key out of `~/.aictl/config`, builds a
//! [`crate::ui::DesktopUI`], and hands the conversation off to the
//! engine. Everything that has to flow back to the webview goes through
//! the `AgentEvent` stream the UI emits.

use std::sync::Arc;

use aictl_core::error::AictlError;
use aictl_core::keys;
use aictl_core::message::{Message, Role};
use aictl_core::run::{self, MemoryMode, Provider};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use crate::ui::DesktopUI;

pub struct ChatRequest {
    pub user_message: String,
    pub session_id: Option<String>,
    /// When `true`, every tool call in this turn is auto-approved and the
    /// engine emits `show_auto_tool` instead of routing through
    /// `confirm_tool_async`. Set per-send from the composer's checkbox; a
    /// fresh `false` is the default for each turn.
    pub auto_accept: bool,
}

/// Drive a single agent turn for the desktop. Spawned as a tokio task
/// from `commands::chat::send_message` so the IPC handler can return
/// immediately while the engine runs.
pub async fn run_turn(
    app: AppHandle,
    state: Arc<AppState>,
    cancel: CancellationToken,
    req: ChatRequest,
) {
    let ui = DesktopUI::new(app, state.clone());

    let (provider, model, api_key) = match resolve_provider_model_key() {
        Ok(t) => t,
        Err(reason) => {
            ui.emit_error(&reason);
            return;
        }
    };

    // Conversation history. v1 does not persist sessions on the desktop
    // (deferred to Phase 3) — each turn starts fresh, seeded with the
    // system prompt at index 0. `run_agent_turn` does not prepend it
    // itself; without this seed the model never sees the tool catalog
    // and `windowed_messages` (ShortTerm memory) blows past `messages[0]`.
    let mut messages: Vec<Message> = vec![Message {
        role: Role::System,
        content: run::build_system_prompt(),
        images: vec![],
    }];
    let mut auto = req.auto_accept;

    let turn = run::run_agent_turn(
        &provider,
        &api_key,
        &model,
        &mut messages,
        &req.user_message,
        &mut auto,
        &ui,
        MemoryMode::ShortTerm,
        // Streaming is on for the desktop unconditionally — the
        // webview consumes `StreamChunk` events and renders progressive
        // markdown. The CLI's `--quiet` / non-TTY auto-disable doesn't
        // apply here.
        true,
        None,
    );

    // Race the turn against the per-turn cancellation token. `stop_turn`
    // cancels the token; the engine itself does not poll cooperatively
    // yet, so the abort surfaces by dropping the future. (Phase 3 will
    // weave the token through `with_esc_cancel` for true cooperative
    // cancellation; for v1 the drop is enough to clear UI state.)
    let outcome = tokio::select! {
        result = turn => Some(result),
        () = cancel.cancelled() => None,
    };

    let _ = req.session_id; // session persistence: Phase 3.

    // Clear the in-flight cancel slot regardless of how the turn ended.
    if let Ok(mut slot) = state.turn_cancel.lock() {
        *slot = None;
    }

    match outcome {
        Some(Ok(_)) => {}
        Some(Err(e)) => ui.emit_error(&format_err(&e)),
        None => ui.emit_warning("turn cancelled"),
    }
}

fn resolve_provider_model_key() -> Result<(Provider, String, String), String> {
    let provider = parse_provider(
        &aictl_core::config::config_get("AICTL_PROVIDER").ok_or_else(|| {
            "no provider configured — set AICTL_PROVIDER in ~/.aictl/config (Settings → Provider)"
                .to_string()
        })?,
    )?;
    let model = aictl_core::config::config_get("AICTL_MODEL").ok_or_else(|| {
        "no model configured — set AICTL_MODEL in ~/.aictl/config (Settings → Provider)".to_string()
    })?;
    let api_key = api_key_for(&provider);
    Ok((provider, model, api_key))
}

fn parse_provider(raw: &str) -> Result<Provider, String> {
    match raw.trim() {
        "openai" => Ok(Provider::Openai),
        "anthropic" => Ok(Provider::Anthropic),
        "gemini" => Ok(Provider::Gemini),
        "grok" => Ok(Provider::Grok),
        "mistral" => Ok(Provider::Mistral),
        "deepseek" => Ok(Provider::Deepseek),
        "kimi" => Ok(Provider::Kimi),
        "zai" => Ok(Provider::Zai),
        "ollama" => Ok(Provider::Ollama),
        "gguf" => Ok(Provider::Gguf),
        "mlx" => Ok(Provider::Mlx),
        "aictl-server" => Ok(Provider::AictlServer),
        other => Err(format!("unrecognized provider '{other}'")),
    }
}

/// Mirror of the CLI's `resolve_api_key` — local providers and the
/// aictl-server proxy need no per-provider key, everything else maps to
/// a `LLM_<NAME>_API_KEY` lookup through the keyring/plain-config
/// fallback in [`keys::get_secret`].
fn api_key_for(provider: &Provider) -> String {
    if provider.is_local() || matches!(provider, Provider::AictlServer | Provider::Mock) {
        return String::new();
    }
    let key_name = match provider {
        Provider::Openai => "LLM_OPENAI_API_KEY",
        Provider::Anthropic => "LLM_ANTHROPIC_API_KEY",
        Provider::Gemini => "LLM_GEMINI_API_KEY",
        Provider::Grok => "LLM_GROK_API_KEY",
        Provider::Mistral => "LLM_MISTRAL_API_KEY",
        Provider::Deepseek => "LLM_DEEPSEEK_API_KEY",
        Provider::Kimi => "LLM_KIMI_API_KEY",
        Provider::Zai => "LLM_ZAI_API_KEY",
        // The early-return covers these, but keeping the arm explicit
        // makes the matrix exhaustive so a new provider variant fails to
        // compile until its key plumbing is added here.
        Provider::Ollama
        | Provider::Gguf
        | Provider::Mlx
        | Provider::Mock
        | Provider::AictlServer => return String::new(),
    };
    keys::get_secret(key_name).unwrap_or_default()
}

fn format_err(e: &AictlError) -> String {
    e.to_string()
}
