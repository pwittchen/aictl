//! Chat-side Tauri commands: send a message, stop the in-flight turn,
//! resolve a tool-approval prompt.

use std::sync::Arc;

use aictl_core::ToolApproval;
use aictl_core::message::{Message, Role};
use aictl_core::run;
use aictl_core::session;
use aictl_core::transcript;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::chat::{self, ChatRequest};
use crate::state::AppState;
use crate::workspace;

#[derive(Deserialize)]
pub struct SendMessageArgs {
    pub text: String,
    /// `true` when the composer's mode picker is "auto-accept tools" —
    /// the agent loop skips `confirm_tool_async` and emits `show_auto_tool`
    /// directly. Active session lives in [`AppState::session_id`]; the
    /// frontend no longer threads a session id through every send.
    #[serde(default)]
    pub auto_accept: bool,
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    args: SendMessageArgs,
) -> Result<(), String> {
    if args.text.trim().is_empty() {
        return Err("message is empty".to_string());
    }
    // Workspace gate. Without a pinned folder the security policy's
    // CWD jail short-circuits every CWD-relative tool call (plan §5.4),
    // so we refuse to even start the turn — the composer should already
    // be locked in this state, but a defensive check here keeps the
    // engine from being called out of contract.
    if !workspace::is_set() {
        return Err(
            "no workspace selected — pick a folder in Settings → Workspace before sending a message"
                .to_string(),
        );
    }

    // Replace any previously running turn's cancel handle with a fresh
    // one. `stop_turn` reads this same slot.
    let cancel = CancellationToken::new();
    {
        let mut slot = state
            .turn_cancel
            .lock()
            .map_err(|_| "turn-cancel mutex poisoned".to_string())?;
        if let Some(prev) = slot.take() {
            // If a previous turn is somehow still in flight, cancel it
            // first so the engine doesn't see two concurrent runs.
            prev.cancel();
        }
        *slot = Some(cancel.clone());
    }

    let req = ChatRequest {
        user_message: args.text,
        auto_accept: args.auto_accept,
    };
    let state_clone: Arc<AppState> = Arc::clone(&state);
    // The agent loop holds an `&dyn AgentUI` across `.await` points. The
    // CLI's `AgentUI` impls (and ours) intentionally aren't `Sync` — they
    // hold `Cell` / `Mutex` / `AppHandle` state that's only ever touched
    // from one thread — so spawning on the multi-threaded Tauri runtime
    // would surface `dyn AgentUI: !Sync`. Driving the turn on a dedicated
    // OS thread with its own current-thread tokio runtime sidesteps the
    // constraint without changing the engine's trait shape.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[aictl-desktop] failed to build agent-turn runtime: {e}");
                return;
            }
        };
        rt.block_on(chat::run_turn(app, state_clone, cancel, req));
    });
    Ok(())
}

