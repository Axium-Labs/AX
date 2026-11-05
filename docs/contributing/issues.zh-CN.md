<a id="issues-and-design-discussion"></a>
# Issue 与设计讨论

[English](issues.md) | **简体中文**

[贡献入口](../../CONTRIBUTING.md) · [导航](README.zh-CN.md)

<a id="search-and-report"></a>
## 搜索与报告

先搜索 [issue](https://github.com/Axium-Labs/AX/issues)，包括已关闭报告，以及 [PR](https://github.com/Axium-Labs/AX/pulls)。已有同问题报告时补充证据。阅读相关[专题文档](../README.zh-CN.md)，区分预期行为、已知平台限制与缺陷。文档未回答的使用问题也欢迎附上下文提 issue，无需另寻服务。

使用 [bug 表单](https://github.com/Axium-Labs/AX/issues/new?template=bug_report.yml)，提供 AX 版本／提交、系统／架构、源码构建的 Rust 版本，以及相关提供商／协议和沙盒模式、最小复现、预期与实际结果、脱敏诊断。优先使用隔离工作区及合成数据，不用真实会话数据库。不要公开 API 密钥、认证文件、私有提示、记忆内容或个人路径。怀疑可利用漏洞时，如可用则使用 GitHub 私有漏洞报告；否则先请求维护者私人联系方式，不公开利用细节。[运行时安全文档](../security.zh-CN.md) 描述实现限制，不承诺已有披露服务或响应时限。

使用[功能表单](https://github.com/Axium-Labs/AX/issues/new?template=feature_request.yml)，说明用户问题、具体流程、替代方案及建议范围，写出已知受影响 crate／契约和兼容、安全、性能影响。不了解全部实现也可以。

<a id="design-discussion"></a>
## 设计讨论

实现新功能、架构调整，或对 Runtime、Agent Loop、Memory、Storage、Permission、Sandbox、Provider、MCP 作重大变更前：

1. 新建或加入 issue，说明问题、范围、替代方案及设计，指出受影响契约、数据格式、风险和验证计划。
2. 与审查者、维护者讨论权衡，关联架构文档和 ADR；有长期影响的决策应提议 ADR。
3. 等待维护者在 issue 中明确同意设计及范围。沉默、分配负责人或社区兴趣不等于设计批准。
4. PR 链接该同意；实现若实质改变已同意设计，返回 issue 讨论。

小型文档修正、独立 bug 修复及回归测试可直接提交。仅因位于某子系统，不代表修复重大；改变语义、公共契约或安全边界才是关键。

<a id="claim-and-coordinate-work"></a>
## 认领与协调

处理已有 issue 前，留言说明方案和范围，检查是否已有实现者并协调。权限允许时维护者可分配 issue，但分配不是前提，也不替代所需设计同意。关联 PR。

停止工作时留更新，方便他人继续。接手停滞工作前先询问贡献者计划，无回应时由维护者澄清归属。复用成果保留署名。不保证审查或完成期限。

<a id="triage-and-resolution"></a>
## 分类与解决

任何人都可复现报告、简化复现、对比版本或推荐相关文档及 issue。结论附证据，分歧针对变更而非个人。维护者决定优先级、范围、分配和关闭。报告可能因重复、预期行为、超范围或无法充分复现而关闭，应给出原因及相关链接；新证据可支持重新打开。

修复遵循 [PR 流程](pull-requests.zh-CN.md)。仅合并后应解决 issue 时使用 `Fixes #123`，部分工作使用 `Refs #123`。受支持平台上的验证是有用的后续证据。
