//! ACP v1 stdio adapter for the existing AX composition root.
//! JSON-RPC is newline framed; no model, tool, session, or memory logic lives here.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use mcp::{CURRENT_PROTOCOL_VERSION, McpConfig, ServerConfig, TransportConfig};
use memory::MessageRole;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{args::Cli, commands::run::run_prompt_with, model_selection, repl::ReplState};
use runtime_core::{AgentEvent, ApprovalPolicy};
use tool::{Capability, PermissionDecision, PermissionStore, SafetyLevel, ToolPermission};

type Outbox = mpsc::UnboundedSender<Value>;
type PendingPermissions = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

struct ActivePrompt {
    request_id: Value,
    session_id: String,
    task: JoinHandle<()>,
    input: runtime_core::TurnInput,
}

fn cancel_saved_goal(state: &mut ReplState, session_id: &str) -> Result<()> {
    let Some(saved) = state
        .store()?
        .latest_agent_state(session_id, runtime_core::task_queue::STATE_PREFIX)?
    else {
        return Ok(());
    };
    let mut queue: runtime_core::task_queue::TaskQueue = serde_json::from_str(
        saved
            .content
            .strip_prefix(runtime_core::task_queue::STATE_PREFIX)
            .unwrap_or_default(),
    )?;
    queue.normalize_legacy();
    queue.stop(
        runtime_core::QueueState::Cancelled,
        "Task queue canceled by user.".into(),
    );
    queue
        .final_response
        .get_or_insert_with(|| "Task queue canceled by user.".into());
    let message = queue.snapshot();
    state.current_session = state.store()?.session(session_id)?;
    state.persist_messages(&[message])
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
        self.request(name, input, permission, true).await
    }
    fn capability_decision(&self, capability: tool::Capability) -> Option<PermissionDecision> {
        Some(self.permissions.decision(capability))
    }
    async fn ask(&self, name: &str, input: &Value, permission: tool::ToolPermission) -> bool {
        self.request(name, input, permission, false).await
    }
}

impl AcpApproval {
    async fn request(
        &self,
        name: &str,
        input: &Value,
        permission: tool::ToolPermission,
        grant_session: bool,
    ) -> bool {
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
                if grant_session {
                    self.permissions.allow_session(permission.capability);
                }
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
    let mut tools = crate::runtime::tools(&[])
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
        let meta = map.entry("_ax").or_insert_with(|| json!({}));
        meta["createdAt"] = json!(at);
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
        AgentEvent::ToolStarted {
            id,
            name,
            detail,
            input,
        } => {
            calls.insert(id.clone(), id.clone());
            json!({"sessionUpdate":"tool_call","toolCallId":id,"title":detail,"kind":name,"status":"pending","rawInput":{"name":name,"arguments":input}})
        }
        AgentEvent::ToolFinished {
            id,
            success,
            diagnostics,
            result,
            ..
        } => {
            let mut update = json!({"sessionUpdate":"tool_call_update","toolCallId":calls.remove(&id).unwrap_or(id),"status":if success {"completed"} else {"failed"},"rawOutput":result});
            if !diagnostics.is_empty() {
                update["rawOutput"]["errors"] = json!(diagnostics);
            }
            update
        }
        AgentEvent::UserQuestion { question } => {
            // Planning input, not an authorization gate: the client answers
            // with an option id or free text and the same prompt run resumes.
            json!({
                "sessionUpdate":"agent_question",
                "question": {
                    "id": question.id,
                    "text": question.question,
                    "allowFreeText": question.allow_free_text,
                    "options": question.options.iter().map(|option| json!({
                        "id": option.id,
                        "label": option.label,
                        "description": option.description,
                    })).collect::<Vec<_>>(),
                }
            })
        }
        AgentEvent::Continuation { .. }
        | AgentEvent::Completion { .. }
        | AgentEvent::StopGuardEvaluated { .. }
        | AgentEvent::SubagentStarted { .. }
        | AgentEvent::SubagentProgress { .. }
        | AgentEvent::SubagentCompleted { .. }
        | AgentEvent::SubagentFailed { .. }
        | AgentEvent::SubagentCancelled { .. }
        | AgentEvent::ModelStarted { .. }
        | AgentEvent::ContextCompressed { .. }
        | AgentEvent::TurnStarted
        | AgentEvent::TurnFinished => return,
    };
    let _ = out.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":stamp(body, now_seconds())}}));
}

