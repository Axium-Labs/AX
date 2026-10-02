//! Project instructions for one turn.
//!
//! Instructions are resolved once per turn from the repository itself: the
//! global file, `AGENTS.md` at every directory level from the repository root
//! down to the cwd, and `.ax/rules/*.md` whose path scope matches a file the
//! task names. The result is deterministic — the same tree and the same prompt
//! always produce the same instruction set — so it is never recalled through
//! memory or a semantic index, and it is kept in its own context slot so it
//! cannot be confused with retrieved memory, a skill or system context.
//!
//! The complete source of every segment (`source_path`, scope and provenance)
//! travels with the content, so a rule can always be traced back to the file
//! that owns it.

use std::path::{Path, PathBuf};

use runtime_core::{InstructionResolution, InstructionResolver};

use crate::{config, repl::ReplState};

/// Tokens a prompt may name; bounded so a pasted document cannot turn into an
/// unbounded filesystem probe.
const MAX_TARGETS: usize = 32;

/// Resolve this turn's instructions without touching the session.
#[must_use]
pub(crate) fn resolve(state: &ReplState, prompt: &str) -> InstructionResolution {
    let resolver = InstructionResolver::new(
        state.project_root.clone(),
        Some(config::ax_home().join(runtime_core::instructions::AGENTS_FILE)),
    );
    let cwd = std::env::current_dir().unwrap_or_else(|_| state.project_root.clone());
    resolver.resolve(&cwd, &prompt_targets(&state.project_root, prompt))
}

/// Install the turn's instructions, replacing the previous turn's copy.
pub(crate) fn install(state: &mut ReplState, prompt: &str, token_budget: usize) -> usize {
    let resolution = resolve(state, prompt);
    let message = resolution.message(token_budget);
    if let Some(runtime) = state.runtime.as_mut() {
        runtime.set_context(runtime_core::instructions::CONTEXT_PREFIX, message);
    }
    resolution.segments.len()
}

/// Repository-relative paths a prompt names: `@` references and bare tokens
/// that resolve to an existing file. No index, no scan, no embedding.
fn prompt_targets(root: &Path, prompt: &str) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = Vec::new();
    for raw in prompt.split_whitespace() {
        if targets.len() >= MAX_TARGETS {
            break;
        }
        let token = raw.trim_start_matches('@').trim_matches(['"', '\'']);
        if token.is_empty() || token.len() > 512 {
            continue;
        }
        let trimmed = token.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
        for candidate in [token, trimmed] {
            if candidate.is_empty() || !looks_like_path(candidate) {
                continue;
            }
            let path = if Path::new(candidate).is_absolute() {
                PathBuf::from(candidate)
            } else {
                root.join(candidate)
            };
            if path.exists() && !targets.contains(&path) {
                targets.push(path);
                break;
            }
        }
    }
    targets
}

fn looks_like_path(token: &str) -> bool {
    token.contains('/') || token.contains('\\') || token.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ax-cli-instructions-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_paths_that_exist_become_instruction_targets() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.0.join("src/api")).unwrap();
        fs::write(fixture.0.join("src/api/handler.rs"), "fn main() {}").unwrap();
        let targets = prompt_targets(
            &fixture.0,
            "@src/api/handler.rs fix the handler, and also src/missing.rs",
        );
        assert_eq!(targets.len(), 1);
        assert!(targets[0].ends_with(Path::new("src/api").join("handler.rs")));
    }

    #[test]
    fn a_prompt_without_paths_yields_no_targets() {
        let fixture = Fixture::new();
        assert!(prompt_targets(&fixture.0, "please make the tests pass").is_empty());
        assert!(prompt_targets(&fixture.0, "v1.2.3 is out").is_empty());
    }
}
