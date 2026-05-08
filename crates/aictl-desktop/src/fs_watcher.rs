//! Recursive filesystem watcher for the active workspace.
//!
//! `notify` ships an inotify/FSEvents/kqueue abstraction; we recursively
//! watch the workspace root and translate every Create/Modify/Remove
//! into a single coalesced `workspace_fs_changed` Tauri event. The
//! frontend reacts by re-fetching the visible directory listings and
//! re-reading the currently-open file — same pattern an IDE side panel
//! uses when an external editor saves a file.
//!
//! Coalescing matters because a single `cp -R` or `git checkout` produces
//! hundreds of events in a burst; without the debounce we would saturate
//! the IPC channel and the frontend would spend all its time re-fetching
//! tree fragments mid-burst. The 250 ms window is short enough to feel
//! instant on save and long enough to absorb a typical Git operation.
//!
//! The watcher's lifetime is tied to the [`crate::state::AppState`];
//! [`start`] replaces any existing instance, so flipping the workspace
//! tears down the old watcher and stands up a new one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

const DEBOUNCE: Duration = Duration::from_millis(250);

/// Owns the platform watcher. Dropping the struct stops watching: the
/// callback's `tx` end of the channel goes with it, the spawned debounce
/// task observes a closed channel and exits, and `notify` tears down its
/// kernel handles via `RecommendedWatcher::Drop`.
pub struct FsWatcher {
    _watcher: RecommendedWatcher,
}

impl FsWatcher {
    /// Begin watching `path` recursively. Errors only on the initial
    /// `Watcher::new` / `watch` syscalls; runtime delivery failures (a
    /// dropped event, a transient IO error from notify) are logged and
    /// otherwise ignored — the next event will trigger a refresh anyway.
    pub fn start(app: AppHandle, path: PathBuf) -> Result<Self, String> {
        if !path.is_dir() {
            return Err(format!("workspace '{}' is not a directory", path.display()));
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                let Ok(event) = res else {
                    return;
                };
                if !is_relevant(event.kind) {
                    return;
                }
                let _ = tx.send(());
            },
            notify::Config::default(),
        )
        .map_err(|e| format!("failed to create fs watcher: {e}"))?;

        watcher
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|e| format!("failed to watch '{}': {e}", path.display()))?;

        // Coalesce bursts: wait for the first event, then drain anything
        // that arrives within `DEBOUNCE`, then emit once. The webview's
        // refresh logic is idempotent so a single pulse covers the whole
        // burst.
        tauri::async_runtime::spawn(async move {
            while rx.recv().await.is_some() {
                let _ =
                    tokio::time::timeout(DEBOUNCE, async { while rx.recv().await.is_some() {} })
                        .await;
                let _ = app.emit("workspace_fs_changed", ());
            }
        });

        Ok(Self { _watcher: watcher })
    }
}

fn is_relevant(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Best-effort: stand up a watcher anchored at `path` and stash it on
/// the shared state, replacing any prior instance. Failures are logged
/// to stderr (the watcher is a UX nicety; the desktop still works
/// without it).
pub fn install(app: &AppHandle, state: &crate::state::AppState, path: &Path) {
    match FsWatcher::start(app.clone(), path.to_path_buf()) {
        Ok(watcher) => {
            let mut slot = state.fs_watcher.lock().expect("fs_watcher lock poisoned");
            *slot = Some(watcher);
        }
        Err(err) => {
            eprintln!(
                "[aictl-desktop] failed to start fs watcher for '{}': {err}",
                path.display()
            );
        }
    }
}

/// Tear down the existing watcher, if any. Used when the user clears
/// the workspace or the app is shutting down.
pub fn uninstall(state: &crate::state::AppState) {
    let mut slot = state.fs_watcher.lock().expect("fs_watcher lock poisoned");
    *slot = None;
}