/// ACP adapter uses the same prompt composition and Agent Loop as CLI/TUI.
pub(super) async fn run_session_prompt(
    state: &mut ReplState,
    selection: &crate::model_selection::ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    prompt: &str,
    out: &Outbox,
    session_id: &str,
) -> Result<String> {
    let mut calls = HashMap::new();
    run_prompt_with(state, selection, approval, prompt, |event| {
        update(out, session_id, event, &mut calls);
    })
    .await
}

/// Every catalog provider AX cannot drive, with the reason. Clients render
/// these as unsupported rather than offering a credential dialog that would
/// silently do nothing.
fn provider_catalog(codex_auth: Option<&PathBuf>) -> Vec<Value> {
    let credentialed = crate::providers::credentialed_providers(codex_auth);
    let stored = model::AuthStorage::new(crate::bootstrap::ax_auth_path())
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
            "auth_kind":match spec.auth {
                model::ProviderAuthKind::ApiKey => "api_key",
                model::ProviderAuthKind::CodexOAuth | model::ProviderAuthKind::ExternalOAuth => "oauth",
                model::ProviderAuthKind::Ambient => "ambient",
            },
            "unsupported_reason":model::provider_unsupported_reason(spec.id),
            "configuration_reason":model::provider_configuration_reason(spec.id),
            "source":if !configured { None } else if stored.iter().any(|id| id == spec.id) { Some("AX") } else { Some("environment") },
            "model_source": if models.is_empty() { "none" } else if crate::bootstrap::ax_models_dir().join(format!("{}.json", spec.id)).is_file() { "cache" } else { "fallback" },
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
    let mut config = state.effective_mcp_config()?;
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
    replay_store(out, session_id, session_id, state.store()?, false)
}

