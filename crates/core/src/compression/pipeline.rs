//! The layered compression pipeline: cleanup, deduplicate, semantic summary.
//!
//! Called before every model request. It only rewrites the effective
//! in-memory transcript; the raw turn log and the JSONL event stream are never
//! touched.

use model::{Message, ModelError, ModelRequest};

use super::summary::{fit_summary, parse_saved_summary, parse_semantic_state};
use crate::{
    AgentError, AgentEvent, AgentKernel,
    token::{estimate_text_tokens, estimate_tokens},
};

#[derive(Clone, Debug)]
pub struct CompressionResult {
    pub summary: String,
    pub removed_messages: usize,
    pub retained_messages: usize,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
    pub tool_outputs_reduced: usize,
    pub tool_outputs_removed: usize,
    pub semantic_called: bool,
}

impl AgentKernel {
    /// Applies the layered compression pipeline when effective context is under
    /// pressure. Called before every model request in the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the summarization model call fails or returns no text.
    pub async fn compress_if_needed<F>(
        &mut self,
        emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compress(false, emit).await
    }

    /// Runs the same pipeline more aggressively on explicit user request.
    /// Recent messages and persistent system context remain protected.
    ///
    /// # Errors
    ///
    /// Returns an error when the summarization model call fails or produces
    /// an invalid response.
    pub async fn compact_now<F>(&mut self, emit: F) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compress(true, emit).await
    }

    #[allow(clippy::too_many_lines)]
    async fn compress<F>(
        &mut self,
        force: bool,
        mut emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let _timer = tool::telemetry::Timer::new("context.compress");
        let before = self.estimated_context_tokens();
        let original = self.messages.clone();
        let budget = self.context_budget();
        if !force && !budget.needs_compaction(before, self.pending_request_growth(budget)) {
            return Ok(None);
        }
        let target = if force {
            budget
                .pressure_target()
                .saturating_sub(budget.pool.next_request_reserve)
        } else {
            budget.pressure_target()
        };
        let need_to_free = before.saturating_sub(target);
        if need_to_free == 0 && !force {
            return Ok(None);
        }
        let recent_start = recent_raw_start(&self.messages, budget.recent_raw_budget());
        let recent_raw_tokens = estimate_tokens(&self.messages[recent_start..]);
        let mut reduced = 0;
        let mut tier = "cleanup";

        // Largest old, reproducible outputs first. Never mutate the raw turn log.
        let mut candidates = (0..recent_start)
            .filter(|&i| {
                self.messages[i].role == model::Role::Tool && self.messages[i].parts.is_empty()
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|&i| {
            std::cmp::Reverse(estimate_tokens(std::slice::from_ref(&self.messages[i])))
        });
        for i in candidates {
            if self.estimated_context_tokens() <= target && !force {
                break;
            }
            let old = self.messages[i].content.clone();
            if let Some(short) = compact_tool_output(&old, budget.compacted_tool_result_chars()) {
                self.messages[i].content = short;
                reduced += 1;
            }
        }

        // Repeated reads and repeated failures: keep the latest occurrence.
        if self.estimated_context_tokens() > target || force {
            tier = "deduplicate";
            let mut seen = std::collections::HashSet::new();
            let call_keys = self
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .map(|call| {
                    (
                        call.id.clone(),
                        format!("{}:{}", call.function.name, call.function.arguments),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>();
            for i in (0..self.messages.len()).rev() {
                if self.messages[i].role != model::Role::Tool || !self.messages[i].parts.is_empty()
                {
                    continue;
                }
                let content = self.messages[i].content.clone();
                let key = self.messages[i]
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| call_keys.get(id))
                    .cloned()
                    .unwrap_or(content.clone());
                if !seen.insert(key) && i < recent_start && estimate_text_tokens(&content) > 6 {
                    self.messages[i].content = "[duplicate tool output]".into();
                    reduced += 1;
                }
            }
        }

        let mut semantic_called = false;
        let mut removed_messages = 0;
        let mut tool_outputs_removed = 0;
        if self.estimated_context_tokens() > target || (force && reduced == 0) {
            // Only complete older turns are eligible. Existing summaries stay verbatim.
            let split = (recent_start..self.messages.len())
                .find(|&i| self.messages[i].role == model::Role::User)
                .unwrap_or(0);
            let old = &self.messages[..split];
            let transcript = old
                .iter()
                .filter(|m| m.role != model::Role::System)
                .map(|m| {
                    format!(
                        "{:?}: {}{}",
                        m.role,
                        m.content,
                        if m.tool_calls.is_empty() {
                            String::new()
                        } else {
                            format!("\nTool calls: {:?}", m.tool_calls)
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !transcript.is_empty() {
                tool_outputs_removed = old.iter().filter(|m| m.role == model::Role::Tool).count();
                let mut compression_messages = vec![Message::system(
                    "Compress the older conversation into the smallest state sufficient to continue the task correctly. Preserve information that may affect future decisions, such as important user constraints, decisions, unresolved problems, relevant failures, current progress, and necessary facts. Decide semantically what matters. Do not invent information or repeat information already preserved by an earlier summary. Return only JSON: {\"state\":[{\"type\":\"other\",\"content\":\"...\",\"importance\":0.8}]}. Choose each entry's type as appropriate, for example constraint, decision, goal, fact, progress, failure, error, next_action, or other. Include only entries that matter; no type is required. Importance must be between 0 and 1.",
                )];
                if let Some(previous) = old
                    .iter()
                    .find(|m| m.content.starts_with("[memory-summary]"))
                {
                    compression_messages.push(Message::system(format!("Already preserved session state, for reference only. Do not rewrite it:\n{}", previous.content)));
                }
                compression_messages.push(Message::user(transcript));
                let response = self
                    .provider
                    .complete(ModelRequest {
                        messages: compression_messages,
                        tools: Vec::new(),
                    })
                    .await
                    .map_err(|error| {
                        self.messages.clone_from(&original);
                        AgentError::Model(error)
                    })?;
                if response.content.trim().is_empty() {
                    self.messages = original;
                    return Err(AgentError::Model(ModelError::InvalidResponse(
                        "context summarizer returned empty text".into(),
                    )));
                }
                let Some(new_state) = parse_semantic_state(&response.content) else {
                    self.messages = original;
                    return Ok(None);
                };
                semantic_called = true;
                tier = "semantic";
                let mut retained = self.messages.split_off(split);
                let mut persistent = self
                    .messages
                    .drain(..)
                    .filter(|m| m.role == model::Role::System)
                    .collect::<Vec<_>>();
                removed_messages = split.saturating_sub(persistent.len());
                let mut state = persistent
                    .iter()
                    .find(|m| m.content.starts_with("[memory-summary]"))
                    .map_or_else(Vec::new, |m| parse_saved_summary(&m.content));
                for entry in new_state {
                    if let Some(existing) = state
                        .iter_mut()
                        .find(|saved| saved.content.eq_ignore_ascii_case(&entry.content))
                    {
                        if entry.importance > existing.importance {
                            *existing = entry;
                        }
                    } else {
                        state.push(entry);
                    }
                }
                let Some(summary) = fit_summary(&state, budget.session_summary_budget()) else {
                    self.messages = original;
                    return Ok(None);
                };
                persistent.retain(|m| !m.content.starts_with("[memory-summary]"));
                persistent.insert(0, Message::system(summary));
                persistent.append(&mut retained);
                self.messages = persistent;
            }
        }
        let after = self.estimated_context_tokens();
        if after >= before {
            self.messages = original;
            return Ok(None);
        }
        let summary = self
            .messages
            .iter()
            .find(|m| m.content.starts_with("[memory-summary]"))
            .map_or_else(String::new, |m| {
                m.content
                    .trim_start_matches("[memory-summary]\n")
                    .to_owned()
            });
        self.compression_dirty = true;
        let ratio_milli = u32::try_from(after.saturating_mul(1000) / before.max(1)).unwrap_or(1000);
        let compression_ratio = f64::from(ratio_milli) / 1000.0;
        eprintln!(
            "[context.compress] tokens_before={before} tokens_after={after} tokens_freed={} compression_ratio={:.3} cleanup_tier={tier} semantic_called={semantic_called} tool_outputs_reduced={reduced} tool_outputs_removed={tool_outputs_removed} recent_raw_tokens={recent_raw_tokens} need_to_free={need_to_free} hard_pressure={}",
            before - after,
            compression_ratio,
            before >= budget.hard_pressure_threshold()
        );
        emit(AgentEvent::ContextCompressed {
            removed_messages,
            estimated_tokens_before: before,
            estimated_tokens_after: after,
            tokens_freed: before - after,
            compression_ratio,
            cleanup_tier: tier,
            semantic_called,
            tool_outputs_reduced: reduced,
            tool_outputs_removed,
            recent_raw_tokens,
        });
        Ok(Some(CompressionResult {
            summary,
            removed_messages,
            retained_messages: self.messages.len(),
            estimated_tokens_before: before,
            estimated_tokens_after: after,
            tool_outputs_reduced: reduced,
            tool_outputs_removed,
            semantic_called,
        }))
    }
}

fn recent_raw_start(messages: &[Message], budget: usize) -> usize {
    let mut start = messages.len();
    let mut used: usize = 0;
    while start > 0 {
        let cost = estimate_tokens(&messages[start - 1..start]);
        if used.saturating_add(cost) > budget {
            break;
        }
        used += cost;
        start -= 1;
    }
    start
}

/// Compact one oversized tool result into a bounded diagnostic slice of itself:
/// a few leading lines plus lines that look like diagnostics. The allowance is
/// supplied by the caller's `ContextBudget`, so this tier owns no private
/// character count.
fn compact_tool_output(output: &str, allowance_chars: usize) -> Option<String> {
    let per_line = (allowance_chars / 8).max(1);
    let retained_total = (allowance_chars * 3 / 4).max(per_line);
    if output.len() <= retained_total {
        return None;
    }
    let lines = output.lines().collect::<Vec<_>>();
    let mut kept: Vec<String> = Vec::new();
    for line in lines.iter().take(2) {
        kept.push(line.chars().take(per_line).collect());
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if ([
            "error",
            "fail",
            "panic",
            "stack overflow",
            "passed",
            "exit=",
            "exit code",
            "warning:",
            "test result:",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || (line.contains(".rs:") && line.chars().any(|c| c.is_ascii_digit())))
            && !kept.iter().any(|saved| saved == line)
        {
            kept.push(line.chars().take(per_line).collect());
        }
        if kept.iter().map(String::len).sum::<usize>() > retained_total {
            break;
        }
    }
    let mut short = format!("[compressed tool output; {} original lines]\n", lines.len());
    for line in kept {
        short.push_str(&line);
        short.push('\n');
    }
    if estimate_text_tokens(&short) >= estimate_text_tokens(output) {
        None
    } else {
        Some(short)
    }
}
