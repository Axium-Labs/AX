# Skills

Skills give AX task-specific instructions that are loaded only when relevant.
This document covers the package format, routing, and the `/skills` command.

## Package format

A skill is a directory with two files:

```text
skills/<skill-name>/
├── skill.toml          # metadata
└── instructions.md     # loaded only when a route hits
```

`skill.toml` fields:

| Field | Meaning |
|---|---|
| `name` | Unique skill identifier |
| `description` | What the skill does; used for routing |
| `trigger_keywords` | Keywords that suggest relevance |
| `required_tools` | Tool names that must be available before the skill can run |

The directory-package format can naturally be downloaded and installed by a
future marketplace.

## Indexing & routing

- On first use, AX indexes only the `skill.toml` **metadata** — instructions
  are never loaded just to browse.
- `route_candidates` returns **every** candidate whose required tools are
  available, not just the top-1 keyword hit; the caller decides.
- `instructions.md` is read only after a route hit and a dependency check.
- Matched instructions are injected into the model context (up to 3 per turn,
  bounded by the skill token reserve of `ContextBudget` — see
  [context.md](context.md)).

## Enable / disable

- **`/skills`** — browse, search, and enable or disable installed skills.
  - Type to search names, descriptions or status.
  - Enter shows the indexed source path, description, triggers, required tools
    and missing dependencies.
  - Space toggles the selected skill; Esc returns.
  - Enabled, disabled and unavailable (missing tools) are distinct states.
- Choices are persisted in `<data-dir>/disabled-skills.json` **before**
  runtime state changes.
- Disabling removes the skill's tagged instructions from effective context and
  prevents future routing; session restore filters disabled-skill
  instructions. Original stored messages remain intact; re-enabling permits
  routing on a later matching turn. Information already incorporated into
  conversation summaries cannot be selectively removed.

## Bundled skills

| Skill | Purpose | Required tools |
|---|---|---|
| `code-review` | Review a code diff for actionable regressions without editing files | `filesystem`, `shell`, `search` |
| `coding` | Implement, debug, test and review software changes | `filesystem`, `shell` |
| `skill-creator` | Create or update an AX skill package with focused instructions and valid metadata | `filesystem`, `shell` |
| `skill-installer` | Import a local or remote skill into the skills directory and adapt it to AX's package format | `filesystem`, `shell` |

## Reference

| Concern | Code |
|---|---|
| Metadata model, indexing, routing | `crates/skill/src/lib.rs` |
| Skill routing in the CLI | `crates/cli/src/main.rs` (`ReplState::route_skills`, `skills`) |
| Persisted enable/disable | `crates/cli/src/skill_settings.rs` |
| `/skills` UI | `crates/cli/src/tui/commands/catalogs.rs` |
