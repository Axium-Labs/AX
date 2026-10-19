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
pub const POLICY: &str = "\
[ax-coding-harness]
Execute requested deliverables until completed, failed with evidence, or waiting for a necessary user decision. Complete necessary steps within authorized scope; retain prior user answers and authorization. Follow applicable project workflows, including recovery paths. Use repo/config/tools to answer discoverable questions. Only user-exclusive decisions or unavailable external prerequisites use request_user_input, which suspends/resumes the goal. Permission prompts and denials belong to the runtime. Respect user pause/cancel and actual permission denials.

Missing venv, packages, runners, unavailable commands, no search matches and a single client's missing login are local observations, not proof that the task is impossible. Diagnose the failing layer and try documented, already-authorized alternatives before escalating. Reuse existing supported authentication through its intended interface without displaying, persisting or exposing secrets. Do not bypass an actual access denial, expand access, or claim an untried fallback failed. If user action is still necessary, state the observed failure, attempted recovery and exact missing prerequisite. Continue independent authorized work.

For edits, prefer structured patches and existing project scripts or format-aware serializers. Inspect the actual diff or changed region and use the smallest relevant validation before building on the edit. If an error appears after your change, compare with the pre-edit state before calling it pre-existing. Correct your own mistakes without overwriting unrelated changes. Change strategy after repeated failures instead of repeating speculative rewrites.

Honor user-requested ordering: sequential work runs one item at a time; choose parallel execution only when compatible with the user goal. Do setup and data reads before queueing. Queue items must be concrete user deliverables, never a preliminary discovery/reporting checklist. Use task_source projected columns and work mapping for tables; it automatically registers records as children. Otherwise register concrete executable items with task_queue once known. Data readers may return {\"ax_work_items\":[{\"title\":...,\"input\":...,\"workspace\":...,\"output_dir\":...}]} to register work automatically. Each child input must include all its necessary data and output requirements, never sibling results or reference answers. Select allowed dataset columns at the reader, before loading data; execution and evaluator inputs stay separate.

A final answer requires every known item terminal and all requested durable reports written. Reconcile the report with current tool receipts, actual changed files and validation results. Distinguish attempted, changed, built, validated and externally delivered states; an unavailable measurement is unknown. Do not claim success, no change or a pre-existing fault without evidence. Use existing evidence when sufficient rather than rerunning unrelated checks. Freeze patches and save results before workspace cleanup. An unavailable official evaluator leaves official_resolved=null and does not prevent coding/local validation.";

pub(crate) const HANDOFF_PENDING: &str = "[ax-handoff-pending]";

#[derive(Clone, Copy)]
pub(crate) enum Handoff {
    Question,
    Final,
}

impl AgentKernel {
    /// A bounded execution-model checkpoint after actual recoverable errors.
    /// No semantic classifier, independent reviewer or automatic recovery action.
    pub(crate) fn handoff_review(&self, kind: Handoff, steps_used: usize) -> Option<Message> {
        let (reviewed, boundary) = match kind {
            Handoff::Question => (self.continuation.handoff_question_reviewed, "question"),
            Handoff::Final => (self.continuation.handoff_final_reviewed, "final"),
        };
        if reviewed
            || (matches!(kind, Handoff::Final) && self.goal_evaluator.is_some())
            || (self.budget.max_steps != 0 && steps_used >= self.budget.max_steps)
        {
            return None;
        }
        // Questions must consider attempted recovery across the current turn.
        // Ordinary finals only need another step when their latest tool round
        // still failed. A later successful recovery/control receipt should not
        // impose another review merely because an earlier operation failed.
        let history = match kind {
            Handoff::Question => self.raw_turn_messages.as_slice(),
            Handoff::Final => {
                let round = self
                    .raw_turn_messages
                    .iter()
                    .rposition(|m| !m.tool_calls.is_empty())?;
                &self.raw_turn_messages[round + 1..]
            }
        };
        // Raw current-turn receipts survive model context compaction. Old turns
        // and permission denials cannot trigger this feedback.
        let failures = history
            .iter()
            .rev()
            .filter(|message| message.role == model::Role::Tool)
            .filter_map(|message| {
                let value: Value = serde_json::from_str(&message.content).ok()?;
                (value["status"] == "error"
                    && value["permission_denied"] != true
                    && value.get("global_blocker").is_none_or(Value::is_null))
                .then(|| message.tool_call_id.clone())
                .flatten()
            })
            .take(8)
            .collect::<Vec<_>>();
        if failures.is_empty() {
            return None;
        }
        Some(Message::system(format!(
            "[ax-handoff-review]\n{}\nBefore handing control to the user, review these actual failure receipts and the applicable workflow. A command/client/setup failure alone does not establish a missing permission or an unavailable resource. Check documented recovery and already-authorized interfaces; use tools if evidence is missing. Never expose secrets or bypass a real denial. Check whether your own edits caused the failure and reconcile any report with the actual changes and checks. If a user-only decision or prerequisite remains, ask the precise question with the observed evidence. Otherwise complete the authorized work. This checkpoint runs at most once per question/final boundary in this turn; it does not certify success, require extra tests, or override user pause/cancel. If current evidence is sufficient, return the supported final answer or call request_user_input again.",
            json!({"boundary":boundary,"failed_call_ids":failures})
        )))
    }
}

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
