//! Misc system commands — version, "Reveal in Finder" entries.

use std::path::PathBuf;

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
