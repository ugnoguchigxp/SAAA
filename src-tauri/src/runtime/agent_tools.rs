use crate::memory::{
    contracts::{RecallConversationInput, RECALL_TOOL_NAME},
    typed_recall::{is_typed_recall_tool, typed_recall_tool_definitions, TYPED_RECALL_TOOL_NAMES},
};
use serde_json::{json, Value};
#[path = "agent_tools/agent_tool_call.rs"]
mod agent_tool_call;
pub use agent_tool_call::{AgentToolCall, parse_recall_arguments, recall_tool_definition, agent_tool_definitions, is_typed_memory_tool, is_context_still_tool, context_still_call_key, tool_error_content};
#[cfg(test)]
pub use agent_tool_call::{ToolProtocolError, ToolCallAccumulator, parse_non_stream_tool_call, is_supported_agent_tool, append_tool_exchange};
#[cfg(test)]
#[path = "agent_tools/tests.rs"]
mod tests;
