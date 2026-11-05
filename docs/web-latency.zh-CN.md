<a id="web-scheduling-latency-benchmark"></a>
# Web 调度延迟基准

[English](web-latency.md) | **简体中文**

以下为 2026-09-30 的历史测量。实时观测所用抓取引擎已移除，数据不代表当前 Native/Remote 服务。参见[当前审计](web-search-audit.zh-CN.md)。

测量于 2026-09-30 的本地 Windows 开发环境。

```powershell
cargo run -p tool --example web_latency
cargo run -p tool --example web_latency -- --live
```

可复现基准对比旧并发 `join_all` 全等待策略与新完成驱动调度器。两条路径使用相同三条查询／URL、共享客户端，目标为一条有效结果。一请求耗时 20 ms，另两条慢请求耗时 300 ms。Fetch 使用回环 HTTP 服务，搜索使用异步提供商 fixture。预热一轮、记录 30 轮，P50/P95 使用最近秩百分位数。开发构建配置和 Windows 计时器粒度包含在测量值中。[原始基准输出](web-latency.json) 与报告保存在同目录。

| 操作 | 改前 P50 | 改前 P95 | 改后 P50 | 改后 P95 |
|---|---:|---:|---:|---:|
| 搜索 | 309.15 ms | 311.70 ms | 31.08 ms | 32.18 ms |
| 抓取 | 309.97 ms | 312.85 ms | 31.21 ms | 31.86 ms |

对这个“已有足够结果”的负载，耗时约降低 90%。它测量受控慢请求下的调度与取消，不承诺公开搜索引擎延迟。抓取全部指定页面仍需等待每页；只需部分页面时使用 `target_pages`。连接池另以连续两次内置搜索复用同一 Keep-Alive 连接测试。DNS 缓存测试验证合并及复用；重试测试验证 401/403/404 只尝试一次，429/5xx 最多两次。

主动选择的实时 smoke test 明确不使用 Brave 密钥，检索公开 Rust 查询，取得三条 DuckDuckGo HTML 结果，不抓取目标正文。单次观测总耗时 1758 ms，响应头到达 1751 ms，与基准百分位数据分开。

`/status` 展示实测 DNS、连接器、响应头／TTFB 及总耗时。连接器测量按情况包含 DNS／代理／TLS；响应头到达时间是 reqwest 可提供的 TTFB 近似。阶段重叠，不可相加。缓存命中及池化连接可能完全避免 DNS 或新建连接，不为这些情况虚构耗时。
