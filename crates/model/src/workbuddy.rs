//! Native `WorkBuddy` model transport. Protocol reference: workbuddy2api-hub
//! 6c2a6637f27dcecb6c4956392dfd936fac687306 (MIT).
//! Only browser authorization, token refresh, catalog and chat endpoints live here.
use crate::{
    AuthStorage, ModelError, ModelInfo, ModelProvider, ModelRequest, ModelResponse,
    OAuthCredential, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub const BASE_URL: &str = "https://www.workbuddy.ai";
/// Region is part of provider identity; credentials never fall back across regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkBuddyRegion {
    International,
    China,
}
impl WorkBuddyRegion {
    #[must_use]
    pub fn from_provider_id(id: &str) -> Option<Self> {
        match id {
            "workbuddy" => Some(Self::International),
            "workbuddy-cn" => Some(Self::China),
            _ => None,
        }
    }
    #[must_use]
    pub const fn provider_id(self) -> &'static str {
        match self {
            Self::International => "workbuddy",
            Self::China => "workbuddy-cn",
        }
    }
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::International => "WorkBuddy International",
            Self::China => "WorkBuddy China",
        }
    }
    #[must_use]
    pub const fn base_url(self) -> &'static str {
        match self {
            Self::International => BASE_URL,
            Self::China => "https://copilot.tencent.com",
        }
    }
    const fn origin(self) -> &'static str {
        match self {
            Self::International => BASE_URL,
            Self::China => "https://www.codebuddy.cn",
        }
    }
    const fn referer(self) -> &'static str {
        match self {
            Self::International => "https://www.workbuddy.ai/",
            Self::China => "https://www.codebuddy.cn/",
        }
    }
    const fn domain(self) -> &'static str {
        match self {
            Self::International => "www.workbuddy.ai",
            Self::China => "copilot.tencent.com",
        }
    }
    const fn user_agent(self) -> &'static str {
        match self {
            Self::International => PRODUCT_UA,
            Self::China => "WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1",
        }
    }
    const fn auth_user_agent(self) -> &'static str {
        match self {
            Self::International => "WorkBuddy/5.5.2",
            Self::China => "WorkBuddy/5.5.6",
        }
    }
    const fn refresh_source(self) -> &'static str {
        match self {
            Self::International => "plugin",
            Self::China => "workbuddy",
        }
    }
    const fn models_path(self) -> &'static str {
        match self {
            Self::International => MODELS_PATH,
            Self::China => "/v3/config",
        }
    }
}
const STATE_PATH: &str = "/v2/plugin/auth/state";
const TOKEN_PATH: &str = "/v2/plugin/auth/token";
const REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";
const MODELS_PATH: &str = "/v2/enterprises/personal/models";
const CHAT_PATH: &str = "/v2/chat/completions";
const LOGIN_PENDING: u64 = 11217;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
// This is protocol negotiation: the model catalog rejects an unknown product UA.
// No machine identifiers, device fingerprint, account pool or browser cookies.
const PRODUCT_UA: &str = "WorkBuddy/5.5.2 WorkBuddy AI/5.5.2 CLI/5.5.2";
static REFRESH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn invalid(message: &str) -> ModelError {
    ModelError::InvalidResponse(format!("WorkBuddy: {message}"))
}
fn headers(region: WorkBuddyRegion) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in [
        ("origin", region.origin()),
        ("referer", region.referer()),
        ("x-requested-with", "XMLHttpRequest"),
        ("x-codebuddy-request", "1"),
        ("x-domain", region.domain()),
        ("x-ide-type", "WorkBuddy"),
        ("x-ide-name", "WorkBuddy"),
        ("x-product", "WorkBuddy"),
        (
            "accept-language",
            match region {
                WorkBuddyRegion::International => "en-US",
                WorkBuddyRegion::China => "zh-CN",
            },
        ),
    ] {
        h.insert(k, HeaderValue::from_static(v));
    }
    h
}
fn client(region: WorkBuddyRegion) -> Result<Client, ModelError> {
    let builder = Client::builder();
    #[cfg(test)]
    let builder = builder.no_proxy();
    Ok(builder
        .user_agent(region.user_agent())
        .default_headers(headers(region))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()?)
}
fn claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|s| URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}
fn tokens(data: &Value, old: Option<&OAuthCredential>) -> Result<OAuthCredential, ModelError> {
    let access = data["accessToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("token response has no access token"))?;
    let c = claims(access);
    let epoch = data["expiresAt"]
        .as_u64()
        .map(|n| if n > 100_000_000_000 { n / 1000 } else { n });
    let expires = epoch
        .or_else(|| c["exp"].as_u64())
        .or_else(|| data["expiresIn"].as_u64().map(|s| now().saturating_add(s)))
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("token response has no expiry"))?;
    let refresh = data["refreshToken"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| old.map(|o| o.refresh.as_str()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("token response has no refresh token"))?;
    Ok(OAuthCredential {
        access: access.into(),
        refresh: refresh.into(),
        expires,
        account_id: c["sub"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| old.and_then(|o| o.account_id.clone())),
    })
}
fn validate_region(region: WorkBuddyRegion, data: &Value) -> Result<(), ModelError> {
    let jwt = claims(data["accessToken"].as_str().unwrap_or_default());
    let issuer = jwt["iss"]
        .as_str()
        .and_then(|s| reqwest::Url::parse(s).ok());
    for host in [
        data["domain"].as_str(),
        issuer.as_ref().and_then(reqwest::Url::host_str),
    ]
    .into_iter()
    .flatten()
    {
        let reported = match host {
            "copilot.tencent.com" | "codebuddy.cn" | "www.codebuddy.cn" | "www.workbuddy.cn" => {
                Some(WorkBuddyRegion::China)
            }
            "www.workbuddy.ai" | "workbuddy.ai" | "www.codebuddy.ai" | "codebuddy.ai" => {
                Some(WorkBuddyRegion::International)
            }
            _ => None,
        };
        if reported.is_some_and(|reported| reported != region) {
            return Err(invalid(
                "token belongs to a different WorkBuddy region; sign in to the selected region",
            ));
        }
    }
    Ok(())
}
fn region_error(error: ModelError, region: WorkBuddyRegion) -> ModelError {
    let error = classify(error);
    let help = format!("ax auth login {}", region.provider_id());
    match error {
        ModelError::HttpStatus { status, message } => ModelError::HttpStatus {
            status,
            message: message.replace("ax auth login workbuddy", &help),
        },
        ModelError::Configuration(message) => {
            ModelError::Configuration(message.replace("ax auth login workbuddy", &help))
        }
        other => other,
    }
}
fn refresh_error(error: ModelError) -> ModelError {
    match error {
        ModelError::InvalidResponse(_) => ModelError::Configuration(
            "WorkBuddy refresh rejected or invalid; run ax auth login workbuddy".into(),
        ),
        other => other,
    }
}
fn classify(error: ModelError) -> ModelError {
    if let ModelError::HttpStatus { status, .. } = error {
        let message = match status {
            401 => "WorkBuddy authentication rejected; run ax auth login workbuddy",
            403 => "WorkBuddy access denied: account or model is not entitled",
            429 => "WorkBuddy rate limit or quota exceeded; retry later",
            _ => "WorkBuddy upstream request failed",
        };
        ModelError::HttpStatus {
            status,
            message: message.into(),
        }
    } else {
        error
    }
}
fn checked(response: reqwest::Response) -> Result<reqwest::Response, ModelError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(classify(ModelError::HttpStatus {
            status: response.status().as_u16(),
            message: String::new(),
        }))
    }
}
async fn envelope(response: reqwest::Response) -> Result<Value, ModelError> {
    let v: Value = checked(response)?.json().await?;
    if v["code"].as_u64().is_some_and(|n| n != 0) {
        return Err(invalid("upstream rejected request"));
    }
    Ok(v)
}

