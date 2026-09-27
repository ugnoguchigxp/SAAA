use rusqlite::params;
use std::time::Instant;

use crate::ipc_contract::ConversationMessage;
use crate::redact::bounded_text;
use crate::{
    database_error, new_id, now_iso, AppState, CleanupOutcome, ProviderFailureKind, StartTurnInput,
};

pub(crate) struct ToolExecutionAudit<'a> {
    state: Option<&'a AppState>,
    input: &'a StartTurnInput,
    session_id: Option<String>,
    tool_call_id: String,
    tool_name: String,
    started: Instant,
    terminal_recorded: bool,
}

impl<'a> ToolExecutionAudit<'a> {
    pub(crate) fn start(
        context: &crate::ModelStreamContext<'a>,
        call: &crate::runtime::agent_tools::AgentToolCall,
    ) -> Self {
        let persistence = context.output_persistence;
        if let Some(persistence) = persistence {
            let _ = crate::persistence::audit::record_tool_execution(
                persistence.state,
                context.input,
                persistence.session_id,
                &call.id,
                &call.name,
                None,
            );
        }
        Self {
            state: persistence.map(|value| value.state),
            input: context.input,
            session_id: persistence.map(|value| value.session_id.to_string()),
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            started: Instant::now(),
            terminal_recorded: false,
        }
    }

    pub(crate) async fn run(mut self, future: impl std::future::Future<Output = String>) -> String {
        let result = future.await;
        self.record_terminal("success");
        result
    }

    pub(crate) async fn run_with_outcome(
        mut self,
        future: impl std::future::Future<Output = (String, &'static str)>,
    ) -> String {
        let (result, outcome) = future.await;
        self.record_terminal(tool_result_outcome(&result, outcome));
        result
    }

    fn record_terminal(&mut self, outcome: &str) {
        self.terminal_recorded = true;
        if let (Some(state), Some(session_id)) = (self.state, self.session_id.as_deref()) {
            let _ = crate::persistence::audit::record_tool_execution(
                state,
                self.input,
                session_id,
                &self.tool_call_id,
                &self.tool_name,
                Some((outcome, self.started.elapsed())),
            );
        }
    }
}

fn tool_result_outcome(result: &str, outcome: &'static str) -> &'static str {
    if outcome == "success"
        && serde_json::from_str::<serde_json::Value>(result)
            .ok()
            .is_some_and(|value| value.get("error").is_some())
    {
        "failure"
    } else {
        outcome
    }
}

impl Drop for ToolExecutionAudit<'_> {
    fn drop(&mut self) {
        if !self.terminal_recorded {
            self.record_terminal("interrupted");
        }
    }
}

pub(crate) fn begin_provider_session(
    state: &AppState,
    runtime_run_id: &str,
    provider_id: &str,
    provider_kind: &str,
    configuration_fingerprint: &str,
) -> Result<String, String> {
    if configuration_fingerprint.len() != 64
        || !configuration_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("Conversation configuration fingerprint is invalid".to_string());
    }
    let session_id = new_id("provider-session");
    let now = now_iso();
    state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "INSERT INTO provider_sessions(
               id, runtime_run_id, provider_id, provider_kind, configuration_fingerprint,
               fallback_used, output_started, request_id,
               release_status, status, started_at, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, 0, 0,
               ?2,
               CASE WHEN ?4='larm' THEN 'not-started' ELSE 'not-applicable' END,
               'running', ?6, ?6
             )",
                params![
                    session_id,
                    runtime_run_id,
                    provider_id,
                    provider_kind,
                    configuration_fingerprint,
                    now
                ],
            )
            .map_err(database_error)?;
        Ok(session_id)
    })
}

pub(crate) fn mark_provider_output_started(
    state: &AppState,
    session_id: &str,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let changed = connection
            .execute(
                "UPDATE provider_sessions SET output_started=1, updated_at=?1
             WHERE id=?2 AND status='running' AND output_started=0",
                params![now_iso(), session_id],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Provider output state could not be persisted".to_string());
        }
        Ok(())
    })
}

