//! Process-wide state for the desktop app.
//!
//! Tauri commands receive `tauri::State<Arc<AppState>>`; everything that
//! needs to live across IPC calls (the active turn's cancellation token,
//! pending tool-approval oneshots, the current session id) lives here so
//! commands stay short and free of `OnceLock` plumbing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use aictl_core::ToolApproval;
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
    /// conversation history. `None` when the user is in incognito mode.
    pub session_id: Mutex<Option<String>>,
    /// In-flight tool-approval requests, keyed by id.
    pub pending_approvals: Mutex<HashMap<ApprovalId, oneshot::Sender<ToolApproval>>>,
    /// Monotonic counter feeding [`AppState::pending_approvals`].
    next_approval_id: AtomicU64,
    /// Monotonic counter feeding [`aictl_core::ui::events::AgentEvent::ProgressBegin`].
    next_progress_id: AtomicU64,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            turn_cancel: Mutex::new(None),
            session_id: Mutex::new(None),
            pending_approvals: Mutex::new(HashMap::new()),
            next_approval_id: AtomicU64::new(1),
            next_progress_id: AtomicU64::new(1),
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
