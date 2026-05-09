//! Plugins-pane Tauri commands.
//!
//! Mirrors the CLI's `/plugins` menu plus authoring: list installed
//! plugins, save a new one (manifest + entrypoint script), and delete
//! the catalogue entry. `aictl_core::plugins::save_plugin` /
//! `delete_plugin` already do the heavy lifting (validation, atomic
//! directory rewrite, `reload`) — these wrappers only translate
//! `serde`-friendly arguments and shape the response.

use aictl_core::plugins;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct PluginRow {
    pub name: String,
    pub description: String,
    pub entrypoint: String,
    pub requires_confirmation: bool,
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize)]
pub struct PluginsStatus {
    pub enabled: bool,
    pub plugins_dir: String,
    pub plugins: Vec<PluginRow>,
}

#[tauri::command]
pub fn plugins_status() -> PluginsStatus {
    // `scan_disk` rather than `list` so a plugin authored while the
    // subsystem is still toggled off still shows up in the table —
    // the agent loop continues to honor `enabled()` separately.
    PluginsStatus {
        enabled: plugins::enabled(),
        plugins_dir: plugins::plugins_dir().display().to_string(),
        plugins: plugins::scan_disk()
            .into_iter()
            .map(|p| PluginRow {
                name: p.name,
                description: p.description,
                entrypoint: p.entrypoint.display().to_string(),
                requires_confirmation: p.requires_confirmation,
                timeout_secs: p.timeout_secs,
            })
            .collect(),
    }
}

#[derive(Deserialize)]
pub struct PluginSaveArgs {
    pub name: String,
    pub description: String,
    /// Body of the entrypoint script. Persisted verbatim into
    /// `<plugin-dir>/run`; on Unix the file is then chmod 755.
    pub body: String,
    #[serde(default = "default_requires_confirmation")]
    pub requires_confirmation: bool,
    pub timeout_secs: Option<u64>,
    /// `true` once the user has confirmed they want to clobber an
    /// existing plugin of the same name.
    #[serde(default)]
    pub overwrite: bool,
}

fn default_requires_confirmation() -> bool {
    true
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSaveOutcome {
    Installed,
    Overwritten,
}

/// Persist a new (or rewritten) plugin and refresh the in-memory
/// catalogue. The webview maps the outcome variant to a toast colour.
#[tauri::command]
pub fn plugin_save(args: PluginSaveArgs) -> Result<PluginSaveOutcome, String> {
    let trimmed = args.name.trim();
    let dir_existed = plugins::plugins_dir().join(trimmed).is_dir();
    if dir_existed && !args.overwrite {
        return Err(format!("plugin '{trimmed}' already exists"));
    }
    plugins::save_plugin(
        &args.name,
        &args.description,
        &args.body,
        args.requires_confirmation,
        args.timeout_secs,
        args.overwrite,
    )?;
    Ok(if dir_existed {
        PluginSaveOutcome::Overwritten
    } else {
        PluginSaveOutcome::Installed
    })
}

#[derive(Deserialize)]
pub struct PluginDeleteArgs {
    pub name: String,
}

#[tauri::command]
pub fn plugin_delete(args: PluginDeleteArgs) -> Result<(), String> {
    plugins::delete_plugin(&args.name)
}

/// Re-walk the plugins directory and refresh the in-memory catalogue.
/// The composer's plugins toggle calls this right after flipping
/// `AICTL_PLUGINS_ENABLED` so the change takes effect on the next
/// agent turn instead of waiting for an app restart.
#[tauri::command]
pub fn plugins_reload() {
    plugins::reload();
}
