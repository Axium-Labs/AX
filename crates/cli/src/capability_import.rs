//! Import into the existing lazy MCP registry; never connect during installation.
use anyhow::{Result, bail};
use std::{fs, io::Write, path::Path};

pub(crate) fn run(cli: &crate::Cli, _data_dir: &Path, _skills_dir: &Path) -> Result<bool> {
    if let Some(crate::Command::Skill {
        command: crate::CapabilityCommand::Import { path, global },
    }) = &cli.command
    {
        let root = if *global {
            crate::config::ax_home().join("skills")
        } else {
            crate::discover_project_root(&std::env::current_dir()?).join(".ax/skills")
        };
        println!("{}", skill::install_skill_directory(path, &root)?.display());
        return Ok(true);
    }
    if let Some(crate::Command::Mcp {
        command: crate::CapabilityCommand::Import { path, global },
    }) = &cli.command
    {
        let destination = if *global {
            crate::config::ax_home().join("mcp.toml")
        } else {
            cli.mcp_config.clone().unwrap_or_else(|| {
                crate::discover_project_root(&std::env::current_dir().expect("cwd"))
                    .join(".ax/mcp.toml")
            })
        };
        import_mcp(path, &destination)?;
        println!("MCP configuration imported: {}", destination.display());
        return Ok(true);
    }
    Ok(false)
}

fn source_config(source: &Path) -> Result<toml::Value> {
    let contents = fs::read_to_string(source)?;
    if source.extension().is_some_and(|ext| ext == "json") {
        let mut value: serde_json::Value = serde_json::from_str(&contents)?;
        if let Some(servers) = value.get_mut("mcpServers") {
            let mut servers = servers
                .as_object()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("mcpServers must be an object"))?;
            for server in servers.values_mut() {
                let fields = server
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("server must be an object"))?;
                if !fields.contains_key("transport") {
                    let transport = if fields.contains_key("command") {
                        "stdio"
                    } else {
                        "http"
                    };
                    fields.insert("transport".into(), transport.into());
                }
                if let Some(kind) = fields.remove("type") {
                    let transport = match kind.as_str() {
                        Some("stdio") => "stdio",
                        Some("http" | "streamable-http") => "http",
                        Some("websocket") => "websocket",
                        _ => bail!("Unsupported MCP transport type"),
                    };
                    fields.insert("transport".into(), transport.into());
                }
            }
            value = serde_json::json!({"servers":servers});
        }
        let config: toml::Value = serde_json::from_value(value)?;
        Ok(config)
    } else {
        let config: toml::Value = toml::from_str(&contents)?;
        if let Some(servers) = config.get("mcp_servers") {
            let mut servers = servers
                .as_table()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("mcp_servers must be a table"))?;
            for (_, server) in &mut servers {
                let fields = server
                    .as_table_mut()
                    .ok_or_else(|| anyhow::anyhow!("server must be a table"))?;
                if fields.contains_key("env_vars")
                    || fields.contains_key("bearer_token_env_var")
                    || fields.contains_key("env_http_headers")
                {
                    bail!(
                        "Environment-referenced Codex credentials are not supported; use explicit env/headers in an export"
                    );
                }
                if let Some(headers) = fields.remove("http_headers") {
                    fields.insert("headers".into(), headers);
                }
                if let Some(timeout) = fields.remove("tool_timeout_sec") {
                    fields.insert("request_timeout_secs".into(), timeout);
                }
                fields.remove("startup_timeout_sec");
                fields.remove("startup_timeout_ms");
                let transport = if fields.contains_key("command") {
                    "stdio"
                } else {
                    "http"
                };
                fields
                    .entry("transport")
                    .or_insert_with(|| toml::Value::String(transport.into()));
            }
            return Ok(toml::Value::Table(toml::Table::from_iter([(
                "servers".into(),
                toml::Value::Table(servers),
            )])));
        }
        Ok(config)
    }
}

pub(crate) fn import_mcp(source: &Path, destination: &Path) -> Result<()> {
    let incoming = source_config(source)?;
    let _: mcp::McpConfig = incoming.clone().try_into()?;
    let additions = incoming
        .get("servers")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("Missing servers table"))?;
    if additions.is_empty() {
        bail!("No MCP servers to import");
    }
    let mut merged: toml::Value = if destination.exists() {
        toml::from_str(&fs::read_to_string(destination)?)?
    } else {
        toml::Value::Table(toml::Table::new())
    };
    let table = merged
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid existing MCP config"))?;
    let servers = table
        .entry("servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid existing servers table"))?;
    for name in additions.keys() {
        if name.trim().is_empty() || servers.contains_key(name) {
            bail!("Empty or conflicting MCP server name: {name}");
        }
    }
    servers.extend(additions.clone());
    let _: mcp::McpConfig = merged.clone().try_into()?;
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid destination"))?;
    fs::create_dir_all(parent)?;
    let temporary = destination.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME")?;
            let status = std::process::Command::new("icacls")
                .arg(&temporary)
                .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
                .output()?;
            if !status.status.success() {
                bail!("Could not secure MCP config permissions");
            }
        }
        file.write_all(toml::to_string_pretty(&merged)?.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_json_and_rejects_conflicts_without_changing_config() {
        let root = std::env::temp_dir().join(format!("ax-mcp-import-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.json");
        let destination = root.join("mcp.toml");
        fs::write(
            &source,
            r#"{"mcpServers":{"files":{"command":"never-run","args":["--stdio"]}}}"#,
        )
        .unwrap();
        import_mcp(&source, &destination).unwrap();
        let before = fs::read(&destination).unwrap();
        assert_eq!(mcp::McpConfig::load(&destination).unwrap().servers.len(), 1);
        assert!(import_mcp(&source, &destination).is_err());
        assert_eq!(before, fs::read(&destination).unwrap());
        fs::write(
            &source,
            r#"{"mcpServers":{"bad":{"type":"sse","url":"https://example.test"}}}"#,
        )
        .unwrap();
        assert!(import_mcp(&source, &destination).is_err());
        assert_eq!(before, fs::read(&destination).unwrap());
        let codex_source = root.join("config.toml");
        fs::write(
            &codex_source,
            "model = 'unrelated'\n[mcp_servers.codex]\ncommand = 'never-run'\n",
        )
        .unwrap();
        import_mcp(&codex_source, &destination).unwrap();
        assert!(
            mcp::McpConfig::load(&destination)
                .unwrap()
                .servers
                .contains_key("codex")
        );
        let toml_source = root.join("source.toml");
        fs::write(
            &toml_source,
            "[servers.remote]\ntransport = \"http\"\nurl = \"https://example.test/mcp\"\n",
        )
        .unwrap();
        import_mcp(&toml_source, &destination).unwrap();
        let config = mcp::McpConfig::load(&destination).unwrap();
        assert_eq!(config.servers.len(), 3);
        assert!(config.servers.contains_key("files"));
        fs::remove_dir_all(root).unwrap();
    }
}
