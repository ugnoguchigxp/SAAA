#![cfg(test)]
//! D4 deterministic acceptance tests. A real HTTP/1.1 test server speaks the MCP Streamable
//! transport (JSON and SSE) so transport, sync, invocation and continuation are exercised over a
//! socket, not against a mocked trait. No external network service is used.

pub(super) use super::config::{McpGrantScope, McpGrantSpec, McpSourceSpec, McpSources};
pub(super) use super::{MCP_PROTOCOL_VERSION, MCP_SOURCE_STALE_AFTER_MILLIS, MCP_RESULT_MAX_BYTES};
pub(super) use super::repository as mcp_repo;
pub(super) use super::descriptors;
pub(super) use super::manager::McpManager;
pub(super) use super::results;
pub(super) use crate::persistence::SqliteWriter;
pub(super) use crate::tool_selection::backends::mcp::McpBackend;
pub(super) use crate::tool_selection::backends::router::BackendRouter;
pub(super) use crate::tool_selection::backends::FixtureBackend;
pub(super) use crate::tool_selection::contracts::now_ms;
pub(super) use crate::tool_selection::extraction::UnconfiguredExtractor;
use crate::tool_selection::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError,
};
pub(super) use crate::tool_selection::repository;
pub(super) use crate::tool_selection::service::ToolSelectionService;
pub(super) use crate::tool_selection::{RequestContext, Scenario};
pub(super) use rusqlite::Connection;
pub(super) use serde_json::{json, Value};
pub(super) use std::collections::HashMap;
pub(super) use std::net::SocketAddr;
pub(super) use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
pub(super) use std::sync::{Arc, Mutex};
pub(super) use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(super) use tokio::net::TcpListener;
#[path = "tests/server_state.rs"]
mod server_state;
use server_state::*;
#[path = "tests/user_grant.rs"]
mod user_grant;
use user_grant::{user_grant, scenario};
pub(super) use user_grant::describe_and_invoke;
#[path = "tests/t07_cancel_before_send_makes_zero_calls.rs"]
mod t07_cancel_before_send_makes_zero_calls;
#[path = "tests/t02_v23_sources_are_rebuilt_without_losing_data.rs"]
mod t02_v23_sources_are_rebuilt_without_losing_data;
#[path = "tests/review_after_send_cancel_is_unknown_and_releases.rs"]
mod review_after_send_cancel_is_unknown_and_releases;
use review_after_send_cancel_is_unknown_and_releases::{d5_config, d5_token, d5_open_session, d5_envelope, d5_tool, d5_post};
#[path = "tests/h10_large_mcp_result_pages_over_the_published_wi.rs"]
mod h10_large_mcp_result_pages_over_the_published_wi;
#[path = "tests/h12_mcp_correction_applies_to_the_matching_conve.rs"]
mod h12_mcp_correction_applies_to_the_matching_conve;
pub(super) use h12_mcp_correction_applies_to_the_matching_conve::redirect_server;
#[path = "tests/a15_redirect_is_not_followed.rs"]
mod a15_redirect_is_not_followed;
