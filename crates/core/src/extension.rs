//! Optional session extensions, wired by the composition root only.
use crate::AgentError;
use async_trait::async_trait;
use std::sync::Arc;
use tool::Tool;

#[derive(Debug)]
pub struct ExtensionPrompt {
    pub text: String,
    pub context: Vec<String>,
    pub response: Option<String>,
}

#[async_trait]
pub trait RuntimeExtension: Send + Sync {
    async fn sync_context(
        &self,
        _messages: &[model::Message],
        _window: usize,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    async fn before_turn(&self, input: &str) -> Result<ExtensionPrompt, AgentError>;
    async fn after_turn(&self, output: Option<&str>, error: Option<&str>)
    -> Result<(), AgentError>;
    fn wrap_tool(&self, tool: Arc<dyn Tool>) -> Arc<dyn Tool>;
}
