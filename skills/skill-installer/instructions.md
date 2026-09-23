# Install a skill into AX

Use the source named by the user, such as a local directory or repository path. If no source is given, find suitable local candidates and ask for a source only when it cannot be inferred. Inspect the source instructions, license, dependencies, scripts, and referenced files before installing.

AX indexes immediate child directories of its configured skills directory. Each installed skill needs a valid `skill.toml` and a nonempty `instructions.md`. Convert Codex `SKILL.md` frontmatter into AX metadata and adapt the body to AX's available tools and paths. A copied `SKILL.md` alone is not loadable by AX. Preserve useful supporting files only when the instructions actually use them. Do not copy scripts that depend on Codex-only APIs or hidden services without replacing those dependencies.

Do not overwrite an existing destination without inspecting and preserving user changes. Keep source and license attribution when adapting third-party material. For remote sources, fetch only the requested package; do not run downloaded scripts just to install it. Validate catalog indexing, instruction loading, and routing with a representative request. Report installed paths and any functionality that could not be carried over.

Adapted for AX from Codex's `skill-installer` sample skill.
