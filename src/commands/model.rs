use crossterm::style::{Color, Stylize};

use crate::Provider;
use crate::llm::MODELS;

use super::menu::select_from_menu;

/// A combined model entry used for building the menu (static + dynamic Ollama models).
struct MenuModel {
    provider: String,
    model: String,
    api_key_name: String,
}

fn build_combined_models(
    ollama_models: &[String],
    local_models: &[String],
    mlx_models: &[String],
) -> Vec<MenuModel> {
    let mut combined: Vec<MenuModel> = MODELS
        .iter()
        .map(|(prov, model, key)| MenuModel {
            provider: (*prov).to_string(),
            model: (*model).to_string(),
            api_key_name: (*key).to_string(),
        })
        .collect();

    for m in ollama_models {
        combined.push(MenuModel {
            provider: "ollama".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    for m in local_models {
        combined.push(MenuModel {
            provider: "gguf".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    for m in mlx_models {
        combined.push(MenuModel {
            provider: "mlx".to_string(),
            model: m.clone(),
            api_key_name: String::new(),
        });
    }

    combined
}

/// Build the display lines for the model menu. Each entry is either a
/// header line (provider name) or a model line with its index into MODELS.
/// Returns `(lines, model_indices)` where `model_indices[i]` maps selectable
/// row `i` to its position in MODELS.
fn build_menu_lines(
    selected: usize,
    current_model: &str,
    models: &[MenuModel],
) -> (Vec<String>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut model_indices = Vec::new();

    for (sel_row, (i, entry)) in models.iter().enumerate().enumerate() {
        // Print provider header when provider changes
        if i == 0 || models[i - 1].provider != entry.provider {
            let label = match entry.provider.as_str() {
                "anthropic" => "Anthropic:",
                "openai" => "OpenAI:",
                "gemini" => "Gemini:",
                "grok" => "Grok:",
                "mistral" => "Mistral:",
                "deepseek" => "DeepSeek:",
                "kimi" => "Kimi:",
                "zai" => "Z.ai:",
                "ollama" => "Ollama:",
                "gguf" => "Native GGUF:",
                "mlx" => "Native MLX (Apple Silicon):",
                _ => entry.provider.as_str(),
            };
            lines.push(format!("  {}", label.with(Color::Cyan)));
        }

        let is_selected = sel_row == selected;
        let is_current = entry.model == current_model;

        let marker = if is_current { "●" } else { " " };
        let name = if is_selected {
            format!(
                "       {} {}",
                marker.with(Color::Green),
                entry
                    .model
                    .as_str()
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "       {} {}",
                marker.with(Color::Green),
                entry.model.as_str().with(Color::DarkGrey)
            )
        };

        let line = if is_selected {
            format!("  {} {name}", "›".with(Color::Cyan))
        } else {
            format!("    {name}")
        };

        lines.push(line);
        model_indices.push(i);
    }

    (lines, model_indices)
}

/// Interactively select a model with arrow keys.
/// `ollama_models` are dynamically fetched model names (empty if Ollama is not running).
/// Returns (Provider, `model_name`, `api_key_config_key`) or None if cancelled (Esc).
pub fn select_model(
    current_model: &str,
    ollama_models: &[String],
    local_models: &[String],
    mlx_models: &[String],
) -> Option<(Provider, String, String)> {
    let combined = build_combined_models(ollama_models, local_models, mlx_models);
    let initial = combined
        .iter()
        .position(|m| m.model == current_model)
        .unwrap_or(0);
    let selected = select_from_menu(combined.len(), initial, |sel| {
        build_menu_lines(sel, current_model, &combined).0
    })?;
    let entry = &combined[selected];
    let provider = match entry.provider.as_str() {
        "openai" => Provider::Openai,
        "anthropic" => Provider::Anthropic,
        "gemini" => Provider::Gemini,
        "grok" => Provider::Grok,
        "mistral" => Provider::Mistral,
        "deepseek" => Provider::Deepseek,
        "kimi" => Provider::Kimi,
        "zai" => Provider::Zai,
        "ollama" => Provider::Ollama,
        "gguf" => Provider::Gguf,
        "mlx" => Provider::Mlx,
        _ => unreachable!(),
    };
    Some((provider, entry.model.clone(), entry.api_key_name.clone()))
}
