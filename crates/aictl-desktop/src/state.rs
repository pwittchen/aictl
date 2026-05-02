//! Process-wide state for the desktop app.
//!
//! Tauri commands receive `tauri::State<Arc<AppState>>`; everything that
//! needs to live across IPC calls (the active turn's cancellation token,
//! pending tool-approval oneshots, the active session id, the
//! conversation transcript) lives here so commands stay short and free
//! of `OnceLock` plumbing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use aictl_core::ToolApproval;
use aictl_core::message::Message;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Map key for an outstanding tool-approval request. The webview
/// receives the id in [`aictl_core::ui::events::AgentEvent::ToolApprovalRequest`]
/// and echoes it back via [`crate::commands::chat::tool_approval_response`].
pub type ApprovalId = u64;

/// Shared, send-safe application state. One instance is constructed in
/// [`crate::run`] and `app.manage`d so every Tauri command can pull it
/// out of the resolver.
pub struct AppState {
    /// Cancellation token of the in-flight agent turn, if any. Replaced
    /// on every `send_message` call; `stop_turn` cancels and clears it.
    pub turn_cancel: Mutex<Option<CancellationToken>>,
    /// Active session id used by `send_message` to load and persist
    /// conversation history. `None` when the user is in incognito mode
    /// or has not yet started a session.
    pub session_id: Mutex<Option<String>>,
    /// Conversation transcript carried across turns within the same
    /// session. The agent loop mutates this in place; on entry to
    /// `send_message` we either reuse the in-memory copy (same session
    /// id) or rehydrate from `~/.aictl/sessions/<id>` (new selection).
    /// Cleared by `clear_chat` and trimmed by retry/undo.
    pub messages: Mutex<Vec<Message>>,
    /// `true` after the user toggled "new incognito session" — disables
    /// `session::save_messages` so nothing lands on disk for the duration
    /// of that conversation. The flag is also mirrored into
    /// `session::set_incognito` so the engine's own paths (compaction,
    /// retry, audit log) honour it.
    pub incognito: Mutex<bool>,
    /// In-flight tool-approval requests, keyed by id.
    pub pending_approvals: Mutex<HashMap<ApprovalId, oneshot::Sender<ToolApproval>>>,
    /// Name of the skill the user has loaded via the composer's skill
    /// picker. `Some(name)` causes `chat::run_turn` to resolve the skill
    /// through `aictl_core::skills::find` and pass it to the engine for
    /// each turn until the user clears it. Skill bodies are looked up
    /// fresh each turn so on-disk edits show up without a desktop
    /// restart.
    pub loaded_skill: Mutex<Option<String>>,
    /// Monotonic counter feeding [`AppState::pending_approvals`].
    next_approval_id: AtomicU64,
    /// Monotonic counter feeding [`aictl_core::ui::events::AgentEvent::ProgressBegin`].
    next_progress_id: AtomicU64,
    /// Most recent provider-reported input-token count for the active
    /// transcript. Updated by `DesktopUI::show_token_usage` after every
    /// LLM call so the Context tab can report the live percentage
    /// without a fresh round-trip to the model. Zero before the first
    /// turn completes.
    pub last_input_tokens: AtomicU64,
    pub last_output_tokens: AtomicU64,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            turn_cancel: Mutex::new(None),
            session_id: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            incognito: Mutex::new(false),
            pending_approvals: Mutex::new(HashMap::new()),
            loaded_skill: Mutex::new(None),
            next_approval_id: AtomicU64::new(1),
            next_progress_id: AtomicU64::new(1),
            last_input_tokens: AtomicU64::new(0),
            last_output_tokens: AtomicU64::new(0),
        }
    }

    pub fn next_approval_id(&self) -> ApprovalId {
        self.next_approval_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn next_progress_id(&self) -> u64 {
        self.next_progress_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
