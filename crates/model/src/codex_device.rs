//! Self-contained Codex (ChatGPT) device-code login.
//!
//! Ported from OpenAI's open-source Codex CLI (`codex-rs/login`, MIT) so AX can
//! complete a Codex OAuth login without shelling out to an installed `codex`
//! binary. Flow: request a device code → show the user a verification URL +
//! one-time code → poll the token endpoint → exchange the authorization code
//! for tokens. The caller persists them in AX's provider-scoped credential
//! store; AX never mutates Codex CLI's own credential cache.
#![allow(clippy::doc_markdown)]

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ModelError;
use crate::OAuthCredential;

const ISSUER: &str = "https://auth.openai.com";
/// Codex's public OAuth client id (from `codex-rs/login/src/auth/manager.rs`).
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// Poll for at most 15 minutes, matching the device code expiry.
const MAX_POLL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug)]
#[allow(clippy::struct_field_names)]
pub struct DeviceAuth {
    pub verification_url: String,
    pub user_code: String,
    device_auth_id: String,
    interval: u64,
    client: reqwest::Client,
}

/// Tokens obtained after a successful device-code exchange.
#[derive(Clone, Debug)]
#[allow(clippy::struct_field_names)]
pub struct DeviceTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: Option<String>,
    pub expires_at: u64,
}

#[derive(Serialize)]
struct UserCodeReq {
    client_id: String,
}

#[derive(Deserialize)]
struct UserCodeResp {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    /// The server sends the interval as a numeric string (e.g. `"5"`), so it is
    /// deserialized leniently (mirrors Codex's `deserialize_interval`).
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .ok_or_else(|| serde::de::Error::custom("interval must be a number or numeric string"))
}

