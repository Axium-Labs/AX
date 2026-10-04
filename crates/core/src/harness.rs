//! Advisory coding policy, typed dynamic work admission and final-response review.
use crate::{AgentError, AgentKernel, task_queue};
use model::{FunctionSpec, Message, ModelRequest, ToolSpec};
use serde_json::{Value, json};

pub const POLICY: &str = "[ax-coding-harness]\nExecute requested deliverables until completed, failed with evidence, or waiting for a necessary user decision. Missing venv, pytest, packages, runner, uncloned repo or no search matches are recoverable setup/task-local observations: attempt installation, creation or fallback, then record a local failure and continue independent work. Honor user-requested ordering: sequential work runs one item at a time; choose parallel execution only when compatible with the user goal. Do setup and data reads before queueing. Queue items must be concrete user deliverables, never a preliminary discovery/reporting checklist. Use task_source projected columns and work mapping for tables; it automatically registers records as children. Otherwise register concrete executable items with task_queue once known. Data readers may return {\"ax_work_items\":[{\"title\":...,\"input\":...,\"workspace\":...,\"output_dir\":...}]} to register work automatically. Each child input must include all its necessary data and output requirements, never sibling results or reference answers. Select allowed dataset columns at the reader, before loading data; execution and evaluator inputs stay separate. Use repo/config/tools to answer discoverable questions. Only user-exclusive decisions use request_user_input, which suspends/resumes the goal. A final answer requires every known item terminal and all requested durable reports written. Freeze patches and save results before workspace cleanup. An unavailable official evaluator leaves official_resolved=null and does not prevent coding/local validation.";
pub(crate) const CHECK: &str = "completion_check";
pub(crate) const PENDING: &str = "[ax-completion-pending]";

impl AgentKernel {
    /// # Panics
    /// Panics if the execution-state mutex was poisoned by a prior panic.
    #[must_use]
    pub fn with_coding_harness(mut self) -> Self {
        self.coding_harness = true;
        self.execution.lock().unwrap().advisory_step_scope = true;
        self
    }

    pub(crate) async fn prepare_environment(&mut self) -> Result<(), AgentError> {
        if !self.coding_harness {
            return Ok(());
        }
        self.execution.lock().unwrap().advisory_step_scope = true;
        let cwd = self.child_run.as_ref().map_or_else(
            || std::env::current_dir().unwrap_or_default(),
            |run| run.cwd.clone(),
        );
        let root = self.child_run.as_ref().map_or_else(
            || self.execution_root.clone().unwrap_or_else(|| cwd.clone()),
            |run| run.workspace_root().to_path_buf(),
        );
        let context =
            tokio::task::spawn_blocking(move || tool::EnvironmentContext::detect(&cwd, &root))
                .await
                .map_err(|error| AgentError::WorkerJoin(error.to_string()))?;
        self.set_context(
            "[ax-environment]\n",
            Some(Message::system(format!(
                "[ax-environment]\n{}",
                json!(context)
            ))),
        );
        if self.coding_harness {
            self.set_context("[ax-coding-harness]\n", Some(Message::system(POLICY)));
        }
        Ok(())
    }

