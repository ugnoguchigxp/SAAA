//! Builds an ephemeral, bounded provider context from raw SQLite conversation events.
//! Continuity groups are source-backed projections and are not long-term memory records.

use super::control_plane;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};
#[path = "context_window/group_older_history.rs"]
mod group_older_history;
#[path = "context_window/projected_context_message.rs"]
mod projected_context_message;
use group_older_history::{
    database_error, event_ref, group_older_history, project_group, quote_history, render_group,
    select_continuity_groups, truncate_utf8,
};
#[cfg(test)]
pub use projected_context_message::build;
#[cfg(test)]
use projected_context_message::build_with_memory;
pub(crate) use projected_context_message::{
    compose, load, validate_current_instruction, LoadedContextWindow,
};
use projected_context_message::{
    render_memory_projection, render_recent_history, render_recent_line, SourceMessage,
    MAX_CONTINUITY_BYTES, MAX_CONTINUITY_GROUPS, MAX_GROUP_MESSAGES, MAX_GROUP_SOURCE_BYTES,
    MAX_GROUP_USER_TURNS, MAX_MEMORY_BYTES, MAX_PROJECTED_INPUT_BYTES, MAX_RECENT_BYTES,
    MAX_RECENT_MESSAGES, MAX_SOURCE_MESSAGES,
};
pub use projected_context_message::{
    ContextHealthReport, ContextWindow, ContinuityGroup, ProjectedContextMessage,
};
#[cfg(test)]
#[path = "context_window/tests/mod.rs"]
mod tests;
