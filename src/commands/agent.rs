use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::agents;
use crate::ui::AgentUI;
use crate::{Message, Provider, Role};

use super::menu::{
    confirm_yn, read_input_line, read_multiline_input, read_multiline_input_prefilled,
    select_from_menu,
};

fn build_agent_menu_lines(selected: usize) -> Vec<String> {
    let has_loaded = agents::loaded_agent_name().is_some();
    let items = agent_menu_items(has_loaded);
    let max_name = items.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (i, (name, desc)) in items.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", *name);
        let name_styled = if is_selected {
            format!(
                "  {}",
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("  {}", padded.with(Color::DarkGrey))
        };
        let desc_styled = format!("{}", desc.with(Color::DarkGrey));
        let line = if is_selected {
            format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

fn agent_menu_items(has_loaded: bool) -> Vec<(&'static str, &'static str)> {
    let mut items = vec![
        ("create agent manually", "type or paste agent prompt"),
        ("create agent with AI", "describe what the agent should do"),
        ("view all agents", "browse, load, or delete agents"),
    ];
    if has_loaded {
        items.push(("edit agent", "edit currently loaded agent prompt"));
        items.push(("unload agent", "remove currently loaded agent"));
    }
    items
}

/// Run the /agent menu. Returns `true` if the system prompt should be rebuilt.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_menu(
    provider: &Provider,
    api_key: &str,
    model: &str,
    messages: &mut [Message],
    ui: &dyn AgentUI,
    show_error: &dyn Fn(&str),
) -> bool {
    let has_loaded = agents::loaded_agent_name().is_some();
    let items = agent_menu_items(has_loaded);
    let Some(sel) = select_from_menu(items.len(), 0, build_agent_menu_lines) else {
        return false;
    };
    match items[sel].0 {
        "create agent manually" => create_agent_manually(show_error),
        "create agent with AI" => {
            create_agent_with_ai(provider, api_key, model, ui, show_error).await
        }
        "view all agents" => view_all_agents(messages, show_error),
        "edit agent" => {
            let name = agents::loaded_agent_name().unwrap_or_default();
            edit_agent_prompt(&name, true, messages, show_error).unwrap_or(false)
        }
        "unload agent" => unload_agent_action(messages),
        _ => false,
    }
}

fn create_agent_manually(show_error: &dyn Fn(&str)) -> bool {
    let Some(name) = read_input_line("agent name:", false) else {
        return false;
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        return false;
    }
    if !agents::is_valid_name(&name) {
        show_error("Invalid name. Use only letters, numbers, underscore, or dash.");
        return false;
    }

    println!();
    println!(
        "  {}",
        "Enter agent prompt (multi-line: Ctrl+D to finish, Esc to cancel):".with(Color::DarkGrey)
    );
    let Some(prompt) = read_multiline_input() else {
        return false;
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        show_error("Empty prompt, agent not created.");
        return false;
    }

    if let Err(e) = agents::save_agent(&name, &prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return false;
    }
    println!();
    println!(
        "  {} agent \"{}\" created",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    false
}

#[allow(clippy::too_many_lines)]
async fn create_agent_with_ai(
    provider: &Provider,
    api_key: &str,
    model: &str,
    ui: &dyn AgentUI,
    show_error: &dyn Fn(&str),
) -> bool {
    let Some(name) = read_input_line("agent name:", false) else {
        return false;
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        return false;
    }
    if !agents::is_valid_name(&name) {
        show_error("Invalid name. Use only letters, numbers, underscore, or dash.");
        return false;
    }

    println!();
    println!(
        "  {}",
        "Describe what the agent should do:".with(Color::DarkGrey)
    );
    let Some(description) = read_input_line("description:", false) else {
        return false;
    };
    let description = description.trim().to_string();
    if description.is_empty() {
        return false;
    }

    ui.start_spinner("generating agent prompt...");

    let gen_messages = vec![
        Message {
            role: Role::System,
            content: "You are an expert at writing system prompts for AI assistants. \
                Generate a clear, detailed system prompt for an AI agent based on the user's \
                description. The prompt should define the agent's role, capabilities, behavior, \
                and constraints. Output ONLY the prompt text, nothing else."
                .to_string(),
            images: vec![],
        },
        Message {
            role: Role::User,
            content: format!(
                "Create a system prompt for an AI agent named \"{name}\" that does the following: {description}"
            ),
            images: vec![],
        },
    ];

    let llm_timeout = crate::config::llm_timeout();
    // The agent-prompt generator runs as a background helper for the
    // /agent menu — its output is consumed programmatically, not shown to
    // the user as a streaming answer. Pass `None` to keep the buffered
    // code path.
    let result = match provider {
        Provider::Openai => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::openai::call_openai(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::anthropic::call_anthropic(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Gemini => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::gemini::call_gemini(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Grok => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::grok::call_grok(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Mistral => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::mistral::call_mistral(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Deepseek => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::deepseek::call_deepseek(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Kimi => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::kimi::call_kimi(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Zai => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::zai::call_zai(api_key, model, &gen_messages, None),
            ))
            .await
        }
        Provider::Ollama => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::ollama::call_ollama(model, &gen_messages, None),
            ))
            .await
        }
        Provider::Gguf => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::gguf::call_gguf(model, &gen_messages, None),
            ))
            .await
        }
        Provider::Mlx => {
            crate::with_esc_cancel(tokio::time::timeout(
                llm_timeout,
                crate::llm::mlx::call_mlx(model, &gen_messages, None),
            ))
            .await
        }
        Provider::Mock => unreachable!("Provider::Mock is test-only and never selected at runtime"),
    };

    ui.stop_spinner();

    let result = match result {
        Ok(inner) => inner,
        Err(_interrupted) => {
            println!("\n  {} interrupted\n", "✗".with(Color::Yellow));
            return false;
        }
    };

    let result = match result {
        Ok(inner) => inner,
        Err(_elapsed) => {
            show_error(&format!(
                "Agent generation timed out after {}s (AICTL_LLM_TIMEOUT).",
                llm_timeout.as_secs()
            ));
            return false;
        }
    };

    let (prompt, _usage) = match result {
        Ok(r) => r,
        Err(e) => {
            show_error(&format!("Failed to generate agent prompt: {e}"));
            return false;
        }
    };

    let prompt = prompt.trim().to_string();
    println!();
    println!("  {}", "Generated agent prompt:".with(Color::Cyan));
    println!();
    for line in prompt.lines() {
        println!("  {}", line.with(Color::DarkGrey));
    }
    println!();

    if !confirm_yn("save this agent?") {
        return false;
    }

    if let Err(e) = agents::save_agent(&name, &prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return false;
    }
    println!();
    println!(
        "  {} agent \"{}\" created",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    false
}

fn build_agents_list_lines(
    selected: usize,
    entries: &[agents::AgentEntry],
    loaded_name: Option<&str>,
) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no agents found)".with(Color::DarkGrey))];
    }
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let is_loaded = loaded_name == Some(e.name.as_str());
        let marker = if is_loaded { "●" } else { " " };
        let body = e.name.as_str();
        let styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                body.with(Color::DarkGrey)
            )
        };
        let line = if is_selected {
            format!("  {} {styled}", "›".with(Color::Cyan))
        } else {
            format!("    {styled}")
        };
        lines.push(line);
    }
    lines
}

