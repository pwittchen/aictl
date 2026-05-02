//! Session-management Tauri commands.
//!
//! The desktop's sidebar uses these to populate the session list, switch
//! the active conversation, rename / delete existing entries, and start a
//! fresh incognito turn. State that has to survive across IPC calls
//! (active session id, in-memory transcript, incognito flag) lives in
//! [`AppState`]; the engine's own `aictl_core::session` module owns the
//! on-disk format and is mirrored into [`AppState::messages`] when a
//! session is loaded.

use std::sync::Arc;

use aictl_core::session;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

#[derive(Serialize)]
pub struct SessionRow {
    pub id: String,
    /// Friendly name set via `aictl_core::session::set_name`. Sessions
    /// that were never named expose just their id; the webview renders
    /// the id with a "(unnamed)" suffix in that case.
    pub name: Option<String>,
    pub size: u64,
    /// Seconds since UNIX epoch — easier for the webview than the
    /// engine's `SystemTime`.
    pub modified_secs: u64,
    /// `true` when this row is the session currently loaded in the chat
    /// surface. Drives the "active" highlight in the sidebar without
    /// requiring a separate `get_active_session` round-trip.
    pub active: bool,
}

/// Lightweight projection of a [`Message`] that the webview can render
/// without depending on the engine's role enum or image attachments.
/// Tool-result envelopes are surfaced as `kind: "tool_result"` so the
/// chat can render them as callouts; everything else falls back to a
/// plain user/assistant body.
#[derive(Serialize)]
pub struct LoadedMessage {
    pub kind: String,
    pub text: String,
}

#[derive(Serialize)]
pub struct LoadSessionResult {
    pub id: String,
    pub name: Option<String>,
    pub messages: Vec<LoadedMessage>,
}

#[derive(Serialize)]
pub struct ActiveSession {
    pub id: Option<String>,
    pub name: Option<String>,
    pub incognito: bool,
}

#[tauri::command]
pub fn list_sessions(state: State<'_, Arc<AppState>>) -> Vec<SessionRow> {
    let active = state.session_id.lock().ok().and_then(|s| s.clone());
    session::list_sessions()
        .into_iter()
        .map(|e| SessionRow {
            active: active.as_deref() == Some(e.id.as_str()),
            id: e.id,
            name: e.name,
            size: e.size,
            modified_secs: e
                .mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        })
        .collect()
}

/// Switch the active session. Resolves the requested id (also accepts
/// friendly names), rehydrates `AppState::messages` from disk, mirrors
/// the id into `aictl_core::session::CURRENT`, and returns a webview-
/// friendly projection of the transcript so the chat surface can render
/// it without a second round-trip.
#[tauri::command]
pub fn load_session(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<LoadSessionResult, String> {
    let resolved = session::resolve(&id).ok_or_else(|| format!("no session matching '{id}'"))?;
    let messages = session::load_messages(&resolved).map_err(|e| format!("load failed: {e}"))?;
    let name = session::name_for(&resolved);
    let projection = messages.iter().map(project_message).collect::<Vec<_>>();

    *state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())? = Some(resolved.clone());
    *state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())? = messages;
    *state
        .incognito
        .lock()
        .map_err(|_| "incognito mutex poisoned".to_string())? = false;
    session::set_incognito(false);
    session::set_current(resolved.clone(), name.clone());

    Ok(LoadSessionResult {
        id: resolved,
        name,
        messages: projection,
    })
}

#[tauri::command]
pub fn delete_session(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    session::delete_session(&id);
    let mut slot = state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())?;
    if slot.as_deref() == Some(id.as_str()) {
        slot.take();
        *state
            .messages
            .lock()
            .map_err(|_| "messages mutex poisoned".to_string())? = Vec::new();
    }
    Ok(())
}

#[tauri::command]
pub fn clear_sessions(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    session::clear_all();
    *state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())? = None;
    *state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())? = Vec::new();
    Ok(())
}

#[derive(Deserialize)]
pub struct RenameArgs {
    pub id: String,
    pub name: String,
}

#[tauri::command]
pub fn rename_session(args: RenameArgs) -> Result<(), String> {
    session::set_name(&args.id, &args.name)
}

/// Start a fresh, *non-incognito* session. The webview calls this when
/// the user clicks "New session" in the sidebar — clears the in-memory
/// transcript so the next `send_message` mints a new UUID via
/// [`crate::chat::run_turn`].
#[tauri::command]
pub fn new_session(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    *state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())? = None;
    *state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())? = Vec::new();
    *state
        .incognito
        .lock()
        .map_err(|_| "incognito mutex poisoned".to_string())? = false;
    session::set_incognito(false);
    Ok(())
}

/// Switch the desktop into incognito mode for the next turn. Clears the
/// in-memory transcript and active session id so nothing carries over,
/// then trips `aictl_core::session::set_incognito(true)` to keep
/// `session::save_messages` from writing back.
#[tauri::command]
pub fn new_incognito_session(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    *state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())? = None;
    *state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())? = Vec::new();
    *state
        .incognito
        .lock()
        .map_err(|_| "incognito mutex poisoned".to_string())? = true;
    session::set_incognito(true);
    Ok(())
}

#[tauri::command]
pub fn get_active_session(state: State<'_, Arc<AppState>>) -> ActiveSession {
    let id = state.session_id.lock().ok().and_then(|s| s.clone());
    let name = id.as_deref().and_then(session::name_for);
    let incognito = state.incognito.lock().is_ok_and(|s| *s);
    ActiveSession {
        id,
        name,
        incognito,
    }
}

fn project_message(m: &aictl_core::Message) -> LoadedMessage {
    use aictl_core::Role;
    match m.role {
        Role::System => LoadedMessage {
            kind: "system".to_string(),
            text: m.content.clone(),
        },
        Role::User => {
            let trimmed = m.content.trim_start();
            if trimmed.starts_with("<tool_result>") {
                LoadedMessage {
                    kind: "tool_result".to_string(),
                    text: m.content.clone(),
                }
            } else {
                LoadedMessage {
                    kind: "user".to_string(),
                    text: m.content.clone(),
                }
            }
        }
        Role::Assistant => LoadedMessage {
            kind: "assistant".to_string(),
            text: m.content.clone(),
        },
    }
}
