//! Voice-to-text backend for the composer mic button.
//!
//! Wraps `whisper-rs` (whisper.cpp) so the webview can capture mic audio,
//! ship the raw 16 kHz mono PCM samples across IPC, and get back a
//! transcribed string. The model lives at `~/.aictl/whisper/ggml-base.bin`
//! (multilingual base, ~140 MB) and is downloaded from the upstream
//! `ggerganov/whisper.cpp` `HuggingFace` repo on first use.
//!
//! The whole module is feature-gated (`voice`) so a `--no-default-features`
//! desktop build skips the whisper.cpp compile entirely.

#![cfg(feature = "voice")]

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use aictl_core::ui::AgentUI;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Cancellation flag for the in-flight model download. Flipped to `true`
/// by `cancel_download` (wired to the modal's Cancel button) and checked
/// inside the chunk loop in `download_model`. Reset to `false` at the
/// start of every fresh `download_model` call so a stale flag from a
/// previous run can't pre-cancel a retry.
static DOWNLOAD_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Signal the in-flight download (if any) to stop at the next chunk
/// boundary. The download routine clears the partial file before
/// returning, and `cleanup_partial_downloads` provides a belt-and-braces
/// pass for the rare case where the worker thread was force-killed
/// before it could observe the flag.
pub fn cancel_download() {
    DOWNLOAD_CANCELLED.store(true, Ordering::SeqCst);
    cleanup_partial_downloads();
}

/// Multilingual base model — best quality/size tradeoff (~140 MB) and
/// covers every language Whisper supports. Mirrors the `gguf::CATALOG`
/// pattern: keep the catalogue as a single source of truth so future
/// expansion (a Settings tab to swap models) reuses the same constant.
pub const DEFAULT_MODEL_FILENAME: &str = "ggml-base.bin";
pub const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin?download=true";

/// `~/.aictl/whisper/`. Created on demand by `ensure_dir`; callers should
/// not assume it exists before calling `download_model` or `transcribe`.
fn voice_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".aictl").join("whisper")
}

/// Absolute path to the bundled default model on disk.
pub fn model_path() -> PathBuf {
    voice_dir().join(DEFAULT_MODEL_FILENAME)
}

/// `true` when the default model file already exists on disk. Used by
/// the frontend to decide whether to show the "Downloading…" state on
/// first invocation.
pub fn model_present() -> bool {
    model_path().is_file()
}

fn ensure_dir() -> Result<PathBuf, String> {
    let dir = voice_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Reap orphaned `*.part` files from interrupted downloads. Called once
/// at app boot; harmless when no whisper models have ever been
/// downloaded (the directory may not exist yet). Best-effort — failures
/// are logged to stderr rather than propagated since this only runs to
/// reclaim disk space, never blocks anything functional.
pub fn cleanup_partial_downloads() {
    let dir = voice_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            eprintln!(
                "[aictl-desktop] could not scan {} for stale .part files: {e}",
                dir.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "part") {
            match std::fs::remove_file(&path) {
                Ok(()) => eprintln!(
                    "[aictl-desktop] removed orphaned voice download {}",
                    path.display()
                ),
                Err(e) => eprintln!("[aictl-desktop] could not remove {}: {e}", path.display()),
            }
        }
    }
}

/// Streaming download of the default Whisper model, reporting progress
/// through the supplied `AgentUI` so the modal can render the same bar
/// the GGUF / MLX downloads use. Writes to `<model>.part` and renames
/// on success — partial files never pollute a future run.
pub async fn download_model(ui: &dyn AgentUI) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    // Reset the cancel flag — without this a previous Cancel click would
    // pre-abort a retry.
    DOWNLOAD_CANCELLED.store(false, Ordering::SeqCst);

    let dir = ensure_dir()?;
    let final_path = dir.join(DEFAULT_MODEL_FILENAME);
    if final_path.is_file() {
        return Ok(());
    }
    let tmp_path = dir.join(format!("{DEFAULT_MODEL_FILENAME}.part"));

    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let response = client
        .get(DEFAULT_MODEL_URL)
        .send()
        .await
        .map_err(|e| format!("download {DEFAULT_MODEL_URL}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download {DEFAULT_MODEL_URL}: {e}"))?;
    let total = response.content_length();
    let progress = ui.progress_begin(DEFAULT_MODEL_FILENAME, total);

    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| format!("create {}: {e}", tmp_path.display()))?;
    let mut stream = response.bytes_stream();
    let mut got: u64 = 0;
    let mut cancelled = false;
    while let Some(chunk) = stream.next().await {
        if DOWNLOAD_CANCELLED.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let chunk = chunk.map_err(|e| format!("read chunk: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write chunk: {e}"))?;
        got = got.saturating_add(chunk.len() as u64);
        ui.progress_update(&progress, got, None);
    }

    if cancelled {
        // Tear down the partial file before signalling end-of-progress so
        // the modal observes a clean filesystem when it polls
        // `voice_status` after the cancel toast.
        drop(file);
        let _ = tokio::fs::remove_file(&tmp_path).await;
        ui.progress_end(progress, Some("download cancelled"));
        return Err("download cancelled".to_string());
    }

    file.flush()
        .await
        .map_err(|e| format!("flush model: {e}"))?;
    drop(file);
    ui.progress_end(progress, Some("voice model ready"));

    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| format!("rename model: {e}"))?;
    Ok(())
}

