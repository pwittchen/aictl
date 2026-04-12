use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

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

fn read_names() -> Vec<(String, String)> {
    let Some(p) = names_path() else {
        return vec![];
    };
    let Ok(c) = fs::read_to_string(&p) else {
        return vec![];
    };
    c.lines()
        .filter_map(|l| {
            l.split_once('\t')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
        .collect()
}

fn write_names(entries: &[(String, String)]) {
    let Some(p) = names_path() else {
        return;
    };
    let s = entries.iter().fold(String::new(), |mut acc, (a, b)| {
        let _ = writeln!(acc, "{a}\t{b}");
        acc
    });
    let _ = fs::write(&p, s);
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

pub fn save_messages(id: &str, messages: &[Message]) {
    let Some(p) = session_file(id) else {
        return;
    };
    let arr: Vec<Value> = messages
        .iter()
        .map(|m| json!({"role": role_str(&m.role), "content": m.content}))
        .collect();
    let v = json!({"id": id, "messages": arr});
    let _ = fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default());
}

pub fn load_messages(id: &str) -> Result<Vec<Message>, String> {
    let p = session_file(id).ok_or("no sessions directory")?;
    let c = fs::read_to_string(&p).map_err(|e| e.to_string())?;
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
