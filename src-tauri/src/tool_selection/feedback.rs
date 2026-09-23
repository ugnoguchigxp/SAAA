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
#[path = "feedback/apply_extraction.rs"]
mod apply_extraction;
#[path = "feedback/extraction_failure.rs"]
mod extraction_failure;
pub use apply_extraction::apply_extraction;
pub use extraction_failure::{parse_extraction, ApplyOutcome, ExtractionFailure, ParsedExtraction};
use extraction_failure::{resolve_scope, resolved_condition, signature, ResolvedScope};
