//! ADC refresh tests, compiled inside the Google adapter to inject only the
//! test OAuth endpoint. Production OAuth URLs remain fixed to Google.
use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn auth_server(count: usize) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut forms = Vec::new();
        for index in 0..count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let split = loop {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            while bytes.len() < split + length {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            forms.push(String::from_utf8(bytes[split..split + length].to_vec()).unwrap());
            let body =
                json!({"access_token":format!("token-{index}"),"expires_in":3600}).to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        forms
    });
    (url, task)
}

fn provider(credentials: &Value, endpoint: String) -> (NativeProvider, PathBuf) {
    let path = std::env::temp_dir().join(format!("ax-google-adc-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&path, credentials.to_string()).unwrap();
    let mut config =
        super::super::NativeConfig::new("google-vertex", "gemini-test".into(), None, 128_000);
    config.google_credentials_file = Some(path.clone());
    let mut provider = NativeProvider::with_client(
        config,
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .unwrap();
    provider.google_auth.token_endpoint = Some(endpoint);
    (provider, path)
}

#[tokio::test]
async fn authorized_user_adc_refreshes_expired_token_and_preserves_project_and_quota() {
    let (url, server) = auth_server(2).await;
    let (provider, path) = provider(
        &json!({"type":"authorized_user","client_id":"client","client_secret":"test-secret","refresh_token":"test-refresh","project_id":"project","quota_project_id":"quota"}),
        url,
    );
    let first = access_token(&provider).await.unwrap();
    assert_eq!(first.access, "token-0");
    let cached = access_token(&provider).await.unwrap();
    assert_eq!(cached.access, "token-0");
    assert_eq!(cached.project.as_deref(), Some("project"));
    assert_eq!(cached.quota.as_deref(), Some("quota"));
    provider
        .google_auth
        .token
        .lock()
        .await
        .as_mut()
        .unwrap()
        .expires = Instant::now();
    assert_eq!(access_token(&provider).await.unwrap().access, "token-1");
    let forms = server.await.unwrap();
    assert_eq!(forms.len(), 2);
    assert!(forms[0].contains("grant_type=refresh_token"));
    assert!(forms[0].contains("refresh_token=test-refresh"));
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn service_account_adc_signs_valid_jwt_for_google_cloud_scope() {
    let (url, server) = auth_server(1).await;
    // Generated test-only RSA key, unrelated to any real service account.
    let (provider, path) = provider(
        &json!({"type":"service_account","client_email":"test@example.invalid","project_id":"test-project","private_key":include_str!("fixtures/service-account-test.pem")}),
        url,
    );
    assert_eq!(access_token(&provider).await.unwrap().access, "token-0");
    let forms = server.await.unwrap();
    let form = reqwest::Url::parse(&format!("http://localhost/?{}", forms[0])).unwrap();
    let assertion = form
        .query_pairs()
        .find(|(key, _)| key == "assertion")
        .unwrap()
        .1
        .into_owned();
    let key = jsonwebtoken::DecodingKey::from_rsa_pem(include_bytes!(
        "fixtures/service-account-test.pub.pem"
    ))
    .unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_audience(&["https://oauth2.googleapis.com/token"]);
    let claims = jsonwebtoken::decode::<Value>(&assertion, &key, &validation)
        .unwrap()
        .claims;
    assert_eq!(claims["iss"], "test@example.invalid");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/cloud-platform"
    );
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn explicit_missing_adc_file_fails_before_network_access() {
    let mut config =
        super::super::NativeConfig::new("google-vertex", "gemini-test".into(), None, 128_000);
    config.google_credentials_file =
        Some(std::env::temp_dir().join(format!("ax-missing-adc-{}.json", uuid::Uuid::new_v4())));
    let provider = NativeProvider::new(config).unwrap();
    assert!(
        matches!(access_token(&provider).await,Err(ModelError::Configuration(message)) if message.contains("does not exist"))
    );
}
