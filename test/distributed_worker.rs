use super::*;

fn config(root: &Path, source: &Path) -> WorkerSettings {
    serde_json::from_value(
        json!({"gateway":"http://127.0.0.1:8765","token":"test","instance_id":"test-instance",
        "projects":{"project-ax":source},"execution_root":root.join("executions"),"sandbox":"off"}),
    )
    .unwrap()
}
fn task(generation: u64) -> Value {
    json!({"id":"01234567-89ab-cdef-0123-456789abcdef","generation":generation,"spec":{"project_id":"project-ax"}})
}

#[tokio::test]
async fn isolated_git_attempts_do_not_modify_the_source_or_each_other() {
    let root = std::env::temp_dir().join(format!("ax-worker-test-{}", uuid::Uuid::new_v4()));
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    git(&source, &["init"]).await.unwrap();
    fs::write(source.join("code.txt"), "original\n").unwrap();
    git(&source, &["add", "."]).await.unwrap();
    git(
        &source,
        &[
            "-c",
            "user.name=AX test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    )
    .await
    .unwrap();
    fs::write(source.join("code.txt"), "uncommitted local change\n").unwrap();
    let mut config = config(&root, &source);
    fs::create_dir_all(&config.execution_root).unwrap();
    config.execution_root = config.execution_root.canonicalize().unwrap();
    config
        .projects
        .insert("project-ax".into(), source.canonicalize().unwrap());
    let (first, _) = prepare(&config, &task(1)).await.unwrap();
    let (second, _) = prepare(&config, &task(2)).await.unwrap();
    assert_eq!(
        fs::read_to_string(first.join("code.txt")).unwrap().trim(),
        "original"
    );
    fs::write(first.join("code.txt"), "remote change\n").unwrap();
    assert_eq!(
        fs::read_to_string(second.join("code.txt")).unwrap().trim(),
        "original"
    );
    assert_eq!(
        fs::read_to_string(source.join("code.txt")).unwrap(),
        "uncommitted local change\n"
    );
    assert!(prepare(&config, &task(1)).await.is_err());
    fs::remove_dir_all(&root).unwrap();
}
#[tokio::test]
async fn non_git_snapshot_isolated_and_requires_logical_mapping() {
    let root = std::env::temp_dir().join(format!("ax-snapshot-test-{}", uuid::Uuid::new_v4()));
    let source = root.join("source");
    fs::create_dir_all(source.join(".ax")).unwrap();
    fs::write(source.join("file.txt"), "original").unwrap();
    fs::write(source.join(".ax").join("private-memory"), "private").unwrap();
    let config = config(&root, &source);
    let (snapshot, is_git) = prepare(&config, &task(1)).await.unwrap();
    assert!(!is_git);
    assert!(!snapshot.join(".ax").exists());
    fs::write(snapshot.join("file.txt"), "changed").unwrap();
    assert_eq!(
        fs::read_to_string(source.join("file.txt")).unwrap(),
        "original"
    );
    let mut invalid = task(2);
    invalid["spec"]["project_id"] = json!("/srv/absolute-path");
    assert!(prepare(&config, &invalid).await.is_err());
    let mut invalid = task(2);
    invalid["spec"]["workspace_revision"] = json!("abc1234");
    assert!(prepare(&config, &invalid).await.is_err());
    fs::remove_dir_all(&root).unwrap();
}
#[test]
fn remote_transport_rejects_plaintext_and_credential_urls() {
    assert!(Client::new("http://remote.example.com", "token".into()).is_err());
    assert!(Client::new("https://user:password@example.com", "token".into()).is_err());
    assert!(Client::new("http://127.0.0.1:8765", "token".into()).is_ok());
}
#[test]
fn large_failure_results_use_artifact_references_instead_of_unreportable_messages() {
    let long = "错误".repeat(40000);
    let text = super::summary(&long, true, &serde_json::json!("report-id"));
    assert!(text.len() < 256);
    assert!(text.contains("failed") && text.contains("report-id"));
    assert_eq!(
        super::summary("test failed", true, &serde_json::json!("report-id")),
        "test failed"
    );
}
