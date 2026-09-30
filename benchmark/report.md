# AX vs Codex Coding Benchmark

Local SWE-bench dev subset (23 rows); user-approved, not standard Lite

Model: `gpt-6-luna`; reasoning: `low`; timeout: 240s.

计时只覆盖 agent 进程；仓库/环境准备与 acceptance 测试另计。null 表示无法观测，绝不填成 0。Codex exec JSON 不公开 model rounds；其 search/read 是命令分类的观测下界。客户端状态与模型缓存不共享；服务端隐式 prompt cache 不能由此 harness 保证关闭，需结合 token usage 中缓存计数解释。

**本轮含接口或执行失败，未形成有效的三题 Coding Benchmark。下表保留真实失败记录；不能据此比较解题速度、工具效率或正确率。403/401 发生在工具调用前时，零调用不代表工具策略更高效。**

## marshmallow-code__marshmallow-1343

| 指标 | AX | Codex |
|---|---:|---:|
| 总耗时 | 3.351 | 20.062 |
| 首工具耗时 | N/A | N/A |
| 首编辑耗时 | N/A | N/A |
| Model rounds | 1 | N/A |
| Tool calls | 0 | 0 |
| Search | 0 | 0 |
| Read | 0 | 0 |
| Patch failures | 0 | 0 |
| 失败调用 | 0 | 0 |
| 重复搜索 | 0 | 0 |
| 重复读取 | 0 | 0 |
| 测试通过 | False | False |
| Resolved | False | False |

ax: agent_error — [memory.retrieve] candidates=0 matched=0 injected=0 dropped_for_budget=0 tokens=0 budget=512
[openai/gpt-6-luna] thinking...
Evolution: model HTTP request failed with status 403: <!DOCTYPE html>
<!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US"> <![endif]-->
<!--[if IE 7]>    <html class="no-js ie7 oldie" lang="en-US"> <![endif]-->
<!--[if IE 8]>    <html class="no-js ie8 oldie" lang="en-US"> <![endif]-->
<!--[if gt IE 8]><!--> <html class="no-js" lang="en-US"> <!--<![endif]-->
<head>
<title>Attention Required! | Cloudflare</title>
<meta charset="UTF-8" />
<meta http-equiv="Content-

codex: agent_error — Access blocked by Cloudflare. This usually happens when connecting from a restricted region (status 403 Forbidden), url: https://new.sharedchat.cc/codex/responses, cf-ray: a43540540e63ebf3-SJC
## marshmallow-code__marshmallow-1359

| 指标 | AX | Codex |
|---|---:|---:|
| 总耗时 | 2.519 | 20.332 |
| 首工具耗时 | N/A | N/A |
| 首编辑耗时 | N/A | N/A |
| Model rounds | 1 | N/A |
| Tool calls | 0 | 0 |
| Search | 0 | 0 |
| Read | 0 | 0 |
| Patch failures | 0 | 0 |
| 失败调用 | 0 | 0 |
| 重复搜索 | 0 | 0 |
| 重复读取 | 0 | 0 |
| 测试通过 | False | False |
| Resolved | False | False |

ax: agent_error — [memory.retrieve] candidates=0 matched=0 injected=0 dropped_for_budget=0 tokens=0 budget=512
[openai/gpt-6-luna] thinking...
Evolution: model HTTP request failed with status 403: <!DOCTYPE html>
<!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US"> <![endif]-->
<!--[if IE 7]>    <html class="no-js ie7 oldie" lang="en-US"> <![endif]-->
<!--[if IE 8]>    <html class="no-js ie8 oldie" lang="en-US"> <![endif]-->
<!--[if gt IE 8]><!--> <html class="no-js" lang="en-US"> <!--<![endif]-->
<head>
<title>Attention Required! | Cloudflare</title>
<meta charset="UTF-8" />
<meta http-equiv="Content-

codex: agent_error — Access blocked by Cloudflare. This usually happens when connecting from a restricted region (status 403 Forbidden), url: https://new.sharedchat.cc/codex/responses, cf-ray: a4354199c8d1b873-SJC
## pydicom__pydicom-1256

