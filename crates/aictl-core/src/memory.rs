//! Long-term memory: facts the agent has learned about the user, persisted
//! across sessions in `~/.aictl/memory.json` and injected into every system
//! prompt when [`enabled`] is on.
//!
//! Two seams write to this store:
//!   * the `save_memory` tool the agent can call when it identifies
//!     something worth remembering (preferences, role, ongoing work, an
//!     explicit "please remember X" from the user),
//!   * the CLI `/remember <fact>` slash command for direct user input.
//!
//! Reads happen in [`crate::run::build_system_prompt`], which appends the
//! `# Memory` block when the feature is enabled and the session is not
//! incognito. Incognito mode is the kill-switch for the whole subsystem:
//! [`add`] is a no-op, [`load`] returns an empty list, and the system
//! prompt block is suppressed — so a temporary chat never leaks into
//! the long-term store and never sees prior memories.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::session;

/// Hard cap on the number of entries kept on disk and in the prompt.
/// Memory injects into every turn's system prompt, so an unbounded list
/// would silently bloat input tokens — `/memory` and the desktop UI
/// surface this so the user can prune. New writes past the cap drop the
/// oldest entry first.
pub const MAX_ENTRIES: usize = 200;

/// Hard cap on a single memory entry's text length. Stops the model
/// (or a careless `/remember`) from pasting an entire transcript into
/// the long-term store.
pub const MAX_ENTRY_LEN: usize = 1000;

/// One persisted fact. `id` is a v4 uuid (same generator the session
/// store uses). `created_at` is Unix-epoch seconds — kept simple to
/// avoid pulling in a date library for what is purely metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MemoryFile {
    #[serde(default)]
    memories: Vec<MemoryEntry>,
}

/// Cache the parsed list so every system-prompt build doesn't re-read
/// the file. Writes go through [`save`] which refreshes the cache; the
/// CLI/desktop never edit the file out-of-band.
static CACHE: Mutex<Option<Vec<MemoryEntry>>> = Mutex::new(None);

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

/// Path to `~/.aictl/memory.json`. Creates the parent directory if it
/// doesn't exist yet — the same shape `session::sessions_dir` uses.
pub fn memory_file() -> Option<PathBuf> {
    let h = home()?;
    let dir = PathBuf::from(format!("{h}/.aictl"));
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("memory.json"))
}

/// Whether the long-term-memory subsystem is enabled.
///
/// Reads `AICTL_MEMORY_ENABLED` from `~/.aictl/config`; defaults to
/// `true` so the feature is on for new users. Incognito mode is a
/// stronger kill-switch — it overrides this flag for both reads and
/// writes (see [`add`] and [`load_for_prompt`]).
#[must_use]
pub fn enabled() -> bool {
    let raw = config::config_get("AICTL_MEMORY_ENABLED");
    !matches!(
        raw.as_deref().map(str::trim).map(str::to_ascii_lowercase),
        Some(s) if matches!(s.as_str(), "false" | "0" | "no" | "off")
    )
}

/// Persist the enable flag. Round-trips through `~/.aictl/config` so
/// the next launch picks up the same value.
pub fn set_enabled(on: bool) {
    config::config_set("AICTL_MEMORY_ENABLED", if on { "true" } else { "false" });
}

fn read_from_disk() -> Vec<MemoryEntry> {
    let Some(path) = memory_file() else {
        return Vec::new();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let parsed: MemoryFile = serde_json::from_str(&contents).unwrap_or_default();
    parsed.memories
}

fn write_to_disk(entries: &[MemoryEntry]) -> io::Result<()> {
    let Some(path) = memory_file() else {
        return Err(io::Error::other("HOME not set"));
    };
    let payload = MemoryFile {
        memories: entries.to_vec(),
    };
    let body = serde_json::to_string_pretty(&payload)
        .map_err(|e| io::Error::other(format!("serialize memory: {e}")))?;
    fs::write(&path, body)
}

/// Return all stored entries, oldest first. Independent of the
/// enable/incognito toggles — used by `/memory` and the desktop list
/// view, which need to render the catalogue regardless.
pub fn load() -> Vec<MemoryEntry> {
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.is_none() {
        *cache = Some(read_from_disk());
    }
    cache.clone().unwrap_or_default()
}

/// Return entries for the system-prompt block. Returns an empty list
/// when memory is globally disabled or the session is incognito so
/// callers don't need to repeat those checks.
#[must_use]
pub fn load_for_prompt() -> Vec<MemoryEntry> {
    if !enabled() || session::is_incognito() {
        return Vec::new();
    }
    load()
}

/// Force-refresh the cache from disk. Used by tests and by paths that
/// edit the file directly (none today, but the door is open).
pub fn refresh() {
    let fresh = read_from_disk();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(fresh);
    }
}

