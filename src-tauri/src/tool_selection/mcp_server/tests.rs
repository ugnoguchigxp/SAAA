#![cfg(test)]
//! D5 acceptance tests. The server is exercised over a real loopback HTTP socket with a real
//! bearer token; the backend is the deterministic fixture so no paid service is contacted.

use super::config::McpServerConfig;
use super::ServerHandle;
use crate::persistence::SqliteWriter;
use crate::tool_selection::backends::{
    BackendOutcome, BackendRequest, FixtureBackend, ToolBackend,
};
use crate::tool_selection::catalog::{self, CatalogEntry, UsagePage};
use crate::tool_selection::contracts::now_ms;
use crate::tool_selection::extraction::UnconfiguredExtractor;
use crate::tool_selection::gateway;
use crate::tool_selection::inference::{EmbeddingProvider, HashEmbedding, HashReranker};
use crate::tool_selection::repository;
use crate::tool_selection::service::{self, ToolSelectionService};
use crate::RunCancellation;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
include!("tests.d/01.rs");
include!("tests.d/02.rs");
include!("tests.d/03.rs");
include!("tests.d/04.rs");
