use super::tools::{ModTool, WrappedTool};
use crate::{capabilities::Kind, repl::ReplState};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use runtime_core::{AgentError, ApprovalPolicy, ExtensionPrompt, RuntimeExtension};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use tool::{PermissionProfile, Tool, ToolError, ToolOutput, ToolRegistry};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    pending: bool,
}
impl Worker {
    async fn write(&mut self, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }
    async fn read(&mut self) -> Result<Value> {
        let mut line = String::new();
        if self.output.read_line(&mut line).await? == 0 {
            bail!("Mod process closed");
        }
        if line.len() > 4 * 1024 * 1024 {
            bail!("Mod response exceeds 4 MiB");
        }
        serde_json::from_str(&line).context("Invalid Mod bridge response")
    }
}
struct Inner {
    worker: Mutex<Option<Worker>>,
}
#[derive(Clone)]
struct Cached {
    inner: Arc<Inner>,
    custom_names: Arc<HashSet<String>>,
    specs: Vec<Value>,
    revision: String,
}
static SESSIONS: LazyLock<std::sync::Mutex<HashMap<String, Cached>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
pub(crate) fn close_session(id: &str) {
    SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(id);
}
pub(crate) async fn close_all() {
    let cached = SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain()
        .map(|(_, entry)| entry.inner)
        .collect::<Vec<_>>();
    for inner in cached {
        let mut guard = inner.worker.lock().await;
        if let Some(mut worker) = guard.take() {
            if !worker.pending {
                let _ = tokio::time::timeout(REQUEST_TIMEOUT, async {
                    worker
                        .write(&json!({"event":"session.end","input":{}}))
                        .await?;
                    worker.read().await
                })
                .await;
            }
            let _ = worker.child.kill().await;
            let _ = worker.child.wait().await;
        }
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        let Some(mut worker) = self.worker.get_mut().take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if !worker.pending {
                    let _ = tokio::time::timeout(REQUEST_TIMEOUT, async {
                        worker
                            .write(&json!({"event":"session.end","input":{}}))
                            .await?;
                        worker.read().await
                    })
                    .await;
                }
                let _ = worker.child.kill().await;
                let _ = worker.child.wait().await;
            });
        }
        // Without a runtime, Child::kill_on_drop still closes the process.
    }
}
#[derive(Clone)]
pub(crate) struct ModHost {
    inner: Arc<Inner>,
    registry: ToolRegistry,
    pub(super) approval: Arc<dyn ApprovalPolicy>,
    pub(super) profiles: Vec<PermissionProfile>,
    custom_names: Arc<HashSet<String>>,
}
impl ModHost {
    pub(crate) async fn load(
        state: &mut ReplState,
        registry: ToolRegistry,
        approval: Arc<dyn ApprovalPolicy>,
        model: &str,
    ) -> Result<Option<(Self, Vec<ModTool>)>> {
        let registry_mods = state.capability_registry(Kind::Mods)?;
        let mods = registry_mods.effective().map(|entry| json!({"name":entry.name,"root":entry.value.source,"entry":entry.value.data["entry"],"userConfig":entry.value.data["userConfig"]})).collect::<Vec<_>>();
        let id = state.current_session_id()?.to_owned();
        if mods.is_empty() {
            close_session(&id);
            return Ok(None);
        }
        if sandbox::SandboxManager::configured_mode()
            .is_some_and(|mode| mode != sandbox::SandboxMode::Off)
        {
            bail!("Node Mods require sandbox off: AX cannot confine arbitrary JavaScript modules");
        }
        if std::env::var_os("AX_SSH_CONTEXT").is_some()
            || std::env::var_os("AX_SSH_CONTEXT_FILE").is_some()
        {
            bail!(
                "Local Mods cannot run in an SSH execution context; install Mods on the remote AX host"
            );
        }
        let revision = super::revision(state)?;
        let cached = SESSIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .filter(|cached| cached.revision == revision)
            .cloned();
        if let Some(cached) = cached {
            let reusable = {
                let mut guard = cached.inner.worker.lock().await;
                guard.as_mut().is_some_and(|worker| {
                    !worker.pending && matches!(worker.child.try_wait(), Ok(None))
                })
            };
            if reusable {
                let host = Self {
                    inner: cached.inner,
                    custom_names: cached.custom_names,
                    registry,
                    approval,
                    profiles: vec![crate::config::AxConfig::load()?.permissions],
                };
                let tools = cached
                    .specs
                    .iter()
                    .map(|spec| ModTool::new(host.clone(), spec))
                    .collect::<Result<Vec<_>>>()?;
                return Ok(Some((host, tools)));
            }
        }
        close_session(&id);
        Self::spawn(state, registry, approval, model, mods, id, revision).await
    }

