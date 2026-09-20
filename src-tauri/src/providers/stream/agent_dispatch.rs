use serde_json::Value;
use std::time::Duration;

use crate::generated_capabilities::{
    publication::{GeneratedToolSnapshot, TOOL_PREFIX},
    tools,
};
use crate::runtime::agent_tools;
use crate::{RunCancellation, StartTurnInput};

use super::attempt::*;
pub(crate) use super::recall_dispatch::execute_recall_tool;
pub(crate) use crate::generated_capabilities::tools::AgentToolOffer;

pub(crate) fn available_agent_tools(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    calls_this_attempt: usize,
    voice_calls_this_attempt: usize,
) -> AgentToolOffer {
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
    if calls_this_attempt < 12
        && output_persistence.is_some_and(|p| {
            p.state
                .sqlite_readers
                .read(crate::generative_ui::store::enabled)
                .unwrap_or(false)
        })
    {
        definitions.extend(crate::generative_ui::tools::definitions());
    }
    if calls_this_attempt < 12
        && output_persistence.is_some_and(|p| {
            p.state
                .sqlite_readers
                .read(crate::coding::repository::enabled)
                .unwrap_or(false)
        })
    {
        definitions.extend(crate::coding::tools::definitions());
    }
    // Discovery mode replaces the generated `gc_` tool surface with the three selection entry
    // points; the legacy direct mode is left untouched. Both are never offered together.
    let discovery = output_persistence
        .is_some_and(|persistence| persistence.state.tool_selection.discovery_configured());
    // Generated tools share the existing coding/UI admission rule and are never offered on a
    // request without persisted output state (JsonProbe/tools=false are filtered by the caller).
    let generated = if calls_this_attempt < 12 && !discovery {
        output_persistence
            .map(|persistence| tools::append_generated(&mut definitions, persistence.state))
            .unwrap_or_default()
    } else {
        GeneratedToolSnapshot::empty()
    };
    if discovery && calls_this_attempt < 12 {
        crate::tool_selection::gateway::append_tool_definitions(
            &mut definitions,
            output_persistence,
        );
    }
    AgentToolOffer {
        definitions,
        generated,
    }
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
    generated: &GeneratedToolSnapshot,
    run_cancellation: &RunCancellation,
) -> String {
    // A `gc_` name is only ever executed from the snapshot that offered it; it never falls
    // through to recall or another tool.
    if call.name.starts_with(TOOL_PREFIX) {
        // M2A direct path: the host attaches the same actor context the discovery backend does.
        let actor = output_persistence.and_then(|persistence| {
            crate::tool_selection::service::ensure_principal(&persistence.state.sqlite_writer)
                .ok()
                .map(
                    |principal| crate::generated_capabilities::contracts::CallActor {
                        principal_id: principal,
                        conversation_id: input.conversation_id.clone(),
                        project_id: None,
                        run_id: input.run_id.clone(),
                    },
                )
        });
        return tools::execute_with_actor(
            output_persistence.map(|p| p.state.generated_capabilities.as_ref()),
            generated,
            call,
            "conversation",
            actor,
            timeout,
            run_cancellation,
        )
        .await;
    }
    if crate::tool_selection::gateway::is_selection_tool(&call.name) {
        return crate::tool_selection::gateway::execute_for_persistence(
            output_persistence,
            input,
            call,
            run_cancellation,
        )
        .await;
    }
    if crate::coding::contracts::NAMES.contains(&call.name.as_str()) {
        return crate::coding::tools::execute(output_persistence.map(|p| p.state), input, call);
    }
    if crate::generative_ui::tools::NAMES.contains(&call.name.as_str()) {
        return crate::generative_ui::tools::execute(
            output_persistence.map(|p| p.state),
            input,
            call,
        );
    }
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
