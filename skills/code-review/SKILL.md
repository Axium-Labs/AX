---
name: code-review
description: Review diffs, commits, pull requests or whole-project code for actionable correctness, security, performance and maintainability issues, following the requested scope.
compatibility: Requires git when reviewing repository changes.
allowed-tools: filesystem search shell
---
# Code review

Use the scope requested by the user. For a diff, commit or pull request review,
read the complete relevant diff and enough surrounding code to understand each
changed path. For a whole-project or existing-code audit, inspect the requested
modules and their interactions, including pre-existing issues. Read repository
instructions and check call sites/tests before reporting a defect. Continue
through the requested scope after finding an issue.

For a branch review, resolve the comparison ref and use the merge base so the
review covers the changes that would merge. For a commit or uncommitted work,
inspect that exact target. If the target is ambiguous, infer it from the current
task and repository state before asking.

For change reviews, default to discrete, actionable regressions introduced by
the change. If the user requests a broader audit or pre-existing issues, include
those findings and distinguish them from newly introduced regressions. For
whole-project audits, do not limit findings to a recent diff. Explain the failure
scenario or concrete impact and cite the smallest useful file and line. Avoid
speculation and cosmetic preferences; when the user requests style/convention
review, assess against the requested standards.

Present findings first, ordered by severity: P0 for a critical release blocker,
P1 for urgent defects, P2 for ordinary defects, and P3 for lower-impact defects.
If there are no findings, say so. Briefly mention the review scope and any
material verification gap. Treat a review request as read-only unless the user
also asks for fixes.

Adapted for AX from Codex's `review-agent` sample skill.
