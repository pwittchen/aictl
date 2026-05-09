//! `save_memory` tool: persist a fact about the user to the long-term
//! memory store (`~/.aictl/memory.json`) so it survives across sessions
//! and gets injected into the system prompt of every future conversation.
//!
//! Input is a single line (or multi-line block — flattened to a paragraph)
//! describing the fact. The agent is expected to call this when it
//! identifies something worth remembering: a stated preference, the
//! user's role, ongoing work, or an explicit "please remember X" /
//! "memorize X" request.
//!
//! Returns one of:
//!   * `Memory saved: <text>` on success — the CLI/desktop UIs surface
//!     this verbatim so the user sees the persisted fact.
//!   * `Memory is disabled` when `AICTL_MEMORY_ENABLED=false` or the
//!     session is incognito (kill-switch path; nothing is written).
//!   * `Memory text is empty` for a blank input.
//!   * `Failed to save memory: <reason>` on I/O error.

use crate::memory::{self, AddOutcome};

pub(super) fn tool_save_memory(input: &str) -> String {
    match memory::add(input) {
        AddOutcome::Saved(entry) => format!("Memory saved: {}", entry.text),
        AddOutcome::Disabled => {
            "Memory is disabled (AICTL_MEMORY_ENABLED=false or incognito mode is on). \
             Tell the user the fact was not saved and that they can enable memory \
             via /memory or by setting AICTL_MEMORY_ENABLED=true in ~/.aictl/config."
                .to_string()
        }
        AddOutcome::Empty => {
            "Memory text is empty — pass the fact to remember as input.".to_string()
        }
        AddOutcome::IoError(e) => format!("Failed to save memory: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty_message() {
        // Pure-logic guard: a blank input must not reach the disk-write path,
        // so the result is recognizable without touching the filesystem.
        let out = tool_save_memory("   ");
        assert!(out.contains("empty"));
    }
}
