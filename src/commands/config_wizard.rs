use crossterm::style::{Color, Stylize};

use crate::llm::MODELS;

use super::menu::{confirm_yn, read_input_line, select_from_menu};

/// All providers and their API key config key names.
const PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "LLM_ANTHROPIC_API_KEY"),
    ("openai", "LLM_OPENAI_API_KEY"),
    ("gemini", "LLM_GEMINI_API_KEY"),
    ("grok", "LLM_GROK_API_KEY"),
    ("mistral", "LLM_MISTRAL_API_KEY"),
    ("deepseek", "LLM_DEEPSEEK_API_KEY"),
    ("kimi", "LLM_KIMI_API_KEY"),
    ("zai", "LLM_ZAI_API_KEY"),
    ("ollama", ""),
];

fn build_provider_menu_lines(selected: usize) -> Vec<String> {
    PROVIDERS
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let is_selected = i == selected;
            let label = match *name {
                "anthropic" => "Anthropic",
                "openai" => "OpenAI",
                "gemini" => "Gemini",
                "grok" => "Grok",
                "mistral" => "Mistral",
                "deepseek" => "DeepSeek",
                "kimi" => "Kimi",
                "zai" => "Z.ai",
                "ollama" => "Ollama (local, no API key)",
                _ => name,
            };
            if is_selected {
                format!(
                    "  {} {}",
                    "›".with(Color::Cyan),
                    label
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("    {}", label.with(Color::DarkGrey))
            }
        })
        .collect()
}

fn build_model_select_lines(selected: usize, models: &[&str]) -> Vec<String> {
    models
        .iter()
        .enumerate()
        .map(|(i, name)| {
            if i == selected {
                format!(
                    "  {} {}",
                    "›".with(Color::Cyan),
                    name.with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("    {}", name.with(Color::DarkGrey))
            }
        })
        .collect()
}

/// Short status hint for an API key based on its current storage location.
/// Returns `None` when the key is not set anywhere.
fn key_status_hint(key_name: &str) -> Option<&'static str> {
    match crate::keys::location(key_name) {
        crate::keys::KeyLocation::None => None,
        crate::keys::KeyLocation::Config => Some("set in plain-text config"),
        crate::keys::KeyLocation::Keyring => Some("set in system keyring"),
        crate::keys::KeyLocation::Both => Some("set in both config and keyring"),
    }
}

