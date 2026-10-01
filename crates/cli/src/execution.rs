//! Launch a Linux AX in the default WSL distribution, preserving stdio and AX home.
use crate::config::{AgentEnvironment, AxConfig, TerminalShell};
use anyhow::Result;

pub(crate) fn configure(
    environment: Option<AgentEnvironment>,
    shell: Option<TerminalShell>,
) -> Result<()> {
    let config = select(environment, shell)?;
    println!("{}", serde_json::to_string_pretty(&config.execution)?);
    Ok(())
}

pub(crate) fn select(
    environment: Option<AgentEnvironment>,
    shell: Option<TerminalShell>,
) -> Result<AxConfig> {
    let mut config = AxConfig::load()?;
    if let Some(environment) = environment {
        if environment == AgentEnvironment::Wsl {
            check_wsl()?;
        }
        config.execution.environment = environment;
    }
    if let Some(shell) = shell {
        config.execution.terminal_shell = shell;
    }
    if environment.is_some() || shell.is_some() {
        config.save()?;
    }
    Ok(config)
}

pub(crate) fn check_wsl() -> Result<()> {
    #[cfg(windows)]
    {
        let output = wsl_command()
            .args([
                "--exec",
                "sh",
                "-lc",
                "exec \"$HOME/.local/bin/ax\" --version",
            ])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "WSL requires Linux AX at ~/.local/bin/ax in the default distribution: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
    #[cfg(not(windows))]
    anyhow::bail!("WSL is available only on Windows")
}

#[cfg(windows)]
fn wsl_command() -> std::process::Command {
    use std::io::IsTerminal;
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("wsl.exe");
    if !std::io::stdout().is_terminal() {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for Crew probes / ACP.
    }
    command
}

#[cfg(windows)]
fn linux_path(path: &std::ffi::OsStr) -> Result<String> {
    use anyhow::Context;
    if path.to_string_lossy().starts_with('/') {
        return Ok(path.to_string_lossy().into_owned());
    }
    let path = std::path::Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let output = wsl_command()
        .args(["--exec", "wslpath", "-a", "-u"])
        .arg(absolute)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Cannot map path into WSL: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)
        .context("Invalid WSL path")?
        .trim()
        .to_owned())
}

pub(crate) fn launch(cli: &crate::Cli) -> Result<Option<i32>> {
    #[cfg(windows)]
    {
        use std::process::Stdio;
        if AxConfig::load()?.execution.environment != AgentEnvironment::Wsl {
            return Ok(None);
        }
        let home = linux_path(crate::config::ax_home().as_os_str())?;
        let cwd = linux_path(std::env::current_dir()?.as_os_str())?;
        let mut args = Vec::new();
        let mut path_next = false;
        let archive_path = match &cli.command {
            Some(crate::Command::Export { path, .. } | crate::Command::Import { path, .. }) => {
                Some(path.as_os_str())
            }
            Some(
                crate::Command::Skill {
                    command: crate::CapabilityCommand::Import { path, .. },
                }
                | crate::Command::Mcp {
                    command: crate::CapabilityCommand::Import { path, .. },
                },
            ) => Some(path.as_os_str()),
            _ => None,
        };
        for argument in std::env::args_os().skip(1) {
            if archive_path == Some(argument.as_os_str()) {
                args.push(linux_path(&argument)?);
                continue;
            }
            if path_next {
                args.push(linux_path(&argument)?);
                path_next = false;
                continue;
            }
            let text = argument.to_string_lossy().into_owned();
            if let Some((flag, value)) = text.split_once('=')
                && matches!(
                    flag,
                    "--data-dir" | "--skills-dir" | "--mcp-config" | "--codex-auth"
                )
            {
                args.push(format!(
                    "{flag}={}",
                    linux_path(std::ffi::OsStr::new(value))?
                ));
                continue;
            }
            path_next = matches!(
                text.as_str(),
                "--data-dir" | "--skills-dir" | "--mcp-config" | "--codex-auth"
            );
            args.push(text);
        }
        let acp = matches!(cli.command, Some(crate::Command::Acp));
        let mut command = wsl_command();
        command
            .args([
                "--cd",
                &cwd,
                "--exec",
                "env",
                &format!("AX_HOME={home}"),
                "sh",
                "-lc",
                "exec \"$HOME/.local/bin/ax\" \"$@\"",
                "ax",
            ])
            .args(args);
        if acp {
            command.stdin(Stdio::piped());
        }
        let mut child = command.spawn()?;
        forward_acp(child.stdin.take());
        Ok(Some(child.wait()?.code().unwrap_or(1)))
    }
    #[cfg(not(windows))]
    {
        let _ = cli;
        Ok(None)
    }
}

#[cfg(windows)]
fn forward_acp(input: Option<std::process::ChildStdin>) {
    use std::io::{BufRead, Write};
    if let Some(mut input) = input {
        // ACP sends a Windows cwd; translate only this typed protocol field.
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(mut line) = line else { break };
                if let Ok(mut message) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(value) = message.pointer_mut("/params/cwd")
                        && let Some(path) = value.as_str()
                    {
                        match linux_path(std::ffi::OsStr::new(path)) {
                            Ok(path) => *value = path.into(),
                            Err(error) => {
                                eprintln!("{error}");
                                break;
                            }
                        }
                    }
                    line = message.to_string();
                }
                if writeln!(input, "{line}")
                    .and_then(|()| input.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn execution_defaults_and_round_trip() {
        let config: AxConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.execution.environment, AgentEnvironment::Native);
        assert_eq!(config.execution.terminal_shell, TerminalShell::Powershell);
        let config: AxConfig = serde_json::from_str(r#"{"execution":{"environment":"wsl","terminal_shell":"git_bash"},"model":{"provider":"deepseek","model":"test"}}"#).unwrap();
        let restored: AxConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.execution.environment, AgentEnvironment::Wsl);
        assert_eq!(restored.execution.terminal_shell, TerminalShell::GitBash);
        assert_eq!(restored.model.unwrap().model, "test");
    }
}
