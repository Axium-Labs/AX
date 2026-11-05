<a id="adr-0011-one-registry-for-capability-scopes"></a>
# ADR 0011：统一能力作用域注册表

[English](0011-scoped-capabilities.md) | **简体中文**

状态：已接受

此前 Skills 按目录顺序选取同名项，MCP 选择整份配置文件，可选 Subagent 执行只有全局开关，导致不同能力类型的项目行为不一致。

使用叶子 `scoped` crate 提供 `Scope`、`ScopePolicy`、泛型 `ScopedRegistry<T>` 和 TOML 策略持久化。CLI 组合根将 Skill 包、MCP 服务元数据和 Agent 清单适配到注册表。项目定义即使禁用也遮蔽全局同名项；项目覆盖优先于继承全局项的屏蔽。执行仅使用已启用条目。

各类型共用列表、启用、禁用、添加、移除管理操作；仅来源加载及安装／移除随格式不同。Crew 委托 AX ACP 目录与 CLI 管理边界，保持单一权威来源。Runtime-core 接收已启用 Agent 描述，调用时读取指令，不感知作用域或终端 UI。

项目能力文件是可移植 `.ax` 配置，复用 AX 已有项目 UUID 与标识格式，项目变更携带 UUID，不以绝对路径作为标识。安装目录拥有的现有运行时存储不变。元数据保持惰性，禁用定义绝不初始化传输、正文或工作进程。

格式和管理语法见[作用域能力](../capabilities.zh-CN.md)。
