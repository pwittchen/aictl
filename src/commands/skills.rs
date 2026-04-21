//! Interactive `/skills` menu and CLI helpers.
//!
//! Mirrors the `/agent` UX so users familiar with one pick up the other
//! immediately. The menu creates, browses, and deletes skill files under
//! `~/.aictl/skills/` (or `AICTL_SKILLS_DIR`). Actual skill *invocation* for
//! `/<skill-name>` happens in the REPL — this module just maintains the
//! storage and offers a convenience "invoke now" entry from the list view.

use std::io::Write;

use crossterm::style::{Color, Stylize};

use crate::Provider;
use crate::skills;
use crate::ui::AgentUI;
use crate::{Message, Role};

use super::menu::{confirm_yn, read_input_line, read_multiline_input, select_from_menu};

const SKILLS_MENU_ITEMS: &[(&str, &str)] = &[
    ("create skill manually", "type or paste skill body"),
    ("create skill with AI", "describe what the skill should do"),
    (
        "browse official skills",
        "pull curated skills from the aictl repo",
    ),
    ("view all skills", "browse, view, invoke, or delete skills"),
];

fn build_skills_menu_lines(selected: usize) -> Vec<String> {
    let max_name = SKILLS_MENU_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    SKILLS_MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == selected;
            let padded = format!("{name:<max_name$}");
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
            if is_selected {
                format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {desc_styled}")
            }
        })
        .collect()
}

/// Returned by the menu so the REPL knows whether to immediately invoke a
/// selected skill after the menu closes.
pub enum SkillsMenuOutcome {
    Nothing,
    Invoke { name: String, task: String },
}

/// Run the `/skills` menu. Returns whether the REPL should invoke a skill
/// after the menu exits (user picked "invoke now" from the list).
pub async fn run_skills_menu(
    provider: &Provider,
    api_key: &str,
    model: &str,
    ui: &dyn AgentUI,
    show_error: &dyn Fn(&str),
) -> SkillsMenuOutcome {
    let Some(sel) = select_from_menu(SKILLS_MENU_ITEMS.len(), 0, build_skills_menu_lines) else {
        return SkillsMenuOutcome::Nothing;
    };
    match SKILLS_MENU_ITEMS[sel].0 {
        "create skill manually" => {
            create_skill_manually(show_error);
            SkillsMenuOutcome::Nothing
        }
        "create skill with AI" => {
            create_skill_with_ai(provider, api_key, model, ui, show_error).await;
            SkillsMenuOutcome::Nothing
        }
        "browse official skills" => {
            browse_official_skills(ui, show_error).await;
            SkillsMenuOutcome::Nothing
        }
        "view all skills" => view_all_skills(show_error),
        _ => SkillsMenuOutcome::Nothing,
    }
}

fn read_name_and_description(show_error: &dyn Fn(&str)) -> Option<(String, String)> {
    let name = read_input_line("skill name:", false)?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    if !skills::is_valid_name(&name) {
        show_error("Invalid name. Use only letters, numbers, underscore, or dash.");
        return None;
    }
    if skills::is_reserved_name(&name) {
        show_error(&format!(
            "\"{name}\" is a reserved slash-command name; choose a different skill name."
        ));
        return None;
    }
    let description = read_input_line("description:", false)?.trim().to_string();
    if description.is_empty() {
        show_error("Description is required.");
        return None;
    }
    Some((name, description))
}

