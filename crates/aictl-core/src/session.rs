use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::security::redaction::{self, RedactionPolicy};
use crate::{Message, Role};

pub struct Session {
    pub id: String,
    pub name: Option<String>,
}

static CURRENT: Mutex<Option<Session>> = Mutex::new(None);
static INCOGNITO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_incognito(on: bool) {
    INCOGNITO.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_incognito() -> bool {
    INCOGNITO.load(std::sync::atomic::Ordering::Relaxed)
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

pub fn sessions_dir() -> Option<PathBuf> {
    let h = home()?;
    let p = PathBuf::from(format!("{h}/.aictl/sessions"));
    let _ = fs::create_dir_all(&p);
    Some(p)
}

fn names_path() -> Option<PathBuf> {
    Some(sessions_dir()?.join(".names"))
}

pub fn session_file(id: &str) -> Option<PathBuf> {
    Some(sessions_dir()?.join(id))
}

fn read_names_at(path: &Path) -> Vec<(String, String)> {
    let Ok(c) = fs::read_to_string(path) else {
        return vec![];
    };
    c.lines()
        .filter_map(|l| {
            l.split_once('\t')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
        .collect()
}

fn write_names_at(path: &Path, entries: &[(String, String)]) {
    let s = entries.iter().fold(String::new(), |mut acc, (a, b)| {
        let _ = writeln!(acc, "{a}\t{b}");
        acc
    });
    let _ = fs::write(path, s);
}

fn read_names() -> Vec<(String, String)> {
    let Some(p) = names_path() else {
        return vec![];
    };
    read_names_at(&p)
}

fn write_names(entries: &[(String, String)]) {
    let Some(p) = names_path() else {
        return;
    };
    write_names_at(&p, entries);
}

pub fn name_for(id: &str) -> Option<String> {
    read_names()
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, n)| n)
}

pub fn id_for_name(name: &str) -> Option<String> {
    read_names()
        .into_iter()
        .find(|(_, n)| n == name)
        .map(|(i, _)| i)
}

/// Err if name is invalid or already used by a different id.
/// Names are normalized to lowercase; only `[a-z0-9_]` are allowed.
pub fn set_name(id: &str, name: &str) -> Result<(), String> {
    let name = normalize_name(name)?;
    let mut entries = read_names();
    if let Some((other_id, _)) = entries.iter().find(|(i, n)| *n == name && i != id) {
        return Err(format!("name already used by session {other_id}"));
    }
    entries.retain(|(i, _)| i != id);
    entries.push((id.to_string(), name.clone()));
    write_names(&entries);
    if let Some(c) = CURRENT.lock().unwrap().as_mut()
        && c.id == id
    {
        c.name = Some(name);
    }
    Ok(())
}

fn normalize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name cannot be empty".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(
            "name may only contain letters, numbers, and underscores (no spaces or special characters)"
                .to_string(),
        );
    }
    Ok(name.to_ascii_lowercase())
}

pub fn remove_name(id: &str) {
    let mut entries = read_names();
    entries.retain(|(i, _)| i != id);
    write_names(&entries);
}

pub fn generate_uuid() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    } else {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((nanos >> (i * 8)) & 0xff) as u8;
        }
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn role_str(r: &Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn role_from(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}

/// Build the on-disk JSON for a session with persistence-boundary
/// redaction applied. Pulled out of [`save_messages_at`] so a test can
/// drive it with an explicit policy without touching the global one.
fn build_session_json(id: &str, messages: &[Message], pol: &RedactionPolicy) -> Value {
    let arr: Vec<Value> = messages
        .iter()
        .map(|m| {
            let scrubbed = redaction::redact_for_persistence(&m.content, pol);
            let body = scrubbed.as_deref().unwrap_or(&m.content);
            json!({"role": role_str(&m.role), "content": body})
        })
        .collect();
    json!({"id": id, "messages": arr})
}

fn save_messages_at(path: &Path, id: &str, messages: &[Message]) {
    // Persistence-boundary redaction: when the policy is `redact` or
    // `block`, scrub each message body before it lands on disk so the
    // session file mirrors what the network seam would have sent.
    // When the policy is `off` (the default) the helper returns None
    // for every message and we serialize the original content.
    let pol = redaction::policy();
    let v = build_session_json(id, messages, &pol);
    let _ = fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
}

fn load_messages_at(path: &Path) -> Result<Vec<Message>, String> {
    let c = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&c).map_err(|e| e.to_string())?;
    let arr = v
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or("invalid session file")?;
    Ok(arr
        .iter()
        .filter_map(|m| {
            let role = m.get("role")?.as_str()?;
            let content = m.get("content")?.as_str()?;
            Some(Message {
                role: role_from(role),
                content: content.to_string(),
                images: vec![],
            })
        })
        .collect())
}

