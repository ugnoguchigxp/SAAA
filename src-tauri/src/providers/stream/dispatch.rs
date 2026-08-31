use serde_json::Value;
use std::time::Duration;

use crate::runtime::agent_tools;
use crate::StartTurnInput;

use super::attempt::*;

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
            .connection
            .lock()
            .ok()
            .and_then(|connection| {
                crate::memory::recall::remaining_calls(&connection, &input.run_id).ok()
            })
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

pub(crate) fn attach_agent_tools(body: &mut Value, tools: &[Value]) {
    if tools.is_empty() {
        return;
    }
    body["tools"] = Value::Array(tools.to_vec());
    body["tool_choice"] = serde_json::json!("auto");
    body["parallel_tool_calls"] = serde_json::json!(false);
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

pub(crate) fn execute_recall_tool(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
) -> String {
    let Some(persistence) = output_persistence else {
        return crate::runtime::agent_tools::tool_error_content(
            "local-recall-unavailable",
            "Local conversation recall is unavailable for this request.",
        );
    };
    let mut connection = match persistence.state.connection.lock() {
        Ok(connection) => connection,
        Err(_) => {
            return crate::runtime::agent_tools::tool_error_content(
                "local-recall-unavailable",
                "Local conversation recall is temporarily unavailable.",
            );
        }
    };
    let context = crate::memory::recall::RecallExecutionContext {
        runtime_run_id: &input.run_id,
        tool_call_id: &call.id,
        now: chrono::Utc::now(),
        timezone: crate::memory::recall::system_timezone(),
    };
    let arguments = match crate::runtime::agent_tools::parse_recall_arguments(&call.arguments) {
        Ok(arguments) => arguments,
        Err(()) => {
            return match crate::memory::recall::record_failed_attempt(&mut connection, &context) {
                Ok(()) => crate::runtime::agent_tools::tool_error_content(
                    "invalid-input",
                    "Tool arguments do not match the recall_conversation schema.",
                ),
                Err(error) => crate::runtime::agent_tools::tool_error_content(
                    error.code.as_str(),
                    error.message,
                ),
            };
        }
    };
    match crate::memory::recall::execute(&mut connection, context, arguments) {
        Ok(output) => serde_json::to_string(&output).unwrap_or_else(|_| {
            crate::runtime::agent_tools::tool_error_content(
                "local-recall-unavailable",
                "The conversation recall result could not be encoded.",
            )
        }),
        Err(error) => {
            crate::runtime::agent_tools::tool_error_content(error.code.as_str(), error.message)
        }
    }
}
