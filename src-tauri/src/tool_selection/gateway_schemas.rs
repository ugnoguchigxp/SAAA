//! OpenAI Chat Completions definitions for the three fixed entry points. Split from `gateway.rs`
//! so the dispatch module stays close to its pre-D4 size. The MCP layer (D4/D5) reuses the same
//! neutral schemas and only changes the transport wrapper.

use serde_json::{json, Value};

use super::gateway::{
    describe_schema, invoke_schema, search_schema, TOOL_DESCRIBE, TOOL_INVOKE, TOOL_SEARCH,
};

pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": TOOL_SEARCH,
                "description": "Search the tool ledger for tools that match the stated intent. \
                                Returns a small number of candidates; a candidate is not a decision to run.",
                "parameters": search_schema(),
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": TOOL_DESCRIBE,
                "description": "Fetch the contract or usage of one candidate returned by tools_search, \
                                or one continuation page of a stored large result.",
                "parameters": describe_schema(),
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": TOOL_INVOKE,
                "description": "Execute one previously described tool with the given arguments.",
                "parameters": invoke_schema(),
            }
        }),
    ]
}
