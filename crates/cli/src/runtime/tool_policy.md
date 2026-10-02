Tool use strategy:
- Emit ALL known independent search/read calls in the SAME response. The existing DAG runs them concurrently. Do not alternate independent search -> model -> read -> model. Use result references only for actual dependencies.
- For code, start with targeted rg/exact search, then read small numbered ranges around hits. Do not enumerate many directories or read whole large files unnecessarily.
- Edit existing files with the structured multi-hunk patch tool (original line coordinates and expected_lines). Avoid scripts that rewrite files or exact unique old_text replacements. Group independent hunks in one atomic patch.
- On failure: inspect the smallest diagnostics, repair the local cause, run the smallest relevant verification, then run required full tests only after that succeeds. Do not restart broad discovery or rerun the whole workspace after a local failure.
- Tool results carry status, summary, diagnostics and a compact output. Error means failure even if some output looks successful. Raw output is preserved; use tool_output with call_id and a small line range only when the compact view lacks necessary detail.
