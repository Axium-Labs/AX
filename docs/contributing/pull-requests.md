# Pull request workflow

[Contribution entry point](../../CONTRIBUTING.md) · [Navigation](README.md)

## Confirm scope

Search issues and PRs. Follow the [issue policy](issues.md) to discuss or claim
work; obtain explicit maintainer design agreement before implementing significant
changes. Small, localized fixes can proceed without a separate issue.
Keep each PR focused on one reviewable outcome; split unrelated cleanup.

## Fork and branch

Fork [AX](https://github.com/Axium-Labs/AX) on GitHub. Replace `YOUR-USERNAME`
with your account; run these commands from a directory where you keep source:

```bash
git clone https://github.com/YOUR-USERNAME/AX.git
cd AX
git remote add upstream https://github.com/Axium-Labs/AX.git
git fetch upstream
git switch -c fix/short-description upstream/main
```

For an existing clone, fetch upstream and create a new branch from `upstream/main`
with a clean working tree. Keep unrelated local changes on their own branch.

## Develop against AX's contracts

Follow [Development](../development.md) for environment setup, build/test commands,
lints and implementation conventions. Use [Architecture](../architecture.md)
and the relevant [topic document](../README.md) for crate responsibilities and
runtime constraints; this guide does not redefine those technical standards.

All code changes need appropriate tests or verification of existing coverage.
Add regression tests for bugs wherever feasible; explain if a reproducer is not
automatable. Include success, error and cancellation paths where applicable.
Update behavior, interface and configuration documentation alongside code. For
lasting architecture decisions, follow the existing [ADR process](../adr/README.md),
link the design issue and let maintainers confirm acceptance.

### Test isolation

Follow [Development's build and test contract](../development.md#build--test).
Use existing tracked Cargo tests for public, reproducible coverage. Keep new
private regression/integration harnesses in local `test/`; do not upload private
test scripts, fixtures, transcripts or logs. Do not add production module paths,
`include!` references or Cargo targets that require unpublished test files.
Existing tracked tests remain part of the published build; do not remove them or
disable checks to make a change pass. If new public coverage requires a different
test arrangement, discuss it with a maintainer before changing that arrangement.
Describe private test scenarios and results in the PR so maintainers can assess
coverage without making CI depend on private files. Keep valuable local tests
when preparing the commit.

## Local validation

From the repository root, run the build, full workspace tests, Clippy and
formatting checks described in [Development](../development.md#build--test).
For a read-only format check use `cargo fmt --all -- --check`.
[PR CI](../../.github/workflows/ci.yml) runs those same four checks with
`--locked` on Cargo test, Clippy and build to use the committed `Cargo.lock`.
Use that flag locally too when reproducing CI. Dependencies should change the
lockfile only intentionally; explain any such change in the PR.

Run relevant integration and platform checks as well. Provider-backed smoke
checks require your own configuration; never commit credentials. Record the
OS, toolchain, exact commands, pass/fail/ignored results and any unavailable
services or platform checks. Compilation alone is not a passing test run.

Before submitting, validate a clean checkout containing only the intended commit
files: build, compile tests and run static checks without private fixtures or
untracked files. Run the full workspace tests there when preparing the PR.
This verifies the public build independently of a local overlay. Do not claim
private integration coverage from `cargo test --workspace` alone.

## Commit and submit

Inspect `git status --short`, `git diff` and `git diff --check`. Stage only intended
source, public documentation and GitHub configuration paths, then inspect
`git diff --cached` and `git diff --cached --check`. Exclude local logs, private
tests, credentials and generated files; avoid indiscriminate `git add .`.

Use logical commits with a concise title explaining the outcome. A prefix such
as `memory:`, `core:`, `docs:` or `ci:` helps identify the area; it is a suggestion,
not an enforced syntax. Explain non-obvious motivation in the body. Then push:

```bash
git commit -m "docs: clarify contribution workflow"
git push -u origin fix/short-description
```

Open a PR from your fork branch to upstream `main`. Fill out the
[PR template](../../.github/PULL_REQUEST_TEMPLATE.md): purpose, related issue and
design agreement, implementation, architecture impact, test evidence,
compatibility and risks. Use `N/A` with a reason when a section does not apply.
Use a draft for incomplete work and mark it ready when reviewable. Explain data
migration, protocol/API/configuration compatibility and performance evidence
when affected. Mention remaining limitations explicitly.

## Review and update

Review follows the [review policy](reviewing.md). Answer substantive comments,
explaining the fix or why you disagree. Push focused follow-up commits and
summarize changes and checks rerun. Prefer preserving review history; coordinate
before rewriting a reviewed branch and use `--force-with-lease` if a rewrite is
agreed. Fetch upstream and merge `upstream/main` when needed to resolve conflicts,
then rerun affected validation. Preserve other contributors' work.

CI runs for PR creation, updates, reopening and readiness for review, including
fork PRs with read-only permissions and no project secrets. The baseline runs on
Windows, matching a supported native toolchain; the existing
[platform workflow](../../.github/workflows/platform-check.yml) retains native
Windows, Linux and macOS checks for its configured paths. A baseline pass does
not establish Linux confinement or macOS desktop correctness. CI changes must
not weaken checks or expose secrets to fork code.

If CI fails, inspect the failing step, reproduce it where possible and push a
fix. Report suspected unrelated failures with evidence to a maintainer; do not
skip failing tests or relax lints to obtain a green result. New commits require
fresh CI and review of the changed areas.

## Merge and follow-up

A maintainer makes the final acceptance, rejection and merge decision after the
[merge gate](reviewing.md#merge-gate) is satisfied. Maintainers may request a
smaller scope or close a PR with an explanation. Design agreement, community
approval and passing checks do not independently authorize merge.

Maintainers choose the merge method and preserve contributor attribution. After
merge, check linked issue resolution and report regressions with a PR reference.
You may delete your merged fork branch. A merged PR is not a release;
maintainers manage versioning and the separate tag-driven release workflow.
