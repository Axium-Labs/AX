//! Configurable limits for one user turn.
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
