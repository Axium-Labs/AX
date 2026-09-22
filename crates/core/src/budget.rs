//! Configurable limits for one user turn.

/// Splits a model's raw context window into what is actually usable for
/// system/runtime context, skills, memory, and conversation history, after
/// reserving room for the model's own reply and the tool schemas sent with
/// every request. Callers derive every context-related limit from this
/// instead of hardcoding their own fraction of the raw window or an
/// unrelated fixed character count.
#[derive(Clone, Copy, Debug)]
pub struct ContextBudget {
    pub context_window: usize,
    pub reserved_output: usize,
    pub tool_schema_tokens: usize,
}

impl ContextBudget {
    /// Conservative reserve for the model's own reply. AX does not track a
    /// per-provider max-output-tokens setting, so a fixed reserve is used
    /// instead of assuming the whole window is available for input.
    pub const RESERVED_OUTPUT_TOKENS: usize = 8_000;
    /// Tokens carved out of the usable budget for lazily loaded skill
    /// instructions.
    pub const SKILLS_RESERVE_TOKENS: usize = 3_000;
    /// Tokens carved out of the usable budget for retrieved long-term memory
    /// facts.
    pub const MEMORY_RESERVE_TOKENS: usize = 200;
    /// Share of the history budget available to the persisted session
    /// summary and other restored system state when a session is reopened;
    /// the remainder is reserved for verbatim recent messages.
    pub const SESSION_SUMMARY_SHARE_PERCENT: u8 = 50;

    #[must_use]
    pub const fn new(context_window: usize, tool_schema_tokens: usize) -> Self {
        Self {
            context_window,
            reserved_output: Self::RESERVED_OUTPUT_TOKENS,
            tool_schema_tokens,
        }
    }

    /// Tokens left for system/runtime context, skills, memory, and
    /// conversation history after reserving room for the reply and the tool
    /// schemas sent with every request.
    #[must_use]
    pub const fn usable(&self) -> usize {
        self.context_window
            .saturating_sub(self.reserved_output)
            .saturating_sub(self.tool_schema_tokens)
    }

    /// Token budget at which compaction should trigger, computed against the
    /// genuinely usable space rather than the raw context window.
    #[must_use]
    pub fn compact_threshold(&self, percent: u8) -> usize {
        self.usable().saturating_mul(usize::from(percent.min(100))) / 100
    }

    /// Total budget for restoring persisted conversation history when a
    /// session is (re)opened, after also carving out room for skills and
    /// memory. Split between [`Self::session_summary_budget`] and
    /// [`Self::recent_messages_budget`].
    #[must_use]
    pub const fn history_budget(&self) -> usize {
        self.usable()
            .saturating_sub(Self::SKILLS_RESERVE_TOKENS)
            .saturating_sub(Self::MEMORY_RESERVE_TOKENS)
    }

    /// Budget for the persisted session summary (and other restored system
    /// state) when a session is reopened, so a large summary cannot silently
    /// consume the entire history budget and starve recent messages.
    #[must_use]
    pub fn session_summary_budget(&self) -> usize {
        self.history_budget()
            .saturating_mul(usize::from(Self::SESSION_SUMMARY_SHARE_PERCENT))
            / 100
    }

    /// Budget for verbatim recent conversation turns restored alongside the
    /// session summary.
    #[must_use]
    pub const fn recent_messages_budget(&self) -> usize {
        self.history_budget()
    }

    /// Character budget for lazily loaded skill instructions (approximating
    /// the crate's own ~4-chars-per-token estimate for ASCII text).
    #[must_use]
    pub const fn skills_budget_chars(&self) -> usize {
        Self::SKILLS_RESERVE_TOKENS * 4
    }

    /// Character budget for retrieved long-term memory facts.
    #[must_use]
    pub const fn memory_budget_chars(&self) -> usize {
        Self::MEMORY_RESERVE_TOKENS * 4
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExecutionBudget {
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub turn_timeout_secs: u64,
    pub tool_timeout_secs: u64,
}
impl Default for ExecutionBudget {
    fn default() -> Self {
        Self {
            max_steps: 64,
            max_tool_calls: 128,
            turn_timeout_secs: 600,
            tool_timeout_secs: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContextBudget;

    #[test]
    fn usable_space_subtracts_output_and_tool_schema_reserves() {
        let budget = ContextBudget::new(100_000, 5_000);
        assert_eq!(
            budget.usable(),
            100_000 - ContextBudget::RESERVED_OUTPUT_TOKENS - 5_000
        );
    }

    #[test]
    fn compact_threshold_is_a_fraction_of_usable_space_not_the_raw_window() {
        let budget = ContextBudget::new(100_000, 0);
        let threshold = budget.compact_threshold(75);
        assert_eq!(threshold, budget.usable() * 75 / 100);
        assert!(threshold < 75_000, "threshold must not use the raw window");
    }

    #[test]
    fn a_larger_tool_schema_cost_shrinks_every_derived_budget() {
        let light = ContextBudget::new(100_000, 0);
        let heavy = ContextBudget::new(100_000, 20_000);
        assert!(heavy.usable() < light.usable());
        assert!(heavy.compact_threshold(75) < light.compact_threshold(75));
        assert!(heavy.history_budget() < light.history_budget());
    }

    #[test]
    fn tiny_context_windows_saturate_instead_of_underflowing() {
        let budget = ContextBudget::new(1_000, 500);
        assert_eq!(budget.usable(), 0);
        assert_eq!(budget.compact_threshold(75), 0);
        assert_eq!(budget.history_budget(), 0);
        assert_eq!(budget.session_summary_budget(), 0);
        assert_eq!(budget.recent_messages_budget(), 0);
    }

    #[test]
    fn session_summary_gets_only_a_share_of_the_history_budget() {
        let budget = ContextBudget::new(100_000, 0);
        assert_eq!(
            budget.session_summary_budget(),
            budget.history_budget() * usize::from(ContextBudget::SESSION_SUMMARY_SHARE_PERCENT)
                / 100
        );
        assert!(budget.session_summary_budget() < budget.history_budget());
        assert_eq!(budget.recent_messages_budget(), budget.history_budget());
    }
}
