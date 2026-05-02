//! Pure helpers for trimming a conversation transcript without invoking
//! the LLM. Used by `/retry` and `/undo` in the CLI and by the desktop's
//! chat toolbar — both surfaces want the same boundary semantics, so the
//! logic lives in core where every frontend can reach it.
//!
//! "Genuine user prompts" exclude the synthetic `<tool_result>` and
//! `Tool call denied` envelopes the agent loop appends after a tool call
//! — those are wire-format artefacts, not turns the user wants to step
//! over. The compaction-summary header injected by `/compact` is treated
//! as a floor: `/undo` will not peel it off because the messages it
//! summarized have been discarded and cannot come back.

use crate::message::{Message, Role};

/// Prefix written by the compaction routine into the synthetic user
/// message at index 1 of a compacted transcript. Exposed so frontends
/// can detect "we're past a compaction boundary" without duplicating
/// the literal.
pub const COMPACTION_SUMMARY_PREFIX: &str = "Here is a summary of our conversation so far:";

/// `true` when `content` is something the user typed (not a tool-result
/// envelope or a denial sentinel). Both retry and undo skip non-prompt
/// user messages so they can find the actual prompt boundary.
#[must_use]
pub fn is_user_prompt(content: &str) -> bool {
    let trimmed = content.trim_start();
    !trimmed.starts_with("<tool_result>") && !trimmed.starts_with("Tool call denied")
}

/// Index of the last genuine user prompt in `messages`, or `None` if the
/// conversation has not progressed past the system prompt.
#[must_use]
pub fn find_last_user_prompt(messages: &[Message]) -> Option<usize> {
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

/// `true` when `messages` starts with a compaction header (system,
/// summary-user, ack-assistant). When this holds, the smallest index
/// `/undo` is allowed to truncate at is `3`.
#[must_use]
pub fn is_post_compaction(messages: &[Message]) -> bool {
    messages.len() >= 2
        && matches!(messages[1].role, Role::User)
        && messages[1].content.starts_with(COMPACTION_SUMMARY_PREFIX)
}

/// Pop up to `requested` turns off the end of `messages`, returning the
/// number actually popped. A "turn" spans from the most recent genuine
/// user prompt through every subsequent assistant / tool-result message.
///
/// Returns `0` when there is nothing to undo (only the system prompt, or
/// only a compaction header and nothing after it). Returns a value less
/// than `requested` when the compaction boundary is hit mid-way — the
/// caller can surface that as a partial success.
pub fn undo_turns(messages: &mut Vec<Message>, requested: usize) -> usize {
    let min_idx = if is_post_compaction(messages) { 3 } else { 1 };
    let mut popped = 0;
    while popped < requested {
        let Some(idx) = find_last_user_prompt(messages) else {
            break;
        };
        if idx < min_idx {
            break;
        }
        messages.truncate(idx);
        popped += 1;
    }
    popped
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
    }

    #[test]
    fn undo_single_turn() {
        let mut msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "first"),
            msg(Role::Assistant, "a1"),
            msg(Role::User, "second"),
            msg(Role::Assistant, "a2"),
        ];
        assert_eq!(undo_turns(&mut msgs, 1), 1);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs.last().unwrap().content, "a1");
    }

    #[test]
    fn undo_pops_tool_round_trip_as_one_turn() {
        let mut msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "first"),
            msg(Role::Assistant, "a1"),
            msg(Role::User, "second"),
            msg(Role::Assistant, "calling tool"),
            msg(Role::User, "<tool_result>\nok\n</tool_result>"),
            msg(Role::Assistant, "a2"),
        ];
        assert_eq!(undo_turns(&mut msgs, 1), 1);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn undo_respects_compaction_boundary() {
        let mut msgs = vec![
            msg(Role::System, "sys"),
            msg(
                Role::User,
                &format!("{COMPACTION_SUMMARY_PREFIX}\n\nprior work..."),
            ),
            msg(Role::Assistant, "Understood. I have the context..."),
            msg(Role::User, "new question"),
            msg(Role::Assistant, "answer"),
        ];
        assert_eq!(undo_turns(&mut msgs, 5), 1);
        assert_eq!(msgs.len(), 3);
    }
}
