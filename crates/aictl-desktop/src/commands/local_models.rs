//! Local-model (GGUF / MLX) management Tauri commands.
//!
//! Mirrors the CLI's `/gguf` and `/mlx` slash commands, but adapts them
//! to the desktop's IPC shape: the heavy work (model downloads) happens
//! in a background tokio task driven by [`crate::ui::DesktopUI`], so
//! per-file progress flows through the same `agent_event` channel the
//! chat surface already listens on. The IPC call returns immediately
//! once the task is spawned — the webview correlates progress events by
//! the [`AgentEvent::ProgressBegin`] id.
//!
//! Both backends are gated behind cargo features (`gguf`, `mlx`) for
//! *inference*; the download/list/remove paths compile on every build,
//! so the desktop can ship the management UI even when the binary
//! cannot run the resulting models. Status fields surface that gap so
//! the webview can render a "downloaded but cannot run on this build"
//! note instead of pretending the models are usable.

use std::sync::Arc;

use aictl_core::llm::{gguf, mlx};
use serde::Serialize;
use tauri::AppHandle;

use crate::state::AppState;
use crate::ui::DesktopUI;

#[derive(Serialize, Clone)]
pub struct CatalogEntryRow {
    pub label: String,
    pub spec: String,
    pub size_label: String,
}

impl From<&gguf::CatalogEntry> for CatalogEntryRow {
    fn from(e: &gguf::CatalogEntry) -> Self {
        Self {
            label: e.label.to_string(),
            spec: e.spec.to_string(),
            size_label: e.size_label.to_string(),
        }
    }
}

