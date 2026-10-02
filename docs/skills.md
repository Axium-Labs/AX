# Skills

AX uses the [Agent Skills specification](https://agentskills.io/specification).
Skills provide task instructions without adding their bodies to startup context.
Skill, MCP and Agent scope management share the [same registry](capabilities.md).

## Package format

```text
skills/<skill-name>/
├── SKILL.md            # required YAML frontmatter and Markdown body
├── scripts/            # optional executable resources
├── references/         # optional documentation
└── assets/             # optional static resources
```

The required `name` is 1–64 lowercase ASCII letters, digits, or single
hyphens. It cannot begin or end with a hyphen, contain two adjacent hyphens,
or differ from its parent directory name. The required `description` is
1–1024 characters and should explain what the skill does and when to use it.

AX retains the optional standard fields `license`, `compatibility`
(1–500 characters), `metadata` (string-to-string map), and `allowed-tools`
(space-separated string). Valid unknown frontmatter extension fields are
preserved. AX-specific data belongs under `metadata`, for example
`ax.required-tools: "filesystem search shell"` to prevent a skill from
activating when a tool is absent. Use this only for true dependencies.
`allowed-tools` is a declaration for the agent; it never grants permission.
Every tool call still goes through the registry and AX's permission check.

Example:

```markdown
---
name: code-review
description: Review changes for regressions. Use when reviewing a diff or commit.
license: Apache-2.0
compatibility: Requires git.
metadata:
  author: axium-labs
  version: "1.0"
allowed-tools: filesystem search shell
---
# Review

Inspect the full diff and report actionable regressions.
```

## Discovery

AX indexes skills on first routing or `/skills`, reading only YAML frontmatter
from each `SKILL.md`. The shared scope registry loads global `$AX_HOME/skills`, then project
`<project>/.ax/skills`, with legacy `<project>/skills` / `--skills-dir` sources
still readable. Project names override global names; canonical project packages
precede legacy packages. Paths within
each root are sorted. A standard `SKILL.md` wins over legacy files in the
same directory. Invalid packages and duplicates are reported individually in
stderr and `/skills`; they do not suppress valid skills.

Release archives carry the repository's bundled packages, and the installer
places them in `$AX_HOME/skills` (usually `<install-dir>/.ax/skills`) so they apply to every
project. A package that already exists there is left untouched, so local edits
survive an upgrade; `ax --update` refreshes them from the new release the same
way. Skills are otherwise plain directories — copy one there (or
into `<project>/.ax/skills`) to install it by hand.

## Routing

Skill activation is model-directed and policy-constrained. AX indexes compact
metadata and precomputes Unicode lexical features once. `route_candidates()`
ranks eligible names/descriptions using their strongest similarity signal;
there are no activation weights or confidence thresholds. Every eligible skill
can reach the bounded compact catalog, including lexically weak matches.
The main model decides relevance and explicitly calls `invoke_skill(name)`.
No planner call is added. The instruction body is read and validated only
inside that tool call, and returned with its skill root. Resources remain lazy.

Enabled state, Global/Project override/mask resolution, package paths,
dependencies, and `allow_implicit_invocation: false` are deterministic admission
filters. A manual-only skill is eligible only when the current user input
explicitly names it. Selection cannot expand this eligible set. `allowed-tools`
is metadata and never grants runtime permissions. The legacy
`auto_route_candidates()` API now returns explicit-name candidates only and
is not used to load bodies in the CLI. `route()` returns a ranking hint.

Catalogs, loaded instructions, memory, summaries, history and tool results
compete in the same elastic context pool. A large package does not reserve a
fixed share of the window. Actual successful invocations, rather than routing
predictions, feed Evolution usage telemetry.

## Legacy compatibility

Existing `skill.toml` plus `instructions.md` packages continue to load when
there is no `SKILL.md` in the same directory. Legacy tool dependencies are
retained. Legacy `trigger_keywords` are ignored for routing. New packages
should use `SKILL.md`; the bundled `skill-creator` and `skill-installer`
create and install that format.

## Enable and disable

`/skills` lists indexed packages and lets the user toggle them. Choices are
stored in the selected scope's `config.toml`, through the same policy used
for MCP and Agents. Legacy `disabled-skills.json` is read as a fallback.
Project disabling of inherited global packages stores a local mask.
Disabling removes that skill's
tagged instructions from effective context while preserving stored history.
Re-enabling permits routing on a later matching turn.

## Bundled skills

The repository's five bundled standard packages are `coding`, `code-review`,
`skill-creator`, `skill-installer`, and `web-research` (batched, source-backed
web research strategy for the `web` tool).

## Self-evolving Skills

The CLI also discovers trial/active packages in
`<data-dir>/evolution/<project-id>/live`,
after project and global Skills. Candidate and archived packages do not route.
Evolution reuses the standard skill-creator instructions and Skill validator,
with separate ownership metadata and automatic mutation restricted to evolved
packages. See [evolution.md](evolution.md) for scheduling, lifecycle and controls.

## Reference

| Concern | Code |
|---|---|
| Parsing, validation, discovery, routing | `crates/skill/src/lib.rs` |
| Unicode features and similarity (shared with memory retrieval) | `crates/lexical/src/lib.rs` |
| CLI routing and context injection | `crates/cli/src/repl/state.rs` (`prepare_skill_context`, `ranked_skill_catalog_context`) |
| Enable and disable | `crates/cli/src/skill_settings.rs` |
| `/skills` UI | `crates/cli/src/tui/commands/catalogs.rs` |

## Importing a local Skill

Use `ax skill import C:/downloads/my-skill` to install a validated package into
the current project's `.ax/skills/` directory, or add `--global` to use
`$AX_HOME/skills`. `--skills-dir` remains a discovery source. AX reuses
the standard package validator, copies resources, rejects symbolic links and
conflicting names, and does not execute package scripts during import.
Crew exposes the same command under Settings → AX capabilities.
