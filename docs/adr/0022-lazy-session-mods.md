# 0022: Lazy session Mods

Status: Accepted
Date: 2026-10-08

## Decision

Keep JavaScript loading in the CLI composition root. Metadata-only scoped
discovery exposes installed Mods through ACP without running package code.
The core `RuntimeExtension` boundary adapts prompt context and wraps tools after
normal scheduling/approval. A session-owned Node process executes hooks, commands
and registered tools, preserving ephemeral state across ACP turn reconstructions.
Config/entry revisions rebuild the extension on the next prompt.

## Consequences

There is no Node dependency or process on the normal no-Mod startup path.
Global/project policy is shared with existing capabilities. Commands may complete
a goal without model inference and keep authoritative history. Tool callback
permissions are checked again; approved arguments cannot be rewritten via next.
Mods are trusted Node code, so confined workspace/SSH-local runtimes reject
loading instead of promising isolation they cannot enforce. This requires a
separate API compatibility document: [Mods](../mods.md), including unsupported
rendering/managed tiers and observation-only turn completion.
