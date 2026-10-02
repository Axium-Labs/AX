#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--check-nested-mount") => {
            let policy = sandbox::SandboxPolicy::workspace(
                args.next().unwrap().into(),
                sandbox::SandboxMode::Strict,
            );
            let error = sandbox::SandboxManager::prepare(&policy)
                .err()
                .expect("nested mount accepted");
            assert!(error.to_string().contains("mount"), "{error}");
            return;
        }
        Some("--ax-sandbox-broker") => {
            sandbox::run_broker(args.next().unwrap().into())
                .await
                .unwrap();
            return;
        }
        Some("--ax-sandbox-proxy") => {
            let socket = args.next().unwrap();
            let spec = args.next().unwrap();
            std::process::exit(sandbox::run_proxy(socket.into(), &spec).await.unwrap());
        }
        _ => {}
    }
    run().await;
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    clippy::items_after_statements,
    reason = "One end-to-end confinement scenario with a local command helper"
)]
async fn run() {
    use sandbox::{CommandSpec, SandboxManager, SandboxMode, SandboxPolicy};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(temp.path().join("outside-secret"), "host secret").unwrap();
    std::os::unix::fs::symlink(temp.path().join("outside-secret"), root.join("escape")).unwrap();
    std::fs::create_dir(root.join(".ssh")).unwrap();
    std::fs::write(root.join(".ssh/id_ed25519"), "private credential").unwrap();
    std::fs::create_dir(root.join("other-workspace")).unwrap();
    std::fs::write(root.join("other-workspace/secret"), "other workspace").unwrap();
    SandboxManager::configure_workspaces(vec![
        root.join("other-workspace").canonicalize().unwrap(),
    ])
    .unwrap();
    let mut policy = SandboxManager::policy_for_workspace(root.clone()).unwrap();
    policy.mode = SandboxMode::Strict;
    policy.protected_paths.push(root.join(".ssh"));
    let manager = SandboxManager::prepare(&policy).unwrap();
    async fn shell(manager: &SandboxManager, text: &str) -> std::process::Output {
        let mut command = CommandSpec::new("/bin/sh");
        command.args = vec!["-c".into(), text.into()];
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            manager.spawn(command).unwrap().wait_with_output(),
        )
        .await
        .unwrap()
        .unwrap()
    }
    let result = shell(&manager, "set -e; mkdir src; printf 'hello' > src/file; cat src/file; mv src/file src/renamed; rm src/renamed; git init -q; git -c user.name=AX -c user.email=ax@example.invalid commit --allow-empty -qm init; python3 -c 'from pathlib import Path; Path(\"python-output\").write_text(\"ok\")'").await;
    // Test POSIX development commands without relying on non-POSIX heredoc syntax.
    assert!(String::from_utf8_lossy(&result.stdout).contains("hello"));
    assert!(
        root.join("python-output").is_file(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let compile = shell(
        &manager,
        "printf 'int main(void){return 0;}' > main.c; cc main.c -o app; ./app",
    )
    .await;
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let cargo = shell(&manager, "set -e; cargo init --name sandbox_check --bin rust-project; cd rust-project; cargo build --offline; cargo test --offline").await;
    assert!(
        cargo.status.success(),
        "{}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    std::fs::write(root.join("package.json"), r#"{"name":"sandbox-check","version":"1.0.0","scripts":{"build":"node -e \"require('fs').writeFileSync('node-output','ok')\"","test":"node -e \"if(require('fs').readFileSync('node-output','utf8')!=='ok')process.exit(1)\""}}"#).unwrap();
    let npm = shell(
        &manager,
        "set -e; npm run build --silent; npm test --silent",
    )
    .await;
    assert!(
        npm.status.success(),
        "{}",
        String::from_utf8_lossy(&npm.stderr)
    );
    for command in [
        "cat ../outside-secret",
        "printf escape > ../outside-write",
        "cat escape",
        "cat ~/.ssh/id_ed25519",
        "cat .ssh/id_ed25519",
        "cat other-workspace/secret",
        "printf escape > /etc/ax-sandbox-escape",
        "cat /root/.ssh/id_ed25519",
        "cat /proc/1/root/root/.ssh/id_ed25519",
        "unshare -Ur sh -c 'echo escape'",
    ] {
        let result = shell(&manager, command).await;
        assert!(
            !result.status.success(),
            "escape unexpectedly succeeded: {command}"
        );
    }
    let first = shell(
        &manager,
        "printf reused > $TMPDIR/ax-task-state; readlink /proc/self/ns/mnt",
    )
    .await;
    let second = shell(
        &manager,
        "test $(cat $TMPDIR/ax-task-state) = reused; readlink /proc/self/ns/mnt",
    )
    .await;
    assert!(second.status.success());
    assert_eq!(
        first.stdout, second.stdout,
        "namespace recreated between calls"
    );
    let mut bad = policy;
    bad.resource_limits.memory_bytes = 1;
    assert!(
        SandboxManager::prepare(&bad).is_err(),
        "strict initialization must fail closed"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("outside-secret")).unwrap(),
        "host secret"
    );
    let detached = shell(
        &manager,
        "(sleep 2; printf late > after-drop) >/dev/null 2>&1 &",
    )
    .await;
    assert!(detached.status.success());
    drop(manager);
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(
        !root.join("after-drop").exists(),
        "descendant survived sandbox teardown"
    );
    std::fs::create_dir(root.join("mounted")).unwrap();
    let nested = std::process::Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "--bind"])
        .arg(temp.path())
        .arg(root.join("mounted"))
        .args(["--proc", "/proc", "--dev", "/dev", "--"])
        .arg(std::env::current_exe().unwrap())
        .arg("--check-nested-mount")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        nested.status.success(),
        "{}",
        String::from_utf8_lossy(&nested.stderr)
    );
    std::fs::hard_link(
        temp.path().join("outside-secret"),
        root.join("hardlink-escape"),
    )
    .unwrap();
    assert!(SandboxManager::prepare(&SandboxPolicy::workspace(root, SandboxMode::Strict)).is_err());
    println!(
        "Linux development, traversal, symlink, protected credential, /etc, namespace escape, nested mount, hardlink, reuse, teardown and fail-closed checks passed"
    );
}
