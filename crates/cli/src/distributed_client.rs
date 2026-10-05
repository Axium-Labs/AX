//! Opt-in collaboration transport; credentials are never included in tool output.
use anyhow::{Result, ensure};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct Rejected {
    pub(crate) status: reqwest::StatusCode,
    detail: String,
}
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Crew: {}", self.detail)
    }
}
impl std::error::Error for Rejected {}

#[derive(Clone)]
pub(crate) struct Client {
    pub(crate) gateway: String,
    token: String,
    http: reqwest::Client,
}
impl Client {
    pub(crate) fn new(gateway: &str, token: String) -> Result<Self> {
        let url = reqwest::Url::parse(gateway)?;
        ensure!(
            url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
            "distributed transport requires HTTPS except loopback"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid gateway URL"
        );
        ensure!(!token.is_empty(), "instance token required");
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none());
        if url.scheme() == "http" {
            builder = builder.no_proxy();
        }
        Ok(Self {
            gateway: gateway.trim_end_matches('/').into(),
            token,
            http: builder.build()?,
        })
    }
    pub(crate) fn from_env() -> Option<Self> {
        Self::new(
            &std::env::var("AX_DISTRIBUTED_GATEWAY").ok()?,
            std::env::var("AX_DISTRIBUTED_TOKEN").ok()?,
        )
        .ok()
    }
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
    pub(crate) async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method.parse()?, format!("{}{path}", self.gateway))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success() {
            return Err(Rejected {
                status,
                detail: value["error"].as_str().unwrap_or("request rejected").into(),
            }
            .into());
        }
        Ok(value)
    }
}