| 指标 | AX | Codex |
|---|---:|---:|
| 总耗时 | 2.179 | 20.173 |
| 首工具耗时 | N/A | N/A |
| 首编辑耗时 | N/A | N/A |
| Model rounds | 1 | N/A |
| Tool calls | 0 | 0 |
| Search | 0 | 0 |
| Read | 0 | 0 |
| Patch failures | 0 | 0 |
| 失败调用 | 0 | 0 |
| 重复搜索 | 0 | 0 |
| 重复读取 | 0 | 0 |
| 测试通过 | False | False |
| Resolved | False | False |

ax: agent_error — [memory.retrieve] candidates=0 matched=0 injected=0 dropped_for_budget=0 tokens=0 budget=512
[openai/gpt-6-luna] thinking...
Evolution: model HTTP request failed with status 403: <!DOCTYPE html>
<!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US"> <![endif]-->
<!--[if IE 7]>    <html class="no-js ie7 oldie" lang="en-US"> <![endif]-->
<!--[if IE 8]>    <html class="no-js ie8 oldie" lang="en-US"> <![endif]-->
<!--[if gt IE 8]><!--> <html class="no-js" lang="en-US"> <!--<![endif]-->
<head>
<title>Attention Required! | Cloudflare</title>
<meta charset="UTF-8" />
<meta http-equiv="Content-

codex: agent_error — Access blocked by Cloudflare. This usually happens when connecting from a restricted region (status 403 Forbidden), url: https://new.sharedchat.cc/codex/responses, cf-ray: a435432cbaefc8bd-SJC

## Median (固定 3 题；未完成或未知值不补零)

| 指标 | AX | Codex |
|---|---:|---:|
| 总耗时 | N/A (0/3 observed) | N/A (0/3 observed) |
| 首工具耗时 | N/A (0/3 observed) | N/A (0/3 observed) |
| 首编辑耗时 | N/A (0/3 observed) | N/A (0/3 observed) |
| Model rounds | N/A (0/3 observed) | N/A (0/3 observed) |
| Tool calls | N/A (0/3 observed) | N/A (0/3 observed) |
| Search | N/A (0/3 observed) | N/A (0/3 observed) |
| Read | N/A (0/3 observed) | N/A (0/3 observed) |
| Patch failures | N/A (0/3 observed) | N/A (0/3 observed) |
| 失败调用 | N/A (0/3 observed) | N/A (0/3 observed) |
| 重复搜索 | N/A (0/3 observed) | N/A (0/3 observed) |
| 重复读取 | N/A (0/3 observed) | N/A (0/3 observed) |
| 测试通过 | N/A (0/3 observed) | N/A (0/3 observed) |
| Resolved | N/A (0/3 observed) | N/A (0/3 observed) |

## AX 耗时诊断

基于事件和原始输出判断；三题不足以证明因果。search/read 碎片化看连续单工具 round；并行看每个 model round 的 call 数及同时运行工具数；过大读取看 large_reads/raw_output_bytes；patch 冲突看 patch_failures；错误恢复看失败后的 call 序列；重验证看 shell 命令与测试日志。

- marshmallow-code__marshmallow-1343: rounds=1, calls=0, max_parallel=0, large_reads=0, raw bytes=0, patch failures=0, failed calls=0, repeated search/read=0/0. Evidence: `C:\Users\14181\Desktop\axlab\benchmark\runs\20261001-022122\ec9b1b72\marshmallow-code__marshmallow-1343\ax`.
- marshmallow-code__marshmallow-1359: rounds=1, calls=0, max_parallel=0, large_reads=0, raw bytes=0, patch failures=0, failed calls=0, repeated search/read=0/0. Evidence: `C:\Users\14181\Desktop\axlab\benchmark\runs\20261001-022122\ec9b1b72\marshmallow-code__marshmallow-1359\ax`.
- pydicom__pydicom-1256: rounds=1, calls=0, max_parallel=0, large_reads=0, raw bytes=0, patch failures=0, failed calls=0, repeated search/read=0/0. Evidence: `C:\Users\14181\Desktop\axlab\benchmark\runs\20261001-022122\ec9b1b72\pydicom__pydicom-1256\ax`.

Windows 本机验收使用固定 FAIL_TO_PASS + PASS_TO_PASS 和 JUnit；原数据 Linux eval_script 保留在数据集，不在 Windows 模拟执行。build 不适用于这三个纯 Python 任务，记 N/A；check 为修改文件的 py_compile。任何 baseline 环境失败会阻止 agent 运行，不能算作模型解题失败。
