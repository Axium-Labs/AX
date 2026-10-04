//! All controller, child and optional stop-guard requests share streaming/retry semantics.
use crate::{AgentError, AgentKernel};
use model::{ModelError, ModelRequest, ModelResponse};

impl AgentKernel {
    pub(crate) async fn request_with_retry(
        &self,
        request: ModelRequest,
        on_delta: impl FnMut(String) + Send,
        on_thinking: impl FnMut(String) + Send,
    ) -> Result<ModelResponse, AgentError> {
        self.request_with_retry_observed(request, on_delta, on_thinking, || {})
            .await
    }

    pub(crate) async fn request_with_retry_observed(
        &self,
        request: ModelRequest,
        mut on_delta: impl FnMut(String) + Send,
        mut on_thinking: impl FnMut(String) + Send,
        mut on_request: impl FnMut() + Send,
    ) -> Result<ModelResponse, AgentError> {
        let started = std::time::Instant::now();
        let mut attempts = 0;
        let mut retry_activity = None;
        loop {
            attempts += 1;
            on_request();
            let emitted = std::sync::atomic::AtomicBool::new(false);
            let response = {
                let mut delta = |text: String| {
                    if !text.is_empty() {
                        emitted.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    on_delta(text);
                };
                let mut thinking = |text: String| {
                    if !text.is_empty() {
                        emitted.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    on_thinking(text);
                };
                let request =
                    self.provider
                        .complete_stream(request.clone(), &mut delta, &mut thinking);
                if attempts == 1 {
                    request.await
                } else {
                    let remaining =
                        std::time::Duration::from_millis(self.retry_policy.time_budget_ms)
                            .saturating_sub(started.elapsed());
                    tokio::time::timeout(remaining, request)
                        .await
                        .unwrap_or_else(|_| {
                            Err(ModelError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "provider retry time budget exhausted",
                            )))
                        })
                }
            };
            match response {
                Ok(response) => return Ok(response),
                Err(error) => {
                    // Replaying a partially streamed response would duplicate output.
                    if emitted.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(error.into());
                    }
                    let entropy = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_nanos();
                    let Some(delay) = self.retry_policy.delay(
                        &error,
                        attempts,
                        started.elapsed(),
                        u64::from(entropy),
                    ) else {
                        return Err(error.into());
                    };
                    retry_activity.get_or_insert_with(|| {
                        crate::continuation::ActivityLease::new(&self.activity.retries)
                    });
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}
