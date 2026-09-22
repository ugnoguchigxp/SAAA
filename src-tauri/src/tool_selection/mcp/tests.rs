#![cfg(test)]
//! D4 deterministic acceptance tests. A real HTTP/1.1 test server speaks the MCP Streamable
//! transport (JSON and SSE) so transport, sync, invocation and continuation are exercised over a
//! socket, not against a mocked trait. No external network service is used.

use super::config::{McpGrantScope, McpGrantSpec, McpSourceSpec, McpSources};
use super::descriptors;
use super::manager::McpManager;
use super::results;
use crate::persistence::SqliteWriter;
use crate::tool_selection::backends::mcp::McpBackend;
use crate::tool_selection::backends::router::BackendRouter;
use crate::tool_selection::backends::FixtureBackend;
use crate::tool_selection::contracts::now_ms;
use crate::tool_selection::extraction::UnconfiguredExtractor;
use crate::tool_selection::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError,
};
use crate::tool_selection::repository;
use crate::tool_selection::service::ToolSelectionService;
use crate::tool_selection::{RequestContext, Scenario};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
include!("tests.d/01.rs");
include!("tests.d/02.rs");
include!("tests.d/03.rs");
include!("tests.d/04.rs");
include!("tests.d/05.rs");
include!("tests.d/06.rs");
include!("tests.d/07.rs");
include!("tests.d/08.rs");
