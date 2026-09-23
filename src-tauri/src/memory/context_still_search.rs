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
#[cfg(test)]
pub(super) use crate::{RunCancellation, RuntimeEvent, StartTurnInput};
#[cfg(test)]
pub(super) use context_still_search_client::SEARCH_CALL_LOG;
pub(super) use context_still_search_client::{compact_result, parse_arguments};
pub use context_still_search_client::{
    is_search_tool, tool_definitions, ContextStillSearchClient, SearchError,
    MAX_CONTEXT_STILL_CALLS_PER_TURN, SEARCH_EPISODES_TOOL_NAME, SEARCH_KNOWLEDGE_TOOL_NAME,
};
#[cfg(test)]
#[path = "context_still_search/tests.rs"]
mod tests;
