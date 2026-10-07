//! The runtime prompt and the capability guidance that is always available.
//!
//! AX is a coding execution harness (see [`crate::harness`]): the current user
//! request defines the work, and the runtime drives requested deliverables to a
//! terminal state. This module owns the identity text injected unconditionally
//! on every run:
//!
//! - [`CORE_GUIDANCE`] — identity, the request/context boundary, the scope rule
//!   and the permission boundary.
//!
//! Per-capability rules are *not* here. Each tool owns its own
//! [`tool::Tool::guidance`], and the kernel assembles those into
//! `[ax-capability-guidance]`. Capability guidance describes how to use a
//! capability once the model has decided to use it; it never instructs the
//! model to use it. Execution and queue discipline live in
//! [`crate::harness::POLICY`].

/// Marker for the always-on runtime prompt.
pub const CORE_PREFIX: &str = "[ax-agent-runtime]\n";
/// Marker for the assembled per-capability guidance.
pub const CAPABILITY_PREFIX: &str = "[ax-capability-guidance]\n";

/// Identity, boundaries and the scope rule that applies to every run.
///
/// This text states who the agent is, what defines the work and how far the
/// work reaches. It is deliberately free of any repo/coding imperative beyond
/// what [`crate::harness::POLICY`] owns.
pub const CORE_GUIDANCE: &str = "\
[ax-agent-runtime]
You are AX, an agent runtime executing the current request. The current user request defines the work. Runtime environment information (cwd, workspace root, sandbox posture), workspace contents, memory, session history, existing files, previous artifacts and project instructions are supporting context: they are available when relevant, but their presence never means the user asked you to inspect, repair, continue, finish or modify them beyond the request.

Use the minimum sufficient actions required to satisfy the current user request, and stop as soon as the request is satisfied. Do not widen scope or re-verify unrelated work; continue past the first satisfactory answer only when the request is itself a long task or a durable task the user started is explicitly being resumed.

When you decide to use a capability, follow that capability's own guidance. Capabilities are options, not obligations. If you repeat the same call with identical arguments and learn nothing new, inspect the previous result, change approach, or conclude that the task is already satisfied.

Permissions, approvals and sandboxing are enforced by the runtime independently of this guidance. They authorize actions; they do not define your behaviour, and they must not be traded for behavioural assumptions.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_prompt_states_the_request_boundary_and_stop_rule() {
        assert!(CORE_GUIDANCE.contains("The current user request defines the work"));
        assert!(CORE_GUIDANCE.contains("supporting context"));
        assert!(CORE_GUIDANCE.contains("minimum sufficient actions"));
        assert!(CORE_GUIDANCE.contains("stop as soon as the request is satisfied"));
        // No repo/coding imperative leaked into the runtime prompt; execution
        // discipline belongs to the harness POLICY.
        for forbidden in ["checkout", "compile", "run the tests"] {
            assert!(
                !CORE_GUIDANCE.to_ascii_lowercase().contains(forbidden),
                "runtime prompt must not contain `{forbidden}`"
            );
        }
    }
}
