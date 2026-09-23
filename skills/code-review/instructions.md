# Code review

Review the change requested by the user. Read any repository instructions, the complete relevant diff, and enough surrounding code to understand each changed path. Check call sites and tests before reporting a defect. Continue through the whole diff after finding an issue.

For a branch review, resolve the comparison ref and use the merge base so the review covers the changes that would merge. For a commit or uncommitted work, inspect that exact target. If the target is ambiguous, infer it from the current task and repository state before asking.

Report only discrete, actionable regressions introduced by the reviewed change that affect correctness, security, performance, or maintainability. Explain the scenario and cite the smallest useful file and line. Do not report speculation, pre-existing issues, or cosmetic style differences.

Present findings first, ordered by severity: P0 for a critical release blocker, P1 for urgent defects, P2 for ordinary defects, and P3 for lower-impact defects. If there are no findings, say so. Briefly mention the review scope and any material verification gap. Treat a review request as read-only unless the user also asks for fixes.

Adapted for AX from Codex's `review-agent` sample skill.
