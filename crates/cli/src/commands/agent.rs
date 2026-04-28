use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::agents;
use crate::ui::AgentUI;
use crate::{Message, Provider, Role};

use super::menu::{
    confirm_yn, menu_viewport_height, read_input_line, read_multiline_input,
    read_multiline_input_prefilled, render_menu_viewport, select_from_menu,
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
        (
            "browse official agents",
            "pull curated agents from the aictl repo",
        ),
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
        "browse official agents" => browse_official_agents(ui, show_error).await,
        "view all agents" => view_all_agents(messages, show_error),
        "edit agent" => {
            let Some(name) = agents::loaded_agent_name() else {
                return false;
            };
            // Resolve the on-disk entry so the edit lands at the same origin
            // that `read_agent` would load from (local file when one exists).
            let Some(entry) = agents::list_agents().into_iter().find(|e| e.name == name) else {
                show_error(&format!("Agent '{name}' file not found on disk."));
                return false;
            };
            edit_agent_prompt(&entry, true, messages, show_error).unwrap_or(false)
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
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::openai::call_openai(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Anthropic => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::anthropic::call_anthropic(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Gemini => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::gemini::call_gemini(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Grok => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::grok::call_grok(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Mistral => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::mistral::call_mistral(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Deepseek => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::deepseek::call_deepseek(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Kimi => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::kimi::call_kimi(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Zai => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::zai::call_zai(api_key, model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Ollama => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::ollama::call_ollama(model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Gguf => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::gguf::call_gguf(model, &gen_messages, None),
                ),
            )
            .await
        }
        Provider::Mlx => {
            crate::with_esc_cancel(
                ui,
                tokio::time::timeout(
                    llm_timeout,
                    crate::llm::mlx::call_mlx(model, &gen_messages, None),
                ),
            )
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
    let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let is_loaded = loaded_name == Some(e.name.as_str());
        let marker = if is_loaded { "●" } else { " " };
        let padded = format!("{:<max_name$}", e.name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!(
                "{} {}",
                marker.with(Color::Green),
                padded.with(Color::DarkGrey)
            )
        };
        let mut badge = format!(
            "  {}",
            format!("[{}]", e.origin.label()).with(Color::DarkGrey)
        );
        if e.is_official() {
            use std::fmt::Write;
            let _ = write!(badge, "  {}", "[official]".with(Color::Cyan));
        }
        let line = if is_selected {
            format!("  {} {name_styled}{badge}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}{badge}")
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
    let mut scroll_offset: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let max_visible = menu_viewport_height();
    let hint = "↑/↓ navigate · l/enter load · v view · e edit · d delete · esc cancel";

    let lines = build_agents_list_lines(selected, entries, loaded_name.as_deref());
    let _ = write!(stdout, "\r\n");
    let mut rendered = render_menu_viewport(
        &mut stdout,
        &lines,
        &mut scroll_offset,
        0,
        max_visible,
        hint,
    );

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

        let lines = build_agents_list_lines(selected, entries, loaded_name.as_deref());
        rendered = render_menu_viewport(
            &mut stdout,
            &lines,
            &mut scroll_offset,
            rendered,
            max_visible,
            hint,
        );
    };

    // +1 consumes the leading `\r\n` written before the first render so the
    // menu leaves the cursor where it started rather than one row down.
    let _ = execute!(
        stdout,
        cursor::MoveUp((rendered + 1) as u16),
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
                let Ok(meta) = agents::read_agent_meta(&entry.name) else {
                    show_error("Failed to read agent file.");
                    continue;
                };
                println!();
                let origin_tag = format!("[{}]", entry.origin.label())
                    .with(Color::DarkGrey)
                    .to_string();
                let title = if entry.is_official() {
                    format!(
                        "  {} {}  {}  {}",
                        "agent:".with(Color::Cyan),
                        entry.name.as_str().with(Color::Magenta),
                        origin_tag,
                        "[official]".with(Color::Cyan)
                    )
                } else {
                    format!(
                        "  {} {}  {}",
                        "agent:".with(Color::Cyan),
                        entry.name.as_str().with(Color::Magenta),
                        origin_tag
                    )
                };
                println!("{title}");
                if let Some(desc) = meta.description.as_deref() {
                    println!("  {} {desc}", "description:".with(Color::Cyan));
                }
                if let Some(cat) = meta.category.as_deref() {
                    println!("  {} {cat}", "category:   ".with(Color::Cyan));
                }
                println!();
                for line in meta.body.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
                // After viewing, return to the list
            }
            AgentListAction::Edit(i) => {
                let entry = &entries[i];
                let is_loaded = agents::loaded_agent_name().as_deref() == Some(entry.name.as_str());
                if edit_agent_prompt(entry, is_loaded, messages, show_error) == Some(true) {
                    return true;
                }
                // Return to the list
            }
            AgentListAction::Delete(i) => {
                let entry = &entries[i];
                if !confirm_yn(&format!(
                    "delete {} agent \"{}\"?",
                    entry.origin.label(),
                    entry.name
                )) {
                    continue;
                }
                // If deleting the currently loaded agent, unload it first
                if agents::loaded_agent_name().as_deref() == Some(entry.name.as_str()) {
                    agents::unload_agent();
                    rebuild_system_prompt(messages);
                }
                if let Err(e) = agents::delete_agent_entry(entry) {
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

/// Edit an agent's prompt at its on-disk location. Returns `Some(true)` if
/// the system prompt was rebuilt, `Some(false)` if saved but agent not
/// loaded, `None` if cancelled. The rewrite stays at `entry.path` so a
/// local-origin agent edit doesn't silently fork into the global catalogue.
fn edit_agent_prompt(
    entry: &agents::AgentEntry,
    is_loaded: bool,
    messages: &mut [Message],
    show_error: &dyn Fn(&str),
) -> Option<bool> {
    let Ok(current_prompt) = std::fs::read_to_string(&entry.path) else {
        show_error("Failed to read agent file.");
        return None;
    };

    println!();
    println!(
        "  {} {}  {}",
        "editing agent:".with(Color::Cyan),
        entry.name.as_str().with(Color::Magenta),
        format!("[{}]", entry.origin.label()).with(Color::DarkGrey)
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

    if let Err(e) = agents::save_agent_entry(entry, &new_prompt) {
        show_error(&format!("Failed to save agent: {e}"));
        return None;
    }

    let rebuilt = if is_loaded {
        agents::load_agent(&entry.name, &new_prompt);
        rebuild_system_prompt(messages);
        true
    } else {
        false
    };

    println!();
    println!(
        "  {} agent \"{}\" updated",
        "✓".with(Color::Green),
        entry.name.as_str().with(Color::Magenta)
    );
    println!();
    Some(rebuilt)
}

/// Load an agent by name directly (bypassing the menu). Used by the REPL
/// `/agent <name>` shortcut. Returns `true` if the agent was loaded and the
/// system prompt rebuilt; `false` on error (error is surfaced through
/// `show_error`).
pub fn load_agent_by_name(name: &str, messages: &mut [Message], show_error: &dyn Fn(&str)) -> bool {
    if !agents::is_valid_name(name) {
        show_error("Invalid agent name. Use only letters, numbers, underscore, or dash.");
        return false;
    }
    let Ok(prompt) = agents::read_agent(name) else {
        show_error(&format!("Agent '{name}' not found"));
        return false;
    };
    agents::load_agent(name, &prompt);
    rebuild_system_prompt(messages);
    println!();
    println!(
        "  {} agent \"{}\" loaded",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
    true
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

// --- Remote catalogue browsing ---

use crate::agents::remote::{self, PullOutcome, RemoteAgent, State};

/// Open the remote catalogue browser. Fetches the list, shows categories,
/// drills into agents, and pulls the selected one to `~/.aictl/agents/`.
/// Returns `false` because pulling a catalogue agent never loads it — the
/// user still has to pick it from "view all" to bring it into the session.
async fn browse_official_agents(ui: &dyn AgentUI, show_error: &dyn Fn(&str)) -> bool {
    ui.start_spinner("fetching catalogue...");
    let agents_list = remote::list_agents().await;
    ui.stop_spinner();
    let agents_list = match agents_list {
        Ok(list) if list.is_empty() => {
            println!();
            println!(
                "  {}",
                "The official catalogue is empty.".with(Color::DarkGrey)
            );
            println!();
            return false;
        }
        Ok(list) => list,
        Err(e) => {
            show_error(&format!("Failed to fetch catalogue: {e}"));
            return false;
        }
    };

    loop {
        let categories = group_by_category(&agents_list);
        let Some(choice) = select_category(&categories, agents_list.len()) else {
            return false;
        };
        let filtered: Vec<&RemoteAgent> = match choice {
            CategoryChoice::All => agents_list.iter().collect(),
            CategoryChoice::Named(name) => agents_list
                .iter()
                .filter(|a| category_label(a).eq_ignore_ascii_case(&name))
                .collect(),
        };
        if filtered.is_empty() {
            continue;
        }
        pull_from_browse(&filtered, show_error);
    }
}

/// What the category picker returns. `All` bypasses category filtering;
/// `Named` holds the label the user selected (e.g. `"dev"`).
enum CategoryChoice {
    All,
    Named(String),
}

/// Return `(label, count)` per category, sorted, with entries that lack a
/// category grouped under `uncategorized` so the browser always renders a
/// complete picture of what's upstream.
fn group_by_category(agents: &[RemoteAgent]) -> Vec<(String, usize)> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for a in agents {
        *counts.entry(category_label(a)).or_default() += 1;
    }
    counts.into_iter().collect()
}

fn category_label(agent: &RemoteAgent) -> String {
    agent
        .category
        .clone()
        .unwrap_or_else(|| "uncategorized".to_string())
}

fn select_category(categories: &[(String, usize)], total: usize) -> Option<CategoryChoice> {
    use super::menu::build_simple_menu_lines;

    let mut items: Vec<(String, String)> = vec![("all".to_string(), format!("{total} agents"))];
    for (name, count) in categories {
        items.push((name.clone(), format!("{count} agent{}", plural(*count))));
    }
    let items_ref: Vec<(&str, &str)> = items
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let sel = select_from_menu(items_ref.len(), 0, |selected| {
        build_simple_menu_lines(&items_ref, selected)
    })?;
    if sel == 0 {
        Some(CategoryChoice::All)
    } else {
        Some(CategoryChoice::Named(items[sel].0.clone()))
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Render the agent list for the selected category and pull whatever the
/// user picks. Returns when the user presses Esc from the agent list.
fn pull_from_browse(agents: &[&RemoteAgent], show_error: &dyn Fn(&str)) {
    let mut selected: usize = 0;
    loop {
        let Some(action) = select_remote_agent(agents, selected) else {
            return;
        };
        match action {
            RemoteListAction::Cancel => return,
            RemoteListAction::Pull(idx) => {
                selected = idx;
                let agent = agents[idx];
                match pull_remote_agent(agent) {
                    Ok(PullOutcome::Installed) => {
                        println!();
                        println!(
                            "  {} agent \"{}\" pulled",
                            "✓".with(Color::Green),
                            agent.name.as_str().with(Color::Magenta)
                        );
                        println!();
                    }
                    Ok(PullOutcome::Overwritten) => {
                        println!();
                        println!(
                            "  {} agent \"{}\" updated",
                            "✓".with(Color::Green),
                            agent.name.as_str().with(Color::Magenta)
                        );
                        println!();
                    }
                    Ok(PullOutcome::SkippedExisting) => {
                        println!();
                        println!(
                            "  {} keeping existing \"{}\"",
                            "·".with(Color::DarkGrey),
                            agent.name.as_str().with(Color::Magenta)
                        );
                        println!();
                    }
                    Err(e) => show_error(&format!("Failed to pull agent: {e}")),
                }
            }
            RemoteListAction::View(idx) => {
                selected = idx;
                let agent = agents[idx];
                println!();
                println!(
                    "  {} {}  {}",
                    "agent:".with(Color::Cyan),
                    agent.name.as_str().with(Color::Magenta),
                    "[official]".with(Color::Cyan)
                );
                if let Some(desc) = agent.description.as_deref() {
                    println!("  {} {desc}", "description:".with(Color::Cyan));
                }
                if let Some(cat) = agent.category.as_deref() {
                    println!("  {} {cat}", "category:   ".with(Color::Cyan));
                }
                println!();
                let body = crate::agents::parse(&agent.body).body;
                for line in body.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
            }
        }
    }
}

fn pull_remote_agent(agent: &RemoteAgent) -> Result<PullOutcome, String> {
    let dir = crate::agents::agents_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let path = dir.join(&agent.name);
    remote::write_with_overwrite(&path, &agent.body, || {
        confirm_yn(&format!(
            "agent \"{}\" already exists. Overwrite?",
            agent.name
        ))
    })
}

enum RemoteListAction {
    Pull(usize),
    View(usize),
    Cancel,
}

fn state_marker(state: State) -> (&'static str, Color) {
    match state {
        State::NotPulled => ("[ ]", Color::DarkGrey),
        State::UpToDate => ("[✓]", Color::Green),
        State::UpstreamNewer => ("[↑]", Color::Yellow),
    }
}

fn build_remote_agents_list_lines(selected: usize, entries: &[&RemoteAgent]) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no agents)".with(Color::DarkGrey))];
    }
    let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let (mark, mark_color) = state_marker(e.state);
        let padded = format!("{:<max_name$}", e.name);
        let name_styled = if is_selected {
            format!(
                "{} {}",
                mark.with(mark_color),
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("{} {}", mark.with(mark_color), padded.with(Color::DarkGrey))
        };
        let desc = e.description.as_deref().unwrap_or("");
        let desc_styled = format!("  {}", desc.with(Color::DarkGrey));
        let line = if is_selected {
            format!("  {} {name_styled}{desc_styled}", "›".with(Color::Cyan))
        } else {
            format!("    {name_styled}{desc_styled}")
        };
        lines.push(line);
    }
    lines
}

#[allow(clippy::cast_possible_truncation)]
fn select_remote_agent(entries: &[&RemoteAgent], initial: usize) -> Option<RemoteListAction> {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected: usize = initial.min(entries.len().saturating_sub(1));
    let mut scroll_offset: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let max_visible = menu_viewport_height();
    let hint = "↑/↓ navigate · p/enter pull · v view · esc back";

    let lines = build_remote_agents_list_lines(selected, entries);
    let _ = write!(stdout, "\r\n");
    let mut rendered = render_menu_viewport(
        &mut stdout,
        &lines,
        &mut scroll_offset,
        0,
        max_visible,
        hint,
    );

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break Some(RemoteListAction::Cancel);
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
                KeyCode::Enter | KeyCode::Char('p' | 'P') => {
                    if !entries.is_empty() {
                        break Some(RemoteListAction::Pull(selected));
                    }
                }
                KeyCode::Char('v' | 'V') => {
                    if !entries.is_empty() {
                        break Some(RemoteListAction::View(selected));
                    }
                }
                KeyCode::Esc => break Some(RemoteListAction::Cancel),
                _ => {}
            }
        } else {
            continue;
        }

        let lines = build_remote_agents_list_lines(selected, entries);
        rendered = render_menu_viewport(
            &mut stdout,
            &lines,
            &mut scroll_offset,
            rendered,
            max_visible,
            hint,
        );
    };

    // +1 consumes the leading `\r\n` written before the first render so the
    // menu leaves the cursor where it started rather than one row down.
    let _ = execute!(
        stdout,
        cursor::MoveUp((rendered + 1) as u16),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show,
    );
    let _ = terminal::disable_raw_mode();
    result
}

/// Print saved agents in non-interactive mode. When `category` is `Some`,
/// only agents whose frontmatter carries a matching `category:` value are
/// shown (case-insensitive).
pub fn print_agents_cli(category: Option<&str>) {
    let entries = crate::agents::list_agents();
    let filtered: Vec<_> = entries
        .iter()
        .filter(|e| match category {
            None => true,
            Some(want) => e
                .category
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(want)),
        })
        .collect();
    if filtered.is_empty() {
        match category {
            None => println!("(no saved agents)"),
            Some(c) => println!("(no saved agents in category '{c}')"),
        }
        return;
    }
    let max_name = filtered.iter().map(|e| e.name.len()).max().unwrap_or(0);
    for e in &filtered {
        let origin_badge = format!(" [{}]", e.origin.label());
        let official_badge = if e.is_official() { " [official]" } else { "" };
        match e.description.as_deref() {
            Some(desc) if !desc.is_empty() => {
                println!(
                    "{:<max_name$}{origin_badge}{official_badge}  {desc}",
                    e.name
                );
            }
            _ => println!("{:<max_name$}{origin_badge}{official_badge}", e.name),
        }
    }
}
