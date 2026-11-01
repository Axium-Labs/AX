# AX and installed peers: same-host --version comparison

same Windows host; --version only; 3 independent rounds; 5 warmup + 20 recorded fresh-process launches per tool per round; OS cache not flushed; shell shim excluded; native Codex and npm launcher separated; isolated homes; parent launch/wait included; Win32 whole-process CPU and working set; npm launcher memory excludes its native child and must not be compared

UTC: 2026-11-01T17:19:13.149374+00:00

- AX (ax 0.3.16): mean 12.427 ms; median 12.383 ms; P95 13.415 ms; min 11.305 ms; max 15.393 ms; peak working set 11.625 MiB; mean/AX 1.00; per-round means [12.840965, 12.48222, 11.957835]
- Codex native (codex-cli 0.153.4): mean 17.785 ms; median 17.655 ms; P95 19.271 ms; min 16.229 ms; max 20.652 ms; peak working set 13.6171875 MiB; mean/AX 1.43; per-round means [17.274865, 18.54158, 17.53913]
- OpenCode (opencode v2.0.26): mean 88.872 ms; median 88.949 ms; P95 91.787 ms; min 85.596 ms; max 95.828 ms; peak working set 86.953125 MiB; mean/AX 7.15; per-round means [87.942965, 89.673435, 88.999275]
- Pi (1.2.0): mean 202.591 ms; median 202.177 ms; P95 210.004 ms; min 195.994 ms; max 211.778 ms; peak working set 79.06640625 MiB; mean/AX 16.30; per-round means [202.973705, 202.56555, 202.235225]
- Codex npm launcher (codex-cli 0.153.4): mean 65.015 ms; median 64.664 ms; P95 69.496 ms; min 60.276 ms; max 75.098 ms; peak working set None MiB; mean/AX 5.23; per-round means [65.496315, 65.583825, 63.966105]

These numbers cover version-command process startup only. They do not rank agent loop efficiency, full TUI readiness, Memory recall, subagent lifecycle, coding quality or long-running footprint.

Failures: []

## Public reference measurements (checked 2026-11-02)

- Prime Intellect's October 9 suite reports cold input-ready latency: Prime Rust 55.8 ms, Codex 0.160.0 324.6 ms, Claude Code 2.1.289 348.4 ms, Pi 1.0.3 317.7 ms. These measure interactive readiness, unlike our version-command exit timer. Developer-authored benchmark; hardware/configuration and versions differ. Source: https://www.primeintellect.ai/blog/prime-agent-rust
- harness-perf-benchmark reports median-run process-tree peak memory for a scripted 100-tool-turn workload: Codex 0.155.1 163.7 MB, Pi 0.85.1 234.4 MB, Claude Code 2.1.277 351.0 MB; OpenCode 2.0.10 849.6 MB came from a separate probe batch. macOS Apple Silicon measurements; not comparable to AX's microbenchmark-worker peak. Source: https://github.com/KonghaYao/harness-perf-benchmark
- No aligned published measurement was established for AX's exact Memory recall, context selection or mock-host subagent lifecycle. They remain internal baselines, without a cross-harness rank.

## Local interpretation

AX's version-command startup is competitive on this host: its mean is about 30% lower than native Codex, 86% lower than OpenCode and 94% lower than Pi. These percentages apply only to the recorded installed versions and launch paths. They do not establish full Runtime or task-completion superiority.

The initial inherited PATH selected Node 20.16.0, on which Pi could not import enableCompileCache. That exploratory campaign remains separately archived. Final measurements explicitly use the existing Node 22.20.0 installation for Pi and the Codex npm launcher; no installation, update or global setting change was performed. PowerShell/.cmd shims are excluded for every CLI. Native executables are used where available.
