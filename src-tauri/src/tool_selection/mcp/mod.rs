//! D4 external-MCP support: host-managed Streamable HTTP servers, an atomic SQLite tool
//! ledger sync, a source-scoped session pool, and continuation storage for large results.
//!
//! The protocol surface is deliberately narrowed to what this milestone verifies:
//! Streamable HTTP (POST JSON plus text/event-stream), protocol version `2025-06-18`, the
//! optional GET stream for `notifications/tools/list_changed`, and `initialize` / `tools/list` /
//! `tools/call` / `notifications/cancelled` / `session DELETE`. stdio, the legacy HTTP+SSE
//! transport, OAuth, sampling, elicitation, roots and resources/read are not implemented. An
//! unsupported server request receives a JSON-RPC method-not-found error and is never executed.

#![allow(private_interfaces)]

pub mod config;
pub mod descriptors;
pub mod manager;
pub mod repository;
pub mod results;
pub mod schema;
pub mod service_support;
pub mod session;
pub mod sync;
pub mod transport;
pub mod wiring;

mod tests;

pub use config::{McpGrantScope, McpGrantSpec, McpSourceSpec, McpSources};

/// Protocol version fixed for this milestone. This is the verified compatibility range, not a
/// claim that it is the newest published revision.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Host-managed source limits from the D4 contract. These are SAAA's own judgement, not MCP
/// protocol limits.
pub const MCP_SOURCES_MAX: usize = 32;
pub const MCP_SOURCES_FILE_MAX_BYTES: u64 = 1024 * 1024;
pub const MCP_TOOLS_PER_SOURCE_MAX: usize = 10_000;
pub const MCP_TOOLS_PER_PROFILE_MAX: usize = 100_000;
pub const MCP_LIST_PAGES_MAX: usize = 2_000;
pub const MCP_LIST_PAGE_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MCP_LIST_TOTAL_MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MCP_LIST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
pub const MCP_DESCRIPTION_MAX_BYTES: usize = 128 * 1024;
pub const MCP_SCHEMA_MAX_BYTES: usize = 8 * 1024;
pub const MCP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const MCP_INITIALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub const MCP_SSE_EVENT_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MCP_CALL_RESPONSE_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MCP_RESULT_MAX_BYTES: usize = 1024 * 1024;
pub const MCP_RESULT_PROFILE_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const MCP_RESULT_MAX_PER_RUN: usize = 64;
pub const MCP_RESULT_TTL_MILLIS: i64 = 10 * 60 * 1000;
pub const MCP_RESULT_PAGE_BYTES: usize = 8 * 1024;
pub const MCP_SOURCE_STALE_AFTER_MILLIS: i64 = 300 * 1000;
pub const MCP_SOURCE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
pub const MCP_NOTIFICATION_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
pub const MCP_CALLS_PER_SOURCE_MAX: usize = 4;
pub const MCP_CALLS_PER_PROFILE_MAX: usize = 16;
pub const MCP_CALL_QUEUE_MAX: usize = 64;
pub const MCP_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
pub const MCP_BACKOFF_SECONDS: [u64; 6] = [1, 2, 4, 8, 16, 30];
