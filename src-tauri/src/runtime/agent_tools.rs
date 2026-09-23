use crate::memory::{
    contracts::{RecallConversationInput, RECALL_TOOL_NAME},
    typed_recall::{is_typed_recall_tool, typed_recall_tool_definitions, TYPED_RECALL_TOOL_NAMES},
};
use serde_json::{json, Value};
#[path = "agent_tools/agent_tool_call.rs"]
mod agent_tool_call;
pub use agent_tool_call::{
    agent_tool_definitions, context_still_call_key, is_context_still_tool, is_typed_memory_tool,
    parse_recall_arguments, recall_tool_definition, tool_error_content, AgentToolCall,
};
#[cfg(test)]
pub use agent_tool_call::{
    append_tool_exchange, is_supported_agent_tool, parse_non_stream_tool_call, ToolCallAccumulator,
    ToolProtocolError,
};
#[cfg(test)]
#[path = "agent_tools/tests.rs"]
mod tests;
