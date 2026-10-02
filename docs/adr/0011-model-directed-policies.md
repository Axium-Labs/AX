# 0011: Model-directed semantics with runtime policy ceilings

Status: accepted. Supersedes automatic list decomposition and retry-once
portions of ADR 0008 and fixed routing/context/lifecycle decision formulas.

The existing main model chooses Skill invocation, task decomposition and
subagent creation. The low-frequency Evolution analyzer proposes semantic
creation/refinement/merge/promotion/retirement. Runtime retains deterministic
filters, evidence/provenance/ownership/validation gates, task state/dependencies,
permission matching, sandbox boundaries, budgets and error classification.

ChildPolicy defaults to isolation and permits explicit bounded inheritance;
no custom child profile can relax parent rules/capabilities or OS confinement.
Context consumers compete in one elastic input pool after hard output/schema
reserves, and compaction considers projected next-request growth. Providers
retry typed transient errors under server waits, backoff/jitter and dual budgets.

This reuses existing modules and durable formats. Legacy config/serialized state
remains readable with default mappings documented in the topic docs. Skill bodies
remain lazy; no vector DB, resident routing/planning model or startup model call
is introduced. Conservative failure is preferred when a host/transport cannot
actually enforce a requested policy.
