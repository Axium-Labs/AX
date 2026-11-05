<a id="pull-request-workflow"></a>
# Pull request 流程

[English](pull-requests.md) | **简体中文**

[贡献入口](../../CONTRIBUTING.md) · [导航](README.zh-CN.md)

<a id="confirm-scope"></a>
## 确认范围

先搜索 issue 和 PR，按 [issue 策略](issues.zh-CN.md)讨论或认领；重大变更实现前取得维护者明确设计同意。局部小修复无需单独 issue。每个 PR 聚焦一个可审查结果，不相关清理另拆。

<a id="fork-and-branch"></a>
## Fork 与分支

在 GitHub fork [AX](https://github.com/Axium-Labs/AX)。将 `YOUR-USERNAME` 换成账户名，在保存源码的目录执行：

```bash
git clone https://github.com/YOUR-USERNAME/AX.git
cd AX
git remote add upstream https://github.com/Axium-Labs/AX.git
git fetch upstream
git switch -c fix/short-description upstream/main
```

已有克隆则 fetch upstream，以干净工作树从 `upstream/main` 新建分支。不相关本地变更放在独立分支。

<a id="develop-against-axs-contracts"></a>
## 按 AX 契约开发

环境配置、构建／测试命令、lint 和实现约定见[开发](../development.zh-CN.md)。crate 职责和运行时约束见[架构](../architecture.zh-CN.md)与相关[专题](../README.zh-CN.md)；本指南不重复定义技术标准。

所有代码变更都需适当测试，或验证已有覆盖。可行时为 bug 增加回归测试；复现无法自动化需解释。按适用范围覆盖成功、错误和取消路径。随代码更新行为、接口及配置文档。长期架构决策遵循 [ADR 流程](../adr/README.zh-CN.md)，链接设计 issue，由维护者确认接受。

<a id="test-isolation"></a>
### 测试隔离

遵循[开发文档的构建与测试契约](../development.zh-CN.md#build--test)。公开可复现覆盖使用现有已跟踪 Cargo 测试。新增私有回归／集成 harness 保存在本地 `test/`，不上传私有测试脚本、fixture、对话记录或日志。生产模块路径、`include!` 引用或 Cargo target 不得依赖未发布测试文件。现有已跟踪测试仍属于公开构建，不得为通过而移除或禁用。新公共覆盖若需不同测试组织，先与维护者讨论。PR 描述私有测试场景和结果，使维护者可评估覆盖，又不让 CI 依赖私有文件。准备提交时保留有价值的本地测试。

<a id="local-validation"></a>
## 本地验证

从仓库根运行[开发文档](../development.zh-CN.md#build--test)规定的构建、完整工作区测试、Clippy 和格式检查。只读格式检查用 `cargo fmt --all -- --check`。[PR CI](../../.github/workflows/ci.yml) 运行相同四项，对 Cargo test、Clippy 和 build 使用 `--locked`，采用已提交 `Cargo.lock`。本地复现 CI 也使用该标记。依赖仅在有意变更时修改锁文件，并在 PR 解释。

同时运行相关集成及平台检查。提供商 smoke check 需自有配置，绝不提交凭据。记录系统、工具链、精确命令、通过／失败／忽略结果，以及不可用服务或平台检查。编译通过不等于测试通过。

提交前，用仅包含拟提交文件的干净 checkout 验证：不使用私有 fixture 或未跟踪文件，执行构建、测试编译和静态检查。准备 PR 时在其中运行完整工作区测试，独立验证公开构建不依赖本地覆盖。仅凭 `cargo test --workspace` 不可声称有私有集成覆盖。

<a id="commit-and-submit"></a>
## 提交与发起 PR

检查 `git status --short`、`git diff` 和 `git diff --check`。只暂存拟提交源码、公共文档及 GitHub 配置路径，再检查 `git diff --cached` 和 `git diff --cached --check`。排除本地日志、私有测试、凭据和生成文件，避免无差别 `git add .`。

按逻辑组织提交，用简洁标题说明结果。`memory:`、`core:`、`docs:` 或 `ci:` 等前缀有助于识别区域，但不是强制语法。不明显的动机在正文解释。然后推送：

```bash
git commit -m "docs: clarify contribution workflow"
git push -u origin fix/short-description
```

从 fork 分支向上游 `main` 发起 PR。填写 [PR 模板](../../.github/PULL_REQUEST_TEMPLATE.md)：目的、相关 issue 及设计同意、实现、架构影响、测试证据、兼容性与风险。不适用项使用 `N/A` 并说明原因。未完成使用 draft，可审查后标记 ready。涉及数据迁移、协议／API／配置兼容或性能时说明证据，明确剩余限制。

<a id="review-and-update"></a>
## 审查与更新

遵循[审查策略](reviewing.zh-CN.md)。回应实质评论，解释修复或不同意见。推送聚焦的后续提交，概述修改和重跑检查。优先保留审查历史；改写已审分支前先协调，取得同意后使用 `--force-with-lease`。需要解决冲突时 fetch upstream 并合并 `upstream/main`，重跑受影响验证，保留他人成果。

CI 在 PR 创建、更新、重开及准备审查时运行，包含只读权限且无项目秘密的 fork PR。基线运行于 Windows，匹配受支持原生工具链；已有[平台工作流](../../.github/workflows/platform-check.yml) 对其配置路径保留 Windows、Linux、macOS 原生检查。基线通过不证明 Linux 隔离或 macOS 桌面正确。CI 修改不得削弱检查或向 fork 代码暴露秘密。

CI 失败时查看失败步骤，尽可能本地复现并推送修复。怀疑无关失败时附证据报告维护者，不跳过失败测试或放松 lint 求绿。新提交需要重新 CI，并审查修改区域。

<a id="merge-and-follow-up"></a>
## 合并与后续

满足[合并条件](reviewing.zh-CN.md#merge-gate)后，由维护者最终决定接受、拒绝或合并。维护者可要求缩小范围，或说明原因后关闭 PR。设计同意、社区批准和检查通过都不能单独授权合并。

维护者选择合并方式并保留贡献者署名。合并后检查关联 issue 是否解决，以 PR 引用报告回归，可删除已合并 fork 分支。PR 合并不等于发布，版本及独立 tag 驱动发布由维护者管理。
