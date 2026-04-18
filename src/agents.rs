use std::path::{Path, PathBuf};
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
fn agents_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/agents"))
}

/// Save an agent prompt into `dir`, creating the directory if needed.
fn save_in(dir: &Path, name: &str, prompt: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(name), prompt)
}

/// Read an agent prompt from `dir`.
fn read_in(dir: &Path, name: &str) -> std::io::Result<String> {
    std::fs::read_to_string(dir.join(name))
}

/// Delete an agent file from `dir`.
fn delete_in(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::remove_file(dir.join(name))
}

/// List entries in `dir` with valid agent names, sorted alphabetically.
fn list_in(dir: &Path) -> Vec<AgentEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
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

/// Save an agent prompt to disk.
pub fn save_agent(name: &str, prompt: &str) -> std::io::Result<()> {
    save_in(&agents_dir(), name, prompt)
}

/// Read an agent prompt from disk.
pub fn read_agent(name: &str) -> std::io::Result<String> {
    read_in(&agents_dir(), name)
}

/// Delete an agent from disk.
pub fn delete_agent(name: &str) -> std::io::Result<()> {
    delete_in(&agents_dir(), name)
}

/// An entry in the agent listing.
pub struct AgentEntry {
    pub name: String,
}

/// List all saved agents, sorted alphabetically.
pub fn list_agents() -> Vec<AgentEntry> {
    list_in(&agents_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that touch the global `LOADED_AGENT`.
    static LOAD_LOCK: Mutex<()> = Mutex::new(());

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("aictl-agents-test-{tag}-{pid}-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn is_valid_name_accepts_alphanumeric_dash_underscore() {
        assert!(is_valid_name("abc123"));
        assert!(is_valid_name("A_B-c"));
        assert!(is_valid_name("1"));
        assert!(is_valid_name("ALL_CAPS"));
        assert!(is_valid_name("snake_case-kebab"));
    }

    #[test]
    fn is_valid_name_rejects_empty_and_special_chars() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("has space"));
        assert!(!is_valid_name("dot.name"));
        assert!(!is_valid_name("slash/name"));
        assert!(!is_valid_name("back\\slash"));
        assert!(!is_valid_name("plus+sign"));
        assert!(!is_valid_name("unicode-π"));
    }

    #[test]
    fn load_and_unload_agent_cycle() {
        let _guard = LOAD_LOCK.lock().unwrap();
        let _ = unload_agent();
        assert_eq!(loaded_agent(), None);
        assert_eq!(loaded_agent_name(), None);

        load_agent("my-agent", "prompt body");
        assert_eq!(
            loaded_agent(),
            Some(("my-agent".to_string(), "prompt body".to_string()))
        );
        assert_eq!(loaded_agent_name(), Some("my-agent".to_string()));

        assert!(unload_agent());
        assert_eq!(loaded_agent(), None);
        assert!(!unload_agent());
    }

    #[test]
    fn load_agent_overwrites_previous() {
        let _guard = LOAD_LOCK.lock().unwrap();
        let _ = unload_agent();
        load_agent("first", "p1");
        load_agent("second", "p2");
        assert_eq!(
            loaded_agent(),
            Some(("second".to_string(), "p2".to_string()))
        );
        let _ = unload_agent();
    }

    #[test]
    fn save_and_read_roundtrip() {
        let dir = unique_temp_dir("rw");
        save_in(&dir, "writer", "my prompt content").unwrap();
        assert_eq!(read_in(&dir, "writer").unwrap(), "my prompt content");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = unique_temp_dir("mkdir").join("nested");
        save_in(&dir, "a", "x").unwrap();
        assert!(dir.join("a").exists());
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn delete_removes_file() {
        let dir = unique_temp_dir("del");
        save_in(&dir, "tempname", "x").unwrap();
        delete_in(&dir, "tempname").unwrap();
        assert!(read_in(&dir, "tempname").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_missing_returns_err() {
        let dir = unique_temp_dir("del-missing");
        assert!(delete_in(&dir, "never-existed").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_sorted_and_filters_invalid_entries() {
        let dir = unique_temp_dir("list");
        save_in(&dir, "zebra", "").unwrap();
        save_in(&dir, "alpha", "").unwrap();
        save_in(&dir, "mango", "").unwrap();
        // Files with disallowed names are filtered out.
        std::fs::write(dir.join("has space"), "").unwrap();
        std::fs::write(dir.join("bad.name"), "").unwrap();
        // Subdirectories are ignored.
        std::fs::create_dir(dir.join("subdir")).unwrap();

        let names: Vec<_> = list_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["alpha", "mango", "zebra"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_missing_dir_returns_empty() {
        let missing = std::env::temp_dir().join("aictl-agents-nonexistent-xyz-12345");
        assert!(list_in(&missing).is_empty());
    }
}
