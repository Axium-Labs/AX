<a id="distributed-collaboration-optional"></a>
# 分布式协作（可选）

[English](distributed-collaboration.md) | **简体中文**

此增量 CLI 适配器及工具由 AX 0.3.7 配合 AXCrew 0.3.3 引入。普通 AX、本地 Subagent、Task Queue 和既有 Crew 设备桥接仍独立工作，普通启动不连接集群。

<a id="responsibilities-and-identity"></a>
## 职责与标识

`Host → AX Instance → Execution` 是层级，不是一对一。Host 为有共享资源容量的真实机器；AX Instance 是其上独立运行配置及凭据。各实例可运行多个 Execution，每个使用现有 ACP 会话并可创建本地子代理。分布式 Task 是 AXCrew 所有的持久工作，不同于本地 Task、Session 或 Subagent。

AX 保留模型推理、规划、工具、技能、MCP、记忆和本地执行。AXCrew 负责分配、租约、放置、重试、取消及工作流状态。代理请求协助，不分配所有权。

协作异步进行：

```text
Task decomposition → capability scheduling → cross-machine execution
→ Event / Artifact return → state persistence
→ failure recovery / replanning → continued execution
```

AX 实例不直接通信。Worker 拉取分配，每五秒续租。Task、Observation、不可变产物引用和带版本 Workflow State 在原拆分 AX 消失后仍保留。重分配根任务接收检查点及已知任务／结果，无需永久 Coordinator 即可继续规划。这恢复持久工作，不恢复原模型完整会话。

<a id="setup"></a>
## 配置

1. 运行当前 AXCrew 服务，打开桌面侧栏**分布式**。
2. 添加 AX 实例，提供稳定 Host ID、名称、项目映射、并发及委派策略。注册无需 CPU/GPU/RAM 或 Skill/MCP/Tool/模型字段。同物理 Host 必须复用 Host ID。
3. 下载 `worker.json`，将同逻辑项目 ID 映射到各机本地目录，如 `D:\AX`、`/srv/AX`、`/workspace/AX`。
4. 检查本地模型凭据、启用能力、路径和权限，在该 Host 执行 `ax crew worker worker.json`。
5. 连接后在 **Hosts** 检查自动硬件。经 **AX 实例 → 配置能力**添加职责、Skill/MCP/Tool 名、模型等。下载设置合入原 worker 配置，安装／启用本地依赖并重启。AX 报告匹配能力前页面显示待应用。
6. 经管理页或 API 创建任务。AXCrew 选择合格 AX，跨 Host 全部实例预留资源。

示例，凭据及 ID 替换为注册值：

```json
{
  "gateway": "https://crew.example.com",
  "token": "instance-specific-secret",
  "instance_id": "enrolled-instance-id",
  "projects": {"project-ax": "/srv/AX"},
  "execution_root": "./executions/ax-test",
  "ax_home": "./ax-home/ax-test",
  "max_executions": 2,
  "provider": "deepseek",
  "model": "deepseek-chat",
  "skills": [],
  "mcp": [],
  "permission_profile": "ask",
  "sandbox": "strict"
}
```

非绝对路径按配置文件位置解析，`execution_root` 必须在来源项目外。`ax_home` 可选，独立记忆／配置用不同主目录。可选 `skills_dir`、`mcp_config` 选本地配置。暂存模型设置需匹配显式 worker 模型，活跃列表来自原生发现。宣称的 Skill/MCP 必须在 worker 设置启用。并发及项目映射必须匹配注册。可选 `roles`、`tools`、`environments` 选择能力元数据。Tool 必须为内置名，MCP 用独立选择。默认工具公布内置项，默认环境为系统名。显式 Skill/MCP 及配置模型由 worker 报告，不复制凭据或记忆。声明只代表启用配置，不是逐个服务健康检查；启动时核对待应用管理设置。

远程连接需有效证书 HTTPS，HTTP 仅 localhost／回环。Worker 主动连接，无需 AX 入站端口。配置含作用域凭据，应私密保存；远程部署用既有 AXCrew 管理令牌及 TLS 反向代理。

默认 `ask`，无人值守 worker 拒绝需交互请求。自主变更需显式预授权本地 `allow` profile，现有沙盒及策略仍生效。集群能力不覆盖本地权限。停滞用户交互报失败，不当成功分布式结果。

