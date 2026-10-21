# Round 4 regression tests

Base: AX `main@d761fdc`; branch `test/round-4-regressions`.

Tests stay under `test/` per workspace rules. No implementation or production
Cargo target depends on these files. `run.py` creates a disposable copy and adds
only test modules and a harness-free Linux test target there. It uses the actual
production implementations, including the private worker `process()` and
`parse_actions()` functions; it does not extract or copy their logic.

```powershell
python test/round4_regressions/run.py ../axtest/round4-regressions-run
cargo test --workspace --manifest-path ../axtest/round4-regressions-run/Cargo.toml
```

Keep separate Cargo target directories for the original tree and the disposable
copy so switching trees cannot reuse a test binary from a different module set.

The runner also emits a tar archive. Extract it to a native Linux filesystem,
then run `cargo test --workspace --no-fail-fast` there. The Linux sandbox cannot
use the Windows-mounted WSL filesystem as its confined workspace. The Linux
target skips its six cases only when `/usr/bin/bwrap` is absent; if bwrap exists,
preparation or runtime failures fail the suite. On other platforms, the Linux
target is excluded and prints a platform skip message.

## Cross-platform unit tests

- `eof_insertion_after_unterminated_lf_preserves_regions`: exact `a\nb` + `c`
  insertion at line 3, full byte result and numbered regions.
- `eof_insertion_after_unterminated_crlf_preserves_regions`: exact CRLF version.
- `bracketed_notice_before_json_array_still_parses_actions`: `[注意]` prose
  followed by a valid IGNORE action.
- `invalid_batch_retries_accumulate_across_new_experiences_and_audit_skip`:
  start at a nonzero processed cursor, append before each retry, verify changed
  end cursors, counts 1/2/3, no early skip, third skip audit and persisted cursor.
- `three_provider_timeouts_only_increment_consecutive_failures`: pending fake
  provider exercises the actual timeout path three times, checks error type,
  unchanged cursors/attempts and persisted backoff counts after each attempt.
- `three_provider_errors_only_increment_consecutive_failures`: HTTP 503 fake
  provider exercises the nested provider error path with the same assertions.

## Linux sandbox cases

- `sandbox_missing_read_is_not_permission_denied`
- `sandbox_missing_search_is_not_permission_denied`
- `sandbox_missing_parent_write_is_not_permission_denied`
- `outside_symlink_is_a_sandbox_violation`
- `successful_command_keeps_background_child`
- `proxy_disconnect_kills_background_child`

The missing-path tests check typed errors and the actual runtime ToolFinished
envelope's `permission_denied` and status. The search case requires an error;
an empty successful result does not satisfy this regression. The disconnect
case kills only the host proxy, then verifies its background child is absent
or a non-executing zombie and the command group's heartbeat has stopped.

## Known failing contract exposed on d761fdc

The missing-search case returns success with an empty matches array and
`permission_denied=false`, rather than an ordinary error. Its assertion is
intentionally retained and is not ignored. This test-only task does not change
the implementation. Therefore the Linux regression workspace has a failing
target until that behavior is fixed in a separate implementation change.
