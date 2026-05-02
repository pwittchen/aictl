//! Plugins-pane Tauri commands.

use aictl_core::plugins;
use serde::Serialize;

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
    PluginsStatus {
        enabled: plugins::enabled(),
        plugins_dir: plugins::plugins_dir().display().to_string(),
        plugins: plugins::list()
            .iter()
            .map(|p| PluginRow {
                name: p.name.clone(),
                description: p.description.clone(),
                entrypoint: p.entrypoint.display().to_string(),
                requires_confirmation: p.requires_confirmation,
                timeout_secs: p.timeout_secs,
            })
            .collect(),
    }
}
