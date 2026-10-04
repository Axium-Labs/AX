use super::*;

#[test]
fn signed_thinking_is_budgeted_when_selecting_restored_turns() {
    let mut assistant = Message::assistant("ok", Vec::new());
    assistant.provider_metadata = Some(
        serde_json::json!({"provider":"anthropic", "model":"claude-test", "content":[{"type":"thinking","thinking":"x".repeat(4_000),"signature":"signed"},{"type":"text","text":"ok"}]}),
    );
    let messages = vec![Message::user("question"), assistant];
    assert!(estimate_tokens(&messages) > 1_000);
    assert!(crate::select_context(&messages, 500, 0).is_empty());
}

#[test]
fn legacy_session_messages_default_to_no_native_metadata() {
    let message: Message = serde_json::from_value(
        serde_json::json!({"role":"assistant","content":"old reply","tool_calls":[]}),
    )
    .unwrap();
    assert!(message.provider_metadata.is_none());
    assert_eq!(estimate_tokens(&[message]), 7);
}
