use super::*;

async fn assert_unterminated_eof_insertion(newline: &str) {
    let path = std::env::temp_dir().join(format!("ax-r4-regression-{}", uuid::Uuid::new_v4()));
    let _cleanup = Temporary(path.clone());
    tokio::fs::write(&path, format!("a{newline}b"))
        .await
        .unwrap();
    let receipt = PatchTool
        .execute(json!({"path":path,"edits":[{
            "start_line":3,"delete_count":0,"new_text":"c"
        }]}))
        .await
        .unwrap();
    let expected = format!("a{newline}b{newline}c");
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), expected);
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["regions"].as_array().unwrap().len(), 1);
    let region = &receipt["regions"][0];
    assert_eq!(region["start_line"], 1);
    assert_eq!(region["total_lines"], 3);
    assert_eq!(region["output"], format!("1: a{newline}2: b{newline}3: c"));
    assert_eq!(receipt["changed_files"][0]["additions"], 1);
}

#[tokio::test]
async fn eof_insertion_after_unterminated_lf_preserves_regions() {
    assert_unterminated_eof_insertion("\n").await;
}

#[tokio::test]
async fn eof_insertion_after_unterminated_crlf_preserves_regions() {
    assert_unterminated_eof_insertion("\r\n").await;
}
