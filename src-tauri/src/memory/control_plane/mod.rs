//! SQLite-owned control plane for sessionless continuity and local memory work.
//! Raw conversation text remains owned exclusively by `conversation_messages`.
//!
//! Projection stays disabled unless `SAAA_MEMORY_ENABLED=1`.

#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::HashSet, env};
mod items;
mod migrate;
use items::*;
pub use migrate::migrate_v11_to_v12;
mod source_window;
pub(super) use source_window::MAX_OBSERVABILITY_EVENTS;
pub use source_window::{
    CONTEXT_POLICY_VERSION, ProjectionItem, memory_enabled, ensure_continuity_state,
    cancel_unhandled_jobs, record_projection_event, load_projection_items,
};
#[cfg(test)]
pub use source_window::{
    SourceWindow, CapsuleItemInput, WorkingStateInput, record_completed_turn,
    insert_profile_candidate, confirm_profile_candidate, put_working_state,
    resolve_working_state, expire_working_state, activate_capsule_revision,
};
#[cfg(test)]
mod tests;
