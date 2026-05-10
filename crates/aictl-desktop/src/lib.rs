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
pub mod fs_watcher;
pub mod state;
pub mod ui;
#[cfg(feature = "voice")]
pub mod voice;
pub mod workspace;

use aictl_core::config::{self, Role};
use tauri::{Manager, RunEvent};

/// Boot the Tauri application. Called from `main.rs`; lives in the lib
/// so integration tests and benches can reuse the wiring.
#[allow(clippy::too_many_lines)]
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

    // Anchor the process cwd to the configured workspace, mirroring the
    // CLI's `apply_cwd_override`. Tools that touch the filesystem with
    // bare relative paths (`tool_generate_image::save_image`,
    // `tool_read_image`, the file-system tools, etc.) operate relative
    // to the process cwd; without this, the desktop saves images into
    // the launch dir while `read_workspace_image` looks for them under
    // the configured workspace, and previews can't find the file.
    if let Ok(Some(ws)) = workspace::resolve()
        && let Err(err) = std::env::set_current_dir(&ws)
    {
        eprintln!(
            "[aictl-desktop] failed to chdir to workspace '{}': {err}",
            ws.display()
        );
    }

    // Spawn configured MCP servers up-front, mirroring the CLI. `init_with`
    // is idempotent and a no-op when `AICTL_MCP_ENABLED` is unset, so this
    // costs nothing for users who haven't opted in. We run it before the
    // Tauri builder boots so the catalogue is ready by the time the first
    // agent turn fires; per-server failures land in `ServerState::Failed`
    // and are surfaced via `mcp_status` rather than blocking startup.
    tauri::async_runtime::block_on(aictl_core::mcp::init_with(None));

    // Discover user-installed plugins for the same reason — the catalogue
    // gets injected into the system prompt and surfaced in the Plugins
    // tab. Gated behind `AICTL_PLUGINS_ENABLED` inside `init`.
    aictl_core::plugins::init();

    // Reap any `~/.aictl/models/whisper/*.part` files left behind by a download
    // that was killed mid-stream (app force-quit, OS reboot, etc.). The
    // download path overwrites with `O_TRUNC` so leftovers are not
    // strictly harmful, but a 140 MB partial sitting forever after a
    // single failed launch is noisy on disk.
    #[cfg(feature = "voice")]
    voice::cleanup_partial_downloads();

    let app_state = std::sync::Arc::new(state::AppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION,
                )
                .build(),
        )
        .setup({
            let state = app_state.clone();
            move |app| {
                app.manage(state.clone());
                ui::install_warning_sink(app.handle().clone());
                // Start the workspace fs watcher (best-effort) so the
                // file pane and editor refresh when anything inside the
                // workspace changes — the assistant's tool calls,
                // external editors, git checkouts, all of it.
                if let Ok(Some(ws)) = workspace::resolve() {
                    fs_watcher::install(app.handle(), &state, &ws);
                }
                // Traffic-light positioning is owned by Tauri's
                // `trafficLightPosition` (in `tauri.conf.json`), which
                // routes through tao's `set_traffic_light_inset` and
                // its content-view `drawRect:` hook. That hook runs the
                // inset synchronously inside every redraw, including
                // each tick of a live drag-resize, so the buttons
                // stay put. We previously had a manual apply path here
                // mirroring tao's algorithm; it was the source of a
                // visible "snap" on first paint and a flicker on the
                // deferred ladder, because our apply set the button
                // y-origin while tao's leaves it at AppKit's default
                // inside the resized title bar — the two values
                // disagreed and fought each other on every redraw.
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::system::version,
            commands::system::build_profile,
            commands::system::build_time,
            commands::system::build_commit,
            commands::system::reveal_audit_log,
            commands::system::reveal_config_dir,
            commands::system::open_url,
            commands::workspace::get_workspace,
            commands::workspace::set_workspace,
            commands::workspace::pick_workspace,
            commands::workspace::use_default_workspace,
            commands::workspace::default_workspace_path,
            commands::chat::send_message,
            commands::chat::stop_turn,
            commands::chat::tool_approval_response,
            commands::chat::clear_chat,
            commands::chat::retry_last,
            commands::chat::undo_last,
            commands::sessions::list_sessions,
            commands::sessions::load_session,
            commands::sessions::delete_session,
            commands::sessions::clear_sessions,
            commands::sessions::rename_session,
            commands::sessions::new_session,
            commands::sessions::new_incognito_session,
            commands::sessions::get_active_session,
            commands::models::list_models,
            commands::models::get_active_model,
            commands::models::set_active_model,
            commands::settings::config_dump,
            commands::settings::config_value,
            commands::settings::config_write,
            commands::settings::config_clear,
            commands::settings::keys_status,
            commands::settings::keys_backend,
            commands::settings::keys_set,
            commands::settings::keys_clear,
            commands::settings::keys_lock,
            commands::settings::keys_unlock,
            commands::settings::keys_lock_all,
            commands::settings::keys_unlock_all,
            commands::settings::tools_list,
            commands::settings::tool_set_disabled,
            commands::chat::compact_chat,
            commands::images::read_workspace_image,
            commands::files::workspace_tree,
            commands::files::workspace_read_file,
            commands::files::workspace_write_file,
            commands::files::workspace_delete,
            commands::files::workspace_create_file,
            commands::files::workspace_create_dir,
            commands::files::workspace_rename,
            commands::files::workspace_upload_file,
            commands::mcp::mcp_status,
            commands::mcp::mcp_toggle,
            commands::mcp::mcp_create,
            commands::mcp::mcp_reload,
            commands::hooks::hooks_status,
            commands::hooks::hook_toggle,
            commands::hooks::hook_delete,
            commands::hooks::hook_create,
            commands::skills::skills_list,
            commands::skills::skill_delete,
            commands::skills::skill_view,
            commands::skills::skill_load,
            commands::skills::skill_unload,
            commands::skills::skill_loaded,
            commands::skills::skills_list_remote,
            commands::skills::skill_pull,
            commands::skills::skill_save,
            commands::skills::skill_generate,
            commands::agents::agents_list,
            commands::agents::agent_delete,
            commands::agents::agent_view,
            commands::agents::agent_load,
            commands::agents::agent_unload,
            commands::agents::agent_loaded,
            commands::agents::agents_list_remote,
            commands::agents::agent_pull,
            commands::agents::agent_save,
            commands::agents::agent_generate,
            commands::plugins::plugins_status,
            commands::plugins::plugin_save,
            commands::plugins::plugin_delete,
            commands::plugins::plugins_reload,
            commands::stats::stats_snapshot,
            commands::stats::stats_clear,
            commands::stats::stats_daily,
            commands::server::server_status,
            commands::server::server_probe,
            commands::server::ollama_status,
            commands::server::ollama_probe,
            commands::context::context_status,
            commands::memory::memory_status,
            commands::memory::memory_set_enabled,
            commands::memory::memory_add,
            commands::memory::memory_remove,
            commands::memory::memory_clear,
            commands::local_models::local_models_status,
            commands::local_models::local_models_pull_gguf,
            commands::local_models::local_models_pull_mlx,
            commands::local_models::local_models_remove_gguf,
            commands::local_models::local_models_remove_mlx,
            commands::local_models::local_models_clear_gguf,
            commands::local_models::local_models_clear_mlx,
            commands::ner::ner_status,
            commands::ner::ner_pull,
            commands::ner::ner_remove,
            commands::voice::voice_status,
            commands::voice::voice_ensure_model,
            commands::voice::voice_transcribe,
            commands::voice::voice_cancel_download,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build aictl-desktop")
        .run(|_handle, event| {
            // Mirror the CLI's MCP cleanup on every exit path. Without
            // this, child MCP processes spawned during the session
            // would survive the desktop quitting.
            if let RunEvent::Exit = event {
                tauri::async_runtime::block_on(aictl_core::mcp::shutdown());
                // Drop the whisper context and any in-flight download
                // *before* the macOS runtime tears down — whisper.cpp's
                // Metal teardown path crashes if it runs after
                // `NSApplication` has finalised, which is what surfaces
                // as "aictl-desktop unexpectedly quit" after a clean
                // ⌘Q. Doing it here, on the main thread, while Cocoa is
                // still live, sidesteps the issue.
                #[cfg(feature = "voice")]
                voice::shutdown();
                // Skip the remaining static destructors. After this
                // hook returns, Tauri's runtime has nothing left to do
                // — every native resource we own is already released.
                // Any third-party crate with a flaky `Drop` (Metal
                // command queues, GPU caches, etc.) running at full
                // teardown would otherwise risk turning a clean quit
                // into an OS-level crash dialog.
                std::process::exit(0);
            }
        });
}
