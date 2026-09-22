#![cfg(test)]
use super::*;
use crate::persistence::sqlite::SqliteWriter;
use rusqlite::{params, Connection};
use serde_json::json;
include!("tests.d/01.rs");
include!("tests.d/02.rs");
