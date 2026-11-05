<a id="workspace-sandbox"></a>
# 工作区沙盒

[English](security.md) | **简体中文**

AX 将意愿与强制约束分离。Permission 决定是否请求某动作，Sandbox 决定操作系统允许动作做什么。批准不会修改已准备沙盒，也不会在主机上重试被拒绝命令。隔离在工具下层强制执行，独立于模型指令。

<a id="configuration-and-boundaries"></a>
## 配置与边界

有原生后端的平台默认 `sandbox = "workspace"`。`ax --sandbox strict` 选择严格隔离，`ax --sandbox off` 显式选择不受限本地执行。目前 workspace 与 strict 都需完整 Linux 后端，准备失败即拒绝执行。Windows/macOS 具相同后端接口，但未实现原生隔离；显式受限模式返回 `SandboxViolation`，不自动回退 off。由于无法在这些平台准备受限默认，默认采用平台实际能执行的最强模式，即无后端时为 `off`；显式请求绝不静默降级。

工作区在本地规范化。其中读取、写入、创建、删除、shell、Git、Python、npm、Cargo、构建和测试无需反复审批，因为调用位于已准备的受限策略之后。减少审批依据实际执行工具绑定的策略，不依据进程全局模式；未准备或不受限绑定保留原审批要求，显式 Permission 拒绝仍生效。工作区之外只暴露开发所需只读系统／工具链文件；用户主目录、AX 存储、凭据目录和其他注册工作区不挂载或被屏蔽。沙盒有私有 HOME 和临时存储。Workspace 默认允许网络。**workspace 模式下 shell 自动批准且可联网**：工作区内的内容、构建脚本和依赖可以向外发送工作区数据，该模式不提供防数据外传保证。Strict 强制 `NetworkMode::Deny`，创建不共享主机网络的隔离命名空间；workspace 保留网络。Strict 也禁止 stdio MCP 服务联网。

<a id="runtime-architecture"></a>
## 运行时架构

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

`sandbox` crate 拥有 `SandboxPolicy`、`SandboxBackend`、`CommandSpec`、类型化 `SandboxViolation` 和 `SandboxManager`。工具声明执行边界，注册表集中包装工作区工具，受限模式拒绝未声明扩展。文件系统、补丁、搜索、图像和 shell 经沙盒内私有 AX worker 执行，Skill 脚本共用这些工具。本地 stdio MCP 经管理器启动；远程 MCP 在自己的服务器执行，远程副作用不承诺本地工作区隔离。可信已编译运行时工具通过狭窄服务 API 持久化 AX 状态，原生扩展属于 AX 可信计算基。

桌面／浏览器声明 `AuthorizedHost`，作为独立应用／网站服务边界，不禁用、修改或继承文件／终端沙盒策略。未知应用／网站需独立审批，通用 Process/Network 规则不能授权；显式任务上限仍生效。可信标识、会话作用域、浏览器限制及平台边界见[主机权限](host-permissions.zh-CN.md)和 [ADR 0024](adr/0024-independent-host-access.zh-CN.md)。

绑定工具保留管理器，按工作区共享。各受限本地 stdio MCP 服务准备独立管理器、broker 和 PID 命名空间，工具命令不能经 `/proc/<pid>/environ` 查看服务凭据。管理器准备持久 broker 和一套命名空间，经继承的私有监听器发送命令，调用复用 broker、文件系统及临时存储。最后一个管理器释放时终止命名空间及后代。主机代理只复制协议字节，不执行请求程序。AX 固定 worker／proxy 可执行文件对象，避免可写工作区内安装路径被替换后改变主机代理代码。会话构建通过 `spawn_blocking` 调度准备；安全查询只检查已成功绑定，不准备命名空间。失败不缓存。失效 broker 被丢弃，为下一次 spawn 重建一次；再失败则返回违规，不回退主机、不重放已执行命令。

Worker 响应结构化标记 output、`tool_error`、`confinement_error`。普通 shell 非零退出，包括缺文件或权限文字，仍为 `permission_denied=false` 的 Execution 错误，不作为授权失败阻塞 Goal。文件系统对象缺失或路径分量非目录，包括写入父目录缺失，都是普通工具错误；工作区内访问错误也由工具处理。逃逸工作区和 worker/broker 启动失败仍为 SandboxViolation，绝不根据错误文字判断隔离状态。

<a id="linux-enforcement"></a>
## Linux 约束

Linux 需要 bubblewrap、prlimit、可用 user/PID/mount/network 命名空间、openat2 和 seccomp，不用 Docker。接受命令前探测完整策略，命名空间、seccomp、对象验证或资源限制失败即终止准备。先检查 `/usr/bin/bwrap` 和 `--disable-userns` 支持，提供安装／升级指引及显式 `ax --sandbox off` 替代方式。

