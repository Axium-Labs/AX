//! Context selection is independent of durable conversation history.
//!
//! Two selections live here: [`select_context`] trims a restored session to
//! its most recent complete turns, and [`request_context`] assembles the exact
//! message list for one model request. Neither mutates the effective
//! transcript.
use model::{Message, Role};

use crate::{AgentError, ContextBudget, ContextDemand, estimate_tokens, execution, task_queue};

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
    let mut used = 0;
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
    let mut selected = Vec::new();
    let mut system_used = 0;
    for message in messages.iter().filter(|m| m.role == Role::System) {
        let cost = estimate_tokens(std::slice::from_ref(message));
        if used + cost <= total_budget && system_used + cost <= system_budget {
            selected.push(message.clone());
            used += cost;
            system_used += cost;
        }
    }
    selected.extend_from_slice(&conversation[start..]);
    selected
}

/// Which pool a message joins when a request is assembled. Classification is
/// purely by the runtime's own context marker, never by tool or model output.
enum RequestPool {
    /// Runtime bookkeeping that must never be re-sent.
    Drop,
    /// Transient progress/recovery state, always kept first.
    Progress,
    /// Retrieved memory, optional when space is tight.
    Memory,
    /// Eligible skill metadata.
    Skill,
    /// Conversation history.
    History,
}

fn request_pool(message: &Message) -> RequestPool {
    if message.role != Role::System {
        return RequestPool::History;
    }
    let content = message.content.as_str();
    if content.starts_with("[ax-changes]\n") || content.starts_with(execution::STATE_PREFIX) {
        return RequestPool::Drop;
    }
    if content.starts_with(execution::CONTEXT_PREFIX)
        || content.starts_with(task_queue::PROGRESS_PREFIX)
        || content.starts_with("[ax-task-summary]")
        || content.starts_with("[ax-recovery]")
    {
        return RequestPool::Progress;
    }
    if content.starts_with("[retrieved-memory]") {
        return RequestPool::Memory;
    }
    if content.starts_with("[ax-skill:") {
        return RequestPool::Skill;
    }
    RequestPool::History
}

/// Selects request context without changing the effective in-memory transcript.
/// Old turns are removed first; optional retrieved context follows only when
/// it fits the same budget used to decide whether to compact.
pub(crate) fn request_context(
    messages: &[Message],
    budget: ContextBudget,
) -> Result<Vec<Message>, AgentError> {
    let mut history = Vec::new();
    let mut memory = Vec::new();
    let mut skills = Vec::new();
    let mut progress = Vec::new();
    for message in messages {
        match request_pool(message) {
            RequestPool::Drop => {}
            RequestPool::Progress => progress.push(message.clone()),
            RequestPool::Memory => memory.push(message.clone()),
            RequestPool::Skill => skills.push(message.clone()),
            RequestPool::History => {
                let mut projected = message.clone();
                if message.role == Role::Tool
                    && let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content)
                {
                    projected.content = result.model_view(budget.tool_result_chars());
                }
                history.push(projected);
            }
        }
    }

    let latest_turn_len = history
        .iter()
        .rposition(|message| message.role == Role::User)
        .map_or(0, |index| {
            history[index..]
                .iter()
                .filter(|message| message.role != Role::System)
                .count()
        });
    let progress_tokens = estimate_tokens(&progress);
    let latest_start = history
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(history.len());
    let latest_tokens = estimate_tokens(&history[latest_start..]);
    let allocations = budget
        .allocate(&[
            ContextDemand {
                demand: progress_tokens,
                minimum: progress_tokens,
                maximum: budget.usable(),
            },
            ContextDemand {
                demand: estimate_tokens(&skills),
                minimum: 0,
                maximum: budget.usable(),
            },
            ContextDemand {
                demand: estimate_tokens(&memory),
                minimum: 0,
                maximum: budget.usable(),
            },
            ContextDemand {
                demand: estimate_tokens(&history),
                minimum: latest_tokens,
                maximum: budget.usable(),
            },
        ])
        .map_err(|e| AgentError::Budget(e.into()))?;
    history = select_context(&history, allocations[3], allocations[3]);
    if history
        .iter()
        .filter(|message| message.role != Role::System)
        .count()
        < latest_turn_len
    {
        return Err(AgentError::Budget(
            "latest user turn exceeds the input budget".into(),
        ));
    }
    let mut selected = progress;
    selected.extend(history);
    let mut remaining = budget.usable().saturating_sub(estimate_tokens(&selected));
    // Routing and retrieval put their highest-priority entries first. When
    // space is tight, optional memory gives way before selected skills.
    for message in skills.into_iter().chain(memory) {
        let cost = estimate_tokens(std::slice::from_ref(&message));
        if cost <= remaining {
            selected.push(message);
            remaining -= cost;
        }
    }
    let estimated = estimate_tokens(&selected);
    if estimated > budget.usable() {
        return Err(AgentError::Budget(format!(
            "request needs {estimated} tokens, allowed {}",
            budget.usable()
        )));
    }
    Ok(selected)
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
