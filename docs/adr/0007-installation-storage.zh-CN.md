<a id="0007-installation-owned-persistent-state"></a>
# 0007. 安装目录拥有的持久状态

[English](0007-installation-storage.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

用户清理工作区会丢失工作区内 `.ax` 数据库和事件日志。AX 状态应随已安装可执行文件保存。

<a id="decision"></a>
## 决策

默认 AX 主目录为可执行文件旁的 `.ax`。项目存储在 `projects/` 下以规范化路径的 SHA-256 为键，同时保留 UUID 事实归属。读取现有工作区 ID，不创建新的工作区元数据。AX_HOME 和 --data-dir 仍是显式覆盖。迁移旧存储时使用 SQLite 快照，在复制 JSONL 时持有写锁，并保留原数据。

<a id="consequences"></a>
## 影响

更改 cwd 或删除工作区内容不会删除默认持久状态。安装目录必须可写。没有旧 ID 的不同路径被视为独立项目，可通过可移植导出／导入迁移数据。卸载并删除整个安装目录，也会删除其中状态。