fn create_skill_manually(show_error: &dyn Fn(&str)) {
    let Some((name, description)) = read_name_and_description(show_error) else {
        return;
    };

    println!();
    println!(
        "  {}",
        "Enter skill body (multi-line: Ctrl+D to finish, Esc to cancel):".with(Color::DarkGrey)
    );
    let Some(body) = read_multiline_input() else {
        return;
    };
    let body = body.trim().to_string();
    if body.is_empty() {
        show_error("Empty body, skill not created.");
        return;
    }

    if let Err(e) = skills::save(&name, &description, &body) {
        show_error(&format!("Failed to save skill: {e}"));
        return;
    }
    println!();
    println!(
        "  {} skill \"{}\" created",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
}

#[allow(clippy::too_many_lines)]
async fn create_skill_with_ai(
    provider: &Provider,
    api_key: &str,
    model: &str,
    ui: &dyn AgentUI,
    show_error: &dyn Fn(&str),
) {
    let Some((name, description)) = read_name_and_description(show_error) else {
        return;
    };

    ui.start_spinner("generating skill body...");

    let gen_messages = vec![
        Message {
            role: Role::System,
            content: "You are an expert at writing procedural \"skills\" — short markdown playbooks that tell another AI assistant how to perform a specific, repeatable task. \
                Generate the body of a skill based on the user's description. The body should be a clear, numbered set of steps the assistant should follow when invoked, \
                including which tools to use and how to phrase the final output. Do NOT include YAML frontmatter or a heading with the skill name — only the procedure body. Output ONLY the markdown body, nothing else."
                .to_string(),
            images: vec![],
        },
        Message {
            role: Role::User,
            content: format!(
                "Create a skill named \"{name}\" that does the following: {description}"
            ),
            images: vec![],
        },
    ];

    let llm_timeout = crate::config::llm_timeout();
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
            return;
        }
    };

    let result = match result {
        Ok(inner) => inner,
        Err(_elapsed) => {
            show_error(&format!(
                "Skill generation timed out after {}s (AICTL_LLM_TIMEOUT).",
                llm_timeout.as_secs()
            ));
            return;
        }
    };

    let (body, _usage) = match result {
        Ok(r) => r,
        Err(e) => {
            show_error(&format!("Failed to generate skill body: {e}"));
            return;
        }
    };

    let body = body.trim().to_string();
    println!();
    println!("  {}", "Generated skill body:".with(Color::Cyan));
    println!();
    for line in body.lines() {
        println!("  {}", line.with(Color::DarkGrey));
    }
    println!();

    if !confirm_yn("save this skill?") {
        return;
    }

    if let Err(e) = skills::save(&name, &description, &body) {
        show_error(&format!("Failed to save skill: {e}"));
        return;
    }
    println!();
    println!(
        "  {} skill \"{}\" created",
        "✓".with(Color::Green),
        name.with(Color::Magenta)
    );
    println!();
}

fn build_skills_list_lines(selected: usize, entries: &[skills::SkillEntry]) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no skills found)".with(Color::DarkGrey))];
    }
    let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let is_selected = i == selected;
        let padded = format!("{:<max_name$}", e.name);
        let name_styled = if is_selected {
            format!(
                "{}",
                padded
                    .as_str()
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
            )
        } else {
            format!("{}", padded.as_str().with(Color::DarkGrey))
        };
        let badge = if e.is_official() {
            format!("  {}", "[official]".with(Color::Cyan))
        } else {
            String::new()
        };
        let desc_styled = format!("{}", e.description.as_str().with(Color::DarkGrey));
        let line = if is_selected {
            format!(
                "  {} {name_styled}{badge}  {desc_styled}",
                "›".with(Color::Cyan)
            )
        } else {
            format!("    {name_styled}{badge}  {desc_styled}")
        };
        lines.push(line);
    }
    lines
}

enum SkillListAction {
    Invoke(usize),
    View(usize),
    Delete(usize),
    Cancel,
}

