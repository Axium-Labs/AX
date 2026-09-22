//! Context selection is independent of durable conversation history.
use crate::estimate_tokens;
use model::{Message, Role};

/// Keep a suffix of complete user turns, plus system context within budget.
/// Never retain a tool result without its assistant call by splitting a turn.
#[must_use]
pub fn select_context(messages: &[Message], budget: usize) -> Vec<Message> {
    let mut system = Vec::new();
    let mut used = 0;
    for message in messages.iter().filter(|m| m.role == Role::System) {
        let cost = estimate_tokens(std::slice::from_ref(message));
        if used + cost <= budget / 2 {
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
        if used + cost > budget {
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
        assert_eq!(select_context(&messages, 10_000).len(), 300);
        assert!(estimate_tokens(&select_context(&messages, 50)) <= 50);
    }
    #[test]
    fn restore_keeps_whole_turns_and_drops_oversize_older_turns() {
        let messages = vec![
            Message::user("x".repeat(1000)),
            Message::assistant("old", vec![]),
            Message::user("new"),
            Message::assistant("answer", vec![]),
        ];
        let selected = select_context(&messages, 30);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].content, "new");
    }
}
