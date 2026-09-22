//! Read-only routing IPC projections. SQLite remains the source of truth for reconnects.
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use ts_rs::{Config, TS};
include!("ipc.d/01.rs");
include!("ipc.d/02.rs");
