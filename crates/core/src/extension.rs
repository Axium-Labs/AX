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
    /// Host-owned durability/fencing. Only explicit optional Goals call this seam.
    async fn checkpoint_goal(
        &self,
        _queue: &crate::task_queue::TaskQueue,
        _messages: &[model::Message],
    ) -> Result<(), AgentError> {
        Ok(())
    }
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

pub(crate) struct ExtensionChain(pub Arc<dyn RuntimeExtension>, pub Arc<dyn RuntimeExtension>);
#[async_trait]
impl RuntimeExtension for ExtensionChain {
    async fn checkpoint_goal(
        &self,
        queue: &crate::task_queue::TaskQueue,
        messages: &[model::Message],
    ) -> Result<(), AgentError> {
        self.0.checkpoint_goal(queue, messages).await?;
        self.1.checkpoint_goal(queue, messages).await
    }
    async fn sync_context(
        &self,
        messages: &[model::Message],
        window: usize,
    ) -> Result<(), AgentError> {
        self.0.sync_context(messages, window).await?;
        self.1.sync_context(messages, window).await
    }
    async fn before_turn(&self, input: &str) -> Result<ExtensionPrompt, AgentError> {
        let first = self.0.before_turn(input).await?;
        if first.response.is_some() {
            return Ok(first);
        }
        let mut second = self.1.before_turn(&first.text).await?;
        second.context.splice(0..0, first.context);
        Ok(second)
    }
    async fn after_turn(
        &self,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), AgentError> {
        self.0.after_turn(output, error).await?;
        self.1.after_turn(output, error).await
    }
    fn wrap_tool(&self, tool: Arc<dyn Tool>) -> Arc<dyn Tool> {
        self.1.wrap_tool(self.0.wrap_tool(tool))
    }
}

impl crate::AgentKernel {
    pub(crate) async fn checkpoint_goal_host(&self) -> Result<(), AgentError> {
        if let Some(queue) = self.task_queue.as_ref().filter(|q| q.goal.is_some())
            && let Some(extension) = &self.extension
        {
            extension.checkpoint_goal(queue, &self.messages).await?;
        }
        Ok(())
    }
}
