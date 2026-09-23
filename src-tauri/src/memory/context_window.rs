//! Builds an ephemeral, bounded provider context from raw SQLite conversation events.
//! Continuity groups are source-backed projections and are not long-term memory records.

use super::control_plane;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};
#[path = "context_window/projected_context_message.rs"]
mod projected_context_message;
#[path = "context_window/group_older_history.rs"]
mod group_older_history;
pub use projected_context_message::{ProjectedContextMessage, ContinuityGroup, ContextHealthReport, ContextWindow};
#[cfg(test)]
pub use projected_context_message::build;
pub(crate) use projected_context_message::{LoadedContextWindow, validate_current_instruction, load, compose};
use projected_context_message::{MAX_SOURCE_MESSAGES, MAX_PROJECTED_INPUT_BYTES, MAX_RECENT_MESSAGES, MAX_RECENT_BYTES, MAX_MEMORY_BYTES, MAX_CONTINUITY_GROUPS, MAX_CONTINUITY_BYTES, MAX_GROUP_MESSAGES, MAX_GROUP_USER_TURNS, MAX_GROUP_SOURCE_BYTES, SourceMessage, render_memory_projection, render_recent_history, render_recent_line};
#[cfg(test)]
use projected_context_message::build_with_memory;
use group_older_history::{group_older_history, project_group, select_continuity_groups, render_group, quote_history, truncate_utf8, event_ref, database_error};
#[cfg(test)]
#[path = "context_window/tests/mod.rs"]
mod tests;
