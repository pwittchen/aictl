//! Skills — one-turn markdown playbooks invoked via `/<skill-name>` or
//! `--skill <name>`. Unlike [`crate::agents`], a skill is scoped to a single
//! agent turn: its body is injected as a transient system message for the
//! in-flight LLM call(s) and is never persisted into session history.
//!
//! Each skill lives at `~/.aictl/skills/<name>/SKILL.md` (overridable via
//! `AICTL_SKILLS_DIR`) with YAML-ish frontmatter (`name`, `description`,
//! optional `source` and `category`) and a markdown body describing the
//! procedure. This module only handles parsing, CRUD, and name validation —
//! the runtime injection happens in [`crate::run::run_agent_turn`].

use std::path::{Path, PathBuf};

use crate::config::config_get;

pub mod remote;

/// Frontmatter value marking a skill as sourced from the first-party
/// catalogue at <https://github.com/pwittchen/aictl/tree/master/.aictl/skills>.
/// Badged as `[official]` in the REPL and `--list-skills` output. Anything
/// else (missing, `user`, or a user-supplied string) renders without the
/// badge.
pub const OFFICIAL_SOURCE: &str = "aictl-official";

/// A loaded skill, ready to be injected as a transient system message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// An entry in the skill listing (metadata only — body not loaded).
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub source: Option<String>,
    pub category: Option<String>,
}

impl SkillEntry {
    /// True if this skill was pulled from the first-party catalogue.
    pub fn is_official(&self) -> bool {
        self.source.as_deref() == Some(OFFICIAL_SOURCE)
    }
}

/// Validate a skill name: only letters, numbers, underscore, dash. Matches
/// [`crate::agents::is_valid_name`] exactly — duplicated rather than shared
/// to avoid a one-function abstraction.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Slash-command names a skill is forbidden from taking, so `/<skill-name>`
/// can never shadow a built-in. Keep in sync with [`crate::commands::COMMANDS`]
/// plus the bare `exit`/`quit` shortcuts the REPL recognizes.
const RESERVED_NAMES: &[&str] = &[
    "agent",
    "behavior",
    "clear",
    "compact",
    "config",
    "context",
    "copy",
    "exit",
    "gguf",
    "help",
    "history",
    "info",
    "keys",
    "memory",
    "mlx",
    "model",
    "ping",
    "quit",
    "retry",
    "security",
    "session",
    "skills",
    "stats",
    "tools",
    "uninstall",
    "update",
    "version",
];

/// Return true if `name` collides with a built-in slash command.
pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_NAMES.contains(&name)
}

/// Return the skills directory path — `AICTL_SKILLS_DIR` when set, otherwise
/// `~/.aictl/skills/`.
pub fn skills_dir() -> PathBuf {
    if let Some(dir) = config_get("AICTL_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/skills"))
}

/// Path to a skill's SKILL.md, given the root skills directory.
fn skill_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(name).join("SKILL.md")
}

/// Parsed frontmatter + body.
pub(crate) struct Parsed {
    pub name: Option<String>,
    pub description: Option<String>,
    pub source: Option<String>,
    pub category: Option<String>,
    pub body: String,
}

/// Parse a SKILL.md. Recognizes `name`, `description`, `source`, `category`;
/// unknown fields are silently ignored so forward-compatible additions don't
/// break older clients. If there is no frontmatter fence at the start of the
/// file the entire content is treated as the body.
pub(crate) fn parse(contents: &str) -> Parsed {
    let mut name = None;
    let mut description = None;
    let mut source = None;
    let mut category = None;
    let trimmed = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let Some(after_open) = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    else {
        return Parsed {
            name,
            description,
            source,
            category,
            body: contents.to_string(),
        };
    };
    // Find the closing fence. It must be a `---` on its own line.
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
            match key {
                "name" => name = Some(value.to_string()),
                "description" => description = Some(value.to_string()),
                "source" => source = Some(value.to_string()),
                "category" => category = Some(value.to_string()),
                _ => {}
            }
        }
        cursor += line.len();
    }
    let Some(start) = body_start else {
        // Unterminated frontmatter → treat whole content as body.
        return Parsed {
            name: None,
            description: None,
            source: None,
            category: None,
            body: contents.to_string(),
        };
    };
    let body = after_open[start..]
        .trim_start_matches(['\n', '\r'])
        .to_string();
    Parsed {
        name,
        description,
        source,
        category,
        body,
    }
}

