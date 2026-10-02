//! The session state and its lifecycle, independent of any frontend.

mod state;

pub(crate) use state::{
    ReplState, SKILL_CONTEXT_PREFIX, active_skill_name, memory_role, restore_message,
};