    async fn spawn(
        state: &mut ReplState,
        registry: ToolRegistry,
        approval: Arc<dyn ApprovalPolicy>,
        model: &str,
        mods: Vec<Value>,
        id: String,
        revision: String,
    ) -> Result<Option<(Self, Vec<ModTool>)>> {
        let mut command =
            Command::new(std::env::var_os("AX_MOD_NODE").unwrap_or_else(|| "node".into()));
        command
            .args(["--input-type=module", "-e", include_str!("worker.mjs")])
            .current_dir(&state.project_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command
            .spawn()
            .context("Mods require Node.js 20.6+ on PATH (or AX_MOD_NODE)")?;
        let input = child.stdin.take().context("Missing Mod stdin")?;
        let output = BufReader::new(child.stdout.take().context("Missing Mod stdout")?);
        let mut host = Self {
            inner: Arc::new(Inner {
                worker: Mutex::new(Some(Worker {
                    child,
                    input,
                    output,
                    pending: false,
                })),
            }),
            registry,
            approval,
            profiles: vec![crate::config::AxConfig::load()?.permissions],
            custom_names: Arc::default(),
        };
        let result = host.request("initialize",json!({"mods":mods,"home":state.capability_home,"session":{"id":state.current_session_id()?,"cwd":state.project_root,"model":model}}),None,&host.profiles).await?;
        host.custom_names = Arc::new(
            result["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|spec| spec["name"].as_str().map(str::to_owned))
                .collect(),
        );
        let tools = result["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|spec| ModTool::new(host.clone(), spec))
            .collect::<Result<Vec<_>>>()?;
        SESSIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id,
                Cached {
                    inner: host.inner.clone(),
                    custom_names: host.custom_names.clone(),
                    specs: result["tools"].as_array().cloned().unwrap_or_default(),
                    revision,
                },
            );
        Ok(Some((host, tools)))
    }

    pub(super) async fn request(
        &self,
        event: &str,
        input: Value,
        next: Option<Arc<dyn Tool>>,
        profiles: &[PermissionProfile],
    ) -> Result<Value> {
        let mut guard = self.inner.worker.lock().await;
        let worker = guard.as_mut().context("Mod process is unavailable")?;
        if worker.pending {
            worker.child.kill().await?;
            bail!("Previous Mod event was cancelled; start a new session to reload Mods");
        }
        worker.pending = true;
        let result = tokio::time::timeout(REQUEST_TIMEOUT, async {
            worker.write(&json!({"event":event,"input":input})).await?;
            loop {
                let reply = worker.read().await?;
                if let Some(call) = reply["call"].as_str() {
                    let result = self
                        .callback(call, &reply["input"], &input, next.as_ref(), profiles)
                        .await;
                    let response = match result {
                        Ok(value) => json!({"reply":reply["callId"],"result":value}),
                        Err(error) => json!({"reply":reply["callId"],"error":error.to_string()}),
                    };
                    worker.write(&response).await?;
                    continue;
                }
                if let Some(error) = reply["error"].as_str() {
                    bail!("{error}");
                }
                return Ok(reply["result"].clone());
            }
        })
        .await;
        match result {
            Ok(Ok(value)) => {
                worker.pending = false;
                Ok(value)
            }
            Ok(Err(error)) => {
                let _ = worker.child.kill().await;
                guard.take();
                Err(error)
            }
            Err(_) => {
                worker.child.kill().await?;
                bail!("Mod event exceeded 15 seconds; start a new session to reload Mods")
            }
        }
    }
    async fn callback(
        &self,
        call: &str,
        requested: &Value,
        original: &Value,
        next: Option<&Arc<dyn Tool>>,
        profiles: &[PermissionProfile],
    ) -> Result<Value> {
        let name = requested["tool"]
            .as_str()
            .context("Missing Mod tool name")?;
        let mut arguments = requested.clone();
        arguments
            .as_object_mut()
            .context("Tool input must be an object")?
            .remove("tool");
        let tool = if call == "next" {
            if requested != original {
                bail!(
                    "Mods cannot change an approved tool's name or arguments; use $.tool.call for a new permission check"
                );
            }
            next.cloned()
                .with_context(|| format!("No implementation for Mod tool {name}"))?
        } else if call == "tool" {
            let tool = self
                .registry
                .get(name)
                .with_context(|| format!("Unknown Mod host tool: {name}"))?;
            let permission = tool.permission(&arguments);
            let decisions = profiles
                .iter()
                .map(|p| {
                    p.decision(
                        name,
                        &arguments,
                        permission.capability,
                        &tool.resources(&arguments),
                    )
                })
                .collect::<Vec<_>>();
            let outcome = tool::resolve_profiles(&decisions);
            if outcome == tool::ProfileDecision::Deny
                || self.approval.capability_decision(permission.capability)
                    == Some(tool::PermissionDecision::Deny)
            {
                bail!("Permission denied for Mod host tool {name}");
            }
            let allowed = match outcome {
                tool::ProfileDecision::Allowed => true,
                tool::ProfileDecision::Ask => self.approval.ask(name, &arguments, permission).await,
                _ => self.approval.approve(name, &arguments, permission).await,
            };
            if !allowed {
                bail!("Permission denied for Mod host tool {name}");
            }
            tool
        } else {
            bail!("Unknown Mod host operation: {call}");
        };
        Ok(
            match tool.execute_output_constrained(arguments, profiles).await {
                Ok(ToolOutput::Text(result)) => json!({"result":result,"isError":false}),
                Ok(ToolOutput::Image {
                    description,
                    media_type,
                    data,
                }) => {
                    json!({"result":description,"isError":false,"image":{"description":description,"media_type":media_type,"data":data}})
                }
                Err(error) => json!({"result":error.to_string(),"isError":true}),
            },
        )
    }

    pub(super) async fn tool_call(
        &self,
        name: &str,
        mut input: Value,
        next: Option<Arc<dyn Tool>>,
        profiles: &[PermissionProfile],
    ) -> Result<ToolOutput, ToolError> {
        input
            .as_object_mut()
            .ok_or_else(|| ToolError::InvalidInput("Mod tool input must be an object".into()))?
            .insert("tool".into(), json!(name));
        let result = self
            .request("tool.call", input, next, profiles)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        if let Some(deny) = result["deny"].as_str() {
            return Err(ToolError::PermissionDenied(deny.into()));
        }
        if result["isError"] == true {
            return Err(ToolError::Execution(
                result["result"]
                    .as_str()
                    .unwrap_or("Mod tool failed")
                    .into(),
            ));
        }
        if let Some(image) = result.get("image") {
            return Ok(ToolOutput::Image {
                description: image["description"].as_str().unwrap_or_default().into(),
                media_type: image["media_type"].as_str().unwrap_or_default().into(),
                data: image["data"].as_str().unwrap_or_default().into(),
            });
        }
        let output = result
            .get("result")
            .context("Mod tool hook must return {result} or call next")
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(ToolOutput::Text(
            output
                .as_str()
                .map_or_else(|| output.to_string(), str::to_owned),
        ))
    }
}
#[async_trait]
impl RuntimeExtension for ModHost {
    async fn sync_context(
        &self,
        messages: &[model::Message],
        window: usize,
    ) -> Result<(), AgentError> {
        self.request(
            "context.update",
            json!({"messages":messages,"window":window}),
            None,
            &self.profiles,
        )
        .await
        .map_err(|error| AgentError::WorkerJoin(error.to_string()))?;
        Ok(())
    }
    async fn before_turn(&self, input: &str) -> Result<ExtensionPrompt, AgentError> {
        let result = self
            .request("prompt.submit", json!({"text":input}), None, &self.profiles)
            .await
            .map_err(|error| AgentError::WorkerJoin(error.to_string()))?;
        Ok(ExtensionPrompt {
            text: result["text"].as_str().unwrap_or(input).into(),
            context: result["context"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            response: result["response"].as_str().map(str::to_owned),
        })
    }
    async fn after_turn(
        &self,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), AgentError> {
        self.request(
            "turn.complete",
            json!({"text":output,"error":error}),
            None,
            &self.profiles,
        )
        .await
        .map_err(|error| AgentError::WorkerJoin(error.to_string()))?;
        Ok(())
    }
    fn wrap_tool(&self, tool: Arc<dyn Tool>) -> Arc<dyn Tool> {
        if self.custom_names.contains(tool.name()) {
            tool
        } else {
            Arc::new(WrappedTool {
                host: self.clone(),
                inner: tool,
            })
        }
    }
}
