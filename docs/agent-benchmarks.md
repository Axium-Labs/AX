# Agent capability and advanced benchmarks

**English** | [简体中文](agent-benchmarks.zh-CN.md)

[AX-Bench Agent](../benchmarks/agent/README.md) implements production CLI capability
execution and Advanced controllers without changing AX/AXCrew runtime logic.
The standard suite has 60 deterministic custom tasks; the 24-scenario Advanced
catalog dispatches long-context/long-stage tasks, compression/rotation assertions,
crash/Session recovery, actual API/MCP transport faults, native constraint checks,
isolated security vectors and real Crew experiments. Each task repeats >=3 times.

Executors retain trajectories and failed evidence, use real configured Providers,
apply task/global budgets and produce JSON/CSV/Markdown reports with source,
model, environment and dataset identities. Missing environments are `not_run`;
executed tests lacking complete billing or official scores are explicitly
`unscorable`/`ungraded` and are excluded from graded success denominators.
No offline plan or controller test is an Agent capability result.

Separate executable adapters cover restricted Docker security tests; real local/
SSH worker lifecycle, fault/restore, DAG/artifact/consistency checks and read-only
resource collectors; and pinned SWE-bench/Terminal-Bench prepared environments
with official deterministic validators. Physical machine identities remain
operator-attested, not inferred from multiple logical Host IDs. The package
README and linked adapter documents specify exact scopes and prerequisites.

Native token/cost caps require complete Provider usage/billing and can be crossed
by an in-flight request. Unknown costs are not zero. Transport reconnection is
not proof of physical network recovery; declared attack vectors do not establish
universal security. Remote and container resource measurements retain explicit
sampling and attribution boundaries. No score is known until a real model run.