<a id="automatic-host-inventory"></a>
## 自动 Host 清单

仅 `ax crew worker` 启动清单检测。原生后台探测截止 10 秒，每 60 秒刷新，独立于五秒租约心跳；心跳投递失败会重试已检测清单。Windows 用 CIM 获取逻辑 CPU、物理 RAM、PCI 显卡；Linux 读 `/proc` 和 `/sys` PCI 显示设备；macOS 用 `sysctl`、`system_profiler`。报告主机名、系统／架构、CPU 名／数、RAM MiB、GPU 数／名及错误。AXCrew 为接受报告加时间戳，按 Host 保留一份共享容量，与实例数无关。

未知 RAM/GPU 为 null、显示未知，对请求该资源的任务提供零可用预留容量。未知 GPU 不等于无 GPU。GPU 包括集成和独立，不是 CUDA 设备数；CPU 是逻辑数，不是物理核心。清单与在线状态独立，离线 Host 保留末次配置和时间。数值是硬件容量，不是利用率、系统配额或 GPU 隔离，不影响普通单机启动。

<a id="collaboration-tool"></a>
## Collaboration 工具

Worker 向普通运行时注入可选 `collaboration`，动作如下：

| 动作 | 用途 |
|---|---|
| `catalog` | 查看 AX 职责／能力和 Host 资源 |
| `delegate` | 提交子任务，含所需能力／资源、摘要、依赖 ID、产物 ID |
| `status`、`wait` | 读取持久任务／工作流状态、结果、错误和观测，等待有界 |
| `observe` | 保存当前执行观测 |
| `cancel`、`retry` | 按所有权／策略请求中央取消或重试 |
| `publish_artifact` | 将执行工作区内文件发布为不可变对象 |
| `read_artifact` | 验证并读取有界文本产物视图 |
| `checkpoint_workflow` | 用当前 revision CAS 保存有界 JSON 检查点 |

每逻辑步骤提供稳定 `request_id`，根重分配后即使所有者换 AX，重放委派仍复用相同子任务。真正新测试／重规划尝试用新 ID，省略时工具从内容派生。

在有意义阶段保存检查点及产物／任务引用。修订冲突先读当前状态再更新。失败依赖阻塞依赖任务；分析失败测试时新建任务并传失败摘要／Artifact，不将失败任务当成功依赖。成功执行测试可返回失败测试报告，执行状态与测试结果是两回事。

<a id="workspace-and-artifacts"></a>
## 工作区与产物

每 `(task_id, generation)` 独立目录。Git 独立克隆，无硬链接，Git 元数据独立，detached 到所需 commit 或来源 HEAD；不复制脏改，未提交工作用 Patch 产物。非 Git 用文件快照，排除 `.ax`、`.git`、`target`、`node_modules`，拒绝符号链接／特殊文件，不支持固定 commit。

输入产物按哈希验证，以产物 ID 写到 `.distributed-inputs/`。Patch 经检查后应用于 Git。完成 Git 可发布变更补丁及执行报告，非 Git 输出需显式发布。来源目录从不直接作为执行目录；应用／合并结果到源项目是另一个需审查的本地操作。

仅任务输入、必需摘要、依赖结果、工作流检查点、观测和产物引用跨边界，不同步完整 Session/Context/Memory。发布报告及日志可能含敏感任务信息，同项目获准实例可见。

<a id="failure-semantics-and-limits"></a>
## 失败语义与限制

控制面按实例、incarnation、generation 和未过期租约隔离报告／上传。重启换 incarnation，旧 worker 不能续过期租约或覆盖重分配任务。重复提交／报告／上传幂等，冲突重复 ID 拒绝。失联时 worker 在最后确认租约截止前停止本地执行，Crew 稍后重试合格工作；中断执行在新工作区从检查点／输入重启。

执行为至少一次。隔离保护权威结果，不保护崩溃前外部副作用，外部变更应幂等。CPU/RAM/GPU 是调度预留，不是 OS 配额或自动 GPU 分配。每产物限 8 MiB，无自动产物／工作区 GC、对象存储或多地区共识。控制面目前为 SQLite 支撑的单一权威 AXCrew 进程，应使用持久存储和备份。物理多机及硬件隔离需部署级验证。

