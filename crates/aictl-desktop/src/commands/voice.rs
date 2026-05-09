//! Voice-to-text Tauri commands backing the composer mic button.
//!
//! Three calls:
//!   * `voice_status` — does the model exist on disk yet, where is it,
//!     and is the build feature-gated `voice` feature compiled in
//!   * `voice_ensure_model` — download the bundled `ggml-base.bin`
//!     model from `HuggingFace`; reports progress through the same
//!     `agent_event` channel the chat surface listens on
//!   * `voice_transcribe` — run whisper inference on a buffer of
//!     16 kHz mono `f32` PCM samples shipped in by the webview
//!
//! When the `voice` feature is off the commands compile as stubs that
//! return a clear "voice support not compiled in" error, so the
//! frontend can disable the mic button without crashing on launch.

use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;

use crate::state::AppState;

#[derive(Serialize, Clone)]
pub struct VoiceStatus {
    /// `true` when the desktop binary was compiled with `--features voice`.
    /// When `false`, every other field is best-effort and the frontend
    /// hides the mic button.
    pub available: bool,
    /// Filesystem path of the bundled default Whisper model, regardless
    /// of whether it has been downloaded yet — handy for the "Reveal in
    /// Finder" affordance.
    pub model_path: String,
    /// `true` when the model file is on disk.
    pub model_present: bool,
    /// Human-readable label for the bundled model (e.g. `ggml-base.bin`).
    pub model_label: String,
}

#[cfg(feature = "voice")]
#[tauri::command]
pub fn voice_status() -> VoiceStatus {
    let path = crate::voice::model_path();
    VoiceStatus {
        available: true,
        model_path: path.display().to_string(),
        model_present: crate::voice::model_present(),
        model_label: crate::voice::DEFAULT_MODEL_FILENAME.to_string(),
    }
}

#[cfg(not(feature = "voice"))]
#[tauri::command]
pub fn voice_status() -> VoiceStatus {
    VoiceStatus {
        available: false,
        model_path: String::new(),
        model_present: false,
        model_label: String::new(),
    }
}

/// Kick off the model download. Reports progress via `progress_begin`/
/// `progress_update`/`progress_end` events on the `agent_event` channel,
/// matching the `gguf` / `mlx` model-pull plumbing so the webview can
/// reuse its existing progress-bar code.
///
/// The call resolves when the download finishes (or errors). The
/// frontend shows an indeterminate spinner against the same progress
/// id while waiting.
/// Outcome of `voice_ensure_model`. `started=true` means the download
/// is now running on a background thread and the frontend should listen
/// for `progress_begin`/`progress_end` events on the `agent_event`
/// channel; `started=false` means the model was already on disk and the
/// caller can transcribe immediately.
#[derive(Serialize, Clone)]
pub struct EnsureModelResult {
    pub started: bool,
    pub status: VoiceStatus,
}

#[cfg(feature = "voice")]
#[tauri::command]
pub fn voice_ensure_model(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<EnsureModelResult, String> {
    if crate::voice::model_present() {
        return Ok(EnsureModelResult {
            started: false,
            status: voice_status(),
        });
    }
    let state = state.inner().clone();
    // Mirror `local_models::spawn_download`: the future is `!Send`
    // because `DesktopUI` is touched from one thread, so we drive it on a
    // dedicated current-thread runtime. The real outcome lands on the
    // frontend through `progress_*` events.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[aictl-desktop] failed to build voice download runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let ui = crate::ui::DesktopUI::new(app, state);
            if let Err(e) = crate::voice::download_model(&ui).await {
                ui.emit_error(&format!("voice model download failed: {e}"));
            }
        });
    });
    Ok(EnsureModelResult {
        started: true,
        status: voice_status(),
    })
}

#[cfg(not(feature = "voice"))]
#[tauri::command]
pub fn voice_ensure_model(
    _app: AppHandle,
    _state: tauri::State<'_, Arc<AppState>>,
) -> Result<EnsureModelResult, String> {
    Err("voice support is not compiled into this build (rebuild with --features voice)".into())
}

/// Run whisper inference on a buffer of 16 kHz mono `f32` PCM samples.
/// The whisper context is loaded lazily inside `crate::voice::transcribe`
/// — the first call after launch pays the model-load cost (~300 ms for
/// `ggml-base.bin`); subsequent calls reuse the same context.
#[cfg(feature = "voice")]
#[tauri::command]
pub async fn voice_transcribe(samples: Vec<f32>) -> Result<String, String> {
    // whisper.cpp's `full` is a CPU/GPU-bound blocking call. Hop onto a
    // blocking thread so we don't stall Tauri's IPC runtime — a single
    // transcription can take a couple of seconds even on Apple Silicon.
    tokio::task::spawn_blocking(move || crate::voice::transcribe(samples))
        .await
        .map_err(|e| format!("voice task join: {e}"))?
}

#[cfg(not(feature = "voice"))]
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub async fn voice_transcribe(_samples: Vec<f32>) -> Result<String, String> {
    Err("voice support is not compiled into this build (rebuild with --features voice)".into())
}

/// Signal the in-flight model download to abort at the next chunk
/// boundary. The download routine clears the partial file before
/// returning. Calling this when no download is running is a no-op (the
/// flag is read-only by the worker).
#[cfg(feature = "voice")]
#[tauri::command]
pub fn voice_cancel_download() {
    crate::voice::cancel_download();
}

#[cfg(not(feature = "voice"))]
#[tauri::command]
pub fn voice_cancel_download() {}
