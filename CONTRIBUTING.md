# Contributing to AX

AX welcomes bug reports, documentation improvements, tests, fixes and focused
features. No community rank is needed to contribute or review a change.
AX is a Rust agent runtime kernel; read its [architecture](docs/architecture.md)
before changing code.

## Choose a contribution

Search [issues](https://github.com/Axium-Labs/AX/issues), including closed reports,
and [open PRs](https://github.com/Axium-Labs/AX/pulls) first. Use the
[bug form](https://github.com/Axium-Labs/AX/issues/new?template=bug_report.yml) or
[feature form](https://github.com/Axium-Labs/AX/issues/new?template=feature_request.yml)
for a new report. Include a minimal reproduction or concrete use case, with
credentials and private data removed. See [issue handling](docs/contributing/issues.md).

Small, localized fixes, regression tests and documentation corrections may go
directly to a PR without prior approval or a mandatory issue. Comment on an
existing issue before taking it on to avoid duplicate work.

**Discuss significant work in an issue and obtain explicit maintainer agreement
on the design before implementation.** This includes new features, architecture
adjustments and substantial changes to Runtime, Agent Loop, Memory, Storage,
Permission, Sandbox, Provider or MCP behavior or contracts. A localized bug fix
in one of those modules does not automatically require design approval. When in
doubt, describe the scope in the issue. Design agreement does not guarantee merge.

## From an idea to a merged PR

1. Check issues and PRs, discuss or confirm the task as appropriate, then fork
   [AX](https://github.com/Axium-Labs/AX) and create a focused branch from upstream
   `main`.
2. Follow [Development](docs/development.md) for environment setup, builds,
   tests, code conventions and extension points. It is the source of technical
   standards; this guide defines the contribution process.
3. Implement one coherent change with appropriate tests. Bug fixes should
   include a regression test wherever feasible. Update topic documentation when
   behavior changes; use the existing [ADR mechanism](docs/adr/README.md) for
   lasting architecture decisions.
4. Run [local validation](docs/contributing/pull-requests.md#local-validation),
   inspect the exact files to be committed, then commit and push your branch.
   Keep private tests, logs, credentials and generated artifacts out of the PR.
5. Open a PR against `main` using the template. Explain the purpose, linked
   issue (or why none is needed), approach, architecture impact, actual test
   results, compatibility and risks. Draft PRs are welcome for early feedback.
6. Respond to review and push focused follow-up commits. CI runs on PR creation
   and updates. Resolve required changes and obtain passing checks on the final
   revision; maintainers decide whether and how to merge.

The [complete PR workflow](docs/contributing/pull-requests.md) includes fork/Git
commands, test isolation and review updates. If a check cannot run locally,
report the limitation and unverified behavior rather than marking it passed.

## How AX reviews contributions

Anyone may provide a constructive review. Contributors own their implementation,
verification and responses; reviewers assess correctness and explain requested
changes; maintainers settle disagreements and retain final authority to accept,
reject and merge PRs. Passing CI alone does not guarantee acceptance.
Reviews cover crate boundaries, runtime correctness, data safety, security,
performance, APIs, dependencies, tests and documentation. See the
[review policy](docs/contributing/reviewing.md) for criteria and the merge gate.

The [contribution navigation](docs/contributing/README.md) links each stage.
Contribution does not authorize a release; release automation remains separate.
