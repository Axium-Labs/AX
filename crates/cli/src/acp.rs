//! ACP v1 stdio adapter for the existing AX composition root.
//! JSON-RPC is newline framed; no model, tool, session, or memory logic lives here.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use mcp::{CURRENT_PROTOCOL_VERSION, McpConfig, ServerConfig, TransportConfig};
use memory::MessageRole;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
    task::AbortHandle,
};
use uuid::Uuid;

use crate::{Cli, ReplState, context_budget, model_selection, run_prompt_with};
use runtime_core::{AgentEvent, ApprovalPolicy};
use tool::{Capability, PermissionDecision, PermissionStore, SafetyLevel, ToolPermission};

type Outbox = mpsc::UnboundedSender<Value>;
type PendingPermissions = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

struct ActivePrompt {
    request_id: Value,
    session_id: String,
    abort: AbortHandle,
}

struct AcpApproval {
    out: Outbox,
    pending: PendingPermissions,
    permissions: PermissionStore,
    session_id: String,
}

#[async_trait]
impl ApprovalPolicy for AcpApproval {
    async fn approve(&self, name: &str, input: &Value, permission: ToolPermission) -> bool {
        match self.permissions.decision(permission.capability) {
            PermissionDecision::Allow => return true,
            PermissionDecision::Deny => return false,
            PermissionDecision::Ask if permission.safety == SafetyLevel::Safe => return true,
            PermissionDecision::Ask => {}
        }
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let _ = self.out.send(json!({
            "jsonrpc":"2.0", "id":id, "method":"session/request_permission",
            "params":{
                "sessionId":self.session_id,
                "toolCall":{"toolCallId":id,"title":name,"kind":"execute","status":"pending","rawInput":input},
                "options":[
                    {"optionId":"allow_once","name":"Allow once","kind":"allow_once"},
                    {"optionId":"allow_session","name":"Allow for session","kind":"allow_always"},
                    {"optionId":"reject_once","name":"Deny","kind":"reject_once"}
                ]
            }
        }));
        match rx.await.as_deref() {
            Ok("allow_once") => true,
            Ok("allow_session") => {
                self.permissions.allow_session(permission.capability);
                true
            }
            _ => false,
        }
    }
}

fn reply(out: &Outbox, id: Value, result: Value) {
    let _ = out.send(Value::Object(serde_json::Map::from_iter([
        ("jsonrpc".into(), json!("2.0")),
        ("id".into(), id),
        ("result".into(), result),
    ])));
}

fn error(out: &Outbox, id: Value, code: i32, message: impl AsRef<str>) {
    let _ = out.send(Value::Object(serde_json::Map::from_iter([
        ("jsonrpc".into(), json!("2.0")),
        ("id".into(), id),
        (
            "error".into(),
            json!({"code":code,"message":message.as_ref()}),
        ),
    ])));
}

/// AX's built-in tool catalog, sorted by name.
///
/// MCP-provided tools only exist after a session connects to each configured
/// server, so this read-only catalog stays offline and reports the tools AX
/// itself ships; `_ax/mcp` reports the servers.
fn builtin_tools() -> Vec<Value> {
    let mut tools = crate::tools(&[])
        .iter()
        .map(|tool| (tool.name().to_owned(), tool.description().to_owned()))
        .collect::<Vec<_>>();
    tools.sort();
    tools
        .into_iter()
        .map(|(name, description)| json!({"name":name,"description":description}))
        .collect()
}

/// ACP has no field for the time a message was produced, so the transcript carries
/// it in the private `_ax` object clients already use for vendor extensions.
fn stamp(mut body: Value, at: i64) -> Value {
    if let Value::Object(ref mut map) = body {
        map.insert("_ax".to_owned(), json!({"createdAt": at}));
    }
    body
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn update(out: &Outbox, session_id: &str, event: AgentEvent, calls: &mut HashMap<String, String>) {
    let body = match event {
        AgentEvent::ContentDelta { delta } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":delta}})
        }
        AgentEvent::ThinkingDelta { delta } => {
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":delta}})
        }
        AgentEvent::ToolStarted { name, detail } => {
            let id = Uuid::new_v4().to_string();
            calls.insert(name.clone(), id.clone());
            json!({"sessionUpdate":"tool_call","toolCallId":id,"title":detail,"kind":"execute","status":"pending","rawInput":{"name":name}})
        }
        AgentEvent::ToolFinished { name, success } => {
            json!({"sessionUpdate":"tool_call_update","toolCallId":calls.remove(&name).unwrap_or_else(|| name.clone()),"status":if success {"completed"} else {"failed"}})
        }
        AgentEvent::ModelStarted { .. }
        | AgentEvent::ContextCompressed { .. }
        | AgentEvent::TurnStarted
        | AgentEvent::TurnFinished => return,
    };
    let _ = out.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":stamp(body, now_seconds())}}));
}

