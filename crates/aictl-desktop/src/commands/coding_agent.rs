//! Coding-agent Tauri commands.
//!
//! The desktop exposes the same master switch the CLI's `/coding-agent`
//! command toggles. Both surfaces (Settings panel + composer-toolbar
//! icon) call these adapters; the engine reads
//! [`aictl_core::config::coding_agent_enabled`] every turn so a flip
//! takes effect on the next message without an app restart.

use aictl_core::config::{AICTL_CODING_AGENT, coding_agent_enabled, config_set};
use serde::Serialize;

#[derive(Serialize)]
pub struct CodingAgentStatus {
    pub enabled: bool,
}

#[tauri::command]
pub fn coding_agent_status() -> CodingAgentStatus {
    CodingAgentStatus {
        enabled: coding_agent_enabled(),
    }
}

#[tauri::command]
pub fn coding_agent_set_enabled(enabled: bool) -> CodingAgentStatus {
    config_set(AICTL_CODING_AGENT, if enabled { "true" } else { "false" });
    coding_agent_status()
}
