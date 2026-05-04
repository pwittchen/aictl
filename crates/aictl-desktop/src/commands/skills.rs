//! Skills-pane Tauri commands.
//!
//! Mirrors the CLI's `/skills` menu: list local + global, delete a
//! specific entry, and (since the desktop now exposes authoring)
//! create new skills either by hand or by asking the active model to
//! draft the body.

use std::sync::Arc;

use aictl_core::config;
use aictl_core::error::AictlError;
use aictl_core::llm;
use aictl_core::message::{Message, Role};
use aictl_core::run::Provider;
use aictl_core::skills;
use aictl_core::skills::remote;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::chat;
use crate::state::AppState;

#[derive(Serialize)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    pub source: Option<String>,
    pub category: Option<String>,
    pub origin: String,
    pub official: bool,
    pub dir: String,
}

#[tauri::command]
pub fn skills_list() -> Vec<SkillRow> {
    skills::list()
        .into_iter()
        .map(|e| SkillRow {
            official: e.is_official(),
            name: e.name,
            description: e.description,
            source: e.source,
            category: e.category,
            origin: e.origin.label().to_string(),
            dir: e.dir.display().to_string(),
        })
        .collect()
}

#[derive(Deserialize)]
pub struct SkillDeleteArgs {
    pub name: String,
    pub origin: String,
}

/// Delete the skill the user actually saw — origin disambiguates the
/// global / local / `.claude` legacy directories.
#[tauri::command]
pub fn skill_delete(args: SkillDeleteArgs) -> Result<(), String> {
    let entries = skills::list();
    let entry = entries
        .iter()
        .find(|e| e.name == args.name && e.origin.label() == args.origin)
        .ok_or_else(|| format!("skill '{}' ({}) not found", args.name, args.origin))?;
    skills::delete_entry(entry).map_err(|e| format!("delete: {e}"))
}

#[derive(serde::Serialize)]
pub struct SkillView {
    pub name: String,
    pub description: String,
    pub origin: String,
    pub path: String,
    pub raw: String,
    pub body: String,
}

/// Read the full SKILL.md for a specific listing entry. Returns both
/// the raw file contents (frontmatter + body) and the parsed body so
/// the webview can render whichever feels more useful — markdown view
/// uses `body`, the source view falls back to `raw`.
#[tauri::command]
pub fn skill_view(args: SkillDeleteArgs) -> Result<SkillView, String> {
    let entries = skills::list();
    let entry = entries
        .iter()
        .find(|e| e.name == args.name && e.origin.label() == args.origin)
        .ok_or_else(|| format!("skill '{}' ({}) not found", args.name, args.origin))?;
    let path = entry.dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read SKILL.md: {e}"))?;
    let parsed = skills::parse(&raw);
    Ok(SkillView {
        name: entry.name.clone(),
        description: entry.description.clone(),
        origin: entry.origin.label().to_string(),
        path: path.display().to_string(),
        raw,
        body: parsed.body,
    })
}

#[derive(Deserialize)]
pub struct SkillLoadArgs {
    pub name: String,
}

/// Pin `name` as the skill that prefixes the system prompt for every
/// turn until [`skill_unload`] is called. The body is *not* cached —
/// `chat::run_turn` re-resolves the file every turn so on-disk edits
/// take effect immediately. Errors out when the skill no longer exists
/// so the picker can surface a clear toast instead of silently failing.
#[tauri::command]
pub fn skill_load(state: State<'_, Arc<AppState>>, args: SkillLoadArgs) -> Result<(), String> {
    if skills::find(&args.name).is_none() {
        return Err(format!("skill '{}' not found", args.name));
    }
    let mut slot = state
        .loaded_skill
        .lock()
        .map_err(|_| "loaded-skill mutex poisoned".to_string())?;
    *slot = Some(args.name);
    Ok(())
}

/// Drop the currently-loaded skill so the next turn runs against the
/// stock system prompt.
#[tauri::command]
pub fn skill_unload(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut slot = state
        .loaded_skill
        .lock()
        .map_err(|_| "loaded-skill mutex poisoned".to_string())?;
    *slot = None;
    Ok(())
}

/// Read the currently-loaded skill name (`None` when no skill is
/// loaded). The webview calls this on mount so the icon's highlight
/// state survives a window reload.
#[tauri::command]
pub fn skill_loaded(state: State<'_, Arc<AppState>>) -> Result<Option<String>, String> {
    let slot = state
        .loaded_skill
        .lock()
        .map_err(|_| "loaded-skill mutex poisoned".to_string())?;
    Ok(slot.clone())
}

