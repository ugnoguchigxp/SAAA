//! SQL access for the tool-selection ledger. Every function takes a `&Connection` so it can run
//! inside the existing `SqliteWriter` transaction; there is no second connection pool.

use super::contracts::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
include!("repository.d/01.rs");
include!("repository.d/02.rs");
