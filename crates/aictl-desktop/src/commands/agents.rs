//! Agents-pane Tauri commands.

use std::sync::Arc;

use aictl_core::agents;
use aictl_core::agents::remote;
use aictl_core::config;
use aictl_core::error::AictlError;
use aictl_core::llm;
use aictl_core::message::{Message, Role};
use aictl_core::run::{self, Provider};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::chat;
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

#[derive(Deserialize)]
pub struct AgentSaveArgs {
    pub name: String,
    pub body: String,
    /// When `true`, overwrite an existing agent of the same name. The
    /// frontend asks the user to confirm before flipping this on so a
    /// stray click doesn't blow away a tuned prompt.
    #[serde(default)]
    pub overwrite: bool,
}

/// Outcome of `agent_save`. Distinguishing `installed` from `overwritten`
/// lets the picker show the right toast and lets the dialog refuse a
/// blind clobber until the user opts in.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSaveOutcome {
    Installed,
    Overwritten,
}

/// Persist a new (or rewritten) agent to `~/.aictl/agents/<name>.md`.
/// Mirrors the CLI's manual-create path; intentionally does **not**
/// auto-load the agent — picking which agent is active is a separate
/// step (the existing `agent_load` command).
#[tauri::command]
pub fn agent_save(args: AgentSaveArgs) -> Result<AgentSaveOutcome, String> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err("agent name is empty".to_string());
    }
    if !agents::is_valid_name(&name) {
        return Err(
            "invalid name — use only letters, numbers, underscore, or dash".to_string(),
        );
    }
    let body = args.body.trim().to_string();
    if body.is_empty() {
        return Err("agent prompt is empty".to_string());
    }
    let exists = agents::list_agents().into_iter().any(|e| e.name == name);
    if exists && !args.overwrite {
        return Err(format!("agent '{name}' already exists"));
    }
    agents::save_agent(&name, &body).map_err(|e| format!("save agent: {e}"))?;
    Ok(if exists {
        AgentSaveOutcome::Overwritten
    } else {
        AgentSaveOutcome::Installed
    })
}

#[derive(Deserialize)]
pub struct AgentGenerateArgs {
    pub name: String,
    pub description: String,
}

/// Generate an agent system-prompt body from a free-text description
/// using the active provider/model. Mirrors the CLI's
/// `create_agent_with_ai` flow but returns the prompt to the webview
/// for review instead of saving it directly — the user clicks Save in
/// the dialog when they're happy.
#[tauri::command]
pub async fn agent_generate(args: AgentGenerateArgs) -> Result<String, String> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err("agent name is empty".to_string());
    }
    if !agents::is_valid_name(&name) {
        return Err(
            "invalid name — use only letters, numbers, underscore, or dash".to_string(),
        );
    }
    let description = args.description.trim().to_string();
    if description.is_empty() {
        return Err("description is empty".to_string());
    }

    let (provider, model, api_key) = chat::resolve_active_provider()?;

    let messages = vec![
        Message {
            role: Role::System,
            content: "You are an expert at writing system prompts for AI assistants. \
                Generate a clear, detailed system prompt for an AI agent based on the user's \
                description. The prompt should define the agent's role, capabilities, behavior, \
                and constraints. Output ONLY the prompt text, nothing else."
                .to_string(),
            images: vec![],
        },
        Message {
            role: Role::User,
            content: format!(
                "Create a system prompt for an AI agent named \"{name}\" that does the following: {description}"
            ),
            images: vec![],
        },
    ];

    let prompt = call_provider_buffered(&provider, &api_key, &model, &messages).await?;
    Ok(prompt.trim().to_string())
}

/// One-shot, non-streaming dispatch to whichever provider is active.
/// Mirrors the matrix in `run::compact_messages`; kept inline here so
/// the agent-generator can run without spinning up the full agent loop.
async fn call_provider_buffered(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &[Message],
) -> Result<String, String> {
    let llm_timeout = config::llm_timeout();
    let server_route = if matches!(provider, Provider::AictlServer) {
        Some(config::active_server().ok_or_else(|| {
            "provider 'aictl-server' selected but AICTL_CLIENT_HOST and/or AICTL_CLIENT_MASTER_KEY are not configured".to_string()
        })?)
    } else {
        None
    };

    let result = tokio::time::timeout(llm_timeout, async {
        if let Some((url, key)) = server_route.as_ref() {
            return llm::server_proxy::call(url, key, model, messages, None).await;
        }
        match provider {
            Provider::Openai => llm::openai::call_openai(api_key, model, messages, None).await,
            Provider::Anthropic => {
                llm::anthropic::call_anthropic(api_key, model, messages, None).await
            }
            Provider::Gemini => llm::gemini::call_gemini(api_key, model, messages, None).await,
            Provider::Grok => llm::grok::call_grok(api_key, model, messages, None).await,
            Provider::Mistral => llm::mistral::call_mistral(api_key, model, messages, None).await,
            Provider::Deepseek => {
                llm::deepseek::call_deepseek(api_key, model, messages, None).await
            }
            Provider::Kimi => llm::kimi::call_kimi(api_key, model, messages, None).await,
            Provider::Zai => llm::zai::call_zai(api_key, model, messages, None).await,
            Provider::Ollama => llm::ollama::call_ollama(model, messages, None).await,
            Provider::Gguf => llm::gguf::call_gguf(model, messages, None).await,
            Provider::Mlx => llm::mlx::call_mlx(model, messages, None).await,
            Provider::Mock => llm::mock::call_mock(model, messages, None).await,
            Provider::AictlServer => unreachable!("server_route covers Provider::AictlServer"),
        }
    })
    .await;

    match result {
        Ok(Ok((text, _usage))) => Ok(text),
        Ok(Err(e)) => Err(format_call_err(&e)),
        Err(_) => Err(format!(
            "agent generation timed out after {}s (AICTL_LLM_TIMEOUT)",
            llm_timeout.as_secs()
        )),
    }
}

fn format_call_err(e: &AictlError) -> String {
    e.to_string()
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
