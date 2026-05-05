//! Misc system commands — version, "Reveal in Finder" entries.

use std::path::PathBuf;

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Reports whether the running binary was compiled in debug or release
/// mode. The About tab shows this so a developer running a local build
/// can tell at a glance which artifact is loaded — useful when a signed
/// release and a `cargo run` build coexist on the same machine.
#[tauri::command]
pub fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Unix epoch seconds captured at compile time by `build.rs`. The
/// frontend formats it with `Date(...)` so the user sees their local
/// timezone.
#[tauri::command]
pub fn build_time() -> &'static str {
    env!("AICTL_BUILD_TIME")
}

/// Short git hash captured at compile time by `build.rs`, or
/// `"unknown"` when the build happened outside a git checkout.
#[tauri::command]
pub fn build_commit() -> &'static str {
    env!("AICTL_BUILD_COMMIT")
}

#[tauri::command]
pub fn reveal_audit_log(app: AppHandle) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let path = PathBuf::from(home).join(".aictl/audit");
    if !path.exists() {
        return Err(format!(
            "audit log directory '{}' does not exist yet",
            path.display()
        ));
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("failed to reveal {}: {e}", path.display()))
}

#[tauri::command]
pub fn reveal_config_dir(app: AppHandle) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let path = PathBuf::from(home).join(".aictl");
    if !path.exists() {
        return Err(format!(
            "config directory '{}' does not exist yet",
            path.display()
        ));
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("failed to reveal {}: {e}", path.display()))
}

/// Open a URL in the user's default browser instead of navigating the
/// webview. Called from the chat surface's delegated `<a>` click handler
/// so markdown links in agent responses behave like links anywhere else
/// in macOS. Restricted to `http(s)://` and `mailto:` so a hostile agent
/// can't smuggle a `file://` or custom-scheme handler through the chat.
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let allowed = trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:");
    if !allowed {
        return Err(format!(
            "refusing to open url with disallowed scheme: {url}"
        ));
    }
    app.opener()
        .open_url(trimmed, None::<&str>)
        .map_err(|e| format!("failed to open url: {e}"))
}
