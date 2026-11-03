## Purpose

<!-- What problem does this solve? Describe the resulting behavior. -->

## Related issue and design agreement

<!-- Fixes #... or Refs #...; for small changes without an issue, explain why.
For new features or significant subsystem/architecture changes, link the issue
and explicit maintainer design agreement obtained before implementation. -->

## Implementation

<!-- Explain the approach and relevant alternatives. -->

## Architecture impact

<!-- Affected crates/contracts and boundaries; link ADRs where needed.
Use N/A with a reason when appropriate. -->

## Validation

<!-- OS/toolchain, exact commands and results (pass/fail/ignored/not run).
Explain regression coverage and platform/integration limitations. Distinguish
public workspace tests, private local scenarios and compilation-only checks.
Confirm validation of the intended public file set without private resources. -->

## Compatibility and risks

<!-- API/CLI/config/protocol/storage compatibility, migration/recovery,
security/sandbox, performance/resources and remaining risks; N/A with a reason. -->

## Checklist

- [ ] Scope is focused; required design discussion and maintainer agreement are linked.
- [ ] Code changes have appropriate tests; bug regression coverage or its limitation is explained.
- [ ] Development checks were run and actual results/limitations are recorded above.
- [ ] Behavior changes update topic docs and significant decisions follow the ADR process.
- [ ] Dependency/lockfile changes are intentional and explained.
- [ ] The public file set builds without private tests or untracked resources.
- [ ] No credentials, private data, local logs, private test files or generated artifacts are included.
