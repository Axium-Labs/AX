<a id="0022-lazy-session-mods"></a>
# 0022：惰性会话 Mod

[English](0022-lazy-session-mods.md) | **简体中文**

状态：已接受
日期：2026-10-08

<a id="decision"></a>
## 决策

JavaScript 加载保留在 CLI 组合根。仅元数据的作用域发现通过 ACP 暴露已安装 Mod，不运行包代码。核心 `RuntimeExtension` 边界适配提示上下文，并在正常调度／审批后包装工具。会话拥有的 Node 进程执行 hook、命令和已注册工具，使临时状态在 ACP 轮次重建间保留。配置／入口修订后于下一提示重建扩展。

<a id="consequences"></a>
## 影响

普通无 Mod 启动路径不依赖 Node，也不启动 Node 进程。全局／项目策略与现有能力共用。命令可不经模型推理完成 Goal，并保留权威历史。工具回调权限重新检查，已批准参数不能经 next 改写。Mod 是可信 Node 代码，因此受限工作区／SSH 本地运行时会拒绝加载，而不承诺无法执行的隔离。需要独立 API 兼容说明，见 [Mods](../mods.zh-CN.md)，包括不支持的渲染／托管层及仅供观测的轮次完成事件。
