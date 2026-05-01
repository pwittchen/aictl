//! Session-management Tauri commands. v1 surfaces just the operations
//! the chat sidebar needs: list, load (no-op until full session
//! rehydration lands), delete, and "new incognito". Rename and
//! cross-session search are deferred to Phase 3.

use std::sync::Arc;

use aictl_core::session;
use serde::Serialize;
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
}

#[tauri::command]
pub fn list_sessions() -> Vec<SessionRow> {
    session::list_sessions()
        .into_iter()
        .map(|e| SessionRow {
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

/// Load (i.e. select) a session as the active one. The chat history is
/// rehydrated lazily on the next `send_message` call when Phase 3 wires
/// `session::load` through `chat::run_turn`.
#[tauri::command]
pub fn load_session(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    let resolved = session::resolve(&id).ok_or_else(|| format!("no session matching '{id}'"))?;
    *state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())? = Some(resolved.clone());
    Ok(resolved)
}

#[tauri::command]
pub fn delete_session(id: String) -> Result<(), String> {
    session::delete_session(&id);
    Ok(())
}

/// Switch the desktop into incognito mode for the next turn. Sessions
/// state is cleared so no history persists; the engine's
/// `session::is_incognito` toggle is tripped via the same `set_incognito`
/// helper the CLI uses (added in Phase 3).
#[tauri::command]
pub fn new_incognito_session(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    *state
        .session_id
        .lock()
        .map_err(|_| "session-id mutex poisoned".to_string())? = None;
    Ok(())
}
