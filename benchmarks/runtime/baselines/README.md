# Published runtime baselines

These selected measurement artifacts are published at the repository owner's
explicit request. Ordinary runner output remains ignored under `../results/`.
No private regression test source, local logs, credentials, databases or compiled
executables are included.

## 2026-11-02 Windows x64

[`2026-11-02-windows-x64/report.md`](2026-11-02-windows-x64/report.md)
contains the full Phase 1 measurement report. Its `raw.json`, `summary.json` and
`samples.csv` preserve all 150 case-rounds / 3000 recorded samples, with zero
benchmark worker failures. The companion
[`startup-comparison/report.md`](2026-11-02-windows-x64/startup-comparison/report.md)
compares installed AX, Codex, OpenCode and Pi version-command launch paths on
the same Windows host (300 recorded launches across five launch paths).

The original benchmark ran against HEAD
`54bcce9e17be15ed09d97e5dc1d496f89c4bf283` with local edits, as recorded in its
metadata. It is **not** represented as a clean-checkout measurement of the
publication commit. Production-source hashes, harness hashes and executable
hashes identify the measured inputs; absolute paths identify that measurement
environment and are not prerequisites for building this package. No private
patch or unrelated local edit is included in this publication.

The Phase 1 measurement itself used Release, offline/locked builds and fresh
worker directories. Its warmups and repetitions, operation definitions, resource
scope and unmeasured features are documented in the generated report and the
[harness README](../README.md). The benchmark numbers are descriptive results
from one machine, not a CI threshold, product ranking or Agent capability score.

The companion startup comparison uses only `--version`. It excludes shell shims
and distinguishes native Codex from its npm launcher. Its reported process
working set is not an entire running agent's memory. The npm launcher's child
memory is intentionally not ranked. Pi and the npm launcher explicitly used the
already installed Node 22.20.0 after an exploratory Node 20 incompatibility;
only the successful Node 22 campaign is published here.

`startup-comparison/runner.py` archives the measurement script with that host's
explicit paths. It is a machine-specific campaign record, not the portable
benchmark entry point: adapt its imports, commands and output paths before
rerunning elsewhere. Use `python benchmarks/runtime/run.py` for the portable
Phase 1 suite. Raw records and summaries remain byte-for-byte copies of the
original successful runs.

See [VALIDATION.md](2026-11-02-windows-x64/VALIDATION.md) for the distinction
between original working-tree verification and checks on the isolated intended
publication file set.
