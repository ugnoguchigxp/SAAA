#![cfg(test)]
//! Deterministic G01–G20 fixtures from the implementation guide. Ranker/extractor doubles return
//! fixed values; the same semantics are exercised against real models in the separate ML lane.

#![allow(clippy::too_many_arguments)]

use super::backends::{BackendOutcome, BackendRequest, FixtureBackend, ToolBackend};
use super::catalog::{self, CatalogEntry, UsagePage};
use super::contracts::*;
use super::extraction::{CorrectionExtractor, FixtureExtractor};
use super::feedback::ParsedExtraction;
use super::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError, RerankProvider,
};
use super::repository::{self, Epochs};
use super::service::ToolSelectionService;
use crate::persistence::SqliteWriter;
use async_trait::async_trait;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
include!("mod.d/01.rs");
include!("mod.d/02.rs");
include!("mod.d/03.rs");
include!("mod.d/04.rs");
