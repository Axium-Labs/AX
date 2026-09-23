---
name: skill-creator
description: Create or update a standard Agent Skill package. Use when a user asks to make, write, or revise a skill with SKILL.md instructions.
allowed-tools: filesystem search shell
---
# Create an Agent Skill

Create or update a skill in the user-requested location. If no location is
specified, use the project's `skills/<name>/` directory. Inspect any existing
package before changing it.

The package must contain `SKILL.md` with YAML frontmatter and Markdown
instructions. The required `name` is 1–64 lowercase ASCII letters, digits,
or single hyphens; it cannot start or end with a hyphen and must match the
parent directory. The required `description` is 1–1024 characters and states
both what the skill does and when to use it. Optional standard fields are
`license`, `compatibility`, `metadata` (string keys and values), and
`allowed-tools` (a space-separated string). Do not create `skill.toml` or
`instructions.md` for a new package.

Keep the main instructions focused. Add `scripts/`, `references/`, or
`assets/` only when useful, and link to them using paths relative to the skill
root. Those resources are read on demand. Put AX-specific data under standard
`metadata` with an `ax.` prefix only when necessary. `allowed-tools` never
grants AX tool permission.

Validate the directory and frontmatter, confirm AX discovers the skill, and
check a representative task against its name and description. Do not overwrite
an existing skill without inspecting it. Report the created paths and checks.
