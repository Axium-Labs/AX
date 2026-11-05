<a id="0003-versioned-portable-ax-data-packages"></a>
# 0003. 带版本的可移植 AX 数据包

[English](0003-portable-axpack.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

SQLite 行和机器相关项目路径不适合作为长期备份格式。会话事件另存于 JSONL 文件，而凭据、缓存和用户数据共用 AX 数据目录。

<a id="decision"></a>
## 决策

通过记忆仓储导出选定用户数据为带版本的 `.axpack` ZIP，包含清单、逐条目 SHA-256 校验及可移植 JSONL 记录。会话消息使用序号，而非 SQLite 行 ID。项目事实保留原作用域，导入时重新绑定目标项目 UUID。导入在写入前验证整个包，提供只读预演报告，合并时不替换已有会话 ID 或记忆键。CLI 是组合根，不新增代理工具或并行数据库路径。

<a id="consequences"></a>
## 影响

格式可独立于 SQLite 迁移演进，目标项目路径和标识可不同于来源机器。目前 AX 仅接受格式版本 1，拒绝未知版本。正常执行期间，项目与全局数据库的 SQLite 变更采用事务；但 SQLite WAL 无法保证跨附加数据库文件的崩溃原子性，会话 JSONL 安装也有小段崩溃窗口。限制详见备份指南。
