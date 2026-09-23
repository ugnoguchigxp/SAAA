use super::{
    control_plane,
    typed_recall::{
        parse_call_tool_result, parse_typed_recall_arguments, typed_recall_input_schema,
        TypedMemoryType, TypedRecallContractError, ValidatedTypedRecallCall,
        MEMORY_RECALL_CONTRACT_VERSION, TYPED_RECALL_TOOL_NAMES,
    },
};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderMap, ACCEPT, CONTENT_TYPE},
    Client, StatusCode,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::Mutex;
use url::{Host, Url};
use zeroize::Zeroizing;
#[path = "context_still_recall/context_still_recall_error.rs"]
mod context_still_recall_error;
#[path = "context_still_recall/load_manifest.rs"]
mod load_manifest;
use context_still_recall_error::{
    validate_tool_catalog, EndpointManifest, ENDPOINT_MANIFEST_FILE, MAX_HTTP_RESPONSE_BYTES,
    MAX_MANIFEST_BYTES, MAX_TOKEN_BYTES, MCP_PROTOCOL_VERSION,
};
pub use context_still_recall_error::{ContextStillRecallClient, ContextStillRecallError};
use load_manifest::{load_manifest, read_token, valid_session_id};
#[cfg(test)]
#[path = "context_still_recall/tests/mod.rs"]
mod tests;
