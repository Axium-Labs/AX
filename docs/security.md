# Workspace Sandbox

AX separates willingness from enforcement. Permission decides whether AX requests
an action; Sandbox decides what the operating system permits that action to do.
An approval does not mutate a prepared sandbox or retry a denied command on the
host. Isolation is enforced below tools, independently of model instructions.

## Configuration and boundaries

`sandbox = "workspace"` is the default configuration on a platform with the
native backend. `ax --sandbox strict` selects strict isolation;
`ax --sandbox off` explicitly selects unrestricted local execution. Both
workspace and strict currently require the complete Linux backend and fail
closed when preparation fails. Windows/macOS have the same backend interface but
native isolation is not implemented; there, an explicitly requested confined mode
returns `SandboxViolation` and there is no automatic off fallback. Because a
confined default could not be prepared on such a platform, the default itself is
the strongest mode the platform can actually enforce (`off` where no backend
exists) — an explicit request is never silently downgraded.

The workspace is canonicalized locally. Within it, read, write, creation,
deletion, shell, Git, Python, npm, Cargo, build and test run without repeated
approval, because the call is executing behind a prepared confined policy. That
reduced approval is derived from the policy bound to the tool that will actually
run — never from the process-wide configured mode — so an unprepared or
unconfined binding keeps its original approval requirement. Explicit Permission
denials remain effective. Outside it, the runtime
exposes only read-only OS/toolchain files needed for development. Home directories,
AX storage, credential directories and other registered workspaces are absent or
masked. Private HOME and temporary storage belong to the sandbox. Network defaults
to allowed in workspace mode. **workspace 模式下 shell 自动批准且可联网**：工作区内的
内容、构建脚本和依赖可以向外发送工作区数据，该模式不提供防数据外传保证。
Strict policies enforce `NetworkMode::Deny`, creating an isolated network
namespace without sharing the host network. Workspace retains networking.

## Runtime architecture

```text
Agent / Subagent / Skill / MCP
             |
         Tool Call
             |
      Sandbox Manager
             |
     Sandboxed Executor
             |
             OS
```

The `sandbox` crate owns `SandboxPolicy`, `SandboxBackend`, `CommandSpec`, typed
`SandboxViolation` and `SandboxManager`. Tools declare an execution boundary;
workspace tools are wrapped centrally by the registry. Undeclared extensions are
rejected in confined modes. Filesystem, patch, search, image and shell operations
execute through a private AX worker inside the sandbox. Skill scripts use these
same tools. Local stdio MCP servers spawn through the manager; remote MCP services
execute on their own servers and cannot promise local workspace confinement for
remote side effects. Trusted, compiled runtime-owned tools persist AX state through
narrow service APIs; native extension code is part of AX's trusted computing base.

Desktop/browser tools declare `AuthorizedHost`, a separate application/site service
boundary. They do not disable, modify or inherit the file/terminal sandbox policy.
Unknown app/site access needs independent approval and cannot be granted by a
generic Process/Network rule. Explicit task ceilings still apply. See
[host-permissions.md](host-permissions.md) and [ADR 0024](adr/0024-independent-host-access.md)
for trusted identities, session scope, browser restrictions and platform limits.

A manager is retained by bound tools and shared by workspace. Each confined local stdio
MCP server prepares its own manager, broker and PID namespace: tool commands
cannot inspect server credentials through `/proc/<pid>/environ`.
It prepares one persistent broker with one namespace set, then sends commands over
an inherited private listener. Calls reuse this broker, filesystem and temporary
storage. Dropping the last manager kills the namespace and its descendants.
The host proxy only copies protocol bytes; it never executes the requested program.
AX pins the executable file object used for workers and proxies so replacing an
installation pathname inside a writable workspace cannot change host proxy code.
Session construction schedules tool sandbox preparation with `spawn_blocking`.
Safety queries only inspect successfully prepared bindings and never prepare a
namespace. Failures are not cached. A dead broker is discarded and prepared once
for the next spawn; another failure returns a violation, without host fallback
or replay of an executed command.

Worker responses tag output, `tool_error`, and `confinement_error`. Ordinary shell
exit failures, including missing files and permission messages, remain Execution
errors with `permission_denied=false`; they do not block a Goal as authorization
failures. Object authorization and worker/broker startup failures remain
SandboxViolation. Error text never decides confinement status.

## Linux enforcement

Linux requires bubblewrap, prlimit, usable user/PID/mount/network namespaces,
openat2 and seccomp. No Docker service is used. Preparation probes the complete
policy before accepting commands. Namespace setup, seccomp, object validation or
resource-limit errors abort preparation.
Preparation checks `/usr/bin/bwrap` and its `--disable-userns` support first, with
install/upgrade guidance and an explicit `ax --sandbox off` alternative.

