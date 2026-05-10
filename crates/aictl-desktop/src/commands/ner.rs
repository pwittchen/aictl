//! NER (Layer-C redaction) model management Tauri commands.
//!
//! The Redaction Settings tab needs to know whether a usable gline-rs
//! model is on disk before the user can flip `AICTL_REDACTION_NER=true`
//! — otherwise enabling NER produces a one-shot warning at the first
//! turn and silently skips Layer C. These commands mirror the CLI's
//! `--pull-ner-model` / `--list-ner-models` flow:
//!
//!   * `ner_status` — feature flag, configured model name, models on
//!     disk, and whether the configured model is fully downloaded.
//!   * `ner_pull` — kick off `download_model` on a background thread so
//!     progress flows through the shared `agent_event` channel (same as
//!     GGUF/MLX/voice).
//!   * `ner_remove` — delete one model directory.
//!
//! All three compile on every build — only the inference path is gated
//! behind `--features redaction-ner` (see `redaction::ner::is_available`).

use std::sync::Arc;

use aictl_core::security::redaction::ner;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::state::AppState;
use crate::ui::DesktopUI;

#[derive(Serialize, Clone)]
pub struct NerStatus {
    /// `true` when the binary was built with `--features redaction-ner`.
    /// Management calls (pull/remove/list) work either way; only the
    /// actual redaction inference is gated on this.
    pub inference_available: bool,
    /// `~/.aictl/models/ner/` — where pulled models live on disk.
    pub dir: String,
    /// Local name of the currently configured model (derived from
    /// `AICTL_REDACTION_NER_MODEL` or the built-in default).
    pub configured_model: String,
    /// The default Hugging Face spec the desktop offers if the user
    /// hasn't picked a custom one yet.
    pub default_spec: String,
    /// `true` when the configured model has both files on disk and is
    /// ready to drive redaction. Drives the enable-NER toggle gate.
    pub configured_model_present: bool,
    /// On-disk size of the configured model in bytes. `0` when the
    /// model isn't downloaded yet.
    pub configured_model_size: u64,
    /// Every model directory under `dir` that contains a usable pair
    /// of files. Empty until the first pull completes.
    pub models: Vec<String>,
}

#[tauri::command]
pub fn ner_status() -> NerStatus {
    let configured = ner::configured_model_name();
    let present = ner::model_files(&configured).is_some();
    let size = if present {
        ner::model_size(&configured)
    } else {
        0
    };
    NerStatus {
        inference_available: ner::is_available(),
        dir: ner::models_dir().display().to_string(),
        configured_model: configured,
        default_spec: ner::DEFAULT_NER_MODEL.to_string(),
        configured_model_present: present,
        configured_model_size: size,
        models: ner::list_models(),
    }
}

#[derive(Deserialize)]
pub struct NerPullArgs {
    pub spec: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct NerPullStarted {
    pub label: String,
}

/// Kick off a model download. Returns immediately; the frontend correlates
/// progress events on the `agent_event` channel by the `label` field
/// (which is what `download_model` passes to `progress_begin`).
#[tauri::command]
pub fn ner_pull(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    args: NerPullArgs,
) -> Result<NerPullStarted, String> {
    let state = state.inner().clone();
    let label = args
        .name
        .clone()
        .unwrap_or_else(|| derive_label_from_spec(&args.spec));
    let label_for_task = label.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[aictl-desktop] failed to build NER download runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let ui = DesktopUI::new(app, state);
            match ner::download_model(&ui, &args.spec, args.name.as_deref()).await {
                Ok(name) => ui.emit_warning(&format!("downloaded NER model '{name}'")),
                Err(e) => {
                    ui.emit_error(&format!("NER download failed for '{label_for_task}': {e}"));
                }
            }
        });
    });
    Ok(NerPullStarted { label })
}

#[derive(Deserialize)]
pub struct NerRemoveArgs {
    pub name: String,
}

#[tauri::command]
pub fn ner_remove(args: NerRemoveArgs) -> Result<(), String> {
    ner::remove_model(&args.name).map_err(|e| format!("remove '{}': {e}", args.name))
}

fn derive_label_from_spec(spec: &str) -> String {
    spec.rsplit('/')
        .next()
        .and_then(|f| f.split('?').next())
        .unwrap_or(spec)
        .to_string()
}
