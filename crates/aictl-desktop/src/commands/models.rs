//! Model catalogue + active provider/model selection.
//!
//! Mirrors the CLI's `/model` command: lists the static
//! [`aictl_core::llm::MODELS`] table merged with dynamically discovered
//! Ollama / GGUF / MLX models and (when configured) the upstream
//! `aictl-server` catalogue, then persists the user's choice via
//! `AICTL_PROVIDER` / `AICTL_MODEL`.

use aictl_core::config::{self, config_set};
use aictl_core::keys;
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

/// True when a cloud-provider API key is configured (keyring, plain
/// config, or process-local override). Local providers and
/// aictl-server are surfaced separately and don't go through this gate.
fn has_api_key(api_key_name: &str) -> bool {
    keys::get_secret(api_key_name).is_some_and(|v| !v.trim().is_empty())
}

#[tauri::command]
pub async fn list_models() -> Vec<ModelEntry> {
    let mut entries: Vec<ModelEntry> = MODELS
        .iter()
        .filter(|(_, _, key)| has_api_key(key))
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

/// Curated catalogues consumed by Settings → Image Models.
/// Both lists are filtered by the user's available API keys: a model
/// whose provider has no key configured is omitted so the dropdown
/// only shows options the user can actually use without first going to
/// the API Keys tab.
#[derive(Serialize)]
pub struct ImageModelCatalogue {
    pub analysis: Vec<ModelEntry>,
    pub generation: Vec<ModelEntry>,
}

#[tauri::command]
pub fn list_image_models() -> ImageModelCatalogue {
    let key_for = |provider: &str| -> Option<&'static str> {
        match provider {
            "openai" => Some("LLM_OPENAI_API_KEY"),
            "anthropic" => Some("LLM_ANTHROPIC_API_KEY"),
            "gemini" => Some("LLM_GEMINI_API_KEY"),
            "grok" => Some("LLM_GROK_API_KEY"),
            "mistral" => Some("LLM_MISTRAL_API_KEY"),
            "deepseek" => Some("LLM_DEEPSEEK_API_KEY"),
            "kimi" => Some("LLM_KIMI_API_KEY"),
            "zai" => Some("LLM_ZAI_API_KEY"),
            _ => None,
        }
    };
    let available = |provider: &str| -> bool { key_for(provider).is_some_and(has_api_key) };

    let analysis = llm::vision_capable_models()
        .into_iter()
        .filter(|(p, _)| available(p))
        .map(|(p, m)| ModelEntry {
            provider: p.to_string(),
            model: m.to_string(),
        })
        .collect();

    let generation = llm::IMAGE_GEN_MODELS
        .iter()
        .filter(|(p, _)| available(p))
        .map(|(p, m)| ModelEntry {
            provider: (*p).to_string(),
            model: (*m).to_string(),
        })
        .collect();

    ImageModelCatalogue {
        analysis,
        generation,
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
