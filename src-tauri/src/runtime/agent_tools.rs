use crate::memory::{
    contracts::{RecallConversationInput, RECALL_TOOL_NAME},
    typed_recall::{is_typed_recall_tool, typed_recall_tool_definitions},
};
use serde_json::{json, Value};
include!("agent_tools.d/01.rs");
include!("agent_tools.d/02.rs");
