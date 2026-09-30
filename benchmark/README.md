Benchmark 随 AX 仓库维护。`dataset_root` 默认为 AX 仓库的父目录，自动查找已有 parquet；可改为本机数据所在目录，不会下载数据。`recorded-config.json` 保留随附失败记录的原始配置。

# AX vs Codex Coding Benchmark

固定使用本地 `swe/dev-00000-of-00001.parquet` 中的三个任务。这是用户确认的 23 个 dev 任务子集，不冒称标准 SWE-bench Lite。runner 只读取本地数据，永不下载数据或自动换题。

```powershell
python -m pip install -r benchmark/requirements.txt
python benchmark/runner.py prepare
benchmark/benchmark.cmd run --agent ax
benchmark/benchmark.cmd run --agent codex
benchmark/benchmark.cmd run --agent both
benchmark/benchmark.cmd run --instance marshmallow-code__marshmallow-1343
python -m unittest discover -s benchmark -v
```

`prepare` 只下载代码仓库。把 benchmark 目录加入当前终端 PATH 后可直接使用 `benchmark run ...`。Linux 可运行 `python benchmark/runner.py`，需修改配置中的 binary/runtime 路径。任务 runtime 固定 Python 3.9.25，旧 marshmallow 在 Python 3.10+ 无法通过保留测试。可用 `uv python install 3.9 --install-dir benchmark/runtime` 准备；绝不修改任务代码来兼容新 Python。

`config.json` 固定 instance_id、commit、`gpt-6-luna`、reasoning `low`、依赖版本及 timeout。三题是 marshmallow-1343（24 个保留测试）、marshmallow-1359（76 个保留测试）和 pydicom-1256（22 个保留测试）。首次选择仅依据依赖和测试规模，后续不依据成绩换题。

每个 agent 使用 `git clone --no-hardlinks` 生成独立 detached snapshot、独立 venv、独立 home 和新进程。只复制认证材料；不复制 config、memory、会话、skills、MCP、hooks 或模型 cache。AX binary 在运行开始时复制到 runs 目录冻结。两边用户 task prompt 的 SHA256 相同。默认两边并行，三题按顺序运行。

设置 `OPENAI_API_KEY` 时，两边强制使用同一 `OPENAI_BASE_URL` 的 Responses API；Codex 的临时 custom provider 仅统一网络入口，不调整其推理策略。否则两边使用独立副本的 ChatGPT OAuth 凭据。不会切模型或改 effort。认证/网络失败保留进程时间，但不能当作 coding 表现。metadata 记录端点、binary/config/dataset SHA256 与平台，不记录密钥。

独立 baseline snapshot 应用官方 test_patch，必须确认 FAIL_TO_PASS 失败而 PASS_TO_PASS 全部通过，才启动 agent。Gold solution patch 从不应用或送给模型。Agent 结束后，其 diff 和新文件应用到第三份 evaluator snapshot，再恢复官方测试并应用 test_patch。正确性由 pytest JUnit、固定 FAIL_TO_PASS/PASS_TO_PASS 和修改 Python 文件的 py_compile 决定；缺失、失败或 skipped case 均不通过。pytest 运行 acceptance case 所在的完整测试文件，保持原数据 pytest parser 对含空格参数名的 canonical ID 规则；同一 canonical key 下必须全部通过。

输出 results.json、results.csv、report.md。原始事件、diff、JUnit、baseline 日志和逐次结果在 runs 下保留。顶层文件代表本次调用，单题调用不冒充三题 median。每个命令有 timeout，Windows 超时结束整个进程树。

指标限制：AX model_rounds 来自主循环 ModelStarted，后台 evolution 请求未公开该事件，wall time 包含其开销。Codex exec JSON 不公开 model_rounds，记 null。shell 内 search/read 分类是可观测下界，复杂脚本的多次操作不能完全拆解。首编辑为 250ms 文件变化采样；短暂修改又恢复可能遗漏。recovery_calls 指失败后至下一次成功 patch/shell 的调用数；single_search_read_rounds 指只含一个 search/read 的轮次，不断言其一定可并行。客户端缓存独立，服务端隐式 prompt cache 无法由此保证关闭，后续有效 coding run 需要核验供应商缓存设置。

当前网关被 Cloudflare 403 阻断，三题两边均未执行代码工具。现有报告是连接失败记录，不能得出性能排名。