impl From<&mlx::CatalogEntry> for CatalogEntryRow {
    fn from(e: &mlx::CatalogEntry) -> Self {
        Self {
            label: e.label.to_string(),
            spec: e.spec.to_string(),
            size_label: e.size_label.to_string(),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct LocalModelRow {
    pub name: String,
    pub size_bytes: u64,
}

#[derive(Serialize, Clone)]
pub struct GgufStatus {
    /// `true` when the binary was built with `--features gguf`. When
    /// `false`, downloaded models exist on disk but cannot be run from
    /// this build — the webview surfaces a corresponding warning.
    pub inference_available: bool,
    pub dir: String,
    pub models: Vec<LocalModelRow>,
    pub catalog: Vec<CatalogEntryRow>,
}

#[derive(Serialize, Clone)]
pub struct MlxStatus {
    pub inference_available: bool,
    /// `true` on macOS + Apple Silicon. When `false`, models can be
    /// downloaded for archival but not run on this host.
    pub host_supports_mlx: bool,
    pub dir: String,
    pub models: Vec<LocalModelRow>,
    pub catalog: Vec<CatalogEntryRow>,
}

#[derive(Serialize, Clone)]
pub struct LocalModelsStatus {
    pub gguf: GgufStatus,
    pub mlx: MlxStatus,
}

#[tauri::command]
pub fn local_models_status() -> LocalModelsStatus {
    let gguf_models = gguf::list_models()
        .into_iter()
        .map(|name| {
            let size_bytes = gguf::model_size(&name);
            LocalModelRow { name, size_bytes }
        })
        .collect();
    let mlx_models = mlx::list_models()
        .into_iter()
        .map(|name| {
            let size_bytes = mlx::model_size(&name);
            LocalModelRow { name, size_bytes }
        })
        .collect();
    LocalModelsStatus {
        gguf: GgufStatus {
            inference_available: gguf::is_available(),
            dir: gguf::models_dir().display().to_string(),
            models: gguf_models,
            catalog: gguf::CATALOG.iter().map(CatalogEntryRow::from).collect(),
        },
        mlx: MlxStatus {
            inference_available: mlx::is_available(),
            host_supports_mlx: mlx::host_supports_mlx(),
            dir: mlx::models_dir().display().to_string(),
            models: mlx_models,
            catalog: mlx::CATALOG.iter().map(CatalogEntryRow::from).collect(),
        },
    }
}

/// Outcome of `pull_*`. `progress_id` is `None` because the download
/// runs in the background and the real id flows through `ProgressBegin`
/// on the `agent_event` channel — frontends correlate by `label` (the
/// human-readable model name we pass to `progress_begin`) until the
/// `id` is observed.
#[derive(Serialize, Clone)]
pub struct PullStarted {
    pub label: String,
}

#[tauri::command]
pub fn local_models_pull_gguf(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    args: PullArgs,
) -> Result<PullStarted, String> {
    let state = state.inner().clone();
    let label = args
        .name
        .clone()
        .unwrap_or_else(|| derive_label_from_spec(&args.spec));
    let label_for_task = label.clone();
    spawn_download(move || {
        let ui = DesktopUI::new(app, state);
        async move {
            match gguf::download_model(&ui, &args.spec, args.name.as_deref()).await {
                Ok(name) => ui.emit_warning(&format!("downloaded GGUF model '{name}'")),
                Err(e) => {
                    ui.emit_error(&format!("GGUF download failed for '{label_for_task}': {e}"));
                }
            }
        }
    });
    Ok(PullStarted { label })
}

#[tauri::command]
pub fn local_models_pull_mlx(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    args: PullArgs,
) -> Result<PullStarted, String> {
    let state = state.inner().clone();
    let label = args
        .name
        .clone()
        .unwrap_or_else(|| derive_label_from_spec(&args.spec));
    let label_for_task = label.clone();
    spawn_download(move || {
        let ui = DesktopUI::new(app, state);
        async move {
            match mlx::download_model(&ui, &args.spec, args.name.as_deref()).await {
                Ok(name) => ui.emit_warning(&format!("downloaded MLX model '{name}'")),
                Err(e) => {
                    ui.emit_error(&format!("MLX download failed for '{label_for_task}': {e}"));
                }
            }
        }
    });
    Ok(PullStarted { label })
}

/// Drive a download future on a dedicated OS thread with its own
/// current-thread tokio runtime. Mirrors the pattern in
/// [`super::chat::send_message`]: `DesktopUI` is intentionally `!Sync`
/// (it holds an `AppHandle` and `Arc<AppState>` that are only touched
/// from one thread), so spawning on Tauri's multi-threaded runtime
/// would fail to compile. A current-thread runtime keeps the future
/// pinned to the spawning thread and sidesteps the bound.
fn spawn_download<F, Fut>(make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[aictl-desktop] failed to build download runtime: {e}");
                return;
            }
        };
        rt.block_on(make_future());
    });
}

#[derive(serde::Deserialize)]
pub struct PullArgs {
    pub spec: String,
    /// Optional override for the on-disk name. When `None`, the engine
    /// derives it from the spec (filename stem for GGUF, `owner__repo`
    /// for MLX).
    #[serde(default)]
    pub name: Option<String>,
}

#[tauri::command]
pub fn local_models_remove_gguf(name: String) -> Result<(), String> {
    gguf::remove_model(&name).map_err(|e| format!("remove '{name}': {e}"))
}

#[tauri::command]
pub fn local_models_remove_mlx(name: String) -> Result<(), String> {
    mlx::remove_model(&name).map_err(|e| format!("remove '{name}': {e}"))
}

#[tauri::command]
pub fn local_models_clear_gguf() -> Result<usize, String> {
    gguf::clear_models().map_err(|e| format!("clear GGUF models: {e}"))
}

#[tauri::command]
pub fn local_models_clear_mlx() -> Result<usize, String> {
    mlx::clear_models().map_err(|e| format!("clear MLX models: {e}"))
}

/// Best-effort human label for a download that the user did not name
/// themselves. Used as the `progress_begin` label and as the fallback
/// id-correlation key on the frontend until the real id arrives.
fn derive_label_from_spec(spec: &str) -> String {
    spec.rsplit('/')
        .next()
        .and_then(|f| f.split('?').next())
        .unwrap_or(spec)
        .to_string()
}