pub fn save_messages(id: &str, messages: &[Message]) {
    let Some(p) = session_file(id) else {
        return;
    };
    save_messages_at(&p, id, messages);
}

pub fn load_messages(id: &str) -> Result<Vec<Message>, String> {
    let p = session_file(id).ok_or("no sessions directory")?;
    load_messages_at(&p)
}

pub struct SessionEntry {
    pub id: String,
    pub name: Option<String>,
    pub size: u64,
    pub mtime: std::time::SystemTime,
}

pub fn list_sessions() -> Vec<SessionEntry> {
    let Some(dir) = sessions_dir() else {
        return vec![];
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let fname = e.file_name().to_string_lossy().into_owned();
        if fname.starts_with('.') {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        out.push(SessionEntry {
            id: fname.clone(),
            name: name_for(&fname),
            size: meta.len(),
            mtime: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
        });
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    out
}

/// Resolve a user-provided key (uuid or name) to a session id.
pub fn resolve(key: &str) -> Option<String> {
    if let Some(p) = session_file(key)
        && p.exists()
    {
        return Some(key.to_string());
    }
    id_for_name(key)
}

pub fn delete_session(id: &str) {
    if let Some(p) = session_file(id) {
        let _ = fs::remove_file(&p);
    }
    remove_name(id);
}

pub fn clear_all() {
    let Some(dir) = sessions_dir() else {
        return;
    };
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let _ = fs::remove_file(e.path());
        }
    }
}

pub fn set_current(id: String, name: Option<String>) {
    *CURRENT.lock().unwrap() = Some(Session { id, name });
}

pub fn current_info() -> Option<(String, Option<String>)> {
    CURRENT
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| (s.id.clone(), s.name.clone()))
}

pub fn current_id() -> Option<String> {
    CURRENT.lock().unwrap().as_ref().map(|s| s.id.clone())
}

pub fn save_current(messages: &[Message]) {
    if is_incognito() {
        return;
    }
    if let Some(id) = current_id() {
        save_messages(&id, messages);
    }
}

