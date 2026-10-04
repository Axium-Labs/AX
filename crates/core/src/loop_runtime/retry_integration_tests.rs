//! Retry behaviour of one model step, exercised through the public turn API.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use model::{ModelError, ModelProvider, ModelRequest};
use tool::ToolRegistry;

use crate::{AgentKernel, AllowAll};

struct Flaky {
    calls: AtomicUsize,
    code: u16,
    partial: bool,
    hang_retry: bool,
}

#[async_trait]
impl ModelProvider for Flaky {
    fn name(&self) -> &'static str {
        "retry-test"
    }
    fn model_id(&self) -> &'static str {
        "retry-test"
    }
    fn context_window(&self) -> usize {
        32000
    }
    async fn complete(&self, _: ModelRequest) -> Result<model::ModelResponse, ModelError> {
        unreachable!()
    }
    async fn complete_stream(
        &self,
        _: ModelRequest,
        delta: &mut (dyn FnMut(String) + Send),
        _: &mut (dyn FnMut(String) + Send),
    ) -> Result<model::ModelResponse, ModelError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 || self.partial {
            if self.partial {
                delta("partial".into());
            }
            return Err(ModelError::HttpResponse {
                status: self.code,
                message: String::new(),
                retry_after: Some(std::time::Duration::ZERO),
            });
        }
        if self.hang_retry {
            std::future::pending::<()>().await;
        }
        Ok(model::ModelResponse {
            provider_metadata: None,
            content: "done".into(),
            tool_calls: vec![],
            usage: None,
            finish_reason: None,
        })
    }
}

#[tokio::test]
async fn retries_transient_errors_only_and_never_replays_partial_stream() {
    for (code, partial, expected) in [
        (503, false, 2),
        (429, false, 2),
        (400, false, 1),
        (401, false, 1),
        (403, false, 1),
        (503, true, 1),
    ] {
        let provider = Arc::new(Flaky {
            calls: AtomicUsize::new(0),
            code,
            partial,
            hang_retry: false,
        });
        let mut kernel = AgentKernel::new(
            provider.clone(),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(AllowAll),
        );
        let result = kernel.run_turn("request", |_| {}).await;
        assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
        assert_eq!(result.is_ok(), expected == 2);
    }
}

#[tokio::test]
async fn retry_timeout_respects_remaining_time_budget() {
    let provider = Arc::new(Flaky {
        calls: AtomicUsize::new(0),
        code: 503,
        partial: false,
        hang_retry: true,
    });
    let mut kernel = AgentKernel::new(
        provider.clone(),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(AllowAll),
    );
    kernel.configure_retry(model::RetryPolicy {
        max_attempts: 4,
        time_budget_ms: 10,
        base_delay_ms: 0,
        max_delay_ms: 0,
    });
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            kernel.run_turn("request", |_| {})
        )
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}
