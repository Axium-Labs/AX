//! Structured session state: what survives a compression pass.
//!
//! The summarizer returns a small JSON state rather than prose, so entries can
//! be merged with the previous summary, ranked by importance, and fitted to
//! the session-summary budget without re-summarising.

use model::Message;
use serde_json::Value;

use crate::token::estimate_tokens;

#[derive(Clone, Debug)]
pub(crate) struct StateEntry {
    pub(crate) kind: String,
    pub(crate) content: String,
    pub(crate) importance: f64,
}

pub(crate) fn parse_semantic_state(output: &str) -> Option<Vec<StateEntry>> {
    let value: Value = serde_json::from_str(output.trim()).ok()?;
    let entries = value.get("state")?.as_array()?;
    if entries.is_empty() {
        return None;
    }
    entries
        .iter()
        .map(|entry| {
            let kind = entry.get("type")?.as_str()?.trim();
            let content = entry.get("content")?.as_str()?.trim();
            let importance = entry.get("importance")?.as_f64()?;
            if kind.is_empty()
                || content.is_empty()
                || !importance.is_finite()
                || !(0.0..=1.0).contains(&importance)
            {
                return None;
            }
            Some(StateEntry {
                kind: kind.to_owned(),
                content: content.to_owned(),
                importance,
            })
        })
        .collect()
}

pub(crate) fn parse_saved_summary(summary: &str) -> Vec<StateEntry> {
    let content = summary
        .strip_prefix("[memory-summary]\n")
        .unwrap_or(summary);
    parse_semantic_state(content).unwrap_or_else(|| {
        vec![StateEntry {
            kind: "other".into(),
            content: content.to_owned(),
            importance: 0.8,
        }]
    })
}

pub(crate) fn serialize_state(entries: &[StateEntry]) -> String {
    let state = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "type": entry.kind, "content": entry.content, "importance": entry.importance,
            })
        })
        .collect::<Vec<_>>();
    format!("[memory-summary]\n{}", serde_json::json!({"state": state}))
}

pub(crate) fn fit_summary(entries: &[StateEntry], budget: usize) -> Option<String> {
    let mut ranked = (0..entries.len()).collect::<Vec<_>>();
    let score = |index: usize| {
        let entry = &entries[index];
        let recent = f64::from(u32::try_from(index).unwrap_or(u32::MAX))
            / f64::from(u32::try_from(entries.len().max(1)).unwrap_or(u32::MAX));
        let type_weight = match entry.kind.as_str() {
            "constraint" | "decision" | "goal" | "failure" | "error" => 0.015,
            "next_action" | "progress" => 0.008,
            _ => 0.0,
        };
        entry.importance + 0.03 * recent + type_weight
    };
    ranked.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then_with(|| b.cmp(&a)));
    let mut selected = Vec::new();
    for index in ranked {
        let mut trial = selected.clone();
        trial.push(index);
        trial.sort_unstable();
        let candidate = serialize_state(
            &trial
                .iter()
                .map(|&i| entries[i].clone())
                .collect::<Vec<_>>(),
        );
        if estimate_tokens(&[Message::system(candidate)]) <= budget {
            selected = trial;
        }
    }
    if selected.is_empty() {
        return None;
    }
    Some(serialize_state(
        &selected
            .into_iter()
            .map(|i| entries[i].clone())
            .collect::<Vec<_>>(),
    ))
}
