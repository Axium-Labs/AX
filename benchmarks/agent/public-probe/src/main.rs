//! Model-free environment preflight using the production sandbox implementation.
use sandbox::{CommandSpec, SandboxManager, SandboxMode, SandboxPolicy};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    #[cfg(target_os = "linux")]
    match first.as_deref() {
        Some("--ax-sandbox-broker") => {
            sandbox::run_broker(PathBuf::from(args.next().ok_or("broker socket required")?))
                .await?;
            return Ok(());
        }
        Some("--ax-sandbox-proxy") => {
            let control = PathBuf::from(args.next().ok_or("control socket required")?);
            let command = args.next().ok_or("command socket required")?;
            std::process::exit(sandbox::run_proxy(control, &command).await?);
        }
        _ => {}
    }
    let workspace =
        PathBuf::from(first.ok_or("usage: ax-bench-sandbox-probe WORKSPACE")?).canonicalize()?;
    let policy = SandboxPolicy::workspace(workspace, SandboxMode::Workspace);
    let manager = SandboxManager::prepare(&policy)?;
    let result = manager.output_blocking(CommandSpec::new("/bin/true"))?;
    if !result.status.success() {
        return Err(format!(
            "production sandbox probe failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    println!(
        "{}",
        serde_json::json!({"probe":"production_sandbox_prepare_and_execute","mode":"workspace","passed":true})
    );
    Ok(())
}
