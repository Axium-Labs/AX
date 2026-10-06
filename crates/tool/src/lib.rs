//! Tool contracts, registry, and lightweight built-in tools.
pub mod environment;
mod task_source;
pub use environment::EnvironmentContext;
pub use task_source::TaskSourceTool;

mod filesystem;
mod sandboxed;
pub use sandbox::SandboxMode;
pub use sandboxed::{SandboxedTool, sandbox_worker};
mod workspace;
pub use workspace::{RunContext, WorkspaceTool};
mod result;
pub use result::{ResultReader, ToolResult, path_error};
mod discovery;
mod find;
mod patch;
mod search;
pub use find::FindFilesTool;
mod web;
pub use web::{
    BochaSearch, BraveSearch, DuckDuckGoSearch, FetchError, FetchErrorKind, MAX_FETCH_URLS,
    MAX_PAGE_CHARS, MAX_QUERIES, MAX_TOTAL_CHARS, ProviderStats, ProviderTimeout, SearchCandidate,
    SearchConfig, SearchProvider, SearchResult, SearchRouter, SearxngSearch, WebTool,
};
mod view_image;
pub use patch::PatchTool;
pub use search::SearchTool;
pub use view_image::ViewImageTool;
mod permission;
mod resources;
pub use resources::{Resource, ResourceAccess};
mod shell;
pub mod ssh;
pub use ssh::{SshContext, SshHost, SshTool};
pub mod telemetry;
pub use permission::{
    Capability, PermissionDecision, PermissionProfile, PermissionRule, PermissionStore,
    ProfileDecision, RuleMatcher, SandboxBoundary, ToolPermission, resolve_profiles,
};

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

