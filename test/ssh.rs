use serde_json::json;
use tool::{SafetyLevel, SshContext, SshHost, SshTool, Tool};
fn host(id: &str) -> SshHost {
    SshHost {
        id: id.into(),
        name: id.into(),
        host: format!("user@{id}"),
        port: None,
        identity_file: None,
    }
}
fn tool() -> SshTool {
    SshTool::new(SshContext {
        hosts: vec![host("one"), host("two")],
        default_host: "one".into(),
        cwd: "/srv/project".into(),
    })
    .unwrap()
}
#[tokio::test]
async fn local_ssh_catalogue_has_no_host_limit_and_hides_key_paths() {
    let hosts = (0..1024).map(|i| host(&format!("host{i}"))).collect();
    let tool = SshTool::new(SshContext {
        hosts,
        default_host: "host0".into(),
        cwd: "/srv".into(),
    })
    .unwrap();
    let raw = tool.execute(json!({"action":"list"})).await.unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["hosts"].as_array().unwrap().len(), 1024);
    assert!(!raw.contains("identity_file"));
    assert_eq!(
        tool.safety(&json!({"action":"exec"})),
        SafetyLevel::RequiresApproval
    );
}
#[tokio::test]
async fn ssh_unknown_hosts_and_invalid_inputs_cannot_spawn_commands() {
    let tool = tool();
    assert!(
        tool.execute(json!({"action":"exec","host_id":"unknown","command":"echo bad"}))
            .await
            .is_err()
    );
    assert!(
        tool.execute(json!({"action":"exec","command":"bad\u{0}"}))
            .await
            .is_err()
    );
    let mut invalid = host("one");
    invalid.host = "-oProxyCommand=evil".into();
    assert!(invalid.command().is_err());
    assert_eq!(
        tool::ssh::quote("/srv/it's $(touch bad)"),
        "'/srv/it'\"'\"'s $(touch bad)'"
    );
}
#[test]
fn different_hosts_have_independent_resources_and_remote_command_is_never_ax() {
    let tool = tool();
    let a = tool.resources(&json!({"action":"exec","host_id":"one"}));
    let b = tool.resources(&json!({"action":"exec","host_id":"two"}));
    assert!(!a[0].resource.overlaps(&b[0].resource));
    let cmd = host("one").command().unwrap();
    let args = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(args.last().unwrap(), "sh -s");
    assert!(!args.iter().any(|a| a.contains("ax")));
}
