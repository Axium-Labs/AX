//! Configurable limits for one user turn.

/// Shares a model's raw context window according to actual demand for
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
    pub pool: ContextPoolPolicy,
}

impl ContextBudget {
    /// Conservative reserve used only when the model catalog does not expose
    /// a maximum output size. Shared with every other budget that must guess.
    pub const RESERVED_OUTPUT_TOKENS: usize = model::DEFAULT_OUTPUT_RESERVE_TOKENS;
    #[must_use]
    pub const fn new(
        context_window: usize,
        max_output_tokens: Option<usize>,
        tool_schema_tokens: usize,
    ) -> Self {
        Self {
            context_window,
            reserved_output: match max_output_tokens {
                Some(tokens) => tokens,
                None => Self::RESERVED_OUTPUT_TOKENS,
            },
            tool_schema_tokens,
            pool: ContextPoolPolicy::DEFAULT,
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

    /// Share of history reserved for one tool's compact projection.
    #[must_use]
    pub fn tool_result_chars(&self) -> usize {
        self.usable()
            .min(self.pool.tool_result_maximum)
            .saturating_mul(4)
    }

    /// Character allowance for one compacted tool result. Derived from the
    /// per-tool token maximum so the cleanup tier scales with the same policy
    /// as the rest of the pipeline instead of owning a private literal.
    #[must_use]
    pub fn compacted_tool_result_chars(&self) -> usize {
        self.tool_result_chars().div_ceil(8)
    }

    #[must_use]
    pub fn pressure_target(&self) -> usize {
        self.usable().saturating_sub(self.pool.next_request_reserve)
    }

    #[must_use]
    pub fn hard_pressure_threshold(&self) -> usize {
        self.usable()
    }

    #[must_use]
    pub fn recent_raw_budget(&self) -> usize {
        self.usable().min(self.pool.recent_raw_maximum)
    }

    /// Compatibility accessors are maxima against one shared pool, never partitions.
    #[must_use]
    pub const fn history_budget(&self) -> usize {
        self.usable()
    }
    #[must_use]
    pub const fn session_summary_budget(&self) -> usize {
        self.usable()
    }
    #[must_use]
    pub const fn recent_messages_budget(&self) -> usize {
        self.usable()
    }
    #[must_use]
    pub fn skills_budget_tokens(&self) -> usize {
        self.usable().min(self.pool.skill_metadata_maximum)
    }
    #[must_use]
    pub fn memory_budget_tokens(&self) -> usize {
        self.usable().min(self.pool.memory_maximum)
    }
    /// Project instructions compete with the rest of the system context; they
    /// are a demand on the same pool, never a separate allowance.
    #[must_use]
    pub fn instructions_budget_tokens(&self) -> usize {
        self.usable().min(self.pool.instructions_maximum)
    }
    /// Project the next request including expected tool results, with hard reply/schema reserves.
    #[must_use]
    pub const fn needs_compaction(&self, current: usize, next_request_growth: usize) -> bool {
        current.saturating_add(next_request_growth) > self.usable()
    }
}

/// Limits only. Demand is supplied by the caller for this request.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ContextPoolPolicy {
    pub next_request_reserve: usize,
    pub tool_result_maximum: usize,
    pub recent_raw_maximum: usize,
    pub memory_maximum: usize,
    pub skill_metadata_maximum: usize,
    pub instructions_maximum: usize,
}
impl ContextPoolPolicy {
    pub const DEFAULT: Self = Self {
        next_request_reserve: 1024,
        tool_result_maximum: 4096,
        recent_raw_maximum: 4096,
        memory_maximum: 8192,
        skill_metadata_maximum: 4096,
        instructions_maximum: 4096,
    };
}
impl Default for ContextPoolPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ContextDemand {
    pub demand: usize,
    pub minimum: usize,
    pub maximum: usize,
}
impl ContextBudget {
    /// Honor minima, then consume remaining space in model/caller supplied priority order.
    /// # Errors
    /// Returns an error when minima are invalid or exceed the shared pool.
    pub fn allocate(&self, demands: &[ContextDemand]) -> Result<Vec<usize>, &'static str> {
        if demands.iter().any(|d| d.minimum > d.maximum) {
            return Err("minimum exceeds maximum");
        }
        let mut allocations: Vec<_> = demands.iter().map(|d| d.minimum).collect();
        let minimum = allocations
            .iter()
            .try_fold(0usize, |sum, n| sum.checked_add(*n))
            .ok_or("minimum overflow")?;
        if minimum > self.usable() {
            return Err("minima exceed context pool");
        }
        let mut remaining = self.usable() - minimum;
        for (allocation, demand) in allocations.iter_mut().zip(demands) {
            let extra = demand
                .demand
                .min(demand.maximum)
                .saturating_sub(*allocation)
                .min(remaining);
            *allocation += extra;
            remaining -= extra;
        }
        Ok(allocations)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionBudget {
    /// Zero disables the corresponding limit.
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub turn_timeout_secs: u64,
    pub tool_timeout_secs: u64,
}
#[cfg(test)]
mod tests {
    use super::{ContextBudget, ContextPoolPolicy, ExecutionBudget};

