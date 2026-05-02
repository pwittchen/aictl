//! Skills-pane Tauri commands.
//!
//! Mirrors the CLI's `/skills` menu: list local + global, delete a
//! specific entry. Authoring is left to the CLI / file system — the
//! desktop pane is for inventory + cleanup.

use aictl_core::skills;
use serde::{Deserialize, Serialize};

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