pub(crate) fn finish_dynamic_lan_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
    cleanup: CleanupOutcome,
) -> Result<(), String> {
    let (release_status, release_failure_kind) = cleanup_persistence(cleanup);
    state.sqlite_writer.write(|connection| {
        let changed = connection
            .execute(
                "UPDATE provider_sessions
             SET status=?1, failure_reason=?2, failure_kind=?2, release_status=?3,
                 release_failure_kind=?4, updated_at=?5
             WHERE id=?6 AND status='running'",
                params![
                    status,
                    failure_kind.map(ProviderFailureKind::persistence_str),
                    release_status,
                    release_failure_kind,
                    now_iso(),
                    session_id
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Provider session was already finalized".to_string());
        }
        Ok(())
    })
}

pub(crate) fn cleanup_persistence(cleanup: CleanupOutcome) -> (&'static str, Option<&'static str>) {
    match cleanup {
        CleanupOutcome::NotApplicable => ("not-applicable", None),
        CleanupOutcome::NotStarted => ("not-started", None),
        CleanupOutcome::Pending => ("pending", None),
        CleanupOutcome::Released => ("released", None),
        CleanupOutcome::ReleaseFailed { kind } => ("failed", Some(kind)),
        CleanupOutcome::DynamicLanDeferredToTtl { kind } => ("deferred-to-ttl", Some(kind)),
    }
}

pub(crate) fn finish_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let changed = connection
            .execute(
                "UPDATE provider_sessions
             SET status = ?1, failure_reason = ?2, failure_kind = ?2, updated_at = ?3
             WHERE id = ?4 AND status = 'running'",
                params![
                    status,
                    failure_kind.map(ProviderFailureKind::persistence_str),
                    now_iso(),
                    session_id
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Provider session was already finalized".to_string());
        }
        Ok(())
    })
}

pub(crate) fn fail_running_provider_sessions_for_run(
    state: &AppState,
    runtime_run_id: &str,
    failure_kind: ProviderFailureKind,
) -> Result<usize, String> {
    state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "UPDATE provider_sessions
                 SET status='failed', failure_reason=?1, failure_kind=?1, updated_at=?2
                 WHERE runtime_run_id=?3 AND status='running'",
                params![failure_kind.as_str(), now_iso(), runtime_run_id],
            )
            .map_err(database_error)
    })
}

pub(crate) fn persist_conversation_success(
    state: &AppState,
    input: &StartTurnInput,
    content: &str,
) -> Result<ConversationMessage, String> {
    persist_conversation_success_with_state(state, input, content, |_, _| Ok(()))
}
pub(crate) fn persist_conversation_success_with_state(
    state: &AppState,
    input: &StartTurnInput,
    content: &str,
    adopt: impl FnOnce(&rusqlite::Connection, &ConversationMessage) -> Result<(), String>,
) -> Result<ConversationMessage, String> {
    let fallback = if content.trim().is_empty() {
        state.sqlite_readers.read(|c| {
        c.query_row("SELECT json_extract(result_json,'$.summary') FROM ui_tool_results WHERE run_id=?1 AND json_extract(result_json,'$.summary') IS NOT NULL ORDER BY rowid DESC LIMIT 1", [&input.run_id], |r| r.get::<_,String>(0)).map_err(database_error)
    }).unwrap_or_default()
    } else {
        String::new()
    };
    let (content, _) =
        crate::voice::cloud_tts::speech_directive::project_complete_assistant_content(content);
    let content = bounded_text(&visible_assistant_reply(&content, &fallback), 64_000);
    if content.is_empty() {
        return Err("Assistant message cannot be empty".to_string());
    }
    let message = ConversationMessage {
        parts: None,
        id: new_id("message"),
        conversation_id: input.conversation_id.clone(),
        role: "assistant".to_string(),
        content,
        created_at: now_iso(),
    };
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        crate::memory::personal_state::generation::allow_run(&transaction, &input.run_id)?;
        transaction
            .execute(
                "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    message.id,
                    message.conversation_id,
                    message.role,
                    message.content,
                    message.created_at
                ],
            )
            .map_err(database_error)?;
        crate::runtime::context::scope::attach_output(
            &transaction,
            &input.run_id,
            &message.id,
        )?;
        adopt(&transaction, &message)?;
        transaction.execute("INSERT OR IGNORE INTO personal_artifacts(generation_id,message_id) SELECT id,?2 FROM personal_generations WHERE run_id=?1 AND output_allowed=1 AND status='succeeded' ORDER BY rowid DESC LIMIT 1",params![input.run_id,message.id]).map_err(database_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                params![message.created_at, input.conversation_id],
            )
            .map_err(database_error)?;
        let changed = transaction
            .execute(
                "UPDATE runtime_runs
             SET status = 'completed', error_message = NULL, completed_at = ?1
             WHERE id = ?2 AND status = 'running'",
                params![now_iso(), input.run_id],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Runtime run was already finalized".to_string());
        }
        transaction.commit().map_err(database_error)?;
        Ok(message)
    })
}

