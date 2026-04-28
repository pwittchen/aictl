use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod remote;

/// Frontmatter value marking an agent as sourced from the first-party
/// catalogue at <https://github.com/pwittchen/aictl/tree/master/.aictl/agents>.
/// Badged as `[official]` in the REPL and `--list-agents` output. Anything else
/// (missing, `user`, or a user-supplied string) renders without the badge.
pub const OFFICIAL_SOURCE: &str = "aictl-official";

/// Where on disk an agent file lives. Surfaces the difference between the
/// per-user catalogue and project-local overrides so listings can render the
/// origin badge (`global`, `local`, `local (claude)`) and so callers know
/// which file to edit or delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `~/.aictl/agents/<name>` — the per-user catalogue.
    Global,
    /// `<cwd>/.aictl/agents/<name>` — project-local override.
    Local,
    /// `<cwd>/.claude/agents/<name>` — legacy local fallback when the
    /// project has no `.aictl/` directory.
    LocalClaude,
}

impl Origin {
    /// Human-readable label for listings (`global` / `local` / `local (claude)`).
    pub fn label(self) -> &'static str {
        match self {
            Origin::Global => "global",
            Origin::Local => "local",
            Origin::LocalClaude => "local (claude)",
        }
    }
}

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

/// Resolve the project-local agents directory and the origin it represents,
/// if any. Honors the `.aictl/` > `.claude/` precedence enforced by
/// [`crate::config::local_config_root`]; returns `None` when no local root
/// is present or its `agents/` subdirectory does not exist.
pub fn local_agents_dir() -> Option<(PathBuf, Origin)> {
    let (root, kind) = crate::config::local_config_root()?;
    let dir = root.join("agents");
    if !dir.is_dir() {
        return None;
    }
    let origin = match kind {
        crate::config::LocalRoot::Aictl => Origin::Local,
        crate::config::LocalRoot::Claude => Origin::LocalClaude,
    };
    Some((dir, origin))
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

/// List entries in `dir` tagged with `origin`, sorted alphabetically.
/// Each entry has its frontmatter parsed so the caller can render badges
/// or filter by category without a second pass. Both `<name>` (the legacy
/// global-catalogue format) and `<name>.md` (the convention used by remote
/// catalogue files and project-local `.aictl/agents/` and `.claude/agents/`
/// directories) are accepted; the suffix is stripped for the entry name.
fn list_in(dir: &Path, origin: Origin) -> Vec<AgentEntry> {
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
            let file_name = e.file_name().to_string_lossy().to_string();
            let name = file_name
                .strip_suffix(".md")
                .unwrap_or(&file_name)
                .to_string();
            if !is_valid_name(&name) {
                return None;
            }
            let path = e.path();
            let contents = std::fs::read_to_string(&path).ok().unwrap_or_default();
            let meta = parse(&contents);
            Some(AgentEntry {
                name,
                description: meta.description,
                source: meta.source,
                category: meta.category,
                origin,
                path,
            })
        })
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

/// Merge global + local agent listings. Local entries override global ones
/// of the same name so the listing surfaces what `read_agent` would actually
/// load. Result is sorted alphabetically.
fn list_combined(global_dir: &Path, local: Option<(&Path, Origin)>) -> Vec<AgentEntry> {
    let mut by_name: HashMap<String, AgentEntry> = HashMap::new();
    for e in list_in(global_dir, Origin::Global) {
        by_name.insert(e.name.clone(), e);
    }
    if let Some((dir, origin)) = local {
        for e in list_in(dir, origin) {
            by_name.insert(e.name.clone(), e);
        }
    }
    let mut agents: Vec<_> = by_name.into_values().collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

/// Save an agent file to disk under the per-user catalogue
/// (`~/.aictl/agents/`). `prompt` is written verbatim — callers that want
/// frontmatter must include it themselves. Local-origin agents are not
/// writable through this path; edit their files directly instead.
pub fn save_agent(name: &str, prompt: &str) -> std::io::Result<()> {
    save_in(&agents_dir(), name, prompt)
}

/// Read an agent's raw file contents (frontmatter included). Local
/// directories take priority — `<cwd>/.aictl/agents/<name>[.md]` (or
/// `<cwd>/.claude/agents/<name>[.md]` as a legacy fallback) is consulted
/// before the per-user `~/.aictl/agents/<name>`. Both extensionless and
/// `.md` filenames are accepted so the same lookup works against the
/// historical global format and the project-local `.md` convention.
pub fn read_agent(name: &str) -> std::io::Result<String> {
    if let Some((dir, _)) = local_agents_dir() {
        for candidate in [dir.join(format!("{name}.md")), dir.join(name)] {
            if candidate.exists() {
                return std::fs::read_to_string(&candidate);
            }
        }
    }
    read_in(&agents_dir(), name)
}

/// Read an agent's parsed metadata + body. Resolves with the same local-first
/// precedence as [`read_agent`].
pub fn read_agent_meta(name: &str) -> std::io::Result<AgentMeta> {
    let raw = read_agent(name)?;
    Ok(parse(&raw))
}

/// Delete the file backing a specific listing entry. Lets the menu act on
/// the location the user actually saw rather than always targeting global.
pub fn delete_agent_entry(entry: &AgentEntry) -> std::io::Result<()> {
    std::fs::remove_file(&entry.path)
}

/// Save (overwrite) the file backing a specific listing entry. Used when
/// the menu edits an existing agent — the rewrite stays at the same origin.
pub fn save_agent_entry(entry: &AgentEntry, prompt: &str) -> std::io::Result<()> {
    if let Some(parent) = entry.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&entry.path, prompt)
}

