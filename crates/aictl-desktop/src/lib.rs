//! `aictl-desktop` — native frontend for `aictl`.
//!
//! Mirrors the CLI behaviour on top of the engine in `aictl-core` and
//! surfaces it through a Tauri v2 webview. macOS-only at this stage —
//! every entry point in this crate is `#[cfg(target_os = "macos")]`.
//! See [`.claude/plans/desktop-app.md`] in the repo root for the full
//! design.

#![cfg(target_os = "macos")]
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    // `#[tauri::command]` handlers must take owned `tauri::State` /
    // `tauri::AppHandle` — clippy's `needless_pass_by_value` would
    // demand `&` references that the IPC macro doesn't accept.
    clippy::needless_pass_by_value
)]

pub mod chat;
pub mod commands;
pub mod state;
pub mod ui;
pub mod workspace;

use aictl_core::config::{self, Role};
use tauri::{Manager, RunEvent};

/// Boot the Tauri application. Called from `main.rs`; lives in the lib
/// so integration tests and benches can reuse the wiring.
pub fn run() {
    // Engine bootstrap — same order the CLI / server use: load config
    // first so `set_role` / `security::init` see persisted keys.
    if let Err(err) = config::load_config() {
        eprintln!("[aictl-desktop] failed to load ~/.aictl/config: {err}");
    }
    config::set_role(Role::Desktop);

    // The desktop never runs in `--unrestricted` mode by default. The
    // sentinel CWD jail (see plan §5.4) means tools are inert until the
    // user picks a workspace; flipping unrestricted is a deliberate
    // power-user toggle that lives in Settings (deferred to Phase 5).
    let _redaction_warnings = aictl_core::security::init(false);

    let app_state = std::sync::Arc::new(state::AppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_shell::init())
        .setup({
            let state = app_state.clone();
            move |app| {
                app.manage(state.clone());
                ui::install_warning_sink(app.handle().clone());
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::system::version,
            commands::system::reveal_audit_log,
            commands::system::reveal_config_dir,
            commands::workspace::get_workspace,
            commands::workspace::set_workspace,
            commands::workspace::pick_workspace,
            commands::chat::send_message,
            commands::chat::stop_turn,
            commands::chat::tool_approval_response,
            commands::sessions::list_sessions,
            commands::sessions::load_session,
            commands::sessions::delete_session,
            commands::sessions::new_incognito_session,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build aictl-desktop")
        .run(|_handle, event| {
            // Mirror the CLI's MCP cleanup on every exit path. Without
            // this, child MCP processes spawned during the session
            // would survive the desktop quitting.
            if let RunEvent::Exit = event {
                tauri::async_runtime::block_on(aictl_core::mcp::shutdown());
            }
        });
}