The backend pins the canonical workspace directory, binds that object, mounts OS
and compiler assets read-only, drops all capabilities, sets no-new-privileges and
disables creation of nested user namespaces. seccomp blocks mount, namespace
changes, ptrace, process-memory access, kernel key APIs, BPF and io_uring. Host Unix
sockets are unavailable; anonymous stream/seqpacket socketpairs remain usable by
development tools. Unrecognized syscall architectures fail closed.

Filesystem worker requests are resolved with kernel openat2 BENEATH, NO_XDEV and
NO_MAGICLINKS flags. Namespace mount boundaries enforce shell and indirect access.
Traversal, external symlinks and pre-existing nested bind mounts cannot grant host
access. Hardlinked input files are conservatively rejected because they could alias
host credentials. The scan follows the discovery generated-directory skip set
(including `.git`, `node_modules`, `target`, virtual environments and caches).
These skipped trees remain mounted and are trusted workspace/build inputs.
The filesystem allowlist includes ext, Btrfs, XFS, tmpfs, overlayfs, ZFS, f2fs,
bcachefs and ecryptfs. Other filesystems, including
WSL Windows-mounted filesystems, are rejected; junction/reparse objects are not
treated as ordinary Linux directories. Move a confined WSL workspace to its native
Linux filesystem. Windows reparse enforcement belongs in its future native backend.

The policy installs inherited RLIMIT_DATA, CPU, process and open-file rlimits and
disables core dumps. These are per-process/per-user kernel limits, not an aggregate
cgroup memory quota. RLIMIT_DATA permits large PROT_NONE address reservations;
it is neither RLIMIT_AS nor an RSS quota. The default per-user process budget is
256 times detected CPU parallelism (one when unavailable); no cgroup controller
is required. Each command starts a new process group, killed with SIGKILL on
cancellation, timeout/disconnect, normal exit and error cleanup. Descendants that
deliberately create another session/group are outside group cleanup; namespace
teardown still kills them. Signal exits use 128+signal and append the signal name
to stderr. PID namespace teardown supplies lifecycle isolation. A hostile
administrator or compromised kernel is outside this threat model. Concurrent host
mutation of workspace contents must be trusted; AX controls all agent execution
that can mutate them.

WireCommand travels in a private socket frame, never in proxy argv; child stdin
remains independent. The existing 16 MiB frame limit applies. Commands serialized
above Linux's per-argument 128 KiB limit work. `/etc/pki` is read-only when present.
Generated single-entry passwd/group files expose only the mapped current uid/gid.
Host account lists and HOME credential directories are not mounted.

Each canonical workspace has a SHA-256-keyed persistent cache below
`<AX_HOME>/sandbox-cache/`, outside the workspace. Only that exact directory is
mounted read-write at `/ax-cache`. CARGO_HOME, NPM_CONFIG_CACHE, GOMODCACHE,
GOCACHE, PIP_CACHE_DIR, UV_CACHE_DIR and XDG_CACHE_HOME use separate subdirectories.
AX no longer creates workspace `.cargo` or `.npm-cache` for sandbox caching.
Caches survive namespace/process restarts and are shared by that workspace's tool
and MCP namespaces; private MCP environments are not stored in them. A cache root
inside the workspace fails closed. See
[ADR 0033](adr/0033-sandbox-process-and-protocol-boundaries.md).

## Children and Crew

Child tools bind to the child's own workspace and manager. Confined child Git
workspaces use private clones without hardlinks, rather than worktrees referencing
parent Git metadata. Trusted provisioning grants only the exact disposable child
directory; patch application grants only that child's read-only state directory.
These lifecycle capabilities are never passed to agent tools. Provisioning Git,
presentation snapshots and child tool execution all use the sandbox executor.
AX's own storage persistence, credential setup and fixed ACP process bootstrap are
trusted runtime services, not model-selected command execution.

Crew sends `workspace_id`; AX resolves it against locally registered canonical
workspaces. Arbitrary Crew `cwd` is rejected. The ACP child starts with strict mode,
and ACP cannot change its bound workspace. Older gateways sending cwd must update
the wire contract; AX never interprets their path as authority.

## Verification

Run on native Linux with bubblewrap, prlimit, Git, Python, cc, Cargo and npm:

```sh
cargo test -p sandbox --test linux_escape_suite
cargo test -p mcp --test runtime_sandbox
```

Published suites and local maintainer child-session validation exercise real OS
execution, development workflows, traversal, symlinks,
credential denial, /etc denial, nested mounts, hardlinks, namespace reuse, descendant
teardown, initialization failure, builtin tools, unbound extensions, Skill scripts,
stdio MCP and actual ChildHost provisioning and cleanup. Windows workspace tests
exercise explicit off mode and unavailable-backend fail-closed behavior; they do
not substitute for Linux escape tests.
