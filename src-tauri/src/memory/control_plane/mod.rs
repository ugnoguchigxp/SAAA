//! SQLite-owned control plane for sessionless continuity and local memory work.
//! Raw conversation text remains owned exclusively by `conversation_messages`.
//!
//! Projection follows the saved preference; SAAA_MEMORY_ENABLED is an explicit override.

#[cfg(test)]
use rusqlite::OptionalExtension;
#[cfg(any(test, feature = "offline-contracts"))]
use rusqlite::{params, Connection};
#[cfg(any(test, feature = "offline-contracts"))]
use serde_json::Value;
#[cfg(any(test, feature = "offline-contracts"))]
use std::{collections::HashSet, env};
mod items;
mod migrate;
#[cfg(any(test, feature = "offline-contracts"))]
use items::*;
pub use migrate::migrate_v11_to_v12;
mod source_window;
#[cfg(any(test, feature = "offline-contracts"))]
pub(super) use source_window::MAX_OBSERVABILITY_EVENTS;
#[cfg(test)]
pub use source_window::{
    activate_capsule_revision, confirm_profile_candidate, expire_working_state,
    insert_profile_candidate, put_working_state, record_completed_turn, resolve_working_state,
    CapsuleItemInput, SourceWindow, WorkingStateInput,
};
pub use source_window::{
    cancel_unhandled_jobs, ensure_continuity_state, load_projection_items, memory_enabled,
    restore_memory_preference, ProjectionItem,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub use source_window::{record_projection_event, CONTEXT_POLICY_VERSION};
#[cfg(test)]
mod tests;
