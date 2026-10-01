# ADR 0011: One registry for capability scopes

Status: Accepted

Skills previously chose duplicate names by directory order, MCP chose one
whole configuration file, and optional Subagent execution had only a global
switch. This made project behavior inconsistent across capability types.

Use the leaf `scoped` crate for `Scope`, `ScopePolicy`, generic
`ScopedRegistry<T>` and TOML policy persistence. The CLI composition root
adapts Skill packages, MCP server metadata and Agent manifests to that registry.
Project definitions shadow global names even when disabled. Project overrides
precede inherited-global masks. Execution consumes only enabled entries.

The same management operation implements list, enable, disable, add and remove
for all kinds. Only source loading and installation/removal differ by format.
Crew delegates to AX's ACP catalog and CLI management boundary, preserving one
source of truth. Runtime-core receives enabled Agent descriptors and reads their
instructions on invocation; it does not know about scope or terminal UI.

Project capability files are portable `.ax` config. They reuse AX's existing
project UUID and identity format, and carry the UUID on project mutation rather
than saving an absolute path as identity. Existing installation-owned runtime
storage remains unchanged. Metadata remains lazy and disabled definitions never
initialize transports, bodies or workers.

See [Scoped capabilities](../capabilities.md) for formats and management syntax.
