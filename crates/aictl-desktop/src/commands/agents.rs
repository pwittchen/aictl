//! Agents-pane Tauri commands.

use std::sync::Arc;

use aictl_core::agents;
use aictl_core::agents::remote;
use aictl_core::message::Role;
use aictl_core::run;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

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
    let raw = std::fs::read_to_string(&entry.path).map_err(|e| format!("read agent file: {e}"))?;
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

#[derive(Deserialize)]
pub struct AgentLoadArgs {
    pub name: String,
}

/// Pin `name` as the active agent. The agent body is stored in
/// `aictl_core::agents::LOADED_AGENT` (a process-wide static) and
/// concatenated into the system prompt by [`run::build_system_prompt`].
/// To pick up the new prompt for a transcript that's already running,
/// rebuild `messages[0]` in place — same recipe the CLI's
/// `load_agent_by_name` uses.
#[tauri::command]
pub fn agent_load(state: State<'_, Arc<AppState>>, args: AgentLoadArgs) -> Result<(), String> {
    if !agents::is_valid_name(&args.name) {
        return Err(format!("invalid agent name '{}'", args.name));
    }
    let prompt =
        agents::read_agent(&args.name).map_err(|_| format!("agent '{}' not found", args.name))?;
    agents::load_agent(&args.name, &prompt);
    rebuild_system_prompt(&state)?;
    Ok(())
}

/// Drop the currently-loaded agent. Idempotent — succeeds even when no
/// agent is loaded so the picker doesn't have to special-case the empty
/// state on its end.
#[tauri::command]
pub fn agent_unload(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    agents::unload_agent();
    rebuild_system_prompt(&state)?;
    Ok(())
}

/// Read the currently-loaded agent name (`None` when no agent is
/// loaded). The webview calls this on mount so the picker icon's
/// highlight state reflects whatever the engine global already holds.
#[tauri::command]
pub fn agent_loaded() -> Option<String> {
    agents::loaded_agent_name()
}

/// Mirror of `RemoteSkillRow` for the agents catalogue.
#[derive(Serialize)]
pub struct RemoteAgentRow {
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub state: String,
}

#[tauri::command]
pub async fn agents_list_remote() -> Result<Vec<RemoteAgentRow>, String> {
    let entries = remote::list_agents().await?;
    Ok(entries
        .into_iter()
        .map(|a| RemoteAgentRow {
            state: state_label(a.state),
            name: a.name,
            description: a.description,
            category: a.category,
        })
        .collect())
}

#[derive(Deserialize)]
pub struct AgentPullArgs {
    pub name: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[tauri::command]
pub async fn agent_pull(args: AgentPullArgs) -> Result<String, String> {
    let outcome = remote::pull(&args.name, || args.overwrite).await?;
    Ok(match outcome {
        remote::PullOutcome::Installed => "installed".to_string(),
        remote::PullOutcome::Overwritten => "overwritten".to_string(),
        remote::PullOutcome::SkippedExisting => "skipped".to_string(),
    })
}

fn state_label(state: remote::State) -> String {
    match state {
        remote::State::NotPulled => "not_pulled".to_string(),
        remote::State::UpToDate => "up_to_date".to_string(),
        remote::State::UpstreamNewer => "upstream_newer".to_string(),
    }
}

fn rebuild_system_prompt(state: &AppState) -> Result<(), String> {
    let mut msgs = state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())?;
    if let Some(first) = msgs.first_mut()
        && matches!(first.role, Role::System)
    {
        first.content = run::build_system_prompt();
    }
    Ok(())
}
