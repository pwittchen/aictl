//! `/undo` — drop the last N user/assistant exchanges from the conversation
//! without resubmitting anything, so the next turn runs as if those exchanges
//! never happened.
//!
//! Differs from [`super::retry`] in two ways: (1) nothing is re-sent — the
//! caller just runs the next turn with fresh input, and (2) it supports
//! popping multiple turns at once (`/undo 3`).
//!
//! Refuses to cross a `/compact` boundary. After compaction, messages 0–2
//! are the system prompt, the synthetic summary user message, and the
//! assistant acknowledgement; anything earlier has been summarized away and
//! cannot be restored, so we treat that block as a floor.

use super::retry::find_last_user_prompt;
use crate::{Message, Role};

/// Prefix written by [`super::compact`] into the synthetic user message it
/// drops in at index 1. Used as the compaction-boundary sentinel so `/undo`
/// refuses to peel it off.
const COMPACTION_SUMMARY_PREFIX: &str = "Here is a summary of our conversation so far:";

/// `true` when `messages` starts with a compaction header (system,
/// summary-user, ack-assistant). When this holds, the smallest index `/undo`
/// is allowed to truncate at is `3`.
fn is_post_compaction(messages: &[Message]) -> bool {
    messages.len() >= 2
        && matches!(messages[1].role, Role::User)
        && messages[1].content.starts_with(COMPACTION_SUMMARY_PREFIX)
}

/// Pop up to `requested` turns off the end of `messages`, returning the
/// number actually popped. A "turn" spans from the most recent genuine user
/// prompt through every subsequent assistant / tool-result message.
///
/// Returns `0` when there is nothing to undo (only the system prompt, or
/// only a compaction header and nothing after it). Returns a value less than
/// `requested` when the compaction boundary is hit mid-way — the caller can
/// surface that as a partial success.
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

    fn base() -> Vec<Message> {
        vec![
            msg(Role::System, "sys"),
            msg(Role::User, "first"),
            msg(Role::Assistant, "a1"),
            msg(Role::User, "second"),
            msg(Role::Assistant, "a2"),
            msg(Role::User, "third"),
            msg(Role::Assistant, "a3"),
        ]
    }

    #[test]
    fn undo_single_turn() {
        let mut msgs = base();
        assert_eq!(undo_turns(&mut msgs, 1), 1);
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs.last().unwrap().content, "a2");
    }

    #[test]
    fn undo_multiple_turns() {
        let mut msgs = base();
        assert_eq!(undo_turns(&mut msgs, 2), 2);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs.last().unwrap().content, "a1");
    }

    #[test]
    fn undo_all_turns() {
        let mut msgs = base();
        assert_eq!(undo_turns(&mut msgs, 3), 3);
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0].role, Role::System));
    }

    #[test]
    fn undo_more_than_available_stops_at_system() {
        let mut msgs = base();
        assert_eq!(undo_turns(&mut msgs, 99), 3);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn undo_none_when_only_system() {
        let mut msgs = vec![msg(Role::System, "sys")];
        assert_eq!(undo_turns(&mut msgs, 1), 0);
        assert_eq!(msgs.len(), 1);
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
        assert_eq!(msgs.last().unwrap().content, "a1");
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
        assert!(msgs[1].content.starts_with(COMPACTION_SUMMARY_PREFIX));
    }

    #[test]
    fn undo_refuses_when_only_compaction_header() {
        let mut msgs = vec![
            msg(Role::System, "sys"),
            msg(
                Role::User,
                &format!("{COMPACTION_SUMMARY_PREFIX}\n\nprior work..."),
            ),
            msg(Role::Assistant, "Understood. I have the context..."),
        ];
        assert_eq!(undo_turns(&mut msgs, 1), 0);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn is_post_compaction_detects_header() {
        let msgs = vec![
            msg(Role::System, "sys"),
            msg(
                Role::User,
                &format!("{COMPACTION_SUMMARY_PREFIX}\n\nprior..."),
            ),
            msg(Role::Assistant, "Understood..."),
        ];
        assert!(is_post_compaction(&msgs));
    }

    #[test]
    fn is_post_compaction_false_for_regular_conversation() {
        let msgs = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi"),
        ];
        assert!(!is_post_compaction(&msgs));
    }
}
