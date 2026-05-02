//! Misc system commands — version, "Reveal in Finder" entries.

use std::path::PathBuf;

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn reveal_audit_log() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let path = PathBuf::from(home).join(".aictl/audit");
    if !path.exists() {
        return Err(format!(
            "audit log directory '{}' does not exist yet",
            path.display()
        ));
    }
    Ok(path)
}

#[tauri::command]
pub fn reveal_config_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(home).join(".aictl"))
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
