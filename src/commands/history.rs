//! `/history` — view and search the current conversation without scrolling.
//!
//! Renders the in-memory `messages` vector as a numbered, role-coloured list
//! with one truncated preview line per entry. Args:
//!
//! - `/history`                       — show all messages
//! - `/history user|assistant|system` — filter by role
//! - `/history <keyword>`             — case-insensitive substring search
//! - `/history <role> <keyword>`      — combine role + keyword
//!
//! Pure read of `&[Message]`; nothing is mutated.

use crossterm::style::{Color, Stylize};

use crate::{Message, Role};

const MAX_PREVIEW_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoleFilter {
    Any,
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Filter {
    role: RoleFilter,
    keyword: Option<String>,
}

fn parse_args(args: &str) -> Filter {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Filter {
            role: RoleFilter::Any,
            keyword: None,
        };
    }
    let (first, rest) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(a, b)| (a, b.trim()));
    let (role, kw_src) = match first.to_lowercase().as_str() {
        "user" => (RoleFilter::User, rest),
        "assistant" => (RoleFilter::Assistant, rest),
        "system" => (RoleFilter::System, rest),
        _ => (RoleFilter::Any, trimmed),
    };
    let keyword = if kw_src.is_empty() {
        None
    } else {
        Some(kw_src.to_string())
    };
    Filter { role, keyword }
}

fn role_matches(filter: RoleFilter, role: &Role) -> bool {
    matches!(
        (filter, role),
        (RoleFilter::Any, _)
            | (RoleFilter::User, Role::User)
            | (RoleFilter::Assistant, Role::Assistant)
            | (RoleFilter::System, Role::System)
    )
}

/// Classify a message into a display label + colour. User messages whose
/// content is wrapped in `<tool_result>` are surfaced as `tool` so they
/// don't get visually mixed in with real user prompts.
fn label_and_color(msg: &Message) -> (&'static str, Color) {
    match msg.role {
        Role::System => ("system", Color::DarkGrey),
        Role::Assistant => ("assistant", Color::Green),
        Role::User => {
            let content = msg.content.trim_start();
            if content.starts_with("<tool_result>") {
                ("tool", Color::Magenta)
            } else if content.starts_with("Tool call denied") {
                ("denied", Color::Yellow)
            } else {
                ("user", Color::Cyan)
            }
        }
    }
}

/// Collapse the message body to a single line and cap its char length.
fn truncate_preview(content: &str) -> String {
    let collapsed: String = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ⏎ ");
    let char_count = collapsed.chars().count();
    if char_count <= MAX_PREVIEW_CHARS {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(MAX_PREVIEW_CHARS).collect();
        format!("{truncated}…")
    }
}