/// An entry in the agent listing.
pub struct AgentEntry {
    pub name: String,
    pub description: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
    /// Where this agent lives on disk. Drives the origin badge in listings
    /// and lets edit/delete actions target the right file.
    pub origin: Origin,
    /// Full path to the backing file.
    pub path: PathBuf,
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

/// List all saved agents, sorted alphabetically. Merges the per-user
/// catalogue with the project-local override directory, with local entries
/// taking precedence on name collisions so the listing matches what
/// [`read_agent`] would resolve.
pub fn list_agents() -> Vec<AgentEntry> {
    list_combined(
        &agents_dir(),
        local_agents_dir().as_ref().map(|(d, o)| (d.as_path(), *o)),
    )
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
    fn delete_entry_removes_file() {
        let dir = unique_temp_dir("del");
        save_in(&dir, "tempname", "x").unwrap();
        let entry = AgentEntry {
            name: "tempname".to_string(),
            description: None,
            source: None,
            category: None,
            origin: Origin::Global,
            path: dir.join("tempname"),
        };
        delete_agent_entry(&entry).unwrap();
        assert!(read_in(&dir, "tempname").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_entry_missing_returns_err() {
        let dir = unique_temp_dir("del-missing");
        let entry = AgentEntry {
            name: "never-existed".to_string(),
            description: None,
            source: None,
            category: None,
            origin: Origin::Global,
            path: dir.join("never-existed"),
        };
        assert!(delete_agent_entry(&entry).is_err());
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

        let names: Vec<_> = list_in(&dir, Origin::Global)
            .into_iter()
            .map(|e| e.name)
            .collect();
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

        let entries = list_in(&dir, Origin::Global);
        let pulled = entries.iter().find(|e| e.name == "pulled").unwrap();
        assert_eq!(pulled.description.as_deref(), Some("official one"));
        assert_eq!(pulled.category.as_deref(), Some("dev"));
        assert!(pulled.is_official());
        assert_eq!(pulled.origin, Origin::Global);

        let plain = entries.iter().find(|e| e.name == "plain").unwrap();
        assert!(plain.description.is_none());
        assert!(plain.category.is_none());
        assert!(!plain.is_official());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_missing_dir_returns_empty() {
        let missing = std::env::temp_dir().join("aictl-agents-nonexistent-xyz-12345");
        assert!(list_in(&missing, Origin::Global).is_empty());
    }

    #[test]
    fn list_combined_local_overrides_global() {
        let global = unique_temp_dir("combined-global");
        let local = unique_temp_dir("combined-local");
        save_in(&global, "shared", "global body").unwrap();
        save_in(&global, "global-only", "g").unwrap();
        save_in(&local, "shared", "local body").unwrap();
        save_in(&local, "local-only", "l").unwrap();

        let entries = list_combined(&global, Some((local.as_path(), Origin::Local)));
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["global-only", "local-only", "shared"]);

        let shared = entries.iter().find(|e| e.name == "shared").unwrap();
        assert_eq!(shared.origin, Origin::Local);
        assert_eq!(std::fs::read_to_string(&shared.path).unwrap(), "local body");

        let global_only = entries.iter().find(|e| e.name == "global-only").unwrap();
        assert_eq!(global_only.origin, Origin::Global);

        let local_only = entries.iter().find(|e| e.name == "local-only").unwrap();
        assert_eq!(local_only.origin, Origin::Local);

        std::fs::remove_dir_all(&global).ok();
        std::fs::remove_dir_all(&local).ok();
    }

    #[test]
    fn list_combined_without_local_returns_global_only() {
        let global = unique_temp_dir("combined-no-local");
        save_in(&global, "alpha", "a").unwrap();
        let entries = list_combined(&global, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].origin, Origin::Global);
        std::fs::remove_dir_all(&global).ok();
    }

    #[test]
    fn list_in_strips_md_suffix_from_filenames() {
        // Project-local `.aictl/agents/` and `.claude/agents/` follow the
        // `<name>.md` convention. The bare-name format from the global
        // catalogue must keep working alongside it.
        let dir = unique_temp_dir("md-suffix");
        save_in(&dir, "with-md.md", "body 1").unwrap();
        save_in(&dir, "without-md", "body 2").unwrap();
        let entries = list_in(&dir, Origin::Local);
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["with-md", "without-md"]);
        let with = entries.iter().find(|e| e.name == "with-md").unwrap();
        assert_eq!(std::fs::read_to_string(&with.path).unwrap(), "body 1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn origin_label_renders_human_readable() {
        assert_eq!(Origin::Global.label(), "global");
        assert_eq!(Origin::Local.label(), "local");
        assert_eq!(Origin::LocalClaude.label(), "local (claude)");
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