/// Serialize a skill back to markdown with frontmatter.
fn serialize(name: &str, description: &str, body: &str) -> String {
    let trimmed_body = body.trim_end();
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{trimmed_body}\n")
}

/// Load a skill by name from `dir`. Returns None if the directory, file, or
/// required fields are missing.
fn find_in(dir: &Path, name: &str) -> Option<Skill> {
    if !is_valid_name(name) {
        return None;
    }
    let contents = std::fs::read_to_string(skill_file(dir, name)).ok()?;
    let parsed = parse(&contents);
    // Directory name is authoritative; skip entries where the frontmatter
    // claims a different identity to avoid silent drift.
    if let Some(fm_name) = parsed.name.as_deref()
        && fm_name != name
    {
        return None;
    }
    Some(Skill {
        name: name.to_string(),
        description: parsed.description.unwrap_or_default(),
        body: parsed.body,
    })
}

/// List valid skill directories in `dir`, sorted alphabetically. Each entry
/// has its frontmatter parsed so callers can render the `[official]` badge
/// or filter by category without re-reading every file.
fn list_in(dir: &Path) -> Vec<SkillEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut skills: Vec<SkillEntry> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            if !e.file_type().ok()?.is_dir() {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if !is_valid_name(&name) {
                return None;
            }
            let contents = std::fs::read_to_string(skill_file(dir, &name)).ok()?;
            let parsed = parse(&contents);
            // Directory name is authoritative — skip on identity drift.
            if let Some(fm_name) = parsed.name.as_deref()
                && fm_name != name
            {
                return None;
            }
            Some(SkillEntry {
                name,
                description: parsed.description.unwrap_or_default(),
                source: parsed.source,
                category: parsed.category,
            })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Save a skill to `dir`, creating the directory if needed.
fn save_in(dir: &Path, name: &str, description: &str, body: &str) -> std::io::Result<()> {
    let skill_dir = dir.join(name);
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(
        skill_dir.join("SKILL.md"),
        serialize(name, description, body),
    )
}

/// Delete a skill directory from `dir`.
fn delete_in(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::remove_dir_all(dir.join(name))
}

/// Find a skill by name.
pub fn find(name: &str) -> Option<Skill> {
    find_in(&skills_dir(), name)
}

/// List all saved skills, sorted alphabetically.
pub fn list() -> Vec<SkillEntry> {
    list_in(&skills_dir())
}

/// Save a skill to disk. Validates the name and rejects reserved names.
pub fn save(name: &str, description: &str, body: &str) -> std::io::Result<()> {
    if !is_valid_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid skill name (use only letters, numbers, underscore, or dash)",
        ));
    }
    if is_reserved_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("\"{name}\" is a reserved slash-command name"),
        ));
    }
    save_in(&skills_dir(), name, description, body)
}

