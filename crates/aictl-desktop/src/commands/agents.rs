//! Agents-pane Tauri commands.

use aictl_core::agents;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct AgentRow {
    pub name: String,
    pub description: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub origin: String,
    pub official: bool,
    pub path: String,
}

#[tauri::command]
pub fn agents_list() -> Vec<AgentRow> {
    agents::list_agents()
        .into_iter()
        .map(|e| AgentRow {
            official: e.is_official(),
            name: e.name,
            description: e.description,
            source: e.source,
            category: e.category,
            origin: e.origin.label().to_string(),
            path: e.path.display().to_string(),
        })
        .collect()
}

#[derive(Deserialize)]
pub struct AgentDeleteArgs {
    pub name: String,
    pub origin: String,
}

#[tauri::command]
pub fn agent_delete(args: AgentDeleteArgs) -> Result<(), String> {
    let entries = agents::list_agents();
    let entry = entries
        .iter()
        .find(|e| e.name == args.name && e.origin.label() == args.origin)
        .ok_or_else(|| format!("agent '{}' ({}) not found", args.name, args.origin))?;
    agents::delete_agent_entry(entry).map_err(|e| format!("delete: {e}"))
}

#[derive(Serialize)]
pub struct AgentView {
    pub name: String,
    pub description: Option<String>,
    pub origin: String,
    pub path: String,
    pub raw: String,
    pub body: String,
}

/// Read the agent file for the listing entry. Returns both the raw
/// frontmatter+body string (so a "show source" view can render it)
/// and the parsed body for the markdown view.
#[tauri::command]
pub fn agent_view(args: AgentDeleteArgs) -> Result<AgentView, String> {
    let entries = agents::list_agents();
    let entry = entries
        .iter()
        .find(|e| e.name == args.name && e.origin.label() == args.origin)
        .ok_or_else(|| format!("agent '{}' ({}) not found", args.name, args.origin))?;
    let raw =
        std::fs::read_to_string(&entry.path).map_err(|e| format!("read agent file: {e}"))?;
    let meta = agents::parse(&raw);
    Ok(AgentView {
        name: entry.name.clone(),
        description: entry.description.clone(),
        origin: entry.origin.label().to_string(),
        path: entry.path.display().to_string(),
        raw,
        body: meta.body,
    })
}
