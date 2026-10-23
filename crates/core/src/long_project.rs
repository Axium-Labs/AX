//! Optional working context. The host owns documents and Sessions; the queue owns execution.
use crate::{AgentError, AgentKernel, CompressionResult, task_queue::TaskQueue};
use model::Message;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

pub const RESULT_PREFIX: &str = "[ax-recent-tool-results]\n";
pub const CONTEXT_PREFIX: &str = "[ax-long-project]\n";
pub const ROTATION_PREFIX: &str = "[ax-session-handoff]\n";
pub const COMMIT_PREFIX: &str = "[ax-session-successor]\n";

/// Shrink one retained round until it fits, halving each result per pass.
/// Halving converges in logarithmically many passes; the previous
/// decrement-by-one step re-serialized the whole body once per pass.
/// Returns the number of passes performed.
fn shrink_round(round: &mut [Message], allowance: usize) -> usize {
    let mut passes = 0;
    while crate::estimate_tokens(round) > allowance {
        let mut changed = false;
        for message in round.iter_mut().filter(|m| m.role == model::Role::Tool) {
            if let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content)
                && result.verbatim
            {
                if !result.raw_output.is_empty() {
                    let compacted = result
                        .compact_verbatim_lines(result.raw_output.chars().count().div_ceil(2));
                    if compacted.raw_output != result.raw_output {
                        message.content = serde_json::to_string(&compacted).unwrap_or_default();
                        changed = true;
                    }
                }
                continue;
            }
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&message.content)
                && let Some(output) = value["output"].as_str().filter(|s| !s.is_empty())
            {
                let short: String = output.chars().take(output.chars().count() / 2).collect();
                value["output"] = serde_json::json!(short);
                value["truncated"] = serde_json::json!(true);
                message.content = value.to_string();
                changed = true;
            }
        }
        if !changed {
            break;
        }
        passes += 1;
    }
    passes
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LongProjectConfig {
    pub enabled: bool,
    /// Optional explicit shared directory; each Goal has its own document.
    pub directory: Option<PathBuf>,
    /// Literal Markdown, never executable instructions for the host.
    pub template: Option<String>,
    pub sections: Vec<String>,
    /// Optional projected fields; Goal identity and checkpoint references are always retained.
    pub save_content: Vec<String>,
    pub auto_update: bool,
    pub update_on: Vec<String>,
    pub max_document_bytes: usize,
    pub evidence_entries: usize,
    pub stale_after_seconds: u64,
    pub rotation: RotationConfig,
}
impl Default for LongProjectConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: None,
            template: None,
            sections: [
                "目标与约束",
                "当前状态",
                "已完成工作",
                "待办任务",
                "关键决策",
                "问题与风险",
                "运行环境",
                "历史索引",
            ]
            .map(str::to_owned)
            .to_vec(),
            save_content: [
                "completed_work",
                "todo_tasks",
                "tool_references",
                "decisions",
                "issues",
                "environment",
                "history",
                "user_guidance",
            ]
            .map(str::to_owned)
            .to_vec(),
            auto_update: true,
            update_on: vec![
                "state".into(),
                "stage".into(),
                "decision".into(),
                "handoff".into(),
            ],
            max_document_bytes: 65536,
            evidence_entries: 24,
            stale_after_seconds: 86400,
            rotation: RotationConfig::default(),
        }
    }
}
impl LongProjectConfig {
    /// # Errors
    /// Rejects invalid percentages, document bounds or update rules.
    pub fn validate(&self) -> Result<(), String> {
        self.rotation.validate()?;
        if !(1024..=1_048_576).contains(&self.max_document_bytes)
            || !(1..=256).contains(&self.evidence_entries)
        {
            return Err(
                "Long Project document/evidence limits are outside supported bounds".into(),
            );
        }
        if self
            .template
            .as_ref()
            .is_some_and(|v| v.len() > self.max_document_bytes || v.contains("<!-- ax:"))
            || self
                .sections
                .iter()
                .any(|s| s.contains(['\n', '\r', '<', '>']))
            || self.save_content.iter().any(|s| {
                !matches!(
                    s.as_str(),
                    "completed_work"
                        | "todo_tasks"
                        | "tool_references"
                        | "decisions"
                        | "issues"
                        | "environment"
                        | "history"
                        | "user_guidance"
                )
            })
            || self
                .update_on
                .iter()
                .any(|s| !matches!(s.as_str(), "state" | "stage" | "decision" | "handoff"))
        {
            return Err("Invalid Long Project template, section or update rule".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationStrategy {
    Pressure,
    #[default]
    AfterCompaction,
    StageBoundary,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
// Independent retention settings, not execution-state flags.
#[allow(clippy::struct_excessive_bools)]
pub struct RotationConfig {
    pub automatic: bool,
    pub strategy: RotationStrategy,
    /// Percentage of `ContextBudget.usable()`, including projected request growth.
    pub context_percent: usize,
    pub minimum_compaction_gain_percent: usize,
    pub retain_recent_tools: bool,
    pub retain_user_guidance: bool,
    pub retain_compaction_summary: bool,
}
impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            automatic: true,
            strategy: RotationStrategy::AfterCompaction,
            context_percent: 85,
            minimum_compaction_gain_percent: 20,
            retain_recent_tools: true,
            retain_user_guidance: true,
            retain_compaction_summary: true,
        }
    }
}
impl RotationConfig {
    /// # Errors
    /// Rejects invalid percentages, document bounds or update rules.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=100).contains(&self.context_percent) || self.minimum_compaction_gain_percent > 100
        {
            return Err("Rotation percentages must be within 1..100 (gain may be zero)".into());
        }
        Ok(())
    }
}

/// No model, timer, task store or memory store is introduced by this seam.
pub trait LongProjectHost: Send + Sync {
    /// # Errors
    /// Returns ownership, conflict, storage or permission errors without discarding context.
    fn update(
        &self,
        queue: &TaskQueue,
        reason: &str,
        messages: &[Message],
    ) -> Result<String, AgentError>;
}

impl AgentKernel {
    pub fn configure_long_project_host(&mut self, host: Option<Arc<dyn LongProjectHost>>) {
        self.long_project_host = host;
    }
    pub fn configure_session_rotation(&mut self, policy: Option<RotationConfig>) {
        self.session_rotation = policy;
    }
    #[must_use]
    pub fn has_session_rotation(&self) -> bool {
        self.session_rotation.is_some()
    }
    pub fn request_session_rotation(&mut self) {
        self.rotation_requested = true;
    }
    pub(crate) fn project_context(&mut self, reason: &str) -> Result<(), AgentError> {
        let Some(queue) = self.task_queue.as_ref().filter(|q| {
            q.goal
                .as_ref()
                .is_some_and(|g| g.config.long_project.as_ref().is_some_and(|p| p.enabled))
        }) else {
            self.set_context(CONTEXT_PREFIX, None);
            return Ok(());
        };
        let host = self
            .long_project_host
            .as_ref()
            .ok_or_else(|| AgentError::GoalStopped {
                state: crate::GoalState::Paused,
                reason: "Long Project requires a document host; context has not been discarded"
                    .into(),
            })?;
        let document = host.update(queue, reason, &self.messages)?;
        let limit = self.context_budget().instructions_budget_tokens();
        let mut text = format!("{CONTEXT_PREFIX}{document}");
        while crate::token::estimate_text_tokens(&text) > limit && text.len() > CONTEXT_PREFIX.len()
        {
            let mut end = text.len().saturating_sub(text.len().div_ceil(8));
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end.max(CONTEXT_PREFIX.len()));
        }
        if text.len() < document.len().saturating_add(CONTEXT_PREFIX.len()) {
            text.push_str("\n[Excerpt truncated; retrieve PROJECT_STATE.md or archived source references on demand.]");
            while crate::token::estimate_text_tokens(&text) > limit
                && text.len() > CONTEXT_PREFIX.len()
            {
                let end = text
                    .char_indices()
                    .rev()
                    .nth(1)
                    .map_or(CONTEXT_PREFIX.len(), |(i, _)| i);
                text.truncate(end.max(CONTEXT_PREFIX.len()));
            }
        }
        self.set_context(CONTEXT_PREFIX, Some(Message::system(text)));
        Ok(())
    }
    // Keep the save/commit/context replacement order visible at one boundary.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn rotate_context<H>(
        &mut self,
        compression: Option<&CompressionResult>,
        checkpoint: &mut H,
    ) -> Result<(), AgentError>
    where
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let policy = self
            .goal()
            .and_then(|g| {
                g.config
                    .long_project
                    .as_ref()
                    .filter(|p| p.enabled)
                    .map(|p| &p.rotation)
            })
            .or_else(|| self.goal().and_then(|g| g.config.session_rotation.as_ref()))
            .or(self.session_rotation.as_ref())
            .cloned();
        let Some(policy) = policy else {
            return Ok(());
        };
        policy.validate().map_err(AgentError::GoalMismatch)?;
        let state = self.turn_state();
        if state.running_children > 0
            || state.pending_approvals > 0
            || state.retry_state
            || state.pending_user_input
            || state.pending_tool_calls > 0
            || self
                .goal()
                .is_some_and(|g| g.pending_validation.is_some() || g.pending_usage.is_some())
        {
            return Ok(());
        }
        let budget = self.context_budget();
        budget
            .validate_capacity(crate::context::minimum_runtime_tokens(&self.messages))
            .map_err(AgentError::Budget)?;
        let projected = self
            .estimated_context_tokens()
            .saturating_add(self.pending_request_growth(budget));
        let pressured =
            projected.saturating_mul(100) >= budget.usable().saturating_mul(policy.context_percent);
        let ineffective = compression.is_some_and(|c| {
            c.estimated_tokens_before
                .saturating_sub(c.estimated_tokens_after)
                .saturating_mul(100)
                < c.estimated_tokens_before
                    .saturating_mul(policy.minimum_compaction_gain_percent)
        });
        let completed_tasks = self.task_queue.as_ref().map_or_else(Vec::new, |q| {
            q.tasks
                .iter()
                .enumerate()
                .filter_map(|(index, t)| {
                    (t.status == crate::task_queue::TaskStatus::Completed).then_some(index)
                })
                .collect::<Vec<_>>()
        });
        let previous_stages = self
            .messages
            .iter()
            .rev()
            .find_map(|m| {
                m.content.strip_prefix("[ax-recovery]\n").and_then(|s| {
                    serde_json::from_str::<serde_json::Value>(s)
                        .ok()
                        .and_then(|v| {
                            serde_json::from_value::<Vec<usize>>(v["completed_tasks"].clone()).ok()
                        })
                })
            })
            .unwrap_or_default();
        let has_completed_stage = !completed_tasks.is_empty() && completed_tasks != previous_stages;
        let automatic = policy.automatic
            && pressured
            && match policy.strategy {
                RotationStrategy::Pressure => true,
                RotationStrategy::AfterCompaction => {
                    ineffective
                        || compression.is_none()
                            && budget.needs_compaction(
                                self.estimated_context_tokens(),
                                self.pending_request_growth(budget),
                            )
                }
                RotationStrategy::StageBoundary => has_completed_stage,
            };
        if !self.rotation_requested && !automatic {
            return Ok(());
        }
        self.project_context("handoff")?;
        self.checkpoint_queue(checkpoint)?;
        let mut context: Vec<_> = self
            .messages
            .iter()
            .filter(|m| {
                m.role == model::Role::System
                    && ![
                        "[session-summary]",
                        "[memory-summary]",
                        "[ax-goal]",
                        "[ax-continuation]",
                        "[ax-goal-usage]",
                        "[ax-goal-allowed]",
                        crate::execution::CONTEXT_PREFIX,
                        crate::execution::STATE_PREFIX,
                        crate::task_queue::STATE_PREFIX,
                        crate::task_queue::PROGRESS_PREFIX,
                        "[ax-historical-summary]",
                        "[ax-recent-receipts]",
                        RESULT_PREFIX,
                        ROTATION_PREFIX,
                        COMMIT_PREFIX,
                        "[ax-recovery]",
                        "[ax-goal-validation]",
                    ]
                    .iter()
                    .any(|prefix| m.content.starts_with(prefix))
            })
            .cloned()
            .collect();
        let recovery = serde_json::json!({"goal_id":self.goal_id,"required_actions":state.required_actions,"completed_tasks":completed_tasks,
            "recovery":"Continue unfinished work only. Saved tool receipts and Task/Checkpoint state are authoritative. Inspect actual environment and stale decisions before use. Never repeat completed or uncertain external effects; retrieve old Sessions on demand."});
        context.push(Message::system(format!("[ax-recovery]\n{recovery}")));
        if let Some(q) = &self.task_queue {
            context.push(q.progress());
        }
        let target = self
            .goal()
            .map_or_else(|| self.execution_state().overall_goal, |g| g.target.clone());
        context.push(Message::user(target.clone()));
        if policy.retain_user_guidance {
            let mut seen = std::collections::HashSet::from([target]);
            for message in self.messages.iter().filter(|m| m.role == model::Role::User) {
                if seen.insert(message.content.clone()) {
                    context.push(message.clone());
                }
            }
        }
        if policy.retain_compaction_summary
            && let Some(summary) = self
                .messages
                .iter()
                .rev()
                .find(|m| m.content.starts_with("[memory-summary]"))
        {
            context.push(Message::system(format!("[ax-historical-summary]\nUnverified historical context; recheck source Sessions and Task/Checkpoint facts before use.\n{}", summary.content)));
        }
        let latest_round = self
            .messages
            .iter()
            .rposition(|m| m.role == model::Role::Assistant && !m.tool_calls.is_empty());
        let recent_ids = latest_round.map_or_else(Vec::new, |start| {
            self.messages[start]
                .tool_calls
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>()
        });
        if policy.retain_recent_tools
            && let Some(start) = latest_round
        {
            let mut round: Vec<_> = self.messages[start..]
                .iter()
                .filter(|m| m.role != model::Role::System)
                .cloned()
                .collect();
            for message in round.iter_mut().filter(|m| m.role == model::Role::Tool) {
                if let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content) {
                    message.content = if result.verbatim {
                        serde_json::to_string(
                            &result.compact_verbatim_lines(budget.tool_result_chars()),
                        )
                        .unwrap_or_default()
                    } else {
                        result.model_view(budget.tool_result_chars())
                    };
                }
            }
            let allowance = budget.recent_raw_budget().min(
                budget
                    .usable()
                    .saturating_sub(crate::estimate_tokens(&context))
                    .saturating_sub(budget.pool.next_request_reserve),
            );
            shrink_round(&mut round, allowance);
            if crate::estimate_tokens(&round) <= allowance {
                context.extend(round);
            }
        }
        context.push(Message::system(format!("[ax-recent-receipts]\nCompleted round call IDs: {}. Raw output remains available through tool_output; retrieve results rather than executing these operations again.", recent_ids.join(", "))));
        budget
            .validate_capacity(crate::estimate_tokens(&context))
            .map_err(|reason| AgentError::GoalStopped {
                state: crate::GoalState::Paused,
                reason: format!(
                    "Required handoff context exceeds ContextBudget; original Session retained: {reason}"
                ),
            })?;
        let after = crate::estimate_tokens(&context);
        let before = self.estimated_context_tokens();
        if !self.rotation_requested
            && before.saturating_sub(after).saturating_mul(100)
                < before.saturating_mul(policy.minimum_compaction_gain_percent)
        {
            return Ok(());
        }
        let mut seeds = context.clone();
        if let Ok(results) = self.result_reader.0.read() {
            let recent: std::collections::BTreeMap<_, _> = results
                .iter()
                .filter(|(id, _)| recent_ids.contains(id))
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect();
            if !recent.is_empty() {
                seeds.push(Message::system(format!(
                    "{RESULT_PREFIX}{}",
                    serde_json::to_string(&recent).unwrap_or_default()
                )));
            }
        }
        if let Some(q) = &self.task_queue {
            seeds.push(q.snapshot());
        }
        seeds.push(self.execution_state().snapshot());
        if let Some(receipts) = crate::child_result::snapshot(&self.child_results) {
            seeds.push(receipts);
        }
        let marker = Message::system(format!(
            "{ROTATION_PREFIX}{}",
            serde_json::json!({"goal_id":self.goal_id,"context":seeds})
        ));
        self.raw_turn_messages.push(marker);
        // The host must durably seed the successor and commit the old Session redirect before returning.
        checkpoint(&self.raw_turn_messages)?;
        self.raw_turn_messages.clear();
        if let Ok(mut results) = self.result_reader.0.write() {
            results.retain(|id, _| recent_ids.contains(id));
        }
        self.messages = context;
        self.compression_dirty = true;
        self.rotation_requested = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fmt::Write as _;

    fn verbatim_result(lines: usize) -> String {
        let text = (1..=lines).fold(String::new(), |mut text, line| {
            let _ = writeln!(text, "{line}: {}", "x".repeat(200));
            text
        });
        serde_json::to_string(&tool::ToolResult::from_output(
            &tool::ToolOutput::Verbatim {
                text,
                total_lines: lines,
                start_line: 1,
                truncated: false,
                line_truncated: false,
            },
        ))
        .unwrap()
    }

    /// Rotation used to drop one line per pass, so the number of full
    /// re-serializations grew with the body. Halving must stay logarithmic.
    #[test]
    fn shrinking_a_large_round_halves_instead_of_removing_one_line() {
        let mut round: Vec<Message> = (0..8)
            .map(|index| Message::tool(format!("call-{index}"), verbatim_result(400)))
            .collect();
        let before = crate::estimate_tokens(&round);
        // A target the old per-line loop needed ~hundreds of passes for.
        let allowance = before / 40;
        let passes = shrink_round(&mut round, allowance);
        assert!(
            passes <= 12,
            "halving must stay logarithmic in body size, took {passes} passes"
        );
        assert!(crate::estimate_tokens(&round) <= before);
        for message in &round {
            let result: tool::ToolResult = serde_json::from_str(&message.content).unwrap();
            assert!(result.verbatim);
            assert!(result.truncated);
            assert!(result.total_lines.is_some());
            // Complete leading lines only, with the declared coordinate prefix.
            assert!(result.raw_output.is_empty() || result.raw_output.ends_with('\n'));
            if !result.raw_output.is_empty() {
                assert!(result.raw_output.starts_with("1: "));
            }
        }
    }

    /// The loop must terminate even when the allowance can never be met.
    #[test]
    fn shrinking_terminates_when_the_allowance_can_never_be_met() {
        let mut round = vec![
            Message::tool("call-0", verbatim_result(400)),
            Message::tool("call-1", json!({"output":"y"}).to_string()),
        ];
        let passes = shrink_round(&mut round, 0);
        assert!(passes <= 16, "bounded passes expected, took {passes}");
        // Re-running is a no-op: nothing below the floor changes any more.
        assert_eq!(shrink_round(&mut round, 0), 0);
    }
}
