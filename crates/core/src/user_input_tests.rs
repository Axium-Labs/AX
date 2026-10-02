//! `request_user_input` contract: ask, suspend, answer, resume — never fail.

use super::*;

fn question() -> UserQuestion {
    UserQuestion {
        id: "q-call-1".into(),
        question: "Which export format should the report use?".into(),
        options: vec![
            UserOption {
                id: "pdf".into(),
                label: "PDF".into(),
                description: "printable".into(),
            },
            UserOption {
                id: "md".into(),
                label: "Markdown".into(),
                description: String::new(),
            },
        ],
        allow_free_text: false,
        tool_call_id: "call-1".into(),
    }
}

fn option_ids() -> Vec<String> {
    question().options.iter().map(|o| o.id.clone()).collect()
}

#[test]
fn schema_requires_a_question_and_carries_options() {
    let spec = spec();
    assert_eq!(spec.function.name, TOOL_NAME);
    assert_eq!(
        spec.function.parameters["required"],
        serde_json::json!(["question"])
    );
    assert!(spec.function.description.contains("permission"));
    assert!(spec.function.description.contains("suspends"));
    let parsed = parse_question(
        &serde_json::json!({"question": "Pick one", "options": [{"id": "a", "label": "A"}]}),
        "call-9",
    )
    .unwrap();
    assert_eq!(parsed.id, "q-call-9");
    assert_eq!(parsed.tool_call_id, "call-9");
    assert!(!parsed.allow_free_text);
    assert!(parse_question(&serde_json::json!({}), "call").is_err());
    assert!(
        parse_question(
            &serde_json::json!({"question": "Pick", "options": [{"id": "a"}, {"id": "a"}]}),
            "call"
        )
        .is_err()
    );
    let free = parse_question(&serde_json::json!({"question": "Name it"}), "call").unwrap();
    assert!(free.allow_free_text);
}

#[test]
fn answers_resolve_ids_numbers_labels_and_free_text() {
    let base = self::question();
    assert_eq!(
        UserAnswer::parse(&base, "pdf").unwrap().option_id,
        Some("pdf".into())
    );
    assert_eq!(
        UserAnswer::parse(&base, "2").unwrap().option_id,
        Some("md".into())
    );
    assert_eq!(
        UserAnswer::parse(&base, "Markdown").unwrap().option_id,
        Some("md".into())
    );
    // Free text is refused unless the question allows it.
    assert!(UserAnswer::parse(&base, "something else").is_none());
    assert!(UserAnswer::parse(&base, "   ").is_none());

    let mut open = base.clone();
    open.allow_free_text = true;
    let answer = UserAnswer::parse(&open, "csv, tab separated").unwrap();
    assert_eq!(answer.text.as_deref(), Some("csv, tab separated"));
    assert_eq!(answer.summary(), "csv, tab separated");
    assert_eq!(UserAnswer::option(&base, "md").summary(), "md");
}

#[test]
fn the_question_marker_is_durable_and_removed_from_context() {
    let question = self::question();
    let mut messages = vec![Message::user("hi"), snapshot(&question)];
    let restored = restore(&mut messages).expect("question restored");
    assert_eq!(restored, question);
    assert_eq!(messages.len(), 1);
    assert!(restore(&mut messages).is_none());
}

#[test]
fn answer_payload_is_machine_readable_and_names_the_tool_call() {
    let question = self::question();
    let answer = UserAnswer::option(&question, "md");
    let payload = UserQuestion::answer_payload(&question, &answer);
    assert_eq!(payload["status"], "answered");
    assert_eq!(payload["question_id"], "q-call-1");
    assert_eq!(payload["answer"]["option_id"], "md");
    assert_eq!(payload["answer"]["text"], serde_json::Value::Null);
}

#[test]
fn rendered_question_lists_options_for_a_text_frontend() {
    let rendered = question().to_string();
    assert!(rendered.starts_with("Which export format"));
    assert!(rendered.contains("1. PDF (pdf) — printable"));
    assert!(rendered.contains("2. Markdown (md)"));
    assert!(!rendered.contains("free text allowed"));
    assert_eq!(option_ids(), ["pdf", "md"]);
}
