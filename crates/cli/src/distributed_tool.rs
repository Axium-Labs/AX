//! The ordinary Tool seam supplies remote results to the existing agent loop.
use crate::distributed_client::Client;
use anyhow::{Result, ensure};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tool::{Capability, ExecutionBoundary, RunContext, SafetyLevel, Tool, ToolError};

#[derive(Clone)]
pub(crate) struct CollaborationTool {
    client: Client,
    root: PathBuf,
    parent: Option<String>,
    generation: Option<u64>,
    incarnation: String,
    project: String,
    workflow: String,
}
impl CollaborationTool {
    pub(crate) fn from_env(root: PathBuf) -> Option<Self> {
        Some(Self {
            client: Client::from_env()?,
            root,
            parent: std::env::var("AX_DISTRIBUTED_TASK").ok(),
            generation: std::env::var("AX_DISTRIBUTED_GENERATION")
                .ok()
                .and_then(|s| s.parse().ok()),
            incarnation: std::env::var("AX_DISTRIBUTED_INCARNATION").unwrap_or_default(),
            project: std::env::var("AX_DISTRIBUTED_PROJECT").unwrap_or_default(),
            workflow: std::env::var("AX_DISTRIBUTED_WORKFLOW").unwrap_or_default(),
        })
    }
    #[allow(clippy::too_many_lines)]
    async fn call(&self, input: Value) -> Result<String> {
        let action = input["action"].as_str().unwrap_or("");
        let required = |field: &str| -> Result<String> {
            Ok(input[field]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{field} required"))?
                .to_owned())
        };
        let value = match action {
            "catalog" | "status" => {
                if action=="catalog" { self.client.request("GET","/api/distributed/catalog",None).await? }
                else if let Some(id)=input["task_id"].as_str() {
                    ensure!(id.chars().all(|c|c.is_ascii_alphanumeric()||c=='-'),"invalid task identity");
                    self.client.request("GET",&format!("/api/distributed/tasks/{id}?after_sequence={}",input["after_sequence"].as_u64().unwrap_or(0)),None).await?
                } else if !self.workflow.is_empty() { self.client.request("GET",&format!("/api/distributed/workflows/{}",self.workflow),None).await? }
                else { self.client.request("GET","/api/distributed",None).await? }
            }
            "delegate" => {
                let mut spec = input["task"].clone();
                ensure!(spec.is_object(), "task object required");
                if spec["project_id"].is_null() { spec["project_id"] = json!(self.project); }
                spec["parent_id"] = json!(self.parent); spec["parent_generation"] = json!(self.generation);
                if spec["workflow_id"].is_null() && !self.workflow.is_empty() { spec["workflow_id"]=json!(self.workflow); }
                if spec["request_id"].is_null() {
                    let mut identity=spec.clone();identity["parent_generation"]=Value::Null;
                    spec["request_id"] = json!(format!("{:x}",sha2::Sha256::digest(identity.to_string().as_bytes())));
                }
                self.client.request("POST", "/api/distributed/tasks", Some(spec)).await?
            }
            "checkpoint_workflow" => {
                ensure!(!self.workflow.is_empty(),"workflow binding required");
                self.client.request("POST",&format!("/api/distributed/workflows/{}/checkpoint",self.workflow),Some(json!({
                    "expected_revision":input["expected_revision"],"state":input["state"],"source":{"incarnation":self.incarnation,
                    "task_id":self.parent,"generation":self.generation,"message_id":"checkpoint","kind":"observation","text":""}}))).await?
            }
            "wait" => {
                let id = required("task_id")?;
                ensure!(id.chars().all(|c|c.is_ascii_alphanumeric()||c=='-'),"invalid task identity");
                let timeout = Duration::from_secs(input["timeout_secs"].as_u64().unwrap_or(30).min(300));
                let start = Instant::now();
                loop {
                    let state = self.client.request("GET", &format!("/api/distributed/tasks/{id}?after_sequence={}",input["after_sequence"].as_u64().unwrap_or(0)), None).await?;
                    let task = &state["task"];
                    ensure!(!task.is_null(), "task missing or inaccessible");
                    if matches!(task["status"].as_str(), Some("completed" | "failed" | "cancelled")) || start.elapsed() >= timeout {
                        break state;
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
            "cancel" | "retry" => {
                let id = required("task_id")?;
                ensure!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'), "invalid task identity");
                self.client.request("POST", &format!("/api/distributed/tasks/{id}/{action}"), Some(json!({}))).await?
            }
            "observe" => self.client.request("POST", "/api/distributed/worker/report", Some(json!({
                "incarnation":self.incarnation,"task_id":self.parent,"generation":self.generation,
                "message_id":input["message_id"].as_str().map_or_else(|| uuid::Uuid::new_v4().to_string(),str::to_owned),
                "kind":"observation","text":required("text")?}))).await?,
            "read_artifact" => {
                let id=required("artifact_id")?;ensure!(id.chars().all(|c|c.is_ascii_hexdigit()),"invalid artifact identity");
                let value=self.client.request("GET",&format!("/api/distributed/artifacts/{id}"),None).await?;
                let bytes=STANDARD.decode(value["content_base64"].as_str().ok_or_else(||anyhow::anyhow!("artifact content missing"))?)?;
                ensure!(format!("{:x}",sha2::Sha256::digest(&bytes))==value["artifact"]["sha256"].as_str().unwrap_or(""),"artifact checksum mismatch");
                let text=String::from_utf8(bytes).map_err(|_|anyhow::anyhow!("binary artifact; use an artifact reference instead"))?;
                let start=usize::try_from(input["start_line"].as_u64().unwrap_or(1))?;
                let end=usize::try_from(input["end_line"].as_u64().unwrap_or(start.saturating_add(79) as u64))?;
                ensure!(start>0 && end>=start,"invalid line range");
                json!({"artifact":value["artifact"],"lines":text.lines().enumerate().skip(start-1).take(end-start+1).map(|(n,line)|format!("{}: {line}",n+1)).collect::<Vec<_>>()})
            }
            "publish_artifact" => {
                let path = self.root.join(required("path")?).canonicalize()?;
                ensure!(path.starts_with(self.root.canonicalize()?), "artifact outside workspace");
                ensure!(std::fs::metadata(&path)?.len() <= 8 * 1024 * 1024, "artifact too large");
                self.client.request("POST", "/api/distributed/artifacts", Some(json!({"incarnation":self.incarnation,
                    "task_id":self.parent,"generation":self.generation,"kind":input["kind"].as_str().unwrap_or("file"),
                    "name":path.file_name().unwrap_or_default().to_string_lossy(),"content_base64":STANDARD.encode(std::fs::read(path)?)}))).await?
            }
            _ => anyhow::bail!("unknown collaboration action"),
        };
        Ok(serde_json::to_string(&value)?)
    }
}
#[async_trait]
impl Tool for CollaborationTool {
    fn name(&self) -> &'static str {
        "collaboration"
    }
    fn description(&self) -> &'static str {
        "Asynchronous collaboration through AXCrew's durable Tasks, Events, Artifacts and Workflow State. Catalog AX capabilities/Host resources, delegate logical-project tasks, inspect/wait for results, failures and observations, checkpoint workflow state using its expected_revision, report observations, publish artifacts. Supply only necessary task context. Inspect the workflow and already submitted tasks before recovering a plan; use stable request_id values to avoid duplicating work. No direct AX-to-AX messaging or permanently live coordinator is required. AXCrew exclusively controls ownership, retries and placement."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","required":["action"],"properties":{
        "action":{"enum":["catalog","delegate","status","wait","cancel","retry","observe","publish_artifact","checkpoint_workflow","read_artifact"]},
        "task":{"type":"object","required":["title","input"],"properties":{
            "request_id":{"type":"string"},"title":{"type":"string"},"input":{"type":"string"},"project_id":{"type":"string"},
            "workspace_revision":{"type":"string"},"context_summary":{"type":"string"},"dependencies":{"type":"array","items":{"type":"string"}},
            "artifacts":{"type":"array","items":{"type":"string"}},"max_attempts":{"type":"integer"},
            "requirements":{"type":"object","properties":{"capabilities":{"type":"object","properties":{
                "roles":{"type":"array","items":{"type":"string"}},"skills":{"type":"array","items":{"type":"string"}},"mcp":{"type":"array","items":{"type":"string"}},
                "tools":{"type":"array","items":{"type":"string"}},"models":{"type":"array","items":{"type":"string"}},"permissions":{"type":"array","items":{"type":"string"}},"environments":{"type":"array","items":{"type":"string"}}}},
                "resources":{"type":"object","properties":{"cpu":{"type":"integer"},"ram_mb":{"type":"integer"},"gpu":{"type":"integer"}}},"instance_id":{"type":"string"}}}}},
        "task_id":{"type":"string"},"after_sequence":{"type":"integer"},"timeout_secs":{"type":"integer"},
        "expected_revision":{"type":"integer"},"state":{"type":"object"},
        "artifact_id":{"type":"string"},"start_line":{"type":"integer"},"end_line":{"type":"integer"},
        "text":{"type":"string"},"message_id":{"type":"string"},"path":{"type":"string"},"kind":{"type":"string"}}})
    }
    fn execution_boundary(&self) -> ExecutionBoundary {
        ExecutionBoundary::Remote
    }
    fn fork_for_run(&self, context: &RunContext) -> Option<Arc<dyn Tool>> {
        let mut copy = self.clone();
        copy.root.clone_from(&context.workspace_root);
        Some(Arc::new(copy))
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        if matches!(
            input["action"].as_str(),
            Some("catalog" | "status" | "wait" | "observe" | "read_artifact")
        ) {
            SafetyLevel::Safe
        } else {
            SafetyLevel::RequiresApproval
        }
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::Network
    }
    fn resources(&self, input: &Value) -> Vec<tool::ResourceAccess> {
        let network = tool::Resource::Named("distributed-control-plane".into());
        if input["action"] == "publish_artifact" {
            vec![
                tool::ResourceAccess::write(network),
                tool::ResourceAccess::read(tool::Resource::path(
                    self.root.join(input["path"].as_str().unwrap_or("")),
                )),
            ]
        } else if matches!(
            input["action"].as_str(),
            Some("catalog" | "status" | "wait" | "read_artifact")
        ) {
            vec![tool::ResourceAccess::read(network)]
        } else {
            vec![tool::ResourceAccess::write(network)]
        }
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        self.call(input)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))
    }
}
