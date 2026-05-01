//! Chat-side Tauri commands: send a message, stop the in-flight turn,
//! resolve a tool-approval prompt.

use std::sync::Arc;

use aictl_core::ToolApproval;
use serde::Deserialize;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::chat::{self, ChatRequest};
use crate::state::AppState;
use crate::workspace;

#[derive(Deserialize)]
pub struct SendMessageArgs {
    pub text: String,
    pub session_id: Option<String>,
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
        session_id: args.session_id,
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
