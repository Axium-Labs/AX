//! Outbound pull worker. ACP remains the runtime; each lease gets an isolated checkout.
use crate::distributed_client::Client;
use anyhow::{Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

fn default_concurrency() -> usize {
    1
}
fn default_profile() -> String {
    "ask".into()
}
fn default_sandbox() -> String {
    "strict".into()
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerSettings {
    gateway: String,
    token: String,
    instance_id: String,
    projects: BTreeMap<String, PathBuf>,
    execution_root: PathBuf,
    #[serde(default)]
    ax_home: Option<PathBuf>,
    #[serde(default = "default_concurrency")]
    max_executions: usize,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skills_dir: Option<PathBuf>,
    #[serde(default)]
    mcp_config: Option<PathBuf>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    mcp: Vec<String>,
    #[serde(default)]
    roles: Option<Vec<String>>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    environments: Option<Vec<String>>,
    #[serde(default = "default_profile")]
    permission_profile: String,
    #[serde(default = "default_sandbox")]
    sandbox: String,
    /// Explicit host policy; paused/blocked Goals otherwise require operator action.
    #[serde(default)]
    resume_goals: bool,
    /// Host opt-in. Administrative configuration never runs during active executions.
    #[serde(default)]
    remote_management: bool,
}
struct Running {
    generation: u64,
    lease_deadline: Instant,
    handle: JoinHandle<Result<Value>>,
}

// Keep lease acknowledgement, completion and cancellation in one ordered state machine.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(path: &Path) -> Result<()> {
    let path = path.canonicalize()?;
    let mut config: WorkerSettings = serde_json::from_slice(&fs::read(&path)?)?;
    ensure!(
        (1..=128).contains(&config.max_executions)
            && ["ask", "allow", "deny"].contains(&config.permission_profile.as_str()),
        "invalid worker configuration"
    );
    ensure!(
        ["strict", "workspace", "off"].contains(&config.sandbox.as_str()),
        "invalid sandbox mode"
    );
    let base = path
        .parent()
        .ok_or_else(|| anyhow!("configuration directory missing"))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("worker.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow!("this worker configuration is already running"))?;
    for root in config.projects.values_mut() {
        *root = base.join(&*root).canonicalize()?;
    }
    config.execution_root = base.join(&config.execution_root);
    fs::create_dir_all(&config.execution_root)?;
    config.execution_root = config.execution_root.canonicalize()?;
    ensure!(
        config.projects.values().all(
            |p| !config.execution_root.starts_with(p) && !p.starts_with(&config.execution_root)
        ),
        "execution_root must be outside project sources"
    );
    config.skills_dir = config.skills_dir.map(|p| base.join(p));
    config.mcp_config = config.mcp_config.map(|p| base.join(p));
    config.ax_home = config.ax_home.map(|p| base.join(p));
    let client = Client::new(&config.gateway, config.token.clone())?;
    let identity = client
        .request("GET", "/api/distributed/worker/identity", None)
        .await?;
    ensure!(
        identity["instance_id"] == config.instance_id,
        "credential belongs to another instance"
    );
    let state = client.request("GET", "/api/distributed", None).await?;
    let approved = &state["instances"][&config.instance_id];
    ensure!(
        !approved.is_null() && approved["enabled"] == true,
        "instance missing or disabled"
    );
    ensure!(
        config.max_executions == usize::try_from(approved["max_executions"].as_u64().unwrap_or(0))?,
        "concurrency must match enrollment policy"
    );
    ensure!(
        config.projects.len() == approved["projects"].as_array().map_or(0, Vec::len),
        "all enrolled projects require local mappings"
    );
    for project in config.projects.keys() {
        ensure!(
            approved["projects"]
                .as_array()
                .is_some_and(|ps| ps.contains(&json!(project))),
            "unapproved project mapping"
        );
    }
    let policy =
        approved["pending_capabilities"]
            .as_object()
            .map_or(
                &approved["capabilities"],
                |_| &approved["pending_capabilities"],
            );
    if let Some(model) = &config.model {
        ensure!(
            policy["models"]
                .as_array()
                .is_none_or(|models| models.is_empty() || models == &vec![json!(model)]),
            "advertised models must match the configured model"
        );
    } else {
        ensure!(
            policy["models"].as_array().is_none_or(Vec::is_empty),
            "an advertised model requires an explicit local model setting"
        );
    }
    for (field, selected) in [("skills", &config.skills), ("mcp", &config.mcp)] {
        ensure!(
            policy[field].as_array().into_iter().flatten().all(|v| v
                .as_str()
                .is_some_and(|name| selected.iter().any(|s| s == name))),
            "advertised {field} must be enabled in worker settings"
        );
    }
    let profile = policy["permissions"].as_array();
    if profile.is_some_and(|ps| {
        ps.iter()
            .any(|p| matches!(p.as_str(), Some("ask" | "allow" | "deny")))
    }) {
        ensure!(
            profile.is_some_and(|ps| ps.contains(&json!(config.permission_profile))),
            "local permission profile differs from enrollment"
        );
    }
    let builtin_tools = [
        "shell",
        "filesystem",
        "find_files",
        "glob",
        "patch",
        "search",
        "web",
        "view_image",
    ];
    let tools = config
        .tools
        .clone()
        .unwrap_or_else(|| builtin_tools.iter().map(|name| (*name).into()).collect());
    ensure!(
        tools
            .iter()
            .all(|name| builtin_tools.contains(&name.as_str())),
        "tools must name available AX built-ins; configure MCP separately"
    );
    let capabilities = json!({
        "roles": config.roles.clone().map_or_else(|| policy["roles"].clone(), |roles| json!(roles)),
        "skills":config.skills, "mcp":config.mcp, "tools":tools,
        "models":config.model.iter().collect::<Vec<_>>(), "permissions":[config.permission_profile],
        "environments":config.environments.clone().unwrap_or_else(||vec![std::env::consts::OS.into()])
    });
    let incarnation = uuid::Uuid::new_v4().to_string();
    let started = client
        .request(
            "POST",
            "/api/distributed/worker/start",
            Some(json!({"incarnation":incarnation})),
        )
        .await?;
    ensure!(
        started["instance_id"] == config.instance_id,
        "credential belongs to another instance"
    );
    eprintln!(
        "Distributed AX {} started as {incarnation}",
        config.instance_id
    );
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    let mut pending: BTreeMap<String, Value> = BTreeMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut inventory_probe = Some(tokio::spawn(crate::distributed_host::detect()));
    let mut next_inventory = Instant::now() + Duration::from_secs(60);
    let mut host_inventory = None;
    let mut management: Option<JoinHandle<Vec<Value>>> = None;
    let mut management_results = Vec::<Value>::new();
    loop {
        tokio::select! { _ = tick.tick() => {}, result = tokio::signal::ctrl_c() => { result?; break; } }
        if management.as_ref().is_some_and(JoinHandle::is_finished) {
            management_results = management.take().unwrap().await.unwrap_or_default();
        }
        if management.is_none()
            && let Ok(commands) = client.request("POST", "/api/distributed/worker/manage", Some(json!({"incarnation":incarnation,"enabled":config.remote_management && config.ax_home.is_some(),"completed":management_results}))).await {
            management_results.clear();
            if let Some(commands) = commands.as_array().filter(|c| !c.is_empty())
                && let Some(home) = config.ax_home.clone().filter(|_| config.remote_management) {
                management = Some(tokio::spawn(crate::distributed_settings::apply(home,config.projects.clone(),commands.clone(),!running.is_empty())));
            }
        }
        let done: Vec<_> = running
            .iter()
            .filter(|(_, r)| r.handle.is_finished())
            .map(|(id, _)| id.clone())
            .collect();
        for id in done {
            let run = running.remove(&id).unwrap();
            let report = match run.handle.await {
                Ok(Ok(value)) => value,
                result => {
                    let text: String = format!("worker execution failed: {result:?}")
                        .chars()
                        .take(4096)
                        .collect();
                    json!({"incarnation":incarnation,"task_id":id,"generation":run.generation,"message_id":uuid::Uuid::new_v4().to_string(),
                    "kind":"failed","text":text,"artifacts":[]})
                }
            };
            pending.insert(id, report);
        }
        let active: Vec<_> = running
            .iter()
            .map(|(id, r)| json!({"task_id":id,"generation":r.generation}))
            .chain(
                pending
                    .iter()
                    .map(|(id, r)| json!({"task_id":id,"generation":r["generation"]})),
            )
            .collect();
        if inventory_probe
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            host_inventory = inventory_probe.take().unwrap().await.ok();
        }
        if inventory_probe.is_none() && Instant::now() >= next_inventory {
            inventory_probe = Some(tokio::spawn(crate::distributed_host::detect()));
            next_inventory = Instant::now() + Duration::from_secs(60);
        }
        let response = client
            .request(
                "POST",
                "/api/distributed/worker/heartbeat",
                Some(json!({"incarnation":incarnation,"active":active,"host_inventory":host_inventory,"capabilities":capabilities})),
            )
            .await;
        match response {
            Ok(response) => {
                host_inventory = None;
                let server_time = response["server_time"]
                    .as_i64()
                    .ok_or_else(|| anyhow!("server time missing"))?;
                let assignments = response["assignments"]
                    .as_array()
                    .ok_or_else(|| anyhow!("invalid assignment response"))?;
                let leases = response["leases"]
                    .as_array()
                    .ok_or_else(|| anyhow!("lease response missing"))?;
                let owns = |id: &str, generation: u64| {
                    leases
                        .iter()
                        .any(|a| a["task_id"] == id && a["generation"] == generation)
                };
                running.retain(|id, r| {
                    if owns(id, r.generation) {
                        true
                    } else {
                        r.handle.abort();
                        false
                    }
                });
                pending.retain(|id, r| owns(id, r["generation"].as_u64().unwrap_or(0)));
                for (id, run) in &mut running {
                    if let Some(lease) = leases
                        .iter()
                        .find(|l| l["task_id"] == id.as_str() && l["generation"] == run.generation)
                    {
                        run.lease_deadline = Instant::now()
                            + Duration::from_secs(
                                lease["lease_until"]
                                    .as_i64()
                                    .unwrap_or(0)
                                    .saturating_sub(server_time)
                                    .max(0)
                                    .cast_unsigned(),
                            );
                    }
                }
                for assignment in assignments {
                    let task = &assignment["task"];
                    let id = task["id"]
                        .as_str()
                        .ok_or_else(|| anyhow!("task identity missing"))?
                        .to_owned();
                    let generation = task["generation"]
                        .as_u64()
                        .ok_or_else(|| anyhow!("generation missing"))?;
                    let lease_deadline = Instant::now()
                        + Duration::from_secs(
                            task["lease_until"]
                                .as_i64()
                                .unwrap_or(0)
                                .saturating_sub(server_time)
                                .max(0)
                                .cast_unsigned(),
                        );
                    if let Some(run) = running.get_mut(&id) {
                        run.lease_deadline = lease_deadline;
                        continue;
                    }
                    if pending.contains_key(&id) || running.len() >= config.max_executions {
                        continue;
                    }
                    let config = config.clone();
                    let client = client.clone();
                    let epoch = incarnation.clone();
                    let assignment = assignment.clone();
                    let handle =
                        tokio::spawn(
                            async move { execute(&config, &client, &epoch, assignment).await },
                        );
                    running.insert(
                        id,
                        Running {
                            generation,
                            lease_deadline,
                            handle,
                        },
                    );
                }
                let reports: Vec<_> = pending
                    .iter()
                    .map(|(id, r)| (id.clone(), r.clone()))
                    .collect();
                let results =
                    futures_util::future::join_all(reports.into_iter().map(|(id, report)| {
                        let client = &client;
                        async move {
                            (
                                id,
                                client
                                    .request("POST", "/api/distributed/worker/report", Some(report))
                                    .await,
                            )
                        }
                    }))
                    .await;
                for (id, result) in results {
                    if result.is_ok() {
                        pending.remove(&id);
                    }
                }
            }
            Err(error) => {
                eprintln!("distributed connection: {error}");
                // Continue during short outages, stop local execution at the last acknowledged lease.
                running.retain(|_, r| {
                    if r.lease_deadline > Instant::now() + Duration::from_secs(5) {
                        true
                    } else {
                        r.handle.abort();
                        false
                    }
                });
            }
        }
    }
    for (_, run) in running {
        run.handle.abort();
    }
    drop(lock);
    Ok(())
}

async fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "git: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}
// Git for Windows does not accept Rust's verbatim canonical path prefix as a CLI argument.
fn command_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_owned()
}
async fn prepare(config: &WorkerSettings, task: &Value) -> Result<(PathBuf, bool)> {
    let project = task["spec"]["project_id"]
        .as_str()
        .ok_or_else(|| anyhow!("logical project missing"))?;
    let source = config
        .projects
        .get(project)
        .ok_or_else(|| anyhow!("logical project has no local mapping"))?;
    let id = task["id"]
        .as_str()
        .ok_or_else(|| anyhow!("task identity missing"))?;
    ensure!(
        id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "invalid task identity"
    );
    let generation = task["generation"]
        .as_u64()
        .ok_or_else(|| anyhow!("generation missing"))?;
    let root = config
        .execution_root
        .join(id)
        .join(generation.to_string())
        .join("workspace");
    ensure!(
        !root.exists(),
        "attempt workspace already exists; refusing to replay effects"
    );
    fs::create_dir_all(
        root.parent()
            .ok_or_else(|| anyhow!("workspace parent missing"))?,
    )?;
    let is_git = source.join(".git").exists();
    if is_git {
        // A separate clone keeps Git metadata, hooks and index out of the source workspace.
        let output = Command::new("git")
            .args(["clone", "--no-hardlinks", "--no-checkout", "--"])
            .arg(command_path(source))
            .arg(command_path(&root))
            .output()
            .await?;
        ensure!(
            output.status.success(),
            "isolated clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let revision = task["spec"]["workspace_revision"]
            .as_str()
            .unwrap_or("HEAD");
        ensure!(
            revision == "HEAD"
                || (revision.len() >= 7
                    && revision.len() <= 64
                    && revision.chars().all(|c| c.is_ascii_hexdigit())),
            "workspace_revision must be a commit hash"
        );
        git(&root, &["checkout", "--detach", revision]).await?;
    } else {
        ensure!(
            task["spec"]["workspace_revision"].is_null(),
            "revision requires a Git workspace"
        );
        copy_snapshot(source, &root)?;
    }
    if config.remote_management {
        crate::distributed_settings::snapshot_project(source, &root)?;
    }
    Ok((root, is_git))
}
pub(crate) fn copy_snapshot(source: &Path, target: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(source)?.file_type().is_symlink(),
        "snapshot rejects symbolic links"
    );
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(".ax" | ".git" | "node_modules" | "target")
        ) {
            continue;
        }
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "snapshot rejects symbolic links: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            copy_snapshot(&entry.path(), &target.join(name))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target.join(name))?;
        } else {
            anyhow::bail!("snapshot rejects special files");
        }
    }
    Ok(())
}
pub(crate) async fn send(input: &mut ChildStdin, value: Value) -> Result<()> {
    input.write_all(value.to_string().as_bytes()).await?;
    input.write_all(b"\n").await?;
    input.flush().await?;
    Ok(())
}
async fn rpc(
    input: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value> {
    send(
        input,
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await?;
    loop {
        let line = lines
            .next_line()
            .await?
            .ok_or_else(|| anyhow!("ACP closed"))?;
        let value: Value = serde_json::from_str(&line)?;
        if value["id"] == id {
            ensure!(value["error"].is_null(), "ACP {method}: {}", value["error"]);
            return Ok(value["result"].clone());
        }
    }
}
async fn publish(
    client: &Client,
    incarnation: &str,
    task: &Value,
    kind: &str,
    name: &str,
    bytes: &[u8],
) -> Result<Value> {
    ensure!(bytes.len() <= 8 * 1024 * 1024, "artifact too large");
    let body = json!({"incarnation":incarnation,"task_id":task["id"],"generation":task["generation"],"kind":kind,"name":name,"content_base64":STANDARD.encode(bytes)});
    loop {
        match client
            .request("POST", "/api/distributed/artifacts", Some(body.clone()))
            .await
        {
            Ok(value) => return Ok(value),
            Err(error)
                if error
                    .downcast_ref::<crate::distributed_client::Rejected>()
                    .is_some_and(|e| e.status.is_client_error()) =>
            {
                return Err(error);
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(3)).await,
        }
    }
}
fn summary(text: &str, failed: bool, artifact_id: &Value) -> String {
    if text.len() <= 128 * 1024 {
        text.to_owned()
    } else {
        format!(
            "Execution {}; details are in artifact {}",
            if failed { "failed" } else { "completed" },
            artifact_id
        )
    }
}
// This is an ACP protocol adapter, ordered from workspace preparation through final report.
#[allow(clippy::too_many_lines)]
async fn execute(
    config: &WorkerSettings,
    client: &Client,
    incarnation: &str,
    assignment: Value,
) -> Result<Value> {
    let task = &assignment["task"];
    let (root, is_git) = prepare(config, task).await?;
    let inputs = root.join(".distributed-inputs");
    fs::create_dir_all(&inputs)?;
    let mut references = task["spec"]["artifacts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for dep in assignment["dependency_results"]
        .as_array()
        .into_iter()
        .flatten()
    {
        references.extend(dep["artifacts"].as_array().cloned().unwrap_or_default());
    }
    references.sort_by_key(Value::to_string);
    references.dedup();
    let mut loaded_hashes = std::collections::BTreeSet::new();
    for reference in references {
        let id = reference
            .as_str()
            .ok_or_else(|| anyhow!("invalid artifact reference"))?;
        ensure!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "invalid artifact identity"
        );
        let value = client
            .request("GET", &format!("/api/distributed/artifacts/{id}"), None)
            .await?;
        let bytes = STANDARD.decode(
            value["content_base64"]
                .as_str()
                .ok_or_else(|| anyhow!("artifact content missing"))?,
        )?;
        ensure!(
            format!("{:x}", sha2::Sha256::digest(&bytes))
                == value["artifact"]["sha256"].as_str().unwrap_or(""),
            "artifact checksum mismatch"
        );
        let file = inputs.join(id);
        fs::write(&file, &bytes)?;
        if value["artifact"]["kind"] == "patch" {
            if !loaded_hashes.insert(value["artifact"]["sha256"].to_string()) {
                continue;
            }
            ensure!(is_git, "patch artifact requires a Git workspace");
            let file = command_path(&file);
            let filename = file
                .to_str()
                .ok_or_else(|| anyhow!("artifact path is not UTF-8"))?;
            git(&root, &["apply", "--check", filename]).await?;
            git(&root, &["apply", filename]).await?;
        }
    }
    let input_patch = if is_git {
        git(
            &root,
            &[
                "diff",
                "--binary",
                "HEAD",
                "--",
                ".",
                ":(exclude).ax",
                ":(exclude).distributed-inputs",
            ],
        )
        .await?
    } else {
        vec![]
    };
    let mut command = Command::new(std::env::current_exe()?);
    let state_dir = root.parent().unwrap().join("state");
    command
        .arg("acp")
        .arg("--data-dir")
        .arg(state_dir)
        .arg("--sandbox")
        .arg(&config.sandbox)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .env("AX_DISTRIBUTED_GATEWAY", &client.gateway)
        .env("AX_DISTRIBUTED_TOKEN", client.token())
        .env(
            "AX_DISTRIBUTED_TASK",
            task["id"].as_str().unwrap_or_default(),
        )
        .env("AX_DISTRIBUTED_GENERATION", task["generation"].to_string())
        .env("AX_DISTRIBUTED_INCARNATION", incarnation)
        .env(
            "AX_DISTRIBUTED_PROJECT",
            task["spec"]["project_id"].as_str().unwrap_or_default(),
        );
    command.env(
        "AX_DISTRIBUTED_WORKFLOW",
        task["spec"]["workflow_id"].as_str().unwrap_or_default(),
    );
    if let Some(provider) = &config.provider {
        command.arg("--provider").arg(provider);
    }
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    if let Some(skills) = &config.skills_dir {
        command.arg("--skills-dir").arg(skills);
    }
    if let Some(mcp) = &config.mcp_config {
        command.arg("--mcp-config").arg(mcp);
    }
    if let Some(home) = &config.ax_home {
        command.env("AX_HOME", home);
    }
    let mut child = command.spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("ACP stdin missing"))?;
    let mut lines = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("ACP stdout missing"))?,
    )
    .lines();
    rpc(&mut input,&mut lines,1,"initialize",json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"AX Distributed Worker","version":env!("CARGO_PKG_VERSION")}})).await?;
    let workflow_tasks:Vec<_>=assignment["workflow_tasks"].as_array().into_iter().flatten().map(|t|json!({"id":t["id"],"request_id":t["spec"]["request_id"],"title":t["spec"]["title"],"status":t["status"],"result":t["result"],"failure":t["failure"],"artifacts":t["artifacts"]})).collect();
    let context = format!(
        "Assigned collaboration role: {}\n\nNecessary context summary:\n{}\n\nDependency results (collaboration metadata):\n{}\n\nDurable workflow state:\n{}\n\nAlready submitted workflow tasks:\n{}\n\nObservations:\n{}\n\nInput artifacts are in .distributed-inputs, named by artifact ID. Patch artifacts have been applied to this isolated workspace. Use the collaboration tool for asynchronous delegation and workflow checkpoints. Recover from stored state and existing task IDs; a coordinator does not need to remain alive.",
        assignment["workflow"]["collaboration"]["participants"][&config.instance_id]
            .as_str()
            .unwrap_or("Contribute to the current work task"),
        task["spec"]["context_summary"].as_str().unwrap_or_default(),
        assignment["dependency_results"],
        assignment["workflow"],
        json!(workflow_tasks),
        assignment["observations"]
    );
    let task_id = task["id"]
        .as_str()
        .ok_or_else(|| anyhow!("durable task ID missing"))?;
    let saved =
        assignment["workflow"]["state"][crate::distributed_goal::CHECKPOINT_KEY].get(task_id);
    if let Some(saved) = saved {
        ensure!(
            config.resume_goals
                || !matches!(
                    saved["queue"]["goal"]["state"].as_str(),
                    Some("paused" | "blocked")
                ),
            "Durable Goal was intentionally paused/blocked; automatic resume is disabled by host policy"
        );
    }
    let original = task["spec"]["input"].as_str().unwrap_or_default();
    let explicit_goal = original
        .trim_start()
        .strip_prefix("/goal")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
    ensure!(
        saved.is_some() || task["generation"].as_u64().unwrap_or(0) <= 1 || !explicit_goal,
        "Goal takeover lacks an authoritative runtime checkpoint; refusing to reset its budget or replay effects"
    );
    let mut ax = json!({"skills":config.skills,"mcpServers":config.mcp,"permissionProfile":config.permission_profile,"taskContext":context});
    if let Some(saved) = saved {
        ax["goalCheckpoint"] = saved.clone();
    }
    let session = rpc(
        &mut input,
        &mut lines,
        2,
        "session/new",
        json!({"cwd":root,"mcpServers":[],"_ax":ax}),
    )
    .await?;
    let (prompt, intent) = if saved.is_some() {
        ("Resume unfinished durable Goal work. Inspect artifacts and actual external state before retrying effects.".to_owned(), json!({"action":"resume","goal_id":task_id}))
    } else {
        (
            original.to_owned(),
            json!({"action":"start","goal_id":task_id}),
        )
    };
    send(&mut input,json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session["sessionId"],"prompt":[{"type":"text","text":prompt}],"_meta":{"axGoal":intent}}})).await?;
    let (output, failure) = if assignment["workflow"]["collaboration"]["kind"]
        .as_str()
        .is_some_and(|kind| !kind.is_empty())
    {
        crate::distributed_session::Bridge {
            client,
            source: json!({"incarnation":incarnation,"task_id":task["id"],"generation":task["generation"],"message_id":"session-inbox","kind":"observation","text":"","artifacts":[]}),
            session,
            session_options: json!({"skills":config.skills,"mcpServers":config.mcp,"permissionProfile":config.permission_profile}),
            root: &root,
        }.drive(&mut input, &mut lines).await?
    } else {
        let mut output = String::new();
        let mut failure = None;
        loop {
            let line = lines
                .next_line()
                .await?
                .ok_or_else(|| anyhow!("AX crashed or ACP closed before result"))?;
            let value: Value = serde_json::from_str(&line)?;
            if value["method"] == "session/request_permission" {
                // Local policy is already applied inside AX. Unattended workers reject unresolved asks.
                send(&mut input,json!({"jsonrpc":"2.0","id":value["id"],"result":{"outcome":{"outcome":"selected","optionId":"reject_once"}}})).await?;
            } else if value["method"] == "session/update" {
                let update = &value["params"]["update"];
                if update["sessionUpdate"] == "agent_message_chunk"
                    && let Some(text) = update["content"]["text"].as_str()
                {
                    output.push_str(text);
                }
            } else if value["id"] == 3 {
                if !value["error"].is_null() {
                    failure = Some(format!("ACP execution error: {}", value["error"]));
                } else if value["result"]["stopReason"] != "end_turn" {
                    failure = Some("AX execution cancelled".into());
                } else if value["result"]["_meta"]["axGoal"]["waiting_for_user"] == true {
                    failure = Some("AX requires user input".into());
                }
                break;
            }
            ensure!(
                output.len() <= 8 * 1024 * 1024,
                "output exceeds artifact limit"
            );
        }
        (output, failure)
    };
    child.kill().await.ok();
    let mut artifacts = Vec::new();
    if is_git {
        // Input references are runtime coordination files, excluded from the generated patch.
        git(
            &root,
            &[
                "add",
                "-A",
                "--",
                ".",
                ":(exclude).distributed-inputs",
                ":(exclude).ax",
            ],
        )
        .await?;
        let patch = git(
            &root,
            &[
                "diff",
                "--cached",
                "--binary",
                "HEAD",
                "--",
                ".",
                ":(exclude).ax",
                ":(exclude).distributed-inputs",
            ],
        )
        .await?;
        if !patch.is_empty() && patch != input_patch {
            artifacts.push(
                publish(client, incarnation, task, "patch", "changes.patch", &patch).await?["id"]
                    .clone(),
            );
        }
    }
    let report_text = failure
        .as_ref()
        .map_or_else(|| output.clone(), |error| format!("{error}\n\n{output}"));
    let report_artifact = publish(
        client,
        incarnation,
        task,
        "report",
        "result.txt",
        report_text.as_bytes(),
    )
    .await?;
    artifacts.push(report_artifact["id"].clone());
    let text = summary(&report_text, failure.is_some(), &report_artifact["id"]);
    Ok(
        json!({"incarnation":incarnation,"task_id":task["id"],"generation":task["generation"],"message_id":uuid::Uuid::new_v4().to_string(),
        "kind":if failure.is_some(){"failed"}else{"completed"},"text":text,"artifacts":artifacts}),
    )
}
