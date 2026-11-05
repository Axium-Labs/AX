<a id="contribution-guide"></a>
# 贡献指南

[English](README.md) | **简体中文**

从 [CONTRIBUTING.md](../../CONTRIBUTING.md) 进入。本目录介绍从报告问题到维护者合并的协作流程。

- [Issue](issues.zh-CN.md)：搜索、报告、讨论设计、认领工作与后续跟进。
- [Pull request](pull-requests.zh-CN.md)：Fork、分支、开发、验证、提交、发起 PR、回应审查及准备合并。
- [审查](reviewing.zh-CN.md)：角色、标准、分歧处理和合并决策。

<a id="technical-references"></a>
## 技术参考

环境要求、Cargo 命令、编码约定和扩展示例见[开发](../development.zh-CN.md)。编码前阅读[架构](../architecture.zh-CN.md)及[文档导航](../README.zh-CN.md)中的相关专题。重要架构决策通过 [ADR](../adr/README.zh-CN.md) 记录。

<a id="workflow-scope"></a>
## 流程范围

小修正可直接提交 PR。新增功能及架构或核心子系统契约的重要变更，需先提 issue 并获得维护者明确设计同意，见[设计讨论](issues.zh-CN.md#design-discussion)。不要求额外贡献者等级或一律预审批。

独立 [PR CI](../../.github/workflows/ci.yml) 检查格式、工作区测试、Clippy 和 Release 编译。现有[平台检查](../../.github/workflows/platform-check.yml) 保留原生平台验证。[发布自动化](../../.github/workflows/release.yml) 仍由 tag 驱动，不替代贡献检查。

<a id="organization-reference"></a>
## 组织方式参考

Issue 讨论、实现和审查分离的方式参考了 [Tokio 贡献指南](https://github.com/tokio-rs/tokio/tree/master/docs/contributing)。AX 策略针对本工作区及运行时契约，Tokio 的技术命令和治理角色不适用于 AX。