    /// Explicit typed observations, never list/keyword parsing. The work input
    /// belongs to the current goal and carries the same schema as queue admission.
    pub(crate) fn admit_work_items(&mut self, raw: &str) -> Result<bool, String> {
        if self.child_run.is_some() {
            return Ok(false);
        }
        // Shell output includes an exit header. Locate only a complete JSON
        // object on a line, not arbitrary bracketed prose or benchmark fields.
        let value = serde_json::from_str::<Value>(raw).ok().or_else(|| {
            raw.lines()
                .find_map(|line| serde_json::from_str::<Value>(line).ok())
        });
        let Some(items) = value
            .as_ref()
            .and_then(|value| value.get("ax_work_items"))
            .and_then(Value::as_array)
        else {
            return Ok(false);
        };
        let items = items
            .iter()
            .filter(|item| {
                !self.task_queue.as_ref().is_some_and(|queue| {
                    queue.tasks.iter().any(|task| {
                        item["title"] == task.title && item["input"] == task.task_input()
                    })
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let append = self
            .task_queue
            .as_ref()
            .is_some_and(|queue| !queue.tasks.is_empty());
        if items.len() < if append { 1 } else { 2 } {
            return Ok(false);
        }
        let goal = self.execution_state().overall_goal;
        task_queue::apply(
            &mut self.task_queue,
            &json!({"action": if append {"append"} else {"start"}, "overall_goal": goal, "tasks":items, "execution": if self.child_host.is_some() {"children"} else {"controller"}}),
            self.goal_id.as_deref().unwrap_or_default(),
            self.parent_goal_id.as_deref(),
        )?;
        Ok(true)
    }

    /// Semantic inventory is model-owned; the controller deterministically
    /// admits typed tasks and enforces pending-state completion. This is one
    /// review at a proposed final, not a planner or a progress/recovery lock.
    pub(crate) fn current_receipts_context(&self) -> Option<Message> {
        if self.child_run.is_some() || self.child_results.is_empty() {
            return None;
        }
        let mut receipts = self.child_results.values().collect::<Vec<_>>();
        receipts.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        let receipts = receipts.into_iter().map(|result| {
            let mut outputs = std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
            for artifact in &result.artifacts {
                if !matches!(artifact.kind.as_str(), "output" | "host-output" | "patch") { continue; }
                let path = std::path::Path::new(&artifact.path);
                if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                    outputs.entry(parent.to_string_lossy().into_owned()).or_default().insert(name.to_string_lossy().into_owned());
                }
            }
            json!({"task_id":result.task_id,"child_id":result.child_id,"status":result.status,"metrics":result.metrics,"diff_stat":result.diff_stat,"current_output_files":outputs})
        }).collect::<Vec<_>>();
        Some(Message::system(format!(
            "[ax-current-receipts]\nThese are this execution's authoritative receipts and current durable files, not historical directory contents. Complete all requested missing per-item reports and aggregate files from current evidence with null unknowns before final. Failed coding still needs its requested report files. Do not substitute an old aggregate report. {}",
            json!({"receipts":receipts})
        )))
    }

    pub(crate) async fn review_completion(&mut self) -> Result<model::ModelResponse, AgentError> {
        let mut history = self.messages.clone();
        if let Some(queue) = self.task_queue.as_ref().filter(|q| {
            matches!(
                q.state,
                task_queue::QueueState::Summarizing | task_queue::QueueState::Completed
            )
        }) {
            history.push(queue.summary_context());
            if let Some(receipts) = self.current_receipts_context() {
                history.push(receipts);
            }
        }
        history.push(Message::system("[ax-completion-review]\nAudit the proposed final against the original user deliverables and actual tool evidence. If all deliverables are complete (or individually failed with evidence), call completion_check with state=complete. If concrete executable items remain and no queue exists, call task_queue start (or append if a queue already exists) with ALL complete independent inputs and workspace specs; include dynamically discovered data items, not rules. If work/setup/reports remain but enumeration is incomplete, call completion_check state=continue with the next recoverable action. Missing local packages/runners/venv are not global blockers. If only the user can supply a necessary decision, call request_user_input. Do not accept a promise, partial result or request for benchmark environment as completion. For cross-item reports verify aggregate statuses against authoritative current receipts, evaluation freshness against artifact manifests, and absent metric/error values remain null. A generated report that contradicts current receipts still needs correction. Before accepting, verify every requested per-item file and aggregate report exists for this run, including failed items. Write missing reports from current evidence with unknown values null; a caveat about missing requested files is not their delivery. If recovery is needed, prefer returning the next executable tool call over repeatedly restating a plan. Return exactly one completion/control or executable tool call; execution still goes through the ordinary scheduler. Workers own no queue: complete one local deliverable or report an evidenced local failure for the controller."));
        let mut tools = self
            .tools
            .iter()
            .map(|tool| ToolSpec {
                kind: "function",
                function: FunctionSpec {
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                    parameters: crate::scheduler::input_schema(tool.input_schema()),
                },
            })
            .collect::<Vec<_>>();
        if self.child_run.is_none() {
            tools.push(task_queue::spec());
            tools.push(crate::user_input::spec());
            if self.child_host.is_some() || !self.child_results.is_empty() {
                tools.push(crate::child_result::spec());
            }
        }
        tools.push(ToolSpec { kind:"function", function: FunctionSpec {
            name: CHECK.into(), description:"Report completion review. continue keeps execution alive; complete requires evidence for every deliverable.".into(),
            parameters:json!({"type":"object","properties":{"state":{"type":"string","enum":["complete","continue"]},"reason":{"type":"string"}},"required":["state","reason"],"additionalProperties":false})
        }});
        let response = self
            .request_with_retry(
                ModelRequest {
                    messages: crate::context::request_context(&history, self.context_budget())?,
                    tools,
                },
                |_| {},
                |_| {},
            )
            .await?;
        Ok(response)
    }
}

impl AgentKernel {
    pub(crate) fn global_stop_evidenced(&self, input: &Value) -> bool {
        input["evidence_call_ids"].as_array().is_some_and(|ids| {
            !ids.is_empty()
                && ids.iter().all(|id| {
                    self.messages.iter().any(|message| {
                        message.tool_call_id.as_deref() == id.as_str()
                            && serde_json::from_str::<tool::ToolResult>(&message.content)
                                .is_ok_and(|result| result.global_blocker.is_some())
                    })
                })
        })
    }
}