/// Delete a skill by name.
pub fn delete(name: &str) -> std::io::Result<()> {
    delete_in(&skills_dir(), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("aictl-skills-test-{tag}-{pid}-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn is_valid_name_matches_agents_rules() {
        assert!(is_valid_name("commit"));
        assert!(is_valid_name("summarize_logs"));
        assert!(is_valid_name("skill-2"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("has space"));
        assert!(!is_valid_name("dot.name"));
    }

    #[test]
    fn reserved_names_include_built_in_commands() {
        assert!(is_reserved_name("help"));
        assert!(is_reserved_name("exit"));
        assert!(is_reserved_name("quit"));
        assert!(is_reserved_name("skills"));
        assert!(!is_reserved_name("commit"));
        assert!(!is_reserved_name("review"));
    }

    #[test]
    fn parse_reads_name_and_description() {
        let src = "---\nname: commit\ndescription: Commit staged changes.\n---\n\nBody here.\n";
        let p = parse(src);
        assert_eq!(p.name.as_deref(), Some("commit"));
        assert_eq!(p.description.as_deref(), Some("Commit staged changes."));
        assert_eq!(p.body, "Body here.\n");
    }

    #[test]
    fn parse_reads_source_and_category() {
        let src = "---\nname: x\ndescription: y\nsource: aictl-official\ncategory: dev\n---\n\nB\n";
        let p = parse(src);
        assert_eq!(p.name.as_deref(), Some("x"));
        assert_eq!(p.description.as_deref(), Some("y"));
        assert_eq!(p.source.as_deref(), Some("aictl-official"));
        assert_eq!(p.category.as_deref(), Some("dev"));
        assert_eq!(p.body, "B\n");
    }

    #[test]
    fn parse_ignores_unknown_fields() {
        let src = "---\nname: x\ndescription: y\nextra: ignored\n---\n\nB\n";
        let p = parse(src);
        assert_eq!(p.name.as_deref(), Some("x"));
        assert_eq!(p.description.as_deref(), Some("y"));
        assert_eq!(p.source, None);
        assert_eq!(p.category, None);
        assert_eq!(p.body, "B\n");
    }

    #[test]
    fn parse_without_frontmatter_treats_all_as_body() {
        let src = "just the body, no fences\nline two\n";
        let p = parse(src);
        assert_eq!(p.name, None);
        assert_eq!(p.description, None);
        assert_eq!(p.source, None);
        assert_eq!(p.category, None);
        assert_eq!(p.body, src);
    }

    #[test]
    fn parse_handles_quoted_values() {
        let src = "---\nname: \"quoted\"\ndescription: 'single'\n---\nbody\n";
        let p = parse(src);
        assert_eq!(p.name.as_deref(), Some("quoted"));
        assert_eq!(p.description.as_deref(), Some("single"));
    }

    #[test]
    fn parse_unterminated_frontmatter_falls_back_to_whole_body() {
        // No closing `---` fence — everything goes to `body`.
        let src = "---\nname: x\nno closing fence here\n";
        let p = parse(src);
        assert!(p.name.is_none());
        assert!(p.source.is_none());
        assert_eq!(p.body, src);
    }

    #[test]
    fn save_find_roundtrip() {
        let dir = unique_temp_dir("rt");
        save_in(&dir, "commit", "Commit staged changes.", "Body line.").unwrap();
        let found = find_in(&dir, "commit").expect("skill should load");
        assert_eq!(found.name, "commit");
        assert_eq!(found.description, "Commit staged changes.");
        assert!(found.body.starts_with("Body line."));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_rejects_name_dir_mismatch() {
        let dir = unique_temp_dir("mismatch");
        let skill_dir = dir.join("real-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let contents = serialize("different-name", "d", "b");
        std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
        assert!(find_in(&dir, "real-name").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_without_frontmatter_uses_directory_name() {
        let dir = unique_temp_dir("nofm");
        let skill_dir = dir.join("plain");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "just body, no fences").unwrap();
        let s = find_in(&dir, "plain").expect("should load");
        assert_eq!(s.name, "plain");
        assert_eq!(s.description, "");
        assert_eq!(s.body, "just body, no fences");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_sorts_and_filters() {
        let dir = unique_temp_dir("list");
        save_in(&dir, "zebra", "z", "bz").unwrap();
        save_in(&dir, "alpha", "a", "ba").unwrap();
        save_in(&dir, "mango", "m", "bm").unwrap();
        // Invalid name directory — skipped.
        std::fs::create_dir(dir.join("bad name")).unwrap();
        // Valid name but no SKILL.md — skipped.
        std::fs::create_dir(dir.join("empty")).unwrap();
        let names: Vec<_> = list_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["alpha", "mango", "zebra"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_populates_frontmatter_fields() {
        let dir = unique_temp_dir("list-meta");
        let pulled_dir = dir.join("pulled");
        std::fs::create_dir_all(&pulled_dir).unwrap();
        std::fs::write(
            pulled_dir.join("SKILL.md"),
            "---\nname: pulled\ndescription: official one\nsource: aictl-official\ncategory: dev\n---\n\nBody.\n",
        )
        .unwrap();
        save_in(&dir, "plain", "hand written", "Body.").unwrap();

        let entries = list_in(&dir);
        let pulled = entries.iter().find(|e| e.name == "pulled").unwrap();
        assert_eq!(pulled.description, "official one");
        assert_eq!(pulled.category.as_deref(), Some("dev"));
        assert!(pulled.is_official());

        let plain = entries.iter().find(|e| e.name == "plain").unwrap();
        assert_eq!(plain.description, "hand written");
        assert!(plain.category.is_none());
        assert!(!plain.is_official());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_missing_dir_returns_empty() {
        let missing = std::env::temp_dir().join("aictl-skills-nonexistent-zzz-000");
        assert!(list_in(&missing).is_empty());
    }

    #[test]
    fn delete_removes_directory() {
        let dir = unique_temp_dir("del");
        save_in(&dir, "tmp", "d", "b").unwrap();
        delete_in(&dir, "tmp").unwrap();
        assert!(find_in(&dir, "tmp").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
