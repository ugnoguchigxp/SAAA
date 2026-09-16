use rusqlite::params;

use crate::ipc_contract::ConversationMessage;
use crate::redact::bounded_text;
use crate::{
    database_error, new_id, now_iso, AppState, CleanupOutcome, ProviderFailureKind, StartTurnInput,
};

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
               fallback_used, output_started,
               release_status, status, started_at, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, 0, 0,
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
                    failure_kind.map(ProviderFailureKind::as_str),
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
                    failure_kind.map(ProviderFailureKind::as_str),
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

pub(crate) fn persist_conversation_success(
    state: &AppState,
    input: &StartTurnInput,
    content: &str,
) -> Result<ConversationMessage, String> {
    persist_conversation_success_with_state(state, input, content, |_| Ok(()))
}
pub(crate) fn persist_conversation_success_with_state(
    state: &AppState,
    input: &StartTurnInput,
    content: &str,
    adopt: impl FnOnce(&rusqlite::Connection) -> Result<(), String>,
) -> Result<ConversationMessage, String> {
    let fallback = if content.trim().is_empty() {
        state.sqlite_readers.read(|c| {
        c.query_row("SELECT json_extract(result_json,'$.summary') FROM ui_tool_results WHERE run_id=?1 AND json_extract(result_json,'$.summary') IS NOT NULL ORDER BY rowid DESC LIMIT 1", [&input.run_id], |r| r.get::<_,String>(0)).map_err(database_error)
    }).unwrap_or_default()
    } else {
        String::new()
    };
    let content = bounded_text(
        if content.trim().is_empty() {
            &fallback
        } else {
            content.trim()
        },
        64_000,
    );
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
        adopt(&transaction)?;
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
