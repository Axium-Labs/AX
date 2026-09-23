---
name: skill-installer
description: Install standard Agent Skills from a local directory or remote source. Use when a user asks to install, import, or add a skill package.
allowed-tools: filesystem search shell
---
# Install an Agent Skill into AX

Use the source named by the user, such as a local directory or repository path.
Inspect its instructions, license, dependencies, scripts, and referenced files.

If the source contains a standard `SKILL.md`, validate its directory name,
YAML frontmatter, required fields, and nonempty body, then copy the package
unchanged into the selected skills root. Preserve optional `scripts/`,
`references/`, and `assets/` files. Do not convert a valid Agent Skill into
AX's legacy format.

If the source is an old AX package with `skill.toml` and
`instructions.md`, migrate it to standard `SKILL.md`. Carry the old name
and description into YAML frontmatter, put the instructions in the Markdown
body, and retain a tool dependency only when needed under
`metadata.ax.required-tools`. Ensure the destination directory matches the
standard name rule. Retain useful supporting files and attribution.

Do not overwrite an existing destination without inspecting and preserving
user changes. For remote sources, fetch only the requested package; do not run
downloaded scripts just to install it. Validate the installed package and
confirm catalog discovery and body loading. Report installed paths and any
unmet runtime dependencies. `allowed-tools` does not bypass AX permissions.
