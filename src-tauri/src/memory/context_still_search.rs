use crate::tool_selection::mcp::transport::{HttpTransport, TransportError};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use url::{Host, Url};
include!("context_still_search.d/01.rs");
include!("context_still_search.d/02.rs");
