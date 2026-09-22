//! Extraction-output validation and the correction-application transaction.
//!
//! Parsing is strict (fixed keys, byte-exact evidence spans, only host-provided IDs) but a
//! malformed proposal never blocks the turn: the scenario degrades and the proposal is recorded
//! without creating a rule.

use super::contracts::*;
use super::repository::{self, NewFeedback, NewRule};
use rusqlite::Connection;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
include!("feedback.d/01.rs");
include!("feedback.d/02.rs");
