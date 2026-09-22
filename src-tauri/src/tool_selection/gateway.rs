#![allow(private_interfaces)]
//! Conversation/MCP gateway contract for the three fixed entry points. Names stay
//! `tools_search`/`tools_describe`/`tools_invoke` for provider compatibility; internally they map
//! to `tools.search` etc. Every response is a fixed `{ok,data}` / `{ok,error}` envelope and no
//! internal error detail is ever returned to the model.

use super::contracts::*;
use super::service::{self, ToolSelectionService};
use crate::persistence::SqliteWriter;
use crate::runtime::agent_tools::AgentToolCall;
use crate::{AppState, RunCancellation};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
include!("gateway.d/01.rs");
include!("gateway.d/02.rs");