    #[test]
    fn execution_limits_are_unlimited_by_default() {
        let budget = ExecutionBudget::default();
        assert_eq!(budget.max_steps, 0);
        assert_eq!(budget.max_tool_calls, 0);
        assert_eq!(budget.turn_timeout_secs, 0);
        assert_eq!(budget.tool_timeout_secs, 0);
    }

    #[test]
    fn usable_space_subtracts_output_and_tool_schema_reserves() {
        let budget = ContextBudget::new(100_000, None, 5_000);
        assert_eq!(
            budget.usable(),
            100_000 - ContextBudget::RESERVED_OUTPUT_TOKENS - 5_000
        );
    }

    #[test]
    fn model_output_limit_replaces_the_fallback_reserve() {
        let budget = ContextBudget::new(100_000, Some(32_000), 5_000);
        assert_eq!(budget.reserved_output, 32_000);
        assert_eq!(budget.usable(), 63_000);
    }

    #[test]
    fn compacted_tool_result_allowance_tracks_the_pool_policy() {
        let budget = ContextBudget::new(100_000, None, 0);
        assert_eq!(
            budget.compacted_tool_result_chars(),
            budget.pool.tool_result_maximum.div_ceil(8) * 4
        );
        let smaller = ContextBudget {
            pool: ContextPoolPolicy {
                tool_result_maximum: 512,
                ..ContextPoolPolicy::DEFAULT
            },
            ..budget
        };
        assert!(smaller.compacted_tool_result_chars() < budget.compacted_tool_result_chars());
    }

    #[test]
    fn tool_schema_cost_shrinks_the_pool_and_respects_tool_maximum() {
        let light = ContextBudget::new(100_000, None, 0);
        let heavy = ContextBudget::new(100_000, None, 20_000);
        assert!(heavy.usable() < light.usable());
        assert_eq!(heavy.tool_result_chars(), light.tool_result_chars());
        assert!(heavy.history_budget() < light.history_budget());
        let constrained = ContextBudget::new(10_000, Some(1_000), 6_000);
        assert_eq!(constrained.tool_result_chars(), 12_000);
        assert!(constrained.tool_result_chars() < light.tool_result_chars());
        assert!(constrained.compacted_tool_result_chars() < light.compacted_tool_result_chars());
    }

    #[test]
    fn tiny_context_windows_saturate_instead_of_underflowing() {
        let budget = ContextBudget::new(1_000, None, 500);
        assert_eq!(budget.usable(), 0);
        assert_eq!(budget.tool_result_chars(), 0);
        assert_eq!(budget.compacted_tool_result_chars(), 0);
        assert_eq!(budget.history_budget(), 0);
        assert_eq!(budget.session_summary_budget(), 0);
        assert_eq!(budget.recent_messages_budget(), 0);
    }

    #[test]
    fn elastic_demands_compete_and_minima_are_hard() {
        use super::ContextDemand;
        let budget = ContextBudget::new(100, Some(20), 10);
        assert_eq!(
            budget
                .allocate(&[
                    ContextDemand {
                        demand: 60,
                        minimum: 0,
                        maximum: 70
                    },
                    ContextDemand {
                        demand: 50,
                        minimum: 20,
                        maximum: 70
                    }
                ])
                .unwrap(),
            vec![50, 20]
        );
        assert!(
            budget
                .allocate(&[ContextDemand {
                    demand: 71,
                    minimum: 71,
                    maximum: 100
                }])
                .is_err()
        );
        assert!(
            budget
                .allocate(&[ContextDemand {
                    demand: 2,
                    minimum: 3,
                    maximum: 2
                }])
                .is_err()
        );
    }
    #[test]
    fn elastic_pool_and_projected_boundary() {
        let budget = ContextBudget::new(100, Some(20), 10);
        assert_eq!(budget.history_budget(), 70);
        assert_eq!(budget.skills_budget_tokens(), 70);
        assert!(!budget.needs_compaction(60, 10));
        assert!(budget.needs_compaction(60, 11));
    }
}
