//! Skills-pane Tauri commands.
//!
//! Mirrors the CLI's `/skills` menu: list local + global, delete a
//! specific entry. Authoring is left to the CLI / file system — the
//! desktop pane is for inventory + cleanup.

use std::sync::Arc;

use aictl_core::skills;
use aictl_core::skills::remote;
use serde::{Deserialize, Serialize};
use tauri::State;

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
