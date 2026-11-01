# Validation and source scope

Publication prepared on 2026-11-02 from
`54bcce9e17be15ed09d97e5dc1d496f89c4bf283` plus only the AX-Bench package,
selected archived results and benchmark documentation. No Runtime business
logic, existing test source, version, release configuration or root Cargo file
was changed. Unrelated local edits were excluded.

## Original measurement environment

- Release/offline/locked Phase 1 run: **50 configurations, 3 process rounds each,
  3000 recorded samples, zero benchmark failures**. Correctness assertions passed.
- Separate same-host `--version` comparison: five launch paths, three rounds,
  300 recorded launches, zero failures in the published Node 22 campaign.
- Original working-tree workspace tests: **596 passed, 0 failed, 3 ignored**.
  This working tree contained extra local tests and previously prepared test
  corrections. Those corrections are not part of this benchmark publication.
- Original reporting/monitoring checks: three private regression tests passed.
  Their source remains local; published code does not depend on it.

Raw results retain the original dirty-tree identity and hashes. They must not
be described as a clean-checkout result for this publication commit. Benchmark
code and data counts were verified against the archived source hashes and
original artifacts before staging. `SHA256.json` covers the measurement
artifacts; Git attributes preserve their original bytes and line endings.

## Isolated intended publication file set

The following checks used a separate Git worktree containing HEAD and only the
intended publication files, without copying private tests or unrelated patches.
Windows x64, Rust 1.92.0, Python 3.14.7; cached dependencies, Cargo offline/locked.

- `cargo build --release --locked --offline -p cli`: **PASS**.
- `cargo test --workspace --locked --offline --no-fail-fast`: **577 passed,
  3 failed, 3 ignored**. Test compilation completed; full tests did **not** pass.
- `cargo clippy --workspace --all-targets --locked --offline`: **PASS**, existing
  warnings retained under the repository's warning policy.
- `cargo fmt --all -- --check`: **PASS**.
- `cargo bench --manifest-path benchmarks/runtime/Cargo.toml --bench micro
  --no-run --locked --offline`: **PASS**, optimized benchmark executable built.
- Benchmark package all-target Clippy and format check: **PASS**.
- Clean optimized benchmark executable: all **48 function/runtime configurations**
  completed isolated correctness smoke checks (one warmup and two recorded
  samples per configuration). These smoke checks are not a new performance run.
- Clean Release CLI `--version` and `--help`: **PASS**. A one-off smoke helper
  initially used the local default GBK decoder for UTF-8 help text; rerunning the
  CLI check with explicit UTF-8 decoding passed. The published process monitor
  already explicitly decodes UTF-8.
- Reporting/monitor regression checks against the copied benchmark package:
  **3 passed**; same-baseline comparison produced 50 unchanged case summaries.
- Archive validation: **3000 Phase 1 rows + 300 peer-comparison rows**, zero
  benchmark worker failures, source/fixture hashes match; staged artifacts match
  original bytes. Credential-pattern scan and staged whitespace checks passed.

### Existing HEAD test failures

These three failures reproduce the already known uncommitted-test-correction
gap. The corresponding production/test files are unchanged by this publication:

1. `child_runtime::harness_workspace_tests::two_repositories_exact_revisions_partial_setup_failure_and_durable_patches`
2. `commands::prompt_execution_tests::cli_tui_acp_share_real_local_children_and_independent_child_budget`
3. `tests::failed_patch_recovers_with_local_read_patch_and_minimal_check`

The benchmark package and its optimized correctness checks pass independently.
The original 596-pass working-tree result is retained as historical evidence,
not substituted for the 577-pass/3-failure clean publication check. This upload
does not claim that all repository tests or GitHub CI pass. No failures were
disabled, hidden or fixed by unrelated changes in this commit.

## Excluded material and limits

Private regression sources, full local logs, temporary datasets/databases,
executables, credentials and unrelated pending edits are not uploaded. Only
selected owner-requested results are archived; ordinary runner outputs remain
ignored. Other platforms, full durable CLI resume, real model/API calls,
external tools, durable child provisioning, long-running stability and Agent
task capability remain outside this measured baseline.
