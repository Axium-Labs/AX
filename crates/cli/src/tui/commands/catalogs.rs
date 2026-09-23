//! Skill, tool, and MCP metadata managers.
use super::{BottomPane, ReplState, Result, SurfaceItem, SurfaceView, item};
use crate::tools;
use tool::Tool;

fn available_tools(state: &ReplState) -> Vec<String> {
    let mut names = tools(&state.mcp_tools)
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.push("mcp".into());
    names.push("memory".into());
    names
}

pub(super) fn skill_items(state: &mut ReplState) -> Result<Vec<SurfaceItem>> {
    let available = available_tools(state);
    let disabled = state.disabled_skills()?;
    Ok(state
        .skills()?
        .statuses(available.iter().map(String::as_str))
        .into_iter()
        .map(|s| {
            let status = if disabled.contains(&s.metadata.name) {
                "disabled".to_owned()
            } else if s.available() {
                "enabled".to_owned()
            } else {
                format!("unavailable: missing {}", s.missing_tools.join(", "))
            };
            SurfaceItem {
                id: s.metadata.name.clone(),
                label: s.metadata.name,
                value: format!("{status} - {}", s.metadata.description),
            }
        })
        .collect())
}

pub(super) fn open_skills(state: &mut ReplState, pane: &mut BottomPane) -> Result<()> {
    let items = skill_items(state)?;
    let mut help = vec!["Type to search names, descriptions, or status".into()];
    help.extend(
        state
            .skills()?
            .issues()
            .iter()
            .map(|issue| format!("Skipped: {issue}")),
    );
    pane.push_view(SurfaceView::manager(
        "Skills",
        "skills",
        help,
        items,
        "Enter details | Space enable/disable | Esc back",
    ));
    Ok(())
}

pub(super) fn open_tools(state: &ReplState, pane: &mut BottomPane) {
    let registry = tools(&state.mcp_tools);
    let mut items = registry
        .names()
        .into_iter()
        .map(|name| {
            let tool = registry.get(name).expect("registered tool");
            let source = if name.starts_with("mcp__") {
                "MCP"
            } else {
                "Built-in"
            };
            SurfaceItem {
                id: name.into(),
                label: name.into(),
                value: format!("{source} - {}", tool.description()),
            }
        })
        .collect::<Vec<_>>();
    items.push(item(
        "mcp",
        "mcp",
        "MCP - capability catalog / lazy gateway",
    ));
    pane.push_view(SurfaceView::manager(
        "Tools",
        "tools",
        vec!["Type to search names, descriptions, or source (Built-in / MCP)".into()],
        items,
        "Enter description and input schema | Esc back",
    ));
}

pub(super) async fn open_mcp(state: &mut ReplState, pane: &mut BottomPane) -> Result<()> {
    let items = mcp_items(state).await?;
    pane.push_view(SurfaceView::manager(
        "MCP Servers",
        "mcp",
        vec!["Servers connect lazily. Type to search.".into()],
        items,
        "Enter details | Alt+C connect | Alt+X disconnect | Alt+R restart | Esc back",
    ));
    Ok(())
}

fn server_state(status: &mcp::ServerStatus) -> &'static str {
    if !status.enabled {
        "disabled"
    } else if status.last_error.is_some() {
        "error"
    } else if status.connected {
        "connected"
    } else {
        "sleeping"
    }
}

pub(super) async fn mcp_items(state: &mut ReplState) -> Result<Vec<SurfaceItem>> {
    Ok(state
        .mcp()?
        .lock()
        .await
        .statuses()
        .iter()
        .map(|s| SurfaceItem {
            id: s.name.clone(),
            label: s.name.clone(),
            value: format!(
                "{} | {} | {} discovered tools",
                server_state(s),
                s.transport,
                s.tools.len()
            ),
        })
        .collect())
}

pub(super) async fn open_detail(
    surface: &str,
    id: &str,
    state: &mut ReplState,
    pane: &mut BottomPane,
) -> Result<()> {
    let mut lines = vec![format!("Name: {id}")];
    match surface {
        "skills" => append_skill_details(state, id, &mut lines)?,
        "tools" => {
            let registry = tools(&state.mcp_tools);
            let gateway = mcp::McpGateway::new(state.mcp()?);
            let memory = crate::memory_tool::MemoryTool::default();
            let registered = registry.get(id);
            let selected: &dyn Tool = if id == "memory" {
                &memory
            } else if id == "mcp" {
                &gateway
            } else {
                registered
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Unknown tool: {id}"))?
                    .as_ref()
            };
            lines.push(format!("Description: {}", selected.description()));
            if let Some(proxy) = state.mcp_tools.iter().find(|proxy| proxy.name() == id) {
                lines.push(format!(
                    "Server: {} / Remote tool: {}",
                    proxy.server(),
                    proxy.remote_name()
                ));
            }
            lines.push(
                "Permissions are evaluated per call using its arguments; see /permissions.".into(),
            );
            lines.push("Input schema:".into());
            lines.extend(
                serde_json::to_string_pretty(&selected.input_schema())?
                    .lines()
                    .map(str::to_owned),
            );
        }
        "mcp" => {
            let statuses = state.mcp()?.lock().await.statuses();
            let status = statuses
                .iter()
                .find(|s| s.name == id)
                .ok_or_else(|| anyhow::anyhow!("Unknown server: {id}"))?;
            lines.push(format!("Status: {}", server_state(status)));
            lines.push(format!("Transport: {}", status.transport));
            lines.push(format!("Configured protocol: {}", status.protocol_version));
            if let Some(error) = &status.last_error {
                lines.push(format!("Last error: {error}"));
            }
            lines.push("Discovered tools (connect to populate):".into());
            for tool in &status.tools {
                lines.push(format!(
                    "{} - {}",
                    tool.name,
                    tool.description.as_deref().unwrap_or("")
                ));
                lines.extend(
                    serde_json::to_string_pretty(&tool.input_schema)?
                        .lines()
                        .map(str::to_owned),
                );
            }
        }
        _ => {}
    }
    pane.push_view(SurfaceView::info("Details", lines));
    Ok(())
}

fn append_skill_details(state: &mut ReplState, id: &str, lines: &mut Vec<String>) -> Result<()> {
    let available = available_tools(state);
    let disabled = state.disabled_skills()?.contains(id);
    let catalog = state.skills()?;
    let status = catalog
        .statuses(available.iter().map(String::as_str))
        .into_iter()
        .find(|s| s.metadata.name == id)
        .ok_or_else(|| anyhow::anyhow!("Unknown skill: {id}"))?;
    lines.push(format!(
        "Source: {}",
        catalog.directory(id).expect("indexed skill").display()
    ));
    lines.push(format!("Enabled: {}", !disabled));
    lines.push(format!(
        "Missing tools: {}",
        status.missing_tools.join(", ")
    ));
    lines.push(format!("Description: {}", status.metadata.description));
    if let Some(license) = &status.metadata.license {
        lines.push(format!("License: {license}"));
    }
    if let Some(compatibility) = &status.metadata.compatibility {
        lines.push(format!("Compatibility: {compatibility}"));
    }
    if let Some(allowed) = &status.metadata.allowed_tools {
        lines.push(format!("Declared allowed tools: {allowed}"));
    }
    lines.push(format!(
        "Required tools: {}",
        status.metadata.required_tools.join(", ")
    ));
    lines.push("Instructions load only when routed into a task.".into());
    Ok(())
}
