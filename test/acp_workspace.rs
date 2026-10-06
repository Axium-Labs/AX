use super::*;

#[test]
fn workspace_lists_only_directories_and_rejects_files_or_missing_paths() {
    let root = std::env::temp_dir().join(format!("ax-workspace-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("folder with spaces")).unwrap();
    std::fs::write(root.join("file.txt"), "content").unwrap();
    let value = listing(&json!({"cwd":root}), None).unwrap();
    assert_eq!(value["directories"].as_array().unwrap().len(), 1);
    assert_eq!(value["directories"][0]["name"], "folder with spaces");
    assert_eq!(
        std::path::PathBuf::from(value["cwd"].as_str().unwrap()),
        root.canonicalize().unwrap()
    );
    assert!(listing(&json!({"cwd":root.join("file.txt")}), None).is_err());
    assert!(listing(&json!({"cwd":root.join("missing")}), None).is_err());
    assert!(listing(&json!({"cwd":root.parent()}), Some(&root)).is_err());
    assert!(listing(&json!({}), Some(&root)).unwrap()["parent"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}