/// Every catalog provider AX cannot drive, with the reason. Clients render
/// these as unsupported rather than offering a credential dialog that would
/// silently do nothing.
fn provider_catalog(codex_auth: Option<&PathBuf>) -> Vec<Value> {
    let credentialed = crate::providers::credentialed_providers(codex_auth);
    let stored = model::AuthStorage::new(crate::ax_auth_path())
        .provider_ids()
        .unwrap_or_default();
    let snapshot = crate::tui::catalog_refresh::cached_snapshot(&PathBuf::new(), codex_auth);
    model::PROVIDERS.iter().map(|spec| {
        let configured = credentialed.iter().any(|id| id == spec.id);
        let supported = model::provider_supported(spec.id);
        let models = if configured && supported {
            snapshot.iter().filter(|item| item.provider == spec.id && item.supports_tools).cloned().collect::<Vec<_>>()
        } else { Vec::new() };
        json!({"id":spec.id,"name":spec.name,"configured":configured,"supported":supported,
            "unsupported_reason":model::provider_unsupported_reason(spec.id),
            "source":if !configured { None } else if stored.iter().any(|id| id == spec.id) { Some("AX") } else { Some("环境变量") },
            "model_source": if models.is_empty() { "none" } else if crate::ax_models_dir().join(format!("{}.json", spec.id)).is_file() { "cache" } else { "fallback" },
            "models":models})
    }).collect()
}

fn unsupported_providers() -> Vec<Value> {
    model::PROVIDERS
        .iter()
        .filter_map(|spec| {
            model::provider_unsupported_reason(spec.id)
                .map(|reason| json!({"id":spec.id,"name":spec.name,"reason":reason}))
        })
        .collect()
}

/// Credentials the user has stored for providers AX cannot drive. These are the
/// entries that look configured but can never run a prompt, so a client can say
/// "key saved, not supported" instead of showing them as connected.
fn unsupported_configured_providers(
    configured: &[String],
    codex_auth: Option<&PathBuf>,
) -> Vec<Value> {
    crate::providers::credentialed_providers(codex_auth)
        .into_iter()
        .filter(|id| !configured.iter().any(|supported| supported == id))
        .filter_map(|id| {
            model::provider_unsupported_reason(&id).map(|reason| json!({"id":id,"reason":reason}))
        })
        .collect()
}

fn prompt_text(params: &Value) -> Result<String> {
    let blocks = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("prompt must be a content-block array"))?;
    let mut texts = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => texts.push(
                block["text"]
                    .as_str()
                    .ok_or_else(|| anyhow!("text block missing text"))?
                    .to_owned(),
            ),
            Some("resource_link") => {
                let uri = block["uri"]
                    .as_str()
                    .ok_or_else(|| anyhow!("resource link missing uri"))?;
                let name = block["name"].as_str().unwrap_or("resource");
                texts.push(format!("Referenced resource: {name} ({uri})"));
            }
            _ => return Err(anyhow!("unsupported ACP prompt content block")),
        }
    }
    if texts.is_empty() {
        return Err(anyhow!("AX currently supports text prompt blocks only"));
    }
    Ok(texts.join("\n"))
}

fn session_cwd(params: &Value, cwd: &std::path::Path) -> Result<()> {
    let requested = params
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("cwd required"))?;
    if PathBuf::from(requested).canonicalize()? != cwd.canonicalize()? {
        return Err(anyhow!("AX ACP process cwd must match session cwd"));
    }
    Ok(())
}

