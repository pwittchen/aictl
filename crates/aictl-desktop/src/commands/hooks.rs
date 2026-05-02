//! Hooks-pane Tauri commands.
//!
//! The webview can list every configured hook, toggle it on/off, and
//! delete it. New hooks are still authored from the CLI's `/hooks`
//! menu — the desktop pane is read/edit, not authoring (the matcher
//! grammar and JSON-stdout protocol are easier to demo from the
//! terminal).

use aictl_core::hooks;
use aictl_core::hooks::{Hook, HookEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize)]
pub struct HookRow {
    /// Numeric index of the hook within its event's array. Used as a
    /// stable handle by toggle/delete; recomputed every time the list
    /// is fetched.
    pub idx: usize,
    pub event: String,
    pub matcher: String,
    pub command: String,
    pub timeout_secs: u64,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct HooksStatus {
    pub config_path: Option<String>,
    pub hooks: Vec<HookRow>,
}

#[tauri::command]
pub fn hooks_status() -> HooksStatus {
    let snapshot = hooks::snapshot();
    let mut rows = Vec::new();
    for ev in HookEvent::ALL {
        if let Some(list) = snapshot.get(ev) {
            for (idx, h) in list.iter().enumerate() {
                rows.push(HookRow {
                    idx,
                    event: ev.as_str().to_string(),
                    matcher: h.matcher.clone(),
                    command: h.command.clone(),
                    timeout_secs: h.timeout_secs,
                    enabled: h.enabled,
                });
            }
        }
    }
    HooksStatus {
        config_path: hooks::hooks_file().map(|p| p.display().to_string()),
        hooks: rows,
    }
}

#[derive(Deserialize)]
pub struct HookMutateArgs {
    pub event: String,
    pub idx: usize,
    pub enabled: Option<bool>,
}

/// Toggle a hook's `enabled` flag, persist the new table, and reload the
/// in-memory cache so subsequent agent turns see the change.
#[tauri::command]
pub fn hook_toggle(args: HookMutateArgs) -> Result<bool, String> {
    let mut map = hooks::snapshot();
    let event = HookEvent::from_str(&args.event)
        .ok_or_else(|| format!("unknown event '{}'", args.event))?;
    let list = map
        .get_mut(&event)
        .ok_or_else(|| format!("no hooks for event {event}"))?;
    let hook = list
        .get_mut(args.idx)
        .ok_or_else(|| format!("hook index {} out of range", args.idx))?;
    let next = args.enabled.unwrap_or(!hook.enabled);
    hook.enabled = next;
    persist(&map)?;
    Ok(next)
}

#[derive(Deserialize)]
pub struct HookDeleteArgs {
    pub event: String,
    pub idx: usize,
}

#[tauri::command]
pub fn hook_delete(args: HookDeleteArgs) -> Result<(), String> {
    let mut map = hooks::snapshot();
    let event = HookEvent::from_str(&args.event)
        .ok_or_else(|| format!("unknown event '{}'", args.event))?;
    let list = map
        .get_mut(&event)
        .ok_or_else(|| format!("no hooks for event {event}"))?;
    if args.idx >= list.len() {
        return Err(format!("hook index {} out of range", args.idx));
    }
    list.remove(args.idx);
    if list.is_empty() {
        map.remove(&event);
    }
    persist(&map)?;
    Ok(())
}

#[derive(Deserialize)]
pub struct HookCreateArgs {
    pub event: String,
    pub matcher: String,
    pub command: String,
    pub timeout_secs: Option<u64>,
}

#[tauri::command]
pub fn hook_create(args: HookCreateArgs) -> Result<(), String> {
    if args.command.trim().is_empty() {
        return Err("command is empty".to_string());
    }
    let event = HookEvent::from_str(&args.event)
        .ok_or_else(|| format!("unknown event '{}'", args.event))?;
    let matcher = if args.matcher.trim().is_empty() {
        "*".to_string()
    } else {
        args.matcher
    };
    let mut map = hooks::snapshot();
    let list = map.entry(event).or_default();
    list.push(Hook {
        event,
        matcher,
        command: args.command,
        timeout_secs: args
            .timeout_secs
            .unwrap_or(hooks::DEFAULT_HOOK_TIMEOUT_SECS),
        enabled: true,
    });
    persist(&map)?;
    Ok(())
}

fn persist(map: &HashMap<HookEvent, Vec<Hook>>) -> Result<(), String> {
    hooks::save(map)?;
    hooks::replace(map.clone());
    Ok(())
}
