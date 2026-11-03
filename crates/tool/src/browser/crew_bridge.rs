//! Explicit desktop-owned transport; never fall back to another browser.
use crate::ToolError;
use serde_json::{Value, json};

pub(super) async fn call(scope: &str, payload: Value) -> Result<Value, ToolError> {
    let endpoint = std::env::var("AX_CREW_BROWSER_URL")
        .map_err(|_| ToolError::Execution("AXCrew browser bridge is not configured".into()))?;
    let url = reqwest::Url::parse(&endpoint)
        .map_err(|_| ToolError::Execution("Invalid AXCrew browser bridge URL".into()))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/browser"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ToolError::Execution(
            "AXCrew browser bridge must use its local loopback endpoint".into(),
        ));
    }
    let token = std::env::var("AX_CREW_BROWSER_TOKEN")
        .map_err(|_| ToolError::Execution("AXCrew browser bridge token missing".into()))?;
    if token.len() < 32 {
        return Err(ToolError::Execution(
            "Invalid AXCrew browser bridge token".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    let response = client
        .post(url)
        .bearer_auth(token)
        .json(&json!({"scope":scope,"input":payload["input"],"origin":payload["origin"]}))
        .send()
        .await
        .map_err(|_| {
            ToolError::Execution(
                "AXCrew built-in browser is unavailable; reopen AXCrew and retry".into(),
            )
        })?;
    if !response.status().is_success() {
        return Err(ToolError::Execution(format!(
            "AXCrew browser bridge rejected request ({})",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|len| len > 1024 * 1024)
    {
        return Err(ToolError::Execution(
            "AXCrew browser response exceeds limit".into(),
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    if bytes.len() > 1024 * 1024 {
        return Err(ToolError::Execution(
            "AXCrew browser response exceeds limit".into(),
        ));
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|e| ToolError::Execution(e.to_string()))?;
    if let Some(error) = value["error"].as_str() {
        return Err(ToolError::Execution(error.into()));
    }
    Ok(value)
}