/// Lazily-initialised whisper context. `WhisperContext` is expensive
/// (~hundreds of ms to load 140 MB from disk + Metal warm-up), so we
/// keep one for the lifetime of the process. A `Mutex` serialises calls
/// — `WhisperState::full` is `&mut`, and even if it weren't, running
/// two transcriptions in parallel on the same context is unsupported.
///
/// Hoisted to module scope (rather than a `static` inside `context()`)
/// so `shutdown` can `OnceLock::get` it on exit without forcing
/// initialisation in builds that never transcribed.
static WHISPER_CTX: std::sync::OnceLock<Mutex<Option<WhisperContext>>> = std::sync::OnceLock::new();

fn context() -> &'static Mutex<Option<WhisperContext>> {
    WHISPER_CTX.get_or_init(|| Mutex::new(None))
}

/// Drop the cached whisper context and abort any in-flight model
/// download. Called from the Tauri `RunEvent::Exit` hook so the heavy
/// native resources (whisper.cpp model buffers, Metal command queues)
/// are released while the macOS Cocoa/Metal stack is still alive —
/// dropping them after `NSApplication` has terminated reliably crashes
/// the process and triggers the "Application unexpectedly quit" dialog.
pub fn shutdown() {
    cancel_download();
    if let Some(mutex) = WHISPER_CTX.get()
        && let Ok(mut guard) = mutex.lock()
    {
        // Replace with `None`. The previous `Some(WhisperContext)` is
        // dropped here, on the main thread, during the Exit event —
        // which is the only deterministic teardown window the runtime
        // gives us.
        *guard = None;
    }
}

/// Transcribe a buffer of 16 kHz mono `f32` PCM samples to text. The
/// caller is responsible for resampling — Whisper's preprocessor only
/// works at 16 kHz. Empty/short buffers return an empty string rather
/// than erroring so a "user pressed Accept too fast" turn doesn't toast.
pub fn transcribe(samples: Vec<f32>) -> Result<String, String> {
    if samples.len() < 1600 {
        return Ok(String::new());
    }

    let path = model_path();
    if !path.is_file() {
        return Err(format!(
            "Whisper model not found at {} — download it first",
            path.display()
        ));
    }

    let mut guard = context()
        .lock()
        .map_err(|e| format!("whisper context poisoned: {e}"))?;
    if guard.is_none() {
        let path_str = path
            .to_str()
            .ok_or_else(|| format!("non-utf8 model path: {}", path.display()))?;
        let ctx = WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
            .map_err(|e| format!("load whisper model: {e}"))?;
        *guard = Some(ctx);
    }
    let ctx = guard.as_ref().expect("ctx initialised above");
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("create whisper state: {e}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // Default whisper.cpp behaviour prints decoder progress to stderr —
    // suppress it so the desktop's stderr stays clean.
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);
    // whisper.cpp defaults to `language = "en"`, so a Polish (or any
    // non-English) utterance gets force-decoded as English noise.
    // Passing `None` switches it into language-auto-detect mode — the
    // model picks the language from the first ~1s of audio and decodes
    // accordingly. The bundled `ggml-base.bin` is the multilingual
    // checkpoint, so this is the only thing standing between an
    // English-only and a fully multilingual transcription.
    params.set_language(None);
    let threads = std::thread::available_parallelism()
        .ok()
        .and_then(|n| i32::try_from(n.get()).ok())
        .map_or(4, |n| n.min(8));
    params.set_n_threads(threads);

    state
        .full(params, &samples)
        .map_err(|e| format!("transcribe: {e}"))?;

    let n = state.full_n_segments();
    let mut out = String::new();
    for i in 0..n {
        if let Some(seg) = state.get_segment(i) {
            let text = seg
                .to_str_lossy()
                .map_err(|e| format!("read segment {i}: {e}"))?;
            out.push_str(text.as_ref());
        }
    }
    Ok(out.trim().to_string())
}