fn client_mcp(params: &Value) -> Result<Option<McpConfig>> {
    let Some(entries) = params.get("mcpServers").and_then(Value::as_array) else {
        return Ok(None);
    };
    if entries.is_empty() {
        return Ok(None);
    }
    let mut servers = BTreeMap::new();
    for entry in entries {
        let name = entry["name"]
            .as_str()
            .ok_or_else(|| anyhow!("MCP server name required"))?;
        let transport = if let Some(command) = entry["command"].as_str() {
            let args = entry["args"]
                .as_array()
                .ok_or_else(|| anyhow!("MCP args required"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| anyhow!("MCP args must be strings"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut env = BTreeMap::new();
            for variable in entry["env"].as_array().into_iter().flatten() {
                let key = variable["name"]
                    .as_str()
                    .ok_or_else(|| anyhow!("MCP env name required"))?;
                let value = variable["value"]
                    .as_str()
                    .ok_or_else(|| anyhow!("MCP env value required"))?;
                env.insert(key.to_owned(), value.to_owned());
            }
            TransportConfig::Stdio {
                command: command.to_owned(),
                args,
                env,
                cwd: None,
            }
        } else if entry["type"] == "http" {
            let url = entry["url"]
                .as_str()
                .ok_or_else(|| anyhow!("MCP HTTP url required"))?;
            let mut headers = BTreeMap::new();
            for header in entry["headers"].as_array().into_iter().flatten() {
                let key = header["name"]
                    .as_str()
                    .ok_or_else(|| anyhow!("MCP header name required"))?;
                let value = header["value"]
                    .as_str()
                    .ok_or_else(|| anyhow!("MCP header value required"))?;
                headers.insert(key.to_owned(), value.to_owned());
            }
            TransportConfig::Http {
                url: url.to_owned(),
                headers,
            }
        } else {
            return Err(anyhow!("unsupported ACP MCP transport"));
        };
        if servers
            .insert(
                name.to_owned(),
                ServerConfig {
                    description: String::new(),
                    capabilities: Vec::new(),
                    enabled: true,
                    protocol_version: CURRENT_PROTOCOL_VERSION.to_owned(),
                    request_timeout_secs: 30,
                    transport,
                },
            )
            .is_some()
        {
            return Err(anyhow!("duplicate MCP server name"));
        }
    }
    Ok(Some(McpConfig { servers }))
}

fn client_skills(params: &Value) -> Result<Option<HashSet<String>>> {
    let Some(value) = params.pointer("/_ax/skills") else {
        return Ok(None);
    };
    let list = value
        .as_array()
        .ok_or_else(|| anyhow!("_ax.skills must be an array"))?;
    list.iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("_ax.skills must contain strings"))
        })
        .collect::<Result<HashSet<_>>>()
        .map(Some)
}
fn client_mcp_names(params: &Value) -> Result<Option<HashSet<String>>> {
    let Some(value) = params.pointer("/_ax/mcpServers") else {
        return Ok(None);
    };
    let list = value
        .as_array()
        .ok_or_else(|| anyhow!("_ax.mcpServers must be an array"))?;
    list.iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("_ax.mcpServers must contain names"))
        })
        .collect::<Result<HashSet<_>>>()
        .map(Some)
}
fn apply_mcp_names(state: &mut ReplState, names: Option<&HashSet<String>>) -> Result<()> {
    let Some(names) = names else { return Ok(()) };
    let mut config = McpConfig::load(&state.mcp_config)?;
    for name in names {
        if !config.servers.contains_key(name) {
            return Err(anyhow!("unknown local MCP server: {name}"));
        }
    }
    config.servers.retain(|name, _| names.contains(name));
    state.mcp_override = Some(config);
    Ok(())
}
fn client_permission_profile(params: &Value) -> Result<Option<String>> {
    let Some(value) = params.pointer("/_ax/permissionProfile") else {
        return Ok(None);
    };
    let profile = value
        .as_str()
        .ok_or_else(|| anyhow!("permissionProfile must be a string"))?;
    if !matches!(profile, "ask" | "allow" | "deny") {
        return Err(anyhow!("unknown permissionProfile"));
    }
    Ok(Some(profile.to_owned()))
}
fn apply_permission_profile(store: &PermissionStore, profile: Option<&str>) {
    let decision = match profile {
        Some("allow") => PermissionDecision::Allow,
        Some("deny") => PermissionDecision::Deny,
        _ => return,
    };
    for capability in [
        Capability::Shell,
        Capability::FilesystemRead,
        Capability::FilesystemWrite,
        Capability::Network,
        Capability::Mcp,
        Capability::Process,
    ] {
        store.set_capability(capability, decision);
    }
}