后端固定规范工作区目录并绑定该对象，只读挂载系统／编译器资产，丢弃所有 capability，设 no-new-privileges，禁用嵌套用户命名空间。Seccomp 阻止 mount、命名空间修改、ptrace、进程内存访问、内核 key API、BPF 和 io_uring。主机 Unix socket 不可用，开发工具仍可用匿名 stream/seqpacket socketpair。未知系统调用架构拒绝执行。

文件系统 worker 用内核 openat2 的 BENEATH、NO_XDEV、NO_MAGICLINKS 解析请求。命名空间挂载边界约束 shell 和间接访问；遍历、外部符号链接、已有嵌套 bind mount 均不能授予主机访问。保守拒绝硬链接输入，因为可能别名到主机凭据。扫描沿用发现中的生成目录跳过集，包括 `.git`、`node_modules`、`target`、虚拟环境和缓存；跳过树仍挂载，视为可信工作区／构建输入。文件系统白名单含 ext、Btrfs、XFS、tmpfs、overlayfs、ZFS、f2fs、bcachefs、ecryptfs。其他文件系统，包括 WSL 挂载 Windows 文件系统，拒绝使用；junction/reparse 不当作普通 Linux 目录。受限 WSL 工作区需移到原生 Linux 文件系统。Windows reparse 约束属于未来原生后端。

策略安装继承的 RLIMIT_DATA、CPU、进程和打开文件 rlimit，并禁用 core dump。这是每进程／用户内核限制，不是聚合 cgroup 内存配额。RLIMIT_DATA 允许大型 PROT_NONE 地址预留，不是 RLIMIT_AS 或 RSS 配额。默认每用户进程预算为检测 CPU 并行度的 256 倍，检测不可用按一计，无需 cgroup controller。各命令新建进程组，取消、超时／断连及提前错误清理时 SIGKILL。正常完成取消组清理，使后台后代可服务后续命令。故意新建会话／进程组的后代不受组清理，但命名空间销毁仍终止它们。信号退出码为 128+signal，并在 stderr 附信号名。PID 命名空间销毁提供生命周期隔离。恶意管理员或被攻陷内核不在威胁模型内。主机并发修改工作区必须可信；AX 控制所有可修改它的代理执行。

WireCommand 经私有 socket 帧传输，不放 proxy argv，子 stdin 独立。保留 16 MiB 帧上限，可支持序列化后超过 Linux 单参数 128 KiB 的命令。存在时只读挂载 `/etc/pki`。生成的单条 passwd/group 只暴露映射的当前 uid/gid，不挂载主机账户列表或 HOME 凭据目录。

各规范工作区在 `<AX_HOME>/sandbox-cache/` 下拥有 SHA-256 键控持久缓存，位于工作区外，只有该精确目录可读写挂到 `/ax-cache`。CARGO_HOME、NPM_CONFIG_CACHE、GOMODCACHE、GOCACHE、PIP_CACHE_DIR、UV_CACHE_DIR、XDG_CACHE_HOME 使用独立子目录。AX 不再为沙盒缓存创建工作区 `.cargo` 或 `.npm-cache`。缓存跨命名空间／进程重启保留，由该工作区工具与 MCP 命名空间共享，不保存 MCP 私有环境。缓存根位于工作区内则拒绝准备。见 [ADR 0033](adr/0033-sandbox-process-and-protocol-boundaries.zh-CN.md)。

<a id="children-and-crew"></a>
## 子执行与 Crew

子工具绑定自己的工作区和管理器。受限 Git 子工作区用无硬链接私有克隆，不使用引用父 Git 元数据的 worktree。可信配置仅授权精确可丢弃子目录，应用补丁仅授权该子只读状态目录；这些生命周期能力从不传给代理工具。配置 Git、展示快照和子工具执行都经沙盒执行器。AX 存储持久化、凭据准备和固定 ACP 进程引导是可信运行时服务，不是模型选择命令。

Crew 发送 `workspace_id`，AX 根据本地注册规范工作区解析；拒绝任意 Crew `cwd`。ACP 子进程以 strict 启动，不能改变绑定工作区。仍发送 cwd 的旧网关需更新协议，AX 绝不把它们的路径当授权依据。

<a id="verification"></a>
## 验证

在有 bubblewrap、prlimit、Git、Python、cc、Cargo、npm 的原生 Linux 运行：

```sh
cargo test -p sandbox --test linux_escape_suite
cargo test -p mcp --test runtime_sandbox
```

公开套件和维护者本地子会话验证覆盖真实操作系统执行、开发流程、遍历、符号链接、凭据拒绝、/etc 拒绝、嵌套挂载、硬链接、命名空间复用、后代终止、初始化失败、内置工具、未绑定扩展、Skill 脚本、stdio MCP、真实 ChildHost 配置及清理。Windows 工作区测试覆盖显式 off 和不可用后端拒绝行为，不能替代 Linux 逃逸测试。