/// Interactive configuration wizard for setting provider, model, and API keys.
///
/// When `from_repl` is true the wizard suppresses the trailing "run aictl..."
/// hint (which doesn't make sense from inside an active REPL session) and lets
/// the caller refresh its in-memory `provider`/`model`/`api_key` after the wizard
/// returns.
#[allow(clippy::too_many_lines)]
pub fn run_config_wizard(from_repl: bool) {
    println!();
    println!(
        "  {}",
        "aictl configuration wizard"
            .with(Color::Cyan)
            .attribute(crossterm::style::Attribute::Bold)
    );
    println!(
        "  {}",
        "You can also edit ~/.aictl/config manually at any time.".with(Color::DarkGrey)
    );
    println!();

    // Step 1: Select provider
    println!("  {}", "Select provider:".with(Color::White));
    let Some(provider_idx) = select_from_menu(PROVIDERS.len(), 0, build_provider_menu_lines) else {
        println!();
        println!("  {} configuration cancelled", "✗".with(Color::Yellow));
        println!();
        return;
    };
    let (provider_name, api_key_name) = PROVIDERS[provider_idx];
    let is_ollama = provider_name == "ollama";

    // Step 2: Select model
    let models_for_provider: Vec<&str> = if is_ollama {
        vec![]
    } else {
        MODELS
            .iter()
            .filter(|(p, _, _)| *p == provider_name)
            .map(|(_, m, _)| *m)
            .collect()
    };

    let model = if is_ollama {
        // For Ollama, ask user to type a model name
        println!();
        println!(
            "  {}",
            "Enter Ollama model name (e.g. llama3, mistral):".with(Color::White)
        );
        let Some(m) = read_input_line("model:", false) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        let m = m.trim().to_string();
        if m.is_empty() {
            println!();
            println!("  {} no model specified, skipping", "⚠".with(Color::Yellow));
            println!();
            return;
        }
        m
    } else if models_for_provider.is_empty() {
        println!();
        println!(
            "  {} no models available for {provider_name}",
            "✗".with(Color::Red)
        );
        println!();
        return;
    } else {
        println!();
        println!("  {}", "Select model:".with(Color::White));
        let models_clone = models_for_provider.clone();
        let Some(model_idx) = select_from_menu(models_for_provider.len(), 0, |sel| {
            build_model_select_lines(sel, &models_clone)
        }) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        models_for_provider[model_idx].to_string()
    };

    // Step 3: API key for the selected provider (required for non-Ollama).
    // If a value is already stored (config or keyring), let the user keep it
    // by pressing Enter without typing anything.
    let mut keys_to_save: Vec<(String, String)> = Vec::new();

    if !is_ollama {
        println!();
        if let Some(hint) = key_status_hint(api_key_name) {
            println!(
                "  {} {}",
                format!("Enter API key for {provider_name}:").with(Color::White),
                format!("({hint} — press Enter to keep)").with(Color::DarkGrey),
            );
        } else {
            println!(
                "  {} {}",
                format!("Enter API key for {provider_name}:").with(Color::White),
                "(required)".with(Color::DarkGrey),
            );
        }
        let Some(key) = read_input_line(&format!("{api_key_name}:"), true) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            if key_status_hint(api_key_name).is_none() {
                println!();
                println!(
                    "  {} API key for {provider_name} is required, aborting",
                    "✗".with(Color::Red)
                );
                println!();
                return;
            }
            // else: keep existing value, don't queue for save
        } else {
            keys_to_save.push((api_key_name.to_string(), key));
        }
    }

    // Step 4: Ask about other API keys (optional). When a key is already set
    // (either in config or keyring), the prompt shows that and Enter keeps it.
    println!();
    println!(
        "  {}",
        "You can also set API keys for other providers (optional, press Enter to skip):"
            .with(Color::DarkGrey)
    );
    for &(prov, key_name) in PROVIDERS {
        if prov == provider_name || prov == "ollama" || key_name.is_empty() {
            continue;
        }
        let label = match prov {
            "anthropic" => "Anthropic",
            "openai" => "OpenAI",
            "gemini" => "Gemini",
            "grok" => "Grok",
            "mistral" => "Mistral",
            "deepseek" => "DeepSeek",
            "kimi" => "Kimi",
            "zai" => "Z.ai",
            _ => prov,
        };
        let prompt_label = if let Some(hint) = key_status_hint(key_name) {
            format!("{label} ({key_name}, {hint}):")
        } else {
            format!("{label} ({key_name}):")
        };
        let Some(key) = read_input_line(&prompt_label, true) else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        };
        let key = key.trim().to_string();
        if !key.is_empty() {
            keys_to_save.push((key_name.to_string(), key));
        }
    }

    // Step 5: Ollama host (optional)
    if is_ollama {
        println!();
        println!(
            "  {}",
            "Enter Ollama host (press Enter for default http://localhost:11434):"
                .with(Color::DarkGrey)
        );
        if let Some(host) = read_input_line("LLM_OLLAMA_HOST:", false) {
            let host = host.trim().to_string();
            if !host.is_empty() {
                keys_to_save.push(("LLM_OLLAMA_HOST".to_string(), host));
            }
        } else {
            println!();
            println!("  {} configuration cancelled", "✗".with(Color::Yellow));
            println!();
            return;
        }
    }

    // Save everything
    crate::config::config_set("AICTL_PROVIDER", provider_name);
    crate::config::config_set("AICTL_MODEL", &model);
    for (key_name, key_value) in &keys_to_save {
        crate::config::config_set(key_name, key_value);
    }

    println!();
    println!(
        "  {} configuration saved to ~/.aictl/config",
        "✓".with(Color::Green)
    );
    println!();
    println!("  {} {provider_name}", "provider:".with(Color::Cyan));
    println!("  {} {model}", "model:   ".with(Color::Cyan));
    if !keys_to_save.is_empty() {
        let saved_keys: Vec<&str> = keys_to_save.iter().map(|(k, _)| k.as_str()).collect();
        println!(
            "  {} {}",
            "keys:    ".with(Color::Cyan),
            saved_keys.join(", ")
        );
    }
    println!();

    // Step 6: Offer to migrate API keys into the system keyring (filter out
    // non-secret entries like LLM_OLLAMA_HOST by intersecting with KEY_NAMES).
    let lockable: Vec<String> = keys_to_save
        .iter()
        .filter(|(k, _)| crate::keys::KEY_NAMES.contains(&k.as_str()))
        .map(|(k, _)| k.clone())
        .collect();
    if !lockable.is_empty() && crate::keys::backend_available() {
        println!(
            "  {} {} {}",
            "→".with(Color::Cyan),
            format!("Lock {} API key(s) into the system keyring", lockable.len())
                .with(Color::White),
            format!("({})", crate::keys::backend_name()).with(Color::Green),
        );
        println!(
            "  {}",
            "Removes plain-text copies from ~/.aictl/config.".with(Color::DarkGrey),
        );
        if confirm_yn("lock keys now?") {
            for key in &lockable {
                match crate::keys::lock_key(key) {
                    crate::keys::LockOutcome::Locked => println!(
                        "  {} {} → keyring",
                        "✓".with(Color::Green),
                        key.as_str().with(Color::White),
                    ),
                    crate::keys::LockOutcome::AlreadyLocked
                    | crate::keys::LockOutcome::NotInConfig => {}
                    crate::keys::LockOutcome::Error(e) => println!(
                        "  {} {} ({})",
                        "✗".with(Color::Red),
                        key.as_str().with(Color::White),
                        e.with(Color::Red),
                    ),
                }
            }
            println!();
        }
    }

    if from_repl {
        println!(
            "  {} configuration applied — continuing your session",
            "→".with(Color::Cyan),
        );
    } else {
        println!(
            "  {} run {} to start a conversation, or {} for a single query",
            "→".with(Color::Cyan),
            "aictl"
                .with(Color::White)
                .attribute(crossterm::style::Attribute::Bold),
            "aictl -m \"your message\""
                .with(Color::White)
                .attribute(crossterm::style::Attribute::Bold),
        );
    }
    println!();
}
