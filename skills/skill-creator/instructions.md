# Create an AX skill

Create or update a skill in the user-requested location. If no location is specified, use the project's `skills/<name>/` directory. Read existing skills and the AX skill catalog implementation before changing the package format.

An AX skill has a `skill.toml` manifest and an `instructions.md` body. The manifest declares `name`, `description`, `trigger_keywords`, and `required_tools`. The directory must be an immediate child of the configured skills directory. Use a short, distinct lowercase name; write a description that identifies when the skill applies; keep trigger phrases specific enough to avoid unrelated requests. Name only tools AX actually registers.

Keep the instructions small and useful. Preserve the user's scope and authorization. Include operational rules that change the agent's decisions; omit generic advice and unsupported commands. Add supporting files only when they are genuinely needed, and link to them from `instructions.md`. AX loads only `instructions.md` into the model context, so references are not read automatically.

Validate the TOML and instruction file, then check that the skill is indexed, routed by a representative request, and not routed by an unrelated request. Do not overwrite an existing skill without inspecting it. Report the created paths and what was verified.

Adapted for AX from Codex's `skill-creator` sample skill.
