use crossterm::style::{Color, Stylize};

use crate::Provider;
use crate::llm::MODELS;

use super::menu::{
    build_simple_menu_lines, prompt_line_cancellable, select_from_menu, show_cancelled,
};

const TOP_MENU_ITEMS: &[(&str, &str)] = &[
    ("browse", "paged provider → model picker"),
    ("search", "type-ahead query over all models"),
];

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

/// Case-insensitive AND-match: every whitespace-separated token in `query`
/// must appear somewhere in `"{provider} {model}"`.
fn matches_query(query: &str, entry: &MenuModel) -> bool {
    let haystack = format!("{} {}", entry.provider, entry.model).to_lowercase();
    query
        .split_whitespace()
        .all(|tok| haystack.contains(&tok.to_lowercase()))
}

fn entry_to_tuple(entry: &MenuModel) -> (Provider, String, String) {
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
    (provider, entry.model.clone(), entry.api_key_name.clone())
}

/// Paged provider → model picker over the full `combined` list.
fn run_browse(combined: &[MenuModel], current_model: &str) -> Option<(Provider, String, String)> {
    let initial = combined
        .iter()
        .position(|m| m.model == current_model)
        .unwrap_or(0);
    let selected = select_from_menu(combined.len(), initial, |sel| {
        build_menu_lines(sel, current_model, combined).0
    })?;
    Some(entry_to_tuple(&combined[selected]))
}

/// Type-ahead picker: prompts for a query (unless one was supplied
/// inline), filters `combined` by substring/token match, and renders the
/// filtered list with the same format as the browse path.
fn run_search(
    combined: &[MenuModel],
    current_model: &str,
    pre_query: Option<String>,
) -> Option<(Provider, String, String)> {
    let query = if let Some(q) = pre_query {
        q
    } else {
        println!();
        let Ok(q) = prompt_line_cancellable("search models:") else {
            show_cancelled();
            return None;
        };
        q
    };
    let query = query.trim().to_string();
    if query.is_empty() {
        show_cancelled();
        return None;
    }

    let filtered: Vec<MenuModel> = combined
        .iter()
        .filter(|m| matches_query(&query, m))
        .map(|m| MenuModel {
            provider: m.provider.clone(),
            model: m.model.clone(),
            api_key_name: m.api_key_name.clone(),
        })
        .collect();

    if filtered.is_empty() {
        println!();
        println!(
            "  {} no models match {}",
            "✗".with(Color::Yellow),
            format!("'{query}'").with(Color::DarkGrey)
        );
        println!();
        return None;
    }

    let initial = filtered
        .iter()
        .position(|m| m.model == current_model)
        .unwrap_or(0);
    let selected = select_from_menu(filtered.len(), initial, |sel| {
        build_menu_lines(sel, current_model, &filtered).0
    })?;
    Some(entry_to_tuple(&filtered[selected]))
}

/// Interactively select a model.
///
/// With `initial_query = None`, presents a top-level Browse / Search menu.
/// With `initial_query = Some(q)`, skips straight to the filtered search
/// results (used by `/model search <query>` for scripted switching).
///
/// `ollama_models` / `local_models` / `mlx_models` are dynamically fetched
/// runtime model names (empty when the corresponding backend isn't
/// available). Returns `(Provider, model_name, api_key_config_key)` or
/// `None` if the user cancelled with Esc.
pub fn select_model(
    current_model: &str,
    ollama_models: &[String],
    local_models: &[String],
    mlx_models: &[String],
    initial_query: Option<&str>,
) -> Option<(Provider, String, String)> {
    let combined = build_combined_models(ollama_models, local_models, mlx_models);

    if let Some(q) = initial_query {
        return run_search(&combined, current_model, Some(q.to_string()));
    }

    let sel = select_from_menu(TOP_MENU_ITEMS.len(), 0, |s| {
        build_simple_menu_lines(TOP_MENU_ITEMS, s)
    })?;
    match sel {
        0 => run_browse(&combined, current_model),
        1 => run_search(&combined, current_model, None),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(provider: &str, model: &str) -> MenuModel {
        MenuModel {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key_name: String::new(),
        }
    }

    #[test]
    fn matches_query_substring_on_model() {
        assert!(matches_query("opus", &m("anthropic", "claude-opus-4-7")));
        assert!(!matches_query("gemini", &m("anthropic", "claude-opus-4-7")));
    }

    #[test]
    fn matches_query_substring_on_provider() {
        assert!(matches_query("anthro", &m("anthropic", "claude-opus-4-7")));
    }

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("OPUS", &m("anthropic", "claude-opus-4-7")));
        assert!(matches_query(
            "Anthropic",
            &m("anthropic", "claude-opus-4-7")
        ));
    }

    #[test]
    fn matches_query_tokens_are_and_joined() {
        let e = m("anthropic", "claude-opus-4-7");
        assert!(matches_query("anthropic opus", &e));
        assert!(!matches_query("anthropic sonnet", &e));
    }

    #[test]
    fn matches_query_empty_tokens_match_everything() {
        // split_whitespace of empty / whitespace-only is an empty iterator,
        // and `all` over an empty iterator returns true — callers reject
        // empty queries before hitting this path.
        assert!(matches_query("", &m("anthropic", "claude-opus-4-7")));
        assert!(matches_query("   ", &m("anthropic", "claude-opus-4-7")));
    }
}
