//! Context selection is independent of durable conversation history.
use crate::estimate_tokens;
use model::{Message, Role};

/// Keep a suffix of complete user turns, plus restored system context (the
/// persisted session summary and agent state), within budget. Never retain a
/// tool result without its assistant call by splitting a turn.
///
/// `system_budget` and `total_budget` are named, explicit inputs (see
/// [`crate::ContextBudget::session_summary_budget`] and
/// [`crate::ContextBudget::recent_messages_budget`]) rather than a fixed
/// fraction chosen inside this function.
#[must_use]
pub fn select_context(
    messages: &[Message],
    total_budget: usize,
    system_budget: usize,
) -> Vec<Message> {
    let mut system = Vec::new();
    let mut used = 0;
    for message in messages.iter().filter(|m| m.role == Role::System) {
        let cost = estimate_tokens(std::slice::from_ref(message));
        if used + cost <= system_budget {
            system.push(message.clone());
            used += cost;
        }
    }
    let conversation = messages
        .iter()
        .filter(|m| m.role != Role::System)
        .cloned()
        .collect::<Vec<_>>();
    let starts = conversation
        .iter()
        .enumerate()
        .filter_map(|(i, m)| (m.role == Role::User).then_some(i))
        .collect::<Vec<_>>();
    let mut start = conversation.len();
    for &index in starts.iter().rev() {
        let cost = estimate_tokens(&conversation[index..start]);
        if used + cost > total_budget {
            break;
        }
        used += cost;
        start = index;
    }
    system.extend_from_slice(&conversation[start..]);
    system
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_more_than_two_hundred_small_messages_within_budget() {
        let messages = (0..300).map(|_| Message::user("hi")).collect::<Vec<_>>();
        assert_eq!(select_context(&messages, 10_000, 5_000).len(), 300);
        assert!(estimate_tokens(&select_context(&messages, 50, 25)) <= 50);
    }
    #[test]
    fn restore_keeps_whole_turns_and_drops_oversize_older_turns() {
        let messages = vec![
            Message::user("x".repeat(1000)),
            Message::assistant("old", vec![]),
            Message::user("new"),
            Message::assistant("answer", vec![]),
        ];
        let selected = select_context(&messages, 30, 15);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].content, "new");
    }

    #[test]
    fn system_and_conversation_budgets_combine_within_the_total() {
        let messages = vec![
            Message::system("[memory-summary]\nshort summary"),
            Message::user("question"),
            Message::assistant("answer", vec![]),
        ];
        let selected = select_context(&messages, 100, 20);
        assert!(selected.iter().any(|m| m.content.contains("short summary")));
        assert!(selected.iter().any(|m| m.content == "question"));
    }

    #[test]
    fn an_oversized_session_summary_is_dropped_without_blocking_recent_messages() {
        let messages = vec![
            Message::system("[memory-summary]\n".to_owned() + &"x".repeat(1000)),
            Message::user("recent question"),
            Message::assistant("recent answer", vec![]),
        ];
        // The oversized summary exceeds system_budget and must be dropped
        // instead of consuming total_budget and starving the conversation.
        let selected = select_context(&messages, 40, 5);
        assert!(
            !selected
                .iter()
                .any(|m| m.content.contains("memory-summary"))
        );
        assert!(selected.iter().any(|m| m.content == "recent question"));
    }
}
