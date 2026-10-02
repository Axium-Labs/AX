//! Outbound AX Crew device bridge. Each routed run is executed by this same AX
//! binary's ACP adapter; this module never implements an agent loop.
use crate::args::CrewCommand;
use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Serialize, Deserialize)]
struct Identity {
    private_key: String,
    device_id: Option<String>,
}
fn path() -> PathBuf {
    crate::config::ax_home().join("crew-device.json")
}
fn identity() -> Result<(Identity, SigningKey)> {
    let path = path();
    if path.exists() {
        let saved: Identity = serde_json::from_slice(&fs::read(path)?)?;
        let bytes = STANDARD.decode(&saved.private_key)?;
        let key = SigningKey::from_bytes(
            &bytes
                .try_into()
                .map_err(|_| anyhow!("invalid device key"))?,
        );
        return Ok((saved, key));
    }
    let key = SigningKey::generate(&mut OsRng);
    let saved = Identity {
        private_key: STANDARD.encode(key.to_bytes()),
        device_id: None,
    };
    save(&saved)?;
    Ok((saved, key))
}
fn save(value: &Identity) -> Result<()> {
    let path = path();
    fs::create_dir_all(path.parent().ok_or_else(|| anyhow!("invalid AX home"))?)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(value)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let _ = std::process::Command::new("icacls")
            .arg(&tmp)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{user}:F"))
            .output();
    }
    fs::rename(tmp, path)?;
    Ok(())
}
pub async fn run(command: &CrewCommand) -> Result<()> {
    match command {
        CrewCommand::Pair { code, gateway } => pair(code, gateway).await,
        CrewCommand::Connect { gateway } => connect(gateway).await,
    }
}
async fn pair(code: &str, gateway: &str) -> Result<()> {
    if !gateway.starts_with("https://")
        && !gateway.starts_with("http://127.0.0.1:")
        && !gateway.starts_with("http://localhost:")
    {
        return Err(anyhow!("pairing requires HTTPS except for loopback"));
    }
    let (mut identity, key) = identity()?;
    if identity.device_id.is_some() {
        return Err(anyhow!("this AX device is already paired"));
    }
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "AX device".into());
    let client =
        if gateway.starts_with("http://127.0.0.1:") || gateway.starts_with("http://localhost:") {
            reqwest::Client::builder().no_proxy().build()?
        } else {
            reqwest::Client::new()
        };
    let response=client.post(format!("{}/api/pairing/redeem",gateway.trim_end_matches('/')))
        .json(&json!({"code":code,"public_key":STANDARD.encode(key.verifying_key().to_bytes()),"name":name,"hostname":name,"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"ax_version":env!("CARGO_PKG_VERSION")}))
        .send().await?.error_for_status()?;
    let value: Value = response.json().await?;
    identity.device_id = Some(
        value["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Crew omitted device id"))?
            .to_owned(),
    );
    save(&identity)?;
    println!(
        "Paired AX device {}",
        identity.device_id.as_deref().unwrap_or("")
    );
    Ok(())
}
async fn connect(gateway: &str) -> Result<()> {
    let (identity, key) = identity()?;
    let device_id = identity
        .device_id
        .ok_or_else(|| anyhow!("run 'ax crew pair <code>' first"))?;
    let url = if gateway.starts_with("https://") {
        gateway.replacen("https://", "wss://", 1)
    } else if gateway.starts_with("http://") {
        gateway.replacen("http://", "ws://", 1)
    } else {
        gateway.to_owned()
    };
    if !url.starts_with("wss://")
        && !url.starts_with("ws://127.0.0.1:")
        && !url.starts_with("ws://localhost:")
    {
        return Err(anyhow!(
            "remote connection requires WSS except for loopback"
        ));
    }
    let url = format!("{}/api/gateway/ws", url.trim_end_matches('/'));
    let mut delay = 1u64;
    loop {
        match connected(&url, &device_id, &key).await {
            Ok(()) => eprintln!("Crew connection closed; reconnecting"),
            Err(err) => eprintln!("Crew connection: {err}; reconnecting"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}
struct RunProcess {
    child: Child,
    input: ChildStdin,
}
#[allow(clippy::too_many_lines)]
async fn connected(url: &str, device_id: &str, key: &SigningKey) -> Result<()> {
    let (stream, _) = connect_async(url).await?;
    let (mut sink, mut source) = stream.split();
    let first = source
        .next()
        .await
        .ok_or_else(|| anyhow!("gateway closed"))??;
    let challenge: Value = serde_json::from_str(first.to_text()?)?;
    let nonce = STANDARD.decode(
        challenge["nonce"]
            .as_str()
            .ok_or_else(|| anyhow!("challenge missing nonce"))?,
    )?;
    sink.send(Message::Text(json!({"type":"authenticate","device_id":device_id,"signature":STANDARD.encode(key.sign(&nonce).to_bytes())}).to_string().into())).await?;
    let ack = source
        .next()
        .await
        .ok_or_else(|| anyhow!("gateway rejected device"))??;
    let ack: Value = serde_json::from_str(ack.to_text()?)?;
    if ack["type"] != "authenticated" {
        return Err(anyhow!("gateway rejected authentication"));
    }
    eprintln!("Connected to AX Crew as {device_id}");
    let (out, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(value) = rx.recv().await {
            if sink
                .send(Message::Text(value.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let mut runs: HashMap<String, RunProcess> = HashMap::new();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        let value = tokio::select! {
            _=heartbeat.tick()=>{out.send(json!({"type":"heartbeat","protocol_version":1,"ax_version":env!("CARGO_PKG_VERSION"),"capabilities":{"acp":true,"sessions":true,"resume":true,"cancel":true,"permissions":true}})).ok();continue;}
            msg=source.next()=>{let Some(msg)=msg else{break};let msg=msg?;if let Message::Text(text)=msg{serde_json::from_str::<Value>(&text)?}else{continue}}
        };
        let Some(run_id) = value["run_id"].as_str().map(str::to_owned) else {
            continue;
        };
        match value["type"].as_str().unwrap_or("") {
            "open" => {
                let result = async {
                    if value.get("cwd").is_some() {
                        return Err(anyhow!(
                            "Crew must send workspace_id; arbitrary cwd is forbidden"
                        ));
                    }
                    let workspace_id = value["workspace_id"]
                        .as_str()
                        .ok_or_else(|| anyhow!("workspace_id missing"))?;
                    let cwd = crate::session_projects::list()?
                        .into_iter()
                        .find(|project| project.id == workspace_id)
                        .ok_or_else(|| anyhow!("unknown local workspace_id"))?
                        .root
                        .canonicalize()?;
                    // Trusted control-plane bootstrap: pin the running AX object,
                    // even when a writable workspace replaces its on-disk pathname.
                    #[cfg(target_os = "linux")]
                    let runtime_binary = PathBuf::from("/proc/self/exe");
                    #[cfg(not(target_os = "linux"))]
                    let runtime_binary = std::env::current_exe()?;
                    let mut cmd = Command::new(runtime_binary);
                    cmd.arg("acp")
                        .arg("--sandbox")
                        .arg("strict")
                        .current_dir(cwd)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::inherit())
                        .kill_on_drop(true);
                    if let Some(provider) = value["provider"].as_str() {
                        cmd.arg("--provider").arg(provider);
                    }
                    if let Some(model) = value["model"].as_str() {
                        cmd.arg("--model").arg(model);
                    }
                    let mut child = cmd.spawn()?;
                    let input = child
                        .stdin
                        .take()
                        .ok_or_else(|| anyhow!("AX stdin unavailable"))?;
                    let stdout = child
                        .stdout
                        .take()
                        .ok_or_else(|| anyhow!("AX stdout unavailable"))?;
                    let output = out.clone();
                    let output_id = run_id.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(payload) = serde_json::from_str::<Value>(&line) {
                                output
                                    .send(
                                        json!({"type":"acp","run_id":output_id,"payload":payload}),
                                    )
                                    .ok();
                            }
                        }
                    });
                    Ok::<_, anyhow::Error>(RunProcess { child, input })
                }
                .await;
                match result {
                    Ok(process) => {
                        runs.insert(run_id.clone(), process);
                        out.send(json!({"type":"opened","run_id":run_id})).ok();
                    }
                    Err(err) => {
                        out.send(json!({"type":"error","run_id":run_id,"error":err.to_string()}))
                            .ok();
                    }
                }
            }
            "acp" => {
                if let Some(process) = runs.get_mut(&run_id) {
                    let line = value["payload"].to_string();
                    process.input.write_all(line.as_bytes()).await?;
                    process.input.write_all(b"\n").await?;
                    process.input.flush().await?;
                }
            }
            "close" => {
                if let Some(mut process) = runs.remove(&run_id) {
                    process.child.kill().await.ok();
                }
            }
            _ => {}
        }
    }
    writer.abort();
    for (_, mut process) in runs {
        process.child.kill().await.ok();
    }
    Ok(())
}
