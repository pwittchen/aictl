use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod remote;

/// Frontmatter value marking an agent as sourced from the first-party
/// catalogue at <https://github.com/pwittchen/aictl/tree/master/.aictl/agents>.
/// Badged as `[official]` in the REPL and `--list-agents` output. Anything else
/// (missing, `user`, or a user-supplied string) renders without the badge.
pub const OFFICIAL_SOURCE: &str = "aictl-official";

/// Global state for the currently loaded agent: (name, prompt body).
static LOADED_AGENT: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Load an agent into the global state. `prompt` is the raw file contents;
/// any YAML frontmatter is stripped before being surfaced in the system
/// prompt, so pulled catalogue agents don't leak their metadata block into
/// the LLM.
pub fn load_agent(name: &str, prompt: &str) {
    let body = parse(prompt).body;
    *LOADED_AGENT.lock().unwrap() = Some((name.to_string(), body));
}

/// Unload the currently loaded agent. Returns true if one was loaded.
pub fn unload_agent() -> bool {
    LOADED_AGENT.lock().unwrap().take().is_some()
}

/// Get the currently loaded agent (name, body).
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
pub fn agents_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/agents"))
}

/// Save an agent prompt into `dir`, creating the directory if needed.
fn save_in(dir: &Path, name: &str, prompt: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(name), prompt)
}

/// Read an agent's raw file contents (including frontmatter) from `dir`.
fn read_in(dir: &Path, name: &str) -> std::io::Result<String> {
    std::fs::read_to_string(dir.join(name))
}

/// Delete an agent file from `dir`.
fn delete_in(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::remove_file(dir.join(name))
}

/// List entries in `dir` with valid agent names, sorted alphabetically.
/// Each entry has its frontmatter parsed so the caller can render badges
/// or filter by category without a second pass.
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
            if !is_valid_name(&name) {
                return None;
            }
            let contents = std::fs::read_to_string(e.path()).ok().unwrap_or_default();
            let meta = parse(&contents);
            Some(AgentEntry {
                name,
                description: meta.description,
                source: meta.source,
                category: meta.category,
            })
        })
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

/// Save an agent file to disk. `prompt` is written verbatim — callers that
/// want frontmatter must include it themselves.
pub fn save_agent(name: &str, prompt: &str) -> std::io::Result<()> {
    save_in(&agents_dir(), name, prompt)
}

/// Read an agent's raw file contents from disk (frontmatter included).
pub fn read_agent(name: &str) -> std::io::Result<String> {
    read_in(&agents_dir(), name)
}

/// Read an agent's parsed metadata + body from disk.
pub fn read_agent_meta(name: &str) -> std::io::Result<AgentMeta> {
    let raw = read_in(&agents_dir(), name)?;
    Ok(parse(&raw))
}

/// Delete an agent from disk.
pub fn delete_agent(name: &str) -> std::io::Result<()> {
    delete_in(&agents_dir(), name)
}

/// An entry in the agent listing.
pub struct AgentEntry {
    pub name: String,
    pub description: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
}

impl AgentEntry {
    /// True if this agent was pulled from the first-party catalogue.
    pub fn is_official(&self) -> bool {
        self.source.as_deref() == Some(OFFICIAL_SOURCE)
    }
}

/// Parsed agent file: optional frontmatter fields plus the prompt body
/// (everything after the closing `---` fence, leading whitespace trimmed).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub body: String,
}

