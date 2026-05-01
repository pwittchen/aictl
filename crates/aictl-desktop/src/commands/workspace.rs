//! Workspace picker — `AICTL_WORKING_DIR_DESKTOP` lifecycle.
//!
//! These three commands implement the onboarding flow described in plan
//! §5.4 and §7.1: read the current pinned workspace, persist a freshly
//! picked path, and open the native folder picker. The webview chains
//! `pick_workspace` → `set_workspace` so the new value lands in
//! `~/.aictl/config` and the security policy reads it on the next turn.

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{ui, workspace};

#[derive(Serialize)]
pub struct WorkspaceState {
    pub path: Option<String>,
    /// `true` when [`workspace::resolve`] returned an error — the
    /// configured path went stale (deleted or replaced by a file).
    /// Frontend uses this to nudge the user back to the picker.
    pub stale: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn get_workspace() -> WorkspaceState {
    match workspace::resolve() {
        Ok(Some(path)) => WorkspaceState {
            path: Some(path.to_string_lossy().into_owned()),
            stale: false,
            error: None,
        },
        Ok(None) => WorkspaceState {
            path: None,
            stale: false,
            error: None,
        },
        Err(reason) => WorkspaceState {
            path: None,
            stale: true,
            error: Some(reason),
        },
    }
}

#[tauri::command]
pub fn set_workspace(app: AppHandle, path: String) -> Result<WorkspaceState, String> {
    let canonical = workspace::set(&path)?;
    let path_str = canonical.to_string_lossy().into_owned();
    ui::emit_workspace_changed(&app, Some(&path_str));
    Ok(WorkspaceState {
        path: Some(path_str),
        stale: false,
        error: None,
    })
}

/// Open the native folder picker. Returns the chosen path (or `None` if
/// the user cancelled). The webview is responsible for calling
/// [`set_workspace`] with the result.
#[tauri::command]
pub async fn pick_workspace(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose a workspace folder")
        .pick_folder(move |path| {
            let chosen = path.and_then(|p| p.into_path().ok());
            let _ = tx.send(chosen.map(|p| p.to_string_lossy().into_owned()));
        });
    rx.await
        .map_err(|_| "folder picker dropped before responding".to_string())
}
