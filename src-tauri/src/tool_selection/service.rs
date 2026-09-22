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
include!("service.d/01.rs");
include!("service.d/02.rs");
include!("service.d/03.rs");
include!("service.d/04.rs");
