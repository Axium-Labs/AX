use crate::ModelError;
use futures_util::StreamExt;
use serde_json::Value;

pub(super) async fn success(response: reqwest::Response) -> Result<reqwest::Response, ModelError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let retry_after = crate::retry::retry_after_header(response.headers());
    let message = response.text().await?.chars().take(800).collect();
    Err(ModelError::HttpResponse {
        status,
        message,
        retry_after,
    })
}

/// Buffer bytes, not decoded strings: a network chunk may split a UTF-8 code
/// point. SSE events may contain multiple data lines or use CRLF separators.
pub(super) async fn sse(
    response: reqwest::Response,
    mut event: impl FnMut(Value) -> Result<(), ModelError> + Send,
) -> Result<(), ModelError> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk?);
        if pending.len() > 8 * 1024 * 1024 {
            return Err(ModelError::InvalidResponse(
                "SSE event exceeds 8 MiB".into(),
            ));
        }
        while let Some((index, size)) = delimiter(&pending) {
            parse(&pending[..index], &mut event)?;
            pending.drain(..index + size);
        }
    }
    if !pending.is_empty() {
        parse(&pending, &mut event)?;
    }
    Ok(())
}

fn delimiter(bytes: &[u8]) -> Option<(usize, usize)> {
    for i in 0..bytes.len() {
        if bytes[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
        if bytes[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
    }
    None
}

fn parse(
    bytes: &[u8],
    event: &mut impl FnMut(Value) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    let text =
        std::str::from_utf8(bytes).map_err(|e| ModelError::InvalidResponse(e.to_string()))?;
    let data = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|s| s.strip_prefix(' ').unwrap_or(s))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let value: Value = serde_json::from_str(&data)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid SSE JSON: {e}")))?;
    if value.get("error").is_some() || value["type"] == "error" {
        let error = value.get("error").unwrap_or(&value);
        let status = error["code"]
            .as_u64()
            .filter(|c| (400..600).contains(c))
            .and_then(|c| u16::try_from(c).ok())
            .or_else(|| match error["type"].as_str() {
                Some("overloaded_error") => Some(503),
                Some("rate_limit_error") => Some(429),
                Some("api_error") => Some(500),
                _ => None,
            });
        let message = format!("provider stream error: {error}");
        return Err(match status {
            Some(status) => ModelError::HttpResponse {
                status,
                message,
                retry_after: None,
            },
            None => ModelError::InvalidResponse(message),
        });
    }
    event(value)
}