fn replay(out: &Outbox, session_id: &str, state: &mut ReplState) -> Result<()> {
    let mut pages = Vec::new();
    let mut before = None;
    loop {
        let page = state.store()?.load_messages(session_id, before, 128)?;
        if page.is_empty() {
            break;
        }
        before = page.first().map(|item| item.id);
        pages.push(page);
    }
    for message in pages.into_iter().rev().flatten() {
        let update = match message.role {
            MessageRole::User => {
                json!({"sessionUpdate":"user_message_chunk","messageId":message.id.to_string(),"content":{"type":"text","text":message.content}})
            }
            MessageRole::Assistant => {
                json!({"sessionUpdate":"agent_message_chunk","messageId":message.id.to_string(),"content":{"type":"text","text":message.content}})
            }
            MessageRole::Tool => {
                json!({"sessionUpdate":"tool_call_update","toolCallId":message.metadata.get("tool_call_id").and_then(Value::as_str).unwrap_or("unknown"),"status":"completed","content":[{"type":"content","content":{"type":"text","text":message.content}}]})
            }
            MessageRole::System => continue,
        };
        out.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":stamp(update, message.created_at)}})).ok();
    }
    Ok(())
}

/// Runs the adapter without changing the ordinary AX CLI startup path.
#[allow(clippy::too_many_lines)]
pub async fn run(cli: &Cli, data_dir: PathBuf, skills_dir: PathBuf) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let mcp_config = cli.mcp_config.clone();
    let (out, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(value) = rx.recv().await {
            stdout
                .write_all(serde_json::to_string(&value)?.as_bytes())
                .await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    });
    let pending: PendingPermissions = Arc::new(Mutex::new(HashMap::new()));
    let active: Arc<Mutex<Option<ActivePrompt>>> = Arc::new(Mutex::new(None));
    let session_mcp: Arc<Mutex<HashMap<String, McpConfig>>> = Arc::new(Mutex::new(HashMap::new()));
    let session_mcp_names: Arc<Mutex<HashMap<String, HashSet<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_skills: Arc<Mutex<HashMap<String, HashSet<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_permissions: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut initialized = false;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let msg: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                error(&out, Value::Null, -32700, err.to_string());
                continue;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        // Responses to agent-initiated permission requests have no method.
        if msg.get("method").is_none() {
            if let Some(key) = id.as_str()
                && let Some(tx) = pending.lock().unwrap().remove(key)
            {
                let option = msg
                    .pointer("/result/outcome/optionId")
                    .and_then(Value::as_str);
                let _ = tx.send(option.unwrap_or("reject_once").to_owned());
            }
            continue;
        }
        let method = msg["method"].as_str().unwrap_or("");
        let params = &msg["params"];
        if method != "initialize" && !initialized {
            error(
                &out,
                id,
                -32000,
                "initialize required before session methods",
            );
            continue;
        }
        match method {
            "initialize" => {
                if params["protocolVersion"].as_i64().is_none() {
                    error(&out, id, -32602, "protocolVersion required");
                    continue;
                }
                initialized = true;
                reply(
                    &out,
                    id,
                    json!({
                        "protocolVersion":1,
                        "agentCapabilities":{"loadSession":true,"sessionCapabilities":{"resume":{}},"promptCapabilities":{"image":false,"audio":false,"embeddedContext":false},"mcpCapabilities":{"http":true,"sse":false}},
                        "agentInfo":{"name":"AX","title":"AX","version":env!("CARGO_PKG_VERSION")},
                        "authMethods":[]
                    }),
                );
            }
            "session/new" => {
                if let Err(err) = session_cwd(params, &cwd) {
                    error(&out, id, -32602, err.to_string());
                    continue;
                }
                let requested_mcp = match client_mcp(params) {
                    Ok(config) => config,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let requested_mcp_names = match client_mcp_names(params) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let requested_skills = match client_skills(params) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let requested_permissions = match client_permission_profile(params) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                match ReplState::new_in_project(
                    data_dir.clone(),
                    skills_dir.clone(),
                    mcp_config.clone(),
                    &cwd,
                )
                .and_then(|mut state| {
                    if let Some(config) = requested_mcp.clone() {
                        state.mcp_override = Some(config);
                    }
                    apply_mcp_names(&mut state, requested_mcp_names.as_ref())?;
                    state.create_session("ACP session")?;
                    Ok(state.current_session_id()?.to_owned())
                }) {
                    Ok(session_id) => {
                        if let Some(config) = requested_mcp {
                            session_mcp
                                .lock()
                                .unwrap()
                                .insert(session_id.clone(), config);
                        }
                        if let Some(names) = requested_mcp_names {
                            session_mcp_names
                                .lock()
                                .unwrap()
                                .insert(session_id.clone(), names);
                        }
                        if let Some(skills) = requested_skills {
                            session_skills
                                .lock()
                                .unwrap()
                                .insert(session_id.clone(), skills);
                        }
                        if let Some(profile) = requested_permissions {
                            session_permissions
                                .lock()
                                .unwrap()
                                .insert(session_id.clone(), profile);
                        }
                        reply(&out, id, json!({"sessionId":session_id}));
                    }
                    Err(err) => error(&out, id, -32000, err.to_string()),
                }
            }
            "session/load" | "session/resume" => {
                let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
                    error(&out, id, -32602, "sessionId required");
                    continue;
                };
                if let Err(err) = session_cwd(params, &cwd) {
                    error(&out, id, -32602, err.to_string());
                    continue;
                }
                let requested_mcp = match client_mcp(params) {
                    Ok(config) => config,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let requested_mcp_names = match client_mcp_names(params) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let requested_skills = match client_skills(params) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let requested_permissions = match client_permission_profile(params) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                match ReplState::new_in_project(
                    data_dir.clone(),
                    skills_dir.clone(),
                    mcp_config.clone(),
                    &cwd,
                )
                .and_then(|mut state| {
                    if let Some(config) = requested_mcp.clone() {
                        state.mcp_override = Some(config);
                    }
                    apply_mcp_names(&mut state, requested_mcp_names.as_ref())?;
                    state.allowed_skills.clone_from(&requested_skills);
                    if method == "session/load" {
                        if state.store()?.session(session_id)?.is_none() {
                            return Ok(false);
                        }
                        replay(&out, session_id, &mut state)?;
                    } else if let Ok(selection) = model_selection::require_resolved(cli) {
                        let budget = context_budget(&selection, &[]);
                        if !state.open_session(session_id, &budget)? {
                            return Ok(false);
                        }
                    } else if state.store()?.session(session_id)?.is_none() {
                        return Ok(false);
                    }
                    Ok(true)
                }) {
                    Ok(true) => {
                        if let Some(config) = requested_mcp {
                            session_mcp
                                .lock()
                                .unwrap()
                                .insert(session_id.to_owned(), config);
                        }
                        if let Some(names) = requested_mcp_names {
                            session_mcp_names
                                .lock()
                                .unwrap()
                                .insert(session_id.to_owned(), names);
                        }
                        if let Some(skills) = requested_skills {
                            session_skills
                                .lock()
                                .unwrap()
                                .insert(session_id.to_owned(), skills);
                        }
                        if let Some(profile) = requested_permissions {
                            session_permissions
                                .lock()
                                .unwrap()
                                .insert(session_id.to_owned(), profile);
                        }
                        reply(&out, id, json!({}));
                    }
                    Ok(false) => error(&out, id, -32001, "AX session not found"),
                    Err(err) => error(&out, id, -32000, err.to_string()),
                }
            }
            "session/prompt" => {
                let Some(session_id) = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                else {
                    error(&out, id, -32602, "sessionId required");
                    continue;
                };
                let prompt = match prompt_text(params) {
                    Ok(prompt) => prompt,
                    Err(err) => {
                        error(&out, id, -32602, err.to_string());
                        continue;
                    }
                };
                let selection = match model_selection::require_resolved(cli) {
                    Ok(value) => value,
                    Err(err) => {
                        error(&out, id, -32000, err.to_string());
                        continue;
                    }
                };
                let mut slot = active.lock().unwrap();
                if slot.is_some() {
                    error(
                        &out,
                        id,
                        -32002,
                        "another prompt is running on this ACP connection",
                    );
                    continue;
                }
                let task_out = out.clone();
                let task_active = active.clone();
                let task_pending = pending.clone();
                let task_id = id.clone();
                let task_session = session_id.clone();
                let task_data = data_dir.clone();
                let task_skills = skills_dir.clone();
                let task_mcp = mcp_config.clone();
                let task_override = session_mcp.lock().unwrap().get(&session_id).cloned();
                let task_mcp_names = session_mcp_names.lock().unwrap().get(&session_id).cloned();
                let task_skills_allowed = session_skills.lock().unwrap().get(&session_id).cloned();
                let task_profile = session_permissions
                    .lock()
                    .unwrap()
                    .get(&session_id)
                    .cloned();
                let task_cwd = cwd.clone();
                let task = tokio::spawn(async move {
                    let result: Result<String> = async {
                        let mut state =
                            ReplState::new_in_project(task_data, task_skills, task_mcp, &task_cwd)?;
                        state.mcp_override = task_override;
                        apply_mcp_names(&mut state, task_mcp_names.as_ref())?;
                        state.allowed_skills = task_skills_allowed;
                        let budget = context_budget(&selection, &[]);
                        if !state.open_session(&task_session, &budget)? {
                            return Err(anyhow!("AX session not found"));
                        }
                        apply_permission_profile(&state.permissions, task_profile.as_deref());
                        let approval: Arc<dyn ApprovalPolicy> = Arc::new(AcpApproval {
                            out: task_out.clone(),
                            pending: task_pending,
                            permissions: state.permissions.clone(),
                            session_id: task_session.clone(),
                        });
                        let mut calls = HashMap::new();
                        run_prompt_with(&mut state, &selection, approval, &prompt, |event| {
                            update(&task_out, &task_session, event, &mut calls);
                        })
                        .await
                    }
                    .await;
                    let should_reply = task_active
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|item| item.request_id == task_id);
                    if should_reply {
                        task_active.lock().unwrap().take();
                        match result {
                            Ok(_) => reply(&task_out, task_id, json!({"stopReason":"end_turn"})),
                            Err(err) => error(&task_out, task_id, -32000, err.to_string()),
                        }
                    }
                });
                *slot = Some(ActivePrompt {
                    request_id: id,
                    session_id,
                    abort: task.abort_handle(),
                });
            }
            "session/cancel" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let taken = { active.lock().unwrap().take() };
                if let Some(item) = taken {
                    if item.session_id == session_id {
                        item.abort.abort();
                        reply(&out, item.request_id, json!({"stopReason":"cancelled"}));
                    } else {
                        *active.lock().unwrap() = Some(item);
                    }
                }
                if !id.is_null() {
                    reply(&out, id, json!({}));
                }
            }
            "_ax/status" => reply(
                &out,
                id,
                json!({"version":env!("CARGO_PKG_VERSION"),"protocolVersion":1,"active":active.lock().unwrap().is_some()}),
            ),
            "_ax/capabilities" => reply(
                &out,
                id,
                json!({
                    "sessions":true,
                    "resume":true,
                    "streaming":true,
                    "cancel":"abort_turn",
                    "permissions":true,
                    "providers":{
                        "supported": model::PROVIDERS.iter()
                            .filter(|spec| model::provider_supported(spec.id))
                            .map(|spec| spec.id)
                            .collect::<Vec<_>>(),
                        "unsupported": unsupported_providers(),
                    },
                }),
            ),
            "_ax/models" => {
                let configured = crate::providers::configured_providers(cli.codex_auth.as_ref());
                let providers = configured
                    .iter()
                    .map(|name| json!({"id":name,"models":model_selection::local_catalog_models(name).into_iter().filter(|item| item.supports_tools).collect::<Vec<_>>()}))
                    .collect::<Vec<_>>();
                reply(
                    &out,
                    id,
                    json!({
                        "providers":providers,
                        "catalog": provider_catalog(cli.codex_auth.as_ref()),
                        "unsupported": unsupported_configured_providers(&configured, cli.codex_auth.as_ref()),
                    }),
                );
            }
            // Discovery for one provider, performed on demand.
            //
            // `_ax/models` only reads the local cache, and a client that stores
            // a credential itself (Crew writes `~/.ax/auth.json` directly) had
            // no way to make AX populate `~/.ax/models/<provider>.json` — the
            // cache only appeared after someone ran the TUI picker. This is the
            // write side of that contract: run discovery, persist the cache,
            // and report why it is empty when it is.
            "_ax/refresh-models" => {
                let provider = params.get("provider").and_then(Value::as_str).unwrap_or("");
                if provider.is_empty() {
                    error(&out, id, -32602, "provider required");
                    continue;
                }
                match crate::tui::catalog_refresh::refresh_provider(
                    &data_dir,
                    cli.codex_auth.clone(),
                    provider,
                )
                .await
                {
                    Some(catalog) => reply(
                        &out,
                        id,
                        json!({
                            "provider": catalog.provider,
                            "source": match catalog.source {
                                model::CatalogSource::Live => "live",
                                model::CatalogSource::Cache => "cache",
                                model::CatalogSource::Fallback => "fallback",
                            },
                            "models": catalog.models.iter()
                                .map(|item| json!({"id":item.id,"display_name":item.display_name}))
                                .collect::<Vec<_>>(),
                            "warning": catalog.warning,
                        }),
                    ),
                    None => error(
                        &out,
                        id,
                        -32000,
                        format!("{provider} has no credential or no usable adapter"),
                    ),
                }
            }
            "_ax/skills" => {
                let global = crate::config::ax_home().join("skills");
                match skill::SkillCatalog::index_sources([&skills_dir, &global]) {
                    Ok(catalog) => {
                        let skills = catalog.statuses(std::iter::empty::<&str>()).into_iter()
                            .map(|status| json!({"name":status.metadata.name,"description":status.metadata.description,"missing_tools":status.missing_tools}))
                            .collect::<Vec<_>>();
                        reply(&out, id, json!({"skills":skills}));
                    }
                    Err(err) => error(&out, id, -32000, err.to_string()),
                }
            }
            "_ax/mcp" => {
                let path = mcp_config
                    .clone()
                    .unwrap_or_else(|| data_dir.join("mcp.toml"));
                match McpConfig::load(path) {
                    Ok(config) => {
                        let servers = config.servers.into_iter()
                            .map(|(name, server)| json!({"name":name,"description":server.description,"enabled":server.enabled,"capabilities":server.capabilities}))
                            .collect::<Vec<_>>();
                        reply(&out, id, json!({"servers":servers}));
                    }
                    Err(err) => error(&out, id, -32000, err.to_string()),
                }
            }
            "_ax/tools" => reply(
                &out,
                id,
                json!({"available":true,"source":"builtin","tools":builtin_tools()}),
            ),
            _ => {
                if !id.is_null() {
                    error(&out, id, -32601, "method not found");
                }
            }
        }
    }
    if let Some(item) = active.lock().unwrap().take() {
        item.abort.abort();
    }
    drop(out);
    writer.await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_keeps_the_update_and_adds_the_private_time() {
        let stamped = stamp(
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}),
            1_790_688_191,
        );
        assert_eq!(stamped["sessionUpdate"], "agent_message_chunk");
        assert_eq!(stamped["content"]["text"], "hi");
        assert_eq!(stamped["_ax"]["createdAt"], 1_790_688_191);
    }

    #[test]
    fn now_seconds_is_a_plausible_epoch_value() {
        assert!(now_seconds() > 1_700_000_000);
    }

    #[test]
    fn builtin_tool_catalog_is_sorted_and_named() {
        let catalog = builtin_tools();
        let names = catalog
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(names.len() >= 5, "expected the shipped tools: {names:?}");
        assert!(names.contains(&"shell".to_owned()));
        // A client renders this list as-is, so a stable order is part of the contract.
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(
            catalog
                .iter()
                .all(|tool| !tool["description"].as_str().unwrap_or_default().is_empty())
        );
    }
}