fn replay_store(
    out: &Outbox,
    session_id: &str,
    source_session: &str,
    store: &mut memory::MemoryStore,
    tools_only: bool,
) -> Result<()> {
    let mut pages = Vec::new();
    let mut before = None;
    loop {
        let page = store.load_messages(source_session, before, 128)?;
        if page.is_empty() {
            break;
        }
        before = page.first().map(|item| item.id);
        pages.push(page);
    }
    let mut children = HashSet::new();
    let call_id = |id: &str| {
        if tools_only {
            format!("{source_session}:{id}")
        } else {
            id.to_owned()
        }
    };
    for message in pages.into_iter().rev().flatten() {
        let update = match message.role {
            MessageRole::User => {
                if tools_only {
                    continue;
                }
                json!({"sessionUpdate":"user_message_chunk","messageId":message.metadata.pointer("/provider_metadata/axMessageId").and_then(Value::as_str).map_or_else(||message.id.to_string(),str::to_owned),"content":{"type":"text","text":message.content},"_ax":{"steering":message.metadata.pointer("/provider_metadata/axSteering").and_then(Value::as_bool).unwrap_or(false)}})
            }
            MessageRole::Assistant => {
                if let Some(calls) = message.metadata.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let name = call["function"]["name"].as_str().unwrap_or("tool");
                        let input = call["function"]["arguments"]
                            .as_str()
                            .and_then(|text| serde_json::from_str::<Value>(text).ok())
                            .unwrap_or(Value::Null);
                        let detail = runtime_core::tool_activity(name, &input);
                        let start = json!({"sessionUpdate":"tool_call","toolCallId":call_id(call["id"].as_str().unwrap_or("unknown")),"title":detail,"kind":name,"rawInput":{"name":name,"arguments":input},"status":"pending"});
                        out.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":stamp(start,message.created_at)}})).ok();
                    }
                }
                if tools_only {
                    continue;
                }
                json!({"sessionUpdate":"agent_message_chunk","messageId":message.id.to_string(),"content":{"type":"text","text":message.content}})
            }
            MessageRole::Tool => {
                let result = serde_json::from_str::<tool::ToolResult>(&message.content)
                    .unwrap_or_else(|_| tool::ToolResult::from_legacy(message.content.clone()));
                json!({"sessionUpdate":"tool_call_update","toolCallId":call_id(message.metadata.get("tool_call_id").and_then(Value::as_str).unwrap_or("unknown")),"status":if result.status=="success" {"completed"} else {"failed"},"rawOutput":result})
            }
            MessageRole::System => {
                if !tools_only {
                    let queue = message
                        .content
                        .strip_prefix(runtime_core::task_queue::STATE_PREFIX)
                        .or_else(|| {
                            message
                                .content
                                .strip_prefix(runtime_core::task_queue::ARCHIVE_PREFIX)
                        })
                        .and_then(|data| {
                            serde_json::from_str::<runtime_core::task_queue::TaskQueue>(data).ok()
                        });
                    if let Some(queue) = queue {
                        for child in queue.tasks.iter().filter_map(|task| task.child.as_ref()) {
                            if !children.insert(child.session_id.clone()) {
                                continue;
                            }
                            let database = child.state_dir.as_ref().map_or_else(
                                || child.cwd.join(".ax").join("child.sqlite3"),
                                |state| state.join("child.sqlite3"),
                            );
                            // Never create a missing child store while viewing history.
                            if database.is_file() {
                                let mut child_store = memory::MemoryStore::open(&database)?;
                                replay_store(
                                    out,
                                    session_id,
                                    &child.session_id,
                                    &mut child_store,
                                    true,
                                )?;
                            }
                        }
                        continue;
                    }
                }
                if tools_only {
                    continue;
                }
                let Some(data) = message.content.strip_prefix("[ax-changes]\n") else {
                    continue;
                };
                json!({"sessionUpdate":"turn_changes","changedFiles":serde_json::from_str::<Value>(data).unwrap_or(Value::Null)})
            }
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
            "_ax/workspace" => {
                let boundary =
                    (cli.sandbox == Some(sandbox::SandboxMode::Strict)).then_some(cwd.as_path());
                match crate::acp_workspace::listing(params, boundary) {
                    Ok(value) => reply(&out, id, value),
                    Err(err) => error(&out, id, -32602, err.to_string()),
                }
            }
            "session/delete" => {
                let session_id = params["sessionId"].as_str().unwrap_or("");
                if uuid::Uuid::parse_str(session_id).is_err() || active.lock().unwrap().is_some() {
                    error(&out, id, -32602, "valid idle session required");
                    continue;
                }
                match ReplState::new_in_project(
                    data_dir.clone(),
                    skills_dir.clone(),
                    mcp_config.clone(),
                    &cwd,
                )
                .and_then(|mut state| Ok(state.store()?.delete_session(session_id)?))
                {
                    Ok(deleted) => {
                        crate::mods::close_session(session_id);
                        reply(&out, id, json!({"deleted":deleted}));
                    }
                    Err(err) => error(&out, id, -32603, err.to_string()),
                }
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
                        state.prepare_runtime(&selection)?;
                        let budget = state.context_budget(&selection);
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
                let goal_turn = match params.pointer("/_meta/axGoal") {
                    Some(value) => {
                        match serde_json::from_value::<runtime_core::GoalTurn>(value.clone()) {
                            Ok(intent) => intent,
                            Err(error_message) => {
                                error(&out, id, -32602, format!("invalid axGoal: {error_message}"));
                                continue;
                            }
                        }
                    }
                    None => runtime_core::GoalTurn::New,
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
                let task_budget = crate::runtime::execution_budget(cli);
                let task_child_timeout = cli.child_timeout_secs;
                let input = runtime_core::TurnInput::default();
                let task_input = input.clone();
                let task = tokio::spawn(async move {
                    let result: Result<(String, bool)> = async {
                        let mut state =
                            ReplState::new_in_project(task_data, task_skills, task_mcp, &task_cwd)?;
                        state.mcp_override = task_override;
                        apply_mcp_names(&mut state, task_mcp_names.as_ref())?;
                        state.allowed_skills = task_skills_allowed;
                        state.next_goal_turn = goal_turn;
                        state.execution_budget = task_budget;
                        state.child_timeout_secs = task_child_timeout;
                        state.prepare_runtime(&selection)?;
                        let budget = state.context_budget(&selection);
                        if !state.open_session(&task_session, &budget)? {
                            return Err(anyhow!("AX session not found"));
                        }
                        state.turn_input = Some(task_input);
                        apply_permission_profile(&state.permissions, task_profile.as_deref());
                        let approval: Arc<dyn ApprovalPolicy> = Arc::new(AcpApproval {
                            out: task_out.clone(),
                            pending: task_pending,
                            permissions: state.permissions.clone(),
                            session_id: task_session.clone(),
                        });
                        let before=crate::worktree_changes::snapshot(&task_cwd);
                        let outcome=run_session_prompt(&mut state, &selection, approval, &prompt, &task_out, &task_session)
                        .await;
                        let files = match (before, crate::worktree_changes::snapshot(&task_cwd)) {
                            (Ok(before), Ok(after)) => Some(crate::worktree_changes::changed(&before, &after)),
                            (Err(error), _) | (_, Err(error)) => {
                                // A failed snapshot is not an unchanged workspace.
                                state.persist_messages(&[model::Message::system(format!(
                                    "[ax-changes]\nworkspace snapshot failed: {error}"
                                ))])?;
                                None
                            }
                        };
                        if let Some(files) = files {
                            state.persist_messages(&[model::Message::system(format!("[ax-changes]\n{}",json!(files)))])?;
                            let body=stamp(json!({"sessionUpdate":"turn_changes","changedFiles":files}),now_seconds());
                            task_out.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":task_session,"update":body}})).ok();
                        }
                        outcome.map(|_| (state.runtime.as_ref().and_then(runtime_core::AgentKernel::goal_id).unwrap_or_default().to_owned(),
                            state.runtime.as_ref().is_some_and(|kernel| kernel.pending_question().is_some())))
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
                            Ok((goal_id, waiting_for_user)) => reply(
                                &task_out,
                                task_id,
                                json!({"stopReason":"end_turn","_meta":{"axGoal":{"goal_id":goal_id,"waiting_for_user":waiting_for_user}}}),
                            ),
                            Err(err) => error(&task_out, task_id, -32000, err.to_string()),
                        }
                    }
                });
                *slot = Some(ActivePrompt {
                    request_id: id,
                    session_id,
                    task,
                    input,
                });
            }
            "_ax/steer" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let prompt = match prompt_text(params) {
                    Ok(text) if !text.trim().is_empty() => text,
                    _ => {
                        error(&out, id, -32602, "nonempty prompt required");
                        continue;
                    }
                };
                let slot = active.lock().unwrap();
                let accepted = slot
                    .as_ref()
                    .filter(|turn| turn.session_id == session_id)
                    .and_then(|turn| turn.input.try_steer(prompt.clone()));
                if let Some(message_id) = accepted {
                    let update = stamp(
                        json!({"sessionUpdate":"user_message_chunk","messageId":message_id,"content":{"type":"text","text":prompt},"_ax":{"steering":true}}),
                        now_seconds(),
                    );
                    out.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session_id,"update":update}})).ok();
                    reply(&out, id, json!({"accepted":true,"messageId":message_id}));
                } else {
                    error(
                        &out,
                        id,
                        -32002,
                        "this session has no active turn accepting guidance",
                    );
                }
            }
            "session/cancel" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let taken = { active.lock().unwrap().take() };
                if let Some(item) = taken {
                    if item.session_id == session_id {
                        item.task.abort();
                        let _ = item.task.await;
                        // Abort has completed before writing cancellation, so a
                        // late checkpoint cannot resurrect the active queue.
                        let cancellation: Result<()> = (|| {
                            let mut state = ReplState::new_in_project(
                                data_dir.clone(),
                                skills_dir.clone(),
                                mcp_config.clone(),
                                &cwd,
                            )?;
                            cancel_saved_goal(&mut state, session_id)
                        })();
                        if let Err(err) = cancellation {
                            error(&out, item.request_id, -32000, err.to_string());
                        } else {
                            reply(&out, item.request_id, json!({"stopReason":"cancelled"}));
                        }
                    } else {
                        *active.lock().unwrap() = Some(item);
                    }
                }
                if !id.is_null() {
                    reply(&out, id, json!({}));
                }
            }
            "_ax/capabilities" => reply(
                &out,
                id,
                json!({
                    "scopedCapabilities":{"method":"_ax/scopedCapabilities","scopes":["global","project"],"kinds":["skills","mcp","agents","mods"],"actions":["list","enable","disable","add","remove"]},
                    "sessions":true,
                    "resume":true,
                    "streaming":true,
                    "cancel":"abort_turn",
                    "steering":{"method":"_ax/steer","scope":"active_turn","interrupts":false},
                    "goals":{"promptMetadata":"_meta.axGoal","actions":["new","start","resume","cancel"]},
                    "permissions":true,
                    "workspace":{"method":"_ax/workspace","readOnly":true},
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
            "_ax/skills" | "_ax/mcp" | "_ax/agents" | "_ax/mods" | "_ax/scopedCapabilities" => {
                let result = (|| -> Result<Value> {
                    let root = crate::bootstrap::discover_project_root(&std::env::current_dir()?);
                    let mut state = ReplState::new_in_project(
                        data_dir.clone(),
                        skills_dir.clone(),
                        mcp_config.clone(),
                        &root,
                    )?;
                    let kind = crate::capabilities::Kind::parse(
                        params
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or(match method {
                                "_ax/skills" => "skills",
                                "_ax/mcp" => "mcp",
                                "_ax/mods" => "mods",
                                _ => "agents",
                            }),
                    )?;
                    let scope = params
                        .get("scope")
                        .and_then(Value::as_str)
                        .map(crate::capabilities::parse_scope)
                        .transpose()?;
                    if let Some(action) = params
                        .get("action")
                        .and_then(Value::as_str)
                        .filter(|action| *action != "list")
                    {
                        let scope = scope.context("mutation requires explicit scope")?;
                        let name = params
                            .get("name")
                            .and_then(Value::as_str)
                            .context("name is required")?;
                        let source = params.get("source").and_then(Value::as_str).map(Path::new);
                        state.manage_capability(kind, scope, action, name, source)?;
                    }
                    let rows = state.capability_rows(kind, scope)?;
                    Ok(
                        json!({"items": rows, kind.key(): rows, "servers": if kind == crate::capabilities::Kind::Mcp { rows } else { Vec::new() }, "project_id":state.project_id}),
                    )
                })();
                match result {
                    Ok(value) => reply(&out, id, value),
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
    let disconnected = active.lock().unwrap().take();
    if let Some(item) = disconnected {
        item.task.abort();
        let _ = item.task.await;
    }
    drop(out);
    writer.await??;
    crate::mods::close_all().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_persists_terminal_goal_across_reconnect() {
        let root = std::env::temp_dir().join(format!(
            "ax-goal-cancel-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let data = root.join("data");
        let skills = root.join("skills");
        let mut state =
            ReplState::new_in_project(data.clone(), skills.clone(), None, &root).unwrap();
        state.create_session("controller").unwrap();
        let session_id = state.current_session.as_ref().unwrap().id.clone();
        let queue: runtime_core::task_queue::TaskQueue = serde_json::from_value(json!({
            "goal_id":"controller-goal", "state":"active", "overall_goal":"benchmark",
            "tasks":[
                {"title":"worker 1","status":"running","failure_reason":null,"outcome":null},
                {"title":"worker 2","status":"pending","failure_reason":null,"outcome":null}
            ], "summarized":false, "stop_reason":null
        }))
        .unwrap();
        state.persist_messages(&[queue.snapshot()]).unwrap();
        cancel_saved_goal(&mut state, &session_id).unwrap();
        drop(state);
        let mut restored = ReplState::new_in_project(data, skills, None, &root).unwrap();
        let saved = restored
            .store()
            .unwrap()
            .latest_agent_state(&session_id, runtime_core::task_queue::STATE_PREFIX)
            .unwrap()
            .unwrap();
        let queue: runtime_core::task_queue::TaskQueue = serde_json::from_str(
            saved
                .content
                .strip_prefix(runtime_core::task_queue::STATE_PREFIX)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(queue.goal_id, "controller-goal");
        assert_eq!(queue.state, runtime_core::QueueState::Cancelled);
        assert_eq!(
            queue.final_response.as_deref(),
            Some("Task queue canceled by user.")
        );
        assert_eq!(
            queue.tasks[1].status,
            runtime_core::task_queue::TaskStatus::Pending
        );
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cancelled_controller_replays_child_tools_once_across_history_pages() {
        let root = std::env::temp_dir().join(format!("ax-child-replay-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut parent = memory::MemoryStore::open(root.join("parent.sqlite3")).unwrap();
        let controller = parent.create_session("controller").unwrap().id;
        parent
            .append_message(
                &controller,
                memory::NewMessage::text(MessageRole::User, "goal"),
            )
            .unwrap();
        let mut tasks = Vec::new();
        let mut expected = Vec::new();
        for index in 0..2 {
            let state_dir = root.join(format!("child-{index}"));
            std::fs::create_dir_all(&state_dir).unwrap();
            let mut store = memory::MemoryStore::open(state_dir.join("child.sqlite3")).unwrap();
            let session = store.create_session("worker").unwrap().id;
            store
                .append_message(
                    &session,
                    memory::NewMessage::text(MessageRole::User, "internal prompt"),
                )
                .unwrap();
            // Cross the replay page boundary; workers deliberately reuse call IDs.
            for call in 0..65 {
                let id = format!("call-{call}");
                expected.push(format!("{session}:{id}"));
                store.append_message(&session, memory::NewMessage {
                    role: MessageRole::Assistant, kind: memory::MessageKind::ToolCall,
                    content: "internal narration".into(),
                    metadata: json!({"tool_calls":[{"id":id,"function":{"name":"shell","arguments":"{\"command\":\"echo kept\"}"}}]}),
                }).unwrap();
                if call < 64 {
                    store
                        .append_message(
                            &session,
                            memory::NewMessage {
                                role: MessageRole::Tool,
                                kind: memory::MessageKind::ToolCall,
                                content: serde_json::to_string(&tool::ToolResult::new(
                                    true,
                                    "kept output".into(),
                                ))
                                .unwrap(),
                                metadata: json!({"tool_call_id":id}),
                            },
                        )
                        .unwrap();
                }
            }
            tasks.push(json!({"title":"worker","status":"running","failure_reason":null,"outcome":null,
                "child":{"goal_id":"goal","session_id":session,"cwd":root.join("deleted-workspace"),"state_dir":state_dir,"memory_scope":"worker"}}));
        }
        let queue: runtime_core::task_queue::TaskQueue = serde_json::from_value(json!({
            "goal_id":"goal","state":"cancelled","overall_goal":"goal","tasks":tasks,"summarized":false,"stop_reason":"cancelled"
        })).unwrap();
        // Repeated queue checkpoints and archives must not duplicate child tools.
        for prefix in [
            runtime_core::task_queue::STATE_PREFIX,
            runtime_core::task_queue::ARCHIVE_PREFIX,
        ] {
            parent
                .append_message(
                    &controller,
                    memory::NewMessage::text(
                        MessageRole::System,
                        format!("{prefix}{}", serde_json::to_string(&queue).unwrap()),
                    ),
                )
                .unwrap();
        }
        let (out, mut rx) = mpsc::unbounded_channel();
        replay_store(&out, &controller, &controller, &mut parent, false).unwrap();
        let mut updates = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            assert_eq!(frame["params"]["sessionId"], controller);
            updates.push(frame["params"]["update"].clone());
        }
        let starts: Vec<_> = updates
            .iter()
            .filter(|u| u["sessionUpdate"] == "tool_call")
            .map(|u| u["toolCallId"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(starts, expected);
        let results: Vec<_> = updates
            .iter()
            .filter(|u| u["sessionUpdate"] == "tool_call_update")
            .collect();
        assert_eq!(results.len(), 128);
        assert!(
            results
                .iter()
                .all(|u| u["rawOutput"]["raw_output"] == "kept output")
        );
        assert_eq!(
            updates
                .iter()
                .filter(|u| u["sessionUpdate"] == "user_message_chunk")
                .count(),
            1
        );
        assert!(
            !updates
                .iter()
                .any(|u| u["sessionUpdate"] == "agent_message_chunk")
        );
        drop(parent);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_tool_update_keeps_call_id_and_structured_failure() {
        let (out, mut rx) = mpsc::unbounded_channel();
        let mut calls = HashMap::new();
        update(
            &out,
            "session",
            AgentEvent::ToolStarted {
                id: "call-1".into(),
                name: "shell".into(),
                detail: "running exit 1".into(),
                input: json!({"command":"exit 1"}),
            },
            &mut calls,
        );
        update(
            &out,
            "session",
            AgentEvent::ToolFinished {
                id: "call-1".into(),
                name: "shell".into(),
                success: false,
                diagnostics: vec![],
                result: tool::ToolResult::new(false, "exit_code: 1\nerror: failure".into()),
            },
            &mut calls,
        );
        let start = rx.try_recv().unwrap();
        let finish = rx.try_recv().unwrap();
        assert_eq!(
            start["params"]["update"]["toolCallId"],
            finish["params"]["update"]["toolCallId"]
        );
        assert_eq!(start["params"]["update"]["kind"], "shell");
        assert_eq!(finish["params"]["update"]["status"], "failed");
        assert_eq!(finish["params"]["update"]["rawOutput"]["status"], "error");
    }

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
