<a id="acp-and-ax-crew-integration"></a>
# ACP 与 AX Crew 集成

[English](acp-crew.md) | **简体中文**

`ax acp` 是位于 `crates/cli/src/acp.rs` 的增量式 [Agent Client Protocol v1](https://agentclientprotocol.com/protocol/v1/initialization) stdio 适配器。使用现有 CLI 组合根：`ReplState`（`crates/cli/src/repl/state.rs`）创建／恢复会话，`run_prompt_with`（`crates/cli/src/commands/run.rs`）调用 `AgentKernel::run_turn_checkpointed`。不实现第二套模型循环、工具注册表、记忆或凭据存储。

CLI/TUI/ACP 均经 `child_runtime::configure_controller` 绑定相同 LocalChildHost 和子预算。ACP `session/prompt` 调用 `run_session_prompt`，使用共用提示 runner。模型创建 `execution="children"` 队列后，自动进入内核 `execute_ready_children()`，按有界资源感知并发执行。顺序资源将用户要求的顺序任务串行化，独立子任务可重叠。控制器文本立即流式输出，只有共享内核发出轮次完成。子失败后继续独立任务。`--child-timeout-secs` 在 Crew/ACP 也独立于控制器轮次超时。子活动在独立信封中保留各自身份和原工具调用 ID。

提示列表绝不暗示可执行任务。读取数据集后，模型可调用 `task_queue start`，提供真实实例及完整 `{title,input}` 任务对象，或通过 `task_source` 显式记录到工作映射自动入队。`append` 追加具体工作，不替换已执行历史。共用 coding 策略／环境、续接循环、可选 Stop Guard 和持久工作区产物见 [coding-harness.md](coding-harness.zh-CN.md)。未分发工作的错误队列可显式重规划，旧状态仍归档于权威历史。

适配器接受 `initialize`、`session/new`、`session/load`、`session/resume`、`session/prompt`、`session/cancel`、`session/delete`。JSON-RPC 每行一个 JSON 对象。ACP 会话 ID 即 AX `MemoryStore` 会话 UUID。`session/load` 将用户、助手、工具历史回放为 `session/update`；`session/resume` 恢复上下文但不回放；`session/delete` 删除空闲会话。客户端 `cwd` 必须等于 ACP 进程工作目录。Crew 在成员配置目录启动各 `ax acp` 进程，保持现有项目根和工具工作区解析。

`AgentEvent` 增量及工具开始／结束转为 `session/update`。生命周期使用模型原 `tool_call_id`，同工具并行调用仍可区分。适配器 `ApprovalPolicy` 将 `session/request_permission` 响应映射到现有 `PermissionStore`，显式 Deny 始终有效。`session/cancel` 终止当前轮次 future，返回 `stopReason: cancelled`，不承诺撤销外部副作用。下次恢复沿用中断工具机制，绝不自动重放工具。

ACP `mcpServers` stdio／HTTP 项在内存转为 AX `McpConfig`，交给惰性 `McpManager`。MCP 凭据不写 Crew 数据库。支持 prompt 文本及资源链接，其他内容块明确报错，不静默丢弃。AX 只读方法有 `_ax/status`、`_ax/models`、`_ax/capabilities`、`_ax/skills`（项目／全局目录及各技能缺失工具）、`_ax/mcp`（配置服务、描述、启用标记和声明能力）、`_ax/mods`（仅元数据的已安装 JavaScript Mod）、`_ax/tools`（AX 内置工具目录）。`_ax/tools` 只列随 AX 提供的工具；MCP 工具只有会话连接服务后存在，因此 `_ax/mcp` 报告服务。

Crew 提供可选 `_ax` 会话元数据。`skills` 是本地已安装技能名白名单，`mcpServers` 是本地已配置服务名白名单，`permissionProfile` 为 `ask`、`allow` 或 `deny`。Crew 保存名称和策略，不保存 MCP 环境值或提供商凭据。适配器应用到 `ReplState` 和 `PermissionStore`，不改变内核。省略 `_ax` 的客户端采用正常 AX Skill/MCP 行为。

`ax crew pair <code> --gateway <https-url>` 在 AX 主目录生成或加载设备 Ed25519 私钥，只向 Crew 发送公钥，一次性兑换成功后记录设备 ID。`ax crew connect <https-url>` 主动发起 WSS，签署网关新挑战、发送心跳、退避重连，通过子 `ax acp` 进程执行路由工作。HTTP/WS 仅接受回环开发环境。WSS TLS 终止部署在 Crew 前，AX 二进制验证网关证书。

Crew 后端及 API 位于同级 `axcrew` 项目。Crew 保存编排元数据和最终任务输出；AX 保存消息历史、上下文快照、记忆、工具结果及提供商凭据。实时 Crew WebSocket 可含消息增量和权限请求，SQLite 事件索引只存脱敏事件元数据。

<a id="workspace-identity-and-confinement"></a>
## 工作区标识与限制

<a id="delegated-agent-activity"></a>
### 委派代理活动

普通 `subagent` / `subagent_fork` 和队列子任务在绑定父 ACP 会话发增量 `sessionUpdate: "subagent"`。各更新含稳定 `agentId`、`task`、`title`、`status`（`running`、`completed`、`failed`、`cancelled`），可选 `parentAgentId`、`childSessionId`、`summary`、`error`。嵌套子代理保留自身标识并注明直接父级。执行 ID 在历史重载间稳定，普通委派 ID 跨轮次唯一，独立于内核紧凑 `subagent-N` 句柄。

可选 `update` 包含普通 ACP `agent_message_chunk`、`tool_call`、`tool_call_update` 或 `model_connection`。子代理文字、工具输入／结果和模型状态专用此信封，不能与控制器或复用相同调用 ID 的兄弟混淆。不含子推理及内部生命周期／控制事件，权限和执行策略仍由现有内核负责。

发布开始和终态活动前，ACP 在父 AX 历史保存紧凑 `[ax-subagent]` system/agent-state 索引。私有 `run` 定位持久子存储，不向客户端发送。原始子消息留在子存储，不复制入父对话，不供父模型使用。恢复模型上下文及 Token 预算排除这些索引，即使有效上下文快照后也如此。

`session/load` 经保存索引从权威子 JSONL 回放文字和工具，保持相同信封及 ID。旧队列检查点也可标识子任务。回放验证子 Session UUID，以及规范化后位于本 AX 安装／项目存储 `child-runs` 根下的路径，绝不接受客户端子路径；读取子文件但不打开或修复 SQLite 索引。重复检查点和环不重复回放。一次重载最多展开 256 个存储、32 MiB 子 JSONL 和 20,000 条子内容／工具更新，不完整时标记 `historyTruncated: true`。子存储缺失或不可读时保留已存生命周期／摘要。只有保存检查点的消息能在中断后保留，未完成流片段可能只在实时客户端存在。

<a id="model-connection-feedback"></a>
### 模型连接反馈

`session/update` 新增 `sessionUpdate: "model_connection"` 帧，含 `status`（`connecting`、`retrying`、`connected`、`failed`）、`attempt`、`maxAttempts` 和可选 `error`。次数来自共享内核当前模型请求，下一模型步骤重置，包含初次尝试。默认及硬上限为五次请求；认证／权限错误、部分输出或连接时间预算可提前停止。首个文本／思考增量或响应成功完成时报告 Connected。状态帧独立于助手文字和工具结果；原 JSON-RPC prompt 错误仍终止失败请求。AXCrew 将最终失败状态保存到任务输出，重开聊天可查看模型错误及实际次数。见[提供商重试策略](providers.zh-CN.md)。

<a id="prompt-admission-stop-and-file-change-previews"></a>
### Prompt 准入、停止与文件变更预览

普通 ACP 提交在打开会话后、工作区基线或提供商请求前，作为用户消息持久保存。首个内核检查点确认已接纳消息，不重复追加；不同轮次重复相同提交仍为不同消息。Goal 控制命令和挂起问题答案保留原生命周期。准备阶段停止仍保留提交供会话回放。

轮次变更预览在独立可取消 AX 子进程中按配置沙盒执行。每个基线有两秒进程截止；枚举／读取还检查一秒工作预算、10,000 文件上限、每文件 2 MiB、总内容 16 MiB。增长中文件使用有界读取，保留 Git ignore 及生成目录排除。大型、不可读或慢工作区报告 `workspace_changes_unavailable` 及诊断 `reason`，对话继续。基线缺失绝不发布空 `turn_changes`，推理后也不重扫。工具副作用和工具报告变更正常继续；任一基线失败则无完整自动轮次 diff。取消丢弃并终止快照子进程，不在 ACP 执行器留下同步扫描。

新设备运行请求携带 workspace_id。AX 经本地注册项目解析并规范化路径。兼容旧 Crew 时，仅接受规范化到已注册项目根的旧 cwd，拒绝嵌套／未注册路径以及混用 workspace_id/cwd。分布式 Task 始终使用逻辑项目标识。设备 ACP 子进程采用 strict 沙盒，不能经 ACP 请求离开绑定根。见[安全](security.zh-CN.md)。

<a id="distributed-worker-adapter"></a>
## 分布式 worker 适配器

`ax crew worker worker.json` 是额外主动拉取适配器，独立于 `ax crew connect`。经现有 ACP 进程及隔离工作区执行持久分布式任务，可选绑定 `collaboration` 工具。`session/prompt` 增量包含 `_meta.axGoal.waiting_for_user`，无人值守 worker 将未解决用户交互视为失败，不发布成功完成。完整 ACP/Session/Memory 状态留本地，只将选定任务摘要、检查点和显式发布产物传给 Crew。见[分布式协作](distributed-collaboration.zh-CN.md)。

<a id="executable-mods"></a>
## 可执行 Mod

ACP 经 `_ax/scopedCapabilities` 的 kind `mods` 提供管理，查询目录不执行 JavaScript。启用 Mod 通过共享提示 runner 运行，会话 Node 状态跨 ACP 轮次重建保留。删除会话和 ACP EOF 销毁 worker。命令、hook、工具权限及兼容要求见 [Mods](mods.zh-CN.md)。
