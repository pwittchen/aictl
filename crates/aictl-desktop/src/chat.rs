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
use aictl_core::session;
use aictl_core::skills;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use crate::ui::DesktopUI;

pub struct ChatRequest {
    pub user_message: String,
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

    // Build the working transcript. Three cases:
    //   * Fresh session — no id, no in-memory history. Mint an id (unless
    //     incognito), seed the system prompt, set it as `current` so
    //     `session::save_current` from inside the engine (e.g. compaction)
    //     lands on the right file.
    //   * Existing session, in-memory cache populated — reuse as-is.
    //   * Existing session, cold cache (process restart, sidebar load) —
    //     `commands::sessions::load_session` already rehydrated
    //     `state.messages`; we just borrow it.
    //
    // The agent loop appends messages in place. After the turn we save
    // back through `session::save_messages` so the next turn (or the
    // next process invocation) sees the same transcript.
    let session_id = ensure_session(&state);
    let mut messages = take_messages(&state);
    if messages.is_empty() {
        messages.push(Message {
            role: Role::System,
            content: run::build_system_prompt(),
            images: vec![],
        });
    }
    let mut auto = req.auto_accept;

    // Re-resolve the loaded skill from disk every turn. The picker only
    // stores a name, so on-disk edits to the SKILL.md show up without a
    // desktop restart, and a missing skill (deleted out from under us)
    // surfaces as a warning rather than silently sticking with stale
    // state.
    let turn_skill = {
        let slot = state
            .loaded_skill
            .lock()
            .expect("loaded-skill mutex poisoned");
        slot.clone()
    };
    let resolved_skill = turn_skill.as_deref().and_then(|name| {
        if let Some(s) = skills::find(name) {
            Some(s)
        } else {
            ui.emit_warning(&format!(
                "loaded skill '{name}' was not found on disk — running without it"
            ));
            None
        }
    });

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
        resolved_skill.as_ref(),
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

    // Whether the turn succeeded or was cancelled, persist whatever the
    // agent loop appended so the user doesn't lose mid-flight progress
    // (a tool-approved file edit still happened on disk regardless of
    // whether the assistant's reply finished). Skip when incognito.
    if let Some(id) = session_id.as_deref()
        && !*state.incognito.lock().expect("incognito mutex poisoned")
    {
        session::save_messages(id, &messages);
    }
    put_messages(&state, messages);

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

/// Make sure `state.session_id` is populated. Returns the active id, or
/// `None` if the user is in incognito mode (in which case nothing is
/// persisted to disk). Mints a fresh UUID when no session is active and
/// not in incognito; mirrors the value into `aictl_core::session::CURRENT`
/// so engine paths that reach for `session::current_id()` (compaction,
/// audit log) see it too.
fn ensure_session(state: &AppState) -> Option<String> {
    let incognito = *state.incognito.lock().expect("incognito mutex poisoned");
    session::set_incognito(incognito);
    if incognito {
        return None;
    }
    let mut slot = state.session_id.lock().expect("session-id mutex poisoned");
    if let Some(id) = slot.as_ref() {
        let name = session::name_for(id);
        session::set_current(id.clone(), name);
        return Some(id.clone());
    }
    let id = session::generate_uuid();
    session::set_current(id.clone(), None);
    *slot = Some(id.clone());
    Some(id)
}

fn take_messages(state: &AppState) -> Vec<Message> {
    std::mem::take(&mut *state.messages.lock().expect("messages mutex poisoned"))
}

fn put_messages(state: &AppState, msgs: Vec<Message>) {
    *state.messages.lock().expect("messages mutex poisoned") = msgs;
}

/// Visible to `commands::chat::compact_chat` so the button shares the
/// same provider/model/key resolution path as `send_message`.
pub(crate) fn resolve_active_provider() -> Result<(Provider, String, String), String> {
    resolve_provider_model_key()
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
