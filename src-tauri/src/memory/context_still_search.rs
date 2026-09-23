use crate::tool_selection::mcp::transport::{HttpTransport, TransportError};
use serde::Deserialize;
pub(super) use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use url::{Host, Url};
#[path = "context_still_search/context_still_search_client.rs"]
mod context_still_search_client;
pub use context_still_search_client::{SEARCH_KNOWLEDGE_TOOL_NAME, SEARCH_EPISODES_TOOL_NAME, MAX_CONTEXT_STILL_CALLS_PER_TURN, ContextStillSearchClient, SearchError, is_search_tool, tool_definitions};
pub(super) use context_still_search_client::{parse_arguments, compact_result};
#[cfg(test)]
pub(super) use context_still_search_client::SEARCH_CALL_LOG;
#[cfg(test)]
pub(super) use crate::{StartTurnInput, RuntimeEvent, RunCancellation};
#[cfg(test)]
#[path = "context_still_search/tests.rs"]
mod tests;
