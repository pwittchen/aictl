//! Model catalogue + active provider/model selection.
//!
//! Mirrors the CLI's `/model` command: lists the static
//! [`aictl_core::llm::MODELS`] table merged with dynamically discovered
//! Ollama / GGUF / MLX models and (when configured) the upstream
//! `aictl-server` catalogue, then persists the user's choice via
//! `AICTL_PROVIDER` / `AICTL_MODEL`.

use aictl_core::config::{self, config_set};
use aictl_core::llm::{self, MODELS};
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct ModelEntry {
    pub provider: String,
    pub model: String,
}

#[derive(Serialize, Clone)]
pub struct ActiveModel {
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[tauri::command]
pub async fn list_models() -> Vec<ModelEntry> {
    let mut entries: Vec<ModelEntry> = MODELS
        .iter()
        .map(|(prov, model, _)| ModelEntry {
            provider: (*prov).to_string(),
            model: (*model).to_string(),
        })
        .collect();

    for m in llm::ollama::list_models().await {
        entries.push(ModelEntry {
            provider: "ollama".into(),
            model: m,
        });
    }
    for m in llm::gguf::list_models() {
        entries.push(ModelEntry {
            provider: "gguf".into(),
            model: m,
        });
    }
    for m in llm::mlx::list_models() {
        entries.push(ModelEntry {
            provider: "mlx".into(),
            model: m,
        });
    }
    if let Some((url, key)) = config::active_server() {
        for m in llm::server_proxy::fetch_models(&url, &key).await {
            entries.push(ModelEntry {
                provider: "aictl-server".into(),
                model: m,
            });
        }
    }
    entries
}

#[tauri::command]
pub fn get_active_model() -> ActiveModel {
    ActiveModel {
        provider: config::config_get("AICTL_PROVIDER"),
        model: config::config_get("AICTL_MODEL"),
    }
}

#[tauri::command]
pub fn set_active_model(provider: String, model: String) -> Result<ActiveModel, String> {
    match provider.as_str() {
        "openai" | "anthropic" | "gemini" | "grok" | "mistral" | "deepseek" | "kimi" | "zai"
        | "ollama" | "gguf" | "mlx" | "aictl-server" => {}
        other => return Err(format!("unrecognized provider '{other}'")),
    }
    if model.trim().is_empty() {
        return Err("model name is empty".to_string());
    }
    config_set("AICTL_PROVIDER", &provider);
    config_set("AICTL_MODEL", &model);
    Ok(ActiveModel {
        provider: Some(provider),
        model: Some(model),
    })
}
