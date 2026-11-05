<a id="0031-tool-declared-verbatim-output"></a>
# 0031. 工具声明的逐字输出

[English](0031-tool-declared-verbatim-output.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

工具结果清理原为命令输出设计，会美化 JSON、移除警告行并提取看似诊断的文本。应用到文件读取后会改变源码空白、遗漏有效行，并把普通代码展示成错误。字符截断也使文件读取缺少可用续读坐标。运行时必须与具体工具无关，兼容旧快照，并从 `ContextBudget` 派生上下文限制。

<a id="decision"></a>
## 决策

工具在每次成功执行时，通过现有类型化输出边界声明源码语义：`ToolOutput::Verbatim { text, total_lines, start_line, truncated, line_truncated }`。`Tool::execute_output` 默认普通 `Text`，已有工具保留行为。调度器将声明保存到增量新增、带 serde 默认值的 `ToolResult` 字段，不按工具名分支，也不从文本推断源码语义。整文件及范围读取均使用该变体，保留相同 `N: ` 坐标前缀和原始行终止符。

Verbatim 信封跳过诊断提取、JSON 格式化和警告过滤。模型读取投影及确定性清理在 `ContextBudget` 配额内保留总行数和原起始坐标。清理／轮换只保留开头完整行，绝不设置 `line_truncated`；首行放不下时，正文为空且标记截断，续读仍指向该行。模型视图截断时提供最后展示行及下一 `start_line`。普通读取模型视图的首行本身超过配额时，在 UTF-8 字符边界内保留行首，设置 `line_truncated`，下一 `start_line` 指向后续行，文件末尾为 null；提示模型用 `search` 定位该行更深处内容。请求范围完整时即使文件后面还有行，也不报告续读。非 Verbatim JSON 视图仅压缩 `output` 字段，保留 `diagnostics` 和有效 JSON。文本推断诊断只在失败时产生，成功时显式 JSON 诊断仍为权威。大文件范围保留使用现有 `MAX_FILE_BYTES` 字节上限，UTF-8 安全的部分尾部由工具显式声明。精确行数通过规范化路径、mtime 和长度为键的惰性有界 LRU 缓存，使重复范围读取能提前停止。请求准备保留类型化普通读取投影，Session 轮换只保留完整行。复用规则见 [0032](0032-verified-patches-and-bounded-receipts.zh-CN.md)。原始历史仍为权威。

恢复 Session 时从已保存工具消息重建回执缓存，消息可能已经是压缩投影。因此同一调用的截断信封绝不替换完整回执；`tool_output` 对截断回执不再保留的范围明确拒绝，不返回空正文。

类型化数据消费者通过 `ToolResult::source_text()` 在独立副本上移除精确声明的坐标前缀序列，保留 JSON pointer、工作清单和执行观测，已保存及模型可见源码正文不变。

<a id="consequences"></a>
## 影响

移除坐标前缀后文件上下文仍适合打补丁。命令输出保留已有清理，旧快照保持普通文本语义。混合输出工具可按次选择变体，无需注册表级工具名例外。单行超过整份配额时仅展示行首并标记 `line_truncated`，有下一行则推进，行内内容用 `search` 定位。压缩在没有完整行能放下时，有意保留首行作为重试坐标。轮换每轮减半保留结果，使工作量呈对数界限，不逐删一行就重新序列化正文。语义压缩仍可摘要旧轮次，完整原始回执留在原始历史。有界输出缓存及补丁验证契约见 [0032](0032-verified-patches-and-bounded-receipts.zh-CN.md)。
