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
include!("mod.d/01.rs");
include!("mod.d/02.rs");
