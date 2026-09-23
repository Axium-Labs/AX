# Skills

AX uses the [Agent Skills specification](https://agentskills.io/specification).
Skills provide task instructions without adding their bodies to startup context.

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

## Discovery and routing

AX indexes skills on first routing or `/skills`, reading only YAML frontmatter
from each `SKILL.md`. The search order is the selected project skills directory
(`--skills-dir` when set, otherwise `<project>/skills`), then
`~/.ax/skills`. The first valid package with a given name wins; paths within
each root are sorted. A standard `SKILL.md` wins over legacy files in the
same directory. Invalid packages and duplicates are reported individually in
stderr and `/skills`; they do not suppress valid skills.

Routing ranks candidates using the task text against the skill's name and
description, including the use cases written in that description. AX also
places a bounded metadata-only catalog in model context, so the agent can
select a skill by meaning when term overlap misses it and read its instruction
file through the ordinary permission-controlled filesystem tool. It does not
use a private trigger list for new skills. An optional
`metadata.ax.required-tools` dependency is checked against registered tools,
but permission is still decided for each actual call. Fast-ranked skills are
loaded on activation and injected as tagged system context (up to three per
turn); the model can read another listed skill on demand through the filesystem
tool. The catalog and injected instructions share the `ContextBudget` skill
reserve. Injected instructions include the skill root so relative resource
paths are available through ordinary
tools. `scripts/`, `references/`, and `assets/` are never preloaded.

## Legacy compatibility

Existing `skill.toml` plus `instructions.md` packages continue to load when
there is no `SKILL.md` in the same directory. Legacy tool dependencies are
retained. Legacy `trigger_keywords` are ignored for routing. New packages
should use `SKILL.md`; the bundled `skill-creator` and `skill-installer`
create and install that format.

## Enable and disable

`/skills` lists indexed packages and lets the user toggle them. Choices are
stored in `<data-dir>/disabled-skills.json`. Disabling removes that skill's
tagged instructions from effective context while preserving stored history.
Re-enabling permits routing on a later matching turn.

## Bundled skills

The repository's four bundled standard packages are `coding`,
`code-review`, `skill-creator`, and `skill-installer`.

## Reference

| Concern | Code |
|---|---|
| Parsing, validation, discovery, routing | `crates/skill/src/lib.rs` |
| CLI routing and context injection | `crates/cli/src/main.rs` |
| Enable and disable | `crates/cli/src/skill_settings.rs` |
| `/skills` UI | `crates/cli/src/tui/commands/catalogs.rs` |
