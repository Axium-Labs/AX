//! Docker-free Linux isolation via the audited bubblewrap launcher.
//! No shell interpolation and no permissive namespace options.
use super::{CommandSpec, FilesystemMode, NetworkMode, Result, SandboxPolicy, SandboxViolation};
use rustix::io::{FdFlags, fcntl_setfd};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    os::fd::AsRawFd,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
};
use tokio::process::Child;

pub(super) struct LinuxBackend {
    root: File,
    filter: File,
    executable: PathBuf,
    binary: File,
    ipc: tempfile::TempDir,
    broker: Mutex<Option<std::process::Child>>,
    rust_toolchain: Option<File>,
    node: Option<File>,
    npm: Option<File>,
    runtime_mounts: Vec<(File, PathBuf, bool)>,
}
impl Drop for LinuxBackend {
    fn drop(&mut self) {
        if let Ok(child) = self.broker.get_mut()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn inherited(file: &File) -> Result<()> {
    fcntl_setfd(file, FdFlags::empty()).map_err(|e| SandboxViolation(e.to_string()))
}
impl LinuxBackend {
    #[allow(
        clippy::too_many_lines,
        reason = "Keep fail-closed backend preparation in one ordered sequence"
    )]
    pub(super) fn prepare(policy: &SandboxPolicy) -> Result<Self> {
        for runtime in [
            "/",
            "/usr",
            "/bin",
            "/lib",
            "/lib64",
            "/etc",
            "/proc",
            "/sys",
            "/dev",
            "/ax-home",
            "/ax-tmp",
            "/ax-node",
            "/ax-rust",
            "/ax-worker",
        ] {
            if policy.workspace_root == Path::new(runtime)
                || (runtime != "/" && policy.workspace_root.starts_with(runtime))
            {
                return Err(SandboxViolation(
                    "workspace overlaps OS runtime or control paths".into(),
                ));
            }
        }
        for path in &policy.protected_paths {
            if policy.workspace_root.starts_with(path) {
                return Err(SandboxViolation(format!(
                    "workspace is protected: {}",
                    path.display()
                )));
            }
        }
        reject_mounts(&policy.workspace_root)?;
        reject_hardlinks(&policy.workspace_root)?;
        let root = OpenOptions::new().read(true).open(&policy.workspace_root)?;
        rustix::fs::openat2(
            &root,
            ".",
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_XDEV
                | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|e| {
            SandboxViolation(format!("required kernel object resolver unavailable: {e}"))
        })?;
        let filesystem = rustix::fs::fstatfs(&root).map_err(|e| SandboxViolation(e.to_string()))?;
        if !matches!(
            filesystem.f_type.cast_unsigned(),
            0xef53 | 0x9123_683e | 0x5846_5342 | 0x0102_1994 | 0x794c_7630
        ) {
            return Err(SandboxViolation("workspace filesystem has no verified object confinement semantics (including WSL drvfs/reparse paths)".into()));
        }
        let mut filter = tempfile::tempfile()?;
        filter.write_all(&seccomp()?)?;
        inherited(&root)?;
        inherited(&filter)?;
        let ipc = tempfile::tempdir()?;
        std::fs::set_permissions(ipc.path(), std::fs::Permissions::from_mode(0o700))?;
        // Expose compiler distributions, never ~/.cargo (tokens/config), and
        // run cargo directly rather than exposing the rustup home/shims.
        let rust_home = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")));
        let rust_toolchain = rust_home.and_then(|home| {
            let mut distributions = std::fs::read_dir(home.join("toolchains"))
                .ok()?
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.join("bin/cargo").is_file())
                .collect::<Vec<_>>();
            distributions.sort();
            distributions.pop().and_then(|path| File::open(path).ok())
        });
        if let Some(toolchain) = &rust_toolchain {
            inherited(toolchain)?;
        }
        let node_path = find_program("node");
        let node = node_path
            .filter(|path| !path.starts_with("/usr"))
            .and_then(|path| File::open(path).ok());
        let npm = find_program("npm")
            .filter(|path| {
                !path.starts_with("/usr")
                    && path.file_name().is_some_and(|name| name == "npm-cli.js")
            })
            .and_then(|path| path.parent()?.parent().map(Path::to_path_buf))
            .and_then(|path| File::open(path).ok());
        for file in [&node, &npm].into_iter().flatten() {
            inherited(file)?;
        }
        let mut runtime_mounts = Vec::new();
        for mount in &policy.runtime_mounts {
            let path = mount.path.canonicalize()?;
            if path.is_dir() {
                reject_mounts(&path)?;
                reject_hardlinks(&path)?;
            }
            let file = File::open(&path)?;
            inherited(&file)?;
            runtime_mounts.push((file, path, mount.read_only));
        }
        let executable = std::env::current_exe()?.canonicalize()?;
        // The pathname may already have been replaced before lazy preparation.
        // procfs identifies the object running this trusted AX process.
        let binary = File::open("/proc/self/exe")?;
        inherited(&binary)?;
        let backend = Self {
            root,
            filter,
            executable,
            binary,
            ipc,
            broker: Mutex::new(None),
            rust_toolchain,
            node,
            npm,
            runtime_mounts,
        };
        // Probe the actual policy, including namespaces, seccomp and rlimits.
        // No command supplied by the caller runs until this succeeds.
        let probe = CommandSpec::new("/bin/true");
        let result = backend.command(policy, probe)?.as_std_mut().output()?;
        if !result.status.success() {
            return Err(SandboxViolation(format!(
                "Linux isolation initialization failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )));
        }
        let listener =
            std::os::unix::net::UnixListener::bind(backend.ipc.path().join("control.sock"))?;
        listener.set_nonblocking(true)?;
        let mut spec = CommandSpec::new(backend.executable.clone());
        spec.args = vec!["--ax-sandbox-broker".into(), "/ax-ipc/control.sock".into()];
        spec.stdin = Stdio::from(std::os::fd::OwnedFd::from(listener));
        let mut command = backend.command(policy, spec)?;
        let mut child = command.as_std_mut().spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxViolation("broker readiness pipe missing".into()))?;
        rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
            .map_err(|e| SandboxViolation(e.to_string()))?;
        let mut ready = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ready.ends_with(b"AX_SANDBOX_READY\n") {
            let mut bytes = [0; 128];
            match stdout.read(&mut bytes) {
                Ok(count) => ready.extend_from_slice(&bytes[..count]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.into());
                }
            }
            if child.try_wait()?.is_some() {
                let output = child.wait_with_output()?;
                return Err(SandboxViolation(format!(
                    "sandbox broker initialization failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SandboxViolation(
                    "sandbox broker readiness timed out".into(),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        *backend
            .broker
            .lock()
            .map_err(|_| SandboxViolation("broker lock poisoned".into()))? = Some(child);
        Ok(backend)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the namespace mount and process policy together for auditing"
    )]
    fn command(
        &self,
        policy: &SandboxPolicy,
        spec: CommandSpec,
    ) -> Result<tokio::process::Command> {
        // Revalidate topology. A privileged host mount change must not become
        // a new recursive bind inside the sandbox.
        reject_mounts(&policy.workspace_root)?;
        let cwd = spec
            .cwd
            .unwrap_or_else(|| policy.workspace_root.clone())
            .canonicalize()?;
        if !cwd.starts_with(&policy.workspace_root) {
            return Err(SandboxViolation("cwd outside workspace".into()));
        }
        let mut cmd = tokio::process::Command::new("/usr/bin/bwrap");
        let mut filter = &self.filter;
        filter.rewind()?;
        cmd.args([
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--assert-userns-disabled",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--clearenv",
        ]);
        if policy.network_mode == NetworkMode::Allow {
            cmd.arg("--share-net");
        }
        for runtime in ["/usr", "/bin", "/lib", "/lib64"] {
            if Path::new(runtime).exists() {
                cmd.args(["--ro-bind", runtime, runtime]);
            }
        }
        if Path::new("/etc/alternatives").exists() {
            cmd.args(["--ro-bind", "/etc/alternatives", "/etc/alternatives"]);
        }
        if let Some(toolchain) = &self.rust_toolchain {
            cmd.arg("--ro-bind-fd")
                .arg(toolchain.as_raw_fd().to_string())
                .arg("/ax-rust");
        }
        if let Some(node) = &self.node {
            cmd.arg("--ro-bind-fd")
                .arg(node.as_raw_fd().to_string())
                .arg("/ax-node/bin/node");
        }
        if let Some(npm) = &self.npm {
            cmd.arg("--ro-bind-fd")
                .arg(npm.as_raw_fd().to_string())
                .arg("/ax-node/npm")
                .args(["--symlink", "../npm/bin/npm-cli.js", "/ax-node/bin/npm"]);
        }
        cmd.args([
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/ax-home",
            "--setenv",
            "HOME",
            "/ax-home",
            "--setenv",
            "PATH",
            "/ax-node/bin:/ax-rust/bin:/usr/local/bin:/usr/bin:/bin",
            "--setenv",
            "LANG",
            "C.UTF-8",
        ]);
        cmd.args(["--setenv", "CARGO_HOME"])
            .arg(policy.workspace_root.join(".cargo"));
        cmd.args(["--tmpfs", "/ax-tmp", "--setenv", "TMPDIR", "/ax-tmp"]);
        cmd.args([
            "--setenv",
            "NPM_CONFIG_USERCONFIG",
            "/ax-home/.npmrc",
            "--setenv",
            "NPM_CONFIG_CACHE",
        ])
        .arg(policy.workspace_root.join(".npm-cache"));
        if policy.network_mode == NetworkMode::Allow {
            for resolver in ["/etc/resolv.conf", "/etc/hosts", "/etc/ssl/certs"] {
                if Path::new(resolver).exists() {
                    cmd.args(["--ro-bind", resolver, resolver]);
                }
            }
        }
        cmd.arg(if policy.filesystem_mode == FilesystemMode::ReadWrite {
            "--bind-fd"
        } else {
            "--ro-bind-fd"
        })
        .arg(self.root.as_raw_fd().to_string())
        .arg(&policy.workspace_root);
        for protected in &policy.protected_paths {
            // Both the supplied location and its current real target are hidden.
            for path in [Some(protected.clone()), protected.canonicalize().ok()]
                .into_iter()
                .flatten()
            {
                if !path.exists() {
                    continue;
                }
                // Paths absent from the namespace already have no authority.
                // Mask only exposed trees: creating read-only placeholder parents
                // elsewhere would obstruct a separate narrow lifecycle mount.
                if !path.starts_with(&policy.workspace_root)
                    && ![
                        "/usr",
                        "/bin",
                        "/lib",
                        "/lib64",
                        "/etc/alternatives",
                        "/etc/ssl/certs",
                    ]
                    .iter()
                    .any(|runtime| path.starts_with(runtime))
                {
                    continue;
                }
                if path.is_dir() {
                    cmd.arg("--tmpfs").arg(&path).arg("--remount-ro").arg(&path);
                } else {
                    cmd.args(["--ro-bind", "/dev/null"]).arg(&path);
                }
            }
        }
        // Explicit service-owned capabilities are mounted only in lifecycle
        // service policies, never the ordinary registry's task namespace.
        for (file, path, read_only) in &self.runtime_mounts {
            cmd.arg(if *read_only {
                "--ro-bind-fd"
            } else {
                "--bind-fd"
            })
            .arg(file.as_raw_fd().to_string())
            .arg(path);
        }
        // Only the executable object is exposed, never the AX installation directory.
        cmd.arg("--ro-bind-fd")
            .arg(self.binary.as_raw_fd().to_string())
            .arg("/ax-worker");
        cmd.args([
            "--remount-ro",
            "/",
            "--remount-ro",
            "/proc",
            "--remount-ro",
            "/tmp",
        ]);
        cmd.arg("--chdir")
            .arg(cwd)
            .arg("--seccomp")
            .arg(self.filter.as_raw_fd().to_string());
        for (key, value) in spec.env {
            if matches!(
                key.as_str(),
                "HOME" | "PATH" | "LD_PRELOAD" | "LD_LIBRARY_PATH" | "SSH_AUTH_SOCK"
            ) {
                return Err(SandboxViolation(format!(
                    "protected child environment key: {key}"
                )));
            }
            cmd.arg("--setenv").arg(key).arg(value);
        }
        let limits = &policy.resource_limits;
        cmd.args(["--", "/usr/bin/prlimit"])
            .arg(format!("--as={0}:{0}", limits.memory_bytes))
            .arg(format!("--cpu={0}:{0}", limits.cpu_seconds))
            .arg(format!("--nproc={0}:{0}", limits.processes))
            .arg(format!("--nofile={0}:{0}", limits.open_files))
            .args(["--core=0:0", "--"]);
        let program = if spec.program == self.executable {
            PathBuf::from("/ax-worker")
        } else {
            spec.program
        };
        cmd.arg(program)
            .args(spec.args)
            .stdin(spec.stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        Ok(cmd)
    }
    pub(super) fn spawn(&self, policy: &SandboxPolicy, spec: CommandSpec) -> Result<Child> {
        let mut guard = self
            .broker
            .lock()
            .map_err(|_| SandboxViolation("broker lock poisoned".into()))?;
        if guard
            .as_mut()
            .ok_or_else(|| SandboxViolation("broker missing".into()))?
            .try_wait()?
            .is_some()
        {
            return Err(SandboxViolation(
                "task sandbox terminated; no host fallback".into(),
            ));
        }
        let cwd = spec.cwd.unwrap_or_else(|| policy.workspace_root.clone());
        // Final resolution happens inside the namespace, which has only the
        // policy's real objects mounted. No host filesystem operation follows.
        for key in spec.env.keys() {
            if matches!(
                key.as_str(),
                "HOME" | "PATH" | "LD_PRELOAD" | "LD_LIBRARY_PATH" | "SSH_AUTH_SOCK"
            ) {
                return Err(SandboxViolation(format!(
                    "protected child environment key: {key}"
                )));
            }
        }
        let program = if spec.program == self.executable {
            PathBuf::from("/ax-worker")
        } else {
            spec.program
        };
        let wire = super::broker::WireCommand {
            program,
            args: spec.args,
            env: spec.env,
            cwd,
        };
        let encoded = serde_json::to_string(&wire).map_err(|e| SandboxViolation(e.to_string()))?;
        // This trusted host-side adapter performs IPC only. The requested
        // program is exclusively spawned by the namespace broker.
        tokio::process::Command::new(format!("/proc/self/fd/{}", self.binary.as_raw_fd()))
            .arg("--ax-sandbox-proxy")
            .arg(self.ipc.path().join("control.sock"))
            .arg(encoded)
            .stdin(spec.stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Into::into)
    }
}
pub(super) fn authorize_path(root: &Path, path: &Path, create: bool) -> Result<()> {
    let directory = File::open(root)?;
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| SandboxViolation("file operation outside workspace".into()))?
    } else {
        path
    };
    let relative = if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    };
    let resolve = |candidate: &Path| {
        rustix::fs::openat2(
            &directory,
            candidate,
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_XDEV
                | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )
    };
    match resolve(relative) {
        Ok(_) => Ok(()),
        Err(rustix::io::Errno::NOENT) if create => resolve(
            relative
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )
        .map(|_| ())
        .map_err(|e| SandboxViolation(format!("workspace object resolution denied: {e}"))),
        Err(e) => Err(SandboxViolation(format!(
            "workspace object resolution denied: {e}"
        ))),
    }
}
fn find_program(name: &str) -> Option<PathBuf> {
    let mut dirs = std::env::split_paths(&std::env::var_os("PATH")?).collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .filter(|path| path.is_file())
        .filter_map(|path| path.canonicalize().ok())
        .find(|path| !path.starts_with("/mnt"))
}
fn reject_mounts(root: &Path) -> Result<()> {
    for line in std::fs::read_to_string("/proc/self/mountinfo")?.lines() {
        let Some(mount) = line.split_whitespace().nth(4) else {
            return Err(SandboxViolation("malformed mountinfo".into()));
        };
        let decoded = mount
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        let path = Path::new(&decoded);
        if path != root && path.starts_with(root) {
            return Err(SandboxViolation(format!(
                "nested mount in workspace: {}",
                path.display()
            )));
        }
    }
    Ok(())
}
fn reject_hardlinks(root: &Path) -> Result<()> {
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let meta = entry.path().symlink_metadata()?;
            if meta.is_dir() {
                dirs.push(entry.path());
            }
            if meta.is_file() && meta.nlink() > 1 {
                return Err(SandboxViolation(format!(
                    "multiply linked workspace file requires a private copy: {}",
                    entry.path().display()
                )));
            }
        }
    }
    Ok(())
}
/// Classic BPF. Architecture mismatch kills the process, including x32 ABI.
/// Namespace/process inspection, kernel control and `io_uring` are denied;
/// ordinary fork/exec, compiler threads and network TCP remain available.
#[allow(
    clippy::unnecessary_wraps,
    reason = "Unsupported architectures return a confinement error"
)]
fn seccomp() -> Result<Vec<u8>> {
    #[cfg(target_arch = "x86_64")]
    let (arch, denied): (u32, &[u32]) = (
        0xc000_003e,
        &[
            101, 155, 165, 166, 167, 169, 175, 176, 246, 248, 249, 250, 272, 298, 304, 308, 310,
            311, 313, 321, 323, 425, 426, 427, 428, 429, 430, 431, 432, 442,
        ],
    );
    #[cfg(target_arch = "aarch64")]
    let (arch, denied): (u32, &[u32]) = (
        0xc000_00b7,
        &[
            39, 40, 41, 97, 104, 105, 106, 117, 142, 217, 218, 219, 241, 264, 265, 268, 280, 282,
            425, 426, 427, 428, 429, 430, 431, 432, 442,
        ],
    );
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return Err(SandboxViolation("seccomp architecture unsupported".into()));
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let mut bytes = Vec::new();
        let mut emit = |code: u16, jt: u8, jf: u8, k: u32| {
            bytes.extend(code.to_ne_bytes());
            bytes.extend([jt, jf]);
            bytes.extend(k.to_ne_bytes());
        };
        emit(0x20, 0, 0, 4); // arch
        emit(0x15, 1, 0, arch);
        emit(0x06, 0, 0, 0x8000_0000); // KILL_PROCESS
        emit(0x20, 0, 0, 0); // syscall
        #[cfg(target_arch = "x86_64")]
        {
            emit(0x45, 0, 1, 0x4000_0000);
            emit(0x06, 0, 0, 0x8000_0000);
        }
        for nr in denied {
            emit(0x15, 0, 1, *nr);
            emit(0x06, 0, 0, 0x0005_0001);
        } // EPERM
        #[cfg(target_arch = "x86_64")]
        let (socket_nr, socketpair_nr, clone_nr) = (41, 53, 56);
        #[cfg(target_arch = "aarch64")]
        let (socket_nr, socketpair_nr, clone_nr) = (198, 199, 220);
        emit(0x15, 0, 3, socket_nr);
        emit(0x20, 0, 0, 16);
        emit(0x15, 0, 1, 1); // AF_UNIX: no host credential/Docker sockets
        emit(0x06, 0, 0, 0x0005_0001);
        emit(0x20, 0, 0, 0);
        emit(0x15, 0, 5, socketpair_nr);
        emit(0x20, 0, 0, 24); // socket type (arg1)
        emit(0x54, 0, 0, 0x0f);
        emit(0x15, 2, 0, 1); // anonymous SOCK_STREAM build IPC
        emit(0x15, 1, 0, 5); // SOCK_SEQPACKET fork/exec error reporting
        emit(0x06, 0, 0, 0x0005_0001); // no reconnectable datagram pairs
        emit(0x20, 0, 0, 0);
        emit(0x15, 0, 3, clone_nr);
        emit(0x20, 0, 0, 16);
        emit(0x45, 0, 1, 0x7e02_0000); // CLONE_NEW*
        emit(0x06, 0, 0, 0x0005_0001);
        emit(0x20, 0, 0, 0);
        emit(0x15, 0, 1, 435);
        emit(0x06, 0, 0, 0x0005_0026); // clone3 ENOSYS for libc fallback
        emit(0x06, 0, 0, 0x7fff_0000); // ALLOW
        Ok(bytes)
    }
}
