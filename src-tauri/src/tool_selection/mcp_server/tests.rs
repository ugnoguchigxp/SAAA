#![cfg(test)]
//! D5 acceptance tests. The server is exercised over a real loopback HTTP socket with a real
//! bearer token; the backend is the deterministic fixture so no paid service is contacted.

pub(super) use super::config::McpServerConfig;
pub(super) use super::start;
pub(super) use super::ServerHandle;
pub(super) use crate::persistence::SqliteWriter;
use crate::tool_selection::backends::{
    BackendOutcome, BackendRequest, FixtureBackend, ToolBackend,
};
pub(super) use crate::tool_selection::catalog::{self, CatalogEntry, UsagePage};
pub(super) use crate::tool_selection::contracts::now_ms;
pub(super) use crate::tool_selection::extraction::UnconfiguredExtractor;
pub(super) use crate::tool_selection::gateway;
pub(super) use crate::tool_selection::inference::{EmbeddingProvider, HashEmbedding, HashReranker};
pub(super) use crate::tool_selection::repository;
pub(super) use crate::tool_selection::service::{self, ToolSelectionService};
pub(super) use crate::RunCancellation;
pub(super) use rusqlite::Connection;
pub(super) use serde_json::{json, Value};
pub(super) use std::sync::Arc;
pub(super) use std::time::Duration;
pub(super) use tokio::sync::Semaphore;
#[path = "tests/d5.rs"]
mod d5;
use d5::*;
#[path = "tests/blocking_backend.rs"]
mod blocking_backend;
use blocking_backend::*;
#[path = "tests/rr_11_detached_owner_settles.rs"]
mod rr_11_detached_owner_settles;
pub(super) use rr_11_detached_owner_settles::{prepare_execution_ref, spawn_tool_call, wait_for_invocation_status};
#[path = "tests/h08_delete_cancels_the_call_and_closes_the_sessi.rs"]
mod h08_delete_cancels_the_call_and_closes_the_sessi;
