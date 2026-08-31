use serde_json::Value;
use std::time::Duration;

use crate::runtime::agent_tools;
use crate::StartTurnInput;

use super::attempt::*;
pub(crate) use super::recall_dispatch::execute_recall_tool;

pub(crate) fn available_agent_tools(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    calls_this_attempt: usize,
    voice_calls_this_attempt: usize,
) -> Vec<Value> {
    let non_voice_calls = calls_this_attempt.saturating_sub(voice_calls_this_attempt);
    let include_conversation = output_persistence.is_some_and(|persistence| {
        persistence
            .state
            .sqlite_readers
            .read(|connection| {
                crate::memory::recall::remaining_calls(connection, &input.run_id)
                    .map_err(|_| "Recall state unavailable".to_string())
            })
            .ok()
            .is_some_and(|remaining| remaining > 0)
    });
    let include_typed_memory = output_persistence
        .is_some_and(|persistence| persistence.state.context_still_recall.is_configured());
    let mut definitions = if non_voice_calls < crate::memory::contracts::MAX_RECALL_CALLS_PER_TURN {
        agent_tools::agent_tool_definitions(include_conversation, include_typed_memory, false)
    } else {
        Vec::new()
    };
    if voice_calls_this_attempt == 0 && output_persistence.is_some() {
        definitions.push(crate::voice_behavior::tool_definition());
    }
    definitions
}

pub(crate) fn tool_was_offered(definitions: &[Value], name: &str) -> bool {
    definitions.iter().any(|definition| {
        definition.pointer("/function/name").and_then(Value::as_str) == Some(name)
    })
}

pub(crate) async fn execute_agent_tool(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
    timeout: Duration,
) -> String {
    if call.name == crate::voice_behavior::UPDATE_VOICE_BEHAVIOR_TOOL_NAME {
        return crate::voice_behavior::execute_tool_for_state(
            output_persistence.map(|persistence| persistence.state),
            input,
            call,
        );
    }
    if crate::runtime::web_fetch::is_web_fetch_tool(&call.name) {
        return crate::runtime::web_fetch::execute(call, timeout).await;
    }
    if crate::runtime::agent_tools::is_typed_memory_tool(&call.name) {
        let Some(persistence) = output_persistence else {
            return crate::runtime::agent_tools::tool_error_content(
                "typed-memory-unavailable",
                "Typed memory recall is temporarily unavailable.",
            );
        };
        return match tokio::time::timeout(
            timeout,
            persistence
                .state
                .context_still_recall
                .recall(&call.name, &call.arguments),
        )
        .await
        {
            Ok(Ok(content)) => content,
            Ok(Err(error)) => crate::runtime::agent_tools::tool_error_content(
                error.tool_code(),
                error.safe_message(),
            ),
            Err(_) => crate::runtime::agent_tools::tool_error_content(
                "typed-memory-unavailable",
                "Typed memory recall is temporarily unavailable.",
            ),
        };
    }
    execute_recall_tool(output_persistence, input, call)
}