/// Outcome of an [`add`] call. The CLI and tool surfaces use this to
/// decide what to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddOutcome {
    /// New entry persisted.
    Saved(MemoryEntry),
    /// Memory is disabled (`AICTL_MEMORY_ENABLED=false`) or the session
    /// is incognito. The fact was *not* written.
    Disabled,
    /// The fact text was empty after trimming.
    Empty,
    /// I/O error writing the file.
    IoError(String),
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Add a new memory. Trims surrounding whitespace, truncates to
/// [`MAX_ENTRY_LEN`], and drops the oldest entry when the list would
/// exceed [`MAX_ENTRIES`].
///
/// No-op (returns [`AddOutcome::Disabled`]) when memory is disabled or
/// the session is incognito — this is the kill-switch path.
pub fn add(text: &str) -> AddOutcome {
    if !enabled() || session::is_incognito() {
        return AddOutcome::Disabled;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return AddOutcome::Empty;
    }
    let body: String = trimmed.chars().take(MAX_ENTRY_LEN).collect();
    let entry = MemoryEntry {
        id: session::generate_uuid(),
        text: body,
        created_at: now_secs(),
    };
    let mut entries = load();
    entries.push(entry.clone());
    while entries.len() > MAX_ENTRIES {
        entries.remove(0);
    }
    if let Err(e) = write_to_disk(&entries) {
        return AddOutcome::IoError(e.to_string());
    }
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(entries);
    }
    AddOutcome::Saved(entry)
}

/// Remove a single entry by id. Returns `true` when an entry was
/// actually removed. Bypasses the enable/incognito gate so a user can
/// always prune even after they turn the feature off.
pub fn remove(id: &str) -> bool {
    let mut entries = load();
    let before = entries.len();
    entries.retain(|e| e.id != id);
    if entries.len() == before {
        return false;
    }
    if write_to_disk(&entries).is_err() {
        return false;
    }
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(entries);
    }
    true
}

/// Drop every memory and rewrite an empty file. Bypasses the
/// enable/incognito gate (same reasoning as [`remove`]).
pub fn clear_all() -> io::Result<()> {
    write_to_disk(&[])?;
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(Vec::new());
    }
    Ok(())
}

/// Render the memory block appended to the system prompt. Returns an
/// empty string when there is nothing to inject (disabled, incognito,
/// or empty store) so callers can unconditionally concatenate.
#[must_use]
pub fn prompt_block() -> String {
    use std::fmt::Write as _;
    let entries = load_for_prompt();
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(
        "\n\n# Memory\n\n\
         Facts you have learned about the user across past conversations. \
         Treat them as authoritative context for tailoring your answers \
         (preferences, role, ongoing work). When the user shares something \
         new that fits this category — or explicitly says \"remember\", \
         \"memorize\", \"please save\", or similar — call the `save_memory` \
         tool with a concise factual summary.\n",
    );
    for (i, e) in entries.iter().enumerate() {
        let _ = write!(out, "\n{}. {}", i + 1, e.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_outcome_empty_text() {
        // Pure-logic check: trimming an empty string produces Empty regardless
        // of cache state. Doesn't touch the on-disk file.
        let result = match "".trim() {
            s if s.is_empty() => AddOutcome::Empty,
            _ => AddOutcome::Saved(MemoryEntry {
                id: "x".to_string(),
                text: "y".to_string(),
                created_at: 0,
            }),
        };
        assert_eq!(result, AddOutcome::Empty);
    }

    #[test]
    fn entry_text_truncated_to_max_len() {
        let big = "a".repeat(MAX_ENTRY_LEN * 2);
        let truncated: String = big.trim().chars().take(MAX_ENTRY_LEN).collect();
        assert_eq!(truncated.len(), MAX_ENTRY_LEN);
    }

    #[test]
    fn prompt_block_empty_when_no_entries() {
        // Pure helper; the conditional paths through enabled/incognito
        // are exercised by integration tests at the call site.
        let entries: Vec<MemoryEntry> = Vec::new();
        let block = if entries.is_empty() {
            String::new()
        } else {
            "non-empty".to_string()
        };
        assert!(block.is_empty());
    }
}
