//! Tool approval policy. The kernel asks; the policy decides.

use async_trait::async_trait;
use serde_json::Value;
use tool::{SafetyLevel, ToolPermission};

/// Decides whether one tool call may run. Implementations are supplied by the
/// composition root, so the kernel never learns about prompts or terminals.
#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    /// App/site grants do not follow shell/network/capability-wide approvals.
    async fn host_access(&self, _request: &tool::HostAccessRequest) -> tool::HostGrant {
        tool::HostGrant::Deny
    }
    async fn approve(&self, tool: &str, input: &Value, permission: ToolPermission) -> bool;
    /// Explicit Ask rules must not be bypassed by capability/session grants.
    fn capability_decision(
        &self,
        _capability: tool::Capability,
    ) -> Option<tool::PermissionDecision> {
        None
    }
    async fn ask(&self, _tool: &str, _input: &Value, _permission: ToolPermission) -> bool {
        false
    }
}

/// Default policy: only tools that declare themselves safe may run unattended.
pub struct DenyDangerous;

#[async_trait]
impl ApprovalPolicy for DenyDangerous {
    async fn approve(&self, _tool: &str, _input: &Value, permission: ToolPermission) -> bool {
        permission.safety == SafetyLevel::Safe
    }
}

/// Opt-in policy used by `--allow-dangerous` and tests.
pub struct AllowAll;

#[async_trait]
impl ApprovalPolicy for AllowAll {
    async fn approve(&self, _tool: &str, _input: &Value, _permission: ToolPermission) -> bool {
        true
    }
}
