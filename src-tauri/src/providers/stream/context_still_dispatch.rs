use super::*;
pub(super) async fn execute(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
    timeout: Duration,
) -> String {
    if call.name == crate::memory::personal_state::sources::episode_export::SOURCE_TOOL {
        let Some(persistence) = output_persistence else {
            return agent_tools::tool_error_content(
                "episode-source-unavailable",
                "原記録を確認できません。",
            );
        };
        return persistence
            .state
            .sqlite_writer
            .transact(|c| {
                crate::memory::personal_state::sources::episode_export::fetch_source(
                    c,
                    &input.run_id,
                    &call.arguments,
                )
            })
            .unwrap_or_else(|_| {
                agent_tools::tool_error_content(
                    "episode-source-unavailable",
                    "許可された最新の原記録を確認できません。",
                )
            });
    }
    if crate::memory::context_still_search::is_search_tool(&call.name) {
        let Some(persistence) = output_persistence else {
            return crate::runtime::agent_tools::tool_error_content(
                "context-still-unavailable",
                "ContextStill search is temporarily unavailable.",
            );
        };
        let scopes = persistence
            .state
            .sqlite_readers
            .read(|c| {
                crate::runtime::context::scope::load(c, &input.run_id)
                    .map(|s| s.scopes.into_iter().map(|s| s.key).collect::<Vec<_>>())
            })
            .unwrap_or_default();
        return match tokio::time::timeout(
            timeout,
            persistence.state.context_still_search.search_scoped(
                &call.name,
                &call.arguments,
                input.workspace_path.as_deref(),
                &scopes,
            ),
        )
        .await
        {
            Ok(Ok(content)) => match persistence.state.sqlite_writer.transact(|c| {
                crate::memory::personal_state::sources::episode_export::capture(
                    c,
                    &input.run_id,
                    &content,
                    &scopes,
                )
            }) {
                Ok(()) => content,
                Err(_) => crate::runtime::agent_tools::tool_error_content(
                    "episode-source-unavailable",
                    "Episode evidence changed or is outside the authorized scope.",
                ),
            },
            Ok(Err(error)) => crate::runtime::agent_tools::tool_error_content(
                error.tool_code(),
                error.safe_message(),
            ),
            Err(_) => crate::runtime::agent_tools::tool_error_content(
                "context-still-unavailable",
                "ContextStill search is temporarily unavailable.",
            ),
        };
    }
    agent_tools::tool_error_content("context-still-unavailable", "参照資料を確認できません。")
}
