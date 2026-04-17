//! `/retry` — remove the last user/assistant exchange and re-submit the
//! removed user prompt so the agent takes another shot at it.
//!
//! An "exchange" spans from the most recent genuine user prompt up to and
//! including the final assistant response, plus any tool-call / tool-result
//! messages the assistant produced in between. Messages whose content starts
//! with `<tool_result>` or `Tool call denied` are artefacts of the previous
//! turn, not user prompts, so they are skipped when locating the boundary.

use crate::{Message, Role};

fn is_user_prompt(content: &str) -> bool {
    let trimmed = content.trim_start();
    !trimmed.starts_with("<tool_result>") && !trimmed.starts_with("Tool call denied")
}

/// Index of the last genuine user prompt in `messages`, or `None` if the
/// conversation has not progressed past the system prompt.
fn find_last_user_prompt(messages: &[Message]) -> Option<usize> {
    messages.iter().enumerate().rev().find_map(|(i, m)| {
        if matches!(m.role, Role::User) && is_user_prompt(&m.content) {
            Some(i)
        } else {
            None
        }
    })
}

/// Remove the last user/assistant exchange and return the removed user
/// prompt so the caller can re-submit it. Returns `None` when there is
/// nothing to retry (only the system prompt, or no genuine user prompt
/// at all).
pub fn retry_last_exchange(messages: &mut Vec<Message>) -> Option<String> {
    let idx = find_last_user_prompt(messages)?;
    let prompt = messages[idx].content.clone();
    messages.truncate(idx);
    Some(prompt)
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
    fn find_none_when_only_system() {
        let msgs = vec![msg(Role::System, "sys")];
        assert_eq!(find_last_user_prompt(&msgs), None);
    }

    #[test]
    fn find_simple_exchange() {
        let msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi"),
        ];
        assert_eq!(find_last_user_prompt(&msgs), Some(1));
    }

    #[test]
    fn find_skips_tool_results() {
        let msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "do thing"),
            msg(Role::Assistant, "running tool"),
            msg(Role::User, "<tool_result>\nok\n</tool_result>"),
            msg(Role::Assistant, "done"),
        ];
        assert_eq!(find_last_user_prompt(&msgs), Some(1));
    }

    #[test]
    fn find_skips_tool_denied() {
        let msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "q"),
            msg(Role::Assistant, "proposing tool"),
            msg(Role::User, "Tool call denied by user. Try again."),
            msg(Role::Assistant, "ok"),
        ];
        assert_eq!(find_last_user_prompt(&msgs), Some(1));
    }

    #[test]
    fn find_last_across_turns() {
        let msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "first"),
            msg(Role::Assistant, "a1"),
            msg(Role::User, "second"),
            msg(Role::Assistant, "a2"),
        ];
        assert_eq!(find_last_user_prompt(&msgs), Some(3));
    }

    #[test]
    fn retry_removes_last_exchange() {
        let mut msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "first"),
            msg(Role::Assistant, "a1"),
            msg(Role::User, "second"),
            msg(Role::Assistant, "a2"),
        ];
        let removed = retry_last_exchange(&mut msgs);
        assert_eq!(removed.as_deref(), Some("second"));
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0].role, Role::System));
        assert!(matches!(msgs[2].role, Role::Assistant));
    }

    #[test]
    fn retry_removes_tool_round_trip() {
        let mut msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "first"),
            msg(Role::Assistant, "a1"),
            msg(Role::User, "second"),
            msg(Role::Assistant, "running"),
            msg(Role::User, "<tool_result>\nok\n</tool_result>"),
            msg(Role::Assistant, "a2"),
        ];
        let removed = retry_last_exchange(&mut msgs);
        assert_eq!(removed.as_deref(), Some("second"));
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn retry_none_when_no_exchange() {
        let mut msgs = vec![msg(Role::System, "sys")];
        assert_eq!(retry_last_exchange(&mut msgs), None);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn is_user_prompt_classifier() {
        assert!(is_user_prompt("hello"));
        assert!(is_user_prompt("  what is rust?"));
        assert!(!is_user_prompt("<tool_result>\nok\n</tool_result>"));
        assert!(!is_user_prompt("  <tool_result>..."));
        assert!(!is_user_prompt("Tool call denied by user. Try again."));
    }
}
