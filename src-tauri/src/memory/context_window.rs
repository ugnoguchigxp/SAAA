//! Builds an ephemeral, bounded provider context from raw SQLite conversation events.
//! Continuity groups are source-backed projections and are not long-term memory records.

use super::control_plane;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};
include!("context_window.d/01.rs");
include!("context_window.d/02.rs");
#[cfg(test)]
mod tests {
    include!("context_window.d/03.rs");
    include!("context_window.d/04.rs");
}
