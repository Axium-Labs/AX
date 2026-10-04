# 2026-10-04 23:10

Task: Cross-reference AX and AX Crew in the READMEs, then push

Changed:
- README.md (hero + Usage-adjacent intro): added a reciprocal mention of
  AX Crew with link to https://github.com/Axium-Labs/AXCrew and
  docs/acp-crew.md

Summary:
- AX README 现与 AXCrew README 互相引用，形成「agent runtime ↔ control plane」
  联动定位；AXCrew README 同步增加对 AX 的引用（hero 段落与 AX vs AX Crew 结尾）。

Tests:
- 无代码变更，未运行测试；仅文档编辑

Commit / push (用户要求提交推送):
- be8081a "docs: cross-reference AX Crew in the README"，仅含 README.md
- 已推送 origin main（https://github.com/Axium-Labs/AX），与远端 0 分叉
- 工作区中大量 coding-harness 相关未提交源码改动（crates/*、test/harness/、
  docs/coding-harness*.md 等）不属于本次任务，未暂存未推送，保持原样

Next:
- 该 feature 工作由对应任务自行提交

第二轮（中文版 README）:
- 新增 README.zh-CN.md（ax README 完整中文版），README.md 顶部加语言切换
- 修复原 README 中 benchmark/README.md 死链（benchmark/ 目录不存在，
  改为纯文本路径引用，中英一致）
- 提交为 b92d312 "docs: add Chinese README"，已推送 origin main（be8081a..b92d312）