/// A single short-lived login attempt. Secrets and OAuth state are never logged.
/// `WorkBuddy`'s CLI protocol currently uses server-generated state and polling,
/// with no localhost redirect or PKCE contract. Dropping this future cancels it.
pub struct BrowserLogin {
    region: WorkBuddyRegion,
    client: Client,
    base: String,
    state: String,
    pub auth_url: String,
}
impl BrowserLogin {
    /// Request a fresh server-issued authorization state.
    /// # Errors
    /// Returns a transport or protocol error for an invalid authorization response.
    pub async fn begin() -> Result<Self, ModelError> {
        Self::begin_for_region(WorkBuddyRegion::International).await
    }
    /// Begin login against the selected region's authentication endpoint.
    /// # Errors
    /// Returns a transport or protocol error for an invalid authorization response.
    pub async fn begin_for_region(region: WorkBuddyRegion) -> Result<Self, ModelError> {
        Self::begin_at(client(region)?, region.base_url(), region)
            .await
            .map_err(|error| region_error(error, region))
    }
    async fn begin_at(
        client: Client,
        base: &str,
        region: WorkBuddyRegion,
    ) -> Result<Self, ModelError> {
        let v = envelope(
            client
                .post(format!("{base}{STATE_PATH}"))
                .header("user-agent", region.auth_user_agent())
                .query(&[("platform", "CLI")])
                .timeout(Duration::from_secs(30))
                .json(&json!({}))
                .send()
                .await?,
        )
        .await?;
        let state = v["data"]["state"]
            .as_str()
            .filter(|s| s.len() >= 16)
            .ok_or_else(|| invalid("missing or invalid OAuth state"))?
            .to_owned();
        let auth_url = v["data"]["authUrl"]
            .as_str()
            .ok_or_else(|| invalid("missing authorization URL"))?
            .to_owned();
        let url =
            reqwest::Url::parse(&auth_url).map_err(|_| invalid("invalid authorization URL"))?;
        let expected = reqwest::Url::parse(base).map_err(|_| invalid("invalid endpoint"))?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.origin() != expected.origin()
            || url.path() != "/login"
            || url
                .query_pairs()
                .filter(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
                .collect::<Vec<_>>()
                != [state.clone()]
        {
            return Err(invalid("authorization URL origin/state mismatch"));
        }
        Ok(Self {
            region,
            client,
            base: base.into(),
            state,
            auth_url,
        })
    }
    /// Poll the token endpoint until authorization succeeds or expires.
    /// # Errors
    /// Returns a transport, protocol or login timeout error.
    pub async fn wait(self) -> Result<OAuthCredential, ModelError> {
        let region = self.region;
        self.wait_with(LOGIN_TIMEOUT, Duration::from_secs(2))
            .await
            .map_err(|error| region_error(error, region))
    }
    async fn wait_with(
        self,
        timeout: Duration,
        interval: Duration,
    ) -> Result<OAuthCredential, ModelError> {
        tokio::time::timeout(timeout, async {
            loop {
                let v: Value = checked(
                    self.client
                        .get(format!("{}{TOKEN_PATH}", self.base))
                        .header("user-agent", self.region.auth_user_agent())
                        .query(&[("state", &self.state)])
                        .timeout(Duration::from_secs(30))
                        .send()
                        .await
                        .map_err(|e| ModelError::Transport(e.without_url()))?,
                )?
                .json()
                .await
                .map_err(|e| ModelError::Transport(e.without_url()))?;
                match v["code"].as_u64() {
                    Some(LOGIN_PENDING) => {}
                    Some(0) => {
                        if v["data"]["accessToken"]
                            .as_str()
                            .is_some_and(|s| !s.is_empty())
                        {
                            validate_region(self.region, &v["data"])?;
                            return tokens(&v["data"], None);
                        }
                    }
                    _ => return Err(invalid("browser authorization failed; start login again")),
                }
                tokio::time::sleep(interval).await;
            }
        })
        .await
        .map_err(|_| {
            ModelError::Configuration(format!(
                "{} login timed out; run ax auth login {} again",
                self.region.label(),
                self.region.provider_id()
            ))
        })?
    }
}

pub struct WorkBuddyProvider {
    region: WorkBuddyRegion,
    storage: AuthStorage,
    config: OpenAiCompatibleConfig,
    base: String,
    client: Client,
    timeout: Duration,
}
impl WorkBuddyProvider {
    /// Construct a lazy provider using AX credential storage.
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(
        storage: AuthStorage,
        model: String,
        context_window: usize,
    ) -> Result<Self, ModelError> {
        Self::for_region(
            storage,
            model,
            context_window,
            WorkBuddyRegion::International,
        )
    }
    /// Construct a provider whose credentials, catalog and endpoints are region-scoped.
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn for_region(
        storage: AuthStorage,
        model: String,
        context_window: usize,
        region: WorkBuddyRegion,
    ) -> Result<Self, ModelError> {
        Ok(Self {
            region,
            storage,
            config: OpenAiCompatibleConfig::new(
                region.provider_id(),
                model,
                String::new(),
                format!("{}{CHAT_PATH}", region.base_url()),
                context_window,
            ),
            base: region.base_url().into(),
            client: client(region)?,
            timeout: Duration::from_secs(180),
        })
    }
    /// Serialized across instances: upstream rotates refresh tokens. Re-read AX's
    /// store under the lock, so a catalog refresh cannot race a model request.
    async fn credential(&self, rejected: Option<&str>) -> Result<OAuthCredential, ModelError> {
        let _guard = REFRESH_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        let old = self
            .storage
            .resolve_oauth(self.region.provider_id())?
            .ok_or_else(|| {
                ModelError::Configuration(format!(
                    "{} is not configured; run ax auth login {}",
                    self.region.label(),
                    self.region.provider_id()
                ))
            })?;
        validate_region(self.region, &json!({"accessToken": old.access}))?;
        if old.expires > now().saturating_add(120) && rejected != Some(old.access.as_str()) {
            return Ok(old);
        }
        let v = envelope(
            self.client
                .post(format!("{}{REFRESH_PATH}", self.base))
                .header("x-refresh-token", &old.refresh)
                .header("x-auth-refresh-source", self.region.refresh_source())
                .header("user-agent", self.region.auth_user_agent())
                .header("x-user-id", old.account_id.as_deref().unwrap_or_default())
                .timeout(Duration::from_secs(30))
                .json(&json!({}))
                .send()
                .await?,
        )
        .await
        .map_err(|error| region_error(refresh_error(error), self.region))?;
        let data = if v["data"]["data"].is_object() {
            &v["data"]["data"]
        } else {
            &v["data"]
        };
        validate_region(self.region, data)?;
        let updated = tokens(data, Some(&old))
            .map_err(|error| region_error(refresh_error(error), self.region))?;
        self.storage
            .store_oauth(self.region.provider_id(), updated.clone())?;
        Ok(updated)
    }
    fn adapter(&self, c: &OAuthCredential) -> Result<OpenAiCompatibleProvider, ModelError> {
        let mut config = self.config.clone();
        config.api_key.clone_from(&c.access);
        let mut h = headers(self.region);
        if let Some(uid) = &c.account_id {
            h.insert(
                "x-user-id",
                HeaderValue::from_str(uid).map_err(|_| invalid("invalid account id"))?,
            );
        }
        let builder = Client::builder();
        #[cfg(test)]
        let builder = builder.no_proxy();
        let client = builder
            .default_headers(h)
            .user_agent(self.region.user_agent())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(self.timeout)
            .build()?;
        Ok(OpenAiCompatibleProvider::with_client(config, client))
    }
    pub fn set_limits(
        &mut self,
        max_output_tokens: Option<usize>,
        reasoning: Option<crate::ReasoningEffort>,
    ) {
        self.config.max_output_tokens = max_output_tokens;
        self.config.reasoning_effort = reasoning;
    }
}
#[async_trait]
impl ModelProvider for WorkBuddyProvider {
    fn name(&self) -> &'static str {
        self.region.provider_id()
    }
    fn model_id(&self) -> &str {
        &self.config.model
    }
    fn context_window(&self) -> usize {
        self.config.context_window
    }
    fn max_output_tokens(&self) -> Option<usize> {
        self.config.max_output_tokens
    }
    fn fallback_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        // The native endpoint is SSE; non-streaming AX callers receive its aggregate.
        self.complete_stream(request, &mut |_| {}, &mut |_| {})
            .await
    }
    async fn complete_stream(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let c = self.credential(None).await?;
        // HTTP 401 is returned before the SSE body, so this retry cannot replay deltas.
        match self
            .adapter(&c)?
            .complete_stream(request.clone(), on_delta, on_thinking)
            .await
        {
            Err(ModelError::HttpStatus { status: 401, .. }) => {
                let c = self.credential(Some(&c.access)).await?;
                self.adapter(&c)?
                    .complete_stream(request, on_delta, on_thinking)
                    .await
                    .map_err(|error| region_error(error, self.region))
            }
            result => result.map_err(|error| region_error(error, self.region)),
        }
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let mut c = self.credential(None).await?;
        let mut response = self.models_response(&c).await?;
        if response.status() == 401 {
            c = self.credential(Some(&c.access)).await?;
            response = self.models_response(&c).await?;
        }
        let v = envelope(response)
            .await
            .map_err(|error| region_error(error, self.region))?;
        let agents = &v["data"]["agents"];
        let entries: Vec<&Value> = match agents {
            Value::Array(a) => a.iter().collect(),
            Value::Object(o) => o.values().collect(),
            _ => return Err(invalid("model catalog has no agents list")),
        };
        let ids: BTreeSet<&str> = entries
            .iter()
            .flat_map(|a| a["models"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .filter(|id| !id.is_empty() && *id != "lite")
            .collect();
        Ok(ids
            .into_iter()
            .map(|id| {
                crate::providers::compatible_model_info(
                    id.into(),
                    self.region.provider_id(),
                    &self.config.endpoint,
                )
            })
            .collect())
    }
}
impl WorkBuddyProvider {
    async fn models_response(&self, c: &OAuthCredential) -> Result<reqwest::Response, ModelError> {
        Ok(self
            .client
            .get(format!("{}{}", self.base, self.region.models_path()))
            .bearer_auth(&c.access)
            .header("x-user-id", c.account_id.as_deref().unwrap_or_default())
            .timeout(Duration::from_secs(10))
            .send()
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    struct Reply {
        status: u16,
        body: String,
        delay: Duration,
        sse: bool,
    }
    #[allow(clippy::needless_pass_by_value)]
    fn reply(body: Value) -> Reply {
        Reply {
            status: 200,
            body: body.to_string(),
            delay: Duration::ZERO,
            sse: false,
        }
    }
    fn text_reply(text: &str) -> Reply {
        Reply {
            body: format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]})
            ),
            sse: true,
            ..reply(json!({}))
        }
    }
    async fn mock(
        replies: Vec<Reply>,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let reply_base = base.clone();
        let task = tokio::spawn(async move {
            for mut r in replies {
                r.body = r.body.replace("$BASE", &reply_base);
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|b| b == b"\r\n\r\n") {
                        let h = String::from_utf8_lossy(&buf[..end]);
                        let length = h
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|n| n.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                captured.lock().await.push(String::from_utf8(buf).unwrap());
                tokio::time::sleep(r.delay).await;
                let kind = if r.sse {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let header = format!(
                    "HTTP/1.1 {} OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    r.status,
                    r.body.len()
                );
                if socket.write_all(header.as_bytes()).await.is_err() {
                    continue;
                }
                // Split UTF-8 and SSE delimiters across transport reads.
                for bytes in r.body.as_bytes().chunks(7) {
                    if socket.write_all(bytes).await.is_err() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }
        });
        (base, requests, task)
    }
    fn credential(expires: u64) -> OAuthCredential {
        OAuthCredential {
            access: "mock-access".into(),
            refresh: "mock-refresh".into(),
            expires,
            account_id: Some("mock-user".into()),
        }
    }
    fn provider(base: &str, expiry: u64) -> WorkBuddyProvider {
        let path = std::env::temp_dir().join(format!(
            "ax-wb-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = AuthStorage::new(path);
        storage
            .store_oauth("workbuddy", credential(expiry))
            .unwrap();
        let mut p = WorkBuddyProvider::new(storage, "model-a".into(), 128_000).unwrap();
        p.base = base.into();
        p.config.endpoint = format!("{base}{CHAT_PATH}");
        p
    }
    fn cleanup(p: &WorkBuddyProvider) {
        std::fs::remove_file(p.storage.path()).unwrap();
    }
    fn request() -> ModelRequest {
        ModelRequest {
            messages: vec![crate::Message::user("AX user message")],
            tools: vec![],
        }
    }
    #[tokio::test]
    async fn oauth_state_validation_and_polling() {
        let (base,captured,task)=mock(vec![reply(json!({"code":LOGIN_PENDING})),reply(json!({"code":0,"data":{"accessToken":"mock-access","refreshToken":"mock-refresh","expiresAt":now()+3600}}))]).await;
        let login = BrowserLogin {
            region: WorkBuddyRegion::International,
            client: client(WorkBuddyRegion::International).unwrap(),
            base,
            state: "random-state-for-attempt".into(),
            auth_url: String::new(),
        };
        let c = login
            .wait_with(Duration::from_secs(2), Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(c.refresh, "mock-refresh");
        task.await.unwrap();
        assert!(
            captured
                .lock()
                .await
                .iter()
                .all(|r| r.contains("state=random-state-for-attempt"))
        );
        for url in [
            "https://evil.invalid/login?state=random-state-for-attempt",
            "$BASE/login?state=wrong",
            "$BASE/login?state=random-state-for-attempt&state=random-state-for-attempt",
        ] {
            let (base, _, task) = mock(vec![reply(
                json!({"code":0,"data":{"state":"random-state-for-attempt","authUrl":url}}),
            )])
            .await;
            assert!(
                BrowserLogin::begin_at(
                    client(WorkBuddyRegion::International).unwrap(),
                    &base,
                    WorkBuddyRegion::International
                )
                .await
                .is_err()
            );
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn oauth_begin_preserves_server_state() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{base}/login?platform=CLI&state=random-state-for-attempt");
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut b = [0; 4096];
            let n = s.read(&mut b).await.unwrap();
            let r = String::from_utf8_lossy(&b[..n]);
            assert!(r.starts_with("POST /v2/plugin/auth/state?platform=CLI"));
            let body = json!({"code":0,"data":{"state":"random-state-for-attempt","authUrl":url}})
                .to_string();
            s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        });
        let login = BrowserLogin::begin_at(
            client(WorkBuddyRegion::International).unwrap(),
            &base,
            WorkBuddyRegion::International,
        )
        .await
        .unwrap();
        assert_eq!(login.state, "random-state-for-attempt");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn expired_tokens_refresh_once_across_provider_instances() {
        let (base,captured,task)=mock(vec![reply(json!({"code":0,"data":{"data":{"accessToken":"rotated","refreshToken":"rotated-refresh","expiresAt":now()+3600}}}))]).await;
        let p = provider(&base, 0);
        let mut second =
            WorkBuddyProvider::new(p.storage.clone(), "model-b".into(), 128_000).unwrap();
        second.base = base;
        let (a, b) = tokio::join!(p.credential(None), second.credential(None));
        assert_eq!(a.unwrap(), b.unwrap());
        task.await.unwrap();
        let stored = p.storage.resolve_oauth("workbuddy").unwrap().unwrap();
        assert_eq!(stored.refresh, "rotated-refresh");
        let r = captured.lock().await;
        assert_eq!(r.len(), 1);
        assert!(
            r[0].to_lowercase()
                .contains("x-refresh-token: mock-refresh")
        );
        cleanup(&p);
    }
    #[tokio::test]
    async fn refresh_failure_preserves_credentials() {
        let (base, _, task) = mock(vec![Reply {
            status: 401,
            ..reply(json!({}))
        }])
        .await;
        let p = provider(&base, 0);
        assert!(matches!(
            p.list_models().await,
            Err(ModelError::HttpStatus { status: 401, .. })
        ));
        task.await.unwrap();
        assert_eq!(
            p.storage.resolve_oauth("workbuddy").unwrap(),
            Some(credential(0))
        );
        cleanup(&p);
    }
    #[tokio::test]
    async fn list_only_account_models_deduplicates_and_uses_bearer() {
        let (base,captured,task)=mock(vec![reply(json!({"code":0,"data":{"agents":[{"models":["model-b","model-a","lite"]},{"models":["model-a"]}]}}))]).await;
        let p = provider(&base, now() + 3600);
        let models = p.list_models().await.unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["model-a", "model-b"]
        );
        assert!(p.fallback_models().is_empty());
        task.await.unwrap();
        let r = captured.lock().await;
        assert!(r[0].starts_with("GET /v2/enterprises/personal/models"));
        assert!(
            r[0].to_lowercase()
                .contains("authorization: bearer mock-access")
        );
        cleanup(&p);
    }
    #[tokio::test]
    async fn text_and_fragmented_stream_forward_ax_messages() {
        let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\",\"reasoning_content\":\"reason\",\"tool_calls\":[]},\"finish_reason\":null}]}\r\n\r\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[]},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (base, captured, task) = mock(vec![
            text_reply("hello"),
            Reply {
                body: stream.into(),
                sse: true,
                ..reply(json!({}))
            },
        ])
        .await;
        let p = provider(&base, now() + 3600);
        assert_eq!(p.complete(request()).await.unwrap().content, "hello");
        let mut deltas = Vec::new();
        let mut thinking = Vec::new();
        let response = p
            .complete_stream(request(), &mut |d| deltas.push(d), &mut |d| {
                thinking.push(d);
            })
            .await
            .unwrap();
        assert_eq!(response.content, "你好");
        assert_eq!(deltas, ["你好"]);
        assert_eq!(thinking, ["reason"]);
        task.await.unwrap();
        for r in captured.lock().await.iter() {
            let body: Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(
                body["messages"],
                json!([{"role":"user","content":"AX user message"}])
            );
            assert_eq!(body["model"], "model-a");
            assert_eq!(body["stream"], true);
            assert!(body.get("agent").is_none());
        }
        cleanup(&p);
    }
    #[tokio::test]
    async fn unauthorized_chat_refreshes_and_retries_once() {
        let (base, captured, task) = mock(vec![
            Reply {
                status: 401,
                ..reply(json!({}))
            },
            reply(json!({"code":0,"data":{"accessToken":"new-access","expiresAt":now()+3600}})),
            text_reply("retried"),
        ])
        .await;
        let p = provider(&base, now() + 3600);
        assert_eq!(p.complete(request()).await.unwrap().content, "retried");
        task.await.unwrap();
        let r = captured.lock().await;
        assert_eq!(r.len(), 3);
        assert!(
            r[2].to_lowercase()
                .contains("authorization: bearer new-access")
        );
        cleanup(&p);
    }
    #[tokio::test]
    async fn forbidden_and_rate_limit_do_not_refresh_or_leak_body() {
        for status in [403, 429] {
            let (base, captured, task) = mock(vec![Reply {
                status,
                ..reply(json!({"token":"do-not-echo"}))
            }])
            .await;
            let p = provider(&base, now() + 3600);
            let err = p.complete(request()).await.unwrap_err();
            assert!(matches!(err,ModelError::HttpStatus{status:s,..} if s==status));
            assert!(!err.to_string().contains("do-not-echo"));
            task.await.unwrap();
            assert_eq!(captured.lock().await.len(), 1);
            cleanup(&p);
        }
    }
    #[tokio::test]
    async fn request_timeout_is_bounded() {
        let (base, _, task) = mock(vec![Reply {
            delay: Duration::from_millis(150),
            ..reply(json!({}))
        }])
        .await;
        let mut p = provider(&base, now() + 3600);
        p.timeout = Duration::from_millis(30);
        assert!(
            matches!(p.complete(request()).await,Err(ModelError::Transport(e)) if e.is_timeout())
        );
        task.await.unwrap();
        cleanup(&p);
    }
    #[tokio::test]
    async fn cancellation_drops_open_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut b = [0; 4096];
            assert!(s.read(&mut b).await.unwrap() > 0);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            ready_tx.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), s.read(&mut b))
                .await
                .unwrap()
                .unwrap()
        });
        let p = Arc::new(provider(&base, now() + 3600));
        let worker = p.clone();
        let task = tokio::spawn(async move {
            worker
                .complete_stream(request(), &mut |_| {}, &mut |_| {})
                .await
        });
        ready_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(server.await.unwrap(), 0);
        cleanup(&p);
    }
    #[tokio::test]
    async fn pending_login_times_out_without_storing_tokens() {
        let (base, _, task) = mock(vec![reply(json!({"code":LOGIN_PENDING}))]).await;
        let login = BrowserLogin {
            region: WorkBuddyRegion::International,
            client: client(WorkBuddyRegion::International).unwrap(),
            base,
            state: "random-state-for-attempt".into(),
            auth_url: String::new(),
        };
        assert!(
            login
                .wait_with(Duration::from_millis(20), Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        task.await.unwrap();
    }
    #[test]
    fn jwt_expiry_millisecond_epoch_and_refresh_retention() {
        let expiry = now() + 3600;
        let token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(json!({"sub":"uid-from-token","exp":expiry}).to_string())
        );
        let old = credential(0);
        let c = tokens(&json!({"accessToken":token}), Some(&old)).unwrap();
        assert_eq!(c.account_id.as_deref(), Some("uid-from-token"));
        assert_eq!(c.expires, expiry);
        assert_eq!(c.refresh, old.refresh);
        let c = tokens(
            &json!({"accessToken":"mock-access","refreshToken":"r","expiresAt":expiry*1000}),
            None,
        )
        .unwrap();
        assert_eq!(c.expires, expiry);
        assert!(
            tokens(
                &json!({"accessToken":"mock-access","refreshToken":"r"}),
                None
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn rejected_refresh_payload_requires_login_without_overwriting_tokens() {
        let (base, _, task) = mock(vec![reply(json!({"code":999,"msg":"secret"}))]).await;
        let p = provider(&base, 0);
        let error = p.list_models().await.unwrap_err();
        assert!(error.to_string().contains("ax auth login workbuddy"));
        assert!(!error.to_string().contains("secret"));
        task.await.unwrap();
        assert_eq!(
            p.storage.resolve_oauth("workbuddy").unwrap(),
            Some(credential(0))
        );
        cleanup(&p);
    }
    #[tokio::test]
    async fn done_ends_stream_without_waiting_for_upstream_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            assert!(socket.read(&mut buf).await.unwrap() > 0);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            let body = "data: data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            socket
                .write_all(format!("{:x}\r\n{body}\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            // Intentionally no HTTP EOF or terminating chunk: DONE must suffice.
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut buf))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
        });
        let p = provider(&base, now() + 3600);
        let response = tokio::time::timeout(Duration::from_secs(1), p.complete(request()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.content, "done");
        server.await.unwrap();
        cleanup(&p);
    }
    #[tokio::test]
    async fn china_refresh_catalog_chat_and_cache_never_touch_international_credentials() {
        let catalog = json!({"code":0,"data":{"agents":{"cli":{"models":["shared-model"]}}}});
        let (base,captured,task) = mock(vec![
            reply(json!({"code":0,"data":{"accessToken":"cn-rotated","refreshToken":"cn-new-refresh","expiresAt":now()+3600,"domain":"copilot.tencent.com"}})),
            reply(catalog.clone()), text_reply("cn-text"), reply(catalog),
        ]).await;
        let intl = provider(&base, now() + 3600);
        let intl_credential = intl.storage.resolve_oauth("workbuddy").unwrap();
        let mut cn = WorkBuddyProvider::for_region(
            intl.storage.clone(),
            "shared-model".into(),
            128_000,
            WorkBuddyRegion::China,
        )
        .unwrap();
        assert_eq!(cn.base, "https://copilot.tencent.com");
        assert!(
            cn.config
                .endpoint
                .starts_with("https://copilot.tencent.com/")
        );
        cn.base.clone_from(&base);
        cn.config.endpoint = format!("{base}{CHAT_PATH}");
        cn.storage
            .store_oauth(
                "workbuddy-cn",
                OAuthCredential {
                    refresh: "cn-old-refresh".into(),
                    ..credential(0)
                },
            )
            .unwrap();
        let cache = intl.storage.path().with_extension("models");
        let registry = crate::ModelRegistry::new(&cache);
        let cn_catalog = registry.discover(&cn).await;
        assert!(cn_catalog.warning.is_none());
        assert_eq!(cn_catalog.models[0].provider, "workbuddy-cn");
        assert_eq!(cn.complete(request()).await.unwrap().content, "cn-text");
        let intl_catalog = registry.discover(&intl).await;
        assert_eq!(intl_catalog.models[0].provider, "workbuddy");
        assert!(cache.join("workbuddy-cn.json").is_file());
        assert!(cache.join("workbuddy.json").is_file());
        assert_eq!(
            cn.storage.resolve_oauth("workbuddy").unwrap(),
            intl_credential
        );
        assert_eq!(
            cn.storage
                .resolve_oauth("workbuddy-cn")
                .unwrap()
                .unwrap()
                .refresh,
            "cn-new-refresh"
        );
        task.await.unwrap();
        let requests = captured.lock().await;
        assert!(requests[0].starts_with("POST /v2/plugin/auth/token/refresh"));
        assert!(
            requests[0]
                .to_lowercase()
                .contains("x-refresh-token: cn-old-refresh")
        );
        assert!(
            requests[0]
                .to_lowercase()
                .contains("x-auth-refresh-source: workbuddy")
        );
        assert!(requests[1].starts_with("GET /v3/config"));
        assert!(requests[2].starts_with("POST /v2/chat/completions"));
        for request in &requests[..3] {
            let headers = request.to_lowercase();
            assert!(headers.contains("origin: https://www.codebuddy.cn"));
            assert!(headers.contains("x-domain: copilot.tencent.com"));
            assert!(!headers.contains("authorization: bearer mock-access"));
        }
        assert!(requests[3].starts_with("GET /v2/enterprises/personal/models"));
        assert!(
            requests[3]
                .to_lowercase()
                .contains("origin: https://www.workbuddy.ai")
        );
        assert!(
            requests[3]
                .to_lowercase()
                .contains("authorization: bearer mock-access")
        );
        std::fs::remove_file(cache.join("workbuddy-cn.json")).unwrap();
        std::fs::remove_file(cache.join("workbuddy.json")).unwrap();
        std::fs::remove_dir(cache).unwrap();
        cleanup(&intl);
    }
    #[tokio::test]
    async fn china_provider_never_falls_back_to_an_international_login() {
        let intl = provider("http://127.0.0.1:1", now() + 3600);
        let cn = WorkBuddyProvider::for_region(
            intl.storage.clone(),
            "m".into(),
            128_000,
            WorkBuddyRegion::China,
        )
        .unwrap();
        let error = cn.list_models().await.unwrap_err();
        assert!(error.to_string().contains("ax auth login workbuddy-cn"));
        assert_eq!(cn.storage.resolve_oauth("workbuddy-cn").unwrap(), None);
        cleanup(&intl);
    }
    #[test]
    fn known_token_region_mismatch_is_rejected_before_use() {
        assert!(
            validate_region(
                WorkBuddyRegion::International,
                &json!({"domain":"copilot.tencent.com"})
            )
            .is_err()
        );
        assert!(
            validate_region(
                WorkBuddyRegion::China,
                &json!({"domain":"www.workbuddy.ai"})
            )
            .is_err()
        );
        let token = format!(
            "h.{}.s",
            URL_SAFE_NO_PAD
                .encode(json!({"iss":"https://www.workbuddy.ai/auth/realms/copilot"}).to_string())
        );
        assert!(validate_region(WorkBuddyRegion::China, &json!({"accessToken":token})).is_err());
        assert!(
            validate_region(
                WorkBuddyRegion::International,
                &json!({"accessToken":token})
            )
            .is_ok()
        );
        let error = region_error(
            ModelError::HttpStatus {
                status: 401,
                message: String::new(),
            },
            WorkBuddyRegion::China,
        );
        assert!(error.to_string().contains("ax auth login workbuddy-cn"));
    }
    #[tokio::test]
    async fn china_browser_login_polls_with_china_headers_and_validates_token_region() {
        for domain in ["copilot.tencent.com", "www.workbuddy.ai"] {
            let (base,captured,task) = mock(vec![reply(json!({"code":0,"data":{"state":"china-random-state","authUrl":"$BASE/login?platform=CLI&state=china-random-state"}})),reply(json!({"code":0,"data":{"accessToken":"cn-access","refreshToken":"cn-refresh","expiresAt":now()+3600,"domain":domain}}))]).await;
            let login = BrowserLogin::begin_at(
                client(WorkBuddyRegion::China).unwrap(),
                &base,
                WorkBuddyRegion::China,
            )
            .await
            .unwrap();
            let result = login.wait().await;
            assert_eq!(result.is_ok(), domain == "copilot.tencent.com");
            task.await.unwrap();
            let requests = captured.lock().await;
            assert!(requests[0].starts_with("POST /v2/plugin/auth/state?platform=CLI"));
            let request = requests[1].to_lowercase();
            assert!(request.contains("state=china-random-state"));
            assert!(request.contains("origin: https://www.codebuddy.cn"));
            assert!(request.contains("user-agent: workbuddy/5.5.6"));
        }
    }
}
