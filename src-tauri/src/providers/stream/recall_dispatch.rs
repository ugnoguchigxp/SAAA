use super::attempt::ProviderOutputPersistence;
use crate::StartTurnInput;

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
    persistence
        .state
        .sqlite_writer
        .write(|connection| {
            let context = crate::memory::recall::RecallExecutionContext {
                runtime_run_id: &input.run_id,
                tool_call_id: &call.id,
                now: chrono::Utc::now(),
                timezone: crate::memory::recall::system_timezone(),
            };
            let arguments =
                match crate::runtime::agent_tools::parse_recall_arguments(&call.arguments) {
                    Ok(arguments) => arguments,
                    Err(()) => {
                        return Ok(
                            match crate::memory::recall::record_failed_attempt(connection, &context)
                            {
                                Ok(()) => crate::runtime::agent_tools::tool_error_content(
                                    "invalid-input",
                                    "Tool arguments do not match the recall_conversation schema.",
                                ),
                                Err(error) => crate::runtime::agent_tools::tool_error_content(
                                    error.code.as_str(),
                                    error.message,
                                ),
                            },
                        );
                    }
                };
            Ok(
                match crate::memory::recall::execute(connection, context, arguments) {
                    Ok(output) => serde_json::to_string(&output).unwrap_or_else(|_| {
                        crate::runtime::agent_tools::tool_error_content(
                            "local-recall-unavailable",
                            "The conversation recall result could not be encoded.",
                        )
                    }),
                    Err(error) => crate::runtime::agent_tools::tool_error_content(
                        error.code.as_str(),
                        error.message,
                    ),
                },
            )
        })
        .unwrap_or_else(|_| {
            crate::runtime::agent_tools::tool_error_content(
                "local-recall-unavailable",
                "Local conversation recall is temporarily unavailable.",
            )
        })
}
