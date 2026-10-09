# Scoped capabilities

Skills, MCP servers, named Agents and Mods share `scoped::ScopedRegistry<T>`, the
same configuration parser and the same management operations. The CLI supplies
only their metadata adapters; Crew uses AX's ACP interface instead of merging
configuration itself.

## Locations

```text
$AX_HOME/
  skills/<name>/SKILL.md
  agents/<name>.toml
  agents/<name>.md
  mods/<name>/mod.json
  mcp.toml
  config.toml
<project>/.ax/
  skills/<name>/SKILL.md
  agents/<name>.toml
  agents/<name>.md
  mods/<name>/mod.json
  mcp.toml
  config.toml
  project.json
```

AX retains its existing installation-owned AX home (and `AX_HOME` override).
Set `AX_HOME=~/.ax` to use the conventional home layout. Capability config is
independent of the session database's `--data-dir`. Legacy `<project>/skills`
and explicit `--skills-dir` remain readable, after canonical `.ax/skills`.
`--mcp-config` replaces the project MCP source, while global servers still merge.
If canonical project MCP config is absent, legacy `<data-dir>/mcp.toml` remains
readable for existing installations.

The existing project UUID identifies the project. On a project mutation, AX
places that same UUID in `.ax/project.json`, using the existing identity format,
so moving the project preserves identity and local configuration. No absolute
workspace path is saved in scope policy or Agent manifests.

## Resolution

Resolution always loads global definitions, then project definitions, applies
project overrides, applies `disabled_global`, and builds the effective registry.
Matching names/IDs replace entire definitions. A disabled project definition
still shadows the global definition; disabling does not fall back to global.
Only enabled entries belong to the effective execution registry.

Each kind uses the same policy format in `config.toml`:

```toml
[skills]
disabled_global = ["reviewer"]
[skills.overrides]
rust-dev = false

[mcp]
disabled_global = ["browser"]
[mcp.overrides]
issues = true

[agents]
disabled_global = ["reviewer"]

[mods]
disabled_global = ["notes"]
[mods.overrides]
metrics = false
```

An optional top-level `disabled_global = ["name"]` masks that name in all
four kinds. Prefer kind-specific masks when names overlap. Enable removes
the matching mask. Project enable/disable of inherited entries never rewrites
global config. A project definition is unaffected by the global-name mask.
`overrides` persists explicit booleans separately from definitions, preserving
unrelated execution/provider config. Legacy `disabled-skills.json` is read as a
fallback; new explicit policy takes priority.

## Management

`/skills`, `/mcp`, `/agents` and `/mods` display Name, Scope and Status, including
`[global]`, `[project]`, `enabled`, `disabled`, and `disabled here`.
Space toggles in the selected configuration scope. `/settings` offers
**Global configuration** and **Current project configuration**, then each kind.
The current project view includes inherited globals; global configuration
shows global definitions even if the current project shadows them.

All four slash commands accept the same syntax:

```text
/skills list
/skills list global
/mcp disable browser project
/agents enable reviewer global
/agents add reviewer project C:\templates\reviewer.toml
/skills remove rust-dev project
```

The common CLI is suitable for scripts and Crew:

```bash
ax capabilities skills list --scope project
ax capabilities mcp disable browser --scope project
ax capabilities agents add reviewer --scope global --source /templates/reviewer.toml
ax capabilities skills remove rust-dev --scope project
```

`add` requires the requested name to match the source definition, and never
overwrites an existing definition in the destination. Skill sources are package
directories; MCP sources are TOML files containing `[servers.<name>]`; Agent
sources are TOML manifests with a relative instruction file. Existing Skill/MCP
`import` commands also install to these canonical directories.
Removing an inherited global entry from Project creates a local mask. Removing
a project definition reveals the global definition if one exists.

ACP provides `_ax/skills`, `_ax/mcp`, `_ax/agents`, `_ax/mods` and the common
`_ax/scopedCapabilities` method. The common method accepts `kind`, `scope`,
`action`, `name` and optional `source`. Mutations require an explicit scope;
listing without scope returns the effective view including disabled metadata.
Rows include `name`, `scope`, `status`, `enabled`, `description`, `source` and
the adapter's metadata. Responses expose the existing `project_id` UUID.
Crew settings use these rows and call AX for every mutation.

Mod sources are directories with `mod.json` and a relative JavaScript entry.
Catalogs only read metadata; enabled Mods execute lazily inside a session-owned
Node host. See [Mods](mods.md) for packaging, hooks, commands, persistent state,
Node requirements and API compatibility limits.

## Named Agents and lazy loading

```toml
# agents/reviewer.toml
name = "reviewer"
description = "Review code for regressions"
enabled = true
instructions = "reviewer.md"
tools = ["filesystem", "find_files", "search"]
```

The optional `tools` list narrows the parent tools; it cannot grant permissions.
The model calls `subagent` with `agent = "reviewer"`, `task` and optional
explicit `context`. Named Agent instructions are read only on this invocation.
Disabled names are rejected and excluded from the tool schema. Unnamed
delegation remains always available.

Global execution defaults still live in the existing `AxConfig` JSON file.
Project `[subagent]` settings in `.ax/config.toml` partially override those
defaults, for example `max_concurrent = 2` for this project alone. When the
depth budget is zero, no named Agent registry is initialized for a model turn.
AXCrew Settings → Plugins → Agents reads and writes these limits through
`ax settings --scope global|project`. Depth 0 disables delegation; 1 permits
direct children; greater values permit descendants. `--reset` restores global
defaults or removes project overrides. Descendants retain the parent's enabled
named-Agent catalog, load instructions only on invocation, and cannot widen the
parent's tool or permission scope. See [delegation limits](tools.md#subagents).

Skill frontmatter and MCP/Agent definitions are metadata only. Disabled Skills
never load bodies or enter routing/catalog context; disabled MCP servers never
enter the runtime manager or start transports. Agent instructions never load
during list/enable/disable. Registries are cached after first use and invalidated
on management/evolution changes. No provider call or connection is needed to
manage capabilities, and raw session history is preserved.
