//! All controller, child and optional Goal verifier requests share streaming/retry semantics.
use crate::{AgentError, AgentEvent, AgentKernel, ModelConnectionStatus};
use model::{ModelError, ModelRequest, ModelResponse};

impl AgentKernel {
    pub(crate) async fn request_with_retry(
        &self,
        request: ModelRequest,
        on_delta: impl FnMut(String) + Send,
        on_thinking: impl FnMut(String) + Send,
        on_connection: impl Fn(AgentEvent) + Send + Sync,
    ) -> Result<ModelResponse, AgentError> {
        self.request_with_retry_observed(request, on_delta, on_thinking, || {}, on_connection)
            .await
    }

    pub(crate) async fn request_with_retry_observed(
        &self,
        request: ModelRequest,
        mut on_delta: impl FnMut(String) + Send,
        mut on_thinking: impl FnMut(String) + Send,
        mut on_request: impl FnMut() + Send,
        on_connection: impl Fn(AgentEvent) + Send + Sync,
    ) -> Result<ModelResponse, AgentError> {
        let started = std::time::Instant::now();
        let mut attempts = 0;
        let mut retry_activity = None;
        let max_attempts = self.retry_policy.attempt_limit();
        loop {
            attempts += 1;
            on_request();
            let connection = |status, error| {
                on_connection(AgentEvent::ModelConnection {
                    status,
                    attempt: attempts,
                    max_attempts,
                    error,
                });
            };
            if attempts == 1 {
                connection(ModelConnectionStatus::Connecting, None);
            }
            let emitted = std::sync::atomic::AtomicBool::new(false);
            let first_output = tokio::sync::Notify::new();
            let mark_connected = || {
                if !emitted.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    connection(ModelConnectionStatus::Connected, None);
                    first_output.notify_one();
                }
            };
            let response = {
                let mut delta = |text: String| {
                    if !text.is_empty() {
                        mark_connected();
                    }
                    on_delta(text);
                };
                let mut thinking = |text: String| {
                    if !text.is_empty() {
                        mark_connected();
                    }
                    on_thinking(text);
                };
                let request =
                    self.provider
                        .complete_stream(request.clone(), &mut delta, &mut thinking);
                tokio::pin!(request);
                let remaining = std::time::Duration::from_millis(self.retry_policy.time_budget_ms)
                    .saturating_sub(started.elapsed());
                // Bound every attempt before its first output, including the first.
                // A healthy stream may run longer than the connection budget.
                tokio::select! {
                    biased;
                    response = &mut request => response,
                    () = first_output.notified() => request.await,
                    () = tokio::time::sleep(remaining) => {
                        Err(ModelError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "model connection timed out before receiving a response",
                        )))
                    }
                }
            };
            match response {
                Ok(response) => {
                    if !emitted.load(std::sync::atomic::Ordering::Relaxed) {
                        connection(ModelConnectionStatus::Connected, None);
                    }
                    return Ok(response);
                }
                Err(error) => {
                    // Replaying a partially streamed response would duplicate output.
                    if emitted.load(std::sync::atomic::Ordering::Relaxed) {
                        connection(ModelConnectionStatus::Failed, Some(error.to_string()));
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
                        connection(ModelConnectionStatus::Failed, Some(error.to_string()));
                        return Err(error.into());
                    };
                    on_connection(AgentEvent::ModelConnection {
                        status: ModelConnectionStatus::Retrying,
                        attempt: attempts + 1,
                        max_attempts,
                        error: Some(error.to_string()),
                    });
                    retry_activity.get_or_insert_with(|| {
                        crate::continuation::ActivityLease::new(&self.activity.retries)
                    });
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}