#[allow(clippy::cast_possible_truncation)]
fn select_skill_from_list(entries: &[skills::SkillEntry]) -> SkillListAction {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected: usize = 0;
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let mut lines = build_skills_list_lines(selected, entries);
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let hint = "↑/↓ navigate · enter/i invoke · v view · d delete · esc cancel";
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(ev) = event::read() else {
            break SkillListAction::Cancel;
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
                KeyCode::Enter | KeyCode::Char('i' | 'I') => {
                    if !entries.is_empty() {
                        break SkillListAction::Invoke(selected);
                    }
                }
                KeyCode::Char('v' | 'V') => {
                    if !entries.is_empty() {
                        break SkillListAction::View(selected);
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if !entries.is_empty() {
                        break SkillListAction::Delete(selected);
                    }
                }
                KeyCode::Esc => break SkillListAction::Cancel,
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
        lines = build_skills_list_lines(selected, entries);
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
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

fn view_all_skills(show_error: &dyn Fn(&str)) -> SkillsMenuOutcome {
    loop {
        let entries = skills::list();
        if entries.is_empty() {
            println!();
            println!(
                "  {}",
                "No skills found. Create one first.".with(Color::DarkGrey)
            );
            println!();
            return SkillsMenuOutcome::Nothing;
        }
        match select_skill_from_list(&entries) {
            SkillListAction::Cancel => return SkillsMenuOutcome::Nothing,
            SkillListAction::Invoke(i) => {
                let entry = &entries[i];
                // No task prompt — the skill body alone drives the turn.
                return SkillsMenuOutcome::Invoke {
                    name: entry.name.clone(),
                    task: String::new(),
                };
            }
            SkillListAction::View(i) => {
                let entry = &entries[i];
                let Some(skill) = skills::find(&entry.name) else {
                    show_error("Failed to read skill file.");
                    continue;
                };
                println!();
                let title = if entry.is_official() {
                    format!(
                        "  {} {}  {}",
                        "skill:".with(Color::Cyan),
                        skill.name.as_str().with(Color::Magenta),
                        "[official]".with(Color::Cyan)
                    )
                } else {
                    format!(
                        "  {} {}",
                        "skill:".with(Color::Cyan),
                        skill.name.as_str().with(Color::Magenta)
                    )
                };
                println!("{title}");
                println!(
                    "  {} {}",
                    "description:".with(Color::Cyan),
                    skill.description.as_str().with(Color::DarkGrey)
                );
                if let Some(cat) = entry.category.as_deref() {
                    println!("  {} {cat}", "category:   ".with(Color::Cyan));
                }
                println!();
                for line in skill.body.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
            }
            SkillListAction::Delete(i) => {
                let entry = &entries[i];
                if !confirm_yn(&format!("delete skill \"{}\"?", entry.name)) {
                    continue;
                }
                if let Err(e) = skills::delete(&entry.name) {
                    show_error(&format!("Failed to delete skill: {e}"));
                } else {
                    println!();
                    println!("  {} skill deleted", "✓".with(Color::Green));
                    println!();
                }
            }
        }
    }
}

/// Print saved skills in non-interactive mode (used by `--list-skills`). When
/// `category` is `Some`, only skills whose frontmatter carries a matching
/// `category:` value are shown (case-insensitive).
pub fn print_skills_cli(category: Option<&str>) {
    let entries = skills::list();
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
            None => println!("(no saved skills)"),
            Some(c) => println!("(no saved skills in category '{c}')"),
        }
        return;
    }
    let max_name = filtered.iter().map(|e| e.name.len()).max().unwrap_or(0);
    for e in &filtered {
        let badge = if e.is_official() { " [official]" } else { "" };
        if e.description.is_empty() {
            println!("{:<max_name$}{badge}", e.name);
        } else {
            println!("{:<max_name$}{badge}  {}", e.name, e.description);
        }
    }
}

// --- Remote catalogue browsing ---

use crate::skills::remote::{self, PullOutcome, RemoteSkill, State};

/// Open the remote catalogue browser. Fetches the list, shows categories,
/// drills into skills, and pulls the selected one to
/// `~/.aictl/skills/<name>/SKILL.md`.
async fn browse_official_skills(ui: &dyn AgentUI, show_error: &dyn Fn(&str)) {
    ui.start_spinner("fetching catalogue...");
    let list = remote::list_skills().await;
    ui.stop_spinner();
    let skills_list = match list {
        Ok(list) if list.is_empty() => {
            println!();
            println!(
                "  {}",
                "The official catalogue is empty.".with(Color::DarkGrey)
            );
            println!();
            return;
        }
        Ok(list) => list,
        Err(e) => {
            show_error(&format!("Failed to fetch catalogue: {e}"));
            return;
        }
    };

    loop {
        let categories = group_by_category(&skills_list);
        let Some(choice) = select_category(&categories, skills_list.len()) else {
            return;
        };
        let filtered: Vec<&RemoteSkill> = match choice {
            CategoryChoice::All => skills_list.iter().collect(),
            CategoryChoice::Named(name) => skills_list
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
fn group_by_category(skills_list: &[RemoteSkill]) -> Vec<(String, usize)> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for s in skills_list {
        *counts.entry(category_label(s)).or_default() += 1;
    }
    counts.into_iter().collect()
}

fn category_label(skill: &RemoteSkill) -> String {
    skill
        .category
        .clone()
        .unwrap_or_else(|| "uncategorized".to_string())
}

fn select_category(categories: &[(String, usize)], total: usize) -> Option<CategoryChoice> {
    use super::menu::build_simple_menu_lines;

    let mut items: Vec<(String, String)> = vec![("all".to_string(), format!("{total} skills"))];
    for (name, count) in categories {
        items.push((name.clone(), format!("{count} skill{}", plural(*count))));
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

/// Render the skill list for the selected category and pull whatever the
/// user picks. Returns when the user presses Esc from the skill list.
fn pull_from_browse(skills_list: &[&RemoteSkill], show_error: &dyn Fn(&str)) {
    let mut selected: usize = 0;
    loop {
        let Some(action) = select_remote_skill(skills_list, selected) else {
            return;
        };
        match action {
            RemoteListAction::Cancel => return,
            RemoteListAction::Pull(idx) => {
                selected = idx;
                let skill = skills_list[idx];
                match pull_remote_skill(skill) {
                    Ok(PullOutcome::Installed) => {
                        println!();
                        println!(
                            "  {} skill \"{}\" pulled",
                            "✓".with(Color::Green),
                            skill.name.as_str().with(Color::Magenta)
                        );
                        println!();
                    }
                    Ok(PullOutcome::Overwritten) => {
                        println!();
                        println!(
                            "  {} skill \"{}\" updated",
                            "✓".with(Color::Green),
                            skill.name.as_str().with(Color::Magenta)
                        );
                        println!();
                    }
                    Ok(PullOutcome::SkippedExisting) => {
                        println!();
                        println!(
                            "  {} keeping existing \"{}\"",
                            "·".with(Color::DarkGrey),
                            skill.name.as_str().with(Color::Magenta)
                        );
                        println!();
                    }
                    Err(e) => show_error(&format!("Failed to pull skill: {e}")),
                }
            }
            RemoteListAction::View(idx) => {
                selected = idx;
                let skill = skills_list[idx];
                println!();
                println!(
                    "  {} {}  {}",
                    "skill:".with(Color::Cyan),
                    skill.name.as_str().with(Color::Magenta),
                    "[official]".with(Color::Cyan)
                );
                if let Some(desc) = skill.description.as_deref() {
                    println!("  {} {desc}", "description:".with(Color::Cyan));
                }
                if let Some(cat) = skill.category.as_deref() {
                    println!("  {} {cat}", "category:   ".with(Color::Cyan));
                }
                println!();
                // Strip frontmatter before displaying so the preview matches
                // what the skill actually injects into a turn.
                let body = crate::skills::parse(&skill.body).body;
                for line in body.lines() {
                    println!("  {}", line.with(Color::DarkGrey));
                }
                println!();
            }
        }
    }
}

fn pull_remote_skill(skill: &RemoteSkill) -> Result<PullOutcome, String> {
    let dir = crate::skills::skills_dir().join(&skill.name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let path = dir.join("SKILL.md");
    remote::write_with_overwrite(&path, &skill.body, || {
        confirm_yn(&format!(
            "skill \"{}\" already exists. Overwrite?",
            skill.name
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

fn build_remote_skills_list_lines(selected: usize, entries: &[&RemoteSkill]) -> Vec<String> {
    if entries.is_empty() {
        return vec![format!("  {}", "(no skills)".with(Color::DarkGrey))];
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
fn select_remote_skill(entries: &[&RemoteSkill], initial: usize) -> Option<RemoteListAction> {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{self, ClearType},
    };

    let mut selected: usize = initial.min(entries.len().saturating_sub(1));
    let _ = terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, cursor::Hide);

    let hint = "↑/↓ navigate · p/enter pull · v view · esc back";

    let mut lines = build_remote_skills_list_lines(selected, entries);
    let _ = write!(stdout, "\r\n");
    for line in &lines {
        let _ = write!(stdout, "{line}\r\n");
    }
    let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
    let _ = stdout.flush();
    let mut rendered = lines.len() + 2;

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

        let _ = execute!(
            stdout,
            cursor::MoveUp(rendered as u16),
            terminal::Clear(ClearType::FromCursorDown),
        );
        lines = build_remote_skills_list_lines(selected, entries);
        for line in &lines {
            let _ = write!(stdout, "{line}\r\n");
        }
        let _ = write!(stdout, "\r\n  {}\r\n", hint.with(Color::DarkGrey));
        let _ = stdout.flush();
        rendered = lines.len() + 2;
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
