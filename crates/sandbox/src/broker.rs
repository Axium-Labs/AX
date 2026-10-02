//! Private IPC broker: all children share a single task namespace.
//! The host proxy only copies pipe bytes; it never executes a requested program.
use super::{Result, SandboxViolation, verify_worker};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

#[derive(Serialize, Deserialize)]
pub(super) struct WireCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
}
const MAX_FRAME: usize = 16 * 1024 * 1024;
async fn send<W: AsyncWrite + Unpin>(writer: &mut W, kind: u8, bytes: &[u8]) -> Result<()> {
    let len =
        u32::try_from(bytes.len()).map_err(|_| SandboxViolation("IPC frame too large".into()))?;
    writer.write_u8(kind).await?;
    writer.write_u32(len).await?;
    writer.write_all(bytes).await?;
    Ok(())
}
async fn receive<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(u8, Vec<u8>)> {
    let kind = reader.read_u8().await?;
    let len = reader.read_u32().await? as usize;
    if len > MAX_FRAME {
        return Err(SandboxViolation("IPC frame too large".into()));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).await?;
    Ok((kind, bytes))
}
/// Serves commands through an inherited listener inside the prepared namespace.
///
/// # Errors
/// Returns a violation if confinement verification, listener setup or IPC fails.
pub async fn run_broker(_socket: PathBuf) -> Result<()> {
    verify_worker()?;
    let fd = rustix::io::dup(std::io::stdin()).map_err(|e| SandboxViolation(e.to_string()))?;
    rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)
        .map_err(|e| SandboxViolation(e.to_string()))?;
    let listener = std::os::unix::net::UnixListener::from(fd);
    listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(listener)?;
    println!("AX_SANDBOX_READY");
    std::io::stdout().flush()?;
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let _ = execute(stream).await;
        });
    }
}
async fn execute(mut stream: UnixStream) -> Result<()> {
    let (kind, payload) = receive(&mut stream).await?;
    if kind != 3 {
        return Err(SandboxViolation("invalid command frame".into()));
    }
    let spec: WireCommand =
        serde_json::from_slice(&payload).map_err(|e| SandboxViolation(e.to_string()))?;
    // This spawn occurs inside the already confined, persistent namespace.
    let mut child = match tokio::process::Command::new(spec.program)
        .args(spec.args)
        .envs(spec.env)
        .current_dir(spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            send(
                &mut stream,
                1,
                format!("SandboxViolation: {error}").as_bytes(),
            )
            .await?;
            send(&mut stream, 2, &126_i32.to_be_bytes()).await?;
            return Ok(());
        }
    };
    let mut stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SandboxViolation("broker stdout missing".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| SandboxViolation("broker stderr missing".into()))?;
    let (mut reader, mut writer) = stream.into_split();
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let input = tokio::spawn(async move {
        loop {
            match receive(&mut reader).await {
                Ok((0, bytes)) => {
                    if let Some(pipe) = &mut stdin
                        && pipe.write_all(&bytes).await.is_err()
                    {
                        stdin = None;
                    }
                }
                Ok((1, _)) => {
                    if let Some(mut pipe) = stdin.take() {
                        let _ = pipe.shutdown().await;
                    }
                }
                _ => break,
            }
        }
        let _ = cancel.send(());
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u8, Vec<u8>)>(16);
    let mut out = tokio::spawn(pump(stdout, 0, tx.clone()));
    let mut err = tokio::spawn(pump(stderr, 1, tx.clone()));
    drop(tx);
    let output = tokio::spawn(async move {
        while let Some((kind, bytes)) = rx.recv().await {
            send(&mut writer, kind, &bytes).await?;
        }
        Ok::<_, SandboxViolation>(writer)
    });
    let status = tokio::select! {
        result = child.wait() => result?,
        _ = cancelled => { let _ = child.start_kill(); child.wait().await? }
    };
    input.abort();
    // Descendants may hold stdout indefinitely. Broker retains confinement;
    // bound draining avoids hanging the task after its immediate child exits.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        let _ = tokio::join!(&mut out, &mut err);
    })
    .await;
    out.abort();
    err.abort();
    // Pump handles must be aborted when inherited descendant pipes stay open.
    // Output channel closes when those readers finish or are cancelled.
    let mut writer = output
        .await
        .map_err(|e| SandboxViolation(e.to_string()))??;
    send(&mut writer, 2, &status.code().unwrap_or(128).to_be_bytes()).await
}
async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    kind: u8,
    tx: tokio::sync::mpsc::Sender<(u8, Vec<u8>)>,
) {
    let mut bytes = vec![0; 32 * 1024];
    while let Ok(count) = reader.read(&mut bytes).await {
        if count == 0 || tx.send((kind, bytes[..count].to_vec())).await.is_err() {
            break;
        }
    }
}
/// Relays one command's input, output and exit status without executing it locally.
///
/// # Errors
/// Returns a violation if connection, framing or pipe I/O fails.
pub async fn run_proxy(socket: PathBuf, command: &str) -> Result<i32> {
    let mut stream = UnixStream::connect(socket).await?;
    send(&mut stream, 3, command.as_bytes()).await?;
    let (mut reader, mut writer) = stream.into_split();
    let input = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut bytes = vec![0; 32 * 1024];
        loop {
            let count = stdin.read(&mut bytes).await?;
            if count == 0 {
                send(&mut writer, 1, &[]).await?;
                std::future::pending::<()>().await;
            }
            send(&mut writer, 0, &bytes[..count]).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), SandboxViolation>(())
    });
    let result = async {
        loop {
            match receive(&mut reader).await? {
                (0, bytes) => tokio::io::stdout().write_all(&bytes).await?,
                (1, bytes) => tokio::io::stderr().write_all(&bytes).await?,
                (2, bytes) if bytes.len() == 4 => {
                    return Ok(i32::from_be_bytes(
                        bytes
                            .try_into()
                            .map_err(|_| SandboxViolation("invalid exit frame".into()))?,
                    ));
                }
                _ => return Err(SandboxViolation("invalid output frame".into())),
            }
        }
    }
    .await;
    input.abort();
    result
}
