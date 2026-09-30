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

## Discovery

AX indexes skills on first routing or `/skills`, reading only YAML frontmatter
from each `SKILL.md`. The search order is the selected project skills directory
(`--skills-dir` when set, otherwise `<project>/skills`), then
`~/.ax/skills`. The first valid package with a given name wins; paths within
each root are sorted. A standard `SKILL.md` wins over legacy files in the
same directory. Invalid packages and duplicates are reported individually in
stderr and `/skills`; they do not suppress valid skills.

Release archives carry the repository's bundled packages, and the installer
places them in `$AX_HOME/skills` (usually `~/.ax/skills`) so they apply to every
project. A package that already exists there is left untouched, so local edits
survive an upgrade; `ax --update` refreshes them from the new release the same
way. Skills are otherwise plain directories — copy one there (or
into `<project>/skills`) to install it by hand.

## Routing

Routing is language-independent lexical similarity — there is no trigger-word
list, no stopword list and no stemming:

1. Every text (each skill's `name`/`description` at index time, the task text
   once per turn) is NFKC-normalized, case-folded and reduced to word-like
   tokens plus Unicode character 2/3/4-grams.
2. Similarity is the weighted overlap coefficient over those four families,
   normalized to `0.0..=1.0`. Normalizing by the smaller side keeps a short
   Chinese or Japanese request comparable with a long English description,
   which a symmetric measure such as Sørensen–Dice cannot do.
3. A skill scores `0.4 × name similarity + 0.6 × description similarity`.
   Explicitly naming a skill always scores `1.0`. No language is detected or
   special-cased, so Chinese, Japanese, English and mixed input share one code
   path.
4. Confidence `>= 0.40` loads the skill body automatically; `>= 0.12` reports
   it as a ranked candidate; below that nothing is reported.

Only an explicit skill name or a high-confidence match triggers automatic body
loading. Otherwise AX puts a compact, bounded metadata catalog in model
context so the agent can choose by meaning and read the listed instruction
file through the ordinary permission-controlled filesystem tool. The catalog
is omitted when automatic routing has already selected a skill. Because
routing is lexical, write the skill description in the words — and the
language — your users will actually type: a description that shares no
vocabulary with a request can only reach the model through that catalog. An
optional `metadata.ax.required-tools` dependency is checked against registered
tools, but permission is still decided for each actual call. Fast-ranked skills
are loaded on activation and injected as tagged system context (up to three per
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

The repository's five bundled standard packages are `coding`, `code-review`,
`skill-creator`, `skill-installer`, and `web-research` (batched, source-backed
web research strategy for the `web` tool).

## Reference

| Concern | Code |
|---|---|
| Parsing, validation, discovery, routing | `crates/skill/src/lib.rs` |
| Unicode features and similarity (shared with memory retrieval) | `crates/lexical/src/lib.rs` |
| CLI routing and context injection | `crates/cli/src/main.rs` |
| Enable and disable | `crates/cli/src/skill_settings.rs` |
| `/skills` UI | `crates/cli/src/tui/commands/catalogs.rs` |

## Importing a local Skill

Use `ax skill import C:/downloads/my-skill` to install a validated package into
the current project's `skills/` directory, or add `--global` to use
`$AX_HOME/skills`. `--skills-dir` also applies to project imports. AX reuses
the standard package validator, copies resources, rejects symbolic links and
conflicting names, and does not execute package scripts during import.
Crew exposes the same command under Settings → AX capabilities.
