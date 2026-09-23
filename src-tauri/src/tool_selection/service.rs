//! Orchestration for the three entry points plus correction ingestion. Reads snapshot the ledger
//! and release the writer lock before any inference; writes re-check epochs inside one immediate
//! transaction so a rule or ACL change cannot be published behind an older selection.

#![allow(private_interfaces)]

use super::backends::mcp::McpBinding;
use super::backends::router::BackendRouter;
use super::backends::{BackendRequest, TechnicalStatus, ToolBackend};
use super::catalog::{self, CatalogEntry};
use super::contracts::*;
use super::extraction::{CorrectionExtractor, ExtractionRequest, RecentDecision};
use super::feedback::{apply_extraction, ParsedExtraction};
use super::inference::{EmbedKind, EmbeddingProvider, RerankProvider};
use super::mcp::manager::McpManager;
use super::references::{ReferenceEntry, ReferenceKind, ReferenceStore};
use super::repository::{self, EligibleRevision, Epochs};
use super::{ranking, retrieval, rules};
use crate::persistence::{load_role_routing_settings, SqliteWriter};
use crate::RunCancellation;
use base64::Engine;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
#[path = "service/search_candidate.rs"]
mod search_candidate;
#[path = "service/rank.rs"]
mod rank;
#[path = "service/persist_error.rs"]
mod persist_error;
#[path = "service/reconcile_interrupted_invocations.rs"]
mod reconcile_interrupted_invocations;
pub use search_candidate::{
    SearchCandidate, SearchResponse, DescribeResponse, InvokeResponse, ResultPageResponse,
    TurnOutcome, ToolSelectionService,
};
use search_candidate::{
    SEARCH_CANDIDATE_POOL, BACKEND_TIMEOUT_MS, EMBED_BATCH_SIZE, MAX_SEARCH_ATTEMPTS, Snapshot,
    RankOutcome,
};
use persist_error::{CHANGED, PersistError};
pub use reconcile_interrupted_invocations::reconcile_interrupted_invocations;
pub use persist_error::ensure_principal;
use reconcile_interrupted_invocations::{
    write_transaction, title_and_summary, validate_arguments, encode_cursor, decode_cursor,
};
// Methods (search, describe, invoke, new, …) live on ToolSelectionService via impls in children.