#[tauri::command]
pub fn stop_turn(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let token = state
        .turn_cancel
        .lock()
        .map_err(|_| "turn-cancel mutex poisoned".to_string())?
        .take();
    if let Some(token) = token {
        token.cancel();
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ToolApprovalResponse {
    pub id: u64,
    pub decision: String,
}

#[tauri::command]
pub fn tool_approval_response(
    state: State<'_, Arc<AppState>>,
    args: ToolApprovalResponse,
) -> Result<(), String> {
    let approval = match args.decision.as_str() {
        "allow" => ToolApproval::Allow,
        "auto_accept" | "always_allow" => ToolApproval::AutoAccept,
        "deny" => ToolApproval::Deny,
        other => return Err(format!("unknown decision '{other}'")),
    };
    let tx = state
        .pending_approvals
        .lock()
        .map_err(|_| "approval map poisoned".to_string())?
        .remove(&args.id);
    let Some(tx) = tx else {
        return Err(format!("no pending approval with id {}", args.id));
    };
    let _ = tx.send(approval);
    Ok(())
}

/// Result returned by `clear_chat` / `retry_last` / `undo_last`. The
/// frontend re-renders the message list from `messages`; `prompt` is set
/// by retry so the composer can pre-fill with the resubmitted text.
#[derive(Serialize)]
pub struct TranscriptUpdate {
    pub messages: Vec<TranscriptMessage>,
    pub prompt: Option<String>,
    pub popped: usize,
}

#[derive(Serialize)]
pub struct TranscriptMessage {
    pub kind: String,
    pub text: String,
}

fn project(messages: &[Message]) -> Vec<TranscriptMessage> {
    messages
        .iter()
        .map(|m| match m.role {
            Role::System => TranscriptMessage {
                kind: "system".to_string(),
                text: m.content.clone(),
            },
            Role::User => {
                let trimmed = m.content.trim_start();
                let kind = if trimmed.starts_with("<tool_result>") {
                    "tool_result"
                } else {
                    "user"
                };
                TranscriptMessage {
                    kind: kind.to_string(),
                    text: m.content.clone(),
                }
            }
            Role::Assistant => TranscriptMessage {
                kind: "assistant".to_string(),
                text: m.content.clone(),
            },
        })
        .collect()
}

/// Persist `messages` back to the active session file (no-op when
/// incognito or when there is no active session).
fn persist(state: &AppState, messages: &[Message]) {
    if *state.incognito.lock().expect("incognito mutex poisoned") {
        return;
    }
    if let Some(id) = state.session_id.lock().ok().and_then(|s| s.clone()) {
        session::save_messages(&id, messages);
    }
}

/// Drop everything except the system prompt. Does **not** delete the
/// session file — the next turn writes a fresh transcript over the top.
/// Mirrors the CLI's `/clear` semantics.
#[tauri::command]
pub fn clear_chat(state: State<'_, Arc<AppState>>) -> Result<TranscriptUpdate, String> {
    let mut msgs = state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())?;
    let system = msgs
        .iter()
        .find(|m| matches!(m.role, Role::System))
        .cloned()
        .unwrap_or_else(|| Message {
            role: Role::System,
            content: run::build_system_prompt(),
            images: vec![],
        });
    *msgs = vec![system];
    let projected = project(&msgs);
    persist(&state, &msgs);
    Ok(TranscriptUpdate {
        messages: projected,
        prompt: None,
        popped: 0,
    })
}

/// Pop the most recent user/assistant exchange and surface the original
/// prompt back to the composer so the user can edit-and-resend (or just
/// hit ↩ to fire the same prompt again). Returns the updated transcript
/// alongside the prompt; the webview clears the composer of any
/// half-typed text and writes `prompt` in.
#[tauri::command]
pub fn retry_last(state: State<'_, Arc<AppState>>) -> Result<TranscriptUpdate, String> {
    let mut msgs = state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())?;
    let prompt =
        transcript::retry_last_exchange(&mut msgs).ok_or_else(|| "nothing to retry".to_string())?;
    let projected = project(&msgs);
    persist(&state, &msgs);
    Ok(TranscriptUpdate {
        messages: projected,
        prompt: Some(prompt),
        popped: 1,
    })
}

#[derive(Deserialize)]
pub struct UndoArgs {
    /// Number of turns to peel off the end. Defaults to 1 to match
    /// `/undo` with no argument; the sidebar lets the user request more.
    #[serde(default = "default_undo_n")]
    pub n: usize,
}

fn default_undo_n() -> usize {
    1
}

#[tauri::command]
pub fn undo_last(
    state: State<'_, Arc<AppState>>,
    args: UndoArgs,
) -> Result<TranscriptUpdate, String> {
    let n = args.n.max(1);
    let mut msgs = state
        .messages
        .lock()
        .map_err(|_| "messages mutex poisoned".to_string())?;
    let popped = transcript::undo_turns(&mut msgs, n);
    if popped == 0 {
        return Err("nothing to undo".to_string());
    }
    let projected = project(&msgs);
    persist(&state, &msgs);
    Ok(TranscriptUpdate {
        messages: projected,
        prompt: None,
        popped,
    })
}
