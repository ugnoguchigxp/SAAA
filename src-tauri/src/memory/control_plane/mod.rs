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
#[cfg(test)]
pub use source_window::{
    activate_capsule_revision, confirm_profile_candidate, expire_working_state,
    insert_profile_candidate, put_working_state, record_completed_turn, resolve_working_state,
    CapsuleItemInput, SourceWindow, WorkingStateInput,
};
pub use source_window::{
    cancel_unhandled_jobs, ensure_continuity_state, load_projection_items, memory_enabled,
    record_projection_event, ProjectionItem, CONTEXT_POLICY_VERSION,
};
#[cfg(test)]
mod tests;