fn filter_label(filter: &Filter) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let role_str = match filter.role {
        RoleFilter::Any => None,
        RoleFilter::User => Some("user"),
        RoleFilter::Assistant => Some("assistant"),
        RoleFilter::System => Some("system"),
    };
    if let Some(r) = role_str {
        parts.push(format!("role={r}"));
    }
    if let Some(kw) = &filter.keyword {
        parts.push(format!("keyword=\"{kw}\""));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

pub fn print_history(messages: &[Message], args: &str) {
    let filter = parse_args(args);
    let kw_lower = filter.keyword.as_ref().map(|k| k.to_lowercase());

    let matches: Vec<(usize, &Message)> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| role_matches(filter.role, &m.role))
        .filter(|(_, m)| {
            kw_lower
                .as_ref()
                .is_none_or(|kw| m.content.to_lowercase().contains(kw))
        })
        .collect();

    println!();
    let header = match filter_label(&filter) {
        Some(label) => format!(
            "{} of {} messages match · filter: {label}",
            matches.len(),
            messages.len(),
        ),
        None => format!("{} messages", messages.len()),
    };
    println!("  {}", header.with(Color::DarkGrey));
    println!();

    if matches.is_empty() {
        println!("  {}", "no messages match".with(Color::Yellow));
        println!();
        return;
    }

    let max_idx_width = matches.last().map_or(1, |(i, _)| (i + 1).to_string().len());

    for (idx, msg) in matches {
        let (label, color) = label_and_color(msg);
        let preview = truncate_preview(&msg.content);
        let img_marker = if msg.images.is_empty() {
            String::new()
        } else {
            let n = msg.images.len();
            let plural = if n == 1 { "" } else { "s" };
            format!(" [+{n} image{plural}]")
        };
        println!(
            "  {} {} {}{}",
            format!("[{:>width$}]", idx + 1, width = max_idx_width).with(Color::DarkGrey),
            format!("{label:<9}").with(color),
            preview,
            img_marker.with(Color::DarkGrey),
        );
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            images: vec![],
        }
    }

    #[test]
    fn parse_empty() {
        let f = parse_args("");
        assert_eq!(f.role, RoleFilter::Any);
        assert!(f.keyword.is_none());
    }

    #[test]
    fn parse_role_only() {
        for (input, expected) in [
            ("user", RoleFilter::User),
            ("assistant", RoleFilter::Assistant),
            ("system", RoleFilter::System),
            ("USER", RoleFilter::User),
            ("  assistant  ", RoleFilter::Assistant),
        ] {
            let f = parse_args(input);
            assert_eq!(f.role, expected, "input={input:?}");
            assert!(f.keyword.is_none(), "input={input:?}");
        }
    }

    #[test]
    fn parse_keyword_only() {
        let f = parse_args("rust");
        assert_eq!(f.role, RoleFilter::Any);
        assert_eq!(f.keyword.as_deref(), Some("rust"));
    }

    #[test]
    fn parse_multi_word_keyword() {
        // First token is not a role → entire input is the keyword.
        let f = parse_args("hello world");
        assert_eq!(f.role, RoleFilter::Any);
        assert_eq!(f.keyword.as_deref(), Some("hello world"));
    }

    #[test]
    fn parse_role_plus_keyword() {
        let f = parse_args("user hello world");
        assert_eq!(f.role, RoleFilter::User);
        assert_eq!(f.keyword.as_deref(), Some("hello world"));
    }

    #[test]
    fn role_matches_works() {
        assert!(role_matches(RoleFilter::Any, &Role::User));
        assert!(role_matches(RoleFilter::User, &Role::User));
        assert!(!role_matches(RoleFilter::User, &Role::Assistant));
        assert!(role_matches(RoleFilter::System, &Role::System));
    }

    #[test]
    fn truncate_collapses_lines() {
        let s = truncate_preview("line one\n\nline two\n  line three  ");
        assert_eq!(s, "line one ⏎ line two ⏎ line three");
    }

    #[test]
    fn truncate_caps_long_content() {
        let long = "a".repeat(MAX_PREVIEW_CHARS + 50);
        let out = truncate_preview(&long);
        assert_eq!(out.chars().count(), MAX_PREVIEW_CHARS + 1); // +1 for the ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_handles_multibyte() {
        // 250 emoji chars (each multiple bytes); should not panic and should cap at MAX+1 chars.
        let s = "🚀".repeat(MAX_PREVIEW_CHARS + 50);
        let out = truncate_preview(&s);
        assert_eq!(out.chars().count(), MAX_PREVIEW_CHARS + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn label_distinguishes_tool_results() {
        let (label, _) = label_and_color(&msg(Role::User, "<tool_result>\nhi\n</tool_result>"));
        assert_eq!(label, "tool");
    }

    #[test]
    fn label_distinguishes_denied_tool() {
        let (label, _) = label_and_color(&msg(Role::User, "Tool call denied by user. Try again."));
        assert_eq!(label, "denied");
    }

    #[test]
    fn label_plain_user_message() {
        let (label, _) = label_and_color(&msg(Role::User, "what's the weather?"));
        assert_eq!(label, "user");
    }

    #[test]
    fn filter_label_renders_combined() {
        let f = Filter {
            role: RoleFilter::User,
            keyword: Some("rust".to_string()),
        };
        assert_eq!(
            filter_label(&f).as_deref(),
            Some("role=user, keyword=\"rust\"")
        );
    }

    #[test]
    fn filter_label_none_for_empty() {
        let f = Filter {
            role: RoleFilter::Any,
            keyword: None,
        };
        assert!(filter_label(&f).is_none());
    }

    #[test]
    fn print_history_does_not_panic_on_empty() {
        // Sanity check: empty messages renders the "no messages match" path.
        print_history(&[], "");
    }

    #[test]
    fn print_history_does_not_panic_with_filters() {
        let messages = vec![
            msg(Role::System, "system prompt"),
            msg(Role::User, "hello rust"),
            msg(Role::Assistant, "hi there"),
            msg(Role::User, "<tool_result>\noutput\n</tool_result>"),
        ];
        print_history(&messages, "user rust");
        print_history(&messages, "assistant");
        print_history(&messages, "nomatch");
    }
}
