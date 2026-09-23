#![cfg(test)]
//! Deterministic G01–G20 fixtures from the implementation guide. Ranker/extractor doubles return
//! fixed values; the same semantics are exercised against real models in the separate ML lane.

#![allow(clippy::too_many_arguments)]

pub(super) use super::backends::{BackendOutcome, BackendRequest, FixtureBackend, ToolBackend};
pub(super) use super::catalog::{self, CatalogEntry, UsagePage};
pub(super) use super::contracts::*;
pub(super) use super::extraction::{CorrectionExtractor, FixtureExtractor};
pub(super) use super::feedback::ParsedExtraction;
use super::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError, RerankProvider,
};
pub(super) use super::repository::{self, Epochs};
pub(super) use super::service::ToolSelectionService;
pub(super) use super::*;
pub(super) use crate::persistence::SqliteWriter;
pub(super) use async_trait::async_trait;
pub(super) use rusqlite::{Connection, TransactionBehavior};
pub(super) use serde_json::{json, Value};
pub(super) use std::sync::atomic::{AtomicUsize, Ordering};
pub(super) use std::sync::{Arc, Mutex};
mod constant_embedding;
pub(super) use constant_embedding::*;
mod g03_other_object_type_keeps_the_base_order;
mod g19_old_execution_ref_is_stale_after_a_revision_;
mod mixed_backend;
