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
#[path = "gateway/authorize_routing_tool.rs"]
mod authorize_routing_tool;
#[path = "gateway/role_step_binding.rs"]
mod role_step_binding;
pub use super::gateway_schemas::definitions;
use authorize_routing_tool::{
    authorize_routing_tool, reserve_routing_operation, resolve_role_tool_effect,
    settle_routing_operation,
};
use role_step_binding::routing_operation_key;
pub use role_step_binding::{
    append_tool_definitions, describe_schema, dispatch, dispatch_external, error_envelope,
    execute_for_persistence, execute_for_role_root, execute_for_turn, internal_name, invoke_schema,
    is_selection_tool, ok, search_schema, RoleStepBinding, TOOL_DESCRIBE, TOOL_INVOKE, TOOL_SEARCH,
};
