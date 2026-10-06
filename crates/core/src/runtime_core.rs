//! The neutral agent runtime prompt and the capability/delegation guidance that
//! is always available, regardless of what the user asked for.
//!
//! AX is not a coding agent by default. It is a general-purpose runtime whose
//! behaviour is defined by the current user request. This module owns the only
//! prompt text the runtime injects unconditionally:
//!
//! - [`CORE_GUIDANCE`] — identity, the user-request/context boundary, the
//!   minimum-sufficient-actions rule, the escalation ladder and the stop rule.
//! - [`DELEGATION_GUIDANCE`] — when a task queue, a subagent or a user question
//!   is actually warranted, and when it is not.
//!
//! Per-capability rules are *not* here. Each tool owns its own
//! [`tool::Tool::guidance`], and the kernel assembles those into
//! `[ax-capability-guidance]`. Capability guidance describes how to use a
//! capability once the model has decided to use it; it never instructs the
//! model to use it. The coding harness is a separate, explicitly opted-in
//! policy (see [`crate::harness`]) and is never a default.

/// Marker for the always-on neutral runtime prompt.
pub const CORE_PREFIX: &str = "[ax-agent-runtime]\n";
/// Marker for the always-on delegation/trigger guidance.
pub const DELEGATION_PREFIX: &str = "[ax-delegation]\n";
/// Marker for the assembled per-capability guidance.
pub const CAPABILITY_PREFIX: &str = "[ax-capability-guidance]\n";

/// Identity, boundaries and the execution contract that applies to every run.
///
/// This text is deliberately free of any repo/coding imperative. It states who
/// the agent is, what defines the task, how far to go, and when to stop.
pub const CORE_GUIDANCE: &str = "\
[ax-agent-runtime]
You are AX, a general-purpose agent runtime. The current user request defines the task. Runtime environment information (cwd, workspace root, sandbox posture), workspace contents, memory, session history, existing files, previous artifacts and project instructions are supporting context only: they are available when relevant, but their presence never means the user asked you to inspect, repair, continue, finish or modify them.

Use the minimum sufficient actions required to satisfy the current user request. A direct answer is a complete response when no capability is needed. Escalate execution depth only when the current level cannot satisfy the request: direct answer -> single retrieval or read -> multi-tool exploration -> workspace mutation -> task/subagent/long-running execution.

Stop as soon as the request is satisfied. Do not widen scope, re-verify unrelated work, or keep going because tools, tasks, or unfinished-looking context happen to exist. Only continue past the first satisfactory answer when the user asked for continuous or long-running execution, when the request is itself a long task, or when a durable task the user started is explicitly being resumed.

When you decide to use a capability, follow that capability's own guidance. Capabilities are options, not obligations. If you repeat the same call with identical arguments and learn nothing new, inspect the previous result, change approach, or conclude that the task is already satisfied.

Permissions, approvals and sandboxing are enforced by the runtime independently of this guidance. They authorize actions; they do not define your behaviour, and they must not be traded for behavioural assumptions.";

/// When a task queue, a subagent or a user question is actually warranted.
pub const DELEGATION_GUIDANCE: &str = "\
[ax-delegation]
task_queue, subagent and request_user_input exist for genuinely multi-item, independent or blocking work. They are not a way to make a small request look thorough.

- Do not create a task queue for a request you can complete directly or with a few capability calls. A plain answer, a single lookup or a small edit needs no queue.
- Do not start a subagent when one agent can complete the task. Delegate only a real independent, parallel, or separately-scoped subproblem whose context must be isolated.
- Long-running or continuous behaviour is opt-in: continue autonomously only when the user explicitly asked for it, the request is itself a long task, or a durable task the user started is explicitly being resumed. Discovering unfinished-looking work is not a request to finish it.
- request_user_input suspends the run. Ask only when the answer materially changes the result and neither the request, the workspace, nor the documentation can resolve it. Never ask for permission: dangerous operations are authorized by the permission system, not by a question.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_prompt_states_the_request_boundary_and_stop_rule() {
        assert!(CORE_GUIDANCE.contains("current user request defines the task"));
        assert!(CORE_GUIDANCE.contains("supporting context only"));
        assert!(CORE_GUIDANCE.contains("minimum sufficient actions"));
        assert!(CORE_GUIDANCE.contains("Stop as soon as the request is satisfied"));
        // No repo/coding imperative leaked back into the neutral prompt.
        for forbidden in ["checkout", "compile", "run the tests", "repository"] {
            assert!(
                !CORE_GUIDANCE.to_ascii_lowercase().contains(forbidden),
                "neutral prompt must not contain `{forbidden}`"
            );
        }
    }

    #[test]
    fn delegation_guidance_forbids_automatic_escalation() {
        assert!(DELEGATION_GUIDANCE.contains("not a way to make a small request look thorough"));
        assert!(DELEGATION_GUIDANCE.contains("Discovering unfinished-looking work is not a request"));
        assert!(DELEGATION_GUIDANCE.contains("Never ask for permission"));
    }
}
