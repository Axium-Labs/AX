//! The coding execution policy, the runtime prompt, and typed dynamic work
//! admission.
//!
//! AX is a coding execution harness, in the `DeepSeek Harness` lineage: every run
//! receives [`crate::runtime_core`] guidance, the assembled per-capability
//! guidance, the full environment snapshot and the coding [`POLICY`], and each
//! goal gets an active task queue. There is no mode switch — this is simply
//! what the runtime is; composition (which tools a host registers) is the only
//! variation point.

use crate::runtime_core;
use crate::{AgentError, AgentKernel, task_queue};
use model::Message;
use serde_json::{Value, json};

/// Coding execution policy, injected on every run.
pub const POLICY: &str = "[ax-coding-harness]\nExecute requested deliverables until completed, failed with evidence, or waiting for a necessary user decision. Missing venv, pytest, packages, runner, uncloned repo or no search matches are recoverable setup/task-local observations: attempt installation, creation or fallback, then record a local failure and continue independent work. Honor user-requested ordering: sequential work runs one item at a time; choose parallel execution only when compatible with the user goal. Do setup and data reads before queueing. Queue items must be concrete user deliverables, never a preliminary discovery/reporting checklist. Use task_source projected columns and work mapping for tables; it automatically registers records as children. Otherwise register concrete executable items with task_queue once known. Data readers may return {\"ax_work_items\":[{\"title\":...,\"input\":...,\"workspace\":...,\"output_dir\":...}]} to register work automatically. Each child input must include all its necessary data and output requirements, never sibling results or reference answers. Select allowed dataset columns at the reader, before loading data; execution and evaluator inputs stay separate. Use repo/config/tools to answer discoverable questions. Only user-exclusive decisions use request_user_input, which suspends/resumes the goal. A final answer requires every known item terminal and all requested durable reports written. Freeze patches and save results before workspace cleanup. An unavailable official evaluator leaves official_resolved=null and does not prevent coding/local validation.";

impl AgentKernel {
    /// Inject the runtime prompt and runtime context every run receives: the
    /// runtime identity, per-capability guidance, the full environment
    /// snapshot and the coding policy.
    pub(crate) async fn prepare_environment(&mut self) -> Result<(), AgentError> {
        // The user request defines the work; these texts describe the runtime's
        // boundaries and how to use a chosen capability. They are replaced in
        // place, never accumulated.
        self.set_context(
            runtime_core::CORE_PREFIX,
            Some(Message::system(runtime_core::CORE_GUIDANCE)),
        );
        self.set_context(runtime_core::CAPABILITY_PREFIX, self.capability_guidance());
        // Full environment snapshot with cached executable probes: the run is a
        // coding execution run. The snapshot describes the environment (cwd,
        // workspace root, sandbox/network posture, shell, executables) so the
        // model can use it; it never asks the model to inspect anything.
        let cwd = self.child_run.as_ref().map_or_else(
            || std::env::current_dir().unwrap_or_default(),
            |run| run.cwd.clone(),
        );
        let root = self.child_run.as_ref().map_or_else(
            || self.execution_root.clone().unwrap_or_else(|| cwd.clone()),
            |run| run.workspace_root().to_path_buf(),
        );
        let (cwd, root) = (cwd.clone(), root.clone());
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
        self.set_context("[ax-coding-harness]\n", Some(Message::system(POLICY)));
        Ok(())
    }

    /// Assemble the per-capability guidance owned by the registered tools, in
    /// deterministic name order. A tool's guidance describes how to use it once
    /// chosen; it never instructs the model to choose it. `None` when no
    /// registered tool contributes guidance.
    fn capability_guidance(&self) -> Option<Message> {
        let mut sections = Vec::new();
        for name in self.tools.names() {
            if let Some(tool) = self.tools.get(name)
                && let Some(text) = tool.guidance()
            {
                sections.push(text);
            }
        }
        if sections.is_empty() {
            return None;
        }
        Some(Message::system(format!(
            "{}How to use each available capability correctly after you decide to use it. This is not an instruction to use any of them.\n{}",
            runtime_core::CAPABILITY_PREFIX,
            sections.join("\n")
        )))
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

    /// Current authoritative receipts for ordinary model context and optional guards.
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