enum AgentListAction {
    Load(usize),
    View(usize),
    Edit(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_agent_from_list(entries: &[agents::AgentEntry]) -> AgentListAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let loaded_name = agents::loaded_agent_name();
    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let mut lines = build_agents_list_lines(selected, entries, loaded_name.as_deref());
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let hint = "↑/↓ navigate · l/enter load · v view · e edit · d delete · esc cancel";
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break AgentListAction::Cancel;
        };
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if !entries.is_empty() && selected + 1 < entries.len() {
                        selected += 1;
                    }
                }
                KeyCode::Enter | KeyCode::Char('l' | 'L') => {
                    if !entries.is_empty() {
                        break AgentListAction::Load(selected);
                    }
                }
                KeyCode::Char('v' | 'V') => {
                    if !entries.is_empty() {
                        break AgentListAction::View(selected);
                    }
                }
                KeyCode::Char('e' | 'E') => {
                    if !entries.is_empty() {
                        break AgentListAction::Edit(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break AgentListAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break AgentListAction::Cancel,
                _ => {}
            }
        } else {
            continue;
        }

        let _ = execute!(
            stdout,
            cursor::MoveUp(rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        lines = build_agents_list_lines(selected, entries, loaded_name.as_deref());
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
    };

    let _ = execute!(
        stdout,
        cursor::MoveUp(rendered as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn view_all_agents(messages: &mut [Message], show_error: &dyn Fn(&str)) -> bool {
    loop {
        let entries = agents::list_agents();
        if entries.is_empty() {
            println!();
            println!(
                "  {}",
                "No agents found. Create one first.".with(Color::DarkGrey)
            );
            println!();
            return false;
        }
        match select_agent_from_list(&entries) {
            AgentListAction::Cancel => return false,
            AgentListAction::Load(i) => {
                let entry = &entries[i];
                let Ok(prompt) = agents::read_agent(&entry.name) else {
                    show_error("Failed to read agent file.");
                    return false;
                };
                agents::load_agent(&entry.name, &prompt);
                rebuild_system_prompt(messages);
                println!();
                println!(
                    "  {} agent \"{}\" loaded",
                    "✓".with(Color::Green),
                    entry.name.as_str().with(Color::Magenta)
                );
                println!();
                return true;
            }
            AgentListAction::View(i) => {
                let entry = &entries[i];
                let Ok(prompt) = agents::read_agent(&entry.name) else {
                    show_error("Failed to read agent file.");
                    continue;
                };
                println!();
                println!(
                    "  {} {}",
                    "agent:".with(Color::Cyan),
                    entry.name.as_str().with(Color::Magenta)
                );
                println!();
                for line in prompt.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
                // After viewing, return to the list
            }
            AgentListAction::Edit(i) => {
                let entry = &entries[i];
                let is_loaded = agents::loaded_agent_name().as_deref() == Some(entry.name.as_str());
                if edit_agent_prompt(&entry.name, is_loaded, messages, show_error) == Some(true) {
                    return true;
                }
                // Return to the list
            }
            AgentListAction::Delete(i) => {
                let entry = &entries[i];
                if !confirm_yn(&format!("delete agent \"{}\"?", entry.name)) {
                    continue;
                }
                // If deleting the currently loaded agent, unload it first
                if agents::loaded_agent_name().as_deref() == Some(entry.name.as_str()) {
                    agents::unload_agent();
                    rebuild_system_prompt(messages);
                }
                if let Err(e) = agents::delete_agent(&entry.name) {
                    show_error(&format!("Failed to delete agent: {e}"));
                } else {
                    println!();
                    println!("  {} agent deleted", "✓".with(Color::Green));
                    println!();
                }
            }
        }
    }
}

/// Edit an agent's prompt. Returns `Some(true)` if the system prompt was rebuilt,
/// `Some(false)` if saved but agent not loaded, `None` if cancelled.
fn edit_agent_prompt(
    name: &str,
    is_loaded: bool,
    messages: &mut [Message],
    show_error: &dyn Fn(&str),
) -> Option<bool> {
    let Ok(current_prompt) = agents::read_agent(name) else {
        show_error("Failed to read agent file.");
        return None;
    };

    println!();
    println!(
        "  {} {}",
        "editing agent:".with(Color::Cyan),
        name.with(Color::Magenta)
    );
    println!();
    println!(
        "  {}",
        "Edit prompt below (Ctrl+D to finish, Esc to cancel):".with(Color::DarkGrey)
    );
    let new_prompt = read_multiline_input_prefilled(&current_prompt)?;
    let new_prompt = new_prompt.trim().to_string();
    if new_prompt.is_empty() {
        show_error("Empty prompt, agent not updated.");
        return None;
    }

    if let Err(e) = agents::save_agent(name, &new_prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return None;
    }

    let rebuilt = if is_loaded {
        agents::load_agent(name, &new_prompt);
        rebuild_system_prompt(messages);
        true
    } else {
        false
    };

    println!();
    println!(
        "  {} agent \"{}\" updated",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    Some(rebuilt)
}

fn unload_agent_action(messages: &mut [Message]) -> bool {
    if agents::unload_agent() {
        rebuild_system_prompt(messages);
        println!();
        println!("  {} agent unloaded", "✓".with(Color::Green));
        println!();
        true
    } else {
        println!();
        println!("  {} no agent loaded", "✗".with(Color::DarkGrey));
        println!();
        false
    }
}

/// Rebuild the system prompt (messages[0]) including any loaded agent prompt.
fn rebuild_system_prompt(messages: &mut [Message]) {
    if messages.is_empty() {
        return;
    }
    messages[0].content = crate::build_system_prompt();
}

/// Print saved agents in non-interactive mode.
pub fn print_agents_cli() {
    let entries = crate::agents::list_agents();
    if entries.is_empty() {
        println!("(no saved agents)");
        return;
    }
    for e in &entries {
        println!("{}", e.name);
    }
}