/// Parse an agent file. Recognizes `name`, `description`, `source`,
/// `category`; unknown fields are silently ignored so older clients don't
/// break on forward-compatible additions. Files without a leading `---`
/// fence are returned as pure body (matches the historical behaviour where
/// agents were plain prompt text).
pub fn parse(contents: &str) -> AgentMeta {
    let mut meta = AgentMeta::default();
    let trimmed = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let Some(after_open) = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    else {
        meta.body = contents.to_string();
        return meta;
    };

    let mut body_start = None;
    let mut cursor = 0usize;
    for line in after_open.split_inclusive('\n') {
        let line_trimmed = line.trim_end_matches(['\n', '\r']);
        if line_trimmed == "---" {
            body_start = Some(cursor + line.len());
            break;
        }
        if let Some((key, value)) = line_trimmed.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            let slot = match key {
                "name" => &mut meta.name,
                "description" => &mut meta.description,
                "source" => &mut meta.source,
                "category" => &mut meta.category,
                _ => {
                    cursor += line.len();
                    continue;
                }
            };
            *slot = Some(value.to_string());
        }
        cursor += line.len();
    }

    let Some(start) = body_start else {
        // Unterminated frontmatter → treat whole file as body so we don't
        // silently drop the prompt.
        meta = AgentMeta::default();
        meta.body = contents.to_string();
        return meta;
    };
    meta.body = after_open[start..]
        .trim_start_matches(['\n', '\r'])
        .to_string();
    meta
}

/// List all saved agents, sorted alphabetically. Each entry has its
/// frontmatter parsed for badge/category rendering.
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
    fn load_agent_strips_frontmatter() {
        let _guard = LOAD_LOCK.lock().unwrap();
        let _ = unload_agent();
        let raw = "---\nname: bug-hunter\nsource: aictl-official\n---\n\nYou are a bug hunter.\n";
        load_agent("bug-hunter", raw);
        let (name, body) = loaded_agent().unwrap();
        assert_eq!(name, "bug-hunter");
        assert_eq!(body, "You are a bug hunter.\n");
        let _ = unload_agent();
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
    fn list_populates_frontmatter_fields() {
        let dir = unique_temp_dir("list-meta");
        save_in(
            &dir,
            "pulled",
            "---\nname: pulled\ndescription: official one\nsource: aictl-official\ncategory: dev\n---\n\nBody.\n",
        )
        .unwrap();
        save_in(&dir, "plain", "Just body, no frontmatter").unwrap();

        let entries = list_in(&dir);
        let pulled = entries.iter().find(|e| e.name == "pulled").unwrap();
        assert_eq!(pulled.description.as_deref(), Some("official one"));
        assert_eq!(pulled.category.as_deref(), Some("dev"));
        assert!(pulled.is_official());

        let plain = entries.iter().find(|e| e.name == "plain").unwrap();
        assert!(plain.description.is_none());
        assert!(plain.category.is_none());
        assert!(!plain.is_official());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_missing_dir_returns_empty() {
        let missing = std::env::temp_dir().join("aictl-agents-nonexistent-xyz-12345");
        assert!(list_in(&missing).is_empty());
    }

    #[test]
    fn parse_reads_known_fields_and_ignores_unknown() {
        let src = "---\nname: x\ndescription: d\nsource: aictl-official\ncategory: dev\nextra: ignored\n---\n\nBody here.\n";
        let m = parse(src);
        assert_eq!(m.name.as_deref(), Some("x"));
        assert_eq!(m.description.as_deref(), Some("d"));
        assert_eq!(m.source.as_deref(), Some("aictl-official"));
        assert_eq!(m.category.as_deref(), Some("dev"));
        assert_eq!(m.body, "Body here.\n");
    }

    #[test]
    fn parse_without_frontmatter_returns_full_body() {
        let src = "just a prompt\nsecond line\n";
        let m = parse(src);
        assert!(m.name.is_none());
        assert_eq!(m.body, src);
    }

    #[test]
    fn parse_unterminated_frontmatter_falls_back_to_whole_body() {
        let src = "---\nname: x\nno closing fence\n";
        let m = parse(src);
        assert!(m.name.is_none());
        assert_eq!(m.body, src);
    }

    #[test]
    fn parse_handles_quoted_values() {
        let src = "---\nname: \"quoted\"\ndescription: 'single'\n---\nbody\n";
        let m = parse(src);
        assert_eq!(m.name.as_deref(), Some("quoted"));
        assert_eq!(m.description.as_deref(), Some("single"));
    }
}
