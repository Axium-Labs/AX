# Runtime performance measurement

AX-Bench Phase 1 is independent benchmark infrastructure under
[`benchmarks/runtime/`](../benchmarks/runtime/README.md). It links public
production APIs without changing runtime behavior or adding production targets.
It measures offline Release CLI startup, local session/context/memory operations,
tool registry/scheduling, child lifecycle and bounded supervisor concurrency.

Model, external tool and child-host persistence boundaries are deterministic
mocks, labeled separately from local production-only paths. Combined mock-kernel
latency is not end-to-end agent performance. Context measurements do not claim
model-driven compression performance; local resume components do not claim the
entire CLI restoration flow. No capability scoring or CI performance gate exists.

The runner uses isolated fixtures and at least three independent process rounds,
and emits raw JSON, CSV and Markdown with environment, source identity, timing
statistics and whole-worker CPU/working-set telemetry. OS cache state remains
uncontrolled. See the benchmark README for exact units, timed boundaries,
offline prerequisites, resource limitations and cross-version comparisons.
