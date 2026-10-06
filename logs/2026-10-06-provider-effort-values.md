## 2026-10-06 16:28

Task: Preserve provider reasoning effort values for the Crew model controls

Changed:
- crates/model/src/lib.rs: add distinct None, Minimal and Ultra variants rather
  than converting minimal to low or ultra to max. Catalogue JSON, CLI parse and
  request serialization keep the provider value. Existing model subsets unchanged.
- crates/cli/src/model_selection.rs: update invalid-effort diagnostic.
- crates/model/Cargo.toml and test/providers/reasoning_catalogue.rs: wire-value
  round-trip and advertised-subset regressions.
- docs/providers.md: document catalogue-backed capability values and Crew usage.

Why:
- Crew must show and send the selected provider/model's actual supported effort
  values; alias normalization lost information before it reached the desktop.

Tests:
- cargo test -p model --test reasoning_catalogue: 2 passed.
- cargo test -p cli model_selection::tests: 10 passed.
- cargo check -p cli: PASS.
- git diff --check: PASS.
- Crew bridge/UI/browser tests passed; see Crew conversation-controls log.

No release, version change, commit or installed binary replacement.