见 [AXCrew 控制面契约](https://github.com/Axium-Labs/AXCrew/blob/main/docs/distributed-collaboration.md)和 [ADR 0021](adr/0021-durable-distributed-collaboration.zh-CN.md)。

<a id="axcrew-collaboration-workbench-adapter"></a>
## AXCrew 协作工作台适配器

桌面围绕既有 Workflow/Task 组织协作，关系为 Host → AX Instance → Execution → 报告 AX Session。仅 Session 隔离不保证并发；每批准 worker 槽运行独立 ACP 子进程、工作区和数据目录，并发需匹配注册和 Host 共享预留。默认注册一个执行。Worker 可跨 Goal 共享，不共享有效 Session 上下文，参与职责进入既有任务上下文。

工作台 Workflow 策略 `collaboration.kind` 启用实时 ACP 适配器 `distributed_session`：有界文本／工具更新及真实 Session ID，含轮换后继，进入既有事件／尝试历史。命令使用现有 worker 凭据、当前 generation/incarnation 及记录 Session。活跃指令调用 `_ax/steer`，答案使用真实问题标识，权限决定限精确给出的请求和单次选择，90 秒过期，不授持久主机权限。Paused/blocked Goal 和待问题保持交互且占执行槽；旧 Workflow 行为仍可用。

Crew 暂停／重分配隔离执行并保留权威检查点。恢复导入既有 TaskQueue、Goal 策略及累计用量；普通副作用／本地子工作区在跨机重放前仍需协调，不迁完整本地数据库／工作区。Long Project 文档／配置原生编辑经同 CAS，轮换保持同 Goal 和历史 Session。原生 project 错误独立使命令失败，不把成功执行改失败。读／配置／导入／历史／轮换使用现有 `/goal` 控制。

仅 worker 原生配置用 `distributed_settings`，调用同一 `ax settings`、`goal-settings`、`personalize`、`capabilities`。默认显式关闭（`remote_management:false`），需目标 `ax_home`。写入需确认 Instance／项目作用域且无活跃工作，Crew 在执行写入时阻止新放置。个性化支持 Memory／风格开关、项目内样本目录和带预期内容冲突检查的自定义指令。所有结果路径均清临时原生输入。插件导入验证名称及来源在获准项目内；结果有界且去凭据字段，提供商秘密、原始 Memory、凭据文件留目标。重启 worker 不自动重试不确定写入。

Host CPU/RAM/NVIDIA GPU、磁盘和网络实测在每 60 秒清单后台任务采样，不阻塞续租。不可用为未知，预留是放置容量非系统配额。拉取 worker 在交互存储初始化前启动，避免并列 worker 因无关启动目录数据库争用。

增量操作者／worker API 和作用域见 AXCrew 的 `docs/distributed-collaboration.md`，现有沙盒、设备认证、原生权限及 Goal 副作用协调仍为权威。

主动启用原生管理后，新执行只将获准源项目 Goal／subagent／能力策略及插件定义快照到隔离 checkout，绝不复制来源 Memory、Session、auth、权限、Long Project 存储。AX 主目录设置仍作全局基线。运行执行保持原快照，保存 Goal 以权威检查点策略为准。自动结果补丁排除 `.ax` 和执行输入元数据，防止本地配置、MCP 凭据及状态作为代码导出。

<a id="per-execution-model-and-current-session-binding"></a>
## 每次执行模型与当前 Session 绑定

Worker 发现在自己的 AX_HOME 复用 ACP `_ax/models`，可选 `current` 返回相同 CLI 参数解析的本地 Provider/Model。有界后台进程每 60 秒报告过滤凭据的本地模型、当前选择和错误，不阻塞续租。Crew 选择器排除离线／过期／错误目录。本地目录存在不证明远程推理成功，不可用服务以原生提供商错误失败，不发送凭据。

Crew 在各 Attempt 冻结精确模型及职责。Worker 以原生发现验证 Provider/Model，经 `--provider`、`--model` 传既有 ACP；Session 报告该进程解析模型。仅覆盖本次尝试，不写主目录模型设置。轮换／恢复留同 ACP 进程，保持模型、Skill/MCP、权限 profile 和累计状态。职责来自任务／成员分配，能力 roles/tags 不成为代理提示。

活动和权限元数据含轮换后实际当前 Session ID。Inbox 仅接受目标为当前 Session 的命令，旧命令失败，不作用后继。既有执行租约继续隔离全部命令确认和结果发布。
