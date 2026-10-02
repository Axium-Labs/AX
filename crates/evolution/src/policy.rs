//! The evolved-skill lifecycle policy: which transitions are legal, and how a
//! model proposal maps onto them.
//!
//! Policy is deliberately separate from the mutation boundary in
//! `Engine::apply`: this module decides, the engine enforces. No
//! function here touches the filesystem or the ledger.

use anyhow::Result;

use crate::{Metadata, State};

/// Legal transitions. Anything else is refused, so a proposal can never skip
/// trial or resurrect a deleted package.
pub(crate) fn transition_allowed(previous: State, next: State) -> bool {
    matches!(
        (previous, next),
        (State::Candidate, State::Trial)
            | (State::Trial, State::Active)
            | (
                State::Candidate | State::Trial | State::Active,
                State::Archived
            )
            | (State::Archived, State::Deleted)
    )
}

/// Candidate becomes a trial, and only a trial with successful usage becomes
/// active. Anything else is a rejected proposal, never a silent promotion.
pub(crate) fn promoted_state(meta: &Metadata) -> Result<State> {
    match meta.state {
        State::Candidate => Ok(State::Trial),
        State::Trial
            if meta.use_count > meta.trial_baseline
                && meta.success_count > meta.trial_success_baseline =>
        {
            Ok(State::Active)
        }
        _ => anyhow::bail!("promotion requires trial usage telemetry"),
    }
}

/// A retired package is archived first, and only deleted once it has survived a
/// full epoch in the archive.
pub(crate) fn retired_state(meta: &Metadata, epoch: u64) -> State {
    if meta.state == State::Archived && meta.epoch < epoch {
        State::Deleted
    } else {
        State::Archived
    }
}