pub use filesystem::FilesystemTool;
pub use shell::ShellTool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyLevel {
    Safe,
    RequiresApproval,
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error(transparent)]
    SandboxViolation(#[from] sandbox::SandboxViolation),
    #[error("global execution blocker: {0}")]
    GlobalBlocked(String),
    #[error("unknown tool: {0}")]
    Unknown(String),
    #[error("invalid tool input: {0}")]
    InvalidInput(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("web fetch failed for every url: {}", serde_json::to_string(.0).unwrap_or_default())]
    WebFetch(Vec<FetchError>),
    #[error("permission denied for tool: {0}")]
    PermissionDenied(String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ToolOutput {
    Text(String),
    Image {
        description: String,
        media_type: String,
        data: String,
    },
}

impl From<std::io::Error> for ToolError {
    fn from(error: std::io::Error) -> Self {
        Self::Execution(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InheritanceClass {
    Tools,
    Memory,
    Skills,
    Mcp,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn inheritance_class(&self) -> InheritanceClass {
        InheritanceClass::Tools
    }
    fn fork_skills(&self, _selected: Option<&[String]>) -> Option<Arc<dyn Tool>> {
        None
    }
    fn fork_memory(&self, _input: &str, _readonly: bool) -> Option<Arc<dyn Tool>> {
        None
    }
    /// Explicit runtime placement; unbound extensions cannot run in confinement.
    /// Remote channels may opt out of the fixed local tool concurrency budget.
    /// Resource dependencies and permissions continue to apply.
    fn independent_remote_execution(&self) -> bool {
        false
    }
    fn execution_boundary(&self) -> ExecutionBoundary {
        ExecutionBoundary::Unbound
    }
    /// Opt in only when all state is rebound to the child's scope. Unbound tools
    /// (including parent MCP processes) are not silently shared across children.
    fn fork_for_run(&self, _context: &RunContext) -> Option<Arc<dyn Tool>> {
        None
    }
    /// Declared paths belong to tool-owned runtime storage, never caller-chosen files.
    fn runtime_owned_resources(&self) -> bool {
        false
    }
    /// Whether this tool recursively traverses a filesystem subtree.
    fn recursive_search(&self) -> bool {
        false
    }
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn safety(&self, input: &Value) -> SafetyLevel;
    /// Permission category determined from structured operation input, never tool names.
    fn capability(&self, input: &Value) -> Capability;
    fn permission(&self, input: &Value) -> ToolPermission {
        ToolPermission {
            capability: self.capability(input),
            safety: self.safety(input),
        }
    }
    /// Explicit effects used by the runtime scheduler. Permissions and safety
    /// do not imply independence: undeclared effects use a global write lock.
    fn resources(&self, _input: &Value) -> Vec<ResourceAccess> {
        vec![ResourceAccess::exclusive()]
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError>;
    /// Network policy must reach the transport. Unknown transports fail closed.
    async fn execute_output_constrained(
        &self,
        input: Value,
        profiles: &[PermissionProfile],
    ) -> Result<ToolOutput, ToolError> {
        if self.capability(&input) == Capability::Network
            && profiles.iter().any(PermissionProfile::has_network_rules)
        {
            return Err(ToolError::PermissionDenied(
                "tool transport cannot enforce domain rules".into(),
            ));
        }
        self.execute_output(input).await
    }
    async fn execute_output(&self, input: Value) -> Result<ToolOutput, ToolError> {
        self.execute(input).await.map(ToolOutput::Text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionBoundary {
    Unbound,
    WorkspaceWorker,
    Sandboxed,
    Remote,
    RuntimeOwned,
}

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    mode: sandbox::SandboxMode,
}
impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: HashMap::new(),
            mode: sandbox::SandboxManager::configured_mode().unwrap_or_default(),
        }
    }
}

/// The kernel's baseline tool set: the runtime-owned `tool_output` reader,
/// and no `view_image` for a provider that cannot see.
///
/// Both the kernel constructor and the run's pre-kernel estimate call this, so
/// the registry that is measured and the registry that is sent are the same.
pub fn kernel_baseline(tools: &mut ToolRegistry, vision: bool) {
    if !vision {
        tools.remove("view_image");
    }
    tools.register(result::ResultReader::default());
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn with_mode(mode: sandbox::SandboxMode) -> Self {
        Self {
            tools: HashMap::new(),
            mode,
        }
    }

    /// Applies the registry's execution-boundary policy to one tool. This is the
    /// single place that decides what a declared boundary means:
    ///
    /// - `WorkspaceWorker` is wrapped in the sandbox executor for `root`;
    /// - `Unbound` is replaced by a denying binding, because an extension with
    ///   no confinement binding may not touch the host;
    /// - `Sandboxed` already carries its own manager (for example a stdio MCP
    ///   transport) and is passed through unchanged;
    /// - `Remote` and `RuntimeOwned` never reach local OS effects.
    ///
    /// Confined wrapping only happens when the registry's mode requires it.
    fn bind(&self, tool: Arc<dyn Tool>, root: &std::path::Path) -> Arc<dyn Tool> {
        let confined = self.mode != sandbox::SandboxMode::Off;
        match tool.execution_boundary() {
            ExecutionBoundary::WorkspaceWorker if confined => {
                Arc::new(SandboxedTool::new(tool, root.to_path_buf()))
            }
            ExecutionBoundary::Unbound if confined => Arc::new(sandboxed::UnboundTool(tool)),
            _ => tool,
        }
    }

    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        let name = tool.name().to_owned();
        let root = std::env::current_dir().unwrap_or_default();
        let tool = self.bind(Arc::new(tool), &root);
        self.tools.insert(name, tool);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .get(name)
            .map(|tool| self.bind(tool.clone(), &std::env::current_dir().unwrap_or_default()))
    }

    #[must_use]
    pub fn fork_for_run(&self, context: &RunContext) -> Self {
        let tools = self
            .tools
            .values()
            .filter_map(|tool| tool.fork_for_run(context))
            .map(|tool| {
                let tool = self.bind(tool, &context.cwd);
                (tool.name().to_owned(), tool)
            })
            .collect();
        Self {
            tools,
            mode: self.mode,
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.tools.remove(name);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }

    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<_> = self.tools.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_returns_sorted_names() {
        let mut registry = ToolRegistry::new();
        registry.register(ShellTool);
        registry.register(FilesystemTool);
        assert_eq!(registry.names(), vec!["filesystem", "shell"]);
    }

    #[test]
    fn shell_always_requires_approval() {
        assert_eq!(
            ShellTool.safety(&serde_json::json!({ "command": "pwd" })),
            SafetyLevel::RequiresApproval
        );
    }

    /// The model picks a discovery tool from its description, so the split
    /// between browsing, finding and searching has to stay explicit.
    #[test]
    fn tool_descriptions_state_the_discovery_split() {
        let root = std::path::PathBuf::from(".");
        let find = FindFilesTool::new(root.clone());
        assert!(
            find.description()
                .contains("without reading their contents")
        );
        assert!(find.description().contains("recursive shell scan"));

        let search = SearchTool::new(root.clone());
        assert!(search.description().contains("grep"));
        assert!(
            search
                .description()
                .contains("No match is a successful empty result")
        );

        assert!(
            FilesystemTool
                .description()
                .contains("not a discovery tool")
        );
        assert!(FilesystemTool.description().contains("find_files"));
        assert!(ShellTool.description().contains("Prefer find_files/glob"));
        assert!(
            ShellTool
                .description()
                .contains("Shell scans are appropriate when explicitly requested")
        );
    }

    #[test]
    fn discovery_tools_are_read_only_and_recursive() {
        let root = std::path::PathBuf::from(".");
        let mut registry = ToolRegistry::with_mode(sandbox::SandboxMode::Off);
        registry.register(FindFilesTool::new(root.clone()));
        registry.register(FindFilesTool::glob(root.clone()));
        registry.register(SearchTool::new(root));
        for name in ["find_files", "glob", "search"] {
            let tool = registry.get(name).unwrap();
            assert!(tool.recursive_search(), "{name} traverses a subtree");
            let access = tool.resources(&serde_json::json!({"query":"x"}));
            assert!(!access.is_empty(), "{name} declares its scope");
            assert!(
                !access.iter().any(|access| access.write),
                "{name} is read-only"
            );
        }
    }
}