#[derive(Serialize)]
struct TokenPollReq {
    device_auth_id: String,
    user_code: String,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct CodeSuccessResp {
    authorization_code: String,
    #[allow(dead_code)]
    code_challenge: String,
    code_verifier: String,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct TokenResp {
    access_token: String,
    refresh_token: Option<String>,
    id_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

const fn default_expires_in() -> u64 {
    3_600
}

#[derive(Deserialize)]
struct RefreshTokenResp {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

/// Refresh an AX-owned OpenAI Codex OAuth credential. This mirrors pi's
/// `openai-codex` provider refresh operation and never touches Codex CLI's
/// credential store.
///
/// # Errors
///
/// Returns an error when the token endpoint cannot be reached or rejects the
/// refresh token.
pub async fn refresh_oauth(credential: &OAuthCredential) -> Result<OAuthCredential, ModelError> {
    let response = reqwest::Client::new()
        .post(format!("{ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", credential.refresh.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await?
        .error_for_status()?;
    let tokens: RefreshTokenResp = response.json().await?;
    let account_id =
        account_id_from_token(&tokens.access_token).or_else(|| credential.account_id.clone());
    Ok(OAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(tokens.expires_in),
        account_id,
    })
}

/// Start a device-code login: request a user code and polling interval from the
/// auth server. Show `prompt()` to the user, then call `poll_and_exchange()`.
///
/// # Errors
///
/// Returns an error if the auth server rejects the device-code request.
pub async fn begin() -> Result<DeviceAuth, ModelError> {
    // auth.openai.com's WAF rejects generic HTTP client user agents (403) but
    // accepts Codex's own; mirror it. The `system-proxy` reqwest feature is
    // enabled so this client also honors the Windows system proxy (e.g. a
    // local Clash on 127.0.0.1:7890) — a direct connection gets 403 for this
    // endpoint, exactly like Codex's client which routes via the system proxy.
    let client = reqwest::Client::builder()
        .user_agent("codex-cli/0.80.7")
        .build()?;
    let url = format!("{ISSUER}/api/accounts/deviceauth/usercode");
    let response = client
        .post(&url)
        .json(&UserCodeReq {
            client_id: CLIENT_ID.to_owned(),
        })
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(ModelError::InvalidResponse(format!(
            "device code request failed with status {}",
            response.status()
        )));
    }
    let code: UserCodeResp = response.json().await?;
    Ok(DeviceAuth {
        verification_url: format!("{ISSUER}/codex/device"),
        user_code: code.user_code,
        device_auth_id: code.device_auth_id,
        interval: code.interval.max(1),
        client,
    })
}

impl DeviceAuth {
    /// Human-readable instructions to open in a browser (mirrors Codex's
    /// device-code prompt: link + one-time code).
    #[must_use]
    pub fn prompt(&self) -> String {
        format!(
            "Follow these steps to sign in with ChatGPT using device code authorization:\n\
\n\
1. Open this link in your browser and sign in to your account\n   {}\n\
\n\
2. Enter this one-time code (expires in 15 minutes)\n   {}\n",
            self.verification_url, self.user_code
        )
    }

    /// Poll the token endpoint until the user authorizes, exchange the
    /// authorization code for tokens, and return them.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, a non-2xx token response, or if the
    /// user never authorizes within 15 minutes.
    pub async fn poll_and_exchange(&self) -> Result<DeviceTokens, ModelError> {
        let url = format!("{ISSUER}/api/accounts/deviceauth/token");
        let started = std::time::Instant::now();
        loop {
            let response = self
                .client
                .post(&url)
                .json(&TokenPollReq {
                    device_auth_id: self.device_auth_id.clone(),
                    user_code: self.user_code.clone(),
                })
                .send()
                .await?;
            if response.status().is_success() {
                let code: CodeSuccessResp = response.json().await?;
                return self.exchange(&code).await;
            }
            let status = response.status();
            if matches!(status.as_u16(), 403 | 404) {
                if started.elapsed() >= MAX_POLL {
                    return Err(ModelError::InvalidResponse(
                        "device auth timed out after 15 minutes".to_owned(),
                    ));
                }
                let wait = Duration::from_secs(self.interval)
                    .min(MAX_POLL.saturating_sub(started.elapsed()));
                tokio::time::sleep(wait).await;
                continue;
            }
            return Err(ModelError::InvalidResponse(format!(
                "device auth failed with status {status}"
            )));
        }
    }

    async fn exchange(&self, code: &CodeSuccessResp) -> Result<DeviceTokens, ModelError> {
        let token_endpoint = format!("{ISSUER}/oauth/token");
        let response = self
            .client
            .post(&token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", code.authorization_code.as_str()),
                ("redirect_uri", REDIRECT_URI),
                ("code_verifier", code.code_verifier.as_str()),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ModelError::InvalidResponse(format!(
                "token exchange failed with status {}",
                response.status()
            )));
        }
        let tokens: TokenResp = response.json().await?;
        // Pi and Codex read the namespaced account claim from the access token.
        // Keep the id-token fallback for older auth-server responses.
        let account_id = account_id_from_token(&tokens.access_token)
            .or_else(|| account_id_from_token(&tokens.id_token));
        Ok(DeviceTokens {
            id_token: tokens.id_token,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token.unwrap_or_default(),
            account_id,
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_add(tokens.expires_in),
        })
    }
}

/// Decode the `chatgpt_account_id` claim from an `id_token` JWT payload.
pub fn account_id_from_token(token: &str) -> Option<String> {
    let claims = decode_jwt_payload(token).ok()?;
    claims
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .or_else(|| claims.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn decode_jwt_payload(token: &str) -> Result<Value, ModelError> {
    use base64::Engine;
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| ModelError::InvalidResponse("id_token has no payload segment".to_owned()))?;
    let mut b64 = payload.replace('-', "+").replace('_', "/");
    while b64.len() % 4 != 0 {
        b64.push('=');
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|error| ModelError::InvalidResponse(format!("invalid id_token: {error}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| ModelError::InvalidResponse(format!("invalid id_token: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_account_id_from_jwt() {
        use base64::Engine;
        // header.payload.signature with a real-ish payload claim.
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"u1","chatgpt_account_id":"acct_123"}"#);
        let id_token = format!("header.{payload}.sig");
        assert_eq!(
            account_id_from_token(&id_token).as_deref(),
            Some("acct_123")
        );
    }

    #[test]
    fn extracts_account_id_from_codex_namespaced_claim() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_nested"}}"#);
        let id_token = format!("header.{payload}.sig");
        assert_eq!(
            account_id_from_token(&id_token).as_deref(),
            Some("acct_nested")
        );
    }

    /// Reach the real auth server and obtain a fresh device code. This creates
    /// a harmless one-time code that expires in 15 minutes; it does not
    /// authorize anything. Run manually with `cargo test -p model -- --ignored`.
    #[tokio::test]
    #[ignore = "requires network access to auth.openai.com"]
    async fn begin_reaches_auth_server() {
        let auth = begin().await.expect("device code request should succeed");
        assert!(!auth.verification_url.is_empty());
        assert!(!auth.user_code.is_empty());
        eprintln!(
            "verification_url={} user_code={}",
            auth.verification_url, auth.user_code
        );
    }
}
