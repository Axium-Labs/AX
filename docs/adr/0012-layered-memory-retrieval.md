# 0012. Layered local memory retrieval

Status: accepted

## Context

A single weighted score mixed ownership, relevance and inferred importance.
Retrieval loaded every owned memory body and could fill the prompt with details
that were not needed. AX requires local SQLite, lazy loading and low latency.

## Decision

Filter current Session/Project and Global ownership before recall; retain
same-key scope overrides. Recall no more than 32 lexical/exact/tag/path matches,
then rerank with small usage, confidence and type-aware freshness bonuses.
Scope and provenance are excluded from ranking. Replace inferred importance
with explicit use counts, confidence, provenance, last use and lifecycle flags.
Use preferences, facts, decisions, tasks, references and experiences to express
content lifetime. Use wall-clock age; reading does not rejuvenate content.

Persist short summaries and existing Unicode lexical features in a lazy SQLite
index. Inject at most six summaries within ContextBudget and fetch full details
by key through the existing memory tool. No embeddings or additional model
calls. Upgrade schemas additively and retain old APIs and serialized records.

## Consequences

Ownership cannot boost unrelated matches. Decay reflects content lifetime, and
superseded/expired content remains manageable without entering automatic recall.
Summaries can be incomplete, so the model may request details through an ordinary
tool call. Legacy/imported entries pay feature derivation once on first retrieval;
subsequent turns read the index without loading detail bodies.
