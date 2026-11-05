<a id="tools-permissions--safety"></a>
# 工具、权限与安全

[English](tools.md) | **简体中文**

本文介绍内置工具、权限声明，以及运行时和 UI 如何强制执行。

<a id="computer-use"></a>
## Computer Use

原生 `desktop` 提供真实无障碍读取和控件动作，`ax computer-use` 管全局配置，AXCrew 编辑同状态。默认开启，保留显式禁用，执行节点／图像限制，脱敏密码并跳过含密码窗口截图。动作、权限、平台和截图限制见 [computer-use.md](computer-use.zh-CN.md)。

<a id="agent-environment-and-crew-terminal-shell"></a>
## Agent 环境与 Crew 终端 shell

`desktop`、`browser` 使用独立 `ComputerUse`、`BrowserUse` 及 `AuthorizedHost` 边界。文件／终端沙盒、应用／网站授权和动作审批分开，`--allow-dangerous` 不覆盖未知主机访问。CLI、精确身份、浏览器运行时及兼容限制见[主机权限](host-permissions.zh-CN.md)。

本地 AXCrew 通过同一 `browser` 暴露内置侧栏，工具声明活跃传输和支持动作，复用 Session 标签，不另开浏览器。Windows 嵌入和独立 Playwright 截图／按键支持不同，见 [Browser Use](host-permissions.zh-CN.md#browser-use)。

`ax environment` 显示执行设置。Windows `ax environment native` 直接执行，`ax environment wsl` 用默认 WSL 发行版 Linux AX。选择前检查 `~/.local/bin/ax --version`，需先安装 Linux 版到该处。TUI `/environment` 管相同设置，下次 AX 启动生效。

Linux/macOS 集成终端选 `default`、`sh`、`bash`、`zsh`、`fish`，Windows 保留原选项，只影响新 Crew 终端，不改 Agent shell POSIX 契约。解析／安装／限制见[平台](platforms.zh-CN.md)。Windows launcher 转发 stdio、退出状态、工作区及路径参数，用 `wslpath` 映射共享 `AX_HOME` 和 ACP `cwd`，WSL 内 shell 用 Linux 运行时。

`ax environment --terminal-shell powershell|cmd|git-bash|wsl` 独立选择新 Crew 终端。Crew 设置 > AX 提供两项，保存前验证程序，已有进程／终端环境不变。

<a id="tool-model"></a>
## 工具模型

每 `shell` 调用在会话或绑定子工作区新开进程，一次切目录不影响后续。`_ax_execution.scope` 声明步骤资源，不选择 cwd。嵌套项目须在命令中显式 cd 或用程序目录选项，如 `git -C ax status`。结果含起始 `working_directory`、退出码、stdout、stderr，按真实检查目录归因；根目录 Git 错误不证明子目录非仓库。

完成跟随实际 shell 退出，不依赖管道 EOF。Stdin 输入和输出收集并发，退出后最多等一秒排空，再关句柄、返回已捕获字节及真实退出码。后台后代持管道时含 `output_capture_incomplete: true` 及说明，文字可不完整。一秒仅排空，不限制前台运行或停止后台。正常 EOF 无额外延迟。取消丢所属 pipe future，无脱离 reader；原超时／权限及非零错误处理不变。

**`Tool`**（`crates/tool`）是带名称和 JSON schema 的可调用能力。`ToolRegistry` 收集 Agent 工具，Loop 与来源无关，MCP 代理为普通 `Tool`，不按来源分支。工具经 `permission()` 返回 `ToolPermission { capability, safety }`，内核／UI 不从 `mcp__`/`::` 等名称猜权限。

| 字段 | 含义 |
|---|---|
| `capability` | 工具使用的能力类别 |
| `safety` | `SafetyLevel::Safe` 或 `SafetyLevel::RequiresApproval` |

<a id="built-in-tools"></a>
## 内置工具

行为指引属于各能力，工具可在 `crates/tool/src/lib.rs` 声明 `guidance()`，说明*模型决定使用后*如何正确调用，内核组装 `[ax-capability-guidance]`；指引不意味着应选工具，没有全局工具策略提示。

运行时提示说明请求定义工作，能力是可选而非义务。队列和委派纪律在 `[ax-coding-harness]`，见[运行时](agent-runtime.zh-CN.md)。沟通指引仍适用：相关调用合为工作阶段，不逐项讲述常规结果，最后给连贯答案。CLI/TUI 和 AXCrew ACP 都适用，不丢文字或历史，Crew 仅在展示层分组折叠。

| 工具 | 用途 | 能力 |
|---|---|---|
| `shell` | 构建、测试、Git 和命令行流程 | `Shell` / `Process` |
| `filesystem` | 读／列／写已知路径 | `FilesystemRead` / `FilesystemWrite` |
| `find_files` / `glob` | 按名称／路径／扩展发现目录文件，不读内容 | `FilesystemRead` |
| `patch` | 结构化多块编辑，任一失败全写终止 | `FilesystemWrite` |
| `search` | 行内文本、符号及正则 grep | `FilesystemRead` |
| `web` | `search` 通过可替换提供商最多四查询并发；`fetch` 最多六 HTTP(S) GET，返回清理 Markdown／文本，合并去重、容忍局部失败 | `Network` |
| `view_image` | 模型支持视觉时读取工作区原生图像 | `FilesystemRead` |

各组合根构建注册表，即 `crates/cli/src/runtime/builder.rs` 的 `runtime::tools`，先内置再发现 MCP 代理。

<a id="local-discovery-and-search"></a>
### 本地发现与搜索

`find_files`（别名 `glob`）与 `search` 共用一套实现：

- `find_files`/`glob` 只回答存在什么路径。`pattern` 为 glob，如 `**/*.jsonl`、`src/**/*.ts`、`*.toml`，纯文件名匹配任意深度；另有 `include`/`exclude`、`type`（`file`/`dir`/`any`）、`max_depth`、`max_results`，不打开文件。
- `search` 只回答文本／符号／正则出现在哪。参数 `mode`（`literal`/`regex`）、`case_sensitive` 默认 true、`include`/`exclude`、`context_lines`、`max_results`、`output_mode`（`content` 或 `files_with_matches`），逐行匹配。
- 二者接受规范 `path` 或别名 `root`，省略为绑定根。结果为工作区相对 `/` 路径，可直接传 `filesystem.read`。
- 默认跳 `.git`、`target`、`node_modules`、`.venv`/`venv`、`dist`、`build`、`__pycache__`、缓存、`coverage`、`vendor`、`.ax`、`child-runs`，遵守 `.gitignore` 及 `.axignore`，非 Git 也适用；不跟符号链接、不读超过 4 MB 文件。
- 范围存在而无匹配时 `success`，返回空 `matches`（`search`）或 `paths`（`find_files`/`glob`）；缺 `path`/`root` 则与 `filesystem` 相同 `path_error`，含邻近候选路径。
- 遍历用 ripgrep 同款 `ignore`，匹配用 `regex`，内置实现，不调用用户 `rg`。

二者声明解析范围为**只读**资源，同轮独立发现可并行，不排全局写锁。同轮同范围不重复遍历：相同只读调用排首次之后并复用，重复仍有自身工具响应；短 `reused_from` 说明相同，保留 `status`、`truncated`，省重复正文／诊断。类型化引用沿原回执 ID。仅状态复用，运行时不选工具、不硬编码搜索顺序。

子代理同注册表。`SandboxedTool::fork_for_run` 委托所包 `fork_for_run`，发现结构体按 `child.cwd` 重绑 `cwd`；`filesystem` 经 `WorkspaceTool` 同样绑定。路径绑定、声明范围检查及沙盒限制子工作区，无第二搜索实现。

<a id="tool-use-and-result-protocol"></a>
### 工具使用与结果协议

工具说明提供偏好，不改权限／DAG。已知路径直接读列，未知位置用 `find_files`/`glob`，内容／符号／正则用 `search`；已知文件仍可针对性查用法。已存在范围空结果是成功观测，避免无新证据重复；文件改变、模式／范围修订或用户显式验证可支持重搜。同轮相同发现保留复用。

Shell 用于构建、测试、Git 和命令行流程。常规发现优先专用工具，用户明确要求、专用不可用或需原生过滤／管道时可 shell 扫描，不设工具选择门槛。独立读取／搜索可批量。失败恢复用相关诊断、局部修复及适当检查，保留平台语法及权限要求。

`patch` 用原文件 1-based `start_line`、`delete_count`、`new_text`、`expected_lines`，`delete_count > 0` 时后者必需。编辑前读足上下文，必要时周边函数和调用方。所有 hunk 写前验证，冲突含位置及上下文。替换跟最后替换行 LF/CRLF；插入跟前行，文件首则跟首行，空文件 LF。非空替换后还有存活行时，缺终止符会补。无终止符末行后插入先终止原末行，沿前 LF/CRLF，无则 LF。未改字节行尾保持，混合文件也如此。输入／最终大小按 `MAX_FILE_BYTES`。成功 `regions` 为各变更区逐字编号文本，加两侧最多两行，使用最终坐标；纯删除显示新边界两侧两行。后续编辑前应查区域，成功不证明语法或构建。支持重复文本，不再用唯一 `old_text` 匹配。缺路径只返邻近候选，不递归搜。

局部编辑优先 `patch`；`filesystem.write` 创建或整体替换，适合用户要求全重写及生成产物，替换前查已有正文；不追加，沿原写权限。`filesystem.read` 可读行范围或按需全文，均返回原 1-based `N: line`。`N: ` 是显示元数据，补丁及 `expected_lines` 不带它。文本保留 CRLF、缩进、紧凑 JSON 和末换行。

文本以无损 `ToolResult` 保存，含 `status`（`success`/`error`）、`summary`、`diagnostics`、`raw_output`、`truncated`，模型投影来自 `ContextBudget`。工具由 `Tool::execute_output` 返回 `ToolOutput::Verbatim { text, total_lines, start_line, truncated, line_truncated }` 声明源码，默认仍普通文本。调度器在新增 `verbatim`、`total_lines`、`start_line` 保留声明；旧快照缺字段默认普通语义，不根据名称或正文猜。

成功逐字结果 `diagnostics` 为空，文件中的 `error`、`failed`、`conflict` 是源码。投影不解析／美化 JSON、不删 `warning:`。超过 `ContextBudget::tool_result_chars()` 只留完整前行，报 `total_lines`、`shown_through_line`、`next_start_line`、`truncated: true`，可用该 `start_line` 继续 `filesystem.read`。单行过大在 UTF-8 边界留行首，带 `line_truncated: true` 和短说明，`next_start_line` 指下一行，EOF 为 null。完整所需范围也为 null，即使还有后续行；仅预算／字节截断提供续读。行内深处用 `search`。逐字视图只含 `verbatim`、`output`、`total_lines`、`start_line`、`shown_through_line`、`next_start_line`、`truncated`、`line_truncated`、`raw_output_available`，因为 `status`、`summary`、`diagnostics` 对源码无信息。普通文本只在失败推断诊断，显式 JSON `diagnostics` 成功也保留，JSON 格式化及编译警告清理不变。原检查点权威，见 [ADR 0031](adr/0031-tool-declared-verbatim-output.zh-CN.md)。

`filesystem.read` 预查大小，复用 `search` 的 4 MiB `MAX_FILE_BYTES`。超大整读报 `invalid tool input`，含实际／上限及范围／`search` 提示。仍可范围流读，含编号最多保留 `MAX_FILE_BYTES`；截到行时 UTF-8 边界缩尾并设 `truncated`、`line_truncated`，不插源码标记。首次计余行但不保留，精确数在惰性进程 LRU 按规范路径、mtime、长度缓存，存储限 `MAX_FILE_BYTES`。命中停于范围末或保留限，元数据变化失效；无法检测外部 writer 恢复同时间／长度。

`tool_output(call_id, start_line, end_line)` 按需读保留原范围，恢复后仍在缓存也可。共用 4 MiB `MAX_RETAINED_OUTPUT_BYTES`，按 ID／原长度估算，FIFO 淘汰，单超限不缓存。未知／淘汰明确提示重读／重跑，不存无界墓碑。新逐字缓存留信封／元数据，旧字符串可读。文件范围用原坐标，保留前缀／终止符，不再加编号。普通 shell 保留 JSON／行号行为。类型化 DAG、JSON 工作清单、`_ax_observe` 经 `ToolResult::source_text()` 在副本移除精确前缀，原存储／模型可见保持字节。

Shell 非零退出失败。ACP 实时／回放保留失败、信封、ID 和工具名。`turn_changes` 对比当前执行前后，含创建、删除、干净跟踪及非 Git 文件，统一 diff 保存在既有历史，不暂存／撤销，不进模型上下文。既有编辑为基线，未动用户改排除；空最终快照清增量。二进制／非 UTF-8 明标，不虚构行数。Git 生成最小块，无 Git 为有效全文替换 diff。排除 Git ignore、`.git`、`.ax`、`AX_HOME`、`target`、`node_modules`、`__pycache__`。区间并发外部编辑也可能纳入，快照失败明确报，不当无变更。可选 `AX_EVENT_LOG` 记录 JSONL，不增加普通启动工作。`benchmark/README.md` 记固定本地基准及限制。

TUI 调用执行时显示查询／路径、文件操作或 shell 命令短说明；完成后短暂保留，再折为成功／失败行。CLI `run` 向 stderr 输出相同活动说明。


<a id="web"></a>
### Web

`web` 接受 `operation: "search"`、`queries`（1–4 字符串）、可选 `limit`（每查询 1–20）；或 `operation: "fetch"`、`urls`（1–6 HTTP(S)）。旧单数 `query`、`url` 作为别名规范到列表，单请求兼容。

搜索经普通内核／子组合绑定实际模型。协议符合时 `NativeFirst` 调官方服务搜索，不支持／拒绝／不完整／超时转 `SearchRouter` 的匿名 Exa/Parallel，最后 Keenable；用户密钥 Brave/Bocha 可选。原生账户可能产生正常搜索／推理费，匿名 Remote 无需额外密钥。网关兼容和模型名不代表原生支持，见[完整审计](web-search-audit.zh-CN.md)、[架构](web-search-architecture.zh-CN.md)。

查询并发、重复输入去重。`limit` 默认是不同有效 URL 总目标，`target_results` 可增至 `limit * query_count`。已完成按查询顺序合并，足量即丢剩余；`cancelled_queries` 标未完成。局部失败保留结果，报告 `succeeded`、`failed`、`errors`，全失败准确报错。

规范 URL 去片段和追踪参数，保留匹配查询、引用和其他后端来源。记录含 `title`、`url`、`snippet`、`source`、`matched_queries`，新 `evidence` 含平台原引用、grounding／用量和来源元数据。标记 `content_trust: untrusted_external_data`，检索指令不授权工具。域名受限服务搜索在联网前拒绝，因为本地无法限制远端访问站点。

原生健康绑定账户／模型，首失败在批次合并。任务 30 秒，原生最多 12 秒或一半预算，远程连接／读／总 5/10/15 秒。各远程三错后熔断冷却，429 立即遵守 Retry-After。达目标丢 future，单查询最多两对冲，无传输重放。schema／握手会话及健康跨调用、克隆、子共享；过期 MCP 下任务重建，不重搜。替换配置／客户端重置状态。代理／TLS 遵循 reqwest，拒重定向，正文有界。

正毫秒设置 `AX_SEARCH_NATIVE_TIMEOUT_MS`、`AX_SEARCH_TASK_TIMEOUT_MS`、`AX_SEARCH_<EXA|PARALLEL|KEENABLE|BRAVE|BOCHA>_{CONNECT_TIMEOUT_MS,READ_TIMEOUT_MS,TIMEOUT_MS}`。端点 `AX_SEARCH_EXA_URL`、`AX_SEARCH_PARALLEL_URL`、`AX_SEARCH_KEENABLE_URL`。嵌入方保留 `with_search_provider`、`with_client`、`SearchConfig`，端点 `None` 禁用候选。惰性共享路由保留可达性和 EWMA，`SearchRouter::stats()` 暴露。无页面引擎抓取、DOM 搜索解析或本地引擎回退。

Fetch 独立调度，最多三请求在途，`target_pages` 可提前返回并报 `cancelled_urls`，完成页保持原顺序和字符预算。取消停本地读取／重试，服务端已收工作无法撤销。

搜索失败时工具说明、错误和内置研究技能要求报告／配置／重试，不自动 shell+Python urllib、curl 或其他抓取引擎。是模型路由指引，非全局 shell 禁令；用户明确替代搜索／网络诊断可在权限内用。`web fetch` 仍为已知 URL HTTP 抓取。

Fetch 共用池、Keep-Alive、30 秒 TCP keepalive、有界 60 秒合并 DNS 缓存。每尝试连接／读／总 2/3/8 秒，仅超时／429/5xx 等 100 ms 重试一次，401/403/404 不重试。注入客户端保留传输配置。搜索共用池／DNS 机制，不共用激进超时／重试。代理／TLS 为 reqwest 默认。

Fetch 只 GET，最多五次重定向，拒非文本，每正文限 2 MB，移注释及 `script`/`style`/`nav`/`footer`/`aside` 后 HTML 转 Markdown。页返回 `url`、有则 `title`、`content_type`、`content`、`truncated`，每页最多 20,000 字符、每调用 60,000，截断明确 `truncated: true`，不静默切，不执行浏览器 JS。

错误含 `url`、完整错误链 `error`、`kind`、短显示 `reason`、外层到根因 `source_chain`。`kind` 为 `dns`、`connect`、`timeout`、`tls`、`proxy`、`redirect`、`http_status`、`unknown`，HTTP 另含 `status_code`。优先 reqwest 类型标志，DNS/TLS/代理按来源文字识别，其他 `unknown`，不按 URL 分类。全失败仍工具错误，诊断含同结构；部分失败在成功结果 `errors`。

TUI 短行如 `web.fetch · failed (connect timeout)`，Ctrl+O 展开／折叠最新诊断，显示完整 URL 和底层错误；原行进入 scrollback 后最新诊断仍可用。ACP 完成更新在 `rawOutput.errors` 暴露。

```json
{
  "operation": "search",
  "queries": ["Rust 1.94 release", "Rust 1.94 changes"],
  "limit": 5
}
```

```json
{
  "operation": "fetch",
  "urls": ["https://example.com/a", "https://example.com/b"]
}
```

`queries` 并发，URL 并发抓取，按 URL 去重，局部失败不取消成功结果。

<a id="lsp-through-mcp"></a>
### 经 MCP 使用 LSP

AX 无 LSP 客户端，不直接启动语言服务器。配置暴露语言操作的独立 MCP，经普通 `mcp` 或代理发现调用。操作、格式及编辑行为由服务决定，配置权限见 [mcp.md](mcp.zh-CN.md)。

<a id="images"></a>
### 图像

`view_image` 检查规范工作区路径、签名和 10 MB 上限，支持 PNG/JPEG/WebP/GIF。返回图像内容，OpenAI Responses 在工具结果以原生 `input_image` 发送。提供商无视觉则内核隐藏。原图字节留会话元数据；图像 Token 依赖大小／处理，因此预算采用近似。

<a id="capabilities"></a>
## 能力

权限检查共用词汇：

| 能力 | 键 |
|---|---|
| Shell | `shell` |
| 文件读取 | `filesystem-read` |
| 文件写入 | `filesystem-write` |
| 网络 | `network` |
| MCP | `mcp` |
| 启动进程 | `process` |

<a id="permissionstore"></a>
## PermissionStore

`PermissionStore`（`tool::permission`）是**唯一**来源，UI／运行时克隆同一 `Arc<RwLock<..>>`，变更即时互通。

- **默认**：`filesystem-read`、`network` 为 `Allow`，其他 `Ask`。
- **决定**：逐能力 `Allow` / `Ask` / `Deny`。
- **会话授权** `allow_session`：本会话临时把 `Ask` 升为 `Allow`，开始／恢复会话自动清，不能盖 `Deny`。
- **策略**：`ApprovalPolicy` 实现 `AllowAll`、`DenyDangerous` 读同库，`--allow-dangerous` 选 `AllowAll`；`DenyDangerous` 拒 `RequiresApproval`。

<a id="user-controls"></a>
## 用户控制

- **`/permissions`** 查看修改能力策略，选择 `Allow` / `Ask` / `Deny` 更新共享 `PermissionStore`，影响后续审批。
- **`/tools`** 查看内置、已发现代理、惰性 `mcp`。详情是真说明／JSON schema，MCP 还含服务和远端名。浏览不连接，也不声称对所有可能调用有静态权限结论。

<a id="telemetry"></a>
## 遥测

`tool::telemetry` 为 `/status` 提供无敏感数据进程内 `Timer` / `snapshot` / `increment`。各模型步／工具计时；批量 `web` 另记录 `web.search`、`web.fetch` 总量，`web.search.query.N`、`web.fetch.url.N` 延迟，`web.search.ok` / `web.search.failed` / `web.fetch.ok` / `web.fetch.failed` 计数。

网络增加 `web.http.dns`、`web.http.connect`、`web.http.ttfb`、`web.http.total`，以及 DNS 命中、重试、回退、取消计数。DNS 为真实解析；connect 含按需 DNS／代理／TLS；TTFB 为响应头到达，因为 reqwest 无首 socket 字节。Total 含全部尝试、退避、正文。阶段重叠不可相加，连接复用／IP 可能无 DNS 或建连，不存 URL、查询、凭据。基准见 [web-latency.md](web-latency.zh-CN.md)。

<a id="concurrency"></a>
## 并发

每模型工具轮建 DAG，有界运行就绪独立调用，默认四，`AgentKernel::with_tool_concurrency` 可调，最多 64。用任务内 future，不逐调用 spawn。权限不变，交互审批串行，批准工作并发。

`Tool::resources(input)` 声明读写。文件读／列、搜索、图像共用路径读租约，写／补丁独占。规范路径、Windows 大小写、目录重叠防别名或父子绕冲突。Memory 用实际 SQLite 路径，跨作用域／内核写串行，读可并行。MCP 目录读可并行，list-tools 修改相应管理资源。Shell 含 Git 及未声明工具持全局独占，不能从命令、名称或权限安全推断影响。只读也须显式声明才并行。租约跨 fork 内核进程级共享，成功、错、超时、取消释放，不协调外部进程。

参数可含 `{"$tool_result":"producer-id","pointer":"/url"}` 类型引用，检测依赖、等生产者、替换 JSON 值，无 pointer 为整文本／JSON。保留 `_ax_depends_on` 数组增成功依赖，权限／执行前移除。schema 明示，不靠提示调度。未知／重复 ID、环在执行前拒绝。输入依赖失败阻止依赖操作；仅共享资源的后续独立调用不因前写失败被禁。

生命周期带原 `tool_call_id`，同名也区分。结果完成即检查点，原历史按完成，下请求按原调用序。轮前核工具预算，中断保留恢复占位。

<a id="reference"></a>
## 参考

| 职责 | 代码 |
|---|---|
| `Tool`、`ToolRegistry`、`SafetyLevel` | `crates/tool/src/lib.rs` |
| 内置工具 | `crates/tool/src/shell.rs`、`filesystem.rs`、`patch.rs`、`find.rs`、`search.rs`、`web.rs`、`view_image.rs` |
| 发现遍历／glob | `crates/tool/src/discovery.rs` |
| 共享注册表路径绑定 | `crates/tool/src/workspace.rs`、`crates/tool/src/sandboxed.rs` |
| `Capability`、`PermissionStore` | `crates/tool/src/permission.rs` |
| 遥测 | `crates/tool/src/telemetry.rs` |
| 调度、资源锁、同轮复用 | `crates/core/src/scheduler.rs`、`crates/tool/src/resources.rs` |
| 注册组装和审批 | `crates/cli/src/runtime/builder.rs`、`crates/cli/src/bootstrap.rs` |
| 控制工具 `task_queue`、`child_result`、`request_user_input` | `crates/core/src/task_queue.rs`、`crates/core/src/child_result.rs`、`crates/core/src/user_input.rs` |
| 并行子分发／回执 | `crates/core/src/child_dispatch.rs`、`crates/core/src/child.rs` |
| 项目指令 | `crates/core/src/instructions.rs`、`crates/cli/src/project_instructions.rs` |
| `/permissions`、`/tools` | `crates/cli/src/tui/commands.rs`、`crates/cli/src/tui/commands/catalogs.rs` |

<a id="shell-and-child-runtime-awareness"></a>
## Shell 与子运行时感知

工具说明／schema 暴露真实平台 shell。Windows 用 `powershell.exe`，即 Windows PowerShell 5.1，非 Bash 或 PS7。未加引号 Bash heredoc `<<`、`&&`、`||` 在创建前拒绝并提示恢复，保留引号字符串／Python 内容，可 here-string 管道到 Python 或分调用。Unix POSIX `sh`，动态元数据非新系统提示。

扩展 `Tool::fork_for_run` 须按给定 `RunContext` 重绑。`SandboxedTool::fork_for_run` 委托所包 `fork_for_run` 再施边界，子得到同工具绑定 `child.cwd`，不另实现；发现保留绑定，`filesystem` 经 `WorkspaceTool` 重绑。内置拒子根外路径含符号链接，资源租约按绑定路径。Shell 继承已装 runtime/PATH，但用子 cwd、AX_HOME／Session／Memory。AX_HOME／数据库绑工作区外 `RunContext.state_dir`，保留普通审批及 DAG，不引入 OS 容器。

<a id="execution-scope-and-advisory-recovery"></a>
## 执行范围与建议性恢复

普通参数可 `_ax_execution` 含 `goal_id`、`step`、`expected_output`、`scope`。先替换结果引用，再验证绑定／路径，执行前去运行专属元数据。新步骤可选初始工作区内目录，子范围建议，但实际根始终强制，越界拒绝。`_ax_observe` 可指实际 true 布尔 JSON pointer 证明输出，重复证据不重置停滞。每完成结果含空／失败都是观测。

进度、恢复、重试、重规划属编排。停滞／重复失败只建议，不拒执行。可重试、search/read/list/shell/MCP 诊断、换步骤或失败后继续独立项。恢复记录步骤／调用／资源，不收窄或全局锁；旧失败收窄快照准入前恢复原范围。权限、沙盒、租约、依赖、预算／超时、取消独立硬约束。

`runtime_owned_resources` 显式声明工具自有存储，memory 用于私有库，不含调用方选择路径。嵌入方 `AgentKernel::with_execution_scope` 选初始工作区并绑定相对路径。未知／全局副作用仍独占并遵守声明权限／沙盒。

嵌入式非 harness 递归搜索遵守当前声明范围，更广可显式新步骤。`search` 和 `find_files`/`glob` 以解析范围读资源声明，同范围检查拒当前步骤外。旧 `fallback_reason` 仍接受，但不授权或控准入。发现跳生成／vendor／cache、符号链接、二进制、超过 4 MB 文件，至 `max_results` 停，`search` 默认 100，`find_files` 200；限额返部分／截断，不报错。


<a id="subagents"></a>
## 子代理

接入 `ChildHost` 默认注册委派工具，一实现实例化两工具，`max_depth = 0` 同时禁用，不初始化命名元数据或准备子执行：

- `subagent(task, context?, tools?, policy?)`：**全新上下文**，无父对话，默认隔离记忆／工作区。任务需自包含，Agent 模板进一步收窄。
- `subagent_fork(prompt, description?)`：以**父已完成轮次**初始化，绝不继承当前在途轮。共享工作区／提供商，使前缀仍适合服务端缓存。固定契约，不选模型、不收窄工具。

可调并发池／深度。`/settings` 改全局；CLI `ax settings --max-concurrent 8 --max-depth 2`。`--scope project` 读写发现项目 `.ax/config.toml`，省字段继承。`--reset` 恢复全局 8 / 1，或移项目两个覆盖，不动其他配置。Crew 设置 → 插件 → Agents 选全局／项目。下一轮加载，运行中 TUI/ACP 也适用。JSON：

```json
{
"subagent": { "max_concurrent": 8, "max_depth": 1 }
}
```

旧 `config.toml` 的 `enabled` 经现有加载迁移，该键忽略。

原语 `spawn_agent`、`wait_agent`、`cancel_agent` 位于内核或可选 manager。嵌入方附 `ChildHost` 后先 `prepare_subagents` 再直接调用；普通轮次自动准备。

独立调用共用 DAG／有界并发。全部层普通与队列子共用 `max_concurrent`，默认 8，1–64；配置中和等待后代的祖先都计。根可排队，嵌套耗尽拒绝，不等占槽祖先。普通整树最多准入 64，队列可超过 64 并复用槽。`max_depth` 非负，0 禁、1 直接子、更大允许到绝对深度。达上限两工具消失，队列子分发拒绝。主模型跨研究／写作／分析／规划／软件判断收益，无职业或复杂阈值。小／紧耦合留主代理，用户独立或顺序优先。改上限不创建子或停止活跃轮。

工具白名单与子重绑注册表取交，未知工具／超深度拒绝，空 tools 禁全部含委派。继承提供商／模型／推理和父审批，工具及历史不跨子会话共享。各子上下文含委派契约：起始权限范围固定，拒绝动作不重试，限制向委派者报告。

各工具返 `{status, summary, artifacts, error}`，状态 `completed`、`failed`、`cancelled`；host 提供时 artifacts 指持久状态目录。可丢修改不合回父。父模型只接最终回执，原历史留子库。ACP 可独立显示公开文字、工具、生命周期，使用 `SubagentActivity` 和父恢复索引，见 [ACP](acp-crew.zh-CN.md#delegated-agent-activity)。事件 `subagent_started`、`subagent_progress`、`subagent_completed`、`subagent_failed`、`subagent_cancelled`。TUI 简报生命周期，不含子文本／输入／推理。

取消／超时停止既有循环，级联后代并关回执。共享工作区子通过自身副作用工具获租约，委派等待不持锁。原语超时含准入／配置，模型工具子继承子超时。丢父工具轮取消未完成委派，提供商、配置、回执及 worker 失败局部封装返回。

<a id="permission-profiles-and-sandbox-boundaries"></a>
## 权限 Profile 与沙盒边界

执行前，结果引用解析后，Runtime 对解析参数和资源评估 `PermissionProfile`。`RuleMatcher` 支持命令 glob／前缀、规范路径 glob、域名 glob、JSON pointer 参数。重叠按 **deny > ask > allow**，与顺序无关。能力 Deny 始终上限。显式 Ask 即使能力有会话 Allow 仍需交互，批准不授权其他调用；非交互 Ask 拒绝。规则在 Runtime／传输强制，不靠指令。

```json
{
"permissions": {
  "rules": [
    {"decision":"allow", "matcher":{"kind":"command","pattern":"cargo *"}},
    {"decision":"ask", "matcher":{"kind":"prefix","value":"git push"}},
    {"decision":"deny", "matcher":{"kind":"path","pattern":"~/.ssh/**"}},
    {"decision":"deny", "matcher":{"kind":"path","pattern":"/etc/**"}},
    {"decision":"deny", "matcher":{"kind":"domain","pattern":"*.internal.example"}},
    {"decision":"ask", "matcher":{"kind":"tool_parameter","tool":"mcp*",
       "pointer":"/arguments/action","pattern":"delete"}}
  ],
  "boundary": {"read_only":false, "deny_network":false}
}
}
```

路径展开 `~`、解析别名／已有符号链接、规范缺失分量，父树／未知资源不能绕拒绝子树。Command Allow 对复合 shell／替换语法仍需 Ask。任意 shell/Process/MCP 无法由参数确定影响，限制性路径／域名规则保守拒绝或询问。OS 沙盒独立追加，规则不能放松或选不可用后端。Fetch 逐跳检查，新 Deny/Ask 主机停止重定向。Search／未知网络无法强制域名则拒绝，不加特权代理。

<a id="childpolicy-inheritance"></a>
## ChildPolicy 继承

各显式子代理用同一 `ChildPolicy`：

```yaml
context: none          # none | summary | last_n | full
last_n: 0             # positive complete-turn count when context=last_n
memory: isolated      # none | parent_readonly | isolated | shared_project
skills: none          # none | selected | inherit
selected_skills: []
tools: inherit        # none | selected | inherit; only child-bindable parent tools
selected_tools: []
mcp: none             # none | selected | inherit
selected_mcp: []
model: inherit        # inherit | override
model_override: null  # parent-registered key
workspace: isolated   # shared | snapshot | isolated
permissions: inherit_restricted # inherit_restricted | custom
custom_permissions: { rules: [], boundary: {} }
```

默认保留旧安全隔离和不递归。旧 `context` 为显式任务，`tools` 为额外收窄。父历史只读并去队列／执行状态，last_n 保完整轮。Memory `none` 移访问，`parent_readonly` 拒写，`shared_project` 仅父项目存储且用子输入来源；隔离记忆有自身 Session／库。技能保留父合格目录，选中正文才读。未绑定扩展不静默继承。

LocalChildHost 将 snapshot/isolated 映射既有过滤配置器，即 Git worktree 加脏输入，否则过滤副本。Shared 将工具绑父工作区，另保子持久状态和可丢生命周期目录，清理不删父。Host 无法实现即拒。MCP 继承需显式共享工作区，因为服务资源是父作用域。全部子保留父有效 profile 和能力审批对象，自定义只可加限制。

父须配置模型覆盖。嵌入方 `register_child_model`，CLI 可选本地 `child_models`：

```json
{
"child_models": { "reviewer": { "provider":"openai", "model":"gpt-4.1" } }
}
```

构建获准提供商不发模型／网络，未知 key 拒绝，子不能创建凭据或提供商。

<a id="explicit-task-queue-planning"></a>
## 显式任务队列规划

格式不创建任务。`TaskQueue::list_hints()` 仅可选元数据，不调度／spawn。主模型显式 `task_queue(action="start", overall_goal, tasks)`。默认 `execution="controller"`，新 `ax_work_items` 也如此；只有 `execution="children"` 经 ChildHost 委派。`action="configure"` 必须 `execution`，修改活跃模式，保留 ID、依赖、进度、回执。追加类型化记录在现限制内保已有显式子选择。`dependencies` 为每任务一组 zero-based 先前索引，无效／前向拒绝；失败跳依赖者，独立继续。队列只管状态、依赖、调度、结果。旧序列化保状态，旧回执使显式 Goal 恢复在现限制内继续委派。深度 0 或继承空委派工具阻新及恢复分发，禁用恢复保清单／句柄并反馈，回控制器直接执行。

每任务可选 `resources`，纯字符串为路径读取，对象带 `path` 或 `name` 及 `write`。冲突不并行，共享文件须声明，不假定安全。整就绪无冲突同批，完成即接新项，使用普通子池默认 8，不另用提示或队列额度，嵌入 `with_child_concurrency` 同上限。队列委派走普通工具 Process/Safe 权限，含 profile Deny/Ask 和保存决定；当前控制器选择分发前授权一次，非逐项，新主机轮次重检。可用性不是授权。

<a id="control-tools-and-structured-receipts"></a>
## 控制工具与结构化回执

三工具由内核处理，不是注册项，因为改变编排而非外部世界：

| 工具 | 用途 |
|---|---|
| `task_queue` | 创建／追加、配置、完成、阻塞或取消 Goal 任务。 |
| `child_result` | 按 `child_id` 读 `full`、`findings`、`diagnostics`、`artifacts`、`diff`、`validation`、`metrics`，省 `child_id` 列表。 |
| `request_user_input` | 答案实质改变结果时提一个结构问题，含 `question`、`options`、`allow_free_text`。 |

必须各自独占轮次，混合整轮拒绝且无调用执行。只供主代理；子无队列，不能挂父或编排。

`request_user_input` 非权限请求，问题不授／拒任何权。提问保存 Goal／队列／Session／问题，挂 `waiting_for_user`，答案写同 ID 原位续接，无新轮。前端经 `AgentEvent::UserQuestion`：TUI 下一提交当答案，ACP 发 `agent_question`，非交互 `ax run` 从 stdin 读。

子 `ChildResult` 只向父投影 `model_summary()`，全记录持久经 `child_result` 读，不重跑恢复细节。旧进程回执短路准备，恢复不重复完成子。墙钟、模型轮、工具数、报告 Token、失败数来自子事件和对话。

<a id="sandbox-enforcement"></a>
## 沙盒约束

Permission 表达 AX 是否愿意请求，Workspace Sandbox 是下方 OS 边界。Worker 常规开发不反复审批，显式拒绝仍有效；减少审批按真实绑定受限策略，不按进程模式，未准备／未限制保自身审批。逃逸返 SandboxViolation，不自动主机重试。新本地副作用需声明 worker 边界并用 SandboxManager，不另加字符串检查或 host spawn，见[安全](security.zh-CN.md)。

运行时将步骤子范围作建议，但始终强制初始根，避免猜错嵌套范围锁恢复，见 [coding-harness.md](coding-harness.zh-CN.md)。

<a id="ssh-execution-context"></a>
## SSH 执行上下文

AX_SSH_CONTEXT_FILE 指向 Crew 给本地 AX 的 JSON 清单，避免大目录命令／环境长度限；AX_SSH_CONTEXT 也可直接 JSON。两者含 hosts 的 id/name/host、可选 port/identity_file、default_host 及远 cwd。格式错失败，不回退本地文件。远程工具集是 ssh/web，不暴露本地 shell/filesystem/patch；推理和凭据本地，无需远 AX。

ssh action=list 返回 ID／名／端点，不含密钥路径。action=exec 接 command、可选 host_id/cwd/timeout_seconds，默认所选主机／cwd，其他主机默认登录目录。命令 stdin 传固定 OpenSSH 远 sh，不经本地 shell。结果 host_id/cwd/exit_code/stdout/stderr。需 Shell 审批，只读／无网络 profile 拒绝，有域名规则也因不能 URL 过滤而拒绝。

无配置主机数量上限或独立主机工具池上限。同主机资源串冲突，不同主机并发。超时杀本地 SSH，远进程清理依赖远 SSH server／shell 断连处理。

<a id="optional-long-project-tool"></a>
## 可选 Long Project 工具

显式启用 Goal 注册运行时所有 `long_project`，支持证据章节笔记、当前文档／修订、归档及同 Goal 历史 Session 分页原回执。Scheduler 租约和声明文件读写仍生效。笔记不能批准完成或替 Task，拒凭据／未验证来源。子不继承，普通任务不注册，参数和冲突见 [Long Project](long-project.zh-CN.md)。
