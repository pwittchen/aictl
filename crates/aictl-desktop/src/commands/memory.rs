//! Memory-pane Tauri commands.
//!
//! Surfaces the same `~/.aictl/memory.json` store the CLI manages from
//! `/memory` and `/remember`. The webview's Settings → Memory panel
//! calls these to render the list, toggle the master switch, and prune
//! entries.

use aictl_core::memory::{self, AddOutcome, MemoryEntry};
use serde::Serialize;

#[derive(Serialize)]
pub struct MemoryRow {
    pub id: String,
    pub text: String,
    pub created_at: u64,
}

impl From<MemoryEntry> for MemoryRow {
    fn from(e: MemoryEntry) -> Self {
        Self {
            id: e.id,
            text: e.text,
            created_at: e.created_at,
        }
    }
}

#[derive(Serialize)]
pub struct MemoryStatus {
    pub enabled: bool,
    pub count: usize,
    pub max_entries: usize,
    pub entries: Vec<MemoryRow>,
}

#[tauri::command]
pub fn memory_status() -> MemoryStatus {
    let entries: Vec<MemoryRow> = memory::load().into_iter().map(Into::into).collect();
    MemoryStatus {
        enabled: memory::enabled(),
        count: entries.len(),
        max_entries: memory::MAX_ENTRIES,
        entries,
    }
}

#[tauri::command]
pub fn memory_set_enabled(enabled: bool) -> MemoryStatus {
    memory::set_enabled(enabled);
    memory_status()
}

#[tauri::command]
pub fn memory_add(text: String) -> Result<MemoryRow, String> {
    match memory::add(&text) {
        AddOutcome::Saved(entry) => Ok(entry.into()),
        AddOutcome::Disabled => Err(
            "Memory is disabled (incognito mode is on or AICTL_MEMORY_ENABLED=false)".to_string(),
        ),
        AddOutcome::Empty => Err("Memory text is empty".to_string()),
        AddOutcome::IoError(e) => Err(format!("Failed to save memory: {e}")),
    }
}

#[tauri::command]
pub fn memory_remove(id: String) -> Result<(), String> {
    if memory::remove(&id) {
        Ok(())
    } else {
        Err("Memory not found".to_string())
    }
}

#[tauri::command]
pub fn memory_clear() -> Result<(), String> {
    memory::clear_all().map_err(|e| e.to_string())
}