/// One row in the remote skills catalogue. `state` is the same enum the
/// CLI prints (`not_pulled` / `up_to_date` / `upstream_newer`) — the
/// webview hides anything that already matches a local entry, so
/// returning every row keeps the API symmetric with the agents side and
/// lets a future "refresh installed" button reuse the same call.
#[derive(Serialize)]
pub struct RemoteSkillRow {
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub state: String,
}

#[tauri::command]
pub async fn skills_list_remote() -> Result<Vec<RemoteSkillRow>, String> {
    let entries = remote::list_skills().await?;
    Ok(entries
        .into_iter()
        .map(|s| RemoteSkillRow {
            state: state_label(s.state),
            name: s.name,
            description: s.description,
            category: s.category,
        })
        .collect())
}

#[derive(Deserialize)]
pub struct SkillPullArgs {
    pub name: String,
    /// `true` when the user has already confirmed they want to clobber an
    /// existing local copy. The desktop confirms in JS before invoking,
    /// so a `false` here means "abort if a local file exists".
    #[serde(default)]
    pub overwrite: bool,
}

/// Outcome string mirrors `remote::PullOutcome` so the webview can pick
/// a different toast for fresh-install vs. overwrite vs. skipped.
#[tauri::command]
pub async fn skill_pull(args: SkillPullArgs) -> Result<String, String> {
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
pub struct SkillSaveArgs {
    pub name: String,
    pub description: String,
    pub body: String,
    /// `true` once the user has confirmed they want to clobber an
    /// existing skill of the same name. Mirrors the agent-save gate.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSaveOutcome {
    Installed,
    Overwritten,
}

/// Persist a new (or rewritten) skill to
/// `~/.aictl/skills/<name>/SKILL.md`. `skills::save` already validates
/// the name and refuses reserved slash-command collisions; this
/// wrapper adds the existence/overwrite gate so the dialog can prompt
/// before clobbering.
#[tauri::command]
pub fn skill_save(args: SkillSaveArgs) -> Result<SkillSaveOutcome, String> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err("skill name is empty".to_string());
    }
    let description = args.description.trim().to_string();
    if description.is_empty() {
        return Err("skill description is empty".to_string());
    }
    let body = args.body.trim().to_string();
    if body.is_empty() {
        return Err("skill body is empty".to_string());
    }
    let exists = skills::list().into_iter().any(|e| e.name == name);
    if exists && !args.overwrite {
        return Err(format!("skill '{name}' already exists"));
    }
    skills::save(&name, &description, &body).map_err(|e| format!("save skill: {e}"))?;
    Ok(if exists {
        SkillSaveOutcome::Overwritten
    } else {
        SkillSaveOutcome::Installed
    })
}

#[derive(Deserialize)]
pub struct SkillGenerateArgs {
    pub name: String,
    pub description: String,
}

/// Draft a skill body via the active provider, mirroring the CLI's
/// `create_skill_with_ai`. Returns the generated text to the webview
/// so the user can review/edit before saving.
#[tauri::command]
pub async fn skill_generate(args: SkillGenerateArgs) -> Result<String, String> {
    let name = args.name.trim().to_string();
    if name.is_empty() {
        return Err("skill name is empty".to_string());
    }
    if !skills::is_valid_name(&name) {
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
            content: "You are an expert at writing procedural \"skills\" — short markdown playbooks that tell another AI assistant how to perform a specific, repeatable task. \
                Generate the body of a skill based on the user's description. The body should be a clear, numbered set of steps the assistant should follow when invoked, \
                including which tools to use and how to phrase the final output. Do NOT include YAML frontmatter or a heading with the skill name — only the procedure body. Output ONLY the markdown body, nothing else."
                .to_string(),
            images: vec![],
        },
        Message {
            role: Role::User,
            content: format!(
                "Create a skill named \"{name}\" that does the following: {description}"
            ),
            images: vec![],
        },
    ];

    let body = call_provider_buffered(&provider, &api_key, &model, &messages).await?;
    Ok(body.trim().to_string())
}

/// One-shot, non-streaming dispatch to whichever provider is active.
/// Duplicates the matrix from `commands::agents` so a future shared
/// helper has only one extraction site.
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
            "skill generation timed out after {}s (AICTL_LLM_TIMEOUT)",
            llm_timeout.as_secs()
        )),
    }
}

fn format_call_err(e: &AictlError) -> String {
    e.to_string()
}
