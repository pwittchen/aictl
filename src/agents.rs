use std::sync::Mutex;

/// Global state for the currently loaded agent: (name, prompt content).
static LOADED_AGENT: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Load an agent into the global state.
pub fn load_agent(name: &str, prompt: &str) {
    *LOADED_AGENT.lock().unwrap() = Some((name.to_string(), prompt.to_string()));
}

/// Unload the currently loaded agent. Returns true if one was loaded.
pub fn unload_agent() -> bool {
    LOADED_AGENT.lock().unwrap().take().is_some()
}

/// Get the currently loaded agent (name, prompt).
pub fn loaded_agent() -> Option<(String, String)> {
    LOADED_AGENT.lock().unwrap().clone()
}

/// Get just the loaded agent name.
pub fn loaded_agent_name() -> Option<String> {
    LOADED_AGENT
        .lock()
        .unwrap()
        .as_ref()
        .map(|(n, _)| n.clone())
}

/// Validate an agent name: only letters, numbers, underscore, dash.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Return the agents directory path (~/.aictl/agents/).
fn agents_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.aictl/agents")
}

/// Save an agent prompt to disk.
pub fn save_agent(name: &str, prompt: &str) -> std::io::Result<()> {
    let dir = agents_dir();
    std::fs::create_dir_all(&dir)?;
    let path = format!("{dir}/{name}");
    std::fs::write(path, prompt)
}

/// Read an agent prompt from disk.
pub fn read_agent(name: &str) -> std::io::Result<String> {
    let dir = agents_dir();
    let path = format!("{dir}/{name}");
    std::fs::read_to_string(path)
}

/// Delete an agent from disk.
pub fn delete_agent(name: &str) -> std::io::Result<()> {
    let dir = agents_dir();
    let path = format!("{dir}/{name}");
    std::fs::remove_file(path)
}

/// An entry in the agent listing.
pub struct AgentEntry {
    pub name: String,
}

/// List all saved agents, sorted alphabetically.
pub fn list_agents() -> Vec<AgentEntry> {
    let dir = agents_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut agents: Vec<AgentEntry> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let ft = e.file_type().ok()?;
            if !ft.is_file() {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if is_valid_name(&name) {
                Some(AgentEntry { name })
            } else {
                None
            }
        })
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}
