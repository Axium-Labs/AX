//! Bounded history loading and recovery of interrupted tool interactions.
use anyhow::Result;
use memory::{MemoryStore, StoredMessage};
use model::Message;

pub(crate) const HISTORY_PAGE_SIZE: u32 = 128;

pub(crate) fn recent_history(
    store: &MemoryStore,
    session: &str,
    budget: usize,
) -> Result<Vec<StoredMessage>> {
    let mut pages = Vec::new();
    let mut before = None;
    let mut tokens = 0;
    let mut has_user = false;
    loop {
        let page = store.load_context_page(session, before, HISTORY_PAGE_SIZE)?;
        if page.is_empty() {
            break;
        }
        before = page.first().map(|message| message.id);
        tokens += page
            .iter()
            .map(|message| runtime_core::estimate_tokens(&[crate::restore_message(message)]))
            .sum::<usize>();
        has_user |= page
            .iter()
            .any(|message| message.role == memory::MessageRole::User);
        pages.push(page);
        if tokens >= budget && has_user {
            break;
        }
    }
    Ok(pages.into_iter().rev().flatten().collect())
}

pub(crate) fn interrupted_results(messages: &[Message]) -> Vec<Message> {
    let start = messages
        .iter()
        .rposition(|message| message.role == model::Role::User)
        .unwrap_or(0);
    let tail = &messages[start..];
    let answered = tail
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    tail.iter().flat_map(|message| &message.tool_calls)
        .filter(|call| !answered.contains(call.id.as_str()))
        .map(|call| Message::tool(call.id.clone(), "AX stopped before this tool result was saved. Its external effects are unknown; inspect the workspace before retrying."))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReplState, memory_role};

    #[test]
    fn restart_repairs_interrupted_calls_once_without_replaying_tools() {
        let root = std::env::temp_dir().join(format!("ax-resume-{}", uuid::Uuid::new_v4()));
        let mut state =
            ReplState::new_in_project(root.clone(), root.join("skills"), None, &root).unwrap();
        state.create_session("test").unwrap();
        let session = state.current_session_id().unwrap().to_owned();
        state
            .persist_messages(&[
                Message::user("run task"),
                Message::assistant(
                    "",
                    vec![model::ToolCall {
                        id: "pending".into(),
                        kind: "function".into(),
                        function: model::FunctionCall {
                            name: "shell".into(),
                            arguments: "{}".into(),
                        },
                    }],
                ),
            ])
            .unwrap();
        drop(state);
        let mut state =
            ReplState::new_in_project(root.clone(), root.join("skills"), None, &root).unwrap();
        let budget = runtime_core::ContextBudget::new(32000, Some(1000), 0);
        state.open_session(&session, &budget).unwrap();
        assert_eq!(
            state
                .loaded_messages
                .last()
                .unwrap()
                .tool_call_id
                .as_deref(),
            Some("pending")
        );
        assert!(
            state
                .loaded_messages
                .last()
                .unwrap()
                .content
                .contains("effects are unknown")
        );
        state.open_session(&session, &budget).unwrap();
        assert_eq!(
            state
                .store()
                .unwrap()
                .load_messages(&session, None, 100)
                .unwrap()
                .len(),
            3
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_is_paged_and_watermarked_before_decoding() {
        let mut store = MemoryStore::open_in_memory().unwrap();
        let session = store.create_session("large").unwrap();
        for i in 0..400 {
            let message = Message::user(format!("turn {i}: {}", "details ".repeat(30)));
            store
                .append_message(
                    &session.id,
                    memory::NewMessage {
                        role: memory_role(&message.role),
                        kind: memory::MessageKind::Message,
                        content: message.content.clone(),
                        metadata: serde_json::to_value(message).unwrap(),
                    },
                )
                .unwrap();
        }
        let recent = recent_history(&store, &session.id, 500).unwrap();
        assert_eq!(recent.len(), 128);
        assert!(recent.last().unwrap().content.starts_with("turn 399:"));
        store.save_effective_context(&session.id, "", "[]").unwrap();
        assert!(
            store
                .load_context_page(&session.id, None, 128)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.load_messages(&session.id, None, 1000).unwrap().len(),
            400
        );
    }
}
