//! External MCP source lifecycle: configuration reload, atomic sync, config-derived grants,
//! embedding backfill, freshness, notification-driven polling and shutdown.
//!
//! The manager is the only place that knows the host-managed source list. The LLM never sees a
//! source id, URL, token or grant, and no management tool is exposed to the model.

use super::super::contracts::now_ms;
use super::super::inference::{EmbedKind, EmbeddingProvider};
use super::super::repository;
use super::config::{McpGrantScope, McpSources};
use super::descriptors;
use super::repository as mcp_repository;
use super::session::{CallError, McpSessionPool, SessionState};
use super::sync::{self, SyncError, SyncOutcome};
use super::{MCP_NOTIFICATION_DEBOUNCE, MCP_SOURCE_POLL_INTERVAL, MCP_SOURCE_STALE_AFTER_MILLIS};
use crate::persistence::SqliteWriter;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock};
#[path = "manager/mcp_manager.rs"]
mod mcp_manager;
#[path = "manager/desired_grants.rs"]
mod desired_grants;
pub use mcp_manager::McpManager;
pub(super) use desired_grants::normalize_loopback;
// Methods live on McpManager via impls in child modules.
