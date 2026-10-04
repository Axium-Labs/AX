## 2026-10-05

Task: Fix native/resource-scoped model provider integration after comparison with pi.

Changed:
- `crates/model/src/native/`: Anthropic Messages、Gemini/Vertex GenerateContent、Bedrock ConverseStream、Radius Pi Messages 原生流式适配；统一消息、图片、工具、usage、错误与签名回放。
- `crates/model/src/adapter.rs`, `providers.rs`: 共享适配器工厂；Cloudflare Gateway `/compat` 端点与专用认证头；Azure 资源端点及 deployment 映射；支持状态与缺少资源字段分离。
- `crates/model/src/openai.rs`, `openai_compatible.rs`: Azure Responses 身份/认证、可注入 HTTP 客户端；Gateway 专用头。既有 OpenAI/Codex 与 Workers AI 推理格式保留。
- `crates/cli/src/providers.rs`, `model_selection.rs`, `runtime/builder.rs`, `tui/catalog_refresh.rs`, `tui/commands.rs`, `acp.rs`: native/provider 检测与统一运行时/目录调度；缺配置保留 warning；Radius 初始无目录进入选择/发现；模型切换同步输出限制；ACP 增加 `configuration_reason`。
- `crates/model/src/lib.rs`, `crates/core/src/loop_runtime/model_step.rs`, `token.rs`: 可选 native assistant 元数据持久化、同 provider/model 回放、计入 context 预算。已有 Message/ModelResponse 构造点补充 `None`，旧会话兼容。
- OAuth 登录能力仅公布 AX 已实现的 Codex、WorkBuddy International/China；其他 API Key 入口继续可用，移除无效 OAuth 选项。
- `crates/model/Cargo.toml`, `Cargo.lock`: 官方 AWS credential chain / SigV4 / Smithy event frame、Google service-account JWT；锁定 Rust 1.92 可构建版本。
- `test/providers/`: 19 项新增回归，测试与生成的测试专用 RSA fixture 均集中在此目录。
- `docs/providers.md`, `docs/architecture.md`, `docs/adr/0020-native-model-providers.md`, ADR 索引同步。

Why:
- 原来多数 native provider 只有 pi 目录元数据，没有 runtime adapter；Azure/Gateway 没有资源配置；Workers AI 缺 account id 被错误标为不支持。
- 只解除 UI unsupported 标记不足以接入。协议、认证、端点、工具 history、签名和目录刷新均需一致。
- Radius 当前 pi 实现使用 Pi Messages，不是 OpenAI Chat Completions。

Reference:
- pi `200387122ca450d6387f033949423114a270b96c`，官方 provider/API 源码。
- Cloudflare Unified API、AWS Bedrock ConverseStream、Google Gemini thought-signature 官方文档。
- 核对官网现有 provider 概述，已有描述仍真实；未改变 AXCrew 接口消费方式，也未修改 AXCrew 或官网。

Tests:
- `cargo test --workspace`: PASS，586 passed / 0 failed / 3 ignored（新增 19 项）。
- 覆盖原生厂商 dispatch、Workers AI 既有请求、专用 auth headers、Azure deployment、分页目录、UTF-8/SSE 与 AWS binary frame、工具参数与签名重载、ADC refresh/cache/JWT、429/Retry-After、流内错误和截断流、native context 预算。
- `cargo clippy --workspace --all-targets`: PASS；本次新增 model 模块/测试无警告，工作区仍有既存 warning。
- `cargo fmt --all --check`: PASS。
- `git diff --check`: PASS。
- 生成 `target/debug/ax.exe` 开发构建；未替换安装目录程序，未提交或发布。

Issues / limits:
- 没有使用真实厂商凭据或发起付费推理；本地模拟通过不代表账号权限、额度、地域模型授权已验证。
- Anthropic / Radius browser OAuth 和 Google external-account workload-federation ADC 未实现；API Key、Vertex authorized-user/service-account/metadata ADC、Bedrock 官方凭据链已实现。
- Vertex 仅 Google publisher 模型；Bedrock 仅 ConverseStream 模型。Vertex/Bedrock 离线目录不会伪装为 live 验证。
- Azure、Gateway、Workers AI 仍需 docs 所列的资源/account/gateway 配置；缺配置时返回明确错误。
- GitHub Copilot 未实现，仍为 unsupported。
- 保留此前 web search 路由及工具指导未提交修改。

Next:
- 用真实凭据配置 docs 所列字段后，进行用户账号的实际推理验证；需要重新构建/使用新版 AX 才能让界面显示新支持状态。
