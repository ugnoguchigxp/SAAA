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
#[path = "gateway/role_step_binding.rs"]
mod role_step_binding;
#[path = "gateway/authorize_routing_tool.rs"]
mod authorize_routing_tool;
pub use super::gateway_schemas::definitions;
pub use role_step_binding::{TOOL_SEARCH, TOOL_DESCRIBE, TOOL_INVOKE, internal_name, is_selection_tool, search_schema, describe_schema, invoke_schema, ok, error_envelope, dispatch, dispatch_external, append_tool_definitions, execute_for_persistence, execute_for_turn, RoleStepBinding, execute_for_role_root};
use role_step_binding::{routing_operation_key};
use authorize_routing_tool::{authorize_routing_tool, resolve_role_tool_effect, reserve_routing_operation, settle_routing_operation};
