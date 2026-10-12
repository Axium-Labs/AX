//! Optional Goal adapter to `AXCrew`'s existing fenced Workflow checkpoint.
//! No worker scheduler, budget ledger or session store is created here.
use crate::distributed_client::Client;
use async_trait::async_trait;
use runtime_core::{
    AgentError, ExtensionPrompt, GoalState, RuntimeExtension, task_queue::TaskQueue,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const CHECKPOINT_KEY: &str = "ax_goal_runtime";
pub(crate) struct DistributedGoal {
    client: Client,
    task: String,
    workflow: String,
    generation: u64,
    incarnation: String,
    root: PathBuf,
}
impl DistributedGoal {
    pub(crate) fn from_env(root: &Path) -> Option<Self> {
        Some(Self {
            client: Client::from_env()?,
            task: std::env::var("AX_DISTRIBUTED_TASK").ok()?,
            workflow: std::env::var("AX_DISTRIBUTED_WORKFLOW").ok()?,
            generation: std::env::var("AX_DISTRIBUTED_GENERATION")
                .ok()?
                .parse()
                .ok()?,
            incarnation: std::env::var("AX_DISTRIBUTED_INCARNATION").ok()?,
            root: root.to_path_buf(),
        })
    }
    async fn save(&self, queue: &TaskQueue, messages: &[model::Message]) -> anyhow::Result<()> {
        anyhow::ensure!(
            queue.goal_id == self.task,
            "Goal must retain its owning durable task identity"
        );
        for _ in 0..3 {
            let current = self
                .client
                .request(
                    "GET",
                    &format!("/api/distributed/workflows/{}", self.workflow),
                    None,
                )
                .await?;
            let workflow = current.get("workflow").unwrap_or(&current);
            let mut state = workflow["state"].clone();
            if state.is_null() {
                state = json!({});
            }
            anyhow::ensure!(state.is_object(), "Workflow state must be an object");
            if state[CHECKPOINT_KEY].is_null() {
                state[CHECKPOINT_KEY] = json!({});
            }
            // Preserve other coordinators' keys. CAS and the source lease decide admission.
            // Existing Message/queue checkpoints, bounded by Crew's Workflow limit.
            // No second session database is created by this adapter.
            state[CHECKPOINT_KEY][&self.task] = json!({"protocol":"ax.goal.checkpoint.v1","task_id":self.task,"generation":self.generation,"workspace_root":self.root,"queue":queue,"messages":messages,"long_project_document":crate::long_project::distributed_document(&self.root,queue)?});
            let body = json!({"expected_revision":workflow["revision"],"state":state,"source":{"incarnation":self.incarnation,"task_id":self.task,"generation":self.generation,"message_id":"goal-checkpoint","kind":"observation","text":""}});
            match self
                .client
                .request(
                    "POST",
                    &format!("/api/distributed/workflows/{}/checkpoint", self.workflow),
                    Some(body),
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) => {
                    // Only an actual CAS conflict is retried; lease/auth/size faults stop execution.
                    if !error.to_string().contains("revision") {
                        return Err(error);
                    }
                }
            }
        }
        anyhow::bail!("Workflow checkpoint revision changed repeatedly")
    }
}

/// Native Goal/document edits use the same checkpoint seam as runtime turns.
/// Read-only native commands do not write a new remote checkpoint.
pub(crate) async fn checkpoint_native(
    state: &mut crate::repl::ReplState,
    prompt: &str,
) -> anyhow::Result<()> {
    if ![
        "/goal project {",
        "/goal project import ",
        "/goal pause",
        "/goal cancel",
        "/goal off",
        "/goal limits ",
        "/goal acknowledge ",
    ]
    .iter()
    .any(|prefix| prompt.starts_with(prefix))
    {
        return Ok(());
    }
    if let Some(host) = DistributedGoal::from_env(&state.project_root) {
        let mut messages = state.loaded_messages.clone();
        if let Some(queue) = TaskQueue::restore(&mut messages).filter(|q| q.goal.is_some()) {
            host.save(&queue, &messages).await?;
        }
    }
    Ok(())
}
#[async_trait]
impl RuntimeExtension for DistributedGoal {
    async fn checkpoint_goal(
        &self,
        queue: &TaskQueue,
        messages: &[model::Message],
    ) -> Result<(), AgentError> {
        self.save(queue, messages).await.map_err(|error| AgentError::GoalStopped { state:GoalState::Paused, reason:format!("Durable Goal checkpoint/fencing unavailable; execution stopped before advancing: {error}") })
    }
    async fn before_turn(&self, input: &str) -> Result<ExtensionPrompt, AgentError> {
        Ok(ExtensionPrompt {
            text: input.into(),
            context: vec![],
            response: None,
        })
    }
    async fn after_turn(&self, _: Option<&str>, _: Option<&str>) -> Result<(), AgentError> {
        Ok(())
    }
    fn wrap_tool(&self, tool: Arc<dyn tool::Tool>) -> Arc<dyn tool::Tool> {
        tool
    }
}

/// Import ONLY host-supplied queue state, not a second session persistence format.
pub(crate) fn restored_queue(value: &Value, task: &str, root: &Path) -> anyhow::Result<TaskQueue> {
    anyhow::ensure!(
        value["protocol"] == "ax.goal.checkpoint.v1" && value["task_id"] == task,
        "Durable Goal checkpoint identity/protocol mismatch"
    );
    let mut queue: TaskQueue = serde_json::from_value(value["queue"].clone())?;
    anyhow::ensure!(
        queue.goal_id == task && queue.goal.is_some(),
        "Durable checkpoint must match the owning Goal task"
    );
    if queue
        .goal
        .as_ref()
        .is_some_and(|g| g.state != GoalState::Completed)
    {
        anyhow::ensure!(
            queue.tasks.iter().all(|t| t.child.is_none()),
            "Local child sessions cannot be silently moved between hosts; inspect receipts/artifacts before takeover"
        );
        let messages: Vec<model::Message> = serde_json::from_value(value["messages"].clone())
            .map_err(|_| anyhow::anyhow!("Durable Goal execution evidence is unavailable"))?;
        // A new isolated workspace does not contain the old owner's local tool effects.
        // Do not ask the model to reconstruct them and risk duplicate remote effects.
        anyhow::ensure!(
            messages.iter().all(|m| m.tool_calls.is_empty()
                || m.provider_metadata
                    .as_ref()
                    .is_some_and(|v| v.get("axGoalValidator").is_some())),
            "Takeover requires host reconciliation of ordinary tool effects/artifacts before automatic Goal execution; saved Goal and budget are retained"
        );
    }
    let previous = PathBuf::from(
        value["workspace_root"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("checkpoint workspace missing"))?,
    );
    let rebase = |path: &mut PathBuf| -> anyhow::Result<()> {
        let relative = path
            .strip_prefix(&previous)
            .map_err(|_| anyhow::anyhow!("Goal policy path outside its saved workspace"))?;
        anyhow::ensure!(
            !relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "invalid Goal policy path"
        );
        *path = root.join(relative);
        Ok(())
    };
    let goal = queue.goal.as_mut().unwrap();
    for path in &mut goal.config.deliverables {
        rebase(path)?;
    }
    for validator in &mut goal.config.validators {
        if let runtime_core::goal::ValidatorKind::Command { reference, .. } = &mut validator.kind {
            rebase(reference)?;
        }
    }
    Ok(queue)
}