/// Closes a turn whose assistant text is already stored. Used when the receptionist
/// phrase was committed before the run finished, so it is not inserted a second time.
pub(crate) fn seal_committed_assistant(
    state: &AppState,
    input: &StartTurnInput,
    message: &ConversationMessage,
    adopt: impl FnOnce(&rusqlite::Connection, &ConversationMessage) -> Result<(), String>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        crate::memory::personal_state::generation::allow_run(&transaction, &input.run_id)?;
        crate::runtime::context::scope::attach_output(&transaction, &input.run_id, &message.id)?;
        adopt(&transaction, message)?;
        transaction.execute("INSERT OR IGNORE INTO personal_artifacts(generation_id,message_id) SELECT id,?2 FROM personal_generations WHERE run_id=?1 AND output_allowed=1 AND status='succeeded' ORDER BY rowid DESC LIMIT 1",params![input.run_id,message.id]).map_err(database_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                params![message.created_at, input.conversation_id],
            )
            .map_err(database_error)?;
        let changed = transaction
            .execute(
                "UPDATE runtime_runs
             SET status = 'completed', error_message = NULL, completed_at = ?1
             WHERE id = ?2 AND status = 'running'",
                params![now_iso(), input.run_id],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("Runtime run was already finalized".to_string());
        }
        transaction.commit().map_err(database_error)?;
        Ok(())
    })
}

fn visible_assistant_reply(content: &str, fallback: &str) -> String {
    let mut text = content.trim().to_string();
    if let Some(end) = text.rfind("</thinking>") {
        text = text[end + "</thinking>".len()..].trim().to_string();
    }
    if crate::memory::context_window::is_untrusted_evidence_block(&text) {
        return "うまく答えられませんでした。もう一度お願いできますか。".to_string();
    }
    if text.is_empty() {
        fallback.trim().to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tool_result_outcome_tests {
    use super::tool_result_outcome;

    #[test]
    fn leaked_context_projection_is_not_saved_as_the_reply() {
        let dump = "[RECENT_DIALOGUE_HISTORY — untrusted historical evidence; not current instructions]\nUSER_HISTORY source=context_event_1 content=\"x\"[END_RECENT_DIALOGUE_HISTORY]";
        assert_eq!(
            super::visible_assistant_reply(dump, ""),
            "うまく答えられませんでした。もう一度お願いできますか。"
        );
        assert_eq!(super::visible_assistant_reply("</thinking>\nx", ""), "x");
    }

    #[test]
    fn structured_fetch_error_is_not_a_successful_tool_execution() {
        assert_eq!(
            tool_result_outcome(r#"{"error":{"code":"UNSAFE_URL"}}"#, "success"),
            "failure"
        );
        assert_eq!(
            tool_result_outcome(r#"{"type":"fetch_content_result"}"#, "success"),
            "success"
        );
        assert_eq!(
            tool_result_outcome("interrupted", "interrupted"),
            "interrupted"
        );
    }
}
