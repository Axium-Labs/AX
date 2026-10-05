use super::*;

#[test]
fn legacy_crew_paths_only_resolve_registered_project_roots() {
    let root = std::env::temp_dir().join(format!("ax-crew-path-{}", uuid::Uuid::new_v4()));
    let project = root.join("project");
    fs::create_dir_all(project.join("nested")).unwrap();
    fs::create_dir_all(root.join("unregistered")).unwrap();
    let projects = vec![crate::session_projects::ProjectLocation {
        id: "project-ax".into(),
        root: project.clone(),
        data_dir: project.join(".ax"),
        skills_dir: project.join("skills"),
        mcp_config: project.join("mcp.json"),
    }];
    assert_eq!(
        resolve_workspace(&json!({"workspace_id":"project-ax"}), &projects).unwrap(),
        project.canonicalize().unwrap()
    );
    assert_eq!(
        resolve_workspace(&json!({"cwd":project}), &projects).unwrap(),
        project.canonicalize().unwrap()
    );
    assert!(resolve_workspace(&json!({"cwd":project.join("nested")}), &projects).is_err());
    assert!(resolve_workspace(&json!({"cwd":root.join("unregistered")}), &projects).is_err());
    assert!(resolve_workspace(&json!({"workspace_id":"unknown"}), &projects).is_err());
    assert!(
        resolve_workspace(
            &json!({"workspace_id":"project-ax","cwd":project}),
            &projects
        )
        .is_err()
    );
    fs::remove_dir_all(root).unwrap();
}