pub fn current_file_size() -> u64 {
    let Some(id) = current_id() else { return 0 };
    session_file(&id)
        .and_then(|p| fs::metadata(p).ok())
        .map_or(0, |m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tests that touch the global `CURRENT` / `INCOGNITO` state.
    static STATE_LOCK: Mutex<()> = Mutex::new(());

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("aictl-session-test-{tag}-{pid}-{nanos}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn generate_uuid_v4_format() {
        let id = generate_uuid();
        assert_eq!(id.len(), 36);
        let bytes = id.as_bytes();
        // Hyphen positions per 8-4-4-4-12 grouping.
        assert_eq!(bytes[8], b'-');
        assert_eq!(bytes[13], b'-');
        assert_eq!(bytes[18], b'-');
        assert_eq!(bytes[23], b'-');
        // Version 4 nibble.
        assert_eq!(bytes[14], b'4');
        // Variant nibble must be one of 8/9/a/b.
        assert!(matches!(bytes[19], b'8' | b'9' | b'a' | b'b'));
        // All other chars are lowercase hex or '-'.
        assert!(
            id.chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }

    #[test]
    fn generate_uuid_values_are_unique() {
        let a = generate_uuid();
        let b = generate_uuid();
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_name_lowercases_and_accepts_allowed_chars() {
        assert_eq!(normalize_name("Alpha_123").unwrap(), "alpha_123");
        assert_eq!(normalize_name("  spaced_ok  ").unwrap(), "spaced_ok");
        assert_eq!(normalize_name("ABC").unwrap(), "abc");
    }

    #[test]
    fn normalize_name_rejects_empty_and_invalid_chars() {
        assert!(normalize_name("").is_err());
        assert!(normalize_name("   ").is_err());
        assert!(normalize_name("with space").is_err());
        assert!(normalize_name("with-dash").is_err());
        assert!(normalize_name("dot.name").is_err());
        assert!(normalize_name("unicode-π").is_err());
    }

    #[test]
    fn role_serialization_roundtrip() {
        assert!(matches!(role_from(role_str(&Role::System)), Role::System));
        assert!(matches!(role_from(role_str(&Role::User)), Role::User));
        assert!(matches!(
            role_from(role_str(&Role::Assistant)),
            Role::Assistant
        ));
        // Unknown strings default to User.
        assert!(matches!(role_from("unknown"), Role::User));
    }

    #[test]
    fn build_session_json_redacts_when_policy_is_redact() {
        // Persistence-seam guard: a Redact (or Block) policy must
        // scrub message bodies before they reach disk, even though
        // the in-memory `Vec<Message>` is left untouched.
        let messages = vec![
            Message {
                role: Role::System,
                content: "system prompt — boring".to_string(),
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "my key is sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
                images: vec![],
            },
        ];
        let pol = RedactionPolicy {
            mode: redaction::RedactionMode::Redact,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        };
        let v = build_session_json("abc", &messages, &pol);
        let body = serde_json::to_string(&v).unwrap();
        assert!(body.contains("[REDACTED:API_KEY]"));
        assert!(!body.contains("sk-proj-"));
        // System message stays as-is.
        assert!(body.contains("system prompt — boring"));
    }

    #[test]
    fn build_session_json_passes_through_when_policy_is_off() {
        let messages = vec![Message {
            role: Role::User,
            content: "raw secret sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb".to_string(),
            images: vec![],
        }];
        let pol = RedactionPolicy {
            mode: redaction::RedactionMode::Off,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        };
        let v = build_session_json("abc", &messages, &pol);
        let body = serde_json::to_string(&v).unwrap();
        assert!(body.contains("sk-proj-"));
        assert!(!body.contains("[REDACTED:"));
    }

    #[test]
    fn save_and_load_messages_roundtrip() {
        let dir = unique_temp_dir("msg");
        let path = dir.join("abc");
        let messages = vec![
            Message {
                role: Role::System,
                content: "sys prompt".to_string(),
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "hello".to_string(),
                images: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "hi there".to_string(),
                images: vec![],
            },
        ];
        save_messages_at(&path, "abc", &messages);

        let loaded = load_messages_at(&path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert!(matches!(loaded[0].role, Role::System));
        assert_eq!(loaded[0].content, "sys prompt");
        assert!(matches!(loaded[1].role, Role::User));
        assert_eq!(loaded[1].content, "hello");
        assert!(matches!(loaded[2].role, Role::Assistant));
        assert_eq!(loaded[2].content, "hi there");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_messages_missing_file_errors() {
        let dir = unique_temp_dir("missing");
        let path = dir.join("nope");
        assert!(load_messages_at(&path).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_messages_malformed_json_errors() {
        let dir = unique_temp_dir("bad-json");
        let path = dir.join("x");
        fs::write(&path, "not json").unwrap();
        assert!(load_messages_at(&path).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_messages_missing_messages_field_errors() {
        let dir = unique_temp_dir("no-messages");
        let path = dir.join("x");
        fs::write(&path, "{\"id\": \"abc\"}").unwrap();
        assert!(load_messages_at(&path).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn names_file_read_write_roundtrip() {
        let dir = unique_temp_dir("names");
        let path = dir.join(".names");
        let entries = vec![
            ("id-one".to_string(), "alpha".to_string()),
            ("id-two".to_string(), "beta".to_string()),
        ];
        write_names_at(&path, &entries);
        assert_eq!(read_names_at(&path), entries);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn names_file_missing_returns_empty() {
        let dir = unique_temp_dir("names-missing");
        let path = dir.join(".names");
        assert!(read_names_at(&path).is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn names_file_skips_lines_without_tab() {
        let dir = unique_temp_dir("names-malformed");
        let path = dir.join(".names");
        fs::write(&path, "good-id\tgood-name\nbad-line-without-tab\n").unwrap();
        let got = read_names_at(&path);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "good-id");
        assert_eq!(got[0].1, "good-name");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incognito_toggle() {
        let _guard = STATE_LOCK.lock().unwrap();
        let prior = is_incognito();
        set_incognito(true);
        assert!(is_incognito());
        set_incognito(false);
        assert!(!is_incognito());
        set_incognito(prior);
    }

    #[test]
    fn set_and_read_current_session() {
        let _guard = STATE_LOCK.lock().unwrap();
        let prior = CURRENT.lock().unwrap().take();

        set_current("abc-123".to_string(), Some("mytag".to_string()));
        assert_eq!(current_id(), Some("abc-123".to_string()));
        assert_eq!(
            current_info(),
            Some(("abc-123".to_string(), Some("mytag".to_string())))
        );

        set_current("xyz".to_string(), None);
        assert_eq!(current_id(), Some("xyz".to_string()));
        assert_eq!(current_info(), Some(("xyz".to_string(), None)));

        *CURRENT.lock().unwrap() = prior;
    }

    #[test]
    fn save_current_noop_when_incognito() {
        let _guard = STATE_LOCK.lock().unwrap();
        let prior_incognito = is_incognito();
        let prior_current = CURRENT.lock().unwrap().take();

        set_incognito(true);
        // Point CURRENT at an id that would map to a sessions-dir file if
        // persistence were attempted; incognito must prevent the write.
        set_current("nonexistent-incognito-id".to_string(), None);
        save_current(&[]);
        if let Some(p) = session_file("nonexistent-incognito-id") {
            assert!(!p.exists());
        }

        set_incognito(prior_incognito);
        *CURRENT.lock().unwrap() = prior_current;
    }
}
