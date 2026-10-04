use super::*;

#[test]
fn mapping_creates_twenty_three_isolated_tasks_with_sequential_failure_continuation() {
    let data = json!({"records":(0..23).map(|index|json!({"id":format!("record-{index}"),"repository":"owner/project","commit":"012345","problem":format!("Repair bug {index}")})).collect::<Vec<_>>()});
    let mapped=map_records(&data,Some(&json!({"title_column":"id","instruction":"Repair the supplied problem, validate, save artifacts; recover setup failures and report evidence.","repo_url":"https://example.invalid/{repository}.git","revision_column":"commit","output_root":"C:/output","sequential":true})),std::path::Path::new("data.parquet")).unwrap();
    let mapped: Value = serde_json::from_str(&mapped).unwrap();
    let tasks = mapped["ax_work_items"].as_array().unwrap();
    assert_eq!(tasks.len(), 23);
    assert_eq!(
        tasks[0]["workspace"]["repo_url"],
        "https://example.invalid/owner/project.git"
    );
    assert_eq!(tasks[0]["workspace"]["revision"], "012345");
    assert!(
        tasks[22]["input"]
            .as_str()
            .unwrap()
            .contains("Repair bug 22")
    );
    assert_eq!(tasks[0]["resources"], tasks[22]["resources"]);
    assert!(
        tasks.iter().all(|task| task.get("depends_on").is_none()),
        "sequential order must not skip successors after failure"
    );
}

#[tokio::test]
async fn projected_json_reader_never_returns_unselected_answer_fields() {
    let root = std::env::temp_dir().join(format!("ax-source-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("tasks.json"),
        r#"[{"id":"one","problem":"fix a bug","answer":"SECRET_REFERENCE"}]"#,
    )
    .unwrap();
    let source = TaskSourceTool::new(root.clone());
    if !EnvironmentContext::detect(&root, &root)
        .executables
        .contains_key("python")
    {
        std::fs::remove_dir_all(root).unwrap();
        return;
    }
    let result = source
        .execute(json!({"path":"tasks.json","columns":["id","problem"]}))
        .await
        .unwrap();
    assert!(result.contains("fix a bug"));
    assert!(!result.contains("answer"));
    assert!(!result.contains("SECRET_REFERENCE"));
    let mapped = source.execute(json!({"path":"tasks.json","columns":["id","problem"],"work":{"title_column":"id","instruction":"Repair bug","output_root":"output","sequential":false}})).await.unwrap();
    let mapped: Value = serde_json::from_str(&mapped).unwrap();
    assert_eq!(
        PathBuf::from(mapped["ax_work_items"][0]["output_dir"].as_str().unwrap()),
        root.join("output").join("one")
    );
    let encoded = source.execute(json!({"path":"tasks.json","columns":["id","problem"],"work":json!({"title_column":"id","instruction":"Repair bug","output_root":"output","sequential":false}).to_string()})).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), mapped);
    assert!(
        source
            .execute(json!({"path":"tasks.json","columns":["id"],"work":"[]"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("work must be an object")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn git_mapping_cannot_silently_omit_revision_or_execution_order() {
    let data = json!({"records":[{"id":"item","commit":"abcdef"}]});
    assert!(map_records(&data,Some(&json!({"title_column":"id","instruction":"fix","repo_url":"repo","sequential":true})),std::path::Path::new("input.json")).unwrap_err().to_string().contains("revision"));
    assert!(map_records(&data,Some(&json!({"title_column":"id","instruction":"fix","repo_url":"repo","revision_column":"commit"})),std::path::Path::new("input.json")).unwrap_err().to_string().contains("sequential"));
}
