# Contribution guide

**English** | [简体中文](README.zh-CN.md)

Start at [CONTRIBUTING.md](../../CONTRIBUTING.md) for the entry point.
This directory covers collaboration from reporting a problem to maintainer merge.

- [Issues](issues.md): search, report, discuss designs, claim work and follow up.
- [Pull requests](pull-requests.md): fork, branch, develop, validate, commit,
  submit, address review and reach merge readiness.
- [Reviewing](reviewing.md): roles, criteria, disagreements and merge decisions.

## Technical references

Use [Development](../development.md) for prerequisites, Cargo commands, coding
conventions and extension walkthroughs. Read [Architecture](../architecture.md)
and the relevant topic from the [documentation map](../README.md) before coding.
Record significant architecture decisions through [ADRs](../adr/README.md).

## Workflow scope

Small corrections can go directly to a PR. New features and significant changes
to architecture or core subsystem contracts require an issue and explicit
maintainer design agreement before implementation; see [design discussion](issues.md#design-discussion).
No additional contributor ranks or blanket preapproval are required.

The independent [PR CI](../../.github/workflows/ci.yml) checks formatting,
workspace tests, Clippy and release compilation. Existing
[platform checks](../../.github/workflows/platform-check.yml) retain native
platform validation. [Release automation](../../.github/workflows/release.yml)
remains tag-driven and does not replace contribution checks.

## Organization reference

The separation of issue discussion, implementation and review was informed by
[Tokio's contribution guide](https://github.com/tokio-rs/tokio/tree/master/docs/contributing).
AX's policies are specific to this workspace and its runtime contracts;
Tokio's technical commands and governance roles do not apply to AX.
