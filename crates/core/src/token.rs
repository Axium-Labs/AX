//! Character-based token estimation.
//!
//! One measurement primitive, used by context budgeting, compression and the
//! composition root. It deliberately owns no policy: every limit derived from
//! these numbers lives in [`crate::ContextBudget`].

use model::{ContentPart, Message};
use tool::ToolRegistry;

use crate::scheduler;

/// Estimated token cost of the JSON tool schemas sent with every model
/// request, so context budgeting accounts for it instead of assuming
/// tool schemas are free.
#[must_use]
pub fn estimate_tool_schema_tokens(tools: &ToolRegistry) -> usize {
    tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(tool.name())
                + estimate_text_tokens(tool.description())
                + estimate_text_tokens(&scheduler::input_schema(tool.input_schema()).to_string())
        })
        .sum()
}

#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            estimate_text_tokens(&message.content)
                + message
                    .parts
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text } => estimate_text_tokens(text),
                        ContentPart::Image { .. } => 2048,
                    })
                    .sum::<usize>()
                + message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        estimate_text_tokens(&call.function.name)
                            + estimate_text_tokens(&call.function.arguments)
                    })
                    .sum::<usize>()
                + message
                    .tool_call_id
                    .as_ref()
                    .map_or(0, |id| estimate_text_tokens(id))
                + 4
        })
        .sum()
}

pub(crate) fn estimate_text_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0_usize, 0_usize), |counts, character| {
        if character.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii.div_ceil(4) + non_ascii.saturating_mul(2)
}
