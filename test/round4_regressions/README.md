# Round 4 regression tests

These tests are part of the workspace's regular test suite:

```sh
cargo test --workspace
```

The unit tests are included by `patch.rs` and `worker.rs` under `#[cfg(test)]`.
They exercise the actual implementations, including the private worker
`process()` and `parse_actions()` functions.

`linux.rs` is the `runtime-core` crate's `round4_regressions_linux` test target
with `harness = false`. On Linux it runs all six sandbox cases when
`/usr/bin/bwrap` exists; preparation or runtime failures fail the target. If
`/usr/bin/bwrap` is absent, or the platform is not Linux, the target prints
`SKIP` and exits successfully. Use a native Linux filesystem for the repository
and sandbox workspace when running through WSL.

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
