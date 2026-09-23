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
#[path = "feedback/extraction_failure.rs"]
mod extraction_failure;
#[path = "feedback/apply_extraction.rs"]
mod apply_extraction;
pub use extraction_failure::{ExtractionFailure, ParsedExtraction, parse_extraction, ApplyOutcome};
use extraction_failure::{ResolvedScope, resolve_scope, resolved_condition, signature};
pub use apply_extraction::{apply_extraction};
