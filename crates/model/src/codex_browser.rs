//! AX-owned Codex browser OAuth (PKCE), with a short-lived loopback callback.
//! Protocol matches `OpenAI`'s MIT-licensed `codex-rs/login/src/server.rs`.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{
    DeviceTokens, ModelError,
    codex_device::{CLIENT_ID, ISSUER, TokenResp},
};

const TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Dropping this value releases the callback port, including cancellation.
pub struct BrowserLogin {
    pub auth_url: String,
    listener: TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
    client: reqwest::Client,
    token_endpoint: String,
}

fn invalid(message: impl Into<String>) -> ModelError {
    ModelError::InvalidResponse(message.into())
}

impl BrowserLogin {
    /// Prepare a browser URL without opening a browser or reading Codex's cache.
    /// # Errors
    /// Fails if both registered loopback ports are occupied.
    pub async fn begin() -> Result<Self, ModelError> {
        let listener = match TcpListener::bind(("127.0.0.1", 1455)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind(("127.0.0.1", 1457)).await.map_err(|_| {
                invalid(
                    "Codex callback ports 1455 and 1457 are unavailable; choose device-code login",
                )
            })?,
        };
        let port = listener
            .local_addr()
            .map_err(|_| invalid("Cannot read callback port"))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let state = uuid::Uuid::new_v4().to_string();
        let auth_url = authorize_url(&redirect_uri, &verifier, &state);
        let client = reqwest::Client::builder()
            .user_agent("codex-cli/0.80.7")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            auth_url,
            listener,
            redirect_uri,
            verifier,
            state,
            client,
            token_endpoint: format!("{ISSUER}/oauth/token"),
        })
    }

    /// Wait for a matching callback, exchange its code, and return AX credentials.
    /// # Errors
    /// Fails on denial, timeout or token-exchange failure. No tokens enter output.
    pub async fn wait(self) -> Result<DeviceTokens, ModelError> {
        tokio::time::timeout(TIMEOUT, self.wait_inner())
            .await
            .map_err(|_| invalid("Codex browser login timed out after 15 minutes"))?
    }

    async fn wait_inner(&self) -> Result<DeviceTokens, ModelError> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|_| invalid("Cannot accept login callback"))?;
            let Ok(Ok(request)) =
                tokio::time::timeout(Duration::from_secs(5), read_request(&mut stream)).await
            else {
                continue;
            };
            let target = request.lines().next().and_then(|line| {
                let mut parts = line.split_whitespace();
                (parts.next() == Some("GET"))
                    .then(|| parts.next())
                    .flatten()
            });
            let callback = target.and_then(|target| callback_code(target, &self.state));
            let Some(callback) = callback else {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    "Invalid login callback. Return to AX and use its sign-in link.",
                )
                .await;
                continue;
            };
            let result = match callback {
                Ok(code) => self.exchange(&code).await,
                Err(()) => Err(invalid(
                    "Codex browser authorization was denied; retry login",
                )),
            };
            if result.is_ok() {
                respond(
                    &mut stream,
                    "200 OK",
                    "Authorization complete. Return to AX or AXCrew to finish signing in.",
                )
                .await;
            } else {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    "Authorization failed. Return to AX or AXCrew and retry.",
                )
                .await;
            }
            return result;
        }
    }

    async fn exchange(&self, code: &str) -> Result<DeviceTokens, ModelError> {
        let response = self
            .client
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code", code),
                ("code_verifier", self.verifier.as_str()),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(invalid(format!(
                "Codex token exchange failed with status {}",
                response.status()
            )));
        }
        Ok(response.json::<TokenResp>().await?.into_tokens())
    }
}

fn authorize_url(redirect_uri: &str, verifier: &str, state: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url =
        reqwest::Url::parse(&format!("{ISSUER}/oauth/authorize")).expect("static issuer URL");
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "codex_cli_rs"),
    ]);
    url.into()
}

fn callback_code(target: &str, state: &str) -> Option<Result<String, ()>> {
    if !target.starts_with('/') || target.starts_with("//") {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/auth/callback" {
        return None;
    }
    let values: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = values.iter().filter(|(key, _)| key == "state").collect();
    if states.len() != 1 || states[0].1 != state {
        return None;
    }
    if values.iter().any(|(key, _)| key == "error") {
        return Some(Err(()));
    }
    let codes: Vec<_> = values.iter().filter(|(key, _)| key == "code").collect();
    if codes.len() != 1 || codes[0].1.is_empty() {
        return None;
    }
    Some(Ok(codes[0].1.to_string()))
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    while bytes.len() < 8192 {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).map_err(|_| std::io::ErrorKind::InvalidData.into());
        }
    }
    Err(std::io::ErrorKind::InvalidData.into())
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        stream.write_all(response.as_bytes()),
    )
    .await;
}
